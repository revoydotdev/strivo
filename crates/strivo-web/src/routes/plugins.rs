//! `/api/v1/plugins/*` read surface for the first-party plugins.
//!
//! The web server is a separate process from the daemon (where the plugin
//! registry actually lives), so action dispatch — "Re-transcribe", "Re-archive
//! channel" — flows over IPC via [`crate::routes::api`]'s `plugin_rpc`. But the
//! plugins also persist everything they produce to SQLite under
//! `<data_dir>/plugins/<name>/`. For *reads* we open those DBs read-only and
//! reuse the plugin crate's query functions directly, which avoids inventing a
//! query/response IPC protocol for what is fundamentally a co-located file read.
//!
//! Every endpoint is auth-gated (session cookie or `X-Api-Key`). When a
//! plugin's DB doesn't exist yet — the plugin has produced nothing — the
//! handlers return `{ "available": false, … }` with an empty payload so the
//! SPA can render a friendly empty state instead of an error.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

use crate::problem::Problem;
use crate::server::AppState;

fn authed(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

// ── DB path resolution ───────────────────────────────────────────────

fn plugins_root() -> PathBuf {
    strivo_core::config::AppConfig::data_dir().join("plugins")
}

fn crunchr_db() -> PathBuf {
    plugins_root().join("crunchr").join("crunchr.db")
}

fn archiver_db() -> PathBuf {
    plugins_root().join("archiver").join("archiver.db")
}

/// Viewguard's `init` joins `plugins/viewguard` onto a data_dir that already
/// ends in `plugins/viewguard`, so the live DB nests twice. Prefer that path
/// but fall back to the un-nested location so this keeps working if the plugin
/// ever corrects the join.
fn viewguard_db() -> Option<PathBuf> {
    let nested = plugins_root()
        .join("viewguard")
        .join("plugins")
        .join("viewguard")
        .join("viewguard.db");
    let flat = plugins_root().join("viewguard").join("viewguard.db");
    if nested.exists() {
        Some(nested)
    } else if flat.exists() {
        Some(flat)
    } else {
        None
    }
}

/// `POST /api/v1/plugins/chapters/<recording_id>` — generate (or
/// re-generate) chapter markers for the given recording. Reads the
/// Crunchr SQLite, runs the heuristic chapter builder, caches result
/// in chapters.db, returns the chapter set inline.
async fn chapters_generate(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("chapters") {
        return r;
    }
    let crunchr_path = crunchr_db();
    if !crunchr_path.exists() {
        return Problem::not_found("crunchr DB not initialised").into_response();
    }
    let req = strivo_chapters::ChapterRequest {
        recording_id: recording_id.clone(),
        min_seconds: None,
        cos_threshold: None,
    };
    let chapters = match strivo_chapters::generate_chapters(
        &crunchr_path,
        &req,
        &strivo_chapters::KeywordTitler,
    ) {
        Ok(c) => c,
        Err(e) => return Problem::internal(format!("chapters: {e}")).into_response(),
    };
    let description = strivo_chapters::format_for_description(&chapters);
    Json(json!({
        "recording_id": recording_id,
        "chapters": chapters,
        "description": description,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/cuepoints/<recording_id>` — extract (or
/// re-extract) scene-change cuepoints for a recording. ffmpeg full
/// pass; cached in cuepoints.db keyed on (recording_id, threshold).
async fn cuepoints_generate(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("cuepoints") {
        return r;
    }
    let id = match uuid::Uuid::parse_str(&recording_id) {
        Ok(u) => u,
        Err(_) => return Problem::bad_request("recording id must be a uuid").into_response(),
    };
    // Resolve the file path via persist DB — same shape as the remux
    // endpoint. No daemon round-trip needed; we only read the row.
    let jobs_db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let input_path = match strivo_core::recording::persist::PersistDb::open(&jobs_db) {
        Ok(db) => match db.load_recording_jobs().await {
            Ok(rows) => rows.into_iter().find(|j| j.id == id).map(|j| j.output_path),
            Err(_) => None,
        },
        Err(_) => None,
    };
    let Some(input) = input_path else {
        return Problem::not_found("recording not found").into_response();
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let threshold = strivo_cuepoints::DEFAULT_THRESHOLD;
    // Check the cache first.
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("cuepoints")
        .join("cuepoints.db");
    let store = match strivo_cuepoints::store::CuepointsStore::open(&store_path) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open cache: {e}")).into_response(),
    };
    if let Ok(Some(points)) = store.load(&recording_id, threshold) {
        return Json(json!({
            "recording_id": recording_id,
            "threshold": threshold,
            "points": points,
            "cached": true,
        }))
        .into_response();
    }
    let points = match strivo_cuepoints::extract_cuepoints(&input, threshold) {
        Ok(p) => p,
        Err(e) => return Problem::internal(format!("ffmpeg: {e}")).into_response(),
    };
    let set = strivo_cuepoints::CuepointSet {
        recording_id: recording_id.clone(),
        threshold,
        points: points.clone(),
    };
    let _ = store.save(&set);
    Json(json!({
        "recording_id": recording_id,
        "threshold": threshold,
        "points": points,
        "cached": false,
    }))
    .into_response()
}

/// Resolve the on-disk output path for a recording job from persist.
/// Shared by the cuepoints/clipper handlers — both need it.
async fn resolve_recording_path(recording_id: &str) -> Result<std::path::PathBuf, String> {
    let id = uuid::Uuid::parse_str(recording_id).map_err(|_| "id must be uuid".to_string())?;
    let jobs_db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let path = match strivo_core::recording::persist::PersistDb::open(&jobs_db) {
        Ok(db) => match db.load_recording_jobs().await {
            Ok(rows) => rows.into_iter().find(|j| j.id == id).map(|j| j.output_path),
            Err(_) => None,
        },
        Err(_) => None,
    };
    path.ok_or_else(|| "recording not found".to_string())
}

#[derive(Debug, Deserialize, Default)]
struct ClipExtractPayload {
    start_sec: f32,
    duration_sec: Option<f32>,
    /// Filename stem the user wants; sanitised server-side.
    #[serde(default)]
    stem: String,
}

/// `POST /api/v1/plugins/clipper/<recording_id>/analyze` — score
/// highlight candidates for the recording. Reuses the cuepoint cache
/// (iter 4) so the analyzer is cheap on a previously-analysed VOD.
async fn clipper_analyze(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("clipper") {
        return r;
    }
    // We need cuepoints to score. If they're not cached we extract
    // them now — same path the standalone Cuepoints button uses.
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let threshold = strivo_cuepoints::DEFAULT_THRESHOLD;
    let cp_store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("cuepoints")
        .join("cuepoints.db");
    let cp_store = match strivo_cuepoints::store::CuepointsStore::open(&cp_store_path) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open cuepoints cache: {e}")).into_response(),
    };
    let cuepoints = match cp_store.load(&recording_id, threshold) {
        Ok(Some(c)) => c,
        _ => match strivo_cuepoints::extract_cuepoints(&input, threshold) {
            Ok(c) => {
                let set = strivo_cuepoints::CuepointSet {
                    recording_id: recording_id.clone(),
                    threshold,
                    points: c.clone(),
                };
                let _ = cp_store.save(&set);
                c
            }
            Err(e) => return Problem::internal(format!("cuepoints: {e}")).into_response(),
        },
    };
    let window = strivo_clipper::DEFAULT_WINDOW_SECS;
    let highlights =
        strivo_clipper::score_highlights(&cuepoints, window, strivo_clipper::DEFAULT_TOP_K);
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    if let Ok(store) = strivo_clipper::store::ClipperStore::open(&store_path) {
        let _ = store.save_highlights(&recording_id, window, &highlights);
    }
    Json(json!({
        "recording_id": recording_id,
        "window_secs": window,
        "highlights": highlights,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/clipper/<recording_id>/extract` — cut the
/// requested clip (lossless ffmpeg `-c copy`) and return the result.
async fn clipper_extract(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<ClipExtractPayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("clipper") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let dur = body
        .duration_sec
        .unwrap_or(strivo_clipper::DEFAULT_CLIP_DURATION_SECS);
    let (start, duration) = strivo_clipper::clamp_request(body.start_sec, dur, None);
    let extension = input.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let fallback_stem = format!("clip_{:.0}", start);
    let safe_stem = sanitize_stem(if body.stem.is_empty() {
        &fallback_stem
    } else {
        &body.stem
    });
    // Land under <recording_parent>/clips/<stem>.<ext>.
    let clip_dir = input
        .parent()
        .map(|p| p.join("clips"))
        .unwrap_or_else(|| std::path::PathBuf::from("./clips"));
    let output = clip_dir.join(format!("{safe_stem}.{extension}"));
    let bytes = match strivo_clipper::extract_clip(&input, &output, start, duration) {
        Ok(n) => n,
        Err(e) => return Problem::internal(format!("ffmpeg: {e}")).into_response(),
    };
    let result = strivo_clipper::ClipResult {
        recording_id: recording_id.clone(),
        clip_path: output.to_string_lossy().to_string(),
        start_sec: start,
        duration_sec: duration,
        bytes,
    };
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    if let Ok(store) = strivo_clipper::store::ClipperStore::open(&store_path) {
        let _ = store.save_clip(&result);
    }
    Json(result).into_response()
}

/// `GET /api/v1/plugins/clipper/<recording_id>/clips` — list previously
/// cut clips for a recording (powers the SPA "Cut clips" list).
async fn clipper_list_clips(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("clipper") {
        return r;
    }
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    let store = match strivo_clipper::store::ClipperStore::open(&store_path) {
        Ok(s) => s,
        Err(_) => return Json(json!({ "clips": [] })).into_response(),
    };
    let clips = store.list_clips(&recording_id).unwrap_or_default();
    Json(json!({ "clips": clips })).into_response()
}

fn sanitize_stem(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "clip".to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Deserialize, Default)]
struct ThumbsPayload {
    /// Source of timestamps to sample at. "cuepoints" reuses the
    /// existing cuepoint set; "even" walks the recording at
    /// `interval_secs` boundaries; "list" takes an explicit list.
    #[serde(default = "default_thumb_source")]
    source: String,
    #[serde(default)]
    times: Vec<f32>,
    #[serde(default)]
    interval_secs: Option<f32>,
    #[serde(default)]
    facecam: Option<strivo_thumbnails::FacecamCorner>,
}

fn default_thumb_source() -> String {
    "cuepoints".to_string()
}

/// `POST /api/v1/plugins/thumbnails/<recording_id>` — generate thumbnail
/// candidates. Source = cuepoints / even / explicit-list; optional
/// facecam corner emits a 9:16 vertical crop per pick.
async fn thumbnails_generate(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<ThumbsPayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("thumbnails") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    // Build the timestamp list.
    let timestamps: Vec<f32> = match body.source.as_str() {
        "list" => body.times.clone(),
        "even" => {
            let interval = body.interval_secs.unwrap_or(600.0).max(15.0);
            // ffprobe duration to know how far to walk.
            let duration = probe_duration(&input).unwrap_or(3600.0);
            let mut out = Vec::new();
            let mut t = 0.0_f32;
            while t < duration {
                out.push(t);
                t += interval;
            }
            out
        }
        _ /* "cuepoints" */ => {
            let cp_path = strivo_core::config::AppConfig::data_dir()
                .join("plugins")
                .join("cuepoints")
                .join("cuepoints.db");
            let store = strivo_cuepoints::store::CuepointsStore::open(&cp_path).ok();
            let cps = store
                .and_then(|s| s.load(&recording_id, strivo_cuepoints::DEFAULT_THRESHOLD).ok().flatten())
                .unwrap_or_default();
            if cps.is_empty() {
                // Fall back to a handful of even samples so the user gets *something*.
                let duration = probe_duration(&input).unwrap_or(3600.0);
                let n = 8;
                (0..n).map(|i| duration * (i as f32 + 0.5) / n as f32).collect()
            } else {
                cps.iter().map(|c| c.time_sec).collect()
            }
        }
    };
    // Cap to a sensible upper bound — running ffmpeg 200 times stalls
    // the UI thread. SPA can request a smaller batch if it wants more.
    let timestamps: Vec<f32> = timestamps.into_iter().take(24).collect();
    if timestamps.is_empty() {
        return Problem::bad_request("no timestamps to sample").into_response();
    }
    // ffprobe resolution for cropping.
    let (w, h) = probe_resolution(&input).unwrap_or((1920, 1080));

    let out_dir = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("thumbnails")
        .join(&recording_id);
    let stem = "candidate";
    let opts = strivo_thumbnails::GenerateOptions {
        timestamps,
        out_dir,
        stem: stem.to_string(),
        facecam: body.facecam,
    };
    let result = match strivo_thumbnails::generate_candidates(&input, (w, h), &opts, &recording_id)
    {
        Ok(r) => r,
        Err(e) => return Problem::internal(format!("thumbnails: {e}")).into_response(),
    };

    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("thumbnails")
        .join("thumbnails.db");
    if let Ok(store) = strivo_thumbnails::store::ThumbnailsStore::open(&store_path) {
        let _ = store.save(&recording_id, stem, &result.candidates);
    }
    Json(json!({
        "recording_id": recording_id,
        "candidates": result.candidates,
    }))
    .into_response()
}

/// Shell out to ffprobe for the duration in seconds.
fn probe_duration(input: &std::path::Path) -> Option<f32> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input)
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse().ok()
}

/// Shell out to ffprobe for the video resolution.
fn probe_resolution(input: &std::path::Path) -> Option<(u32, u32)> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    let parts: Vec<&str> = s.trim().split(',').collect();
    if parts.len() == 2 {
        Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
    } else {
        None
    }
}

/// `GET /api/v1/plugins/thumbnails/<recording_id>/<stem>` — list the
/// cached candidate set so a page reload doesn't lose state.
async fn thumbnails_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((recording_id, stem)): Path<(String, String)>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("thumbnails") {
        return r;
    }
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("thumbnails")
        .join("thumbnails.db");
    let store = match strivo_thumbnails::store::ThumbnailsStore::open(&store_path) {
        Ok(s) => s,
        Err(_) => return Json(json!({ "candidates": [] })).into_response(),
    };
    let candidates = store
        .load(&recording_id, &stem)
        .ok()
        .flatten()
        .unwrap_or_default();
    Json(json!({ "candidates": candidates })).into_response()
}

/// `GET /api/v1/plugins/thumbnails/file?p=<absolute_path>` — serve a
/// generated thumbnail file. We refuse anything outside the
/// thumbnails data dir so the route can't be used as a generic
/// file-read.
async fn thumbnails_serve_file(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<ThumbFilePayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("thumbnails") {
        return r;
    }
    let root = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("thumbnails");
    let path = std::path::PathBuf::from(&q.p);
    // Canonicalise both so symlinks can't escape.
    let canon_root = match root.canonicalize() {
        Ok(p) => p,
        Err(_) => return Problem::internal("thumb root").into_response(),
    };
    let canon_path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return Problem::not_found("thumb").into_response(),
    };
    if !canon_path.starts_with(&canon_root) {
        return Problem::bad_request("path outside thumbnails dir").into_response();
    }
    let body = match std::fs::read(&canon_path) {
        Ok(b) => b,
        Err(_) => return Problem::not_found("thumb").into_response(),
    };
    let mime = if canon_path.extension().and_then(|e| e.to_str()) == Some("jpg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };
    ([(axum::http::header::CONTENT_TYPE, mime)], body).into_response()
}

#[derive(Debug, Deserialize)]
struct ThumbFilePayload {
    p: String,
}

#[derive(Debug, Deserialize)]
struct InsightsCompareQuery {
    /// Comma-separated list of recording UUIDs (exactly two; first two
    /// taken if more are supplied).
    recs: String,
    #[serde(default = "default_compare_limit")]
    limit: u32,
    #[serde(default)]
    include_stopwords: bool,
}

fn default_compare_limit() -> u32 {
    50
}

/// `GET /api/v1/plugins/insights/compare?recs=A,B` — pull each
/// recording's top-N word list from Crunchr and run the comparator.
/// Returns the shared / only_a / only_b sets + Jaccard score.
async fn insights_compare(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<InsightsCompareQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    let ids: Vec<String> = q
        .recs
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.len() < 2 {
        return Problem::bad_request("recs= must list at least two recording ids").into_response();
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let limit = q.limit.clamp(10, 500) as usize;
    let fetch = |rid: &str| -> Vec<strivo_insights_compare::WordCount> {
        strivo_plugins::insights::frequency::top_words_for_recording(
            &conn,
            rid,
            limit,
            q.include_stopwords,
        )
        .ok()
        .unwrap_or_default()
        .into_iter()
        .map(|w| strivo_insights_compare::WordCount {
            word: w.word,
            count: w.count as u64,
        })
        .collect()
    };
    let a = fetch(&ids[0]);
    let b = fetch(&ids[1]);
    let cmp = strivo_insights_compare::compare_words(&a, &b);
    Json(json!({
        "recording_a": ids[0],
        "recording_b": ids[1],
        "limit": limit,
        "comparison": cmp,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct RetentionQuery {
    #[serde(default = "default_bucket_secs")]
    bucket_secs: f32,
}

fn default_bucket_secs() -> f32 {
    30.0
}

/// `GET /api/v1/plugins/insights/retention/<recording_id>` — bucket
/// transcript activity + cuepoint density into a retention curve.
async fn insights_retention(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<RetentionQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    // Pull Crunchr segments.
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let detail = match strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id) {
        Ok(Some(d)) => d,
        Ok(None) => return Problem::not_found("recording not transcribed").into_response(),
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };
    let segments: Vec<strivo_insights_compare::Segment> = detail
        .segments
        .iter()
        .map(|s| {
            // Word count proxy: split on whitespace. Cheap, deterministic.
            let words = s.text.split_whitespace().count() as u32;
            strivo_insights_compare::Segment {
                start_sec: s.start_sec as f32,
                end_sec: s.end_sec as f32,
                word_count: words,
            }
        })
        .collect();
    // Pull cuepoints for the same recording (best-effort).
    let cp_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("cuepoints")
        .join("cuepoints.db");
    let cuepoint_times: Vec<f32> = strivo_cuepoints::store::CuepointsStore::open(&cp_path)
        .ok()
        .and_then(|store| {
            store
                .load(&recording_id, strivo_cuepoints::DEFAULT_THRESHOLD)
                .ok()
                .flatten()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.time_sec)
        .collect();
    // Duration: max segment end_sec; clamp to a sane floor so even
    // a 3-second test recording yields a curve.
    let duration = segments
        .iter()
        .map(|s| s.end_sec)
        .fold(0.0_f32, f32::max)
        .max(q.bucket_secs * 2.0);
    let bucket = q.bucket_secs.max(5.0);
    let curve =
        strivo_insights_compare::compute_retention(&segments, &cuepoint_times, duration, bucket);
    Json(json!({
        "recording_id": recording_id,
        "duration_sec": duration,
        "bucket_secs": bucket,
        "points": curve,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct CaptionsQuery {
    /// Output format. One of `srt`, `vtt`, `txt`.
    #[serde(default = "default_captions_fmt")]
    fmt: String,
    /// Optional target language. Default `en` = identity.
    #[serde(default = "default_captions_lang")]
    lang: String,
}

fn default_captions_fmt() -> String {
    "srt".to_string()
}
fn default_captions_lang() -> String {
    "en".to_string()
}

/// `GET /api/v1/plugins/captions/<recording_id>?fmt=srt&lang=en` —
/// emit a caption file for the recording in the requested format. The
/// `lang` knob is currently routed through an identity translator; the
/// pluggable `Translator` trait will get a real backend in a follow-up.
async fn captions_export(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<CaptionsQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("captions") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let detail = match strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id) {
        Ok(Some(d)) => d,
        Ok(None) => return Problem::not_found("recording not transcribed").into_response(),
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };
    let segments: Vec<strivo_captions::Segment> = detail
        .segments
        .iter()
        .map(|s| strivo_captions::Segment {
            start_sec: s.start_sec as f32,
            end_sec: s.end_sec as f32,
            text: s.text.clone(),
            speaker: s.speaker.clone(),
        })
        .collect();
    // Apply translation. Today only IdentityTranslator ships; a future
    // iteration registers real backends (NLLB / Argos / OpenAI).
    let translator = strivo_captions::IdentityTranslator;
    let translated = match strivo_captions::apply_translation(&segments, &translator) {
        Ok(out) => out,
        Err(e) => return Problem::internal(format!("translate: {e}")).into_response(),
    };
    let (body, mime, ext) = match q.fmt.as_str() {
        "vtt" => (strivo_captions::to_vtt(&translated), "text/vtt", "vtt"),
        "txt" => (strivo_captions::to_txt(&translated), "text/plain", "txt"),
        "ass" => {
            // Pull the styled-subtitles spec from the plugin store if the
            // user has saved customisations; otherwise use defaults. Word
            // timings come from the Crunchr detail when available.
            let style: strivo_captions::AssStyle = std::fs::read_to_string(captions_style_path(&recording_id))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            // Per-recording word timings are loaded on demand from the
            // crunchr SQLite; the detail's segments carry `words: Option<…>`
            // which is None until hydrated. Pull them now so karaoke
            // exports get word-level highlight.
            let karaoke: Vec<strivo_captions::KaraokeSegment> = detail
                .segments
                .iter()
                .filter_map(|s| {
                    let words = s.words.as_ref()?;
                    if words.is_empty() {
                        return None;
                    }
                    Some(strivo_captions::KaraokeSegment {
                        start_sec: s.start_sec as f32,
                        end_sec: s.end_sec as f32,
                        speaker: s.speaker.clone(),
                        // Crunchr's wire shape uses single-letter fields
                        // (w, s, e, c) to keep the SQLite blob tight.
                        words: words
                            .iter()
                            .map(|w| strivo_captions::WordTiming {
                                text: w.w.clone(),
                                start_sec: w.s as f32,
                                end_sec: w.e as f32,
                            })
                            .collect(),
                    })
                })
                .collect();
            (
                strivo_captions::to_ass(&translated, &style, &karaoke),
                "text/x-ssa",
                "ass",
            )
        }
        _ /* srt */ => (
            strivo_captions::to_srt(&translated),
            "application/x-subrip",
            "srt",
        ),
    };
    let safe = recording_id.replace(
        |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
        "_",
    );
    let filename = format!("{safe}.{}.{ext}", q.lang);
    (
        [
            (axum::http::header::CONTENT_TYPE, mime),
            (
                axum::http::header::CONTENT_DISPOSITION,
                Box::leak(format!("attachment; filename=\"{filename}\"").into_boxed_str()),
            ),
        ],
        body,
    )
        .into_response()
}

/// `GET /api/v1/plugins/multitrack/<recording_id>` — list the audio
/// tracks present in the recording. Pure ffprobe call — fast.
async fn multitrack_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("multitrack") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    match strivo_multitrack::probe_audio_tracks(&input) {
        Ok(tracks) => Json(json!({
            "recording_id": recording_id,
            "tracks": tracks,
        }))
        .into_response(),
        Err(e) => Problem::internal(format!("ffprobe: {e}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct MultitrackExtractPayload {
    /// Stream index of the track to extract (matches AudioTrack.index).
    track_index: u32,
    /// Optional filename stem; defaults to "track_<idx>".
    #[serde(default)]
    stem: String,
    /// Optional output extension — overrides the source codec's
    /// natural extension when set. Caller is responsible for picking
    /// something the codec actually fits into.
    #[serde(default)]
    ext: String,
}

/// `POST /api/v1/plugins/multitrack/<recording_id>/extract` — cut a
/// single audio track to a standalone file in `<recording>/tracks/`.
async fn multitrack_extract(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<MultitrackExtractPayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("multitrack") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    // Pick a sensible extension. Default to the source file's extension
    // since `-c copy` keeps the codec; user can override via the payload.
    let src_ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mka");
    let ext = if body.ext.is_empty() {
        src_ext
    } else {
        body.ext.as_str()
    };
    let stem = if body.stem.is_empty() {
        format!("track_{}", body.track_index)
    } else {
        body.stem.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-',
            "_",
        )
    };
    let out_dir = input
        .parent()
        .map(|p| p.join("tracks"))
        .unwrap_or_else(|| std::path::PathBuf::from("./tracks"));
    let output = out_dir.join(format!("{stem}.{ext}"));
    let bytes = match strivo_multitrack::extract_track(&input, body.track_index, &output) {
        Ok(b) => b,
        Err(e) => return Problem::internal(format!("ffmpeg: {e}")).into_response(),
    };
    Json(json!({
        "recording_id": recording_id,
        "track_index": body.track_index,
        "output_path": output.to_string_lossy(),
        "bytes": bytes,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct BrandsafeQuery {
    /// Comma-separated platform list to consult restricted-game allow
    /// lists for. Defaults to `Twitch,YouTube` — the two big surfaces.
    #[serde(default)]
    platforms: Option<String>,
    /// Category / game name override. Without this we use whatever
    /// the recording's source channel name is, which isn't ideal but
    /// keeps the surface useful before the streamer has tagged.
    #[serde(default)]
    category: Option<String>,
}

/// `GET /api/v1/plugins/brandsafe/<recording_id>` — run every scanner
/// across the transcript + category + platforms and return verdicts.
async fn brandsafe_scan(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<BrandsafeQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("brandsafe") {
        return r;
    }

    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let detail = match strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id) {
        Ok(Some(d)) => d,
        Ok(None) => return Problem::not_found("recording not transcribed").into_response(),
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };
    let segments: Vec<strivo_brandsafe::Segment> = detail
        .segments
        .iter()
        .map(|s| strivo_brandsafe::Segment {
            start_sec: s.start_sec as f32,
            end_sec: s.end_sec as f32,
            text: s.text.clone(),
        })
        .collect();
    // Platforms: parse the comma list or fall back to the big two.
    let platforms_raw: String = q.platforms.unwrap_or_else(|| "Twitch,YouTube".to_string());
    let platforms: Vec<&str> = platforms_raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    // Category: query string wins; fall back to channel_name as a
    // proxy. Better than empty for the UX.
    let category = q.category.unwrap_or(detail.channel_name.clone());
    let verdicts = strivo_brandsafe::scan_all(&segments, &category, &platforms);
    Json(json!({
        "recording_id": recording_id,
        "category": category,
        "platforms": platforms,
        "verdicts": verdicts,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/reuse/<recording_id>/generate` — build the
/// default-format draft set for a recording by composing the existing
/// Crunchr / Clipper / Chapters outputs. Cached in reuse.db.
async fn reuse_generate(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("reuse") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    // Pull title + channel + duration via the persist row + a quick
    // ffprobe — both are cheap.
    let (title, channel_name) = match resolve_recording_meta(&recording_id).await {
        Some((t, c)) => (t, c),
        None => (recording_id.clone(), String::new()),
    };
    let duration_sec = probe_duration(&input).unwrap_or(0.0);

    let crunchr_conn = open_ro(&crunchr_db());
    // Crunchr summary + top words / topics, when available. Best-effort.
    let mut summary = String::new();
    let mut topics: Vec<String> = Vec::new();
    let mut top_words: Vec<String> = Vec::new();
    if let Some(conn) = &crunchr_conn {
        if let Ok(Some(detail)) = strivo_plugins::crunchr::db::recording_detail(conn, &recording_id)
        {
            summary = detail.summary.unwrap_or_default();
            topics = detail.topics;
        }
        if let Ok(words) = strivo_plugins::insights::frequency::top_words_for_recording(
            conn,
            &recording_id,
            30,
            false,
        ) {
            top_words = words.into_iter().map(|w| w.word).collect();
        }
    }
    // Clipper highlights, when available.
    let clipper_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    let clip_starts: Vec<f32> = strivo_clipper::store::ClipperStore::open(&clipper_path)
        .ok()
        .and_then(|s| {
            s.load_highlights(&recording_id, strivo_clipper::DEFAULT_WINDOW_SECS)
                .ok()
                .flatten()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.time_sec)
        .collect();
    // Chapters block — call the same algorithm chapters_generate would
    // produce, but inline so reuse doesn't depend on its REST surface.
    let chapters_block = if crunchr_path_exists() {
        let req = strivo_chapters::ChapterRequest {
            recording_id: recording_id.clone(),
            min_seconds: None,
            cos_threshold: None,
        };
        strivo_chapters::generate_chapters(&crunchr_db(), &req, &strivo_chapters::KeywordTitler)
            .ok()
            .map(|c| strivo_chapters::format_for_description(&c))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let src = strivo_reuse::SourceRecording {
        recording_id: recording_id.clone(),
        title,
        channel_name,
        source_path: input.to_string_lossy().to_string(),
        duration_sec,
    };
    let inputs = strivo_reuse::DraftInputs {
        top_words,
        topics,
        clip_starts,
        chapters_block,
        summary,
    };
    let drafts = strivo_reuse::generate_drafts(&src, &inputs);
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("reuse")
        .join("reuse.db");
    if let Ok(store) = strivo_reuse::store::ReuseStore::open(&store_path) {
        let _ = store.save_set(&recording_id, &drafts);
    }
    Json(json!({
        "recording_id": recording_id,
        "drafts": drafts,
    }))
    .into_response()
}

/// `GET /api/v1/plugins/reuse/<recording_id>` — list cached drafts.
async fn reuse_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("reuse") {
        return r;
    }
    let store_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("reuse")
        .join("reuse.db");
    let drafts = strivo_reuse::store::ReuseStore::open(&store_path)
        .ok()
        .and_then(|s| s.list(&recording_id).ok())
        .unwrap_or_default();
    Json(json!({ "drafts": drafts })).into_response()
}

fn crunchr_path_exists() -> bool {
    crunchr_db().exists()
}

/// Best-effort title + channel lookup via the persist DB.
async fn resolve_recording_meta(recording_id: &str) -> Option<(String, String)> {
    let id = uuid::Uuid::parse_str(recording_id).ok()?;
    let jobs_db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let db = strivo_core::recording::persist::PersistDb::open(&jobs_db).ok()?;
    let rows = db.load_recording_jobs().await.ok()?;
    rows.into_iter().find(|j| j.id == id).map(|j| {
        (
            j.stream_title.unwrap_or_else(|| j.channel_name.clone()),
            j.channel_name,
        )
    })
}

/// `GET /api/v1/plugins/casebook/<recording_id>?fmt=json|markdown` —
/// compose the post-stream Casebook. Pulls Crunchr/Insights/Chapters/
/// Clipper/Brandsafe/Viewguard results and folds them into a report.
async fn casebook_generate(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<CasebookQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("casebook") {
        return r;
    }

    let crunchr_conn = open_ro(&crunchr_db());
    // Title + channel + started_at from persist; duration from ffprobe.
    let (mut title, channel_name, started_at) =
        match resolve_recording_meta_full(&recording_id).await {
            Some((t, c, s)) => (t, c, s),
            None => (recording_id.clone(), String::new(), None),
        };
    let input_path = resolve_recording_path(&recording_id).await.ok();
    let duration_sec = input_path
        .as_ref()
        .and_then(|p| probe_duration(p))
        .unwrap_or(0.0);

    // Crunchr summary + topics.
    let mut summary = String::new();
    let mut topics: Vec<String> = Vec::new();
    let mut top_words: Vec<strivo_casebook::WordCount> = Vec::new();
    if let Some(conn) = &crunchr_conn {
        if let Ok(Some(detail)) = strivo_plugins::crunchr::db::recording_detail(conn, &recording_id)
        {
            summary = detail.summary.unwrap_or_default();
            topics = detail.topics;
            if title == recording_id || title.is_empty() {
                title = detail.title;
            }
        }
        if let Ok(words) = strivo_plugins::insights::frequency::top_words_for_recording(
            conn,
            &recording_id,
            30,
            false,
        ) {
            top_words = words
                .into_iter()
                .map(|w| strivo_casebook::WordCount {
                    word: w.word,
                    count: w.count as u64,
                })
                .collect();
        }
    }

    // Chapters.
    let chapters: Vec<strivo_casebook::Chapter> = if crunchr_db().exists() {
        let req = strivo_chapters::ChapterRequest {
            recording_id: recording_id.clone(),
            min_seconds: None,
            cos_threshold: None,
        };
        strivo_chapters::generate_chapters(&crunchr_db(), &req, &strivo_chapters::KeywordTitler)
            .ok()
            .unwrap_or_default()
            .into_iter()
            .map(|c| strivo_casebook::Chapter {
                start_sec: c.start_sec,
                title: c.title,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Clipper highlights — cached.
    let clipper_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    let highlights: Vec<strivo_casebook::Highlight> =
        strivo_clipper::store::ClipperStore::open(&clipper_path)
            .ok()
            .and_then(|s| {
                s.load_highlights(&recording_id, strivo_clipper::DEFAULT_WINDOW_SECS)
                    .ok()
                    .flatten()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|h| strivo_casebook::Highlight {
                time_sec: h.time_sec,
                score: h.score,
            })
            .collect();

    // Brandsafe — best-effort scan now so the count is fresh.
    let mut bs_counts = strivo_casebook::BrandsafeCounts::default();
    if let Some(conn) = &crunchr_conn {
        if let Ok(Some(detail)) = strivo_plugins::crunchr::db::recording_detail(conn, &recording_id)
        {
            let segments: Vec<strivo_brandsafe::Segment> = detail
                .segments
                .iter()
                .map(|s| strivo_brandsafe::Segment {
                    start_sec: s.start_sec as f32,
                    end_sec: s.end_sec as f32,
                    text: s.text.clone(),
                })
                .collect();
            let verdicts =
                strivo_brandsafe::scan_all(&segments, &channel_name, &["Twitch", "YouTube"]);
            for v in &verdicts {
                match v.severity {
                    strivo_brandsafe::Severity::Critical => bs_counts.critical += 1,
                    strivo_brandsafe::Severity::High => bs_counts.high += 1,
                    strivo_brandsafe::Severity::Medium => bs_counts.medium += 1,
                    strivo_brandsafe::Severity::Low => bs_counts.low += 1,
                }
            }
        }
    }

    // Viewguard: try the viewguard DB if it's there.
    let viewbot_score: Option<f32> = viewguard_db()
        .as_deref()
        .and_then(open_ro)
        .and_then(|c| {
            // We can't reach into strivo_plugins::viewguard without a stable
            // shape — use the SQL we know lives in the DB. Fall back to
            // None if the schema isn't there yet.
            c.query_row(
                "SELECT final_score FROM verdicts ORDER BY stream_started_at DESC LIMIT 1",
                [],
                |r| r.get::<_, f64>(0),
            )
            .ok()
        })
        .map(|s| s as f32);

    let inputs = strivo_casebook::CasebookInputs {
        recording_id: recording_id.clone(),
        title,
        channel_name,
        started_at,
        duration_sec,
        summary,
        topics,
        top_words,
        chapters,
        highlights,
        viewbot_score,
        brandsafe_counts: bs_counts,
    };
    let report = strivo_casebook::compose_report(&inputs);

    let fmt = q.fmt.unwrap_or_else(|| "json".to_string());
    if fmt == "markdown" || fmt == "md" {
        let body = strivo_casebook::to_markdown(&report);
        let safe = recording_id.replace(
            |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
            "_",
        );
        let filename = format!("casebook_{safe}.md");
        return (
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "text/markdown; charset=utf-8",
                ),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    Box::leak(format!("attachment; filename=\"{filename}\"").into_boxed_str()),
                ),
            ],
            body,
        )
            .into_response();
    }
    Json(json!({
        "report": report,
        "markdown": strivo_casebook::to_markdown(&report),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct CasebookQuery {
    #[serde(default)]
    fmt: Option<String>,
}

async fn resolve_recording_meta_full(
    recording_id: &str,
) -> Option<(String, String, Option<String>)> {
    let id = uuid::Uuid::parse_str(recording_id).ok()?;
    let jobs_db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let db = strivo_core::recording::persist::PersistDb::open(&jobs_db).ok()?;
    let rows = db.load_recording_jobs().await.ok()?;
    rows.into_iter().find(|j| j.id == id).map(|j| {
        (
            j.stream_title
                .clone()
                .unwrap_or_else(|| j.channel_name.clone()),
            j.channel_name.clone(),
            Some(j.started_at.to_rfc3339()),
        )
    })
}

#[derive(Debug, Deserialize, Default)]
struct HeatmapQuery {
    #[serde(default)]
    bucket_secs: Option<f32>,
}

/// `GET /api/v1/plugins/heatmap/<recording_id>?bucket_secs=30` —
/// fuse transcript talk density, cuepoint action density, clipper
/// highlight scores, and brand-safety verdicts into a per-bucket
/// retention curve. Channels exposed for SPA breakdown rendering.
async fn heatmap_compute(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<HeatmapQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("heatmap") {
        return r;
    }

    let input_path = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    let probed = probe_duration(&input_path).unwrap_or(0.0);

    // Transcript segments.
    let mut segments: Vec<strivo_heatmap::TranscriptSegment> = Vec::new();
    let mut transcript_max_end: f32 = 0.0;
    if let Some(conn) = open_ro(&crunchr_db()) {
        if let Ok(Some(detail)) =
            strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id)
        {
            segments = detail
                .segments
                .iter()
                .map(|s| {
                    transcript_max_end = transcript_max_end.max(s.end_sec as f32);
                    strivo_heatmap::TranscriptSegment {
                        start_sec: s.start_sec as f32,
                        end_sec: s.end_sec as f32,
                        word_count: s.text.split_whitespace().count() as u32,
                    }
                })
                .collect();
        }
    }
    let duration_sec = if probed > 0.0 {
        probed
    } else {
        transcript_max_end
    };

    // Cuepoints — best-effort from cache.
    let cp_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("cuepoints")
        .join("cuepoints.db");
    let cuepoint_times: Vec<f32> = strivo_cuepoints::store::CuepointsStore::open(&cp_path)
        .ok()
        .and_then(|s| {
            s.load(&recording_id, strivo_cuepoints::DEFAULT_THRESHOLD)
                .ok()
                .flatten()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.time_sec)
        .collect();

    // Highlights — best-effort from cache.
    let clipper_path = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("clipper")
        .join("clipper.db");
    let highlights: Vec<strivo_heatmap::ScoredEvent> =
        strivo_clipper::store::ClipperStore::open(&clipper_path)
            .ok()
            .and_then(|s| {
                s.load_highlights(&recording_id, strivo_clipper::DEFAULT_WINDOW_SECS)
                    .ok()
                    .flatten()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|h| strivo_heatmap::ScoredEvent {
                time_sec: h.time_sec,
                score: h.score,
            })
            .collect();

    // Brand-safety — fresh scan; harvest the verdict times.
    let brandsafe_times: Vec<f32> = if !segments.is_empty() {
        let bs_segments: Vec<strivo_brandsafe::Segment> = segments
            .iter()
            .map(|s| strivo_brandsafe::Segment {
                start_sec: s.start_sec,
                end_sec: s.end_sec,
                text: String::new(), // text not used here; we want the segment shape
            })
            .collect();
        // To scan we need the actual text — re-pull from crunchr.
        let mut times = Vec::new();
        if let Some(conn) = open_ro(&crunchr_db()) {
            if let Ok(Some(detail)) =
                strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id)
            {
                let bs_segs: Vec<strivo_brandsafe::Segment> = detail
                    .segments
                    .iter()
                    .map(|s| strivo_brandsafe::Segment {
                        start_sec: s.start_sec as f32,
                        end_sec: s.end_sec as f32,
                        text: s.text.clone(),
                    })
                    .collect();
                let verdicts = strivo_brandsafe::scan_all(&bs_segs, "", &["Twitch", "YouTube"]);
                for v in &verdicts {
                    if let Some(t) = v.at_sec {
                        times.push(t);
                    }
                }
            }
        }
        let _ = bs_segments;
        times
    } else {
        Vec::new()
    };

    let bucket_secs = q.bucket_secs.unwrap_or(30.0).max(5.0);
    let dur = duration_sec.max(bucket_secs * 2.0);
    let inputs = strivo_heatmap::HeatmapInputs {
        segments: &segments,
        cuepoint_times: &cuepoint_times,
        highlights: &highlights,
        brandsafe_times: &brandsafe_times,
        duration_sec: dur,
        bucket_secs,
    };
    let buckets = strivo_heatmap::compute_heatmap(&inputs);
    let top = strivo_heatmap::top_k_buckets(&buckets, 5);

    Json(json!({
        "recording_id": recording_id,
        "duration_sec": dur,
        "bucket_secs": bucket_secs,
        "buckets": buckets,
        "top_k": top,
    }))
    .into_response()
}

fn editor_store_path() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("editor")
        .join("editor.db")
}

/// `GET /api/v1/plugins/editor/<recording_id>` — load the cached EDL,
/// initialising a default whole-source EDL when none exists.
async fn editor_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("editor") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    let store = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open store: {e}")).into_response(),
    };
    let edl = match store.load(&recording_id).ok().flatten() {
        Some(e) => e,
        None => {
            let dur = probe_duration(&input).unwrap_or(0.0);
            strivo_editor::Edl::from_source(&recording_id, &input.to_string_lossy(), dur)
        }
    };
    Json(json!({
        "edl": edl,
        "total_duration": edl.total_duration(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct EditorSaveQuery {
    /// Short label describing this edit (e.g. "manual edit", "trim dead air").
    /// Recorded in the revision history so the SPA can render meaningful
    /// undo targets. Defaults to "manual edit" when absent.
    #[serde(default)]
    label: Option<String>,
}

/// `POST /api/v1/plugins/editor/<recording_id>` — save the EDL the SPA
/// has been editing locally. Appends a revision tagged with `?label=` so
/// the user can revert across saves (DAW-style undo across reloads).
async fn editor_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<EditorSaveQuery>,
    Json(body): Json<strivo_editor::Edl>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("editor") {
        return r;
    }
    let mut edl = body;
    edl.recording_id = recording_id.clone();
    edl.compact();
    let store = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open store: {e}")).into_response(),
    };
    let label = q.label.as_deref().unwrap_or("manual edit");
    if let Err(e) = store.save_with_label(&edl, label) {
        return Problem::internal(format!("save: {e}")).into_response();
    }
    Json(json!({ "ok": true, "total_duration": edl.total_duration() })).into_response()
}

/// `GET /api/v1/plugins/editor/<recording_id>/revisions` — newest-first
/// metadata list of every saved revision; the SPA renders it as the undo
/// stack.
async fn editor_revisions_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("editor") {
        return r;
    }
    let store = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open store: {e}")).into_response(),
    };
    let revisions = match store.list_revisions(&recording_id, 200) {
        Ok(r) => r,
        Err(e) => return Problem::internal(format!("list: {e}")).into_response(),
    };
    Json(json!({ "recording_id": recording_id, "revisions": revisions })).into_response()
}

/// `POST /api/v1/plugins/editor/<recording_id>/revisions/<rev_id>/restore`
/// — replace the current EDL with the contents of revision `rev_id` and
/// append a new revision tagged "revert to v<rev_id>" so the restoration
/// itself is undoable.
async fn editor_revisions_restore(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((recording_id, rev_id)): Path<(String, i64)>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("editor") {
        return r;
    }
    let store = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open store: {e}")).into_response(),
    };
    let edl = match store.load_revision(&recording_id, rev_id) {
        Ok(Some(e)) => e,
        Ok(None) => return Problem::not_found("revision not found").into_response(),
        Err(e) => return Problem::internal(format!("load: {e}")).into_response(),
    };
    let label = format!("revert to v{rev_id}");
    if let Err(e) = store.save_with_label(&edl, &label) {
        return Problem::internal(format!("save: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "edl": edl,
        "total_duration": edl.total_duration(),
        "label": label,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/editor/<recording_id>/render` — bake the EDL
/// into a final file under `<recording_parent>/edl/<recording_id>.mkv`.
async fn editor_render(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("editor") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    let store = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open store: {e}")).into_response(),
    };
    let edl = match store.load(&recording_id).ok().flatten() {
        Some(e) => e,
        None => return Problem::not_found("no EDL saved yet").into_response(),
    };
    let out_dir = input
        .parent()
        .map(|p| p.join("edl"))
        .unwrap_or_else(|| std::path::PathBuf::from("./edl"));
    let safe = recording_id.replace(
        |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
        "_",
    );
    let output = out_dir.join(format!("{safe}.mkv"));
    // Pick up any saved branding overlay; absent or empty spec → passthrough,
    // so the fast `-c copy` concat path stays in play.
    let branding_spec: strivo_branding::BrandingSpec =
        std::fs::read_to_string(branding_path(&recording_id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    let chain = branding_spec.build_filter_chain("[0:v]");
    let fc = chain.filter_complex.clone();
    // Same for the saved volume automation. Empty automation produces
    // an `anull` filter which the renderer detects + falls back to `-c copy`.
    let automation: strivo_automation::VolumeAutomation =
        std::fs::read_to_string(automation_path(&recording_id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    let audio_filter = automation.build_audio_filter(0.05);
    match strivo_editor::render_edl_with_filters(&edl, &output, Some(&fc), Some(&audio_filter)) {
        Ok(bytes) => Json(json!({
            "ok": true,
            "output_path": output.to_string_lossy(),
            "bytes": bytes,
            "total_duration": edl.total_duration(),
        }))
        .into_response(),
        Err(e) => Problem::internal(format!("render: {e}")).into_response(),
    }
}

/// `GET /api/v1/plugins/viewguard/trend` — pull every verdict row
/// from the viewguard DB and run the cross-stream trend analyzer.
/// Returns a watchlist banded by Critical/Warning/Watch/Clear.
async fn viewguard_trend(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("viewguard") {
        return r;
    }
    let Some(conn) = viewguard_db().as_deref().and_then(open_ro) else {
        return Json(json!({
            "watchlist": {
                "critical": [], "warning": [], "watch": [], "clear": []
            },
            "samples": 0,
        }))
        .into_response();
    };
    // Pull verdict rows directly via SQL — the strivo_plugins::viewguard
    // crate doesn't expose a "list every verdict" helper today.
    let mut stmt = match conn.prepare(
        "SELECT channel_id, channel_name, final_score, stream_started_at
         FROM verdicts
         ORDER BY stream_started_at",
    ) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("prepare: {e}")).into_response(),
    };
    let rows: Vec<strivo_viewguard_trend::VerdictRow> = match stmt
        .query_map([], |r| {
            Ok(strivo_viewguard_trend::VerdictRow {
                channel_id: r.get::<_, String>(0)?,
                channel_name: r.get::<_, Option<String>>(1)?,
                final_score: r.get::<_, f64>(2)? as f32,
                stream_started_at: r.get::<_, String>(3)?,
            })
        })
        .and_then(|mapped| mapped.collect::<rusqlite::Result<Vec<_>>>())
    {
        Ok(v) => v,
        Err(e) => return Problem::internal(format!("query: {e}")).into_response(),
    };
    let watchlist = strivo_viewguard_trend::build_watchlist(&rows);
    Json(json!({
        "watchlist": watchlist,
        "samples": rows.len(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BrollPayload {
    /// JSON-serialised BrollLibrary the streamer maintains locally.
    /// SPA pulls it from the user's settings; we accept it inline here
    /// so the backend stays stateless.
    library: strivo_broll::BrollLibrary,
    #[serde(default = "default_broll_top_k")]
    top_k: usize,
}

fn default_broll_top_k() -> usize {
    12
}

/// `POST /api/v1/plugins/broll/<recording_id>` — turn a recording's
/// Crunchr segments into TopicSlices, score them against the supplied
/// library, return ranked B-roll suggestions.
async fn broll_suggest(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<BrollPayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("broll") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let detail = match strivo_plugins::crunchr::db::recording_detail(&conn, &recording_id) {
        Ok(Some(d)) => d,
        Ok(None) => return Problem::not_found("recording not transcribed").into_response(),
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };
    let slices: Vec<strivo_broll::TopicSlice> = detail
        .segments
        .iter()
        .map(|s| strivo_broll::TopicSlice {
            start_sec: s.start_sec as f32,
            end_sec: s.end_sec as f32,
            topics: detail.topics.clone(),
            text: s.text.clone(),
        })
        .collect();
    let top_k = body.top_k.clamp(1, 50);
    let suggestions = strivo_broll::suggest_brolls(&slices, &body.library, top_k);
    Json(json!({
        "recording_id": recording_id,
        "suggestions": suggestions,
        "library_size": body.library.assets.len(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct ChatDensityPayload {
    /// Chat log payload. One of `log` (IRC dump) or `csv` (CSV with
    /// `time_sec,user,message`) must be set.
    #[serde(default)]
    log: Option<String>,
    #[serde(default)]
    csv: Option<String>,
    /// Stream-start epoch-ms; used to convert IRC `tmi-sent-ts` tags
    /// into relative seconds. Unused for CSV input.
    #[serde(default)]
    stream_start_ts_ms: u64,
    /// Bucket size in seconds (default 30).
    #[serde(default = "default_bucket_secs_chat")]
    bucket_secs: f32,
}

fn default_bucket_secs_chat() -> f32 {
    30.0
}

/// `POST /api/v1/plugins/chat-density/<recording_id>` — parse an IRC
/// dump or CSV log, bucket events, and return the density / engagement
/// curve. Pure transform; no persistence today (the SPA owns the
/// upload). Wires the `x.chat_density` capability that Heatmap reserves.
async fn chat_density_compute(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<ChatDensityPayload>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("chat_density") {
        return r;
    }
    let events = if let Some(csv) = &body.csv {
        strivo_chat_density::parse_csv_log(csv)
    } else if let Some(log) = &body.log {
        strivo_chat_density::parse_irc_log(log, body.stream_start_ts_ms)
    } else {
        return Problem::bad_request("supply either `csv` or `log` body").into_response();
    };
    // Duration = max event time + 5% pad so the strip isn't clipped.
    let max_t = events.iter().map(|e| e.time_sec).fold(0.0_f32, f32::max);
    let duration_sec = (max_t * 1.05).max(body.bucket_secs * 2.0);
    let bucket_secs = body.bucket_secs.max(5.0);
    let points = strivo_chat_density::compute_density(&events, duration_sec, bucket_secs);
    Json(json!({
        "recording_id": recording_id,
        "event_count": events.len(),
        "duration_sec": duration_sec,
        "bucket_secs": bucket_secs,
        "points": points,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
struct DeadairQuery {
    #[serde(default)]
    noise_db: Option<f32>,
    #[serde(default)]
    min_span_secs: Option<f32>,
    #[serde(default)]
    trim_threshold_secs: Option<f32>,
}

/// `POST /api/v1/plugins/deadair/<recording_id>` — run ffmpeg's
/// silencedetect filter against the recording and return the span
/// list + recommended cuts the editor can apply.
async fn deadair_detect(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<DeadairQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("deadair") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let noise_db = q.noise_db.unwrap_or(strivo_deadair::DEFAULT_NOISE_DB);
    let min_span = q
        .min_span_secs
        .unwrap_or(strivo_deadair::DEFAULT_MIN_SPAN_SECS);
    let trim_threshold = q
        .trim_threshold_secs
        .unwrap_or(strivo_deadair::DEFAULT_TRIM_THRESHOLD_SECS);
    match strivo_deadair::detect_silences(&input, noise_db, min_span, trim_threshold) {
        Ok(r) => Json(json!({
            "recording_id": recording_id,
            "result": r,
        }))
        .into_response(),
        Err(e) => Problem::internal(format!("deadair: {e}")).into_response(),
    }
}

/// `GET /api/v1/plugins/chat/rooms` — list followed Twitch channels (live
/// first) the SPA can join over IRC. YouTube live chat needs an OAuth
/// flow we haven't built yet, so we surface those rooms tagged
/// `connectable=false`. Pro-gated.
async fn chat_rooms(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("chat") {
        return r;
    }
    let channels = match state.ipc.snapshot().await {
        Ok(strivo_core::ipc::ServerMessage::StateSnapshot { channels, .. }) => channels,
        Ok(_) => vec![],
        Err(e) => return Problem::internal(format!("snapshot: {e}")).into_response(),
    };
    let rooms: Vec<serde_json::Value> = channels
        .into_iter()
        .filter_map(|c| {
            let (platform, connectable) = match c.platform {
                strivo_core::platform::PlatformKind::Twitch => ("twitch", true),
                strivo_core::platform::PlatformKind::YouTube => ("youtube", false),
                strivo_core::platform::PlatformKind::Patreon => return None,
            };
            // Twitch user ID surfaces so the SPA can hit the
            // legacy badges + per-channel BTTV/FFZ/7TV emote
            // endpoints without re-resolving the login server-side.
            // ChannelEntry.id is the platform's stable numeric id
            // for Twitch.
            let user_id = if matches!(c.platform, strivo_core::platform::PlatformKind::Twitch) {
                Some(c.id.clone())
            } else {
                None
            };
            Some(serde_json::json!({
                "room": c.name,
                "display_name": if c.display_name.is_empty() { c.name.clone() } else { c.display_name },
                "platform": platform,
                "is_live": c.is_live,
                "viewer_count": c.viewer_count,
                "connectable": connectable,
                "user_id": user_id,
            }))
        })
        .collect();
    Json(json!({ "rooms": rooms })).into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatParseQuery {
    /// Single IRC line; the SPA also batches via `lines` (newline-delimited).
    #[serde(default)]
    line: Option<String>,
    #[serde(default)]
    lines: Option<String>,
}

/// `POST /api/v1/plugins/chat/parse` — parse a batch of Twitch IRC PRIVMSG
/// lines into `ChatMessage`s + token streams. Lets the SPA offload regex /
/// tag work to the host so the browser tab stays light during big chats.
async fn chat_parse(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ChatParseQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("chat") {
        return r;
    }
    let mut parsed: Vec<serde_json::Value> = Vec::new();
    let empty_emotes = strivo_chat::EmoteMap::new();
    let mut handle = |line: &str| {
        if let Some(m) = strivo_chat::parse_twitch_irc(line) {
            let tokens = strivo_chat::tokenize_text(&m.text, &empty_emotes);
            parsed.push(serde_json::json!({
                "message": m,
                "tokens": tokens,
            }));
        }
    };
    if let Some(line) = &body.line {
        handle(line);
    }
    if let Some(batch) = &body.lines {
        for line in batch.lines() {
            handle(line);
        }
    }
    Json(json!({ "parsed": parsed })).into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct MultistreamQuery {
    /// Container width in CSS pixels. SPA reports its own viewport so the
    /// tile maths stay client-driven without a round-trip on every resize.
    container_w: u32,
    container_h: u32,
    /// JSON-encoded `LayoutMode`. Defaults to `{"mode":"auto"}` when absent.
    #[serde(default)]
    mode: Option<String>,
    /// Host the iframe is served from — Twitch embeds need this in `parent=`.
    host: String,
}

/// `GET /api/v1/plugins/multistream/tiles?container_w=…&container_h=…&host=…`
/// — fetch the list of currently live followed channels from the daemon
/// and emit the tile layout for the given container, plus a ready-to-mount
/// embed URL per stream. Pro-gated.
async fn multistream_tiles(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<MultistreamQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("multistream") {
        return r;
    }
    let channels = match state.ipc.snapshot().await {
        Ok(strivo_core::ipc::ServerMessage::StateSnapshot { channels, .. }) => channels,
        Ok(_) => vec![],
        Err(e) => return Problem::internal(format!("snapshot: {e}")).into_response(),
    };
    let streams: Vec<strivo_multistream::Stream> = channels
        .into_iter()
        .filter(|c| c.is_live)
        .filter_map(|c| {
            let platform = match c.platform {
                strivo_core::platform::PlatformKind::Twitch => {
                    Some(strivo_multistream::Platform::Twitch)
                }
                strivo_core::platform::PlatformKind::YouTube => {
                    Some(strivo_multistream::Platform::YouTube)
                }
                strivo_core::platform::PlatformKind::Patreon => None,
            }?;
            Some(strivo_multistream::Stream {
                id: format!("{:?}:{}", c.platform, c.id),
                channel_name: if c.display_name.is_empty() {
                    c.name.clone()
                } else {
                    c.display_name
                },
                platform,
                embed_key: c.name,
                viewer_count: c.viewer_count.map(|v| v as u32),
            })
        })
        .collect();
    let mode: strivo_multistream::LayoutMode = q
        .mode
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(strivo_multistream::LayoutMode::Auto);
    let tiles = strivo_multistream::compute_tiles(&streams, q.container_w, q.container_h, &mode);
    let embeds: Vec<serde_json::Value> = streams
        .iter()
        .map(|s| {
            serde_json::json!({
                "stream_id": s.id,
                "channel_name": s.channel_name,
                "platform": s.platform,
                "viewer_count": s.viewer_count,
                "embed_url": strivo_multistream::embed_url(s, &q.host),
            })
        })
        .collect();
    Json(json!({
        "streams": embeds,
        "tiles": tiles,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct SidechainBody {
    pub voice_intervals: Vec<strivo_vad::VoiceInterval>,
    pub total_duration_sec: f32,
    #[serde(default)]
    pub knobs: Option<strivo_sidechain::SidechainKnobs>,
    /// Optional: persist the resulting automation directly to this
    /// recording's volume-automation store so the editor render bakes
    /// it on the next ⚡ Render. Default true (the typical flow).
    #[serde(default = "default_true_sc")]
    pub persist: bool,
}

fn default_true_sc() -> bool {
    true
}

/// `POST /api/v1/plugins/sidechain/<recording_id>` — translate VAD
/// voice intervals into a sidechain ducking automation curve and
/// optionally persist it as the recording's volume automation. The
/// existing render pipeline bakes the points via `asendcmd` — no new
/// ffmpeg plumbing.
async fn sidechain_run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<SidechainBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("sidechain") {
        return r;
    }
    let knobs = body.knobs.unwrap_or_default();
    let automation =
        strivo_sidechain::build_automation(&body.voice_intervals, body.total_duration_sec, &knobs);
    let mut persisted = false;
    if body.persist {
        let path = automation_path(&recording_id);
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Problem::internal(format!("mkdir: {e}")).into_response();
            }
        }
        match serde_json::to_string_pretty(&automation) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    return Problem::internal(format!("write: {e}")).into_response();
                }
                persisted = true;
            }
            Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
        }
    }
    let preview = automation.build_audio_filter(0.05);
    Json(json!({
        "recording_id": recording_id,
        "point_count": automation.points.len(),
        "knobs": knobs,
        "automation": automation,
        "audio_filter_preview": preview,
        "persisted_to_automation_store": persisted,
    }))
    .into_response()
}

fn pitch_path(recording_id: &str) -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("pitch")
        .join(format!("{recording_id}.json"))
}

/// `GET /api/v1/plugins/pitch/<id>` — load saved tempo/pitch settings
/// (defaults to identity) plus the composed `rubberband=` filter the
/// render path will splice in and the projected output duration based
/// on the recording's known length.
async fn pitch_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("pitch") {
        return r;
    }
    let pt: strivo_pitch::PitchTime = std::fs::read_to_string(pitch_path(&recording_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Json(json!({
        "recording_id": recording_id,
        "pitch_time": pt,
        "audio_filter": pt.to_filter(),
        "semitones": pt.semitones(),
        "is_identity": pt.is_identity(),
    }))
    .into_response()
}

/// `POST /api/v1/plugins/pitch/<id>` — persist a fresh tempo/pitch
/// state. Echoes back the filter + projected output duration if the
/// caller passes `source_duration_sec` so the UI can update without
/// a second round trip.
#[derive(Debug, Deserialize)]
pub(super) struct PitchSaveBody {
    pub pitch_time: strivo_pitch::PitchTime,
    #[serde(default)]
    pub source_duration_sec: Option<f64>,
}

async fn pitch_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<PitchSaveBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("pitch") {
        return r;
    }
    let path = pitch_path(&recording_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Problem::internal(format!("mkdir: {e}")).into_response();
        }
    }
    match serde_json::to_string_pretty(&body.pitch_time) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                return Problem::internal(format!("write: {e}")).into_response();
            }
        }
        Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
    }
    let projected = body
        .source_duration_sec
        .map(|d| body.pitch_time.output_duration_sec(d));
    Json(json!({
        "recording_id": recording_id,
        "ok": true,
        "audio_filter": body.pitch_time.to_filter(),
        "semitones": body.pitch_time.semitones(),
        "projected_output_duration_sec": projected,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/pitch/<id>/fit` — one-shot helper that
/// computes the tempo factor to land `source_duration_sec` on
/// `target_duration_sec` and persists it. Pitch left at identity so
/// voices keep the same pitch at the new tempo.
#[derive(Debug, Deserialize)]
pub(super) struct PitchFitBody {
    pub source_duration_sec: f64,
    pub target_duration_sec: f64,
}

async fn pitch_fit(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<PitchFitBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("pitch") {
        return r;
    }
    if body.source_duration_sec <= 0.0 || body.target_duration_sec <= 0.0 {
        return Problem::bad_request("source/target duration must be > 0").into_response();
    }
    let pt = strivo_pitch::PitchTime::fit_to_duration(
        body.source_duration_sec,
        body.target_duration_sec,
    );
    let path = pitch_path(&recording_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&pt) {
        let _ = std::fs::write(&path, json);
    }
    Json(json!({
        "recording_id": recording_id,
        "pitch_time": pt,
        "audio_filter": pt.to_filter(),
        "projected_output_duration_sec": pt.output_duration_sec(body.source_duration_sec),
    }))
    .into_response()
}

fn insert_fx_path(recording_id: &str) -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("insert-fx")
        .join(format!("{recording_id}.json"))
}

/// `GET /api/v1/plugins/insert-fx/<id>` — load the saved insert chain
/// (empty when absent) plus the composed ffmpeg `-af` value the render
/// path will splice in. The SPA renders the effects as draggable cards.
async fn insert_fx_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insert-fx") {
        return r;
    }
    let chain: strivo_insert_fx::InsertChain =
        std::fs::read_to_string(insert_fx_path(&recording_id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    let filter = chain.to_filter();
    Json(json!({
        "recording_id": recording_id,
        "chain": chain,
        "audio_filter": filter,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/insert-fx/<id>` — persist a fresh ordered
/// chain (reorder, edit, delete all happen client-side; server just
/// stores the final state). Echoes back the composed filter so the
/// SPA can re-render without a follow-up GET.
async fn insert_fx_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(chain): Json<strivo_insert_fx::InsertChain>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insert-fx") {
        return r;
    }
    let path = insert_fx_path(&recording_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Problem::internal(format!("mkdir: {e}")).into_response();
        }
    }
    match serde_json::to_string_pretty(&chain) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                return Problem::internal(format!("write: {e}")).into_response();
            }
        }
        Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
    }
    Json(json!({
        "recording_id": recording_id,
        "ok": true,
        "audio_filter": chain.to_filter(),
        "stage_count": chain.effects.len(),
    }))
    .into_response()
}

/// `POST /api/v1/plugins/insert-fx/<id>/preset/<bus>` — install one of
/// the bundled presets (`voice` or `game`) without the user having to
/// hand-stack the standard five-/three-stage chain.
async fn insert_fx_preset(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((recording_id, bus)): Path<(String, String)>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insert-fx") {
        return r;
    }
    let chain = match bus.as_str() {
        "voice" => strivo_insert_fx::InsertChain::voice_bus_default(),
        "game" => strivo_insert_fx::InsertChain::game_bus_default(),
        other => {
            return Problem::bad_request(format!(
                "unknown bus '{other}' (expected 'voice' or 'game')"
            ))
            .into_response();
        }
    };
    let path = insert_fx_path(&recording_id);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&chain) {
        let _ = std::fs::write(&path, json);
    }
    Json(json!({
        "recording_id": recording_id,
        "bus": bus,
        "chain": chain,
        "audio_filter": chain.to_filter(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct VadQuery {
    /// Window seconds to analyse from t=0 (0 = whole recording). Capped
    /// 10..=3600.
    #[serde(default)]
    window_sec: Option<f32>,
    /// dB threshold the gate opens at (defaults to GateKnobs::default().open_db).
    #[serde(default)]
    open_db: Option<f32>,
    /// dB threshold the gate closes at (defaults to GateKnobs::default().close_db).
    #[serde(default)]
    close_db: Option<f32>,
    /// Minimum natural pause to keep when surfacing tightening
    /// recommendations (defaults to 1.0s — anything shorter is a
    /// breath, not editable dead air).
    #[serde(default)]
    min_keep_sec: Option<f32>,
}

/// `POST /api/v1/plugins/vad/<recording_id>?window_sec=600&open_db=-30…`
/// — extract the RMS envelope via ffmpeg's astats filter (same pipeline
/// the beat-detect endpoint uses) and run the hysteresis gate. Returns
/// voice intervals + recommended ripple-delete gaps the editor can
/// apply via auto-tighten. Pro-gated.
async fn vad_run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<VadQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("vad") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let window = q.window_sec.unwrap_or(600.0).clamp(10.0, 3600.0);
    let tmp =
        std::env::temp_dir().join(format!("strivo-vad-{}.log", uuid::Uuid::new_v4().simple()));
    let filter = format!(
        "aresample=8000,asetnsamples=400:p=0,astats=metadata=1:reset=1,ametadata=print:file={}",
        tmp.display()
    );
    let output = match tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-t",
            &format!("{:.3}", window),
            "-i",
        ])
        .arg(&input)
        .args(["-af", &filter, "-vn", "-ac", "1", "-f", "null", "-"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Problem::internal(format!("spawn ffmpeg: {e}")).into_response();
        }
    };
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Problem::internal(format!(
            "ffmpeg exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
        .into_response();
    }
    let metadata = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    // Reuse beat-detect's parser shape — same astats output. Each
    // OnsetSample becomes a vad EnvelopeFrame; the field names match
    // so we can swap straight through.
    let mut current_t: Option<f32> = None;
    let mut envelope: Vec<strivo_vad::EnvelopeFrame> = Vec::new();
    for line in metadata.lines() {
        if let Some(idx) = line.find("pts_time:") {
            let tail = &line[idx + "pts_time:".len()..];
            let token: String = tail.chars().take_while(|c| !c.is_whitespace()).collect();
            current_t = token.parse().ok();
        }
        if let Some(rest) = line
            .trim_start()
            .strip_prefix("lavfi.astats.Overall.RMS_level=")
        {
            if let (Some(t), Ok(db)) = (current_t, rest.trim().parse::<f32>()) {
                if db.is_finite() {
                    envelope.push(strivo_vad::EnvelopeFrame {
                        time_sec: t,
                        rms_db: db,
                    });
                }
            }
        }
    }
    if envelope.is_empty() {
        return Problem::internal("vad: ffmpeg emitted no astats frames").into_response();
    }
    let default_knobs = strivo_vad::GateKnobs::default();
    let knobs = strivo_vad::GateKnobs {
        open_db: q.open_db.unwrap_or(default_knobs.open_db),
        close_db: q.close_db.unwrap_or(default_knobs.close_db),
        ..default_knobs
    };
    let voice_intervals = strivo_vad::detect_voice(&envelope, &knobs);
    let total = envelope.last().map(|f| f.time_sec).unwrap_or(0.0);
    let recommended_gaps = strivo_vad::tightening_recommendations(
        &voice_intervals,
        total,
        q.min_keep_sec.unwrap_or(1.0),
    );
    let total_savings: f32 = recommended_gaps.iter().map(|g| g.duration()).sum();
    Json(json!({
        "recording_id": recording_id,
        "window_sec": window,
        "envelope_frames": envelope.len(),
        "voice_intervals": voice_intervals,
        "recommended_gaps": recommended_gaps,
        "total_savings_sec": total_savings,
        "knobs": knobs,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct BeatDetectQuery {
    /// Window seconds to sample (0 = whole recording). Capped server-
    /// side so a one-hour stream doesn't run the detector across the
    /// full envelope; 120s is plenty for tempo detection.
    #[serde(default)]
    window_sec: Option<f32>,
    /// BPM search range.
    #[serde(default)]
    min_bpm: Option<f32>,
    #[serde(default)]
    max_bpm: Option<f32>,
    #[serde(default)]
    top_n: Option<usize>,
}

/// Parse `ametadata=print` output (channel-aware RMS dB) into the
/// envelope shape the beat-detect crate expects. The format is:
///   frame:N    pts:N       pts_time:T
///   lavfi.astats.1.RMS_level=DB
///   lavfi.astats.Overall.RMS_level=DB
///   …
/// where pts_time is mid-line, not line-start. Pick up the first
/// non-finite-skipped Overall.RMS per frame.
fn parse_astats_rms(stderr: &str) -> Vec<strivo_beat_detect::OnsetSample> {
    let mut out = Vec::new();
    let mut current_t: Option<f32> = None;
    for line in stderr.lines() {
        if let Some(idx) = line.find("pts_time:") {
            let tail = &line[idx + "pts_time:".len()..];
            let token: String = tail.chars().take_while(|c| !c.is_whitespace()).collect();
            current_t = token.parse().ok();
        }
        if let Some(rest) = line
            .trim_start()
            .strip_prefix("lavfi.astats.Overall.RMS_level=")
        {
            if let (Some(t), Ok(db)) = (current_t, rest.trim().parse::<f32>()) {
                if db.is_finite() {
                    out.push(strivo_beat_detect::OnsetSample {
                        time_sec: t,
                        rms_db: db,
                    });
                }
            }
        }
    }
    out
}

/// `POST /api/v1/plugins/beat-detect/<recording_id>?window_sec=120` —
/// extract the RMS envelope via ffmpeg's astats filter, run the onset
/// picker + BPM autocorrelation, and return the detected tempo + onset
/// list + tempo grid the editor can render. Pro-gated.
async fn beat_detect_run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<BeatDetectQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("beat-detect") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let window = q.window_sec.unwrap_or(120.0).clamp(10.0, 600.0);
    let min_bpm = q.min_bpm.unwrap_or(60.0);
    let max_bpm = q.max_bpm.unwrap_or(200.0);
    let top_n = q.top_n.unwrap_or(3).clamp(1, 8);
    // Strategy: take the first `window` seconds + downmix to mono +
    // resample to 8 kHz (the envelope doesn't need high fidelity) +
    // emit RMS every 50 ms via astats. ametadata=print's `file=-` route
    // doesn't flush through the null muxer reliably; write to a temp
    // file and read it back instead.
    let tmp = std::env::temp_dir().join(format!("strivo-bd-{}.log", uuid::Uuid::new_v4().simple()));
    // aresample=8000 downsamples to 8 kHz (the envelope doesn't need
    // higher fidelity); asetnsamples=400:p=0 chunks the stream into
    // exact 50 ms frames; astats reset=1 emits per-frame (not
    // cumulative) RMS so clicks/hits register as discrete peaks
    // against the silent floor.
    let filter = format!(
        "aresample=8000,asetnsamples=400:p=0,astats=metadata=1:reset=1,ametadata=print:file={}",
        tmp.display()
    );
    let output = match tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-t",
            &format!("{:.3}", window),
            "-i",
        ])
        .arg(&input)
        .args(["-af", &filter, "-vn", "-ac", "1", "-f", "null", "-"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Problem::internal(format!("spawn ffmpeg: {e}")).into_response();
        }
    };
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Problem::internal(format!(
            "ffmpeg exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
        .into_response();
    }
    let metadata = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    let envelope = parse_astats_rms(&metadata);
    if envelope.is_empty() {
        return Problem::internal("beat-detect: ffmpeg emitted no astats frames").into_response();
    }
    let knobs = strivo_beat_detect::OnsetKnobs::default();
    let onsets = strivo_beat_detect::detect_onsets(&envelope, &knobs);
    let candidates = strivo_beat_detect::estimate_bpm(&onsets, min_bpm, max_bpm, top_n);
    let grid = if let Some(top) = candidates.first() {
        strivo_beat_detect::align_to_grid(&onsets, top.bpm, 0.0)
    } else {
        vec![]
    };
    Json(json!({
        "recording_id": recording_id,
        "window_sec": window,
        "envelope_frames": envelope.len(),
        "onset_count": onsets.len(),
        "tempo_candidates": candidates,
        "tempo_grid_secs": grid,
        "onsets": onsets,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct ScheduleOptimizerBody {
    pub samples: Vec<strivo_schedule_optimizer::EngagementSample>,
    /// How many slots to return. Defaults to 3 (a typical "post N times
    /// this week" recommendation).
    #[serde(default = "default_top_n")]
    pub top_n: usize,
    /// "greedy" picks the top N regardless of proximity; "spread" picks
    /// the top N but enforces a minimum hour gap per day. Default
    /// "spread" with min_gap_hours = 4.
    #[serde(default)]
    pub mode: Option<String>,
    /// Min hour gap for spread mode. Ignored otherwise.
    #[serde(default = "default_min_gap")]
    pub min_gap_hours: u8,
}

fn default_top_n() -> usize {
    3
}
fn default_min_gap() -> u8 {
    4
}

/// `POST /api/v1/plugins/schedule-optimizer/<recording_id>` — aggregate
/// caller-supplied engagement samples and return the top weekly publish
/// slots. Pro-gated. recording_id is just a tag in the response — the
/// math doesn't need any per-recording state.
async fn schedule_optimizer_recommend(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<ScheduleOptimizerBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("schedule-optimizer") {
        return r;
    }
    let grid = strivo_schedule_optimizer::aggregate(&body.samples);
    let mode = match body.mode.as_deref().unwrap_or("spread") {
        "greedy" => strivo_schedule_optimizer::RankMode::Greedy,
        _ => strivo_schedule_optimizer::RankMode::Spread {
            min_gap_hours: body.min_gap_hours,
        },
    };
    let picks = strivo_schedule_optimizer::top_slots(&grid, body.top_n, mode);
    Json(json!({
        "recording_id": recording_id,
        "grid": grid,
        "recommendations": picks,
        "sample_count": body.samples.len(),
    }))
    .into_response()
}

fn scenes_db_path() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("scenes")
        .join("scenes.db")
}

/// Read every per-recording plugin state file we know about into a
/// component map. Missing files map to JSON null (compose() filters
/// nulls so the manifest stays tight). Keeps the list in one place so
/// adding a new plugin's state to a Scene is a one-line change.
fn read_recording_components(
    recording_id: &str,
) -> std::collections::BTreeMap<String, serde_json::Value> {
    let mut out = std::collections::BTreeMap::new();
    let load = |p: std::path::PathBuf| -> serde_json::Value {
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null)
    };
    out.insert("branding".into(), load(branding_path(recording_id)));
    out.insert("automation".into(), load(automation_path(recording_id)));
    out.insert(
        "captions_style".into(),
        load(captions_style_path(recording_id)),
    );
    // EDL state lives in a SQLite shared by all recordings; pull just this
    // one's row via the existing helper.
    let edl_json = match strivo_editor::store::EdlStore::open(&editor_store_path()) {
        Ok(store) => store
            .load(recording_id)
            .ok()
            .flatten()
            .and_then(|edl| serde_json::to_value(&edl).ok())
            .unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    };
    out.insert("editor".into(), edl_json);
    out
}

/// Write each component in the manifest back to its plugin's storage.
/// Returns the names of components that were restored vs skipped.
fn write_recording_components(
    recording_id: &str,
    manifest: &strivo_scenes::SceneManifest,
) -> (Vec<String>, Vec<String>) {
    let mut restored = Vec::new();
    let mut skipped = Vec::new();
    let write_file = |p: std::path::PathBuf, v: &serde_json::Value| -> bool {
        if let Some(parent) = p.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        serde_json::to_string_pretty(v)
            .ok()
            .and_then(|s| std::fs::write(&p, s).ok())
            .is_some()
    };
    for (name, value) in strivo_scenes::decompose(manifest) {
        let ok = match name {
            "branding" => write_file(branding_path(recording_id), value),
            "automation" => write_file(automation_path(recording_id), value),
            "captions_style" => write_file(captions_style_path(recording_id), value),
            "editor" => {
                // EDL deserialises through the editor crate to validate
                // shape before we commit it to the shared store.
                match serde_json::from_value::<strivo_editor::Edl>(value.clone()) {
                    Ok(mut edl) => {
                        edl.recording_id = recording_id.to_string();
                        edl.compact();
                        if let Ok(store) =
                            strivo_editor::store::EdlStore::open(&editor_store_path())
                        {
                            store.save_with_label(&edl, "restored from scene").is_ok()
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                }
            }
            _ => false, // Unknown component — preserve forward-compat by
                        // listing it as skipped rather than crashing.
        };
        if ok {
            restored.push(name.to_string());
        } else {
            skipped.push(name.to_string());
        }
    }
    (restored, skipped)
}

#[derive(Debug, Deserialize)]
pub(super) struct SceneCaptureBody {
    pub name: String,
    #[serde(default)]
    pub thumbnail_data_url: Option<String>,
}

/// `GET /api/v1/plugins/scenes/<recording_id>` — newest-first list of
/// every scene saved for this recording.
async fn scenes_list(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("scenes") {
        return r;
    }
    let store = match strivo_scenes::SceneStore::open(&scenes_db_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open: {e}")).into_response(),
    };
    let list = store.list(&recording_id, 100).unwrap_or_default();
    Json(json!({ "recording_id": recording_id, "scenes": list })).into_response()
}

/// `POST /api/v1/plugins/scenes/<recording_id>` — capture every current
/// per-recording plugin state into a named scene.
async fn scenes_capture(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<SceneCaptureBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("scenes") {
        return r;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let comps = read_recording_components(&recording_id);
    let manifest = strivo_scenes::compose(
        &id,
        body.name,
        &recording_id,
        comps,
        body.thumbnail_data_url,
    );
    let store = match strivo_scenes::SceneStore::open(&scenes_db_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open: {e}")).into_response(),
    };
    if let Err(e) = store.save(&manifest) {
        return Problem::internal(format!("save: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "id": manifest.id,
        "name": manifest.name,
        "component_keys": manifest.components.keys().collect::<Vec<_>>(),
        "size_bytes": manifest.approx_size_bytes(),
    }))
    .into_response()
}

/// `POST /api/v1/plugins/scenes/<recording_id>/<scene_id>/restore` —
/// write every component back to its plugin store. Returns the names
/// of restored + skipped components so the SPA can toast which plugins
/// were touched.
async fn scenes_restore(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((recording_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("scenes") {
        return r;
    }
    let store = match strivo_scenes::SceneStore::open(&scenes_db_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open: {e}")).into_response(),
    };
    let manifest = match store.load(&recording_id, &scene_id) {
        Ok(Some(m)) => m,
        Ok(None) => return Problem::not_found("scene not found").into_response(),
        Err(e) => return Problem::internal(format!("load: {e}")).into_response(),
    };
    let (restored, skipped) = write_recording_components(&recording_id, &manifest);
    Json(json!({
        "ok": true,
        "id": manifest.id,
        "name": manifest.name,
        "restored": restored,
        "skipped": skipped,
    }))
    .into_response()
}

/// `DELETE /api/v1/plugins/scenes/<recording_id>/<scene_id>` — idempotent.
async fn scenes_delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((recording_id, scene_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("scenes") {
        return r;
    }
    let store = match strivo_scenes::SceneStore::open(&scenes_db_path()) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("open: {e}")).into_response(),
    };
    let did = store.delete(&recording_id, &scene_id).unwrap_or(false);
    Json(json!({ "ok": true, "removed": did })).into_response()
}

fn captions_style_path(recording_id: &str) -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("captions")
        .join(format!("{recording_id}.style.json"))
}

/// `GET /api/v1/plugins/captions/<id>/style` — load the saved ASS style
/// spec for this recording; returns the default when none saved.
async fn captions_style_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("captions") {
        return r;
    }
    let style: strivo_captions::AssStyle =
        std::fs::read_to_string(captions_style_path(&recording_id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    Json(json!({ "recording_id": recording_id, "style": style })).into_response()
}

/// `POST /api/v1/plugins/captions/<id>/style` — persist the ASS style
/// spec for this recording. The next `?fmt=ass` export consults this.
async fn captions_style_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<strivo_captions::AssStyle>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("captions") {
        return r;
    }
    let path = captions_style_path(&recording_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Problem::internal(format!("mkdir: {e}")).into_response();
        }
    }
    let json = match serde_json::to_string_pretty(&body) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
    };
    if let Err(e) = std::fs::write(&path, json) {
        return Problem::internal(format!("write: {e}")).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

fn automation_path(recording_id: &str) -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("automation")
        .join(format!("{recording_id}.json"))
}

/// `GET /api/v1/plugins/automation/<recording_id>` — load the saved
/// volume automation; empty list when absent. Returns the live
/// asendcmd preview alongside so the SPA can show the ffmpeg command
/// without rebuilding it client-side.
async fn automation_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("automation") {
        return r;
    }
    let path = automation_path(&recording_id);
    let automation: strivo_automation::VolumeAutomation = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let asendcmd = automation.to_asendcmd(0.05);
    let filter = automation.build_audio_filter(0.05);
    Json(json!({
        "recording_id": recording_id,
        "automation": automation,
        "asendcmd": asendcmd,
        "audio_filter": filter,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/automation/<recording_id>` — persist a volume
/// automation curve. The Editor render path consults this file to
/// decide whether to bake automation into the output.
async fn automation_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<strivo_automation::VolumeAutomation>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("automation") {
        return r;
    }
    let path = automation_path(&recording_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Problem::internal(format!("mkdir: {e}")).into_response();
        }
    }
    let json = match serde_json::to_string_pretty(&body) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
    };
    if let Err(e) = std::fs::write(&path, json) {
        return Problem::internal(format!("write: {e}")).into_response();
    }
    let asendcmd = body.to_asendcmd(0.05);
    let filter = body.build_audio_filter(0.05);
    Json(json!({
        "ok": true,
        "point_count": body.points.len(),
        "asendcmd": asendcmd,
        "audio_filter": filter,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct StructureBody {
    pub total_duration_sec: f32,
    #[serde(default)]
    pub chapters: Vec<strivo_structure::ChapterSpan>,
    #[serde(default)]
    pub chat_buckets: Vec<strivo_structure::ChatBucket>,
    #[serde(default)]
    pub scene_cuts_sec: Vec<f32>,
    #[serde(default)]
    pub knobs: Option<strivo_structure::ClassifierKnobs>,
}

/// `POST /api/v1/plugins/structure/<recording_id>` — run the DAW-style
/// section labeler. Body carries the inputs (chapters / chat density /
/// scene cuepoints / duration) so the host can feed in already-computed
/// artefacts from other plugins. Pro-gated.
async fn structure_classify(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(body): Json<StructureBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("structure") {
        return r;
    }
    let inputs = strivo_structure::StructureInputs {
        total_duration_sec: body.total_duration_sec,
        chapters: body.chapters,
        chat_buckets: body.chat_buckets,
        scene_cuts_sec: body.scene_cuts_sec,
    };
    let knobs = body.knobs.unwrap_or_default();
    let segments = strivo_structure::classify(&inputs, &knobs);
    let totals: std::collections::HashMap<String, f32> =
        segments
            .iter()
            .fold(std::collections::HashMap::new(), |mut acc, s| {
                *acc.entry(format!("{:?}", s.kind).to_lowercase())
                    .or_insert(0.0) += s.duration();
                acc
            });
    Json(json!({
        "recording_id": recording_id,
        "segments": segments,
        "totals_sec_by_kind": totals,
        "knobs": knobs,
    }))
    .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct LoudnessQuery {
    /// Platform preset name (youtube, spotify, apple_music, ebu_r128,
    /// twitch). Used both to seed the target and to label the response.
    #[serde(default)]
    platform: Option<String>,
}

fn loudness_platform(s: Option<&str>) -> strivo_loudness::Platform {
    match s.unwrap_or("youtube") {
        "spotify" => strivo_loudness::Platform::Spotify,
        "apple_music" | "apple-music" => strivo_loudness::Platform::AppleMusic,
        "ebu_r128" | "ebu-r128" | "ebu" => strivo_loudness::Platform::EbuR128,
        "twitch" => strivo_loudness::Platform::Twitch,
        _ => strivo_loudness::Platform::YouTube,
    }
}

/// `POST /api/v1/plugins/loudness/<recording_id>?platform=…` — run the
/// ffmpeg loudnorm pass-1 measurement against the recording and return
/// the parsed statistics + signed delta from the chosen platform target.
/// The Editor's Loudness panel renders the delta as a colour-coded gauge
/// so users see at a glance whether the source needs gain.
async fn loudness_measure(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(q): Query<LoudnessQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("loudness") {
        return r;
    }
    let input = match resolve_recording_path(&recording_id).await {
        Ok(p) => p,
        Err(e) => return Problem::not_found(e).into_response(),
    };
    if !input.exists() {
        return Problem::not_found("recording file missing").into_response();
    }
    let plat = loudness_platform(q.platform.as_deref());
    let target = strivo_loudness::preset_for(plat);
    let filter = strivo_loudness::pass1_filter(target);
    let output = match tokio::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(&input)
        .args(["-af", &filter, "-vn", "-sn", "-f", "null", "-"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => return Problem::internal(format!("spawn ffmpeg: {e}")).into_response(),
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let pass1 = match strivo_loudness::parse_pass1(&stderr) {
        Some(p) => p,
        None => {
            return Problem::internal("loudnorm: pass-1 JSON not found in stderr").into_response()
        }
    };
    let delta = strivo_loudness::delta_from_target(target, &pass1);
    let pass2 = strivo_loudness::pass2_filter(target, &pass1);
    Json(json!({
        "recording_id": recording_id,
        "platform": q.platform.unwrap_or_else(|| "youtube".into()),
        "target": target,
        "measurement": pass1,
        "delta": delta,
        "pass2_filter": pass2,
    }))
    .into_response()
}

fn branding_path(recording_id: &str) -> std::path::PathBuf {
    // Per-recording JSON spec; tiny payload, no schema migration overhead.
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("branding")
        .join(format!("{recording_id}.json"))
}

/// `GET /api/v1/plugins/branding/<recording_id>` — load the saved overlay
/// spec; returns an empty default when the file is absent.
async fn branding_load(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("branding") {
        return r;
    }
    let path = branding_path(&recording_id);
    let spec: strivo_branding::BrandingSpec = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => strivo_branding::BrandingSpec::default(),
    };
    let chain = spec.build_filter_chain("[0:v]");
    Json(json!({
        "recording_id": recording_id,
        "spec": spec,
        "filter_complex": chain.filter_complex,
        "video_label": chain.video_label,
    }))
    .into_response()
}

/// `POST /api/v1/plugins/branding/<recording_id>` — persist the overlay spec
/// the SPA composed; returns the resulting ffmpeg filter so the editor can
/// preview the chain that will be applied at render time.
async fn branding_save(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Json(spec): Json<strivo_branding::BrandingSpec>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("branding") {
        return r;
    }
    let path = branding_path(&recording_id);
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Problem::internal(format!("mkdir: {e}")).into_response();
        }
    }
    let json = match serde_json::to_string_pretty(&spec) {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("serialise: {e}")).into_response(),
    };
    if let Err(e) = std::fs::write(&path, json) {
        return Problem::internal(format!("write: {e}")).into_response();
    }
    let chain = spec.build_filter_chain("[0:v]");
    Json(json!({
        "ok": true,
        "filter_complex": chain.filter_complex,
        "video_label": chain.video_label,
    }))
    .into_response()
}

/// Open a plugin DB read-only. Returns None when the file is absent (plugin
/// idle) so callers can serve an empty payload.
fn open_ro(path: &std::path::Path) -> Option<Connection> {
    if !path.exists() {
        return None;
    }
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

/// Returns Err(402) when `name` is a Pro plugin and this machine is not
/// entitled. Free plugins always Ok. The check is centralised here so
/// every data route shares the same gate without forgetting one.
// The Err is an axum `Response` by design (it's `?`-propagated straight into
// the handler's return); boxing it to satisfy `result_large_err` would only
// add an indirection at every `gate_pro(name)?` call site.
#[allow(clippy::result_large_err)]
fn gate_pro(name: &str) -> Result<(), axum::response::Response> {
    if strivo_core::licence::gate::is_entitled(name) {
        return Ok(());
    }
    Err(Problem::payment_required(format!(
        "{name} is a Strivo Pro plugin — activate or start a 3-day trial from the Plugins page."
    ))
    .into_response())
}

// ── Plugin index ─────────────────────────────────────────────────────

/// `GET /api/v1/plugins` — the four data-backed first-party plugins with a
/// readiness flag + rollup counts, so the hub can show "12 recordings" etc.
async fn index(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }

    // Pro-plugin entitlement decides whether each first-party plugin
    // unlocks. We still include locked Pro plugins in the response so
    // the SPA can render them with a lock badge + upgrade CTA — hiding
    // them entirely would make the upgrade card feel disconnected.
    let pro_entitled = |name: &str| strivo_core::licence::gate::is_entitled(name);
    let crunchr_ok = pro_entitled("crunchr");
    let archiver_ok = pro_entitled("archiver");
    let viewguard_ok = pro_entitled("viewguard");
    let insights_ok = pro_entitled("insights");

    // Per-plugin data_dir surfaces in the response so the SPA can show
    // a "data" hint without the user having to know the layout (M6).
    let crunchr_data = crunchr_db()
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let archiver_data = archiver_db()
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let viewguard_data = viewguard_db()
        .as_deref()
        .and_then(|p| p.parent().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_default();

    let crunchr_conn = open_ro(&crunchr_db());
    let crunchr = match &crunchr_conn {
        Some(c) => json!({
            "name": "crunchr",
            "display": "Crunchr",
            "description": "AI transcription, diarization & analysis",
            "available": crunchr_ok,
            "pro": true,
            "entitled": crunchr_ok,
            "data_dir": crunchr_data,
            "stats": {
                "recordings": count(c, "SELECT COUNT(*) FROM videos"),
                "analyzed": count(c, "SELECT COUNT(*) FROM video_analysis"),
            },
            "verbs": [
                // Verb name is the dispatch key (matches the plugin's
                // `PluginCommand::item("Re-transcribe", …)`); label is the
                // user-facing string the SPA renders.
                { "verb": "Re-transcribe", "scope": "recording", "label": "Generate subtitles" },
            ],
        }),
        None => json!({
            "name": "crunchr", "display": "Crunchr",
            "description": "AI transcription, diarization & analysis",
            "available": false, "pro": true, "entitled": crunchr_ok, "stats": {}, "verbs": []
        }),
    };

    let insights = match &crunchr_conn {
        Some(c) => json!({
            "name": "insights",
            "display": "Insights",
            "description": "Word frequency, speaker airtime, topics & sentiment",
            "available": insights_ok,
            "pro": true,
            "entitled": insights_ok,
            "data_dir": crunchr_data,
            "stats": {
                "words": count(c, "SELECT COUNT(DISTINCT word) FROM word_frequency"),
                "topics_videos": count(c, "SELECT COUNT(*) FROM video_analysis WHERE topics IS NOT NULL AND topics != ''"),
            },
            "verbs": [],
        }),
        None => json!({
            "name": "insights", "display": "Insights",
            "description": "Word frequency, speaker airtime, topics & sentiment",
            "available": false, "pro": true, "entitled": insights_ok, "stats": {}, "verbs": []
        }),
    };

    let archiver = match open_ro(&archiver_db()) {
        Some(c) => json!({
            "name": "archiver",
            "display": "Archiver",
            "description": "Back-catalog VOD pulls & download tracking",
            "available": archiver_ok,
            "pro": true,
            "entitled": archiver_ok,
            "data_dir": archiver_data,
            "stats": {
                "channels": count(&c, "SELECT COUNT(*) FROM channels"),
                "videos": count(&c, "SELECT COUNT(*) FROM videos"),
                "downloaded": count(&c, "SELECT COUNT(*) FROM videos WHERE downloaded"),
            },
            "verbs": [
                { "verb": "Re-archive channel", "scope": "recording", "label": "Re-archive channel" },
            ],
        }),
        None => json!({
            "name": "archiver", "display": "Archiver",
            "description": "Back-catalog VOD pulls & download tracking",
            "available": false, "pro": true, "entitled": archiver_ok, "stats": {}, "verbs": []
        }),
    };

    let viewguard = match viewguard_db().as_deref().and_then(open_ro) {
        Some(c) => json!({
            "name": "viewguard",
            "display": "Viewguard",
            "description": "Viewbot fraud detection — verdicts & viewer signals",
            "available": viewguard_ok,
            "pro": true,
            "entitled": viewguard_ok,
            "data_dir": viewguard_data,
            "stats": {
                "verdicts": count(&c, "SELECT COUNT(DISTINCT channel_id) FROM verdicts"),
                "samples": count(&c, "SELECT COUNT(*) FROM samples"),
            },
            "verbs": [],
        }),
        None => json!({
            "name": "viewguard", "display": "Viewguard",
            "description": "Viewbot fraud detection — verdicts & viewer signals",
            "available": false, "pro": true, "entitled": viewguard_ok, "stats": {}, "verbs": []
        }),
    };

    // Hide locked Pro plugins entirely — the upgrade card on the same
    // page carries the unlock story, so surfacing a dimmed row would
    // just be noise. Free plugins (none today) would still appear.
    let plugins: Vec<Value> = [
        (crunchr_ok, crunchr),
        (insights_ok, insights),
        (archiver_ok, archiver),
        (viewguard_ok, viewguard),
    ]
    .into_iter()
    .filter_map(|(ok, v)| if ok { Some(v) } else { None })
    .collect();
    Json(json!({ "plugins": plugins })).into_response()
}

// ── Crunchr ──────────────────────────────────────────────────────────

async fn crunchr_recordings(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("crunchr") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Json(json!({ "available": false, "recordings": [] })).into_response();
    };
    match strivo_plugins::crunchr::db::list_videos(&conn) {
        Ok(vids) => {
            let items: Vec<Value> = vids
                .into_iter()
                .map(|v| {
                    json!({
                        "recording_id": v.recording_id,
                        "channel_name": v.channel_name,
                        "title": v.title,
                        "status": v.status,
                        "segment_count": v.segment_count,
                        "has_analysis": v.has_analysis,
                        "created_at": v.created_at,
                    })
                })
                .collect();
            Json(json!({ "available": true, "recordings": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

async fn crunchr_recording(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("crunchr") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    match strivo_plugins::crunchr::db::recording_detail(&conn, &id) {
        Ok(Some(d)) => {
            let segments: Vec<Value> = d
                .segments
                .into_iter()
                .map(|s| {
                    json!({
                        "index": s.index,
                        "start_sec": s.start_sec,
                        "end_sec": s.end_sec,
                        "text": s.text,
                        "speaker": s.speaker,
                        "confidence": s.confidence,
                    })
                })
                .collect();
            Json(json!({
                "recording_id": d.recording_id,
                "channel_name": d.channel_name,
                "title": d.title,
                "status": d.status,
                "summary": d.summary,
                "topics": d.topics,
                "sentiment": d.sentiment,
                "segments": segments,
            }))
            .into_response()
        }
        Ok(None) => Problem::not_found("recording not transcribed").into_response(),
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
}

async fn crunchr_search(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("crunchr") {
        return r;
    }
    let query = q.q.trim();
    if query.is_empty() {
        return Json(json!({ "results": [] })).into_response();
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Json(json!({ "available": false, "results": [] })).into_response();
    };
    match strivo_plugins::crunchr::db::fts_search(&conn, query, 50) {
        Ok(results) => {
            let items: Vec<Value> = results
                .into_iter()
                .map(|r| {
                    json!({
                        "chunk_id": r.chunk_id,
                        "video_title": r.video_title,
                        "channel_name": r.channel_name,
                        "snippet": r.snippet,
                        "start_sec": r.start_sec,
                        "end_sec": r.end_sec,
                        "score": r.score,
                    })
                })
                .collect();
            Json(json!({ "available": true, "results": items })).into_response()
        }
        // FTS rejects some punctuation as a malformed MATCH expression; treat
        // that as "no results" rather than a 500 so typing mid-query is calm.
        Err(_) => Json(json!({ "available": true, "results": [] })).into_response(),
    }
}

// ── Archiver ─────────────────────────────────────────────────────────

async fn archiver_channels(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("archiver") {
        return r;
    }
    let Some(conn) = open_ro(&archiver_db()) else {
        return Json(json!({ "available": false, "channels": [] })).into_response();
    };
    match strivo_plugins::archiver::db::list_channels(&conn) {
        Ok(chans) => {
            let items: Vec<Value> = chans
                .into_iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "name": c.name,
                        "url": c.url,
                        "platform": c.platform,
                        "archive_dir": c.archive_dir,
                        "last_scan": c.last_scan,
                        "video_count": c.video_count,
                        "downloaded_count": c.downloaded_count,
                    })
                })
                .collect();
            Json(json!({ "available": true, "channels": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

async fn archiver_videos(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("archiver") {
        return r;
    }
    let Some(conn) = open_ro(&archiver_db()) else {
        return Json(json!({ "available": false, "videos": [] })).into_response();
    };
    match strivo_plugins::archiver::db::list_videos(&conn, channel_id) {
        Ok(vids) => {
            let items: Vec<Value> = vids
                .into_iter()
                .map(|v| {
                    json!({
                        "video_id": v.video_id,
                        "title": v.title,
                        "upload_date": v.upload_date,
                        "duration": v.duration,
                        "playlist": v.playlist,
                        "downloaded": v.downloaded,
                    })
                })
                .collect();
            Json(json!({ "available": true, "videos": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

// ── Viewguard ────────────────────────────────────────────────────────

async fn viewguard_verdicts(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("viewguard") {
        return r;
    }
    let Some(conn) = viewguard_db().as_deref().and_then(open_ro) else {
        return Json(json!({ "available": false, "verdicts": [] })).into_response();
    };
    match strivo_plugins::viewguard::store::all_verdicts(&conn) {
        Ok(verdicts) => {
            let items: Vec<Value> = verdicts
                .into_iter()
                .map(|v| {
                    let contributors: Value =
                        serde_json::from_str(&v.contributors_json).unwrap_or(Value::Null);
                    json!({
                        "channel_id": v.channel_id,
                        "stream_started_at": v.stream_started_at.to_rfc3339(),
                        "stream_ended_at": v.stream_ended_at.map(|t| t.to_rfc3339()),
                        "final_score": v.final_score,
                        "band": v.band,
                        "contributors": contributors,
                    })
                })
                .collect();
            Json(json!({ "available": true, "verdicts": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

async fn viewguard_samples(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("viewguard") {
        return r;
    }
    let Some(conn) = viewguard_db().as_deref().and_then(open_ro) else {
        return Json(json!({ "available": false, "samples": [] })).into_response();
    };
    match strivo_plugins::viewguard::store::samples_for(&conn, &channel_id, 240) {
        Ok(samples) => {
            let items: Vec<Value> = samples
                .into_iter()
                .map(|s| json!({ "ts": s.ts, "viewers": s.viewers }))
                .collect();
            Json(json!({ "available": true, "samples": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

// ── Insights (read crunchr.db) ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WordsQuery {
    /// "global" (default) or "recording".
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    recording: Option<String>,
    #[serde(default)]
    stopwords: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn insights_words(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<WordsQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Json(json!({ "available": false, "words": [] })).into_response();
    };
    let include_stopwords = q.stopwords.unwrap_or(false);
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let result = if q.scope.as_deref() == Some("recording") {
        match q.recording.as_deref() {
            Some(rec) => strivo_plugins::insights::frequency::top_words_for_recording(
                &conn,
                rec,
                limit,
                include_stopwords,
            ),
            None => {
                return Problem::bad_request("recording scope needs ?recording=<id>")
                    .into_response()
            }
        }
    } else {
        strivo_plugins::insights::frequency::top_words_global(&conn, limit, include_stopwords)
    };
    match result {
        Ok(rows) => {
            let items: Vec<Value> = rows
                .into_iter()
                .map(|r| json!({ "word": r.word, "count": r.count }))
                .collect();
            Json(json!({ "available": true, "words": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

async fn insights_topics(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Json(json!({ "available": false, "topics": [] })).into_response();
    };
    match strivo_plugins::insights::topics::cross_recording_topics(&conn) {
        Ok(mut rows) => {
            rows.sort_by_key(|t| std::cmp::Reverse(t.count));
            let items: Vec<Value> = rows
                .into_iter()
                .map(|t| {
                    json!({
                        "topic": t.topic,
                        "count": t.count,
                        "first_seen": t.first_seen,
                        "last_seen": t.last_seen,
                    })
                })
                .collect();
            Json(json!({ "available": true, "topics": items })).into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

async fn insights_speakers(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Json(json!({ "available": false, "speakers": [], "sentiment": null }))
            .into_response();
    };
    let airtime = strivo_plugins::insights::speakers::airtime_for_recording(&conn, &id);
    let sentiment = strivo_plugins::insights::speakers::sentiment_for_recording(&conn, &id);
    match airtime {
        Ok(rows) => {
            let speakers: Vec<Value> = rows
                .into_iter()
                .map(|s| {
                    json!({ "speaker": s.speaker, "seconds": s.seconds, "segments": s.segments })
                })
                .collect();
            let sentiment_label = sentiment
                .ok()
                .flatten()
                .map(|p| p.label.label().to_string());
            Json(json!({
                "available": true,
                "speakers": speakers,
                "sentiment": sentiment_label,
            }))
            .into_response()
        }
        Err(e) => Problem::internal(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ExportQuery {
    /// "csv" (default) or "json".
    #[serde(default)]
    fmt: Option<String>,
    #[serde(default)]
    stopwords: Option<bool>,
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/v1/plugins/insights/export` — global word frequencies as a
/// downloadable CSV or JSON. Built inline (not via the plugin's disk export)
/// so the browser gets a direct attachment.
async fn insights_export(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    if let Err(r) = gate_pro("insights") {
        return r;
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let include_stopwords = q.stopwords.unwrap_or(false);
    let limit = q.limit.unwrap_or(200).clamp(1, 5000);
    let rows = match strivo_plugins::insights::frequency::top_words_global(
        &conn,
        limit,
        include_stopwords,
    ) {
        Ok(r) => r,
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };

    let as_json = q.fmt.as_deref() == Some("json");
    if as_json {
        let body: Vec<Value> = rows
            .into_iter()
            .map(|r| json!({ "word": r.word, "count": r.count }))
            .collect();
        (
            [
                (header::CONTENT_TYPE, "application/json"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"word-frequency.json\"",
                ),
            ],
            serde_json::to_string_pretty(&body).unwrap_or_default(),
        )
            .into_response()
    } else {
        let mut csv = String::from("word,count\n");
        for r in rows {
            // Quote the word and escape embedded quotes — words are user
            // speech, so commas/quotes are possible.
            let w = r.word.replace('"', "\"\"");
            csv.push_str(&format!("\"{}\",{}\n", w, r.count));
        }
        (
            [
                (header::CONTENT_TYPE, "text/csv"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"word-frequency.csv\"",
                ),
            ],
            csv,
        )
            .into_response()
    }
}

// ── Captions sidecar (Crunchr → WebVTT) ──────────────────────────────

fn fmt_vtt_time(sec: f64) -> String {
    let sec = sec.max(0.0);
    let total_ms = (sec * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let s = (total_ms / 1000) % 60;
    let m = (total_ms / 60_000) % 60;
    let h = total_ms / 3_600_000;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Escape `<` / `>` / `&` for embedding in a VTT cue body. `<v Name>` is a
/// real VTT voice tag and must NOT be escaped — we emit that ourselves.
fn vtt_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `GET /api/v1/recordings/{id}/captions.vtt` — Crunchr's transcript segments
/// rendered as WebVTT so the in-app player's `<track>` picks them up. Returns
/// 404 when Crunchr hasn't transcribed the recording yet (so the player's
/// HEAD probe correctly hides the CC button).
async fn recording_captions(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let Some(conn) = open_ro(&crunchr_db()) else {
        return Problem::not_found("crunchr has no data yet").into_response();
    };
    let detail = match strivo_plugins::crunchr::db::recording_detail(&conn, &id) {
        Ok(Some(d)) => d,
        Ok(None) => return Problem::not_found("recording not transcribed").into_response(),
        Err(e) => return Problem::internal(e.to_string()).into_response(),
    };
    if detail.segments.is_empty() {
        return Problem::not_found("no segments").into_response();
    }
    let mut body = String::from("WEBVTT\n\n");
    for seg in &detail.segments {
        body.push_str(&format!(
            "{} --> {}\n",
            fmt_vtt_time(seg.start_sec),
            fmt_vtt_time(seg.end_sec)
        ));
        let text = vtt_escape(seg.text.trim());
        if let Some(spk) = seg.speaker.as_deref().filter(|s| !s.is_empty()) {
            body.push_str(&format!("<v {}>{}\n\n", vtt_escape(spk), text));
        } else {
            body.push_str(&format!("{text}\n\n"));
        }
    }
    (
        [
            (header::CONTENT_TYPE, "text/vtt; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/plugins", get(index))
        .route(
            "/api/v1/plugins/crunchr/recordings",
            get(crunchr_recordings),
        )
        .route(
            "/api/v1/plugins/crunchr/recordings/{id}",
            get(crunchr_recording),
        )
        .route("/api/v1/plugins/crunchr/search", get(crunchr_search))
        .route("/api/v1/plugins/archiver/channels", get(archiver_channels))
        .route(
            "/api/v1/plugins/archiver/channels/{channel_id}/videos",
            get(archiver_videos),
        )
        .route(
            "/api/v1/plugins/viewguard/verdicts",
            get(viewguard_verdicts),
        )
        .route(
            "/api/v1/plugins/viewguard/channels/{channel_id}/samples",
            get(viewguard_samples),
        )
        .route("/api/v1/plugins/insights/words", get(insights_words))
        .route("/api/v1/plugins/insights/topics", get(insights_topics))
        .route(
            "/api/v1/plugins/insights/recordings/{id}/speakers",
            get(insights_speakers),
        )
        .route("/api/v1/plugins/insights/export", get(insights_export))
        .route(
            "/api/v1/recordings/{id}/captions.vtt",
            get(recording_captions),
        )
        .route(
            "/api/v1/plugins/chapters/{id}",
            axum::routing::post(chapters_generate),
        )
        .route(
            "/api/v1/plugins/cuepoints/{id}",
            axum::routing::post(cuepoints_generate),
        )
        .route(
            "/api/v1/plugins/clipper/{id}/analyze",
            axum::routing::post(clipper_analyze),
        )
        .route(
            "/api/v1/plugins/clipper/{id}/extract",
            axum::routing::post(clipper_extract),
        )
        .route(
            "/api/v1/plugins/clipper/{id}/clips",
            get(clipper_list_clips),
        )
        .route(
            "/api/v1/plugins/thumbnails/{id}",
            axum::routing::post(thumbnails_generate),
        )
        .route(
            "/api/v1/plugins/thumbnails/{id}/{stem}",
            get(thumbnails_list),
        )
        .route(
            "/api/v1/plugins/thumbnails/file",
            get(thumbnails_serve_file),
        )
        .route("/api/v1/plugins/insights/compare", get(insights_compare))
        .route(
            "/api/v1/plugins/insights/retention/{id}",
            get(insights_retention),
        )
        .route("/api/v1/plugins/captions/{id}", get(captions_export))
        .route(
            "/api/v1/plugins/captions/{id}/style",
            get(captions_style_load).post(captions_style_save),
        )
        .route("/api/v1/plugins/multitrack/{id}", get(multitrack_list))
        .route(
            "/api/v1/plugins/multitrack/{id}/extract",
            axum::routing::post(multitrack_extract),
        )
        .route("/api/v1/plugins/brandsafe/{id}", get(brandsafe_scan))
        .route(
            "/api/v1/plugins/reuse/{id}/generate",
            axum::routing::post(reuse_generate),
        )
        .route("/api/v1/plugins/reuse/{id}", get(reuse_list))
        .route("/api/v1/plugins/casebook/{id}", get(casebook_generate))
        .route("/api/v1/plugins/heatmap/{id}", get(heatmap_compute))
        .route(
            "/api/v1/plugins/editor/{id}",
            get(editor_load).post(editor_save),
        )
        .route(
            "/api/v1/plugins/editor/{id}/render",
            axum::routing::post(editor_render),
        )
        .route(
            "/api/v1/plugins/editor/{id}/revisions",
            get(editor_revisions_list),
        )
        .route(
            "/api/v1/plugins/editor/{id}/revisions/{rev_id}/restore",
            axum::routing::post(editor_revisions_restore),
        )
        .route("/api/v1/plugins/viewguard/trend", get(viewguard_trend))
        .route(
            "/api/v1/plugins/broll/{id}",
            axum::routing::post(broll_suggest),
        )
        .route(
            "/api/v1/plugins/chat-density/{id}",
            axum::routing::post(chat_density_compute),
        )
        .route(
            "/api/v1/plugins/deadair/{id}",
            axum::routing::post(deadair_detect),
        )
        .route(
            "/api/v1/plugins/branding/{id}",
            get(branding_load).post(branding_save),
        )
        .route("/api/v1/plugins/multistream/tiles", get(multistream_tiles))
        .route("/api/v1/plugins/chat/rooms", get(chat_rooms))
        .route(
            "/api/v1/plugins/chat/parse",
            axum::routing::post(chat_parse),
        )
        .route("/api/v1/chat/send", axum::routing::post(chat_send_message))
        .route("/api/v1/dataviz/run", axum::routing::post(dataviz_run))
        .route(
            "/api/v1/plugins/loudness/{id}",
            axum::routing::post(loudness_measure),
        )
        .route(
            "/api/v1/plugins/structure/{id}",
            axum::routing::post(structure_classify),
        )
        .route(
            "/api/v1/plugins/automation/{id}",
            get(automation_load).post(automation_save),
        )
        .route(
            "/api/v1/plugins/scenes/{id}",
            get(scenes_list).post(scenes_capture),
        )
        .route(
            "/api/v1/plugins/scenes/{id}/{scene_id}/restore",
            axum::routing::post(scenes_restore),
        )
        .route(
            "/api/v1/plugins/scenes/{id}/{scene_id}",
            axum::routing::delete(scenes_delete),
        )
        .route(
            "/api/v1/plugins/schedule-optimizer/{id}",
            axum::routing::post(schedule_optimizer_recommend),
        )
        .route(
            "/api/v1/plugins/beat-detect/{id}",
            axum::routing::post(beat_detect_run),
        )
        .route("/api/v1/plugins/vad/{id}", axum::routing::post(vad_run))
        .route(
            "/api/v1/plugins/sidechain/{id}",
            axum::routing::post(sidechain_run),
        )
        .route(
            "/api/v1/plugins/insert-fx/{id}",
            get(insert_fx_load).post(insert_fx_save),
        )
        .route(
            "/api/v1/plugins/insert-fx/{id}/preset/{bus}",
            axum::routing::post(insert_fx_preset),
        )
        .route(
            "/api/v1/plugins/pitch/{id}",
            get(pitch_load).post(pitch_save),
        )
        .route(
            "/api/v1/plugins/pitch/{id}/fit",
            axum::routing::post(pitch_fit),
        )
        // Distinct namespace so we don't collide with existing
        // /plugins/<name>/<id> routes (sidechain, automation, etc.)
        // where 'storage' would otherwise be interpreted as a
        // recording id.
        .route(
            "/api/v1/plugin-storage/{name}",
            get(plugin_storage_size).delete(plugin_storage_clear),
        )
}

/// Per-plugin storage directory under data_dir/plugins/<name>. Only
/// the alphanumeric + hyphen subset that matches our shipping plugin
/// names is allowed through — guard against path traversal.
#[derive(Debug, Deserialize)]
pub(super) struct DatavizBody {
    pub corpus: strivo_dataviz::Corpus,
    pub experiment: strivo_dataviz::Experiment,
}

/// `POST /api/v1/dataviz/run` — run a single experiment over an
/// arbitrary corpus and return the chart-ready Series. The corpus
/// is supplied by the client (assembled SPA-side from Crunchr
/// transcripts) so this endpoint is a pure transform; no IO.
async fn dataviz_run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<DatavizBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let series = strivo_dataviz::run(&body.corpus, &body.experiment);
    Json(json!({ "series": series })).into_response()
}

#[derive(Debug, Deserialize)]
pub(super) struct ChatSendBody {
    pub room: String,
    pub text: String,
}

/// `POST /api/v1/chat/send` — outbound message to a Twitch IRC room.
/// Requires a `STRIVO_TWITCH_CHAT_OAUTH` env var (`oauth:<token>`
/// scoped `chat:edit`) and `STRIVO_TWITCH_CHAT_LOGIN`. When unset,
/// returns a 503 Problem describing the missing config so the SPA
/// can surface a useful hint without guessing.
async fn chat_send_message(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ChatSendBody>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let token = std::env::var("STRIVO_TWITCH_CHAT_OAUTH").unwrap_or_default();
    let login = std::env::var("STRIVO_TWITCH_CHAT_LOGIN").unwrap_or_default();
    if token.is_empty() || login.is_empty() {
        return Problem::unavailable(
            "chat compose needs STRIVO_TWITCH_CHAT_OAUTH (oauth:<token> with chat:edit scope) and STRIVO_TWITCH_CHAT_LOGIN set on the daemon environment",
        )
        .into_response();
    }
    let room = body.room.trim_start_matches('#').to_ascii_lowercase();
    if room.is_empty() {
        return Problem::bad_request("room required").into_response();
    }
    let text = body.text.replace(['\r', '\n'], " ");
    // Open a short-lived IRC connection — keeps the implementation
    // simple (one connect per message). The per-session join + listen
    // happens entirely client-side over WS today; outbound messages
    // are infrequent enough that connection overhead is fine.
    let irc = match tokio::net::TcpStream::connect("irc.chat.twitch.tv:6667").await {
        Ok(s) => s,
        Err(e) => return Problem::internal(format!("irc connect: {e}")).into_response(),
    };
    use tokio::io::AsyncWriteExt;
    let (rx, mut tx) = tokio::io::split(irc);
    drop(rx); // read half unused — we don't ack
              // PASS / NICK auth then PRIVMSG.
    let pass = if token.starts_with("oauth:") {
        token.clone()
    } else {
        format!("oauth:{token}")
    };
    let lines = [
        format!("PASS {pass}\r\n"),
        format!("NICK {}\r\n", login.to_ascii_lowercase()),
        format!("PRIVMSG #{room} :{text}\r\n"),
        "QUIT :strivo\r\n".to_string(),
    ];
    for line in lines {
        if let Err(e) = tx.write_all(line.as_bytes()).await {
            return Problem::internal(format!("irc write: {e}")).into_response();
        }
    }
    let _ = tx.shutdown().await;
    Json(json!({ "ok": true, "room": room })).into_response()
}

fn plugin_storage_dir(name: &str) -> Option<std::path::PathBuf> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(
        strivo_core::config::AppConfig::data_dir()
            .join("plugins")
            .join(name),
    )
}

/// Walk a directory and return (total_bytes, file_count). Symlinks
/// are not followed. Missing dir = (0, 0).
fn plugin_storage_walk(dir: &std::path::Path) -> (u64, u64) {
    fn inner(dir: &std::path::Path, bytes: &mut u64, files: &mut u64) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() {
                *bytes += meta.len();
                *files += 1;
            } else if meta.is_dir() {
                inner(&entry.path(), bytes, files);
            }
        }
    }
    let mut bytes = 0u64;
    let mut files = 0u64;
    inner(dir, &mut bytes, &mut files);
    (bytes, files)
}

/// `GET /api/v1/plugins/<name>/storage` — disk usage of the plugin's
/// data directory (path + total bytes + file count).
async fn plugin_storage_size(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let Some(dir) = plugin_storage_dir(&name) else {
        return Problem::bad_request("plugin name must be alphanumeric / dash / underscore")
            .into_response();
    };
    let (bytes, file_count) = plugin_storage_walk(&dir);
    Json(json!({
        "plugin": name,
        "path": dir.to_string_lossy(),
        "bytes": bytes,
        "file_count": file_count,
        "exists": dir.exists(),
    }))
    .into_response()
}

/// `DELETE /api/v1/plugins/<name>/storage` — wipe the plugin's data
/// directory. Returns the byte/file counts that were freed.
async fn plugin_storage_clear(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let Some(dir) = plugin_storage_dir(&name) else {
        return Problem::bad_request("plugin name must be alphanumeric / dash / underscore")
            .into_response();
    };
    if !dir.exists() {
        return Json(json!({
            "plugin": name,
            "bytes_removed": 0,
            "files_removed": 0,
        }))
        .into_response();
    }
    let (bytes, files) = plugin_storage_walk(&dir);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Json(json!({
            "plugin": name,
            "bytes_removed": bytes,
            "files_removed": files,
        }))
        .into_response(),
        Err(e) => Problem::internal(format!("remove_dir_all: {e}")).into_response(),
    }
}
