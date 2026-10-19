use super::client::{
    UntisSession, WebUntisClient, WebUntisError, check_status, is_not_authenticated_code,
};
use crate::models::{SchoolYear, parse_untis_date};
use reqwest::header::COOKIE;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(super) struct RpcEnvelope<T> {
    pub(super) result: Option<T>,
    pub(super) error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RpcError {
    pub(super) code: Option<i64>,
    pub(super) message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawSchoolYear {
    pub(super) id: i64,
    #[serde(default)]
    pub(super) name: String,
    pub(super) start_date: UntisDate,
    pub(super) end_date: UntisDate,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum UntisDate {
    Number(i32),
    Text(String),
}

impl UntisDate {
    fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Number(value) => Some(*value),
            Self::Text(value) => value.parse().ok(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawTimeGridDay {
    pub(super) time_units: Vec<RawTimeUnit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawTimeUnit {
    pub(super) name: String,
    pub(super) start_time: i32,
    pub(super) end_time: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawTeacher {
    pub(super) id: i64,
    pub(super) name: String,
    #[serde(default)]
    pub(super) fore_name: String,
    #[serde(default)]
    pub(super) long_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawRoom {
    pub(super) id: i64,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) long_name: String,
    #[serde(default)]
    pub(super) alternate_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RawClass {
    pub(super) id: i64,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) long_name: String,
}

impl WebUntisClient {
    pub(super) async fn rpc_request<T: DeserializeOwned, P: Serialize>(
        &self,
        session: &UntisSession,
        method: &str,
        params: P,
    ) -> Result<T, WebUntisError> {
        let response = self
            .client
            .post(self.url("/WebUntis/jsonrpc.do"))
            .query(&[("school", self.config.school.as_str())])
            .header(COOKIE, self.cookie_header(session))
            .json(&serde_json::json!({
                "id": super::client::APP_IDENTITY,
                "method": method,
                "params": params,
                "jsonrpc": "2.0",
            }))
            .send()
            .await?;
        let response = check_status(response)?;

        let envelope = response.json::<RpcEnvelope<T>>().await?;
        if let Some(result) = envelope.result {
            return Ok(result);
        }

        let error = envelope.error;
        let code = error.as_ref().and_then(|error| error.code);
        let message = error
            .and_then(|error| error.message)
            .unwrap_or_else(|| format!("Server didn't return any result for {method}"));
        if is_not_authenticated_code(code) {
            return Err(WebUntisError::Unauthorized(message));
        }
        Err(WebUntisError::Message(message))
    }

    pub(super) async fn get_schoolyears(
        &self,
        session: &UntisSession,
    ) -> Result<Vec<SchoolYear>, WebUntisError> {
        let raw = self
            .rpc_request::<Vec<RawSchoolYear>, _>(session, "getSchoolyears", serde_json::json!({}))
            .await?;
        map_school_years(raw)
    }

    pub(super) async fn get_teachers(
        &self,
        session: &UntisSession,
    ) -> Result<Vec<RawTeacher>, WebUntisError> {
        self.rpc_request(session, "getTeachers", serde_json::json!({}))
            .await
    }

    pub(super) async fn get_rooms(
        &self,
        session: &UntisSession,
    ) -> Result<Vec<RawRoom>, WebUntisError> {
        self.rpc_request(session, "getRooms", serde_json::json!({}))
            .await
    }

    pub(super) async fn get_classes(
        &self,
        session: &UntisSession,
        schoolyear_id: Option<i64>,
    ) -> Result<Vec<RawClass>, WebUntisError> {
        let params = match schoolyear_id {
            Some(id) => serde_json::json!({ "schoolyearId": id }),
            None => serde_json::json!({}),
        };
        self.rpc_request(session, "getKlassen", params).await
    }

    pub(super) async fn get_timegrid(
        &self,
        session: &UntisSession,
    ) -> Result<Vec<RawTimeGridDay>, WebUntisError> {
        self.rpc_request(session, "getTimegridUnits", serde_json::json!({}))
            .await
    }
}

fn map_school_years(raw: Vec<RawSchoolYear>) -> Result<Vec<SchoolYear>, WebUntisError> {
    let mut school_years = raw
        .into_iter()
        .map(|year| {
            let start_date = year
                .start_date
                .as_i32()
                .and_then(parse_untis_date)
                .ok_or_else(|| {
                    WebUntisError::Message(format!(
                        "Invalid start date for school year {}",
                        year.id
                    ))
                })?;
            let end_date = year
                .end_date
                .as_i32()
                .and_then(parse_untis_date)
                .ok_or_else(|| {
                    WebUntisError::Message(format!("Invalid end date for school year {}", year.id))
                })?;
            Ok(SchoolYear {
                id: year.id,
                name: if year.name.trim().is_empty() {
                    format!("{} / {}", start_date.format("%Y"), end_date.format("%Y"))
                } else {
                    year.name
                },
                start_date,
                end_date,
            })
        })
        .collect::<Result<Vec<_>, WebUntisError>>()?;
    school_years.sort_by_key(|year| std::cmp::Reverse(year.start_date));
    if school_years.is_empty() {
        return Err(WebUntisError::Message(
            "WebUntis returned no school years".to_owned(),
        ));
    }
    Ok(school_years)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn school_years_accept_string_and_numeric_dates_and_sort_newest_first() {
        let raw: Vec<RawSchoolYear> = serde_json::from_str(
            r#"[
                {"id": 1, "name": "2024/2025", "startDate": "20240902", "endDate": "20250704"},
                {"id": 2, "name": "2025/2026", "startDate": 20250901, "endDate": 20260703}
            ]"#,
        )
        .expect("school year payload");

        let years = map_school_years(raw).expect("valid school years");

        assert_eq!(
            years.iter().map(|year| year.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(
            years[0].start_date,
            chrono::NaiveDate::from_ymd_opt(2025, 9, 1).unwrap()
        );
    }
}
