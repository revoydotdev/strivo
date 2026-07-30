//! Content-free product telemetry (CE-Fusion F3 / research-roadmap Phase 0).
//!
//! Local-only, process-lifetime operational metrics for the HTTP API: per
//! `(method, matched route template)` request count, 4xx/5xx counts, and a
//! latency histogram. Nothing content-derived is ever recorded — no bodies,
//! no headers, no query strings, no path parameter values, no raw URIs.
//! Cardinality is bounded by the router's own route count: unmatched
//! requests collapse into a single `"unmatched"` key instead of one entry
//! per garbage/probed path.
//!
//! Two pieces:
//!
//! - [`record_request`] — an `axum::middleware::from_fn` layer that times
//!   each request and records it against [`extract::MatchedPath`] (the
//!   route template, e.g. `/api/v1/recordings/{id}`), never the concrete
//!   URI.
//! - [`telemetry_handler`] — `GET /api/v1/telemetry`, a JSON summary of the
//!   registry, gated by the same dual auth (`X-Api-Key` header or session
//!   cookie) every other non-creator `/api/v1` route uses.
//!
//! Serves both editions: not gated behind the `creator` feature.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{MatchedPath, State};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::server::AppState;

/// Log-spaced histogram bucket upper bounds, in milliseconds. A duration
/// falls into the first bucket whose bound is `>=` it; anything past the
/// last bound (5s) lands in an implicit `+inf` overflow bucket. Bounded
/// array size keeps memory per route key fixed regardless of traffic.
const BUCKET_BOUNDS_MS: [u64; 12] = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000];
/// One slot per bound plus the `+inf` overflow bucket.
const BUCKET_COUNT: usize = BUCKET_BOUNDS_MS.len() + 1;

/// Key unmatched requests (no router entry matched, e.g. a 404 probe)
/// aggregate under. Keeps cardinality bounded by the router's route count
/// instead of growing with every garbage path an attacker or bot tries.
const UNMATCHED_ROUTE: &str = "unmatched";

/// Per-`(method, route template)` running aggregate.
#[derive(Debug, Default)]
struct RouteStats {
    count: u64,
    errors_4xx: u64,
    errors_5xx: u64,
    max_ms: u64,
    buckets: [u64; BUCKET_COUNT],
}

impl RouteStats {
    fn record(&mut self, status: StatusCode, duration: Duration) {
        self.count += 1;
        if status.is_client_error() {
            self.errors_4xx += 1;
        } else if status.is_server_error() {
            self.errors_5xx += 1;
        }
        let ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        self.max_ms = self.max_ms.max(ms);
        let idx = BUCKET_BOUNDS_MS
            .iter()
            .position(|&bound| ms <= bound)
            .unwrap_or(BUCKET_COUNT - 1);
        self.buckets[idx] += 1;
    }

    /// Nearest-rank quantile estimate from the histogram: walk buckets in
    /// order, accumulating counts, and return the bound of the first
    /// bucket whose cumulative count reaches `ceil(p * count)`.
    ///
    /// Tolerance: the reported value is the *bucket upper bound*, not the
    /// true observation, so it can overstate the real latency by up to the
    /// width of that bucket. Buckets are log-spaced with a growth factor of
    /// at most 2.5x (e.g. 1→2, 2→5, 1000→2500), so the worst-case relative
    /// error is a 2.5x overstatement (e.g. a true 201ms sample reads as the
    /// 250ms bucket) and tightens to +/-1ms at the low end. The overflow
    /// bucket (>5000ms) reports the observed `max_ms` instead of a bound,
    /// since it has none.
    fn quantile_ms(&self, p: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let target = ((self.count as f64) * p).ceil().max(1.0) as u64;
        let mut cumulative = 0u64;
        for (i, &bucket_count) in self.buckets.iter().enumerate() {
            cumulative += bucket_count;
            if cumulative >= target {
                return if i < BUCKET_BOUNDS_MS.len() {
                    BUCKET_BOUNDS_MS[i]
                } else {
                    self.max_ms
                };
            }
        }
        self.max_ms
    }
}

type RegistryKey = (Method, String);

static REGISTRY: OnceLock<Mutex<HashMap<RegistryKey, RouteStats>>> = OnceLock::new();
static STARTED_AT: OnceLock<SystemTime> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<RegistryKey, RouteStats>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record the process start time. Called once from [`crate::server::serve`]
/// so `started_at` reflects when the server actually came up rather than
/// whenever the first request (or first `/telemetry` read) happened to
/// land.
pub fn init() {
    STARTED_AT.get_or_init(SystemTime::now);
}

fn started_at() -> SystemTime {
    *STARTED_AT.get_or_init(SystemTime::now)
}

fn record(method: Method, route: String, status: StatusCode, duration: Duration) {
    // A poisoned lock still holds valid data (telemetry is best-effort, not
    // safety-critical); recover it rather than dropping the request's
    // handling on the floor for an unrelated panic elsewhere.
    let mut guard = registry().lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry((method, route))
        .or_default()
        .record(status, duration);
}

/// `axum::middleware::from_fn` layer: times the request and records it
/// against the matched route template. Content-free by construction — only
/// method, route template, status class, and duration are ever touched; no
/// bodies, headers, query strings, or path parameter values are read.
///
/// Must be wired with `Router::layer` (not `Router::route_layer`) so it
/// also wraps the router's catch-all 404 fallback — that's what lets
/// unmatched requests reach here at all and fall into the `"unmatched"`
/// bucket instead of silently escaping telemetry.
pub async fn record_request(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| UNMATCHED_ROUTE.to_string());
    let started = Instant::now();
    let response = next.run(req).await;
    let elapsed = started.elapsed();
    record(method, route, response.status(), elapsed);
    response
}

#[derive(Debug, Serialize)]
struct RouteSummary {
    method: String,
    route: String,
    count: u64,
    errors_4xx: u64,
    errors_5xx: u64,
    p50_ms: u64,
    p95_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Serialize)]
struct TelemetrySnapshot {
    started_at: chrono::DateTime<chrono::Utc>,
    routes: Vec<RouteSummary>,
}

/// Deterministically ordered by `(route, method)` so repeated calls diff
/// cleanly and clients don't see registry-internal (HashMap) ordering.
fn snapshot() -> TelemetrySnapshot {
    let guard = registry().lock().unwrap_or_else(|e| e.into_inner());
    let mut routes: Vec<RouteSummary> = guard
        .iter()
        .map(|((method, route), stats)| RouteSummary {
            method: method.to_string(),
            route: route.clone(),
            count: stats.count,
            errors_4xx: stats.errors_4xx,
            errors_5xx: stats.errors_5xx,
            p50_ms: stats.quantile_ms(0.5),
            p95_ms: stats.quantile_ms(0.95),
            max_ms: stats.max_ms,
        })
        .collect();
    drop(guard);
    routes.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.method.cmp(&b.method)));
    TelemetrySnapshot {
        started_at: started_at().into(),
        routes,
    }
}

/// `GET /api/v1/telemetry` — content-free latency/reliability summary.
/// Served in both editions (not gated behind `creator`). Auth mirrors every
/// other non-creator `/api/v1` route: a valid `X-Api-Key` header or a valid
/// `strivo_session` cookie (`routes::login::check_dual`).
pub async fn telemetry_handler(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if crate::routes::login::check_dual(&headers, &state.api_key, &state.session_secret).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    Json(snapshot()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    // ── Aggregator unit tests ───────────────────────────────────────────

    #[test]
    fn bucket_assignment_picks_first_bound_gte_duration() {
        let mut s = RouteStats::default();
        s.record(StatusCode::OK, Duration::from_millis(0)); // <=1 -> bucket 0
        s.record(StatusCode::OK, Duration::from_millis(1)); // <=1 -> bucket 0
        s.record(StatusCode::OK, Duration::from_millis(2)); // <=2 -> bucket 1
        s.record(StatusCode::OK, Duration::from_millis(6)); // <=10 -> bucket 3
        s.record(StatusCode::OK, Duration::from_millis(10_000)); // overflow
        assert_eq!(s.buckets[0], 2);
        assert_eq!(s.buckets[1], 1);
        assert_eq!(s.buckets[3], 1);
        assert_eq!(s.buckets[BUCKET_COUNT - 1], 1);
        assert_eq!(s.count, 5);
    }

    #[test]
    fn quantiles_land_in_documented_bucket_tolerance() {
        let mut s = RouteStats::default();
        for _ in 0..100 {
            s.record(StatusCode::OK, Duration::from_millis(100));
        }
        // All samples land exactly on the 100ms bound -> both quantiles
        // read exactly 100, the zero-error case.
        assert_eq!(s.quantile_ms(0.5), 100);
        assert_eq!(s.quantile_ms(0.95), 100);

        let mut mixed = RouteStats::default();
        for _ in 0..50 {
            mixed.record(StatusCode::OK, Duration::from_millis(90)); // bucket <=100
        }
        for _ in 0..50 {
            mixed.record(StatusCode::OK, Duration::from_millis(400)); // bucket <=500
        }
        // p50 falls at the boundary between the two halves; nearest-rank
        // picks the lower bucket's bound (100ms), 10ms above the true
        // p50 (90ms) — within the documented one-bucket tolerance.
        assert_eq!(mixed.quantile_ms(0.5), 100);
        // p95 is within the upper half -> the 500ms bucket bound, exactly
        // matching the documented worst case (true 400ms reads as 500ms).
        assert_eq!(mixed.quantile_ms(0.95), 500);
    }

    #[test]
    fn counts_4xx_and_5xx_separately_from_total() {
        let mut s = RouteStats::default();
        s.record(StatusCode::OK, Duration::from_millis(1));
        s.record(StatusCode::NOT_FOUND, Duration::from_millis(1));
        s.record(StatusCode::BAD_REQUEST, Duration::from_millis(1));
        s.record(StatusCode::INTERNAL_SERVER_ERROR, Duration::from_millis(1));
        assert_eq!(s.count, 4);
        assert_eq!(s.errors_4xx, 2);
        assert_eq!(s.errors_5xx, 1);
    }

    #[test]
    fn tracks_max_latency_across_records() {
        let mut s = RouteStats::default();
        s.record(StatusCode::OK, Duration::from_millis(5));
        s.record(StatusCode::OK, Duration::from_millis(4000));
        s.record(StatusCode::OK, Duration::from_millis(20));
        assert_eq!(s.max_ms, 4000);
    }

    #[test]
    fn snapshot_is_deterministically_ordered_by_route_then_method() {
        record(
            Method::POST,
            "/__telemetry_test__/order/zzz".into(),
            StatusCode::OK,
            Duration::from_millis(1),
        );
        record(
            Method::GET,
            "/__telemetry_test__/order/aaa".into(),
            StatusCode::OK,
            Duration::from_millis(1),
        );
        record(
            Method::GET,
            "/__telemetry_test__/order/zzz".into(),
            StatusCode::OK,
            Duration::from_millis(1),
        );
        let snap = snapshot();
        let idx = |route: &str, method: &str| {
            snap.routes
                .iter()
                .position(|r| r.route == route && r.method == method)
                .unwrap_or_else(|| panic!("missing {method} {route} in snapshot"))
        };
        let aaa_get = idx("/__telemetry_test__/order/aaa", "GET");
        let zzz_get = idx("/__telemetry_test__/order/zzz", "GET");
        let zzz_post = idx("/__telemetry_test__/order/zzz", "POST");
        // Route ordering wins first (aaa < zzz), then method within a tie.
        assert!(aaa_get < zzz_get);
        assert!(zzz_get < zzz_post);
    }

    #[test]
    fn concurrent_recording_is_thread_safe() {
        let route = "/__telemetry_test__/thread_safety".to_string();
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 200;
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let route = route.clone();
                std::thread::spawn(move || {
                    for i in 0..PER_THREAD {
                        record(
                            Method::GET,
                            route.clone(),
                            StatusCode::OK,
                            Duration::from_millis(i % 50 + 1),
                        );
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("recorder thread panicked");
        }
        let guard = registry().lock().unwrap();
        let stats = guard
            .get(&(Method::GET, route))
            .expect("route recorded by concurrent threads");
        assert_eq!(stats.count, THREADS * PER_THREAD);
    }

    // ── Router-level tests ──────────────────────────────────────────────

    async fn ok_handler() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn records_matched_route_template_not_concrete_path() {
        let test_route = "/__telemetry_test__/things/{id}";
        let router: Router<()> = Router::new()
            .route(test_route, get(ok_handler))
            .layer(axum::middleware::from_fn(record_request));

        let req = Request::builder()
            .uri("/__telemetry_test__/things/42")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let guard = registry().lock().unwrap();
        let stats = guard
            .get(&(Method::GET, test_route.to_string()))
            .expect("recorded under the route template");
        assert!(stats.count >= 1);
        assert!(!guard.contains_key(&(Method::GET, "/__telemetry_test__/things/42".to_string())));
    }

    #[tokio::test]
    async fn unmatched_requests_aggregate_under_bounded_unmatched_key() {
        let router: Router<()> = Router::new()
            .route("/__telemetry_test__/known", get(ok_handler))
            .layer(axum::middleware::from_fn(record_request));

        let before = registry()
            .lock()
            .unwrap()
            .get(&(Method::GET, UNMATCHED_ROUTE.to_string()))
            .map(|s| s.count)
            .unwrap_or(0);

        let req = Request::builder()
            .uri("/__telemetry_test__/does-not-exist-anywhere")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let after = registry()
            .lock()
            .unwrap()
            .get(&(Method::GET, UNMATCHED_ROUTE.to_string()))
            .map(|s| s.count)
            .unwrap_or(0);
        assert_eq!(after, before + 1);
    }

    fn test_state(api_key: &str) -> AppState {
        AppState {
            ipc: std::sync::Arc::new(crate::ipc_client::IpcClient::disconnected()),
            api_key: crate::auth::ApiKey(api_key.to_string()),
            config_path: None,
            config_write_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            session_secret: "telemetry-test-session-secret".to_string(),
            login_limiter: crate::ratelimit::LoginLimiter::new(),
            probe_cache: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            probe_slots: std::sync::Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    #[tokio::test]
    async fn telemetry_endpoint_requires_auth_and_returns_recorded_routes() {
        let test_route = "/__telemetry_test__/auth_probe";
        record(
            Method::GET,
            test_route.to_string(),
            StatusCode::OK,
            Duration::from_millis(3),
        );

        let state = test_state("telemetry-test-api-key");
        let router = Router::new()
            .route("/api/v1/telemetry", get(telemetry_handler))
            .with_state(state.clone());

        // No credentials -> 401, same as every other guarded /api/v1 route.
        let unauth = Request::builder()
            .uri("/api/v1/telemetry")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(unauth).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Valid X-Api-Key -> 200 with our test route's aggregate present.
        let auth = Request::builder()
            .uri("/api/v1/telemetry")
            .header("x-api-key", state.api_key.as_str())
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(auth).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body["started_at"].is_string());
        let routes = body["routes"].as_array().expect("routes array");
        let found = routes
            .iter()
            .find(|r| r["route"] == test_route && r["method"] == "GET")
            .expect("test route present in telemetry snapshot");
        assert!(found["count"].as_u64().unwrap() >= 1);
    }
}
