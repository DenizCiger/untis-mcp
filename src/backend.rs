//! Data access for the MCP tools: either a live WebUntis account or the bundled
//! demo data, with short-lived caches so agents can make many calls cheaply.

use crate::demo;
use crate::models::{
    Config, ParsedAbsence, SchoolYear, TimetableSearchItem, TimetableTarget, WeekTimetable,
    get_monday, select_school_year, today_local,
};
use crate::webuntis::{WebUntisClient, WebUntisError};
use chrono::NaiveDate;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const DIRECTORY_TTL: Duration = Duration::from_secs(60 * 60);
const TIMETABLE_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_CACHED_WEEKS: usize = 1024;

enum Source {
    Live(Box<WebUntisClient>),
    Demo,
}

struct Cached<T> {
    fetched_at: Instant,
    value: Arc<T>,
}

impl<T> Cached<T> {
    fn new(value: T) -> Self {
        Self {
            fetched_at: Instant::now(),
            value: Arc::new(value),
        }
    }

    fn fresh(&self, ttl: Duration) -> Option<Arc<T>> {
        (self.fetched_at.elapsed() < ttl).then(|| self.value.clone())
    }
}

pub struct Backend {
    source: Source,
    config: Config,
    school_years: Mutex<Option<Cached<Vec<SchoolYear>>>>,
    search_index: Mutex<Option<Cached<Vec<TimetableSearchItem>>>>,
    weeks: Mutex<HashMap<(String, NaiveDate), Cached<WeekTimetable>>>,
}

impl Backend {
    pub fn live(config: Config) -> Result<Self, WebUntisError> {
        Ok(Self::with_source(
            Source::Live(Box::new(WebUntisClient::new(config.clone())?)),
            config,
        ))
    }

    pub fn demo() -> Self {
        Self::with_source(Source::Demo, demo::demo_config())
    }

    fn with_source(source: Source, config: Config) -> Self {
        Self {
            source,
            config,
            school_years: Mutex::new(None),
            search_index: Mutex::new(None),
            weeks: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_demo(&self) -> bool {
        matches!(self.source, Source::Demo)
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn verify_login(&self) -> Result<(), WebUntisError> {
        match &self.source {
            Source::Live(client) => client.verify_login().await,
            Source::Demo => Ok(()),
        }
    }

    pub async fn close(&self) {
        if let Source::Live(client) = &self.source {
            client.close().await;
        }
    }

    pub async fn school_years(&self) -> Result<Arc<Vec<SchoolYear>>, WebUntisError> {
        let mut guard = self.school_years.lock().await;
        if let Some(value) = guard
            .as_ref()
            .and_then(|cached| cached.fresh(DIRECTORY_TTL))
        {
            return Ok(value);
        }
        let years = match &self.source {
            Source::Live(client) => client.fetch_school_years().await?,
            Source::Demo => demo::demo_school_years(),
        };
        let cached = Cached::new(years);
        let value = cached.value.clone();
        *guard = Some(cached);
        Ok(value)
    }

    /// The school year containing `date`, or the closest one if none does.
    pub async fn school_year_for(&self, date: NaiveDate) -> Result<SchoolYear, WebUntisError> {
        let years = self.school_years().await?;
        select_school_year(&years, date)
            .cloned()
            .ok_or_else(|| WebUntisError::Message("WebUntis returned no school years".to_owned()))
    }

    pub async fn search_index(&self) -> Result<Arc<Vec<TimetableSearchItem>>, WebUntisError> {
        let mut guard = self.search_index.lock().await;
        if let Some(value) = guard
            .as_ref()
            .and_then(|cached| cached.fresh(DIRECTORY_TTL))
        {
            return Ok(value);
        }
        let items = match &self.source {
            Source::Live(client) => {
                let school_year_id = self.school_year_for(today_local()).await.ok().map(|y| y.id);
                client.fetch_timetable_search_index(school_year_id).await?
            }
            Source::Demo => demo::demo_search_index(),
        };
        let cached = Cached::new(items);
        let value = cached.value.clone();
        *guard = Some(cached);
        Ok(value)
    }

    pub async fn week_timetable(
        &self,
        date: NaiveDate,
        target: &TimetableTarget,
    ) -> Result<Arc<WeekTimetable>, WebUntisError> {
        let key = (target_cache_key(target), get_monday(date));
        if let Some(value) = self
            .weeks
            .lock()
            .await
            .get(&key)
            .and_then(|cached| cached.fresh(TIMETABLE_TTL))
        {
            return Ok(value);
        }

        // Fetched without holding the lock so room scans can run in parallel.
        let week = match &self.source {
            Source::Live(client) => client.fetch_week_timetable(date, target).await?,
            Source::Demo => demo::demo_week_timetable(date, target),
        };
        let cached = Cached::new(week);
        let value = cached.value.clone();
        let mut weeks = self.weeks.lock().await;
        if weeks.len() >= MAX_CACHED_WEEKS {
            weeks.retain(|_, cached| cached.fresh(TIMETABLE_TTL).is_some());
        }
        weeks.insert(key, cached);
        Ok(value)
    }

    pub async fn absences(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<ParsedAbsence>, WebUntisError> {
        match &self.source {
            Source::Live(client) => client.fetch_absences_for_range(start, end).await,
            Source::Demo => Ok(demo::demo_absences()
                .into_iter()
                .filter(|absence| absence.end_date >= start && absence.start_date <= end)
                .collect()),
        }
    }
}

fn target_cache_key(target: &TimetableTarget) -> String {
    match target {
        TimetableTarget::Own => "own".to_owned(),
        TimetableTarget::Class { id, .. } => format!("class:{id}"),
        TimetableTarget::Room { id, .. } => format!("room:{id}"),
        TimetableTarget::Teacher { id, .. } => format!("teacher:{id}"),
    }
}
