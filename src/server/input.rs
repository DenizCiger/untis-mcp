//! Tool parameter types and the lenient parsing agents benefit from
//! ("today", "3. EH", "8:00" ...).

use crate::models::{TimeUnit, TimetableSearchTargetType, add_days, today_local};
use chrono::{Local, NaiveDate, Timelike};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    /// The logged-in user's own timetable.
    Own,
    Class,
    Room,
    Teacher,
}

impl TargetType {
    pub fn search_type(self) -> Option<TimetableSearchTargetType> {
        match self {
            Self::Own => None,
            Self::Class => Some(TimetableSearchTargetType::Class),
            Self::Room => Some(TimetableSearchTargetType::Room),
            Self::Teacher => Some(TimetableSearchTargetType::Teacher),
        }
    }
}

/// Selects whose timetable to read. Leave everything empty for your own.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TargetParams {
    /// Whose timetable to read: "own" (default), "class", "room" or "teacher".
    pub target_type: Option<TargetType>,
    /// Exact id of the class, room or teacher (from search_timetable_targets). Requires target_type.
    pub target_id: Option<i64>,
    /// Name to look up instead of an id, e.g. "4AHIF", "B204" or "Miller". The best match is used
    /// and reported back in the result.
    pub target_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Free-text query matched against short and long names, initials and typos.
    /// Empty returns everything (up to limit).
    #[serde(default)]
    pub query: String,
    /// Only return this type: "class", "room" or "teacher". Omit to search all.
    pub target_type: Option<TimetableSearchTargetType>,
    /// Maximum number of results (default 10, max 100).
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WeekParams {
    /// Any day within the wanted week: YYYY-MM-DD, DD.MM.YYYY, "today", "tomorrow",
    /// "yesterday", "next week" or "last week". Defaults to the current week.
    pub date: Option<String>,
    #[serde(flatten)]
    pub target: TargetParams,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct DayParams {
    /// The day: YYYY-MM-DD, DD.MM.YYYY, "today", "tomorrow" or "yesterday". Defaults to today.
    pub date: Option<String>,
    #[serde(flatten)]
    pub target: TargetParams,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ChangesParams {
    /// First day of the range (inclusive). Defaults to today.
    pub start_date: Option<String>,
    /// Last day of the range (inclusive). Defaults to 7 days after start_date. At most 62 days.
    pub end_date: Option<String>,
    /// Also include otherwise regular lessons that carry a remark or lesson text.
    #[serde(default)]
    pub include_remarks: bool,
    #[serde(flatten)]
    pub target: TargetParams,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct FreeRoomsParams {
    /// The day to check. Defaults to today.
    pub date: Option<String>,
    /// Period name from the school's timegrid, e.g. "3", "3. EH", or a range like "3-4".
    /// Alternative to start_time/end_time.
    pub period: Option<String>,
    /// Start of the time window, HH:MM. Without any time or period the current time is used.
    pub start_time: Option<String>,
    /// End of the time window, HH:MM. Defaults to one minute after start_time.
    pub end_time: Option<String>,
    /// Only consider rooms matching this name query, e.g. "EDV" or "B2".
    pub room_query: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AbsenceParams {
    /// First day of the range (inclusive). Defaults to the start of the current school year.
    pub start_date: Option<String>,
    /// Last day of the range (inclusive). Defaults to today.
    pub end_date: Option<String>,
    /// Only return absences that are not excused yet.
    #[serde(default)]
    pub only_unexcused: bool,
}

pub fn parse_date(input: Option<&str>, field: &str) -> Result<Option<NaiveDate>, String> {
    let Some(raw) = input.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let today = today_local();
    let lowered = raw.to_lowercase();
    let relative = match lowered.as_str() {
        "today" | "now" | "heute" => Some(today),
        "tomorrow" | "morgen" => Some(add_days(today, 1)),
        "yesterday" | "gestern" => Some(add_days(today, -1)),
        "next week" => Some(add_days(today, 7)),
        "last week" | "previous week" => Some(add_days(today, -7)),
        _ => None,
    };
    if relative.is_some() {
        return Ok(relative);
    }
    for format in ["%Y-%m-%d", "%d.%m.%Y", "%Y%m%d", "%d/%m/%Y"] {
        if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
            return Ok(Some(date));
        }
    }
    Err(format!(
        "Invalid {field} \"{raw}\". Use YYYY-MM-DD or today/tomorrow/yesterday."
    ))
}

/// Parses "8:00", "08:00" or "0800" into minutes after midnight.
pub fn parse_clock(raw: &str, field: &str) -> Result<i32, String> {
    let trimmed = raw.trim();
    let (hours, minutes) = match trimmed.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if trimmed.len() == 4 => trimmed.split_at(2),
        None => (trimmed, "0"),
    };
    match (hours.parse::<i32>(), minutes.parse::<i32>()) {
        (Ok(hours), Ok(minutes)) if (0..24).contains(&hours) && (0..60).contains(&minutes) => {
            Ok(hours * 60 + minutes)
        }
        _ => Err(format!("Invalid {field} \"{raw}\". Use HH:MM.")),
    }
}

pub fn now_minutes() -> i32 {
    let now = Local::now();
    (now.hour() * 60 + now.minute()) as i32
}

/// Finds the timegrid unit(s) named by `period`; "3-4" spans from 3's start to 4's end.
pub fn resolve_period<'a>(
    timegrid: &'a [TimeUnit],
    period: &str,
) -> Result<(&'a TimeUnit, &'a TimeUnit), String> {
    let find = |name: &str| {
        let wanted = normalize_period(name);
        timegrid
            .iter()
            .find(|unit| normalize_period(&unit.name) == wanted)
            .ok_or_else(|| {
                let names = timegrid
                    .iter()
                    .map(|unit| unit.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("Unknown period \"{name}\". Known periods: {names}")
            })
    };
    match period.split_once('-') {
        Some((first, last)) => Ok((find(first)?, find(last)?)),
        None => {
            let unit = find(period)?;
            Ok((unit, unit))
        }
    }
}

/// "3. EH", "3.", "3" and " 3 " all normalize to "3"; non-numeric names compare case-insensitively.
fn normalize_period(name: &str) -> String {
    let trimmed = name.trim();
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        trimmed.to_lowercase()
    } else {
        digits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, start: &str, end: &str) -> TimeUnit {
        TimeUnit {
            name: name.to_owned(),
            start_time: start.to_owned(),
            end_time: end.to_owned(),
        }
    }

    #[test]
    fn parses_absolute_and_relative_dates() {
        assert_eq!(
            parse_date(Some("2026-03-04"), "date").unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 4)
        );
        assert_eq!(
            parse_date(Some("04.03.2026"), "date").unwrap(),
            NaiveDate::from_ymd_opt(2026, 3, 4)
        );
        assert_eq!(
            parse_date(Some("Tomorrow"), "date").unwrap(),
            Some(add_days(today_local(), 1))
        );
        assert_eq!(parse_date(None, "date").unwrap(), None);
        assert!(parse_date(Some("soon"), "date").is_err());
    }

    #[test]
    fn parses_clock_times() {
        assert_eq!(parse_clock("8:05", "t").unwrap(), 485);
        assert_eq!(parse_clock("0805", "t").unwrap(), 485);
        assert_eq!(parse_clock("14", "t").unwrap(), 840);
        assert!(parse_clock("25:00", "t").is_err());
    }

    #[test]
    fn resolves_periods_by_number_and_range() {
        let grid = [
            unit("1. EH", "08:00", "08:50"),
            unit("2. EH", "08:55", "09:45"),
            unit("3. EH", "09:55", "10:45"),
        ];
        let (first, last) = resolve_period(&grid, "2").unwrap();
        assert_eq!(
            (first.start_time.as_str(), last.end_time.as_str()),
            ("08:55", "09:45")
        );
        let (first, last) = resolve_period(&grid, "1. EH-3").unwrap();
        assert_eq!(
            (first.start_time.as_str(), last.end_time.as_str()),
            ("08:00", "10:45")
        );
        assert!(resolve_period(&grid, "9").is_err());
    }
}
