use crate::models::Config;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use reqwest::StatusCode;
use reqwest::header::{CACHE_CONTROL, COOKIE, HeaderMap, HeaderValue, PRAGMA, USER_AGENT};
use serde::Deserialize;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub(super) const APP_IDENTITY: &str = "untis-mcp";

/// WebUntis drops idle sessions after roughly 10 minutes; renew a bit earlier.
const SESSION_IDLE_LIMIT: Duration = Duration::from_secs(8 * 60);

/// JSON-RPC error code WebUntis returns for an expired or missing session.
const RPC_NOT_AUTHENTICATED: i64 = -8520;

#[derive(Debug, thiserror::Error)]
pub enum WebUntisError {
    #[error("{0}")]
    Message(String),
    #[error("WebUntis rejected the request ({0})")]
    Unauthorized(String),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UntisSession {
    pub(super) session_id: String,
    pub(super) person_id: i64,
    pub(super) person_type: i64,
}

struct CachedSession {
    session: UntisSession,
    last_used: Instant,
}

pub struct WebUntisClient {
    pub(super) client: reqwest::Client,
    pub(super) config: Config,
    session: Mutex<Option<CachedSession>>,
    pub(super) master_data: Mutex<Option<super::queries::MasterData>>,
}

impl WebUntisClient {
    pub fn new(config: Config) -> Result<Self, WebUntisError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_6) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/61.0.3163.79 Safari/537.36",
            ),
        );
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
        headers.insert(
            "X-Requested-With",
            HeaderValue::from_static("XMLHttpRequest"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            config,
            session: Mutex::new(None),
            master_data: Mutex::new(None),
        })
    }

    /// Runs `operation` with a valid session, logging in on first use and
    /// re-authenticating once if WebUntis reports the session as expired.
    pub(super) async fn with_session<T, F, Fut>(&self, operation: F) -> Result<T, WebUntisError>
    where
        F: Fn(UntisSession) -> Fut,
        Fut: Future<Output = Result<T, WebUntisError>>,
    {
        let session = self.current_session(false).await?;
        match operation(session).await {
            Err(WebUntisError::Unauthorized(reason)) => {
                tracing::info!(%reason, "WebUntis session expired, logging in again");
                let session = self.current_session(true).await?;
                operation(session).await
            }
            result => result,
        }
    }

    async fn current_session(&self, force_login: bool) -> Result<UntisSession, WebUntisError> {
        let mut guard = self.session.lock().await;
        if let Some(cached) = guard.as_mut()
            && !force_login
            && cached.last_used.elapsed() < SESSION_IDLE_LIMIT
        {
            cached.last_used = Instant::now();
            return Ok(cached.session.clone());
        }

        let session = self.login().await?;
        tracing::debug!("logged in to WebUntis");
        *guard = Some(CachedSession {
            session: session.clone(),
            last_used: Instant::now(),
        });
        Ok(session)
    }

    /// Checks credentials by logging in. The session is kept for later calls.
    pub async fn verify_login(&self) -> Result<(), WebUntisError> {
        self.current_session(true).await.map(|_| ())
    }

    /// Ends the cached session, if any.
    pub async fn close(&self) {
        if let Some(cached) = self.session.lock().await.take() {
            let _ = self.logout(&cached.session).await;
        }
    }

    async fn login(&self) -> Result<UntisSession, WebUntisError> {
        let response = self
            .client
            .post(self.url("/WebUntis/jsonrpc.do"))
            .query(&[("school", self.config.school.as_str())])
            .json(&serde_json::json!({
                "id": APP_IDENTITY,
                "method": "authenticate",
                "params": {
                    "user": self.config.username,
                    "password": self.config.password,
                    "client": APP_IDENTITY,
                },
                "jsonrpc": "2.0",
            }))
            .send()
            .await?;
        let response = check_status(response)?;

        let envelope = response
            .json::<super::api::RpcEnvelope<UntisSession>>()
            .await?;
        if let Some(result) = envelope.result {
            if result.session_id.is_empty() {
                return Err(WebUntisError::Message(
                    "Failed to login. No session id.".to_owned(),
                ));
            }
            return Ok(result);
        }

        Err(WebUntisError::Message(format!(
            "WebUntis login failed: {}",
            envelope
                .error
                .and_then(|error| error.message)
                .unwrap_or_else(|| "unknown error".to_owned())
        )))
    }

    async fn logout(&self, session: &UntisSession) -> Result<(), WebUntisError> {
        let _ = self
            .client
            .post(self.url("/WebUntis/jsonrpc.do"))
            .query(&[("school", self.config.school.as_str())])
            .header(COOKIE, self.cookie_header(session))
            .json(&serde_json::json!({
                "id": APP_IDENTITY,
                "method": "logout",
                "params": {},
                "jsonrpc": "2.0",
            }))
            .send()
            .await?;
        Ok(())
    }

    pub(super) fn cookie_header(&self, session: &UntisSession) -> String {
        let school_cookie = format!("_{}", BASE64_STANDARD.encode(self.config.school.as_bytes()));
        format!(
            "JSESSIONID={}; schoolname={school_cookie}",
            session.session_id
        )
    }

    pub(super) fn url(&self, path: &str) -> String {
        format!("https://{}{}", self.config.server, path)
    }
}

pub(super) fn is_not_authenticated_code(code: Option<i64>) -> bool {
    code == Some(RPC_NOT_AUTHENTICATED)
}

/// Maps HTTP-level auth failures (including redirects to the login page,
/// which WebUntis issues for expired sessions) to `Unauthorized`.
pub(super) fn check_status(
    response: reqwest::Response,
) -> Result<reqwest::Response, WebUntisError> {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED
        || status == StatusCode::FORBIDDEN
        || status.is_redirection()
    {
        return Err(WebUntisError::Unauthorized(format!("HTTP {status}")));
    }
    if !status.is_success() {
        return Err(WebUntisError::Message(format!(
            "WebUntis responded with HTTP {status}"
        )));
    }
    Ok(response)
}
