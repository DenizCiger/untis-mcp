use super::api::{RawTeacher, RawTimeGridDay, RawTimeUnit};
use super::client::{UntisSession, WebUntisClient, WebUntisError, check_status};
use crate::models::{
    ParsedLesson, TimeUnit, TimetableElementType, WeekTimetable, add_days, format_untis_date,
    format_untis_time, format_web_date, get_monday, get_weekday_name,
};
use chrono::NaiveDate;
use reqwest::header::COOKIE;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Deserialize)]
struct WeeklyResponse {
    data: WeeklyInnerResponse,
}

#[derive(Debug, Deserialize)]
struct WeeklyInnerResponse {
    result: Option<WeeklyResult>,
    error: Option<WeeklyApiError>,
}

#[derive(Debug, Deserialize)]
struct WeeklyApiError {
    data: Option<WeeklyApiErrorData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WeeklyApiErrorData {
    message_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeeklyResult {
    data: WeeklyPayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WeeklyPayload {
    element_periods: HashMap<String, Vec<RawWeeklyLesson>>,
    #[serde(default)]
    elements: Vec<RawDirectoryElement>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDirectoryElement {
    #[serde(rename = "type")]
    element_type: i64,
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    long_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawElementRef {
    #[serde(rename = "type")]
    element_type: i64,
    id: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLessonFlags {
    substitution: Option<bool>,
    room_substitution: Option<bool>,
    #[serde(alias = "roomSubstition", alias = "roomsubstition")]
    room_substition: Option<bool>,
    standard: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawWeeklyLesson {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    lesson_id: i64,
    date: i32,
    start_time: i32,
    end_time: i32,
    #[serde(default)]
    lesson_code: String,
    #[serde(default)]
    lesson_text: String,
    #[serde(default, rename = "periodInfo", alias = "info")]
    period_info: String,
    #[serde(default, rename = "substText")]
    subst_text: String,
    #[serde(default)]
    cell_state: String,
    #[serde(default)]
    student_group: String,
    #[serde(default)]
    is: RawLessonFlags,
    #[serde(default)]
    elements: Vec<RawElementRef>,
}

impl WebUntisClient {
    pub(super) async fn get_weekly_timetable(
        &self,
        session: &UntisSession,
        date: NaiveDate,
        element_id: i64,
        element_type: i64,
    ) -> Result<WeeklyPayload, WebUntisError> {
        let response = self
            .client
            .get(self.url("/WebUntis/api/public/timetable/weekly/data"))
            .header(COOKIE, self.cookie_header(session))
            .query(&[
                ("elementType", element_type.to_string()),
                ("elementId", element_id.to_string()),
                ("date", format_web_date(date)),
                ("formatId", "1".to_owned()),
            ])
            .send()
            .await?;
        let payload = check_status(response)?.json::<WeeklyResponse>().await?;
        if let Some(error) = payload.data.error {
            return Err(WebUntisError::Message(
                error
                    .data
                    .and_then(|data| data.message_key)
                    .unwrap_or_else(|| "Server responded with error".to_owned()),
            ));
        }
        payload
            .data
            .result
            .map(|result| result.data)
            .ok_or_else(|| WebUntisError::Message("Invalid weekly timetable response".to_owned()))
    }
}

pub(super) fn derive_timegrid_from_weekly(weekly: &WeeklyPayload) -> Vec<RawTimeGridDay> {
    let periods = weekly
        .element_periods
        .values()
        .flatten()
        .map(|lesson| (lesson.start_time, lesson.end_time))
        .collect::<BTreeSet<_>>();
    if periods.is_empty() {
        return Vec::new();
    }
    vec![RawTimeGridDay {
        time_units: periods
            .into_iter()
            .enumerate()
            .map(|(index, (start_time, end_time))| RawTimeUnit {
                name: (index + 1).to_string(),
                start_time,
                end_time,
            })
            .collect(),
    }]
}

pub(super) fn build_week_timetable(
    week_date: NaiveDate,
    element_id: i64,
    weekly: WeeklyPayload,
    timegrid: Vec<RawTimeGridDay>,
    teachers: &[RawTeacher],
) -> WeekTimetable {
    let directories = weekly
        .elements
        .into_iter()
        .map(|element| ((element.element_type, element.id), element))
        .collect::<HashMap<_, _>>();
    let teacher_names = build_teacher_full_name_map(teachers);
    let monday = get_monday(week_date);
    let first_grid = timegrid
        .into_iter()
        .find(|grid| !grid.time_units.is_empty())
        .unwrap_or(RawTimeGridDay {
            time_units: Vec::new(),
        });
    let mapped_timegrid = first_grid
        .time_units
        .into_iter()
        .map(|unit| TimeUnit {
            name: unit.name,
            start_time: format_untis_time(unit.start_time),
            end_time: format_untis_time(unit.end_time),
        })
        .collect::<Vec<_>>();

    let mut by_date = HashMap::<i32, Vec<RawWeeklyLesson>>::new();
    let element_entries = weekly
        .element_periods
        .get(&element_id.to_string())
        .cloned()
        .unwrap_or_default();
    for entry in element_entries {
        by_date.entry(entry.date).or_default().push(entry);
    }

    let mut days = Vec::new();
    for offset in 0..7 {
        let day_date = add_days(monday, i64::from(offset));
        let date_num = format_untis_date(day_date)
            .parse::<i32>()
            .unwrap_or_default();
        let mut entries = by_date.remove(&date_num).unwrap_or_default();
        // Weekend days are only listed when the school actually teaches on them.
        if offset >= 5 && entries.is_empty() {
            continue;
        }
        dedupe_day_entries(&mut entries);
        entries.sort_by(compare_lessons_for_display);
        days.push(crate::models::DayTimetable {
            date: day_date,
            day_name: get_weekday_name(day_date),
            lessons: entries
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    parse_timetable_entry(entry, index, date_num, &directories, &teacher_names)
                })
                .collect(),
        });
    }

    WeekTimetable {
        days,
        timegrid: mapped_timegrid,
    }
}

fn build_teacher_full_name_map(teachers: &[RawTeacher]) -> HashMap<i64, String> {
    let mut result = HashMap::new();
    for teacher in teachers {
        let combined = format!("{} {}", teacher.long_name.trim(), teacher.fore_name.trim())
            .trim()
            .to_owned();
        let display = if !combined.is_empty() {
            combined
        } else if !teacher.long_name.trim().is_empty() {
            teacher.long_name.trim().to_owned()
        } else {
            teacher.name.trim().to_owned()
        };
        if !display.is_empty() {
            result.insert(teacher.id, display);
        }
    }
    result
}

fn compare_lessons_for_display(left: &RawWeeklyLesson, right: &RawWeeklyLesson) -> Ordering {
    left.start_time
        .cmp(&right.start_time)
        .then_with(|| left.end_time.cmp(&right.end_time))
        .then_with(|| left.lesson_text.cmp(&right.lesson_text))
        .then_with(|| left.id.cmp(&right.id))
}

fn dedupe_day_entries(entries: &mut Vec<RawWeeklyLesson>) {
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(build_duplicate_entry_key(entry)));
}

fn build_duplicate_entry_key(entry: &RawWeeklyLesson) -> String {
    let mut classes = Vec::new();
    let mut teachers = Vec::new();
    let mut subjects = Vec::new();
    let mut rooms = Vec::new();
    for element in &entry.elements {
        match element.element_type {
            1 => classes.push(element.id),
            2 => teachers.push(element.id),
            3 => subjects.push(element.id),
            4 => rooms.push(element.id),
            _ => {}
        }
    }
    classes.sort_unstable();
    teachers.sort_unstable();
    subjects.sort_unstable();
    rooms.sort_unstable();

    let flags = [
        entry.is.substitution == Some(true),
        entry.is.room_substitution == Some(true) || entry.is.room_substition == Some(true),
        entry.is.standard == Some(true),
    ];

    format!(
        "{}|{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}|{}|{:?}",
        entry.date,
        entry.start_time,
        entry.end_time,
        entry.lesson_id,
        entry.lesson_code,
        entry.cell_state,
        classes,
        teachers,
        subjects,
        rooms,
        entry.student_group,
        flags
    )
}

fn parse_timetable_entry(
    entry: &RawWeeklyLesson,
    index_in_day: usize,
    date_num: i32,
    directories: &HashMap<(i64, i64), RawDirectoryElement>,
    teacher_full_name_by_id: &HashMap<i64, String>,
) -> ParsedLesson {
    let resolved = entry
        .elements
        .iter()
        .filter_map(|element| {
            directories
                .get(&(element.element_type, element.id))
                .map(|directory| {
                    (
                        element.element_type,
                        element.id,
                        directory.name.clone(),
                        if directory.long_name.is_empty() {
                            directory.name.clone()
                        } else {
                            directory.long_name.clone()
                        },
                    )
                })
        })
        .collect::<Vec<_>>();

    let subject = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Subject as i64)
        .map(|element| element.2.clone())
        .unwrap_or_else(|| "Unknown".to_owned());
    let subject_long_name = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Subject as i64)
        .map(|element| element.3.clone())
        .unwrap_or_else(|| subject.clone());
    let teacher = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Teacher as i64)
        .map(|element| element.2.clone())
        .unwrap_or_default();
    let teacher_long_name = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Teacher as i64)
        .map(|element| {
            teacher_full_name_by_id
                .get(&element.1)
                .cloned()
                .unwrap_or_else(|| element.3.clone())
        })
        .unwrap_or_else(|| teacher.clone());
    let room = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Room as i64)
        .map(|element| element.2.clone())
        .unwrap_or_default();
    let room_long_name = resolved
        .iter()
        .find(|element| element.0 == TimetableElementType::Room as i64)
        .map(|element| element.3.clone())
        .unwrap_or_else(|| room.clone());

    let all_teachers = resolved
        .iter()
        .filter(|element| element.0 == TimetableElementType::Teacher as i64)
        .map(|element| element.2.clone())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let all_teacher_long_names = resolved
        .iter()
        .filter(|element| element.0 == TimetableElementType::Teacher as i64)
        .map(|element| {
            teacher_full_name_by_id
                .get(&element.1)
                .cloned()
                .unwrap_or_else(|| element.3.clone())
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let all_classes = resolved
        .iter()
        .filter(|element| element.0 == TimetableElementType::Class as i64)
        .map(|element| element.2.clone())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    let instance_id = if entry.id > 0 {
        entry.id.to_string()
    } else if entry.lesson_id > 0 {
        entry.lesson_id.to_string()
    } else {
        format!(
            "{date_num}-{}-{}-{}-{}-{}-{index_in_day}",
            entry.start_time, entry.end_time, subject, teacher, room
        )
    };

    ParsedLesson {
        instance_id,
        subject,
        subject_long_name,
        lesson_text: entry.lesson_text.clone(),
        cell_state: entry.cell_state.clone(),
        teacher,
        teacher_long_name,
        all_teachers,
        all_teacher_long_names,
        room,
        room_long_name,
        all_classes,
        start_time: format_untis_time(entry.start_time),
        end_time: format_untis_time(entry.end_time),
        cancelled: (entry.is.standard == Some(false) && entry.cell_state == "SUBSTITUTION")
            || entry.lesson_code == "cancelled",
        substitution: entry.is.substitution == Some(true)
            || entry.is.room_substitution == Some(true)
            || entry.is.room_substition == Some(true),
        remarks: if !entry.period_info.is_empty() {
            entry.period_info.clone()
        } else {
            entry.subst_text.clone()
        },
    }
}

#[cfg(test)]
mod timegrid_fallback_tests {
    use super::*;

    #[test]
    fn derives_a_usable_timegrid_when_webuntis_has_no_active_timegrid() {
        let response: WeeklyResponse = serde_json::from_str(
            r#"{
                "data": {
                    "result": {
                        "data": {
                            "elementPeriods": {
                                "42": [
                                    {"date": 20260914, "startTime": 800, "endTime": 850},
                                    {"date": 20260914, "startTime": 850, "endTime": 940}
                                ]
                            }
                        }
                    },
                    "error": null
                }
            }"#,
        )
        .expect("weekly response");
        let weekly = response.data.result.expect("weekly result").data;

        let timegrid = derive_timegrid_from_weekly(&weekly);

        assert_eq!(timegrid.len(), 1);
        assert_eq!(timegrid[0].time_units.len(), 2);
        assert_eq!(timegrid[0].time_units[0].start_time, 800);
        assert_eq!(timegrid[0].time_units[1].end_time, 940);
    }
}
