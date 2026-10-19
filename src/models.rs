use chrono::{Datelike, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    pub school: String,
    pub username: String,
    pub password: String,
    pub server: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("school", &self.school)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("server", &self.server)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchoolYear {
    pub id: i64,
    pub name: String,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedLesson {
    #[serde(default)]
    pub instance_id: String,
    pub subject: String,
    pub subject_long_name: String,
    #[serde(default)]
    pub lesson_text: String,
    #[serde(default)]
    pub cell_state: String,
    #[serde(default)]
    pub teacher: String,
    #[serde(default)]
    pub teacher_long_name: String,
    #[serde(default)]
    pub all_teachers: Vec<String>,
    #[serde(default)]
    pub all_teacher_long_names: Vec<String>,
    #[serde(default)]
    pub room: String,
    #[serde(default)]
    pub room_long_name: String,
    #[serde(default)]
    pub all_classes: Vec<String>,
    pub start_time: String,
    pub end_time: String,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub substitution: bool,
    #[serde(default)]
    pub remarks: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAbsence {
    pub id: i64,
    pub student_name: String,
    pub reason: String,
    pub text: String,
    pub excuse_status: String,
    pub is_excused: bool,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeUnit {
    pub name: String,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayTimetable {
    pub date: NaiveDate,
    pub day_name: String,
    pub lessons: Vec<ParsedLesson>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeekTimetable {
    pub days: Vec<DayTimetable>,
    pub timegrid: Vec<TimeUnit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimetableSearchTargetType {
    Class,
    Room,
    Teacher,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TimetableElementType {
    Class = 1,
    Teacher = 2,
    Subject = 3,
    Room = 4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimetableTarget {
    Own,
    Class {
        id: i64,
        name: String,
        long_name: String,
    },
    Room {
        id: i64,
        name: String,
        long_name: String,
    },
    Teacher {
        id: i64,
        name: String,
        long_name: String,
    },
}

impl TimetableTarget {
    pub fn from_search_item(item: &TimetableSearchItem) -> Self {
        let (id, name, long_name) = (item.id, item.name.clone(), item.long_name.clone());
        match item.r#type {
            TimetableSearchTargetType::Class => Self::Class {
                id,
                name,
                long_name,
            },
            TimetableSearchTargetType::Room => Self::Room {
                id,
                name,
                long_name,
            },
            TimetableSearchTargetType::Teacher => Self::Teacher {
                id,
                name,
                long_name,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimetableSearchItem {
    pub r#type: TimetableSearchTargetType,
    pub id: i64,
    pub name: String,
    pub long_name: String,
    pub search_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimetableRequestTarget {
    Own,
    Target {
        id: i64,
        element_type: TimetableElementType,
    },
}

pub fn resolve_timetable_request(target: &TimetableTarget) -> TimetableRequestTarget {
    match target {
        TimetableTarget::Own => TimetableRequestTarget::Own,
        TimetableTarget::Class { id, .. } => TimetableRequestTarget::Target {
            id: *id,
            element_type: TimetableElementType::Class,
        },
        TimetableTarget::Room { id, .. } => TimetableRequestTarget::Target {
            id: *id,
            element_type: TimetableElementType::Room,
        },
        TimetableTarget::Teacher { id, .. } => TimetableRequestTarget::Target {
            id: *id,
            element_type: TimetableElementType::Teacher,
        },
    }
}

pub fn today_local() -> NaiveDate {
    Local::now().date_naive()
}

pub fn add_days(date: NaiveDate, days: i64) -> NaiveDate {
    date + Duration::days(days)
}

pub fn get_monday(date: NaiveDate) -> NaiveDate {
    let weekday = i64::from(date.weekday().num_days_from_monday());
    date - Duration::days(weekday)
}

pub fn date_in_school_year(date: NaiveDate, school_year: &SchoolYear) -> bool {
    date >= school_year.start_date && date <= school_year.end_date
}

pub fn clamp_date_to_school_year(date: NaiveDate, school_year: &SchoolYear) -> NaiveDate {
    date.max(school_year.start_date).min(school_year.end_date)
}

/// Picks the school year containing `date`, falling back to the nearest one.
pub fn select_school_year(school_years: &[SchoolYear], date: NaiveDate) -> Option<&SchoolYear> {
    school_years
        .iter()
        .filter(|school_year| date_in_school_year(date, school_year))
        .max_by_key(|school_year| school_year.start_date)
        .or_else(|| {
            school_years.iter().min_by(|left, right| {
                let left_distance = (date - clamp_date_to_school_year(date, left))
                    .num_days()
                    .abs();
                let right_distance = (date - clamp_date_to_school_year(date, right))
                    .num_days()
                    .abs();
                left_distance
                    .cmp(&right_distance)
                    .then_with(|| right.start_date.cmp(&left.start_date))
            })
        })
}

pub fn parse_untis_date(value: i32) -> Option<NaiveDate> {
    let year = value / 10_000;
    let month = ((value / 100) % 100) as u32;
    let day = (value % 100) as u32;
    NaiveDate::from_ymd_opt(year, month, day)
}

pub fn format_untis_date(date: NaiveDate) -> String {
    format!("{:04}{:02}{:02}", date.year(), date.month(), date.day())
}

pub fn format_web_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

pub fn format_untis_time(value: i32) -> String {
    let hours = value.div_euclid(100);
    let minutes = value.rem_euclid(100);
    format!("{hours:02}:{minutes:02}")
}

pub fn parse_time_to_minutes(value: &str) -> i32 {
    let mut parts = value.split(':');
    let hours = parts
        .next()
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    let minutes = parts
        .next()
        .and_then(|part| part.parse::<i32>().ok())
        .unwrap_or(0);
    hours * 60 + minutes
}

pub fn get_weekday_name(date: NaiveDate) -> String {
    match date.weekday().number_from_monday() {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        _ => "Sunday",
    }
    .to_owned()
}

pub fn compare_absence_newest_first(
    left: &ParsedAbsence,
    right: &ParsedAbsence,
) -> std::cmp::Ordering {
    right
        .start_date
        .cmp(&left.start_date)
        .then_with(|| right.start_time.cmp(&left.start_time))
        .then_with(|| right.id.cmp(&left.id))
}
