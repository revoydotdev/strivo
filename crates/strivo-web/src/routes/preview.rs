//! Proxy-backed preview reads for the recording library (report R02).
//!
//! A six-hour recording cannot be scrubbed by decoding it. These two routes
//! are the interaction surface that makes that possible, and they are the
//! routes a *browser* uses — the browser half of playback is out of reach here
//! because this Chromium cannot decode the fixture corpus at all (todo 43's
//! finding), so the measurement behind the 500 ms budget is the server path.
//!
//! Both routes go through [`strivo_core::media::preview::PreviewSurface`],
//! which reads a content-addressed proxy. Neither takes a filesystem path from
//! the client: the source is resolved from a recording id, exactly as
//! `routes::settings::recording_thumb` resolves one, so this module needs no
//! media-admission gate. A route that accepted a caller-supplied path would,
//! and would be Creator-only for that reason.
//!
//! What makes "no interaction path spawns an original-resolution decode"
//! checkable at *this* surface is `strivo_core::media::spawn`: the record is
//! process-wide, so a test can drive these handlers over HTTP and then read
//! what actually ran.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

use crate::routes::settings::{check_key, resolve_recording};
use crate::server::AppState;

/// The proxy shape the library serves. 1280x720 is the size report R02's
/// proxy-seeking target names; the constant lives in core, so the route and the
/// measurement cannot disagree about what "the proxy" is.
fn proxy_root() -> &'static std::path::PathBuf {
    static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| strivo_core::config::AppConfig::data_dir().join("preview"))
}

/// One surface per process, so the bounded preview cache is bounded *per
/// process* rather than once per request — a cache constructed per request
/// would have a bound but no reuse, which is the failure mode a bound is
/// supposed to prevent.
fn surface(
) -> &'static Result<Arc<tokio::sync::Mutex<strivo_core::media::preview::PreviewSurface>>, String> {
    static SURFACE: OnceLock<
        Result<Arc<tokio::sync::Mutex<strivo_core::media::preview::PreviewSurface>>, String>,
    > = OnceLock::new();
    SURFACE.get_or_init(|| {
        strivo_core::media::preview::PreviewSurface::new(
            proxy_root(),
            strivo_core::media::proxy::ProxySpec::default(),
        )
        .map(|s| Arc::new(tokio::sync::Mutex::new(s)))
        .map_err(|e| e.to_string())
    })
}

#[derive(Debug, Deserialize)]
pub struct FrameQuery {
    /// Position in the ORIGINAL timeline, in seconds.
    #[serde(default)]
    pub t: Option<f64>,
    /// Requested frame width. The proxy is always the decode; this only sizes
    /// the returned image.
    #[serde(default)]
    pub w: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct WaveformQuery {
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub points: Option<usize>,
}

fn png_response(bytes: Vec<u8>) -> axum::response::Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/png"),
            // The key already encodes the content, so a browser may hold one
            // for a day; a changed source changes the key and therefore the URL.
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        bytes,
    )
        .into_response()
}

fn json_response(value: serde_json::Value) -> axum::response::Response {
    (axum::http::StatusCode::OK, axum::Json(value)).into_response()
}

/// `GET /api/v1/recordings/{id}/preview` — one frame at `t`, decoded from the
/// proxy.
pub async fn recording_preview(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<FrameQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let surface = match surface() {
        Ok(s) => s,
        Err(why) => {
            return crate::problem::Problem::unavailable(format!(
                "preview cache unavailable: {why}"
            ))
            .into_response()
        }
    };
    let Some((source, fingerprint)) = source_of(&state, id).await else {
        return crate::problem::Problem::not_found("no preview: recording not found")
            .into_response();
    };
    let t = q.t.unwrap_or(0.0);
    if !t.is_finite() || t < 0.0 {
        return crate::problem::Problem::bad_request("t must be a non-negative number of seconds")
            .into_response();
    }
    let width = q.w.unwrap_or(640).clamp(16, 3840);
    let mut guard = surface.lock().await;
    match guard.frame(&source, &fingerprint, t, width).await {
        Ok(path) => match tokio::fs::read(&path).await {
            Ok(bytes) => png_response(bytes),
            Err(e) => {
                crate::problem::Problem::unavailable(format!("preview read: {e}")).into_response()
            }
        },
        Err(e) => crate::problem::Problem::unavailable(format!("preview: {e}")).into_response(),
    }
}

/// `GET /api/v1/recordings/{id}/waveform` — peaks over `[start, end)` from the
/// pyramid built off the proxy's audio.
pub async fn recording_waveform(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<WaveformQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let surface = match surface() {
        Ok(s) => s,
        Err(why) => {
            return crate::problem::Problem::unavailable(format!(
                "preview cache unavailable: {why}"
            ))
            .into_response()
        }
    };
    let Some((source, fingerprint)) = source_of(&state, id).await else {
        return crate::problem::Problem::not_found("no waveform: recording not found")
            .into_response();
    };
    let guard = surface.lock().await;
    let pyramid = match guard.waveform(&source, &fingerprint).await {
        Ok(p) => p,
        Err(e) => {
            return crate::problem::Problem::unavailable(format!("waveform: {e}")).into_response()
        }
    };
    let start = q.start.unwrap_or(0.0).max(0.0);
    let end = q
        .end
        .unwrap_or(pyramid.duration_secs)
        .min(pyramid.duration_secs.max(start));
    let points = q.points.unwrap_or(1_000).clamp(1, 65_536);
    let level = pyramid.level_for_points(points);
    let peaks = pyramid.peaks(level, start, end.max(start));
    json_response(serde_json::json!({
        "sample_rate": pyramid.sample_rate,
        "level": level,
        "levels": pyramid.levels.len(),
        "start_secs": start,
        "end_secs": end,
        "peaks": peaks,
    }))
}

/// Resolve a recording id to its source path and the CONTENT fingerprint of
/// that file.
///
/// The fingerprint is the cache key todo 33 defines. It is computed here,
/// against the resolved path, because a PVR install has no research store to
/// carry one — see `strivo_core::media::proxy::content_fingerprint` for why the
/// rule has one implementation per crate rather than one per workspace.
async fn source_of(state: &AppState, id: Uuid) -> Option<(std::path::PathBuf, String)> {
    let job = resolve_recording(state, id).await.ok()?;
    if job.bytes_written == 0 {
        return None;
    }
    let path = std::path::PathBuf::from(&job.output_path);
    if !path.exists() {
        return None;
    }
    let fingerprint = strivo_core::media::proxy::content_fingerprint(&path).ok()?;
    Some((path, fingerprint))
}

/// Registered on the PVR router: preview navigation is core library
/// functionality, and its vocabulary is not Creator-only.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/recordings/{id}/preview", get(recording_preview))
        .route("/api/v1/recordings/{id}/waveform", get(recording_waveform))
}

/// The routes this module serves, for the route-coverage check.
pub fn paths() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("/api/v1/recordings/{id}/preview", "GET"),
        ("/api/v1/recordings/{id}/waveform", "GET"),
    ])
}
