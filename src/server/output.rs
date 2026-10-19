//! Structured tool results. Empty fields are omitted to keep responses compact
//! for agents.

use crate::models::{
    ParsedAbsence, ParsedLesson, SchoolYear, TimeUnit, TimetableSearchItem,
    TimetableSearchTargetType, TimetableTarget, format_web_date,
};
use chrono::NaiveDate;
use schemars::JsonSchema;
use serde::Serialize;

fn is_false(value: &bool) -> bool {
    !*value
}

/// Omits a long name that adds nothing over the short one.
fn distinct(long: &str, short: &str) -> Option<String> {
    let long = long.trim();
    (!long.is_empty() && long != short.trim()).then(|| long.to_owned())
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// "own", "class", "room" or "teacher".
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
}

impl From<&TimetableTarget> for Target {
    fn from(target: &TimetableTarget) -> Self {
        let (kind, id, name, long_name) = match target {
            TimetableTarget::Own => return Self::own(),
            TimetableTarget::Class {
                id,
                name,
                long_name,
            } => ("class", id, name, long_name),
            TimetableTarget::Room {
                id,
                name,
                long_name,
            } => ("room", id, name, long_name),
            TimetableTarget::Teacher {
                id,
                name,
                long_name,
            } => ("teacher", id, name, long_name),
        };
        Self {
            kind,
            id: Some(*id),
            name: name.clone(),
            long_name: distinct(long_name, name),
        }
    }
}

impl Target {
    fn own() -> Self {
        Self {
            kind: "own",
            id: None,
            name: "My timetable".to_owned(),
            long_name: None,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Period {
    pub name: String,
    pub start: String,
    pub end: String,
}

impl From<&TimeUnit> for Period {
    fn from(unit: &TimeUnit) -> Self {
        Self {
            name: unit.name.clone(),
            start: unit.start_time.clone(),
            end: unit.end_time.clone(),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lesson {
    /// Start time, HH:MM.
    pub start: String,
    /// End time, HH:MM.
    pub end: String,
    /// Timegrid period name(s), e.g. "3. EH" or "3. EH-4. EH" for double lessons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
    /// Subject short name.
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_long_name: Option<String>,
    /// Teachers, formatted as "Full Name (SHORT)" where both are known.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub teachers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_long_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    /// "regular", "cancelled", "substitution", "room_change", "additional", "exam", "shifted",
    /// "free", or another lower-cased WebUntis cell state.
    pub status: String,
    #[serde(skip_serializing_if = "is_false")]
    pub cancelled: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub substitution: bool,
    /// Lesson text set by the school (topic, info).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Substitution or period remarks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remarks: Option<String>,
}

impl Lesson {
    pub fn from_parsed(lesson: &ParsedLesson, timegrid: &[TimeUnit]) -> Self {
        let teachers = if lesson.all_teachers.is_empty() && !lesson.teacher.is_empty() {
            vec![format_teacher(&lesson.teacher, &lesson.teacher_long_name)]
        } else {
            lesson
                .all_teachers
                .iter()
                .enumerate()
                .map(|(index, short)| {
                    let long = lesson
                        .all_teacher_long_names
                        .get(index)
                        .map(String::as_str)
                        .unwrap_or_default();
                    format_teacher(short, long)
                })
                .collect()
        };
        let status = lesson_status(lesson);
        Self {
            start: lesson.start_time.clone(),
            end: lesson.end_time.clone(),
            period: period_label(timegrid, &lesson.start_time, &lesson.end_time),
            subject: lesson.subject.clone(),
            subject_long_name: distinct(&lesson.subject_long_name, &lesson.subject),
            teachers,
            room: non_empty(&lesson.room),
            room_long_name: distinct(&lesson.room_long_name, &lesson.room),
            classes: lesson.all_classes.clone(),
            status: status.to_owned(),
            cancelled: status == "cancelled",
            substitution: lesson.substitution,
            text: non_empty(&lesson.lesson_text),
            remarks: non_empty(&lesson.remarks),
        }
    }

    pub fn is_irregular(&self) -> bool {
        self.status != "regular"
    }
}

fn format_teacher(short: &str, long: &str) -> String {
    match distinct(long, short) {
        Some(long) if !short.trim().is_empty() => format!("{long} ({})", short.trim()),
        Some(long) => long,
        None => short.trim().to_owned(),
    }
}

pub fn lesson_status(lesson: &ParsedLesson) -> &'static str {
    if lesson.cancelled {
        return "cancelled";
    }
    match lesson.cell_state.to_ascii_uppercase().as_str() {
        "" | "STANDARD" if lesson.substitution => "substitution",
        "" | "STANDARD" => "regular",
        "CANCEL" | "CANCELLED" => "cancelled",
        "SUBSTITUTION" => "substitution",
        "ROOMSUBSTITUTION" => "room_change",
        "ADDITIONAL" => "additional",
        "EXAM" => "exam",
        "SHIFT" => "shifted",
        "FREE" => "free",
        _ if lesson.substitution => "substitution",
        _ => "other",
    }
}

fn period_label(timegrid: &[TimeUnit], start: &str, end: &str) -> Option<String> {
    let first = timegrid.iter().find(|unit| unit.start_time == start)?;
    let last = timegrid
        .iter()
        .find(|unit| unit.end_time == end)
        .unwrap_or(first);
    if first.name == last.name {
        Some(first.name.clone())
    } else {
        Some(format!("{}-{}", first.name, last.name))
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    /// YYYY-MM-DD.
    pub date: String,
    pub weekday: String,
    pub lessons: Vec<Lesson>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WeekTimetableResult {
    pub target: Target,
    /// Monday of the week, YYYY-MM-DD.
    pub week_start: String,
    pub days: Vec<Day>,
    /// The school's period grid.
    pub timegrid: Vec<Period>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DayTimetableResult {
    pub target: Target,
    pub date: String,
    pub weekday: String,
    pub lessons: Vec<Lesson>,
    /// Set when the day has no lessons, e.g. a weekend or holiday.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangedLesson {
    pub date: String,
    pub weekday: String,
    #[serde(flatten)]
    pub lesson: Lesson,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChangesResult {
    pub target: Target,
    pub start_date: String,
    pub end_date: String,
    pub changes: Vec<ChangedLesson>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultItem {
    /// "class", "room" or "teacher".
    #[serde(rename = "type")]
    pub kind: TimetableSearchTargetType,
    pub id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_name: Option<String>,
}

impl From<&TimetableSearchItem> for SearchResultItem {
    fn from(item: &TimetableSearchItem) -> Self {
        Self {
            kind: item.r#type,
            id: item.id,
            name: item.name.clone(),
            long_name: distinct(&item.long_name, &item.name),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub results: Vec<SearchResultItem>,
    /// Number of matches before applying the limit.
    pub total_matches: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FreeRoomsResult {
    pub date: String,
    /// Start of the checked window, HH:MM.
    pub from: String,
    /// End of the checked window, HH:MM.
    pub to: String,
    pub free_rooms: Vec<SearchResultItem>,
    pub occupied_count: usize,
    /// Rooms whose timetable could not be read (e.g. no permission); their state is unknown.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_rooms: Vec<String>,
    pub note: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchoolYearItem {
    pub id: i64,
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    /// True for the school year containing today (or the closest one).
    pub current: bool,
}

impl SchoolYearItem {
    pub fn new(year: &SchoolYear, current_id: Option<i64>) -> Self {
        Self {
            id: year.id,
            name: year.name.clone(),
            start_date: format_web_date(year.start_date),
            end_date: format_web_date(year.end_date),
            current: Some(year.id) == current_id,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SchoolYearsResult {
    pub school_years: Vec<SchoolYearItem>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Absence {
    pub id: i64,
    pub start_date: String,
    pub end_date: String,
    pub start_time: String,
    pub end_time: String,
    pub excused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excuse_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl From<&ParsedAbsence> for Absence {
    fn from(absence: &ParsedAbsence) -> Self {
        Self {
            id: absence.id,
            start_date: format_web_date(absence.start_date),
            end_date: format_web_date(absence.end_date),
            start_time: absence.start_time.clone(),
            end_time: absence.end_time.clone(),
            excused: absence.is_excused,
            excuse_status: non_empty(&absence.excuse_status),
            reason: non_empty(&absence.reason),
            text: non_empty(&absence.text),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AbsencesResult {
    pub start_date: String,
    pub end_date: String,
    pub total: usize,
    pub excused: usize,
    pub unexcused: usize,
    /// Newest first.
    pub absences: Vec<Absence>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ContextResult {
    /// Today's date on the server, YYYY-MM-DD.
    pub today: String,
    pub weekday: String,
    /// Current local time, HH:MM.
    pub time: String,
    pub server: String,
    pub school: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_school_year: Option<SchoolYearItem>,
    /// True when serving offline demo data instead of a real account.
    #[serde(skip_serializing_if = "is_false")]
    pub demo: bool,
}

pub fn date_string(date: NaiveDate) -> String {
    format_web_date(date)
}
