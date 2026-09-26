use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, HeaderValue, Method, Request};
use axum::{middleware, response::IntoResponse, response::Response, Router};
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::auth::ApiKey;
use crate::csrf;
use crate::ipc_client::IpcClient;
use crate::routes;

/// A cached IPC snapshot plus the instant it was fetched, used to bound the
/// short-TTL cache in `AppState::snapshot_cache` (B-06).
type SnapshotCacheEntry = (Arc<strivo_core::ipc::ServerMessage>, std::time::Instant);

#[derive(Debug, Clone)]
pub struct ProbeCacheEntry {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub value: serde_json::Value,
}

/// A minimal `AppState` for a handler test: a disconnected IPC client, a
/// known API key, and every cache/limiter/semaphore a handler might reach for.
///
/// One definition, because a second copy of a fifteen-field struct literal is a
/// compile error the next time a field is added — and a handler test that
/// stops compiling is a handler test that quietly stops running.
#[cfg(test)]
pub(crate) fn test_state(api_key: &str) -> AppState {
    AppState {
        ipc: std::sync::Arc::new(crate::ipc_client::IpcClient::disconnected()),
        api_key: crate::auth::ApiKey(api_key.to_string()),
        config_path: None,
        config_write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        config_cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        snapshot_cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        session_secret: "handler-test-session-secret".to_string(),
        login_limiter: crate::ratelimit::LoginLimiter::new(),
        probe_cache: std::sync::Arc::new(
            tokio::sync::RwLock::new(std::collections::HashMap::new()),
        ),
        probe_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
        thumbnail_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        thumbnail_locks: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        thumbnail_failures: std::sync::Arc::new(tokio::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        jobs_db: std::sync::Arc::new(tokio::sync::OnceCell::new()),
        jobs_db_path: std::sync::Arc::new(
            strivo_core::config::AppConfig::data_dir().join("jobs.db"),
        ),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub ipc: Arc<IpcClient>,
    pub api_key: ApiKey,
    /// Configuration selected at startup; shared by every HTTP handler.
    pub config_path: Option<Arc<std::path::PathBuf>>,
    /// Serialize read-modify-write configuration updates in this web process.
    pub config_write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Cached `AppConfig`, populated by `AppConfig::load` on first access
    /// and refreshed by mutation handlers (via `refresh_config`) after a
    /// successful on-disk save. Removes the per-request blocking
    /// `AppConfig::load` (fs read + TOML parse) that previously ran on
    /// every download/Range request and every read-only settings/schedule/
    /// health endpoint (B-01/B-02).
    pub config_cache: Arc<tokio::sync::RwLock<Option<Arc<strivo_core::config::AppConfig>>>>,
    /// Short-TTL cache over `ipc.snapshot()`. A snapshot is a full socket
    /// round-trip that clones the daemon's entire in-memory state (~200
    /// jobs + channels); 11 handlers previously called it independently on
    /// every request. Invalidated immediately by the event relay spawned in
    /// `serve` on any `DaemonEvent`; the TTL below is only the fallback for
    /// when no event has arrived (B-06).
    pub snapshot_cache: Arc<tokio::sync::RwLock<Option<SnapshotCacheEntry>>>,
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
    /// Thumbnail extraction is lower priority than probe/remux work and is
    /// additionally serialised per recording.  A cold gallery must not turn
    /// into one ffmpeg process per visible card.
    pub thumbnail_slots: Arc<tokio::sync::Semaphore>,
    pub thumbnail_locks:
        Arc<tokio::sync::Mutex<HashMap<uuid::Uuid, std::sync::Weak<tokio::sync::Mutex<()>>>>>,
    pub thumbnail_failures: Arc<tokio::sync::Mutex<HashMap<uuid::Uuid, std::time::Instant>>>,
    /// One shared `jobs.db` handle for the whole web process, mirroring the
    /// daemon's single `Arc<Mutex<Connection>>`. Opening per request re-ran
    /// the full PRAGMA + `CREATE TABLE IF NOT EXISTS` batch
    /// (`src/recording/persist.rs`) on every history, blocklist, remux and
    /// plugin call. Lazily initialised so an unavailable database still
    /// surfaces as a per-request error rather than failing server startup.
    pub jobs_db: Arc<tokio::sync::OnceCell<strivo_core::recording::persist::PersistDb>>,
    /// Location of `jobs.db`. Defaults to the user data dir; tests point it
    /// at a temporary directory so the shared-handle accessor can be driven
    /// without touching real user data.
    pub jobs_db_path: Arc<PathBuf>,
}

impl AppState {
    pub fn config_path(&self) -> Option<&std::path::Path> {
        self.config_path.as_deref().map(|path| path.as_path())
    }

    /// Cached config snapshot — loaded once and reused by every read-only
    /// handler instead of re-reading + re-parsing `config.toml` per request.
    /// Populated lazily on first call; refreshed by `refresh_config` after
    /// any handler persists a change.
    pub async fn config(&self) -> Result<Arc<strivo_core::config::AppConfig>, String> {
        if let Some(cfg) = self.config_cache.read().await.as_ref().cloned() {
            return Ok(cfg);
        }
        self.refresh_config().await
    }

    /// Reload `config.toml` from disk and replace the cached snapshot.
    /// Mutation handlers call this after a successful `AppConfig::save`,
    /// while still holding `config_write_lock`, so no reader can observe
    /// the gap between "saved" and "cache updated".
    pub async fn refresh_config(&self) -> Result<Arc<strivo_core::config::AppConfig>, String> {
        let cfg =
            strivo_core::config::AppConfig::load(self.config_path()).map_err(|e| e.to_string())?;
        let arc = Arc::new(cfg);
        *self.config_cache.write().await = Some(arc.clone());
        Ok(arc)
    }

    /// The IPC snapshot, served from a short-TTL cache instead of a fresh
    /// socket round-trip on every call (B-06). Callers that need a specific
    /// field pattern-match the returned `ServerMessage` exactly as they did
    /// against `ipc.snapshot()` directly.
    const SNAPSHOT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

    pub async fn snapshot(&self) -> anyhow::Result<strivo_core::ipc::ServerMessage> {
        {
            let guard = self.snapshot_cache.read().await;
            if let Some((msg, at)) = guard.as_ref() {
                if at.elapsed() < Self::SNAPSHOT_CACHE_TTL {
                    return Ok((**msg).clone());
                }
            }
        }
        let msg = tokio::time::timeout(Self::SNAPSHOT_CACHE_TTL, self.ipc.snapshot())
            .await
            .map_err(|_| anyhow::anyhow!("daemon snapshot timed out"))??;
        *self.snapshot_cache.write().await =
            Some((Arc::new(msg.clone()), std::time::Instant::now()));
        Ok(msg)
    }

    /// Drop the cached snapshot immediately. Called by the event relay
    /// spawned in `serve` whenever a `DaemonEvent` arrives, so a handler
    /// right after a mutation (start/stop recording, a live-status change)
    /// doesn't serve state that's already stale.
    pub async fn invalidate_snapshot(&self) {
        *self.snapshot_cache.write().await = None;
    }

    /// The shared `jobs.db` handle, opened on first use. `PersistDb` is
    /// `Clone` over an inner `Arc<Mutex<Connection>>`, so every caller shares
    /// one connection and one schema initialisation.
    pub async fn jobs_db(&self) -> Result<&strivo_core::recording::persist::PersistDb, String> {
        self.jobs_db
            .get_or_try_init(|| async {
                strivo_core::recording::persist::PersistDb::open(&self.jobs_db_path)
                    .map_err(|e| e.to_string())
            })
            .await
    }

    /// Build an `AppState` for route-shape tests: real auth/routing logic,
    /// wired to an `IpcClient` bound to a socket that never connects. Any
    /// handler that touches IPC will observe a connection failure (a 503,
    /// or in `recordings::download`'s case a "not found" from `lookup_path`)
    /// rather than panicking — that's expected and is what lets tests
    /// assert "reached the handler, not authorized" without a live daemon.
    /// Not `#[cfg(test)]`-gated: `tests/routes.rs` links this crate as an
    /// ordinary dependency (no `cfg(test)`), so the constructor needs to be
    /// reachable from there too.
    pub fn test_state(api_key: &str) -> Self {
        Self {
            ipc: Arc::new(crate::ipc_client::IpcClient::disconnected()),
            api_key: ApiKey(api_key.to_string()),
            config_path: None,
            config_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            config_cache: Arc::new(tokio::sync::RwLock::new(None)),
            snapshot_cache: Arc::new(tokio::sync::RwLock::new(None)),
            session_secret: "route-shape-test-session-secret".to_string(),
            login_limiter: crate::ratelimit::LoginLimiter::new(),
            probe_cache: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            probe_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            thumbnail_slots: Arc::new(tokio::sync::Semaphore::new(1)),
            thumbnail_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            thumbnail_failures: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            jobs_db: Arc::new(tokio::sync::OnceCell::new()),
            jobs_db_path: Arc::new(strivo_core::config::AppConfig::data_dir().join("jobs.db")),
        }
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
    // Record process start time for GET /api/v1/telemetry's `started_at`
    // before anything else — a no-op after the first call.
    crate::telemetry::init();
    // Creator edition only: the canonical project/source/composition schema
    // is applied once, before the first request, so the tables exist by the
    // time anything asks for them. In a PVR build this line does not exist —
    // `creator_schema` is cfg'd out and `strivo-research` is not linked, so a
    // PVR install has no code that could create these tables.
    #[cfg(feature = "creator")]
    crate::creator_schema::ensure_creator_schema();
    let ipc = Arc::new(IpcClient::connect_or_err()?);
    // Session secret must exist before the first request so the cookie
    // /login signs is verifiable by check_key on the same process. Read
    // from config, else generate + persist now (don't defer to /login —
    // that left AppState's copy out of sync with the signed cookie).
    let mut cfg_file = strivo_core::config::AppConfig::load(cfg.config_path.as_deref()).ok();
    let session_secret = {
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
        // Seed the cache from the load already done above for the session
        // secret, instead of reading config.toml a second time at startup.
        config_cache: Arc::new(tokio::sync::RwLock::new(cfg_file.map(Arc::new))),
        snapshot_cache: Arc::new(tokio::sync::RwLock::new(None)),
        session_secret,
        login_limiter: crate::ratelimit::LoginLimiter::new(),
        probe_cache: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        probe_slots: Arc::new(tokio::sync::Semaphore::new(2)),
        thumbnail_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        thumbnail_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        thumbnail_failures: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        jobs_db: Arc::new(tokio::sync::OnceCell::new()),
        jobs_db_path: Arc::new(strivo_core::config::AppConfig::data_dir().join("jobs.db")),
    };

    // Background relay: invalidate the cached IPC snapshot the instant a
    // DaemonEvent arrives, rather than leaving every handler to wait out
    // the 2s TTL (B-06). Runs for the process lifetime; a dead/absent
    // daemon just means `events()` errors immediately and the loop retries
    // after a short backoff, same as a browser's own /events reconnect.
    {
        let relay_state = state.clone();
        tokio::spawn(async move {
            loop {
                let mut stream = relay_state.ipc.events();
                loop {
                    match futures::StreamExt::next(&mut stream).await {
                        Some(Ok(_)) => relay_state.invalidate_snapshot().await,
                        Some(Err(e)) => {
                            tracing::debug!("snapshot-cache event relay error: {e}");
                            break;
                        }
                        None => break,
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    let app = build_router(state);

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

/// Compose the full HTTP router — every route merged, plus the auth/CSRF/
/// telemetry/compression middleware stack — for the given `state`. `serve`
/// calls this for the production binary; route-shape tests call it directly
/// with `AppState::test_state(..)` to exercise the real router (auth
/// included) without a live daemon.
pub fn build_router(state: AppState) -> Router {
    // The SPA (served by assets::router at / and /app) is the webui; it
    // talks to the daemon exclusively through the JSON api + events + auth
    // routers. The legacy askama/htmx page routers (dashboard, channels,
    // recordings, schedule, settings, logs, system) are retired — they
    // served the old server-rendered UI at /, /channels, … and were the
    // reason the bare root showed the pre-redesign dashboard.
    let guarded = Router::new()
        .merge(routes::events::router())
        .merge(routes::api::router())
        .merge(routes::settings::router())
        .merge(routes::backup::router())
        .merge(routes::blocklist::router())
        .merge(routes::capture_profiles::router())
        .merge(routes::import_export::router())
        .merge(routes::recordings::router())
        .merge(routes::login::router())
        .merge(routes::multistream::router())
        .merge(routes::assets::router())
        // Twitch IRC chat's two server-side routes (list rooms, relay a
        // send) are core, every-edition functionality, not Creator-tier —
        // see routes::chat's module doc (S17).
        .merge(routes::chat::router());
    // Creator Edition mounts the rest of the plugin/tooling routes, plus
    // Pro licence status/activate/trial/refresh (ADR 0002 / CE06 — "Pro" is
    // a Creator Edition concept the PVR build has no notion of at all); the
    // PVR build omits both.
    #[cfg(feature = "creator")]
    let guarded = guarded
        .merge(routes::plugins::router())
        .merge(routes::workspace::router())
        .merge(routes::timeline::router())
        .merge(routes::licence::router())
        .merge(routes::backup::creator_router());
    let guarded = guarded
        // S16: authenticate ahead of body/query extraction. `route_layer`
        // (unlike `layer`) wraps only routes already registered on `guarded`
        // above, not its 404 fallback — so an unmatched path still falls
        // through to a plain 404 instead of a 401, and this can't silently
        // start covering routes merged in later (e.g. the public websub
        // router below, which is deliberately merged after this point).
        // Wrapping the route service directly means require_auth's own body
        // runs before axum resolves that route's `Json`/`Query` extractors,
        // closing the "malformed body 422s before auth check ever runs"
        // class of bug for every route under this router in one place,
        // rather than relying on each handler to remember its own check.
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            routes::login::session_refresh,
        ))
        .layer(middleware::from_fn(csrf::csrf_guard));

    // The YouTube WebSub callback is merged AFTER the auth/CSRF layers so it
    // stays public — Google's PubSubHubbub hub sends no API key or CSRF token.
    // It exposes only a verification echo and a poll-trigger, no data.
    // tower_http's DefaultPredicate already skips already-encoded, Range,
    // gRPC, image, and SSE responses, but not video/audio — so a plain
    // `CompressionLayer::new()` gzips the recordings download route on any
    // request that omits `Range` (some players' initial probe, `curl`,
    // download managers). Measured: a 258MB recording went from a 0.59s
    // identity stream to 6.2s once gzip kicked in (~36MB/s, CPU-bound on an
    // already-compressed container) and lost Content-Length/Accept-Ranges
    // in favour of chunked transfer. Recording bytes are already compressed
    // by the source codec, so exclude them the same way images are excluded.
    let compression_predicate = DefaultPredicate::new()
        .and(NotForContentType::const_new("video/"))
        .and(NotForContentType::const_new("audio/"));
    guarded
        .merge(routes::websub::router())
        .layer(CompressionLayer::new().compress_when(compression_predicate))
        // CE-Fusion F3: content-free per-route latency/reliability
        // telemetry, queryable at GET /api/v1/telemetry. Wired with
        // `.layer()` (not `.route_layer()`) at this router-composition
        // point so it also wraps the catch-all 404 fallback — that's what
        // lets truly unmatched requests reach the recorder and fall into
        // the bounded "unmatched" bucket instead of escaping telemetry
        // entirely. axum inserts `MatchedPath` into request extensions
        // before dispatching into a route's (already-layered) service, so
        // `.layer()` here sees it same as `.route_layer()` would.
        .layer(middleware::from_fn(crate::telemetry::record_request))
        .layer(middleware::from_fn(performance_timing))
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The single source of truth for every intentionally-public (no
/// credential required) route, as `(method, path-or-pattern)` pairs — the
/// same shape `.route(...)` call sites and the S08 auth-sweep test
/// (`crates/strivo-web/tests/routes.rs`) already use. `is_public_route`
/// interprets this table at runtime; the test imports it directly instead
/// of hand-maintaining its own copy, so the two can no longer drift apart.
///
/// A `{*path}` catch-all pattern (only `/assets/{*path}` uses one) is
/// matched as a path prefix, regardless of method — matching the
/// static-asset handler, which isn't method-gated either. Every other
/// entry is matched by exact method + path.
///
/// `/yt-websub` is public too, but it's merged onto the router *after*
/// `require_auth`'s `route_layer` runs, so `is_public_route` never
/// actually gets called with that path — see `build_router`. It's still
/// listed here so the auth-sweep test can treat it as accounted-for.
pub const PUBLIC_ROUTES: &[(&str, &str)] = &[
    ("get", "/api/v1/health"),
    ("get", "/"),
    ("get", "/app"),
    ("get", "/assets/{*path}"),
    ("get", "/yt-websub"),
    ("post", "/yt-websub"),
    ("post", "/api/v1/auth/login"),
];

fn is_public_route(method: &Method, path: &str) -> bool {
    PUBLIC_ROUTES.iter().any(|&(m, pattern)| {
        if let Some(prefix) = pattern.strip_suffix("{*path}") {
            path.starts_with(prefix)
        } else {
            method.as_str().eq_ignore_ascii_case(m) && path == pattern
        }
    })
}

/// S16: auth gate applied via `route_layer` (see `build_router`) so it runs
/// on every already-registered route before axum resolves that route's own
/// extractors — including `Json`/`Query`, which otherwise reject a
/// malformed request with 400/422 before a handler's own auth check gets a
/// chance to run at all. A handler may still carry its own `check_key`/
/// `check_dual` call (several do, for defense in depth); this layer is what
/// makes that check load-bearing for every route rather than opt-in per
/// handler.
async fn require_auth(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    if is_public_route(req.method(), req.uri().path()) {
        return next.run(req).await;
    }
    if routes::login::check_dual(req.headers(), &state.api_key, &state.session_secret).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    next.run(req).await
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
