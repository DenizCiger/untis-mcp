mod input;
mod output;

use crate::backend::Backend;
use crate::models::{
    TimetableSearchItem, TimetableTarget, add_days, get_monday, get_weekday_name,
    parse_time_to_minutes, today_local,
};
use crate::webuntis::{WebUntisError, search_timetable_targets};
use chrono::NaiveDate;
use futures::stream::{self, StreamExt};
use input::{
    AbsenceParams, ChangesParams, DayParams, FreeRoomsParams, SearchParams, TargetParams,
    TargetType, WeekParams, now_minutes, parse_clock, parse_date, resolve_period,
};
use output::{
    Absence, AbsencesResult, ChangedLesson, ChangesResult, ContextResult, Day, DayTimetableResult,
    FreeRoomsResult, Lesson, Period, SchoolYearItem, SchoolYearsResult, SearchResult,
    SearchResultItem, Target, WeekTimetableResult, date_string,
};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use std::sync::Arc;

const MAX_CHANGES_RANGE_DAYS: i64 = 62;
const MAX_SEARCH_LIMIT: usize = 100;
const ROOM_SCAN_CONCURRENCY: usize = 6;
const MAX_UNKNOWN_ROOMS_LISTED: usize = 20;

const INSTRUCTIONS: &str = "Read-only access to a WebUntis school account: timetables of the \
user, any class, room or teacher; timetable changes and cancellations; free rooms; absences and \
school years. Call get_context first if you need today's date or the account's school. \
Timetable tools default to the user's own timetable; to read another one pass target_type plus \
target_name (or target_id from search_timetable_targets). Dates accept YYYY-MM-DD or \
today/tomorrow/yesterday.";

#[derive(Clone)]
pub struct UntisServer {
    backend: Arc<Backend>,
    tool_router: ToolRouter<Self>,
}

fn upstream(error: WebUntisError) -> String {
    format!("WebUntis request failed: {error}")
}

#[tool_router]
impl UntisServer {
    pub fn new(backend: Arc<Backend>) -> Self {
        Self {
            backend,
            tool_router: Self::tool_router(),
        }
    }

    /// Get today's date, current time, the connected school/account, and the current school
    /// year. Use this to anchor relative dates.
    #[tool(annotations(title = "Get context", read_only_hint = true, open_world_hint = false))]
    async fn get_context(&self) -> Result<Json<ContextResult>, String> {
        let today = today_local();
        let current_school_year = self
            .backend
            .school_year_for(today)
            .await
            .ok()
            .map(|year| SchoolYearItem::new(&year, Some(year.id)));
        let config = self.backend.config();
        Ok(Json(ContextResult {
            today: date_string(today),
            weekday: get_weekday_name(today),
            time: minutes_to_clock(now_minutes()),
            server: config.server.clone(),
            school: config.school.clone(),
            username: config.username.clone(),
            current_school_year,
            demo: self.backend.is_demo(),
        }))
    }

    /// List the school years known to WebUntis, newest first, marking the current one.
    #[tool(annotations(
        title = "List school years",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_school_years(&self) -> Result<Json<SchoolYearsResult>, String> {
        let years = self.backend.school_years().await.map_err(upstream)?;
        let current = self.backend.school_year_for(today_local()).await.ok();
        Ok(Json(SchoolYearsResult {
            school_years: years
                .iter()
                .map(|year| SchoolYearItem::new(year, current.as_ref().map(|y| y.id)))
                .collect(),
        }))
    }

    /// Search classes, rooms and teachers by name (fuzzy: prefixes, initials and small typos
    /// work). Returns ids usable as target_id in the timetable tools.
    #[tool(annotations(
        title = "Search classes, rooms, teachers",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn search_timetable_targets(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<SearchResult>, String> {
        let index = self.backend.search_index().await.map_err(upstream)?;
        let candidates = filter_kind(&index, params.target_type);
        let matches = search_timetable_targets(&candidates, &params.query, None);
        let limit = params.limit.unwrap_or(10).clamp(1, MAX_SEARCH_LIMIT);
        Ok(Json(SearchResult {
            total_matches: matches.len(),
            results: matches
                .iter()
                .take(limit)
                .map(SearchResultItem::from)
                .collect(),
        }))
    }

    /// Get a full week's timetable (Monday to Friday, plus weekend days that have lessons) for
    /// the user, a class, a room or a teacher, including the school's period grid.
    #[tool(annotations(
        title = "Get week timetable",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_week_timetable(
        &self,
        Parameters(params): Parameters<WeekParams>,
    ) -> Result<Json<WeekTimetableResult>, String> {
        let date = parse_date(params.date.as_deref(), "date")?.unwrap_or_else(today_local);
        let target = self.resolve_target(&params.target).await?;
        let week = self
            .backend
            .week_timetable(date, &target)
            .await
            .map_err(upstream)?;
        Ok(Json(WeekTimetableResult {
            target: Target::from(&target),
            week_start: date_string(get_monday(date)),
            days: week
                .days
                .iter()
                .map(|day| Day {
                    date: date_string(day.date),
                    weekday: day.day_name.clone(),
                    lessons: day
                        .lessons
                        .iter()
                        .map(|lesson| Lesson::from_parsed(lesson, &week.timegrid))
                        .collect(),
                })
                .collect(),
            timegrid: week.timegrid.iter().map(Period::from).collect(),
        }))
    }

    /// Get the lessons of a single day (default today) for the user, a class, a room or a
    /// teacher. Each lesson includes its period, status (regular/cancelled/substitution/...),
    /// teachers, room and remarks.
    #[tool(annotations(
        title = "Get day timetable",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_day_timetable(
        &self,
        Parameters(params): Parameters<DayParams>,
    ) -> Result<Json<DayTimetableResult>, String> {
        let date = parse_date(params.date.as_deref(), "date")?.unwrap_or_else(today_local);
        let target = self.resolve_target(&params.target).await?;
        let week = self
            .backend
            .week_timetable(date, &target)
            .await
            .map_err(upstream)?;
        let lessons: Vec<Lesson> = week
            .days
            .iter()
            .find(|day| day.date == date)
            .map(|day| {
                day.lessons
                    .iter()
                    .map(|lesson| Lesson::from_parsed(lesson, &week.timegrid))
                    .collect()
            })
            .unwrap_or_default();
        let note = lessons
            .is_empty()
            .then(|| "No lessons on this day (weekend, holiday or free day).".to_owned());
        Ok(Json(DayTimetableResult {
            target: Target::from(&target),
            date: date_string(date),
            weekday: get_weekday_name(date),
            lessons,
            note,
        }))
    }

    /// List timetable changes in a date range: cancelled lessons, substitutions, room changes,
    /// additional lessons, exams and shifted lessons. Defaults to the next 7 days of the user's
    /// own timetable.
    #[tool(annotations(
        title = "Get timetable changes",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn get_timetable_changes(
        &self,
        Parameters(params): Parameters<ChangesParams>,
    ) -> Result<Json<ChangesResult>, String> {
        let start =
            parse_date(params.start_date.as_deref(), "start_date")?.unwrap_or_else(today_local);
        let end = parse_date(params.end_date.as_deref(), "end_date")?
            .unwrap_or_else(|| add_days(start, 7));
        if end < start {
            return Err("end_date must not be before start_date".to_owned());
        }
        if (end - start).num_days() > MAX_CHANGES_RANGE_DAYS {
            return Err(format!(
                "Range too long; at most {MAX_CHANGES_RANGE_DAYS} days per call"
            ));
        }
        let target = self.resolve_target(&params.target).await?;

        let mut changes = Vec::new();
        let mut week_start = get_monday(start);
        while week_start <= end {
            let week = self
                .backend
                .week_timetable(week_start, &target)
                .await
                .map_err(upstream)?;
            for day in week
                .days
                .iter()
                .filter(|day| day.date >= start && day.date <= end)
            {
                for parsed in &day.lessons {
                    let lesson = Lesson::from_parsed(parsed, &week.timegrid);
                    let has_note = lesson.remarks.is_some() || lesson.text.is_some();
                    if lesson.is_irregular() || (params.include_remarks && has_note) {
                        changes.push(ChangedLesson {
                            date: date_string(day.date),
                            weekday: day.day_name.clone(),
                            lesson,
                        });
                    }
                }
            }
            week_start = add_days(week_start, 7);
        }

        Ok(Json(ChangesResult {
            target: Target::from(&target),
            start_date: date_string(start),
            end_date: date_string(end),
            changes,
        }))
    }

    /// Find rooms with no (non-cancelled) lesson during a time window: a timegrid period like
    /// "3" or "3-4", an explicit start_time/end_time, or right now. Checks every room's
    /// timetable, so it may take a few seconds on first use.
    #[tool(annotations(
        title = "Find free rooms",
        read_only_hint = true,
        open_world_hint = true
    ))]
    async fn find_free_rooms(
        &self,
        Parameters(params): Parameters<FreeRoomsParams>,
    ) -> Result<Json<FreeRoomsResult>, String> {
        let date = parse_date(params.date.as_deref(), "date")?.unwrap_or_else(today_local);
        let (from, to) = self.resolve_window(date, &params).await?;

        let index = self.backend.search_index().await.map_err(upstream)?;
        let rooms = filter_kind(&index, Some(crate::models::TimetableSearchTargetType::Room));
        let rooms = match params.room_query.as_deref().map(str::trim) {
            Some(query) if !query.is_empty() => search_timetable_targets(&rooms, query, None),
            _ => rooms,
        };
        if rooms.is_empty() {
            return Err(
                "No rooms are visible to this account (or none match room_query)".to_owned(),
            );
        }

        let backend = self.backend.clone();
        let checks = stream::iter(rooms)
            .map(move |room| {
                let backend = backend.clone();
                async move {
                    let target = TimetableTarget::from_search_item(&room);
                    let occupied = backend.week_timetable(date, &target).await.map(|week| {
                        week.days
                            .iter()
                            .filter(|day| day.date == date)
                            .flat_map(|day| &day.lessons)
                            .filter(|lesson| {
                                !matches!(output::lesson_status(lesson), "cancelled" | "free")
                            })
                            .any(|lesson| {
                                parse_time_to_minutes(&lesson.start_time) < to
                                    && parse_time_to_minutes(&lesson.end_time) > from
                            })
                    });
                    (room, occupied)
                }
            })
            .buffer_unordered(ROOM_SCAN_CONCURRENCY)
            .collect::<Vec<_>>()
            .await;

        let mut free_rooms = Vec::new();
        let mut occupied_count = 0;
        let mut unknown = Vec::new();
        for (room, result) in checks {
            match result {
                Ok(false) => free_rooms.push(room),
                Ok(true) => occupied_count += 1,
                Err(error) => {
                    tracing::debug!(room = %room.name, %error, "room timetable unavailable");
                    unknown.push(room.name);
                }
            }
        }
        if free_rooms.is_empty() && occupied_count == 0 {
            return Err(format!(
                "Could not read any room timetable ({} rooms tried); this account may not be \
                 allowed to view room timetables",
                unknown.len()
            ));
        }
        free_rooms.sort_by(|left, right| left.name.cmp(&right.name));
        unknown.sort();
        let unknown_total = unknown.len();
        unknown.truncate(MAX_UNKNOWN_ROOMS_LISTED);

        let mut note =
            "Rooms without any timetable (offices, storage) also appear free.".to_owned();
        if unknown_total > MAX_UNKNOWN_ROOMS_LISTED {
            note.push_str(&format!(
                " {unknown_total} rooms could not be checked; only the first \
                 {MAX_UNKNOWN_ROOMS_LISTED} are listed."
            ));
        }
        Ok(Json(FreeRoomsResult {
            date: date_string(date),
            from: minutes_to_clock(from),
            to: minutes_to_clock(to),
            free_rooms: free_rooms.iter().map(SearchResultItem::from).collect(),
            occupied_count,
            unknown_rooms: unknown,
            note,
        }))
    }

    /// List the user's absences in a date range (default: current school year up to today),
    /// with excuse status and totals.
    #[tool(annotations(title = "Get absences", read_only_hint = true, open_world_hint = true))]
    async fn get_absences(
        &self,
        Parameters(params): Parameters<AbsenceParams>,
    ) -> Result<Json<AbsencesResult>, String> {
        let today = today_local();
        let start = match parse_date(params.start_date.as_deref(), "start_date")? {
            Some(date) => date,
            None => self
                .backend
                .school_year_for(today)
                .await
                .map(|year| year.start_date)
                .map_err(upstream)?,
        };
        let end = parse_date(params.end_date.as_deref(), "end_date")?.unwrap_or(today);
        if end < start {
            return Err("end_date must not be before start_date".to_owned());
        }
        let absences = self.backend.absences(start, end).await.map_err(upstream)?;
        let excused = absences.iter().filter(|absence| absence.is_excused).count();
        let total = absences.len();
        Ok(Json(AbsencesResult {
            start_date: date_string(start),
            end_date: date_string(end),
            total,
            excused,
            unexcused: total - excused,
            absences: absences
                .iter()
                .filter(|absence| !params.only_unexcused || !absence.is_excused)
                .map(Absence::from)
                .collect(),
        }))
    }
}

impl UntisServer {
    async fn resolve_target(&self, params: &TargetParams) -> Result<TimetableTarget, String> {
        let kind = params.target_type.unwrap_or(TargetType::Own);
        let name = params
            .target_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        if kind == TargetType::Own && params.target_id.is_none() && name.is_none() {
            return Ok(TimetableTarget::Own);
        }

        let index = self.backend.search_index().await.map_err(upstream)?;
        if let Some(id) = params.target_id {
            let search_type = kind
                .search_type()
                .ok_or("target_id requires target_type to be \"class\", \"room\" or \"teacher\"")?;
            let item = index
                .iter()
                .find(|item| item.r#type == search_type && item.id == id)
                .cloned()
                .unwrap_or_else(|| TimetableSearchItem {
                    r#type: search_type,
                    id,
                    name: id.to_string(),
                    long_name: String::new(),
                    search_text: String::new(),
                });
            return Ok(TimetableTarget::from_search_item(&item));
        }

        let name = name.ok_or("target_name or target_id is required for this target_type")?;
        let candidates = filter_kind(&index, kind.search_type());
        search_timetable_targets(&candidates, name, Some(1))
            .first()
            .map(TimetableTarget::from_search_item)
            .ok_or_else(|| {
                format!(
                    "No class, room or teacher matches \"{name}\". Try search_timetable_targets."
                )
            })
    }

    /// Resolves the free-room window into minutes after midnight.
    async fn resolve_window(
        &self,
        date: NaiveDate,
        params: &FreeRoomsParams,
    ) -> Result<(i32, i32), String> {
        if let Some(period) = params.period.as_deref().filter(|p| !p.trim().is_empty()) {
            let week = self
                .backend
                .week_timetable(date, &TimetableTarget::Own)
                .await
                .map_err(upstream)?;
            let (first, last) = resolve_period(&week.timegrid, period)?;
            return Ok((
                parse_time_to_minutes(&first.start_time),
                parse_time_to_minutes(&last.end_time),
            ));
        }

        let from = match params.start_time.as_deref() {
            Some(start) => parse_clock(start, "start_time")?,
            None if date == today_local() => now_minutes(),
            None => {
                return Err(
                    "Give a period or start_time when checking a day other than today".to_owned(),
                );
            }
        };
        let to = match params.end_time.as_deref() {
            Some(end) => parse_clock(end, "end_time")?,
            None => from + 1,
        };
        if to <= from {
            return Err("end_time must be after start_time".to_owned());
        }
        Ok((from, to))
    }
}

fn filter_kind(
    index: &[TimetableSearchItem],
    kind: Option<crate::models::TimetableSearchTargetType>,
) -> Vec<TimetableSearchItem> {
    index
        .iter()
        .filter(|item| kind.is_none_or(|kind| item.r#type == kind))
        .cloned()
        .collect()
}

fn minutes_to_clock(minutes: i32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for UntisServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("WebUntis")
                    .with_description("Timetables, changes, free rooms and absences from WebUntis"),
            )
            .with_instructions(INSTRUCTIONS)
    }
}
