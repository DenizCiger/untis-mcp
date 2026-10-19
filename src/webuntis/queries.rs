use super::api::{RawTeacher, RawTimeGridDay};
use super::client::{WebUntisClient, WebUntisError};
use super::{absences, search, timetable};
use crate::models::{
    ParsedAbsence, SchoolYear, TimetableRequestTarget, TimetableSearchItem, TimetableTarget,
    WeekTimetable, resolve_timetable_request,
};
use chrono::NaiveDate;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Teachers and the timegrid rarely change, but every timetable request needs
/// them, so they are cached instead of being refetched per week or per room.
const MASTER_DATA_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
pub(super) struct MasterData {
    fetched_at: Instant,
    teachers: Arc<Vec<RawTeacher>>,
    /// `None` when the school has no active timegrid; derived per week instead.
    timegrid: Option<Arc<Vec<RawTimeGridDay>>>,
}

impl WebUntisClient {
    pub async fn fetch_school_years(&self) -> Result<Vec<SchoolYear>, WebUntisError> {
        self.with_session(|session| async move { self.get_schoolyears(&session).await })
            .await
    }

    pub async fn fetch_timetable_search_index(
        &self,
        school_year_id: Option<i64>,
    ) -> Result<Vec<TimetableSearchItem>, WebUntisError> {
        self.with_session(|session| async move {
            let teachers = optional(self.get_teachers(&session).await, "teachers")?;
            let rooms = optional(self.get_rooms(&session).await, "rooms")?;
            let classes = match self.get_classes(&session, school_year_id).await {
                Err(WebUntisError::Message(_)) if school_year_id.is_some() => {
                    self.get_classes(&session, None).await
                }
                result => result,
            };
            let classes = optional(classes, "classes")?;
            if teachers.is_empty() && rooms.is_empty() && classes.is_empty() {
                return Err(WebUntisError::Message(
                    "WebUntis returned no classes, rooms or teachers for this account".to_owned(),
                ));
            }
            Ok(search::normalize_search_items(
                search::map_classes_to_search_items(&classes)
                    .into_iter()
                    .chain(search::map_rooms_to_search_items(&rooms))
                    .chain(search::map_teachers_to_search_items(&teachers))
                    .collect(),
            ))
        })
        .await
    }

    pub async fn fetch_week_timetable(
        &self,
        week_date: NaiveDate,
        target: &TimetableTarget,
    ) -> Result<WeekTimetable, WebUntisError> {
        let master = self.master_data().await?;
        self.with_session(|session| {
            let master = master.clone();
            async move {
                let (element_id, element_type) = match resolve_timetable_request(target) {
                    TimetableRequestTarget::Own => (session.person_id, session.person_type),
                    TimetableRequestTarget::Target { id, element_type } => {
                        (id, element_type as i64)
                    }
                };
                let weekly = self
                    .get_weekly_timetable(&session, week_date, element_id, element_type)
                    .await?;
                let timegrid = match &master.timegrid {
                    Some(timegrid) => timegrid.as_ref().clone(),
                    None => timetable::derive_timegrid_from_weekly(&weekly),
                };
                Ok(timetable::build_week_timetable(
                    week_date,
                    element_id,
                    weekly,
                    timegrid,
                    &master.teachers,
                ))
            }
        })
        .await
    }

    pub async fn fetch_absences_for_range(
        &self,
        range_start: NaiveDate,
        range_end: NaiveDate,
    ) -> Result<Vec<ParsedAbsence>, WebUntisError> {
        self.with_session(|session| async move {
            let payload = self.get_absences(&session, range_start, range_end).await?;
            Ok(absences::map_absence_payload(&self.config, payload))
        })
        .await
    }

    async fn master_data(&self) -> Result<MasterData, WebUntisError> {
        let mut guard = self.master_data.lock().await;
        if let Some(cached) = guard.as_ref()
            && cached.fetched_at.elapsed() < MASTER_DATA_TTL
        {
            return Ok(cached.clone());
        }

        let fresh = self
            .with_session(|session| async move {
                let teachers = optional(self.get_teachers(&session).await, "teachers")?;
                let timegrid = match self.get_timegrid(&session).await {
                    Ok(timegrid) if timegrid.iter().any(|day| !day.time_units.is_empty()) => {
                        Some(Arc::new(timegrid))
                    }
                    Err(WebUntisError::Unauthorized(reason)) => {
                        return Err(WebUntisError::Unauthorized(reason));
                    }
                    _ => None,
                };
                Ok(MasterData {
                    fetched_at: Instant::now(),
                    teachers: Arc::new(teachers),
                    timegrid,
                })
            })
            .await?;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }
}

/// Treats a permission or lookup failure as "no data" so that one restricted
/// list does not fail the whole request. Session failures still propagate.
fn optional<T>(result: Result<Vec<T>, WebUntisError>, what: &str) -> Result<Vec<T>, WebUntisError> {
    match result {
        Ok(items) => Ok(items),
        Err(WebUntisError::Message(reason)) => {
            tracing::warn!(%reason, "could not load {what}, continuing without them");
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}
