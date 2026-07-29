use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use axum::http::{header, HeaderValue, Request};
use axum::{middleware, response::Response, Router};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::auth::ApiKey;
use crate::csrf;
use crate::ipc_client::IpcClient;
use crate::routes;

#[derive(Debug, Clone)]
pub struct ProbeCacheEntry {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub value: serde_json::Value,
}

#[derive(Clone)]
pub struct AppState {
    pub ipc: Arc<IpcClient>,
    pub api_key: ApiKey,
    /// Configuration selected at startup; shared by every HTTP handler.
    pub config_path: Option<Arc<std::path::PathBuf>>,
    /// Serialize read-modify-write configuration updates in this web process.
    pub config_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// HMAC secret for browser-session cookies (W3). Loaded from
    /// `WebConfig.session_secret`, or generated + persisted at startup
    /// (see `serve`), so it always exists.
    pub session_secret: String,
    /// Per-IP failed-login throttle (roadmap Phase 1).
    pub login_limiter: crate::ratelimit::LoginLimiter,
    /// Normalised ffprobe results keyed by canonical media path. Entries are
    /// invalidated by file length or mtime changes.
    pub probe_cache: Arc<tokio::sync::RwLock<std::collections::HashMap<PathBuf, ProbeCacheEntry>>>,
    /// FFprobe is disk and process intensive. Bound concurrent probes so a
    /// gallery of Info modals cannot starve active recordings.
    pub probe_slots: Arc<tokio::sync::Semaphore>,
}

impl AppState {
    pub fn config_path(&self) -> Option<&std::path::Path> {
        self.config_path.as_deref().map(|path| path.as_path())
    }
}

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub bind: SocketAddr,
    pub api_key: ApiKey,
    pub config_path: Option<std::path::PathBuf>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8181".parse().expect("hardcoded addr parses"),
            api_key: ApiKey::generate(),
            config_path: None,
        }
    }
}

pub async fn serve(cfg: ServeConfig) -> Result<()> {
    let ipc = Arc::new(IpcClient::connect_or_err()?);
    // Session secret must exist before the first request so the cookie
    // /login signs is verifiable by check_key on the same process. Read
    // from config, else generate + persist now (don't defer to /login —
    // that left AppState's copy out of sync with the signed cookie).
    let session_secret = {
        let mut cfg_file = strivo_core::config::AppConfig::load(cfg.config_path.as_deref()).ok();
        let existing = cfg_file.as_ref().and_then(|c| c.web.session_secret.clone());
        match existing {
            Some(s) => s,
            None => {
                let s = crate::auth::generate_session_secret();
                if let Some(ref mut c) = cfg_file {
                    c.web.session_secret = Some(s.clone());
                    if let Err(e) = c.save(cfg.config_path.as_deref()) {
                        tracing::warn!("could not persist [web].session_secret: {e}");
                    }
                }
                s
            }
        }
    };
    let state = AppState {
        ipc,
        api_key: cfg.api_key,
        config_path: cfg.config_path.map(Arc::new),
        config_write_lock: Arc::new(tokio::sync::Mutex::new(())),
        session_secret,
        login_limiter: crate::ratelimit::LoginLimiter::new(),
        probe_cache: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        probe_slots: Arc::new(tokio::sync::Semaphore::new(2)),
    };

    // The SPA (served by assets::router at / and /app) is the webui; it
    // talks to the daemon exclusively through the JSON api + events + auth
    // routers. The legacy askama/htmx page routers (dashboard, channels,
    // recordings, schedule, settings, logs, system) are retired — they
    // served the old server-rendered UI at /, /channels, … and were the
    // reason the bare root showed the pre-redesign dashboard.
    let guarded = Router::new()
        .merge(routes::events::router())
        .merge(routes::api::router())
        .merge(routes::licence::router())
        .merge(routes::recordings::router())
        .merge(routes::login::router())
        .merge(routes::multistream::router())
        .merge(routes::assets::router());
    // Creator Edition mounts the plugin/tooling routes; the PVR build omits them.
    #[cfg(feature = "creator")]
    let guarded = guarded.merge(routes::plugins::router());
    let guarded = guarded
        .layer(middleware::from_fn_with_state(
            state.clone(),
            routes::login::session_refresh,
        ))
        .layer(middleware::from_fn(csrf::csrf_guard));

    // The YouTube WebSub callback is merged AFTER the auth/CSRF layers so it
    // stays public — Google's PubSubHubbub hub sends no API key or CSRF token.
    // It exposes only a verification echo and a poll-trigger, no data.
    let app = guarded
        .merge(routes::websub::router())
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(performance_timing))
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;

    tracing::info!(addr = %cfg.bind, "strivo-web listening");
    // ConnectInfo carries the peer IP into the login rate limiter.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn performance_timing(
    request: Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started = std::time::Instant::now();
    let mut response = next.run(request).await;
    let elapsed = started.elapsed();
    if let Ok(value) =
        HeaderValue::from_str(&format!("app;dur={:.2}", elapsed.as_secs_f64() * 1000.0))
    {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("server-timing"), value);
    }
    tracing::info!(
        http.method = %method,
        http.route = %path,
        http.status = response.status().as_u16(),
        duration_ms = elapsed.as_secs_f64() * 1000.0,
        "http request completed"
    );
    response
}

async fn security_headers(request: Request<axum::body::Body>, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}
