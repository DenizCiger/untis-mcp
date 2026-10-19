mod backend;
mod demo;
mod models;
mod server;
mod webuntis;

use anyhow::{Context, bail};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use backend::Backend;
use clap::{Parser, ValueEnum};
use models::Config;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use server::UntisServer;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

/// MCP server for WebUntis: timetables, changes, free rooms and absences.
///
/// Credentials are read from environment variables (UNTIS_SERVER, UNTIS_SCHOOL,
/// UNTIS_USERNAME, UNTIS_PASSWORD), as set in your MCP client's server config.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Transport to serve MCP over.
    #[arg(long, env = "UNTIS_MCP_TRANSPORT", value_enum, default_value_t = Transport::Stdio)]
    transport: Transport,

    /// Address to listen on with the HTTP transport.
    #[arg(long, env = "UNTIS_MCP_BIND", default_value = "127.0.0.1:8787")]
    bind: SocketAddr,

    /// URL path of the MCP endpoint with the HTTP transport.
    #[arg(long, env = "UNTIS_MCP_PATH", default_value = "/mcp")]
    path: String,

    /// Bearer token HTTP clients must send. Required when binding to a non-loopback address.
    #[arg(long, env = "UNTIS_MCP_AUTH_TOKEN", hide_env_values = true)]
    auth_token: Option<String>,

    /// Extra Host header values to accept over HTTP (e.g. a public hostname behind a proxy).
    /// Loopback hosts are always accepted.
    #[arg(
        long = "allowed-host",
        env = "UNTIS_MCP_ALLOWED_HOSTS",
        value_delimiter = ','
    )]
    allowed_hosts: Vec<String>,

    /// Allow serving HTTP on a non-loopback address without --auth-token.
    #[arg(long)]
    allow_unauthenticated: bool,

    /// Serve built-in sample data instead of connecting to WebUntis.
    #[arg(long, env = "UNTIS_MCP_DEMO")]
    demo: bool,

    /// Verify the credentials by logging in, then exit.
    #[arg(long)]
    check: bool,

    /// WebUntis server host, e.g. "nessa.webuntis.com" (a full URL is accepted too).
    #[arg(long, env = "UNTIS_SERVER")]
    server: Option<String>,

    /// School name as used in the WebUntis login URL.
    #[arg(long, env = "UNTIS_SCHOOL")]
    school: Option<String>,

    /// WebUntis username.
    #[arg(long, env = "UNTIS_USERNAME")]
    username: Option<String>,

    /// File containing the password, as an alternative to UNTIS_PASSWORD.
    #[arg(long, env = "UNTIS_PASSWORD_FILE")]
    password_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdout carries the MCP protocol for stdio, so logs always go to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("UNTIS_MCP_LOG")
                .unwrap_or_else(|_| EnvFilter::new("untis_mcp=info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cli = Cli::parse();
    let backend = Arc::new(if cli.demo {
        tracing::info!("serving demo data");
        Backend::demo()
    } else {
        Backend::live(load_config(&cli)?)?
    });

    if cli.check {
        backend
            .verify_login()
            .await
            .context("WebUntis login failed")?;
        let config = backend.config();
        eprintln!(
            "Login OK: {} @ {} ({})",
            config.username, config.school, config.server
        );
        backend.close().await;
        return Ok(());
    }

    let result = match cli.transport {
        Transport::Stdio => serve_stdio(backend.clone()).await,
        Transport::Http => serve_http(&cli, backend.clone()).await,
    };
    backend.close().await;
    result
}

fn load_config(cli: &Cli) -> anyhow::Result<Config> {
    let required = |value: &Option<String>, name: &str| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .with_context(|| format!("{name} is not set (or run with --demo)"))
    };
    let password = match &cli.password_file {
        Some(path) => std::fs::read_to_string(path)
            .with_context(|| format!("reading password file {}", path.display()))?
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
        None => std::env::var("UNTIS_PASSWORD")
            .ok()
            .filter(|value| !value.is_empty())
            .context("UNTIS_PASSWORD is not set (or use UNTIS_PASSWORD_FILE / --demo)")?,
    };
    Ok(Config {
        server: normalize_server(&required(&cli.server, "UNTIS_SERVER")?),
        school: required(&cli.school, "UNTIS_SCHOOL")?,
        username: required(&cli.username, "UNTIS_USERNAME")?,
        password,
    })
}

/// Accepts "host", "https://host" or a full WebUntis URL and returns the host.
fn normalize_server(raw: &str) -> String {
    let without_scheme = raw
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .to_owned()
}

async fn serve_stdio(backend: Arc<Backend>) -> anyhow::Result<()> {
    tracing::info!("serving MCP over stdio");
    let service = UntisServer::new(backend)
        .serve(stdio())
        .await
        .context("starting stdio MCP server")?;
    service.waiting().await?;
    Ok(())
}

async fn serve_http(cli: &Cli, backend: Arc<Backend>) -> anyhow::Result<()> {
    let token = cli.auth_token.clone().filter(|token| !token.is_empty());
    if token.is_none() && !cli.bind.ip().is_loopback() && !cli.allow_unauthenticated {
        bail!(
            "refusing to serve on {} without UNTIS_MCP_AUTH_TOKEN; set a token or pass \
             --allow-unauthenticated if a proxy handles auth",
            cli.bind
        );
    }
    if !cli.path.starts_with('/') {
        bail!("--path must start with '/'");
    }

    let cancel = CancellationToken::new();
    let mut allowed_hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    allowed_hosts.extend(cli.allowed_hosts.iter().cloned());
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(allowed_hosts)
        .with_cancellation_token(cancel.child_token());
    let service: StreamableHttpService<UntisServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(UntisServer::new(backend.clone())),
            Default::default(),
            config,
        );

    let mut router = axum::Router::new().nest_service(&cli.path, service);
    if let Some(token) = token {
        router = router.layer(middleware::from_fn_with_state(
            Arc::<str>::from(token),
            require_bearer,
        ));
    }

    let listener = tokio::net::TcpListener::bind(cli.bind)
        .await
        .with_context(|| format!("binding {}", cli.bind))?;
    tracing::info!(
        "serving MCP over streamable HTTP at http://{}{}",
        listener.local_addr()?,
        cli.path
    );
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
            cancel.cancel();
        })
        .await?;
    Ok(())
}

async fn require_bearer(State(token): State<Arc<str>>, request: Request, next: Next) -> Response {
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match provided {
        Some(provided) if constant_time_eq(provided.as_bytes(), token.as_bytes()) => {
            next.run(request).await
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or invalid bearer token",
        )
            .into_response(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_server_urls_to_hosts() {
        assert_eq!(normalize_server("nessa.webuntis.com"), "nessa.webuntis.com");
        assert_eq!(
            normalize_server("https://nessa.webuntis.com/WebUntis/?school=x"),
            "nessa.webuntis.com"
        );
    }

    #[test]
    fn compares_tokens() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secrets"));
    }
}
