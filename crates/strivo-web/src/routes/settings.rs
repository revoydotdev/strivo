//! `/api/v1/*` core surface: health, channels/patreon snapshots, recordings
//! CRUD (start/stop/thumb/probe/remux/delete), schedule, settings + platform
//! credentials, storage/gantt gauges, and logs.
//!
//! Split out of `routes::api` — see that module's doc comment for the
//! overall `/api/v1/*` auth model (`X-Api-Key` header or session cookie via
//! `check_key`).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use strivo_core::ipc::{ClientMessage, ServerMessage};
use strivo_core::platform::PlatformKind;
use strivo_core::recording::RecordingCommand;
// Archiver's own `[archiver]` config.toml section isn't a strivo-core
// type (see ADR 0001 CE01) — it's owned by strivo-plugins, which this
// crate already depends on under the creator feature.
use std::time::Duration;
#[cfg(feature = "creator")]
use strivo_plugins::archiver::types::ArchiverConfig;
use uuid::Uuid;

use crate::server::AppState;

/// Authorize a request via EITHER the `X-Api-Key` header (programmatic
/// clients) OR a valid `strivo_session` cookie (browser, set by /login).
/// The browser SPA only carries the cookie, so cookie support is what
/// lets it reach /channels, /recordings, … after logging in. (W3.)
///
/// `pub(crate)` — every other `routes::*` handler module that moved out of
/// `routes::api` calls this same gate.
pub(crate) fn check_key(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    // Single dual-track gate: valid X-Api-Key header OR a valid session
    // cookie (plain or `__Host-` name). See login::check_dual.
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

async fn channels(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.snapshot().await {
        Ok(ServerMessage::StateSnapshot { channels, .. }) => {
            Json(json!({ "channels": channels })).into_response()
        }
        Ok(_) => Json(json!({ "channels": [] })).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `GET /api/v1/patreon` — current Patreon creators + their video posts,
/// cached in the daemon snapshot so the SPA can seed its Patreon section
/// on load instead of waiting up to a full poll interval for the next
/// patreon-state SSE event.
async fn patreon(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.snapshot().await {
        Ok(ServerMessage::StateSnapshot {
            patreon_creators,
            patreon_posts,
            ..
        }) => Json(json!({ "creators": patreon_creators, "posts": patreon_posts })).into_response(),
        Ok(_) => Json(json!({ "creators": [], "posts": [] })).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// Serialise a RecordingJob and inject a `file_exists` flag computed from
/// the on-disk state of its `output_path`. The webui surfaces this as a
/// "FILE MISSING" overlay over the thumb so dead journal rows (file moved
/// or deleted out from under the daemon) are visible without a route probe.
fn augment_recording(job: &strivo_core::recording::job::RecordingJob) -> serde_json::Value {
    let mut v = serde_json::to_value(job).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "file_exists".to_string(),
            serde_json::Value::Bool(job.output_path.exists()),
        );
    }
    v
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct PageQuery {
    /// Zero-based opaque position. Kept numeric in the first version so old
    /// clients can adopt pagination without a second identifier format.
    #[serde(default)]
    pub(crate) cursor: Option<usize>,
    /// Omitted means the legacy full snapshot. Supplying a limit opts into
    /// the scalable response while preserving existing integrations.
    #[serde(default)]
    pub(crate) limit: Option<usize>,
}

pub(crate) fn page_bounds(total: usize, query: &PageQuery) -> (usize, usize, Option<usize>) {
    let start = query.cursor.unwrap_or(0).min(total);
    let end = match query.limit {
        Some(limit) => start.saturating_add(limit.clamp(1, 500)).min(total),
        None => total,
    };
    let next = (end < total).then_some(end);
    (start, end, next)
}

/// The journal is the archive authority; the daemon snapshot is only an
/// overlay for jobs which are still present in memory.  Keeping this lookup
/// here makes detail, media serving, probing and artwork agree about what an
/// archived recording is.
pub(crate) async fn resolve_recording(
    state: &AppState,
    id: Uuid,
) -> Result<strivo_core::recording::job::RecordingJob, String> {
    if let Ok(ServerMessage::StateSnapshot { recordings, .. }) = state.snapshot().await {
        if let Some(job) = recordings.get(&id) {
            return Ok(job.clone());
        }
    }
    let db = state.jobs_db().await?;
    db.load_recording_job(id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "recording not found".to_string())
}

async fn thumbnail_lock(state: &AppState, id: Uuid) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let mut locks = state.thumbnail_locks.lock().await;
    locks.retain(|_, weak| weak.strong_count() > 0);
    if let Some(lock) = locks.get(&id).and_then(std::sync::Weak::upgrade) {
        lock
    } else {
        let lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(id, std::sync::Arc::downgrade(&lock));
        lock
    }
}

fn combined_page_bounds(
    extras_total: usize,
    start: usize,
    limit: usize,
) -> (usize, usize, usize, usize) {
    let extra_start = start.min(extras_total);
    let extra_end = extra_start.saturating_add(limit).min(extras_total);
    let durable_start = start.saturating_sub(extras_total);
    let durable_room = limit.saturating_sub(extra_end - extra_start);
    (extra_start, extra_end, durable_start, durable_room)
}

async fn thumbnail_failed_recently(state: &AppState, id: Uuid) -> bool {
    state
        .thumbnail_failures
        .lock()
        .await
        .get(&id)
        .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
}

async fn recordings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let db = match state.jobs_db().await {
        Ok(db) => db,
        Err(e) => return crate::problem::Problem::unavailable(e).into_response(),
    };
    let start = query.cursor.unwrap_or(0);
    let limit = query.limit.unwrap_or(500).clamp(1, 500);
    let snapshot = match state.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => Some(recordings),
        _ => None,
    };
    let mut live_extras = Vec::new();
    if let Some(recordings) = snapshot.as_ref() {
        for live in recordings.values() {
            if matches!(
                live.state,
                strivo_core::recording::job::RecordingState::Finished
                    | strivo_core::recording::job::RecordingState::Failed
            ) {
                continue;
            }
            if db
                .load_recording_job(live.id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                live_extras.push(live.clone());
            }
        }
    }
    live_extras.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    let extras_total = live_extras.len();
    let (extra_start, extra_end, durable_start, durable_room) =
        combined_page_bounds(extras_total, start, limit);
    let mut items = live_extras[extra_start..extra_end].to_vec();
    // Ask for one row even when live items fill this page: the query also
    // returns the durable total needed to produce a progressing cursor.
    match db
        .load_recording_jobs_page(durable_start, durable_room.max(1))
        .await
    {
        Ok((mut durable, durable_total)) => {
            durable.truncate(durable_room);
            if let Some(recordings) = snapshot.as_ref() {
                for item in &mut durable {
                    if let Some(live) = recordings.get(&item.id) {
                        *item = live.clone();
                    }
                }
            }
            items.append(&mut durable);
            let total = extras_total.saturating_add(durable_total);
            let end = start.saturating_add(items.len());
            let next_cursor = (end < total).then_some(end);
            // Up to 500 rows, each stat'ing output_path via augment_recording's
            // Path::exists() — a blocking syscall per row. Run off the reactor
            // so a large page doesn't stall every other request on this
            // worker thread while the syscalls complete (B-03), mirroring the
            // spawn_blocking wrap around scan_existing_recordings
            // (src/daemon.rs).
            let augmented: Vec<_> = tokio::task::spawn_blocking(move || {
                items.iter().map(augment_recording).collect::<Vec<_>>()
            })
            .await
            .unwrap_or_else(|e| {
                tracing::error!("augment_recording task panicked: {e}");
                Vec::new()
            });
            Json(json!({
                "recordings": augmented,
                "total": total,
                "next_cursor": next_cursor,
            }))
            .into_response()
        }
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

async fn recording_one(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match resolve_recording(&state, id).await {
        Ok(job) => {
            let value = tokio::task::spawn_blocking(move || augment_recording(&job))
                .await
                .unwrap_or_else(|e| {
                    tracing::error!("augment_recording task panicked: {e}");
                    serde_json::Value::Null
                });
            Json(value).into_response()
        }
        Err(e) if e == "recording not found" => {
            crate::problem::Problem::not_found(e).into_response()
        }
        Err(e) => crate::problem::Problem::unavailable(e).into_response(),
    }
}

/// `GET /api/v1/recordings/{id}/probe` — on-demand ffprobe against the
/// recording's `output_path`. Returns a normalised summary (container,
/// duration, bitrate, per-stream codec/resolution/fps for video and
/// codec/channels/sample-rate/bitrate for audio). 404 if the recording
/// isn't in the snapshot, 503 if ffprobe isn't on PATH, 502 if ffprobe
/// errors.
async fn recording_probe(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let path = match resolve_recording(&state, id).await {
        Ok(job) => job.output_path,
        Err(e) if e == "recording not found" => {
            return crate::problem::Problem::not_found(e).into_response()
        }
        Err(e) => return crate::problem::Problem::unavailable(e).into_response(),
    };
    if !path.exists() {
        return crate::problem::Problem::not_found("file missing").into_response();
    }
    let canonical = match tokio::fs::canonicalize(&path).await {
        Ok(path) => path,
        Err(e) => {
            return crate::problem::Problem::internal(format!("probe canonicalize: {e}"))
                .into_response()
        }
    };
    let metadata = match tokio::fs::metadata(&canonical).await {
        Ok(metadata) => metadata,
        Err(e) => {
            return crate::problem::Problem::internal(format!("probe metadata: {e}"))
                .into_response()
        }
    };
    let modified = metadata.modified().ok();
    if let Some(hit) = state.probe_cache.read().await.get(&canonical) {
        if hit.len == metadata.len() && hit.modified == modified {
            let mut value = hit.value.clone();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("cached".into(), serde_json::Value::Bool(true));
            }
            return Json(value).into_response();
        }
    }
    let _probe_permit = match state.probe_slots.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return crate::problem::Problem::unavailable("probe worker pool closed").into_response()
        }
    };
    // Recheck after queueing: a preceding waiter may have populated the
    // cache while this request waited for a process slot.
    if let Some(hit) = state.probe_cache.read().await.get(&canonical) {
        if hit.len == metadata.len() && hit.modified == modified {
            let mut value = hit.value.clone();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("cached".into(), serde_json::Value::Bool(true));
            }
            return Json(value).into_response();
        }
    }
    let probe_started = std::time::Instant::now();
    let mut probe = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffprobe"));
    probe.kill_on_drop(true);
    probe
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(&canonical);
    let out = match tokio::time::timeout(Duration::from_secs(30), probe.output()).await {
        Ok(Ok(o)) => o,
        Err(_) => return crate::problem::Problem::unavailable("ffprobe timed out").into_response(),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return crate::problem::Problem::unavailable("ffprobe not installed").into_response();
        }
        Ok(Err(e)) => {
            return crate::problem::Problem::internal(format!("ffprobe spawn: {e}"))
                .into_response();
        }
    };
    if !out.status.success() {
        return crate::problem::Problem::new(
            StatusCode::BAD_GATEWAY,
            format!(
                "ffprobe failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        )
        .into_response();
    }
    let raw: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(e) => {
            return crate::problem::Problem::internal(format!("ffprobe parse: {e}")).into_response()
        }
    };

    // Normalise: pull just the bits the Info modal renders. Keep `raw` out of
    // the response — the full ffprobe dump is noisy and exposes internals.
    let fmt = raw
        .get("format")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let duration_secs = fmt
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok());
    let bit_rate = fmt
        .get("bit_rate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());
    let size_bytes = fmt
        .get("size")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());
    let container = fmt
        .get("format_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    fn parse_fps(s: &str) -> Option<f64> {
        // r_frame_rate is "30000/1001" or "30/1" — never a bare float.
        let mut it = s.split('/');
        let n: f64 = it.next()?.parse().ok()?;
        let d: f64 = it.next().unwrap_or("1").parse().ok()?;
        if d == 0.0 {
            None
        } else {
            Some(n / d)
        }
    }

    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subtitle = Vec::new();
    if let Some(arr) = raw.get("streams").and_then(|v| v.as_array()) {
        for s in arr {
            let codec_type = s.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
            let codec_name = s
                .get("codec_name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let br = s
                .get("bit_rate")
                .and_then(|v| v.as_str())
                .and_then(|x| x.parse::<u64>().ok());
            match codec_type {
                "video" => video.push(json!({
                    "codec": codec_name,
                    "width": s.get("width").and_then(|v| v.as_u64()),
                    "height": s.get("height").and_then(|v| v.as_u64()),
                    "fps": s.get("r_frame_rate").and_then(|v| v.as_str()).and_then(parse_fps),
                    "bit_rate": br,
                    "pix_fmt": s.get("pix_fmt").and_then(|v| v.as_str()).map(str::to_string),
                })),
                "audio" => audio.push(json!({
                    "codec": codec_name,
                    "channels": s.get("channels").and_then(|v| v.as_u64()),
                    "channel_layout": s.get("channel_layout").and_then(|v| v.as_str()).map(str::to_string),
                    "sample_rate": s.get("sample_rate").and_then(|v| v.as_str()).and_then(|x| x.parse::<u64>().ok()),
                    "bit_rate": br,
                    "language": s.get("tags").and_then(|t| t.get("language")).and_then(|v| v.as_str()).map(str::to_string),
                })),
                "subtitle" => subtitle.push(json!({
                    "codec": codec_name,
                    "language": s.get("tags").and_then(|t| t.get("language")).and_then(|v| v.as_str()).map(str::to_string),
                })),
                _ => {}
            }
        }
    }

    let value = json!({
        "container": container,
        "duration_secs": duration_secs,
        "bit_rate": bit_rate,
        "size_bytes": size_bytes,
        "video": video,
        "audio": audio,
        "subtitle": subtitle,
        "cached": false,
    });
    tracing::info!(
        media.path = %canonical.display(),
        media.bytes = metadata.len(),
        duration_ms = probe_started.elapsed().as_secs_f64() * 1000.0,
        "ffprobe completed"
    );
    {
        let mut cache = state.probe_cache.write().await;
        cache.insert(
            canonical,
            crate::server::ProbeCacheEntry {
                len: metadata.len(),
                modified,
                value: value.clone(),
            },
        );
        // The cache is advisory and bounded. Oldest-by-mtime is not worth a
        // second index here; a deterministic clear prevents unbounded growth.
        if cache.len() > 1_024 {
            cache.clear();
        }
    }
    Json(value).into_response()
}

#[derive(Debug, Deserialize)]
struct ScheduleAddPayload {
    channel: String,
    cron: String,
    #[serde(default)]
    duration: String,
}

/// `POST /api/v1/schedule` — append a new entry to config.schedule.
async fn schedule_add(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ScheduleAddPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if body.channel.trim().is_empty() || body.cron.trim().is_empty() {
        return crate::problem::Problem::bad_request("channel and cron required").into_response();
    }
    // Validate the cron expression now so a typo doesn't show up at
    // the next firing minute as a silent no-op.
    let cron_expr = if body.cron.split_whitespace().count() == 5 {
        format!("0 {}", body.cron)
    } else {
        body.cron.clone()
    };
    if <cron::Schedule as std::str::FromStr>::from_str(&cron_expr).is_err() {
        return crate::problem::Problem::bad_request("invalid cron expression").into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    cfg.schedule.push(strivo_core::config::ScheduleEntry {
        channel: body.channel.trim().to_string(),
        cron: body.cron.trim().to_string(),
        duration: if body.duration.trim().is_empty() {
            "4h".to_string()
        } else {
            body.duration.trim().to_string()
        },
    });
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response()
}

/// `DELETE /api/v1/schedule/<index>` — drop one entry by zero-based
/// index. The schedule isn't named so indices are the natural key.
async fn schedule_delete(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(index): Path<usize>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    if index >= cfg.schedule.len() {
        return crate::problem::Problem::not_found("no such schedule entry").into_response();
    }
    cfg.schedule.remove(index);
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    Json(json!({ "ok": true })).into_response()
}

async fn schedule(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.config().await {
        Ok(cfg) => {
            // Annotate each entry with its next fire time (RFC3339) so the
            // webui "Upcoming" row can sort + display it. Mirrors the cron
            // parse in src/recording/schedule.rs (5-field → prepend "0 ").
            let entries: Vec<_> = cfg
                .schedule
                .iter()
                .map(|e| {
                    let cron_expr = if e.cron.split_whitespace().count() == 5 {
                        format!("0 {}", e.cron)
                    } else {
                        e.cron.clone()
                    };
                    let next_fire = std::str::FromStr::from_str(&cron_expr)
                        .ok()
                        .and_then(|s: cron::Schedule| s.upcoming(chrono::Utc).next())
                        .map(|dt| dt.to_rfc3339());
                    json!({
                        "channel": e.channel,
                        "cron": e.cron,
                        "duration": e.duration,
                        "next_fire": next_fire,
                    })
                })
                .collect();
            Json(json!({ "schedule": entries })).into_response()
        }
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

async fn settings(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.config().await {
        Ok(cfg) => {
            // Strip secrets — never expose client_secret / cookies_path.
            // We surface only the existence (`configured: bool`) of each
            // platform, not their credential payloads.
            #[allow(unused_mut)]
            let mut body = json!({
                "recording_dir": cfg.recording_dir,
                "poll_interval_secs": cfg.poll_interval_secs,
                "recording": cfg.recording,
                "ui": cfg.ui,
                "auto_record_channels": cfg.auto_record_channels,
                "capture_profiles": cfg.capture_profiles,
                "schedule": cfg.schedule,
                "notifications": cfg.notifications,
                "monitor_limits": cfg.monitor_limits,
                "plugin_toggles": cfg.plugin_toggles,
                "twitch_configured": cfg.twitch.is_some(),
                "youtube_configured": cfg.youtube.is_some(),
                "patreon_configured": cfg.patreon.is_some(),
                // Creator work is not publicly available, including in a
                // feature-enabled build. Keep the SPA on the released PVR
                // surface until the product clears its security/release gate;
                // only the developer override (debug or `dev-unlock` build
                // plus STRIVO_DEV_UNLOCK_ALL) exposes it.
                "creator_enabled": cfg!(feature = "creator")
                    && strivo_core::licence::gate::dev_unlock(),
            });
            // Creator Edition surfaces the Archiver config section.
            #[cfg(feature = "creator")]
            if let Some(obj) = body.as_object_mut() {
                let archiver: ArchiverConfig = cfg.plugin_section("archiver");
                obj.insert(
                    "archiver".into(),
                    serde_json::to_value(&archiver).unwrap_or(serde_json::Value::Null),
                );
            }
            Json(body).into_response()
        }
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

/// `GET /api/v1/health` — machine-readable health for CI/monitoring
/// (roadmap item 11). Unauthenticated liveness+readiness: probes the daemon
/// (IPC snapshot round-trip), the jobs DB (open), and free disk on the
/// recording filesystem. 200 when all pass, 503 when any check is degraded,
/// so a monitor can alert on the status code alone.
async fn health(State(state): State<AppState>) -> impl IntoResponse {
    // Daemon reachable: a successful snapshot proves the recorder process is
    // alive and answering on the IPC socket.
    let daemon_ok = state.snapshot().await.is_ok();

    // Jobs DB usable. This reports on the process's shared handle rather than
    // opening a second connection: re-opening ran the full schema batch on
    // every health poll, and the shared handle is also the more accurate
    // signal, since it is the one every other route actually uses.
    let db_ok = state.jobs_db().await.is_ok();

    // Free disk on the recording filesystem.
    let (disk, disk_ok) = match state.config().await {
        Ok(cfg) => {
            let (total, avail) = statvfs_bytes(&cfg.recording_dir).unwrap_or((0, 0));
            (
                json!({
                    "recording_dir": cfg.recording_dir,
                    "filesystem_total_bytes": total,
                    "filesystem_avail_bytes": avail,
                }),
                avail > 0,
            )
        }
        Err(_) => (serde_json::Value::Null, false),
    };

    let ok = daemon_ok && db_ok && disk_ok;
    let body = json!({
        "status": if ok { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "checks": {
            "daemon": if daemon_ok { "ok" } else { "unreachable" },
            "db": if db_ok { "ok" } else { "error" },
            "disk": if disk_ok { "ok" } else { "warn" },
        },
        "disk": disk,
    });
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

fn human_bytes(b: u64) -> String {
    const GB: u64 = 1 << 30;
    const MB: u64 = 1 << 20;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.0} MB", b as f64 / MB as f64)
    } else {
        format!("{b} B")
    }
}

/// Disk free-space severity for the recording filesystem (roadmap item 13).
/// <5% free = error, <15% = warn, else ok; unknown (total 0) = warn.
fn disk_severity(total: u64, avail: u64) -> &'static str {
    if total == 0 {
        return "warn";
    }
    let frac = avail as f64 / total as f64;
    if frac < 0.05 {
        "error"
    } else if frac < 0.15 {
        "warn"
    } else {
        "ok"
    }
}

fn add_check(
    checks: &mut Vec<serde_json::Value>,
    domain: &str,
    name: &str,
    sev: &str,
    message: String,
    fix: &str,
) {
    checks.push(json!({
        "domain": domain, "name": name, "severity": sev,
        "message": message, "fix": fix,
    }));
}

/// Build the "Platform Auth" domain rows for `/api/v1/health/checks`.
///
/// Pure so it's unit-testable without a running daemon: `snapshot` is
/// `None` when the IPC call failed, in which case every configured
/// platform reports "configured but not yet authenticated" (the daemon's
/// own unreachability is already covered by the Network domain).
///
/// Priority per configured platform, most specific first:
///   1. an `AuthIssue { source: OAuth }`     -> error, "credentials rejected"
///   2. an `AuthIssue { source: Cookies }`   -> error, "cookie session rejected"
///   3. a pending device-code login          -> warn, waiting-for-login
///   4. connected                            -> ok
///   5. otherwise                            -> warn, generic not-yet-authenticated
fn platform_auth_checks(
    cfg: &strivo_core::config::AppConfig,
    snapshot: Option<&ServerMessage>,
) -> Vec<serde_json::Value> {
    use strivo_core::platform::AuthSource;

    type PendingAuth<'a> = Option<&'a (PlatformKind, String, String)>;
    let (connected, pending_auth, auth_issues): (
        [bool; 3],
        PendingAuth<'_>,
        &[strivo_core::platform::AuthIssue],
    ) = match snapshot {
        Some(ServerMessage::StateSnapshot {
            twitch_connected,
            youtube_connected,
            patreon_connected,
            pending_auth,
            auth_issues,
            ..
        }) => (
            [*twitch_connected, *youtube_connected, *patreon_connected],
            pending_auth.as_ref(),
            auth_issues.as_slice(),
        ),
        _ => ([false, false, false], None, &[]),
    };

    let platforms = [
        (
            "Twitch",
            PlatformKind::Twitch,
            cfg.twitch.is_some(),
            connected[0],
        ),
        (
            "YouTube",
            PlatformKind::YouTube,
            cfg.youtube.is_some(),
            connected[1],
        ),
        (
            "Patreon",
            PlatformKind::Patreon,
            cfg.patreon.is_some(),
            connected[2],
        ),
    ];

    let mut out = Vec::new();
    for (name, kind, configured, connected) in platforms {
        if !configured {
            continue;
        }

        let oauth_issue = auth_issues
            .iter()
            .find(|i| i.kind == kind && i.source == AuthSource::OAuth);
        let cookie_issue = auth_issues
            .iter()
            .find(|i| i.kind == kind && i.source == AuthSource::Cookies);
        let pending = pending_auth.filter(|(p, ..)| *p == kind);

        // The base row for the platform's OAuth/device-code state.
        if let Some(issue) = oauth_issue {
            add_check(
                &mut out,
                "Platform Auth",
                name,
                "error",
                format!("{name}: credentials rejected — {}.", issue.reason),
                "Re-authenticate from Settings → Platforms (the daemon will show a device-code prompt).",
            );
        } else if let Some((_, uri, code)) = pending {
            add_check(
                &mut out,
                "Platform Auth",
                name,
                "warn",
                format!("Waiting for device-code login: enter {code} at {uri}."),
                "",
            );
        } else if connected {
            add_check(
                &mut out,
                "Platform Auth",
                name,
                "ok",
                format!("{name} authenticated."),
                "",
            );
        } else {
            add_check(
                &mut out,
                "Platform Auth",
                name,
                "warn",
                format!("{name} configured but not yet authenticated."),
                "The daemon retries automatically; if this persists, re-authenticate from Settings → Platforms.",
            );
        }

        // The cookie jar is a separate credential from the OAuth token, so
        // its row is independent — a platform can show both a rejected
        // OAuth row and a rejected cookies row at once.
        if let Some(issue) = cookie_issue {
            add_check(
                &mut out,
                "Platform Auth",
                &format!("{name} cookies"),
                "error",
                format!("{name} cookie session rejected — {}.", issue.reason),
                &format!(
                    "Re-import with: strivo setup cookies {} --browser <browser>",
                    name.to_lowercase()
                ),
            );
        }
    }
    out
}

/// `GET /api/v1/health/checks` — grouped, retestable health checks for the
/// System page (roadmap item 13). Each check carries {domain, name,
/// severity (ok|warn|error), message, fix}; overall status is the worst
/// severity. Authenticated (unlike the plain `/health` liveness probe).
async fn health_checks(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // Network — daemon reachable.
    let snap = state.snapshot().await;
    if snap.is_ok() {
        add_check(
            &mut checks,
            "Network",
            "Daemon IPC",
            "ok",
            "Daemon reachable.".into(),
            "",
        );
    } else {
        add_check(
            &mut checks,
            "Network",
            "Daemon IPC",
            "error",
            "Daemon unreachable over the IPC socket.".into(),
            "Start the daemon (strivo daemon) or check the socket.",
        );
    }

    // Storage — free space on the recording filesystem.
    let cfg = state.config().await;
    match &cfg {
        Ok(cfg) => {
            let (total, avail) = statvfs_bytes(&cfg.recording_dir).unwrap_or((0, 0));
            let sev = disk_severity(total, avail);
            let msg = if total == 0 {
                format!("Cannot stat recording dir {}.", cfg.recording_dir.display())
            } else {
                format!("{} free of {}.", human_bytes(avail), human_bytes(total))
            };
            let fix = if sev == "ok" {
                ""
            } else {
                "Free space or change recording_dir."
            };
            add_check(&mut checks, "Storage", "Disk space", sev, msg, fix);
        }
        Err(e) => add_check(
            &mut checks,
            "Storage",
            "Config",
            "error",
            format!("config load failed: {e}"),
            "Fix config.toml.",
        ),
    }

    // Platform Auth — tells the truth about *why* a platform isn't
    // authenticated instead of a blanket "configured but not authenticated".
    if let Ok(cfg) = &cfg {
        checks.extend(platform_auth_checks(cfg, snap.as_ref().ok()));
    }

    let worst = if checks.iter().any(|c| c["severity"] == "error") {
        "error"
    } else if checks.iter().any(|c| c["severity"] == "warn") {
        "warn"
    } else {
        "ok"
    };
    Json(json!({ "status": worst, "checks": checks })).into_response()
}

/// `GET /api/v1/storage` — disk usage of the recording directory.
/// (W5 — storage gauges.) Returns bytes_used + bytes_free for the
/// filesystem the recording dir lives on, plus per-platform totals
/// computed by walking the recording-dir tree.
async fn storage(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cfg = match state.config().await {
        Ok(c) => c,
        Err(e) => {
            return crate::problem::Problem::internal(e).into_response();
        }
    };
    let path = cfg.recording_dir.clone();
    let total_used = walk_dir_bytes(&path).unwrap_or(0);
    // Filesystem stats via statvfs — bytes_free is the more useful
    // signal than bytes_total for "do I have room for the next
    // recording".
    let (fs_total, fs_avail) = statvfs_bytes(&path).unwrap_or((0, 0));
    Json(json!({
        "recording_dir": path,
        "bytes_used_by_recordings": total_used,
        "filesystem_total_bytes": fs_total,
        "filesystem_avail_bytes": fs_avail,
    }))
    .into_response()
}

/// `pub(crate)` — `routes::backup`'s backup-set listing also walks a
/// directory tree for its byte totals.
pub(crate) fn walk_dir_bytes(p: &std::path::Path) -> std::io::Result<u64> {
    let mut sum: u64 = 0;
    if !p.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(p)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            sum = sum.saturating_add(walk_dir_bytes(&entry.path()).unwrap_or(0));
        } else if meta.is_file() {
            sum = sum.saturating_add(meta.len());
        }
    }
    Ok(sum)
}

#[cfg(unix)]
fn statvfs_bytes(p: &std::path::Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    let c_path = CString::new(p.to_str()?).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc != 0 {
        return None;
    }
    let block_size = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block_size;
    let avail = stat.f_bavail as u64 * block_size;
    Some((total, avail))
}

#[cfg(windows)]
fn statvfs_bytes(p: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = p.as_os_str().encode_wide().collect();
    wide.push(0);

    // `lpFreeBytesAvailable` (bytes free on the disk that are available
    // to the calling user) is the Windows analogue of statvfs's
    // `f_bavail`, matching the `avail` half of this function's
    // (total, avail) contract — not `lpTotalNumberOfFreeBytes`, which
    // mirrors `f_bfree` and can include quota-reserved space the caller
    // cannot actually use.
    let mut free_bytes_available_to_caller: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_bytes_available_to_caller,
            &mut total_bytes,
            &mut total_free_bytes,
        )
    };
    if ok == 0 {
        return None;
    }
    Some((total_bytes, free_bytes_available_to_caller))
}

#[cfg(not(any(unix, windows)))]
fn statvfs_bytes(_p: &std::path::Path) -> Option<(u64, u64)> {
    None
}

/// `GET /api/v1/gantt?hours=24` — recordings as Gantt segments for
/// the dashboard's 24h timeline. Returns
/// `[{ id, channel_name, start_at, end_at, state }, …]`. (W5.)
async fn gantt(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => {
            let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
            // Sort by the actual timestamp before serialising into JSON
            // values (audit P1 perf #6). The previous implementation
            // stringified every JSON value per pairwise compare —
            // O(N log N) string allocations for a hot dashboard endpoint.
            let mut sorted: Vec<_> = recordings
                .values()
                .filter(|r| r.started_at > cutoff)
                .collect();
            sorted.sort_by_key(|r| r.started_at);
            let items: Vec<_> = sorted
                .into_iter()
                .map(|r| {
                    // RecordingJob has no separate ended_at field;
                    // compute end as start + duration_secs.
                    let end = r.started_at
                        + chrono::Duration::milliseconds((r.duration_secs * 1000.0) as i64);
                    json!({
                        "id": r.id,
                        "channel_name": r.channel_name,
                        "platform": r.platform,
                        "stream_title": r.stream_title,
                        "start_at": r.started_at,
                        "end_at": end,
                        "state": format!("{:?}", r.state),
                        "bytes_written": r.bytes_written,
                    })
                })
                .collect();
            Json(json!({ "window_hours": 24, "items": items })).into_response()
        }
        _ => Json(json!({ "window_hours": 24, "items": [] })).into_response(),
    }
}

// ── W1: mutation endpoints ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct StartRecordingPayload {
    channel_id: String,
    channel_name: String,
    #[serde(default)]
    display_name: Option<String>,
    platform: PlatformKind,
    #[serde(default)]
    transcode: bool,
    #[serde(default)]
    from_start: bool,
    #[serde(default)]
    stream_title: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
}

/// `POST /api/v1/recordings` — start a new recording. Equivalent to
/// the TUI's `r` / `R` keys on the Detail pane. (W1.)
async fn start_recording(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<StartRecordingPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    // Minimal envelope; the daemon resolves cookies + transcode default
    // against its own AppConfig. Replaces the prior fat
    // `Recording(RecordingCommand::Start)` shape that hardcoded
    // `cookies_path: None` and silently broke gated YouTube starts.
    let cmd = ClientMessage::Start {
        channel_id: body.channel_id,
        channel_name: body.channel_name,
        display_name: body.display_name,
        platform: body.platform,
        stream_title: body.stream_title,
        thumbnail_url: body.thumbnail_url,
        from_start: body.from_start,
        transcode_override: Some(body.transcode),
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `GET /api/v1/recordings/{id}/thumb` — serve the recording's source thumbnail.
///
/// Resolution order:
///   1. Cached jpg at `<data_dir>/thumbs/<id>.jpg` (snapshotted at record-start
///      for new recordings).
///   2. Lazy ffmpeg extraction from the recording's `output_path` for old
///      recordings that pre-date the start-time snapshot. Result is cached
///      next to (1) so subsequent hits are fast.
///   3. 404 — the SPA falls back to a channel-initials placeholder.
async fn recording_thumb(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cache_path = strivo_core::config::AppConfig::data_dir()
        .join("thumbs")
        .join(format!("{id}.jpg"));
    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return thumb_response(bytes);
    }
    if thumbnail_failed_recently(&state, id).await {
        return crate::problem::Problem::not_found("no thumbnail: recent extraction failed")
            .into_response();
    }
    // Fall through to ffmpeg extraction. Archived jobs resolve through the
    // durable journal, so old artwork remains available after snapshot eviction.
    let source = resolve_recording(&state, id)
        .await
        .ok()
        .map(|r| (r.output_path, r.bytes_written));
    let Some((output_path, bytes_written)) = source else {
        return crate::problem::Problem::not_found("no thumbnail: recording not found")
            .into_response();
    };
    if bytes_written == 0 {
        return crate::problem::Problem::not_found("no thumbnail: zero bytes").into_response();
    }
    if !output_path.exists() {
        return crate::problem::Problem::not_found(format!(
            "no thumbnail: file missing ({})",
            output_path.display()
        ))
        .into_response();
    }

    let lock = thumbnail_lock(&state, id).await;
    let _job_lock = lock.lock().await;
    // A peer may have filled the cache while this request waited for its ID.
    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return thumb_response(bytes);
    }
    if thumbnail_failed_recently(&state, id).await {
        return crate::problem::Problem::not_found("no thumbnail: recent extraction failed")
            .into_response();
    }
    let _thumbnail_permit = match state.thumbnail_slots.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return crate::problem::Problem::unavailable("thumbnail queue closed").into_response();
        }
    };
    let _media_permit = match state.probe_slots.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return crate::problem::Problem::unavailable("media worker pool closed")
                .into_response();
        }
    };
    let response = match extract_thumb_with_ffmpeg(&output_path, &cache_path).await {
        Ok(bytes) => {
            state.thumbnail_failures.lock().await.remove(&id);
            thumb_response(bytes)
        }
        Err(e) => {
            let mut failures = state.thumbnail_failures.lock().await;
            failures.insert(id, std::time::Instant::now());
            if failures.len() > 1_024 {
                failures.clear();
                failures.insert(id, std::time::Instant::now());
            }
            crate::problem::Problem::not_found(format!("no thumbnail: ffmpeg: {e}")).into_response()
        }
    };
    response
}

fn thumb_response(bytes: Vec<u8>) -> axum::response::Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "image/jpeg"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        bytes,
    )
        .into_response()
}

/// Best-effort cleanup also runs when the request future is cancelled while a
/// child is being killed.  The temp name is unique, so removing it cannot
/// affect a concurrent extraction.
struct ThumbnailTemp(std::path::PathBuf);
impl Drop for ThumbnailTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Extract a single jpg frame at ~10s into the file and cache it. Writes to
/// `<cache_path>.tmp` then atomic-renames so concurrent requests for the same
/// id can't tear a half-written file (last writer wins, both readers see a
/// complete jpg). Returns the freshly-written bytes.
async fn extract_thumb_with_ffmpeg(
    source: &std::path::Path,
    cache_path: &std::path::Path,
) -> anyhow::Result<Vec<u8>> {
    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Temp filename ends in `.jpg` (with `.tmp` BEFORE it) — `.jpg.tmp`
    // masks the extension and ffmpeg's muxer-auto-detection fails with
    // "Unable to choose an output format". Keeping `.jpg` last and
    // `-f image2 -update 1` belt-and-suspenders works on every ffmpeg
    // we ship against.
    let tmp = cache_path.with_file_name(format!(
        ".{}.{}.tmp.jpg",
        cache_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("thumbnail"),
        Uuid::new_v4()
    ));
    let _cleanup = ThumbnailTemp(tmp.clone());

    // -ss before -i is the fast keyframe seek; -frames:v 1 caps output;
    // scale=440:-2 matches the cd-poster width and keeps an even height for
    // mjpeg. -q:v 5 is a good size/quality midpoint. -f image2 -update 1
    // pins the muxer to single-image jpeg.
    let mut cmd = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffmpeg"));
    cmd.kill_on_drop(true);
    cmd.args(["-nostdin", "-y", "-ss", "10", "-i"])
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=440:-2",
            "-q:v",
            "5",
            "-f",
            "image2",
            "-update",
            "1",
        ])
        .arg(&tmp);
    let status = match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(anyhow::anyhow!("ffmpeg thumbnail timed out")),
    };
    if !status.status.success() || tokio::fs::metadata(&tmp).await.is_err() {
        // Stream may be <10s; retry at 0s before giving up.
        let _ = tokio::fs::remove_file(&tmp).await;
        let mut cmd = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffmpeg"));
        cmd.kill_on_drop(true);
        cmd.args(["-nostdin", "-y", "-ss", "0", "-i"])
            .arg(source)
            .args([
                "-frames:v",
                "1",
                "-vf",
                "scale=440:-2",
                "-q:v",
                "5",
                "-f",
                "image2",
                "-update",
                "1",
            ])
            .arg(&tmp);
        let status = match tokio::time::timeout(Duration::from_secs(30), cmd.output()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => return Err(anyhow::anyhow!("ffmpeg thumbnail timed out")),
        };
        if !status.status.success() {
            return Err(anyhow::anyhow!(
                "ffmpeg exit {}: {}",
                status.status,
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
    }
    let bytes = tokio::fs::read(&tmp).await?;
    if bytes.is_empty() {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(anyhow::anyhow!("ffmpeg produced no frame"));
    }
    tokio::fs::rename(&tmp, cache_path)
        .await
        .map_err(|e| anyhow::anyhow!("install thumbnail cache: {e}"))?;
    Ok(bytes)
}

/// `DELETE /api/v1/recordings/<id>` — stop the recording with the
/// given job id. Equivalent to the TUI's `s` on a RecordingList row. (W1.)
async fn stop_recording(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cmd = ClientMessage::Recording(RecordingCommand::Stop { job_id: id });
    match state.ipc.send_command(cmd).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "stop sent", "job_id": id})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/recordings/stop_all` — equivalent to the TUI's quit
/// confirmation flow when active recordings are running.
async fn stop_all_recordings(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::Recording(RecordingCommand::StopAll))
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "stop_all sent"})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/recordings/<id>/file` — hard-delete a finished or
/// errored recording: trash the file (7-day retention) and drop the row
/// from `jobs.db`. Active recordings should be Stop'd first; this is the
/// management action, not the lifecycle action.
async fn delete_recording_file(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::DeleteRecording { job_id: id })
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "queued", "job_id": id})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/recordings/<id>/remux` — remux an existing recording
/// in place so the browser can play it. ffmpeg copies streams into a
/// Matroska container with the `aac_adtstoasc` bitstream filter — the
/// same combination newer recordings use by default. The original is
/// kept as `<base>.orig.<ext>` until the remux exits clean, then
/// dropped. No transcode, no quality loss.
async fn remux_recording(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let path = match state.jobs_db().await {
        Ok(db) => match db.load_recording_jobs().await {
            Ok(rows) => rows.into_iter().find(|j| j.id == id).map(|j| j.output_path),
            Err(_) => None,
        },
        Err(_) => None,
    };
    let Some(input) = path else {
        return crate::problem::Problem::not_found("recording not found").into_response();
    };
    if !input.exists() {
        return crate::problem::Problem::not_found("file missing on disk").into_response();
    }
    let orig = input.with_extension(format!(
        "orig.{}",
        input.extension().and_then(|e| e.to_str()).unwrap_or("mkv"),
    ));
    let tmp = input.with_extension("remuxed.mkv");
    let _media_permit = match state.probe_slots.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return crate::problem::Problem::unavailable("media worker pool closed").into_response()
        }
    };
    let mut remux = tokio::process::Command::new(strivo_core::tools::resolve_tool("ffmpeg"));
    remux.kill_on_drop(true);
    remux
        .args(["-y", "-hide_banner", "-loglevel", "warning"])
        .arg("-i")
        .arg(&input)
        .args(["-c", "copy", "-bsf:a", "aac_adtstoasc", "-f", "matroska"])
        .arg(&tmp);
    // A multi-hour archive can take more than two minutes merely to copy.
    // Keep a finite deadline, with a conservative 4 MiB/s allowance plus
    // startup overhead, instead of timing out healthy large-file remuxes.
    let input_bytes = tokio::fs::metadata(&input)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let remux_deadline = Duration::from_secs(120 + input_bytes.div_ceil(4 * 1024 * 1024));
    let status = tokio::time::timeout(remux_deadline, remux.status()).await;
    let ok = matches!(status, Ok(Ok(s)) if s.success());
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        return crate::problem::Problem::internal("ffmpeg remux failed").into_response();
    }
    // Atomic swap: input → orig (keep), tmp → input.
    if let Err(e) = std::fs::rename(&input, &orig) {
        let _ = std::fs::remove_file(&tmp);
        return crate::problem::Problem::internal(format!("rename original: {e}")).into_response();
    }
    if let Err(e) = std::fs::rename(&tmp, &input) {
        let _ = std::fs::rename(&orig, &input); // best-effort restore
        return crate::problem::Problem::internal(format!("install remuxed: {e}")).into_response();
    }
    Json(json!({
        "ok": true,
        "kept": orig.to_string_lossy(),
        "remuxed": input.to_string_lossy(),
    }))
    .into_response()
}

/// `POST /api/v1/recordings/clear_errored` — bulk-delete every recording
/// whose state is failed or interrupted. Same trash-then-drop semantics
/// as `delete_recording_file` per row.
async fn clear_errored_recordings(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::ClearErroredRecordings)
        .await
    {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/poll_now` — pokes the channel monitor (TUI sends
/// `ClientMessage::PollNow` via the same path).
async fn poll_now(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.ipc.send_command(ClientMessage::PollNow).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "polled"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct PollIntervalPayload {
    secs: u64,
}

/// `POST /api/v1/settings/poll_interval` — persist a new channel-poll interval
/// to config.toml AND apply it live to the running daemon (item 14b). Clamped
/// to a 15s floor (matching the monitor).
async fn set_poll_interval(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<PollIntervalPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let secs = body.secs.max(15);
    // Persist to config.toml so the change survives a restart.
    match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(mut cfg) => {
            cfg.poll_interval_secs = secs;
            let path = cfg.config_path.clone();
            if let Err(e) = cfg.save(path.as_deref()) {
                return crate::problem::Problem::internal(format!("save config: {e}"))
                    .into_response();
            }
            if let Err(e) = state.refresh_config().await {
                tracing::warn!("config cache refresh failed: {e}");
            }
        }
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    }
    // Apply live to the running monitor.
    match state
        .ipc
        .send_command(ClientMessage::SetPollInterval(secs))
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"poll_interval_secs": secs})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct SettingsUpdatePayload {
    /// Dotted path identifying which knob to mutate.
    path: String,
    value: serde_json::Value,
}

/// One web-writable configuration path.
///
/// `restart` records that the daemon owns a startup-time config snapshot, so
/// persisting the value only affects captures started after a restart. The
/// handler reports the flag from this same row on every accepted write.
struct SettingsKey {
    path: &'static str,
    restart: bool,
}

/// Every path `POST /api/v1/settings/update` accepts.
///
/// This is the allow-list, not a copy of one: `update_setting` refuses any
/// path absent from it before the `match` runs, and reads the response's
/// `restart_required` flag from the same row. `docs/SETTINGS-COVERAGE.md` is
/// generated from this const by `scripts/gen-settings-coverage.sh`.
///
/// `plugins.*.enabled` is one wildcard row so a new plugin lands by writing
/// `plugins.<name>.enabled` without an allow-list edit; the arm still validates
/// the name's shape.
const SETTINGS_KEYS: &[SettingsKey] = &[
    SettingsKey {
        path: "recording_dir",
        restart: true,
    },
    SettingsKey {
        path: "recording.format.format",
        restart: true,
    },
    SettingsKey {
        path: "recording.format.bitrate_kbps",
        restart: true,
    },
    SettingsKey {
        path: "recording.format.video_codec",
        restart: true,
    },
    SettingsKey {
        path: "recording.format.audio_codec",
        restart: true,
    },
    SettingsKey {
        path: "recording.transcode",
        restart: false,
    },
    SettingsKey {
        path: "recording.twitch_live_from_start",
        restart: false,
    },
    SettingsKey {
        path: "recording.auto_vod_backfill",
        restart: false,
    },
    SettingsKey {
        path: "recording.auto_trim_ads",
        restart: false,
    },
    SettingsKey {
        path: "recording.filename_template",
        restart: false,
    },
    SettingsKey {
        path: "recording.container",
        restart: false,
    },
    SettingsKey {
        path: "ui.reduce_motion",
        restart: false,
    },
    SettingsKey {
        path: "ui.verbose_status",
        restart: false,
    },
    SettingsKey {
        path: "notifications.desktop_enabled",
        restart: false,
    },
    SettingsKey {
        path: "notifications.on_go_live",
        restart: false,
    },
    SettingsKey {
        path: "notifications.on_recording_finished",
        restart: false,
    },
    SettingsKey {
        path: "notifications.on_recording_failed",
        restart: false,
    },
    SettingsKey {
        path: "notifications.on_vod_ready",
        restart: false,
    },
    SettingsKey {
        path: "notifications.webhook.enabled",
        restart: false,
    },
    SettingsKey {
        path: "notifications.webhook.url",
        restart: false,
    },
    SettingsKey {
        path: "monitor_limits.max_concurrent_recordings",
        restart: false,
    },
    SettingsKey {
        path: "monitor_limits.disk_budget_reserved_gb",
        restart: false,
    },
    SettingsKey {
        path: "plugins.*.enabled",
        restart: false,
    },
    #[cfg(feature = "creator")]
    SettingsKey {
        path: "archiver.enabled",
        restart: false,
    },
    #[cfg(feature = "creator")]
    SettingsKey {
        path: "archiver.concurrent_fragments",
        restart: false,
    },
    #[cfg(feature = "creator")]
    SettingsKey {
        path: "archiver.archive_dir",
        restart: false,
    },
    #[cfg(feature = "creator")]
    SettingsKey {
        path: "archiver.format",
        restart: false,
    },
];

/// The allow-list gate for `update_setting`. A `*` in a const path matches one
/// arbitrary segment, which is how the per-plugin toggle family is admitted.
fn setting(path: &str) -> Option<&'static SettingsKey> {
    SETTINGS_KEYS.iter().find(|key| {
        key.path == path
            || key.path.split_once('*').is_some_and(|(prefix, suffix)| {
                path.len() > prefix.len() + suffix.len()
                    && path.starts_with(prefix)
                    && path.ends_with(suffix)
            })
    })
}

/// `POST /api/v1/settings/update` — persist a single config knob.
///
/// Strict allow-list: anything `setting` does not recognise is rejected with
/// 400. Each accepted entry validates type, applies it to the loaded AppConfig,
/// and saves. The daemon owns a startup-time config snapshot, so recording
/// destination and format changes take effect only after it restarts; the
/// response makes that fact explicit. The poll-interval knob keeps its own
/// endpoint because it has to be re-armed in the monitor immediately.
async fn update_setting(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<SettingsUpdatePayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };

    // Allow-list. The const decides admission; the `match` below owns the type
    // and safety validation of each admitted path.
    let Some(key) = setting(body.path.as_str()) else {
        return crate::problem::Problem::bad_request(format!(
            "unknown or read-only setting: {}",
            body.path
        ))
        .into_response();
    };
    let restart_required = key.restart;
    let result: Result<(), String> = match body.path.as_str() {
        // The daemon keeps a config snapshot while it is running. Persisting
        // this never moves existing recordings or changes an active capture;
        // a restart is required before newly-started captures use it.
        "recording_dir" => take_recording_dir(&body.value).map(|dir| cfg.recording_dir = dir),
        // These are yt-dlp/ffmpeg arguments. Validate their shape enough to
        // reject accidental controls or argument injection, but do not claim
        // an allowlist of codecs: available encoders depend on the user's
        // ffmpeg build. JSON null resets an optional global override.
        "recording.format.format" => {
            take_optional_format_selector(&body.value).map(|v| cfg.recording.format.format = v)
        }
        "recording.format.bitrate_kbps" => {
            take_optional_positive_u32(&body.value).map(|v| cfg.recording.format.bitrate_kbps = v)
        }
        "recording.format.video_codec" => {
            take_optional_codec(&body.value).map(|v| cfg.recording.format.video_codec = v)
        }
        "recording.format.audio_codec" => {
            take_optional_codec(&body.value).map(|v| cfg.recording.format.audio_codec = v)
        }
        "recording.transcode" => take_bool(&body.value).map(|v| cfg.recording.transcode = v),
        "recording.twitch_live_from_start" => {
            take_bool(&body.value).map(|v| cfg.recording.twitch_live_from_start = v)
        }
        "recording.auto_vod_backfill" => {
            take_bool(&body.value).map(|v| cfg.recording.auto_vod_backfill = v)
        }
        "recording.auto_trim_ads" => {
            take_bool(&body.value).map(|v| cfg.recording.auto_trim_ads = v)
        }
        "ui.reduce_motion" => take_bool(&body.value).map(|v| cfg.ui.reduce_motion = v),
        "ui.verbose_status" => take_bool(&body.value).map(|v| cfg.ui.verbose_status = v),
        #[cfg(feature = "creator")]
        "archiver.enabled" => take_bool(&body.value).map(|v| {
            let mut a: ArchiverConfig = cfg.plugin_section("archiver");
            a.enabled = v;
            let _ = cfg.set_plugin_section("archiver", &a);
        }),
        // Notifications — every flag is a bool the daemon's notify-rust
        // integration consults before firing a banner.
        "notifications.desktop_enabled" => {
            take_bool(&body.value).map(|v| cfg.notifications.desktop_enabled = v)
        }
        "notifications.on_go_live" => {
            take_bool(&body.value).map(|v| cfg.notifications.on_go_live = v)
        }
        "notifications.on_recording_finished" => {
            take_bool(&body.value).map(|v| cfg.notifications.on_recording_finished = v)
        }
        "notifications.on_recording_failed" => {
            take_bool(&body.value).map(|v| cfg.notifications.on_recording_failed = v)
        }
        "notifications.on_vod_ready" => {
            take_bool(&body.value).map(|v| cfg.notifications.on_vod_ready = v)
        }
        "notifications.webhook.enabled" => {
            take_bool(&body.value).map(|v| cfg.notifications.webhook.enabled = v)
        }
        "notifications.webhook.url" => {
            take_webhook_url(&body.value).map(|u| cfg.notifications.webhook.url = u)
        }
        // Monitor safety knobs. 0 disables; we clamp upper bounds at sane
        // values so a fat-fingered '999999' doesn't accidentally pin the
        // daemon trying to start a thousand simultaneous captures.
        "monitor_limits.max_concurrent_recordings" => take_u32(&body.value).and_then(|v| {
            if v <= 64 {
                cfg.monitor_limits.max_concurrent_recordings = v;
                Ok(())
            } else {
                Err("max_concurrent_recordings must be 0..=64".into())
            }
        }),
        // Per-plugin toggles. Path shape: plugins.<name>.enabled — the
        // <name> is allowed to be any kebab/snake-case identifier; we
        // validate it inline rather than enumerating every plugin so
        // new plugins land without an allowlist update.
        path if path.starts_with("plugins.") && path.ends_with(".enabled") => {
            let name = &path["plugins.".len()..path.len() - ".enabled".len()];
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                Err(format!("invalid plugin name in path: {name:?}"))
            } else {
                take_bool(&body.value).map(|v| {
                    let entry = cfg
                        .plugin_toggles
                        .entry(name.to_string())
                        .or_insert_with(strivo_core::config::PluginToggle::default);
                    entry.enabled = v;
                })
            }
        }
        "monitor_limits.disk_budget_reserved_gb" => take_u32(&body.value).and_then(|v| {
            if v <= 100_000 {
                cfg.monitor_limits.disk_budget_reserved_gb = v;
                Ok(())
            } else {
                Err("disk_budget_reserved_gb must be 0..=100000".into())
            }
        }),
        #[cfg(feature = "creator")]
        "archiver.concurrent_fragments" => {
            // Clamp to 1..=16 — yt-dlp accepts more but past 16 you're
            // just thrashing the platform's rate limiter.
            take_u32(&body.value).and_then(|v| {
                if (1..=16).contains(&v) {
                    let mut a: ArchiverConfig = cfg.plugin_section("archiver");
                    a.concurrent_fragments = v;
                    let _ = cfg.set_plugin_section("archiver", &a);
                    Ok(())
                } else {
                    Err("concurrent_fragments must be 1..=16".into())
                }
            })
        }
        "recording.filename_template" => take_nonempty_str(&body.value).map(|s| {
            cfg.recording.filename_template = s;
        }),
        "recording.container" => take_str_in(&body.value, &["matroska", "mp4", "webm"])
            .map(|s| cfg.recording.format.container = Some(s)),
        #[cfg(feature = "creator")]
        "archiver.archive_dir" => take_nonempty_str(&body.value).map(|s| {
            let mut a: ArchiverConfig = cfg.plugin_section("archiver");
            a.archive_dir = std::path::PathBuf::from(s);
            let _ = cfg.set_plugin_section("archiver", &a);
        }),
        #[cfg(feature = "creator")]
        "archiver.format" => take_nonempty_str(&body.value).map(|s| {
            let mut a: ArchiverConfig = cfg.plugin_section("archiver");
            a.format = s;
            let _ = cfg.set_plugin_section("archiver", &a);
        }),
        // Unreachable: `setting` already rejected every path outside
        // SETTINGS_KEYS, so landing here means a listed path with no mutation.
        _ => Err("allow-listed setting has no mutation".into()),
    };

    if let Err(e) = result {
        return crate::problem::Problem::bad_request(e).into_response();
    }
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "path": body.path, "restart_required": restart_required})),
    )
        .into_response()
}

fn take_bool(v: &serde_json::Value) -> Result<bool, String> {
    v.as_bool().ok_or_else(|| "expected boolean".into())
}

fn take_u32(v: &serde_json::Value) -> Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| "expected non-negative integer".into())
}

fn take_nonempty_str(v: &serde_json::Value) -> Result<String, String> {
    let s = v.as_str().ok_or_else(|| "expected string".to_string())?;
    let t = s.trim();
    if t.is_empty() {
        return Err("value must not be empty".into());
    }
    Ok(t.to_string())
}

/// Accept a real server-side recording directory. We deliberately don't
/// create a missing directory: a typo must not silently scatter a new folder
/// on the daemon host. The tiny create/remove probe gives a useful answer for
/// ACL and mounted-volume failures that `metadata().permissions()` cannot
/// express. It creates no recording and leaves no durable file behind.
fn take_recording_dir(v: &serde_json::Value) -> Result<std::path::PathBuf, String> {
    let raw = take_nonempty_str(v)?;
    if raw.chars().any(char::is_control) {
        return Err("recording_dir must not contain control characters".into());
    }
    let dir = std::path::PathBuf::from(raw);
    if !dir.is_absolute() {
        return Err("recording_dir must be an absolute server path".into());
    }
    let metadata =
        std::fs::metadata(&dir).map_err(|e| format!("recording_dir is not accessible: {e}"))?;
    if !metadata.is_dir() {
        return Err("recording_dir must be an existing directory".into());
    }
    let probe = dir.join(format!(
        ".strivo-write-check-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut probe_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| format!("recording_dir is not writable: {e}"))?;
    // Keep a cleanup guard armed until removal succeeds, so an early return
    // from cleanup still gets one final best-effort removal while unwinding.
    let mut cleanup = RecordingDirWriteProbe(Some(probe));
    let write_result = std::io::Write::write_all(&mut probe_file, b"x");
    drop(probe_file);
    write_result.map_err(|e| format!("recording_dir write check failed: {e}"))?;
    std::fs::remove_file(cleanup.0.as_ref().expect("write probe path exists"))
        .map_err(|e| format!("recording_dir write check cleanup failed: {e}"))?;
    cleanup.0 = None;
    Ok(dir)
}

/// Owns only a file created by `take_recording_dir` with `create_new`, and
/// makes cleanup robust if an error interrupts the validation path.
struct RecordingDirWriteProbe(Option<std::path::PathBuf>);

impl Drop for RecordingDirWriteProbe {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn take_optional_format_selector(v: &serde_json::Value) -> Result<Option<String>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let raw = v.as_str().ok_or_else(|| "expected string".to_string())?;
    if raw.chars().any(char::is_control) {
        return Err("format selector must not contain control characters".into());
    }
    let selector = take_nonempty_str(v)?;
    if selector.len() > 512 {
        return Err("format selector must be at most 512 characters".into());
    }
    Ok(Some(selector))
}

fn take_optional_positive_u32(v: &serde_json::Value) -> Result<Option<u32>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let bitrate = take_u32(v)?;
    if !(1..=1_000_000).contains(&bitrate) {
        return Err("bitrate_kbps must be an integer from 1 to 1000000".into());
    }
    Ok(Some(bitrate))
}

fn take_optional_codec(v: &serde_json::Value) -> Result<Option<String>, String> {
    if v.is_null() {
        return Ok(None);
    }
    let raw = v.as_str().ok_or_else(|| "expected string".to_string())?;
    if raw.chars().any(char::is_control) {
        return Err("codec must not contain control characters".into());
    }
    let codec = take_nonempty_str(v)?;
    if codec.len() > 128
        || !codec
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("codec must be 1..=128 ASCII letters, digits, underscores, or hyphens".into());
    }
    Ok(Some(codec))
}

fn take_str_in(v: &serde_json::Value, allowed: &[&str]) -> Result<String, String> {
    let s = take_nonempty_str(v)?;
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(&s)) {
        Ok(s.to_lowercase())
    } else {
        Err(format!("must be one of {allowed:?}"))
    }
}

/// Validate the outbound webhook URL. An empty string clears the setting
/// (`None` — `dispatch_webhook` already no-ops without a URL); a non-empty
/// value must parse as an absolute `http`/`https` URL so a typo doesn't
/// silently sit in config until the first failed delivery.
fn take_webhook_url(v: &serde_json::Value) -> Result<Option<String>, String> {
    let s = v
        .as_str()
        .ok_or_else(|| "expected string".to_string())?
        .trim();
    if s.is_empty() {
        return Ok(None);
    }
    let parsed = reqwest::Url::parse(s).map_err(|e| format!("invalid URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("URL must use http or https".into());
    }
    Ok(Some(s.to_string()))
}

#[derive(Debug, Deserialize)]
struct PlatformConfigPayload {
    client_id: String,
    client_secret: String,
    /// Optional path on disk to a Netscape cookies file. Used by
    /// YouTube + Patreon. Empty string = unchanged.
    #[serde(default)]
    cookies_path: String,
    /// YouTube only — optional WebSub callback URL.
    #[serde(default)]
    websub_callback_url: String,
}

/// `POST /api/v1/settings/platform/<name>` — persist credentials for
/// one of the three first-party platforms. Saves to config.toml, tells the
/// running daemon to pick them up, and echoes the resulting "configured" flag
/// so the SPA can update its status badge without a follow-up GET.
///
/// The daemon builds its platform clients from the config it read at startup,
/// so without that second step a saved credential change sat inert on disk
/// until the next restart while this endpoint reported success.
async fn set_platform(
    Path(name): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<PlatformConfigPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if body.client_id.trim().is_empty() || body.client_secret.trim().is_empty() {
        return crate::problem::Problem::bad_request("client_id and client_secret are required")
            .into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };

    let cookies_opt = (!body.cookies_path.trim().is_empty())
        .then(|| std::path::PathBuf::from(body.cookies_path.clone()));

    let Some(kind) = crate::routes::blocklist::parse_platform(&name) else {
        return crate::problem::Problem::bad_request(format!("unknown platform: {name}"))
            .into_response();
    };

    match kind {
        PlatformKind::Twitch => {
            cfg.twitch = Some(strivo_core::config::TwitchConfig {
                client_id: body.client_id,
                client_secret: body.client_secret,
            });
        }
        PlatformKind::YouTube => {
            cfg.youtube = Some(strivo_core::config::YouTubeConfig {
                client_id: body.client_id,
                client_secret: body.client_secret,
                cookies_path: cookies_opt,
                websub_callback_url: (!body.websub_callback_url.trim().is_empty())
                    .then(|| body.websub_callback_url.clone()),
            });
        }
        PlatformKind::Patreon => {
            cfg.patreon = Some(strivo_core::config::PatreonConfig {
                client_id: body.client_id,
                client_secret: body.client_secret,
                poll_interval_secs: cfg
                    .patreon
                    .as_ref()
                    .map(|p| p.poll_interval_secs)
                    .unwrap_or(300),
                cookies_path: cookies_opt,
            });
        }
    }

    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    drop(_config_guard);

    // Apply them live. A daemon that is down is not an error here — the
    // credentials are saved, and it will read them at its next start — but the
    // response has to say which happened so the SPA can stop claiming the
    // change took effect when it did not.
    let applied = state
        .ipc
        .send_command(ClientMessage::ReloadPlatformCredentials { kind })
        .await
        .is_ok();
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "ok": true,
            "platform": name,
            "configured": true,
            "applied": applied,
        })),
    )
        .into_response()
}

/// Numeric severity for a `tracing` level token, higher = more severe.
fn level_rank(level: &str) -> u8 {
    match level.to_ascii_uppercase().as_str() {
        "ERROR" => 4,
        "WARN" => 3,
        "INFO" => 2,
        "DEBUG" => 1,
        "TRACE" => 0,
        _ => 2,
    }
}

/// Extract the level token from a `tracing` fmt line, e.g.
/// `2026-05-26T18:00:00Z  INFO strivo::x: msg` → `INFO`.
fn line_level(line: &str) -> Option<&str> {
    line.split_whitespace()
        .find(|t| matches!(*t, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"))
}

/// Newest `strivo.<date>.log` in the state dir (rolling appender output).
fn newest_log_file(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("strivo") && n.ends_with(".log"))
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

#[derive(Debug, Deserialize)]
struct LogQuery {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    lines: Option<usize>,
}

/// `GET /api/v1/logs?level=info&lines=200` — tail the newest rolling log
/// file, filtered to the given minimum level (roadmap item 15). Bounded by
/// `lines` (default 200, max 2000) so users never SSH for logs.
async fn logs(
    headers: HeaderMap,
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let dir = strivo_core::config::AppConfig::state_dir();
    let Some(path) = newest_log_file(&dir) else {
        return Json(json!({ "lines": [], "level": "info", "file": null })).into_response();
    };
    let body = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    let min = level_rank(q.level.as_deref().unwrap_or("trace"));
    let cap = q.lines.unwrap_or(200).clamp(1, 2000);
    let mut filtered: Vec<&str> = body
        .lines()
        .filter(|l| line_level(l).is_none_or(|lv| level_rank(lv) >= min))
        .collect();
    if filtered.len() > cap {
        filtered = filtered.split_off(filtered.len() - cap);
    }
    Json(json!({
        "lines": filtered,
        "level": q.level.unwrap_or_else(|| "trace".into()),
        "file": path.file_name().and_then(|n| n.to_str()),
    }))
    .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/health/checks", get(health_checks))
        // CE-Fusion F3: content-free product telemetry. Not gated behind
        // `creator` — serves both editions.
        //
        // **Operator query endpoint, with no SPA caller.** It is a POST for
        // a client to *report* an event and a GET for an operator to *read*
        // back what has been reported. The SPA never calls either — the only
        // in-tree caller of the GET is this crate's own test
        // (`crate::telemetry`'s route tests) — and there is deliberately no
        // nav item for it, because a product-telemetry inbox is not a user
        // surface. Query it with `curl -H "X-Api-Key: $KEY"
        // http://127.0.0.1:8181/api/v1/telemetry`. See `scripts/check-client-routes.sh`,
        // which reports uncalled server routes as information rather than as
        // contract gaps: this one is intentional.
        .route(
            "/api/v1/telemetry",
            get(crate::telemetry::telemetry_handler),
        )
        .route("/api/v1/channels", get(channels))
        .route("/api/v1/patreon", get(patreon))
        .route("/api/v1/recordings", get(recordings).post(start_recording))
        .route(
            "/api/v1/recordings/{id}",
            get(recording_one).delete(stop_recording),
        )
        .route("/api/v1/recordings/{id}/thumb", get(recording_thumb))
        .route("/api/v1/recordings/{id}/probe", get(recording_probe))
        .route("/api/v1/recordings/stop_all", post(stop_all_recordings))
        .route(
            "/api/v1/recordings/clear_errored",
            post(clear_errored_recordings),
        )
        .route(
            "/api/v1/recordings/{id}/file",
            axum::routing::delete(delete_recording_file),
        )
        .route("/api/v1/recordings/{id}/remux", post(remux_recording))
        .route("/api/v1/schedule", get(schedule).post(schedule_add))
        .route(
            "/api/v1/schedule/{index}",
            axum::routing::delete(schedule_delete),
        )
        .route("/api/v1/settings", get(settings))
        .route("/api/v1/poll_now", post(poll_now))
        .route("/api/v1/settings/poll_interval", post(set_poll_interval))
        .route("/api/v1/settings/update", post(update_setting))
        .route("/api/v1/settings/platform/{name}", post(set_platform))
        .route("/api/v1/logs", get(logs))
        // W5: stream-recorder surfaces
        .route("/api/v1/storage", get(storage))
        .route("/api/v1/gantt", get(gantt))
}

#[cfg(test)]
mod performance_tests {
    use super::{page_bounds, PageQuery};

    #[test]
    fn pagination_is_backwards_compatible_when_limit_is_omitted() {
        assert_eq!(
            page_bounds(
                1_000,
                &PageQuery {
                    cursor: None,
                    limit: None,
                }
            ),
            (0, 1_000, None)
        );
    }

    #[test]
    fn pagination_clamps_limits_and_emits_next_cursor() {
        assert_eq!(
            page_bounds(
                1_000,
                &PageQuery {
                    cursor: Some(100),
                    limit: Some(10_000),
                }
            ),
            (100, 600, Some(600))
        );
        assert_eq!(
            page_bounds(
                12,
                &PageQuery {
                    cursor: Some(10),
                    limit: Some(10),
                }
            ),
            (10, 12, None)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn thumbnail_lock_keeps_one_lock_while_a_waiter_is_queued() {
        let state = AppState::test_state("thumbnail-lock-test");
        let id = Uuid::new_v4();
        let first = thumbnail_lock(&state, id).await;
        let queued_waiter = first.clone();
        drop(first);
        let next = thumbnail_lock(&state, id).await;
        assert!(std::sync::Arc::ptr_eq(&queued_waiter, &next));
        drop(queued_waiter);
        drop(next);
        // A dropped/cancelled extraction leaves only a dead Weak entry; the
        // next lookup prunes it instead of retaining every historical ID.
        let replacement = thumbnail_lock(&state, id).await;
        assert_eq!(state.thumbnail_locks.lock().await.len(), 1);
        drop(replacement);
    }

    #[tokio::test]
    async fn queued_thumbnail_rechecks_a_recent_failure_after_its_id_lock() {
        let state = AppState::test_state("thumbnail-failure-test");
        let id = Uuid::new_v4();
        let first = thumbnail_lock(&state, id).await;
        let waiter = thumbnail_lock(&state, id).await;
        let held = first.lock().await;
        state
            .thumbnail_failures
            .lock()
            .await
            .insert(id, std::time::Instant::now());
        drop(held);
        let _waiter_guard = waiter.lock().await;
        assert!(thumbnail_failed_recently(&state, id).await);
    }

    #[test]
    fn live_and_durable_pages_share_a_progressing_numeric_cursor() {
        assert_eq!(combined_page_bounds(2, 0, 1), (0, 1, 0, 0));
        assert_eq!(combined_page_bounds(2, 1, 1), (1, 2, 0, 0));
        assert_eq!(combined_page_bounds(2, 2, 1), (2, 2, 0, 1));
        assert_eq!(combined_page_bounds(2, 3, 1), (2, 2, 1, 1));
    }

    #[test]
    fn statvfs_bytes_reports_a_sane_total_and_avail_for_an_existing_dir() {
        // Exercises the real statvfs/GetDiskFreeSpaceExW call behind the
        // `/api/v1/*` storage-gauge endpoints. A regression that silently
        // turns this into `None` (wrong path encoding, wrong out-param)
        // shows up here as `unwrap_or((0, 0))` — not as a compile error —
        // so it needs a real assertion, not just "it compiles".
        let dir = std::env::temp_dir();
        let (total, avail) = statvfs_bytes(&dir)
            .unwrap_or_else(|| panic!("expected fs stats for {}", dir.display()));
        assert!(total > 0, "total bytes should be nonzero");
        assert!(
            avail <= total,
            "avail ({avail}) should not exceed total ({total})"
        );
    }

    #[test]
    fn statvfs_bytes_returns_none_for_a_nonexistent_path() {
        let bogus = std::path::Path::new("/this/path/does/not/exist/strivo-test-9f3c2");
        assert_eq!(statvfs_bytes(bogus), None);
    }

    #[test]
    fn log_level_parse_and_rank() {
        assert_eq!(
            line_level("2026-05-26T00:00:00Z  INFO mod: hi"),
            Some("INFO")
        );
        assert_eq!(
            line_level("2026-05-26T00:00:00Z ERROR mod: boom"),
            Some("ERROR")
        );
        assert_eq!(line_level("continuation line with no level"), None);
        assert!(level_rank("ERROR") > level_rank("WARN"));
        assert!(level_rank("WARN") > level_rank("INFO"));
        assert!(level_rank("INFO") > level_rank("DEBUG"));
        assert!(level_rank("DEBUG") > level_rank("TRACE"));
    }

    #[test]
    fn disk_severity_thresholds() {
        assert_eq!(disk_severity(0, 0), "warn"); // unknown
        assert_eq!(disk_severity(100, 2), "error"); // 2% free
        assert_eq!(disk_severity(100, 10), "warn"); // 10% free
        assert_eq!(disk_severity(100, 50), "ok"); // 50% free
        assert_eq!(disk_severity(100, 15), "ok"); // exactly 15% → ok (>= boundary)
        assert_eq!(disk_severity(100, 14), "warn");
    }

    #[test]
    fn webhook_url_accepts_http_and_https() {
        assert_eq!(
            take_webhook_url(&serde_json::json!("https://example.com/hook")).unwrap(),
            Some("https://example.com/hook".to_string())
        );
        assert_eq!(
            take_webhook_url(&serde_json::json!("http://localhost:9000/x")).unwrap(),
            Some("http://localhost:9000/x".to_string())
        );
    }

    #[test]
    fn webhook_url_empty_string_clears_setting() {
        assert_eq!(take_webhook_url(&serde_json::json!("")).unwrap(), None);
        assert_eq!(take_webhook_url(&serde_json::json!("   ")).unwrap(), None);
    }

    #[test]
    fn webhook_url_rejects_malformed_and_non_http_schemes() {
        assert!(take_webhook_url(&serde_json::json!("not a url")).is_err());
        assert!(take_webhook_url(&serde_json::json!("ftp://example.com/x")).is_err());
        assert!(take_webhook_url(&serde_json::json!(42)).is_err());
    }

    /// Hard-coded literals on purpose: a test that iterated SETTINGS_KEYS would
    /// pass just as happily against an empty const.
    #[test]
    fn the_allowlist_const_is_what_the_update_gate_admits() {
        assert!(setting("recording_dir").is_some());
        assert!(setting("notifications.webhook.url").is_some());
        assert!(setting("plugins.crunchr.enabled").is_some());
        assert!(setting("plugins..enabled").is_none());
        assert!(setting("plugins.crunchr").is_none());
        assert!(setting("recording_dir.extra").is_none());
        assert!(setting("notifications.webhook.secret").is_none());
        assert!(setting("").is_none());
    }

    #[test]
    fn the_allowlist_const_carries_the_restart_flags_the_response_reports() {
        for path in [
            "recording_dir",
            "recording.format.format",
            "recording.format.bitrate_kbps",
            "recording.format.video_codec",
            "recording.format.audio_codec",
        ] {
            assert!(setting(path).is_some_and(|key| key.restart), "{path}");
        }
        assert!(setting("ui.reduce_motion").is_some_and(|key| !key.restart));
    }
}

#[cfg(test)]
mod platform_auth_checks_tests {
    use super::platform_auth_checks;
    use strivo_core::config::{AppConfig, PatreonConfig, TwitchConfig, YouTubeConfig};
    use strivo_core::ipc::ServerMessage;
    use strivo_core::platform::{AuthIssue, AuthSource, PlatformKind};

    fn cfg_with(twitch: bool, youtube: bool, patreon: bool) -> AppConfig {
        AppConfig {
            twitch: twitch.then(|| TwitchConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
            }),
            youtube: youtube.then(|| YouTubeConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                cookies_path: None,
                websub_callback_url: None,
            }),
            patreon: patreon.then(|| PatreonConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                poll_interval_secs: 300,
                cookies_path: None,
            }),
            ..AppConfig::default()
        }
    }

    fn snapshot(
        connected: (bool, bool, bool),
        pending_auth: Option<(PlatformKind, String, String)>,
        auth_issues: Vec<AuthIssue>,
    ) -> ServerMessage {
        ServerMessage::StateSnapshot {
            version: strivo_core::ipc::IPC_PROTOCOL_VERSION,
            channels: Vec::new(),
            recordings: std::collections::HashMap::new(),
            twitch_connected: connected.0,
            youtube_connected: connected.1,
            patreon_connected: connected.2,
            pending_auth,
            patreon_creators: Vec::new(),
            patreon_posts: Vec::new(),
            auth_issues,
        }
    }

    fn find<'a>(checks: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
        checks
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no check named {name} in {checks:?}"))
    }

    #[test]
    fn unconfigured_platform_produces_no_row() {
        let cfg = cfg_with(false, true, false);
        let checks = platform_auth_checks(&cfg, None);
        assert_eq!(checks.len(), 1, "only YouTube is configured: {checks:?}");
    }

    #[test]
    fn no_snapshot_reports_not_yet_authenticated() {
        let cfg = cfg_with(true, false, false);
        let checks = platform_auth_checks(&cfg, None);
        let row = find(&checks, "Twitch");
        assert_eq!(row["severity"], "warn");
        assert_eq!(
            row["message"],
            "Twitch configured but not yet authenticated."
        );
    }

    #[test]
    fn connected_platform_is_ok() {
        let cfg = cfg_with(true, false, false);
        let snap = snapshot((true, false, false), None, Vec::new());
        let checks = platform_auth_checks(&cfg, Some(&snap));
        let row = find(&checks, "Twitch");
        assert_eq!(row["severity"], "ok");
    }

    #[test]
    fn pending_device_code_is_warn_with_code_and_url() {
        let cfg = cfg_with(true, false, false);
        let snap = snapshot(
            (false, false, false),
            Some((
                PlatformKind::Twitch,
                "https://example.com/activate".into(),
                "ABCD-1234".into(),
            )),
            Vec::new(),
        );
        let checks = platform_auth_checks(&cfg, Some(&snap));
        let row = find(&checks, "Twitch");
        assert_eq!(row["severity"], "warn");
        assert!(row["message"].as_str().unwrap().contains("ABCD-1234"));
        assert!(row["message"]
            .as_str()
            .unwrap()
            .contains("https://example.com/activate"));
    }

    #[test]
    fn oauth_rejection_is_error_and_names_no_tui() {
        let cfg = cfg_with(false, true, false);
        let issue = AuthIssue {
            kind: PlatformKind::YouTube,
            source: AuthSource::OAuth,
            reason: "Token has been expired or revoked.".into(),
            since: chrono::Utc::now(),
        };
        let snap = snapshot((false, false, false), None, vec![issue]);
        let checks = platform_auth_checks(&cfg, Some(&snap));
        let row = find(&checks, "YouTube");
        assert_eq!(row["severity"], "error");
        assert!(row["message"]
            .as_str()
            .unwrap()
            .contains("credentials rejected — Token has been expired or revoked."));
        let fix = row["fix"].as_str().unwrap();
        assert!(fix.contains("Settings"));
        assert!(!fix.to_lowercase().contains("tui"));
    }

    #[test]
    fn cookie_rejection_produces_a_separate_row_alongside_oauth() {
        let cfg = cfg_with(false, true, false);
        let oauth_issue = AuthIssue {
            kind: PlatformKind::YouTube,
            source: AuthSource::OAuth,
            reason: "invalid_grant".into(),
            since: chrono::Utc::now(),
        };
        let cookie_issue = AuthIssue {
            kind: PlatformKind::YouTube,
            source: AuthSource::Cookies,
            reason: "cookies are no longer valid".into(),
            since: chrono::Utc::now(),
        };
        let snap = snapshot((false, false, false), None, vec![oauth_issue, cookie_issue]);
        let checks = platform_auth_checks(&cfg, Some(&snap));
        assert_eq!(checks.len(), 2, "{checks:?}");

        let oauth_row = find(&checks, "YouTube");
        assert_eq!(oauth_row["severity"], "error");

        let cookie_row = find(&checks, "YouTube cookies");
        assert_eq!(cookie_row["severity"], "error");
        assert!(cookie_row["message"]
            .as_str()
            .unwrap()
            .contains("cookie session rejected — cookies are no longer valid"));
        assert_eq!(
            cookie_row["fix"],
            "Re-import with: strivo setup cookies youtube --browser <browser>"
        );
    }
}
