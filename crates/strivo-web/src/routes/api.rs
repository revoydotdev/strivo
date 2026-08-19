//! /api/v1/* JSON surface (webui phase 9).
//!
//! Read-only endpoints today — every consumer (Tdarr, Bazarr, custom
//! automations) needs them, and writes are scoped to the HTML form
//! handlers until we've vetted a CSRF posture.
//!
//! Auth: `X-Api-Key: <key>` header. Constant-time compare via
//! `auth::ApiKey::matches`.
//!
//! Endpoints:
//!   GET /api/v1/health           — liveness probe, no auth required
//!   GET /api/v1/channels         — snapshot of every monitored channel
//!   GET /api/v1/recordings       — snapshot of every recording
//!   GET /api/v1/recordings/<id>  — single recording
//!   GET /api/v1/schedule         — config.schedule entries
//!   GET /api/v1/settings         — non-secret config snapshot

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use strivo_core::ipc::{BulkAction, ClientMessage, ServerMessage};
use strivo_core::platform::PlatformKind;
use strivo_core::recording::RecordingCommand;
// Bare `Problem` is used only by the creator-gated pipelines handlers; PVR
// handlers reference `crate::problem::Problem` by full path.
#[cfg(feature = "creator")]
use crate::problem::Problem;
use uuid::Uuid;

use crate::server::AppState;

/// Authorize a request via EITHER the `X-Api-Key` header (programmatic
/// clients) OR a valid `strivo_session` cookie (browser, set by /login).
/// The browser SPA only carries the cookie, so cookie support is what
/// lets it reach /channels, /recordings, … after logging in. (W3.)
fn check_key(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    // Single dual-track gate: valid X-Api-Key header OR a valid session
    // cookie (plain or `__Host-` name). See login::check_dual.
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

async fn channels(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.ipc.snapshot().await {
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
    match state.ipc.snapshot().await {
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
struct PageQuery {
    /// Zero-based opaque position. Kept numeric in the first version so old
    /// clients can adopt pagination without a second identifier format.
    #[serde(default)]
    cursor: Option<usize>,
    /// Omitted means the legacy full snapshot. Supplying a limit opts into
    /// the scalable response while preserving existing integrations.
    #[serde(default)]
    limit: Option<usize>,
}

fn page_bounds(total: usize, query: &PageQuery) -> (usize, usize, Option<usize>) {
    let start = query.cursor.unwrap_or(0).min(total);
    let end = match query.limit {
        Some(limit) => start.saturating_add(limit.clamp(1, 500)).min(total),
        None => total,
    };
    let next = (end < total).then_some(end);
    (start, end, next)
}

async fn recordings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state.ipc.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => {
            // recordings is a HashMap<Uuid, RecordingJob>; flatten to a list
            // so the response is stable and order-by-newest-first.
            let mut items: Vec<_> = recordings.into_values().collect();
            items.sort_by_key(|r| std::cmp::Reverse(r.started_at));
            let total = items.len();
            let (start, end, next_cursor) = page_bounds(total, &query);
            let augmented: Vec<_> = items[start..end].iter().map(augment_recording).collect();
            Json(json!({
                "recordings": augmented,
                "total": total,
                "next_cursor": next_cursor,
            }))
            .into_response()
        }
        Ok(_) => Json(json!({ "recordings": [] })).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
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
    match state.ipc.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => match recordings.get(&id) {
            Some(j) => Json(augment_recording(j)).into_response(),
            None => crate::problem::Problem::not_found("recording not found").into_response(),
        },
        Ok(_) => crate::problem::Problem::internal("unexpected response").into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
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
    let path = match state.ipc.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => match recordings.get(&id) {
            Some(j) => j.output_path.clone(),
            None => {
                return crate::problem::Problem::not_found("recording not found").into_response()
            }
        },
        Ok(_) => return crate::problem::Problem::internal("unexpected response").into_response(),
        Err(e) => return crate::problem::Problem::unavailable(e.to_string()).into_response(),
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
    let out = match tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(&canonical)
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return crate::problem::Problem::unavailable("ffprobe not installed").into_response();
        }
        Err(e) => {
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
    Json(json!({ "ok": true })).into_response()
}

async fn schedule(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match strivo_core::config::AppConfig::load(state.config_path()) {
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
    match strivo_core::config::AppConfig::load(state.config_path()) {
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
                // Edition flag: false in the pure-PVR build so the SPA can hide
                // creator-only nav/actions instead of linking to absent routes.
                "creator_enabled": cfg!(feature = "creator"),
            });
            // Creator Edition surfaces the Archiver config section.
            #[cfg(feature = "creator")]
            if let Some(obj) = body.as_object_mut() {
                obj.insert(
                    "archiver".into(),
                    serde_json::to_value(&cfg.archiver).unwrap_or(serde_json::Value::Null),
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
    let daemon_ok = state.ipc.snapshot().await.is_ok();

    // Jobs DB openable.
    let db_path = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let db_ok = strivo_core::recording::persist::PersistDb::open(&db_path).is_ok();

    // Free disk on the recording filesystem.
    let (disk, disk_ok) = match strivo_core::config::AppConfig::load(state.config_path()) {
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

/// `GET /api/v1/health/checks` — grouped, retestable health checks for the
/// System page (roadmap item 13). Each check carries {domain, name,
/// severity (ok|warn|error), message, fix}; overall status is the worst
/// severity. Authenticated (unlike the plain `/health` liveness probe).
async fn health_checks(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // Network — daemon reachable, and the platform-connected flags it reports.
    let snap = state.ipc.snapshot().await;
    let (tw_c, yt_c, pt_c) = match &snap {
        Ok(ServerMessage::StateSnapshot {
            twitch_connected,
            youtube_connected,
            patreon_connected,
            ..
        }) => (*twitch_connected, *youtube_connected, *patreon_connected),
        _ => (false, false, false),
    };
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
    let cfg = strivo_core::config::AppConfig::load(state.config_path());
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

    // Platform Auth — configured-but-not-authenticated is a warning.
    if let Ok(cfg) = &cfg {
        for (name, configured, connected) in [
            ("Twitch", cfg.twitch.is_some(), tw_c),
            ("YouTube", cfg.youtube.is_some(), yt_c),
            ("Patreon", cfg.patreon.is_some(), pt_c),
        ] {
            if configured && connected {
                add_check(
                    &mut checks,
                    "Platform Auth",
                    name,
                    "ok",
                    format!("{name} authenticated."),
                    "",
                );
            } else if configured {
                add_check(
                    &mut checks,
                    "Platform Auth",
                    name,
                    "warn",
                    format!("{name} configured but not authenticated."),
                    "Authenticate the platform (TUI login or re-run auth).",
                );
            }
        }
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
    let cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => {
            return crate::problem::Problem::internal(e.to_string()).into_response();
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

fn walk_dir_bytes(p: &std::path::Path) -> std::io::Result<u64> {
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
    match state.ipc.snapshot().await {
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

    // Fall through to ffmpeg extraction.
    let source = match state.ipc.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => recordings
            .get(&id)
            .map(|r| (r.output_path.clone(), r.bytes_written)),
        _ => None,
    };
    let Some((output_path, bytes_written)) = source else {
        return crate::problem::Problem::not_found("no thumbnail: not in snapshot").into_response();
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

    match extract_thumb_with_ffmpeg(&output_path, &cache_path).await {
        Ok(bytes) => thumb_response(bytes),
        Err(e) => {
            crate::problem::Problem::not_found(format!("no thumbnail: ffmpeg: {e}")).into_response()
        }
    }
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
    let tmp = cache_path.with_extension("tmp.jpg");

    // -ss before -i is the fast keyframe seek; -frames:v 1 caps output;
    // scale=440:-2 matches the cd-poster width and keeps an even height for
    // mjpeg. -q:v 5 is a good size/quality midpoint. -f image2 -update 1
    // pins the muxer to single-image jpeg.
    let mut cmd = tokio::process::Command::new("ffmpeg");
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
    let status = cmd.output().await?;
    if !status.status.success() || tokio::fs::metadata(&tmp).await.is_err() {
        // Stream may be <10s; retry at 0s before giving up.
        let _ = tokio::fs::remove_file(&tmp).await;
        let mut cmd = tokio::process::Command::new("ffmpeg");
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
        let status = cmd.output().await?;
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
    // Atomic rename; ignore the case where another concurrent extract already
    // landed (read above succeeded so we have the bytes either way).
    let _ = tokio::fs::rename(&tmp, cache_path).await;
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
    let jobs_path = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let path = match strivo_core::recording::persist::PersistDb::open(&jobs_path) {
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
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "warning"])
        .arg("-i")
        .arg(&input)
        .args(["-c", "copy", "-bsf:a", "aac_adtstoasc", "-f", "matroska"])
        .arg(&tmp)
        .status()
        .await;
    let ok = matches!(status, Ok(s) if s.success());
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

/// `POST /api/v1/settings/update` — persist a single config knob.
///
/// Strict allow-list: anything not enumerated here is rejected with 400.
/// Each entry validates type, applies it to the loaded AppConfig, and
/// saves. None of these need a live daemon-side apply — they're read
/// at the start of each new recording / archive job, or by the SPA on
/// next /settings fetch. The poll-interval knob keeps its own endpoint
/// because it has to be re-armed in the monitor immediately.
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

    // Allow-list. Booleans and small ints only — anything that could
    // break recordings on a typo (paths, templates, format strings)
    // stays out until phase 2c adds proper validation + a wizard.
    let result: Result<(), String> = match body.path.as_str() {
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
        "archiver.enabled" => take_bool(&body.value).map(|v| cfg.archiver.enabled = v),
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
                    cfg.archiver.concurrent_fragments = v;
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
            cfg.archiver.archive_dir = std::path::PathBuf::from(s);
        }),
        #[cfg(feature = "creator")]
        "archiver.format" => take_nonempty_str(&body.value).map(|s| {
            cfg.archiver.format = s;
        }),
        other => Err(format!("unknown or read-only setting: {other}")),
    };

    if let Err(e) = result {
        return crate::problem::Problem::bad_request(e).into_response();
    }
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"ok": true, "path": body.path})),
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
/// one of the three first-party platforms. Saves to config.toml and
/// echoes the resulting "configured" flag so the SPA can update its
/// status badge without a follow-up GET.
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

    match name.as_str() {
        "twitch" => {
            cfg.twitch = Some(strivo_core::config::TwitchConfig {
                client_id: body.client_id,
                client_secret: body.client_secret,
            });
        }
        "youtube" => {
            cfg.youtube = Some(strivo_core::config::YouTubeConfig {
                client_id: body.client_id,
                client_secret: body.client_secret,
                cookies_path: cookies_opt,
                websub_callback_url: (!body.websub_callback_url.trim().is_empty())
                    .then(|| body.websub_callback_url.clone()),
            });
        }
        "patreon" => {
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
        other => {
            return crate::problem::Problem::bad_request(format!("unknown platform: {other}"))
                .into_response()
        }
    }

    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({ "ok": true, "platform": name, "configured": true })),
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
        .filter(|l| line_level(l).map_or(true, |lv| level_rank(lv) >= min))
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

// ── Config/DB backup + restore (roadmap item 16) ─────────────────────
// Dep-free: a backup is a directory `data_dir/backups/<timestamp>/`
// holding copies of config.toml + jobs.db.

fn backups_dir() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir().join("backups")
}

/// Validate a user-supplied backup name: plain filename, no path parts.
/// Prevents traversal on the restore/download path.
fn safe_backup_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

async fn backup_create(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let name = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let dest = backups_dir().join(&name);
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return crate::problem::Problem::internal(format!("create backup dir: {e}"))
            .into_response();
    }
    let cfg = state
        .config_path()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(strivo_core::config::AppConfig::config_path);
    let db = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    let mut copied = Vec::new();
    if cfg.exists() {
        if let Err(e) = std::fs::copy(&cfg, dest.join("config.toml")) {
            return crate::problem::Problem::internal(format!("copy config: {e}")).into_response();
        }
        copied.push("config.toml");
    }
    if db.exists() {
        if let Err(e) = std::fs::copy(&db, dest.join("jobs.db")) {
            return crate::problem::Problem::internal(format!("copy jobs.db: {e}")).into_response();
        }
        copied.push("jobs.db");
    }
    (
        StatusCode::CREATED,
        Json(json!({ "name": name, "files": copied, "bytes": walk_dir_bytes(&dest).unwrap_or(0) })),
    )
        .into_response()
}

async fn backups_list(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let dir = backups_dir();
    let mut sets: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let p = e.path();
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            json!({
                "name": name,
                "bytes": walk_dir_bytes(&p).unwrap_or(0),
                "files": std::fs::read_dir(&p)
                    .into_iter().flatten().flatten()
                    .filter_map(|f| f.file_name().to_str().map(String::from))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    // Newest first (names are sortable timestamps).
    sets.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    Json(json!({ "backups": sets })).into_response()
}

/// `GET /api/v1/backups/<name>/download` — stream the backup as a
/// tarball so the user can pull a copy off-box. Same name/safety rules
/// as restore; no auth bypass.
async fn backup_download(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if !safe_backup_name(&name) {
        return crate::problem::Problem::bad_request("invalid backup name").into_response();
    }
    let dir = backups_dir().join(&name);
    if !dir.is_dir() {
        return crate::problem::Problem::not_found("backup not found").into_response();
    }
    // Archive work is blocking, so keep it off Tokio's request workers.
    // Conservative content-type makes browsers use "Save As".
    let archive_name = name.clone();
    let archive = tokio::task::spawn_blocking(move || {
        let child = std::process::Command::new("tar")
            .args([
                "-C",
                &dir.parent().unwrap_or(&dir).to_string_lossy(),
                "-czf",
                "-",
                &archive_name,
            ])
            .stdout(std::process::Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(child) => child,
            Err(error) => return Err(format!("tar: {error}")),
        };
        let mut out = Vec::new();
        use std::io::Read;
        if let Some(mut stdout) = child.stdout.take() {
            stdout
                .read_to_end(&mut out)
                .map_err(|error| error.to_string())?;
        }
        let status = child.wait().map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("tar exited with {status}"));
        }
        Ok::<_, String>(out)
    })
    .await;
    let out = match archive {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return crate::problem::Problem::internal(e).into_response(),
        Err(e) => {
            return crate::problem::Problem::internal(format!("archive worker crashed: {e}"))
                .into_response()
        }
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/gzip"),
            (
                axum::http::header::CONTENT_DISPOSITION,
                Box::leak(
                    format!("attachment; filename=\"strivo-backup-{name}.tar.gz\"")
                        .into_boxed_str(),
                ),
            ),
        ],
        out,
    )
        .into_response()
}

async fn backup_restore(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    if !safe_backup_name(&name) {
        return crate::problem::Problem::bad_request("invalid backup name").into_response();
    }
    let src = backups_dir().join(&name);
    if !src.is_dir() {
        return crate::problem::Problem::not_found("backup not found").into_response();
    }
    let mut restored = Vec::new();
    let cfg_src = src.join("config.toml");
    if cfg_src.exists() {
        let cfg_dest = state
            .config_path()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(strivo_core::config::AppConfig::config_path);
        if let Err(e) = std::fs::copy(&cfg_src, cfg_dest) {
            return crate::problem::Problem::internal(format!("restore config: {e}"))
                .into_response();
        }
        restored.push("config.toml");
    }
    let db_src = src.join("jobs.db");
    if db_src.exists() {
        let db_dest = strivo_core::config::AppConfig::data_dir().join("jobs.db");
        if let Err(e) = std::fs::copy(&db_src, &db_dest) {
            return crate::problem::Problem::internal(format!("restore jobs.db: {e}"))
                .into_response();
        }
        restored.push("jobs.db");
    }
    Json(json!({
        "restored": restored,
        "note": "Restart the daemon for the restored config/DB to take effect.",
    }))
    .into_response()
}

/// `GET /api/v1/history` — durable recording history from the jobs DB
/// (roadmap item 17). Unlike `/recordings` (the in-memory, bounded daemon
/// snapshot), this survives restarts and includes completed/failed jobs.
async fn history(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let db = match open_jobs_db() {
        Ok(d) => d,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    if let Some(limit) = query.limit {
        let start = query.cursor.unwrap_or(0);
        return match db
            .load_recording_jobs_page(start, limit.clamp(1, 500))
            .await
        {
            Ok((jobs, total)) => {
                let end = start.saturating_add(jobs.len());
                Json(json!({
                    "history": jobs,
                    "total": total,
                    "next_cursor": (end < total).then_some(end),
                }))
                .into_response()
            }
            Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
        };
    }
    match db.load_recording_jobs().await {
        Ok(mut jobs) => {
            jobs.sort_by_key(|j| std::cmp::Reverse(j.started_at));
            let total = jobs.len();
            let (start, end, next_cursor) = page_bounds(total, &query);
            Json(json!({
                "history": &jobs[start..end],
                "total": total,
                "next_cursor": next_cursor,
            }))
            .into_response()
        }
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

// ── Blocklist (roadmap item 17) ──────────────────────────────────────

fn parse_platform(s: &str) -> Option<PlatformKind> {
    match s.to_ascii_lowercase().as_str() {
        "twitch" => Some(PlatformKind::Twitch),
        "youtube" => Some(PlatformKind::YouTube),
        "patreon" => Some(PlatformKind::Patreon),
        _ => None,
    }
}

fn open_jobs_db() -> Result<strivo_core::recording::persist::PersistDb, String> {
    let db_path = strivo_core::config::AppConfig::data_dir().join("jobs.db");
    strivo_core::recording::persist::PersistDb::open(&db_path).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct BlockPayload {
    platform: String,
    channel_id: String,
    #[serde(default)]
    vod_id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

async fn blocklist_get(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let db = match open_jobs_db() {
        Ok(d) => d,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    match db.list_blocklist().await {
        Ok(entries) => Json(json!({ "blocklist": entries })).into_response(),
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

async fn blocklist_add(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<BlockPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let Some(platform) = parse_platform(&body.platform) else {
        return crate::problem::Problem::bad_request("unknown platform").into_response();
    };
    let db = match open_jobs_db() {
        Ok(d) => d,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    match db
        .add_blocklist(
            platform,
            &body.channel_id,
            body.vod_id.as_deref(),
            body.reason.as_deref(),
        )
        .await
    {
        Ok(()) => (StatusCode::CREATED, Json(json!({"status": "blocked"}))).into_response(),
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

async fn blocklist_remove(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<BlockPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let Some(platform) = parse_platform(&body.platform) else {
        return crate::problem::Problem::bad_request("unknown platform").into_response();
    };
    let db = match open_jobs_db() {
        Ok(d) => d,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    match db
        .remove_blocklist(platform, &body.channel_id, body.vod_id.as_deref())
        .await
    {
        Ok(()) => Json(json!({"status": "unblocked"})).into_response(),
        Err(e) => crate::problem::Problem::internal(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct AutoRecordPayload {
    enabled: bool,
    /// Per-channel container override (e.g. "mkv", "mp4"). Empty string = use global.
    #[serde(default)]
    format: Option<String>,
    /// Named capture-profile to apply (e.g. "1080p60+transcript"). Empty = global.
    #[serde(default)]
    profile: Option<String>,
}

/// Build a `RecordingFormat` with only the container field set from a
/// UI-supplied string. Returns `None` when the string is absent or empty
/// (meaning "use the global default").
fn build_recording_format(
    container: &Option<String>,
) -> Option<strivo_core::config::RecordingFormat> {
    container
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| strivo_core::config::RecordingFormat {
            container: Some(c.to_string()),
            ..Default::default()
        })
}

/// `PUT /api/v1/channels/<channel_key>/auto_record` — toggle
/// `auto_record_channels` membership for the given Platform:id key. (W1.)
async fn put_auto_record(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
    Json(body): Json<AutoRecordPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => {
            return crate::problem::Problem::internal(e.to_string()).into_response();
        }
    };
    let already_in = cfg
        .auto_record_channels
        .iter()
        .any(|c| format!("{}:{}", c.platform, c.channel_id) == channel_key);
    match (body.enabled, already_in) {
        (true, false) => {
            let Some((plat_str, ch_id)) = channel_key.split_once(':') else {
                return crate::problem::Problem::bad_request("channel_key must be Platform:id")
                    .into_response();
            };
            let platform = match plat_str.to_lowercase().as_str() {
                "twitch" => PlatformKind::Twitch,
                "youtube" => PlatformKind::YouTube,
                "patreon" => PlatformKind::Patreon,
                _ => {
                    return crate::problem::Problem::bad_request(format!(
                        "unknown platform: {plat_str}"
                    ))
                    .into_response();
                }
            };
            // Look up the channel's display name from the snapshot —
            // AutoRecordEntry requires it. Fall back to the platform
            // identifier when the channel isn't currently in the
            // cached snapshot (rare; only happens before the first
            // monitor poll completes).
            let display_name = match state.ipc.snapshot().await {
                Ok(ServerMessage::StateSnapshot { channels, .. }) => channels
                    .iter()
                    .find(|c| c.id == ch_id)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| ch_id.to_string()),
                _ => ch_id.to_string(),
            };
            cfg.auto_record_channels
                .push(strivo_core::config::AutoRecordEntry {
                    platform: format!("{platform:?}"),
                    channel_id: ch_id.to_string(),
                    channel_name: display_name,
                    format: build_recording_format(&body.format),
                    profile: body.profile.clone().filter(|s| !s.is_empty()),
                });
        }
        (true, true) => {
            // Already in list — update format/profile overrides in place.
            if let Some(entry) = cfg
                .auto_record_channels
                .iter_mut()
                .find(|c| format!("{}:{}", c.platform, c.channel_id) == channel_key)
            {
                entry.format = build_recording_format(&body.format);
                entry.profile = body.profile.clone().filter(|s| !s.is_empty());
            }
        }
        (false, true) => {
            cfg.auto_record_channels
                .retain(|c| format!("{}:{}", c.platform, c.channel_id) != channel_key);
        }
        _ => {}
    }
    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    let _ = state.ipc.send_command(ClientMessage::PollNow).await;
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "enabled": body.enabled})),
    )
        .into_response()
}

#[cfg(feature = "creator")]
#[derive(Debug, Deserialize)]
struct TandemPayload {
    enabled: bool,
}

#[cfg(feature = "creator")]
#[derive(Debug, Deserialize)]
struct TandemPlaylistsPayload {
    /// Per-key entries the user wants captured. Empty string at the end
    /// is dropped; duplicates de-duped server-side.
    playlists: Vec<String>,
}

/// `PUT /api/v1/channels/<channel_key>/archiver_tandem` — toggle
/// archiver tandem mode for the given Platform:channel_id key. When
/// enabled, the daemon auto-downloads new uploads as the monitor
/// discovers them.
#[cfg(feature = "creator")]
async fn put_archiver_tandem(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
    Json(body): Json<TandemPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    let already_in = cfg
        .archiver
        .tandem_channels
        .iter()
        .any(|c| c == &channel_key);
    match (body.enabled, already_in) {
        (true, false) => cfg.archiver.tandem_channels.push(channel_key.clone()),
        (false, true) => cfg.archiver.tandem_channels.retain(|c| c != &channel_key),
        _ => {}
    }
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    (
        StatusCode::OK,
        Json(json!({"ok": true, "enabled": body.enabled})),
    )
        .into_response()
}

/// `PUT /api/v1/channels/<channel_key>/archiver_playlists` — set the
/// playlist allow-list for a YouTube channel under archiver tandem.
/// Empty list = whole channel. Channel-level archiver tandem is
/// independent; this just narrows the scope.
#[cfg(feature = "creator")]
async fn put_archiver_playlists(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
    Json(body): Json<TandemPlaylistsPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    // Strip the existing entries for this channel, then push the new
    // set with the channel_key prefix. Format: "Platform:channel_id/<playlist>".
    cfg.archiver
        .tandem_playlists
        .retain(|p| !p.starts_with(&format!("{channel_key}/")));
    let mut seen = std::collections::HashSet::new();
    for raw in body.playlists.into_iter() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = format!("{channel_key}/{trimmed}");
        if seen.insert(entry.clone()) {
            cfg.archiver.tandem_playlists.push(entry);
        }
    }
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

// ── Capture-profile CRUD (quality tiers, task 1) ─────────────────────

/// `POST /api/v1/capture_profiles` — create a new named capture profile.
/// Body: `{ name, quality_tier?, format?, transcode?, audio_only, transcript, cutoff_episodes? }`.
async fn create_capture_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => return crate::problem::Problem::bad_request("name is required").into_response(),
    };
    let quality_tier = body
        .get("quality_tier")
        .and_then(|v| serde_json::from_value::<strivo_core::config::QualityTier>(v.clone()).ok());
    let format = body.get("format").and_then(|v| {
        serde_json::from_value::<strivo_core::config::RecordingFormat>(v.clone()).ok()
    });
    let transcode = body.get("transcode").and_then(|v| v.as_bool());
    let audio_only = body
        .get("audio_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let transcript = body
        .get("transcript")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let cutoff_episodes = body
        .get("cutoff_episodes")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    if cfg.capture_profiles.iter().any(|p| p.name == name) {
        return crate::problem::Problem::bad_request(format!("profile '{name}' already exists"))
            .into_response();
    }
    cfg.capture_profiles
        .push(strivo_core::config::CaptureProfile {
            name: name.clone(),
            quality_tier,
            format,
            transcode,
            audio_only,
            transcript,
            cutoff_episodes,
        });
    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    (
        StatusCode::CREATED,
        Json(json!({ "ok": true, "name": name })),
    )
        .into_response()
}

/// `PUT /api/v1/capture_profiles/{name}` — update an existing capture profile.
async fn update_capture_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    let Some(profile) = cfg.capture_profiles.iter_mut().find(|p| p.name == name) else {
        return crate::problem::Problem::not_found("capture profile not found").into_response();
    };
    if let Some(tier) = body.get("quality_tier") {
        profile.quality_tier = if tier.is_null() {
            None
        } else {
            serde_json::from_value(tier.clone()).ok()
        };
    }
    if let Some(fmt) = body.get("format") {
        profile.format = if fmt.is_null() {
            None
        } else {
            serde_json::from_value(fmt.clone()).ok()
        };
    }
    if let Some(v) = body.get("transcode") {
        profile.transcode = if v.is_null() { None } else { v.as_bool() };
    }
    if let Some(v) = body.get("audio_only").and_then(|v| v.as_bool()) {
        profile.audio_only = v;
    }
    if let Some(v) = body.get("transcript").and_then(|v| v.as_bool()) {
        profile.transcript = v;
    }
    if let Some(v) = body.get("cutoff_episodes") {
        profile.cutoff_episodes = if v.is_null() {
            None
        } else {
            v.as_u64().map(|n| n as u32)
        };
    }
    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    Json(json!({ "ok": true, "name": name })).into_response()
}

/// `DELETE /api/v1/capture_profiles/{name}` — delete a capture profile.
async fn delete_capture_profile(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    let before = cfg.capture_profiles.len();
    cfg.capture_profiles.retain(|p| p.name != name);
    if cfg.capture_profiles.len() == before {
        return crate::problem::Problem::not_found("capture profile not found").into_response();
    }
    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    Json(json!({ "ok": true })).into_response()
}

// ── JSON channel import/export (task 4) ──────────────────────────────

/// `GET /api/v1/channels/export` — export the auto-record channel list
/// (with per-channel overrides) as a JSON document. Safe: read-only.
async fn channels_export(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    Json(json!({
        "version": 1,
        "channels": cfg.auto_record_channels,
        "capture_profiles": cfg.capture_profiles,
    }))
    .into_response()
}

/// `POST /api/v1/channels/import` — import channel list from a JSON document.
///
/// Merge strategy: channels that don't already exist (matched by
/// `platform + channel_id`) are appended; existing channels are updated
/// with the incoming overrides. Unrelated config (recording settings,
/// platform credentials, schedule, etc.) is left untouched.
async fn channels_import(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    // Validate version field.
    let version = body.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
    if version != 1 {
        return crate::problem::Problem::bad_request("unsupported import version — expected 1")
            .into_response();
    }
    let incoming: Vec<strivo_core::config::AutoRecordEntry> = match body
        .get("channels")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
    {
        Some(v) => v,
        None => {
            return crate::problem::Problem::bad_request(
                "'channels' array is required and must be valid AutoRecordEntry objects",
            )
            .into_response()
        }
    };
    // Optional: also import capture profiles from the export document.
    let incoming_profiles: Vec<strivo_core::config::CaptureProfile> = body
        .get("capture_profiles")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };

    let mut added = 0u32;
    let mut updated = 0u32;
    for ch in incoming {
        let key = format!("{}:{}", ch.platform, ch.channel_id);
        if let Some(existing) = cfg
            .auto_record_channels
            .iter_mut()
            .find(|c| format!("{}:{}", c.platform, c.channel_id) == key)
        {
            // Update format + profile overrides only; preserve channel_name from
            // existing record so a rename in the export doesn't silently clobber
            // a corrected local name.
            existing.format = ch.format;
            existing.profile = ch.profile;
            updated += 1;
        } else {
            cfg.auto_record_channels.push(ch);
            added += 1;
        }
    }
    // Merge capture profiles: append new, leave existing untouched.
    let mut profiles_added = 0u32;
    for p in incoming_profiles {
        if !cfg
            .capture_profiles
            .iter()
            .any(|existing| existing.name == p.name)
        {
            cfg.capture_profiles.push(p);
            profiles_added += 1;
        }
    }

    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "channels_added": added,
            "channels_updated": updated,
            "profiles_added": profiles_added,
        })),
    )
        .into_response()
}

/// `GET /api/v1/monitor` — unified view for the Monitor page. Returns
/// every channel currently set to record-when-live, every channel
/// flagged for archiver tandem download, and the per-channel playlist
/// allow-lists.
async fn monitor_state(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    let auto_record: Vec<serde_json::Value> = cfg
        .auto_record_channels
        .iter()
        .map(|c| {
            // Expose the container override (from format.container) and
            // profile name so the Monitor page can render per-channel
            // format/quality selects without an extra API call.
            let container = c.format.as_ref().and_then(|f| f.container.as_deref());
            json!({
                "platform": c.platform,
                "channel_id": c.channel_id,
                "channel_name": c.channel_name,
                "key": format!("{}:{}", c.platform, c.channel_id),
                "format": container,
                "profile": c.profile,
            })
        })
        .collect();
    // Archiver tandem downloads are a Creator Edition surface; the PVR build
    // reports an empty list so the SPA's Monitor page renders identically.
    #[cfg(feature = "creator")]
    let auto_download: Vec<serde_json::Value> = {
        // Pivot tandem_playlists from "Key/Playlist" back to per-channel
        // lists so the SPA can render them grouped.
        let mut tandem: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for raw in &cfg.archiver.tandem_playlists {
            if let Some((key, pl)) = raw.split_once('/') {
                tandem
                    .entry(key.to_string())
                    .or_default()
                    .push(pl.to_string());
            }
        }
        cfg.archiver
            .tandem_channels
            .iter()
            .map(|key| {
                let (platform, channel_id) = key.split_once(':').unwrap_or((key.as_str(), ""));
                json!({
                    "platform": platform,
                    "channel_id": channel_id,
                    "key": key,
                    "playlists": tandem.get(key).cloned().unwrap_or_default(),
                })
            })
            .collect()
    };
    #[cfg(not(feature = "creator"))]
    let auto_download: Vec<serde_json::Value> = Vec::new();
    Json(json!({
        "auto_record": auto_record,
        "auto_download": auto_download,
    }))
    .into_response()
}

/// `GET /api/v1/plugins/capabilities` — the DAW-vision capability
/// matrix: which plugins (or roadmap slots) fulfil each capability
/// tag. First-party providers are hard-coded here today; once the
/// daemon exposes the registry over IPC we'll switch to that. The
/// SPA renders this as the cross-plugin pipeline graph and lets
/// users discover "who does what".
#[cfg(feature = "creator")]
async fn plugin_capabilities() -> impl IntoResponse {
    use strivo_core::plugin::capability as cap;
    // (capability, [(plugin, status)]) where status is "available"
    // if the plugin is shipped or "roadmap" if the slot is reserved
    // for an upcoming plugin (matches the DAW-vision plan).
    // Every entry here corresponds to an in-tree, shipped + tested
    // plugin crate. The `roadmap` status now applies to the one
    // marketplace entry (`yt-publish`) that still depends on external
    // work (YouTube OAuth + API creds) — surfaced from the
    // marketplace catalog below.
    let matrix = json!([
        { "capability": cap::TRANSCRIPTION,        "providers": [{"plugin": "crunchr", "status": "available"}] },
        { "capability": cap::WORD_TIMESTAMPS,      "providers": [{"plugin": "crunchr", "status": "available"}] },
        { "capability": cap::DIARISATION,          "providers": [{"plugin": "crunchr", "status": "available"}] },
        { "capability": cap::TOPIC_SEGMENTATION,   "providers": [{"plugin": "crunchr", "status": "available"}] },
        { "capability": cap::CHAPTERS,             "providers": [{"plugin": "chapters", "status": "available"}] },
        { "capability": cap::SCENE_DETECTION,      "providers": [{"plugin": "cuepoints", "status": "available"}] },
        { "capability": cap::THUMBNAIL_RANKING,    "providers": [{"plugin": "thumbnails", "status": "available"}] },
        { "capability": cap::HIGHLIGHT_DETECTION,  "providers": [{"plugin": "clipper", "status": "available"}] },
        { "capability": cap::CLIP_EXTRACTION,      "providers": [{"plugin": "clipper", "status": "available"}] },
        { "capability": cap::TRANSLATION,          "providers": [{"plugin": "captions", "status": "available"}] },
        { "capability": cap::CAPTIONS,             "providers": [
            {"plugin": "captions",       "status": "available"},
            {"plugin": "captions-ass",   "status": "available"}
        ] },
        { "capability": cap::AUDIENCE_RETENTION,   "providers": [
            {"plugin": "heatmap",      "status": "available"},
            {"plugin": "chat-density", "status": "available"}
        ] },
        { "capability": cap::FRAUD_DETECTION,      "providers": [{"plugin": "viewguard", "status": "available"}] },
        { "capability": cap::STREAM_COMPARISON,    "providers": [
            {"plugin": "insights",         "status": "available"},
            {"plugin": "viewguard-trend",  "status": "available"}
        ] },
        { "capability": cap::REPORTING,            "providers": [{"plugin": "casebook", "status": "available"}] },
        { "capability": cap::BRAND_SAFETY,         "providers": [{"plugin": "brandsafe", "status": "available"}] },
        { "capability": cap::ASSET_CATALOG,        "providers": [{"plugin": "archiver", "status": "available"}] },
        { "capability": cap::SOURCE_TRACK_SPLIT,   "providers": [
            {"plugin": "multitrack",    "status": "available"}
        ] },
        { "capability": cap::PUBLISH_QUEUE,        "providers": [
            {"plugin": "reuse",      "status": "available"},
            {"plugin": "yt-publish", "status": "roadmap"}
        ] },
        { "capability": cap::EDL_EDITOR,           "providers": [
            {"plugin": "editor",   "status": "available"},
            {"plugin": "deadair",  "status": "available"},
            {"plugin": "branding", "status": "available"},
            {"plugin": "broll",    "status": "available"}
        ] },
        // Net-new capabilities from iters 23–24. Use `x.`-prefixed tags so
        // they don't drift from the WELL_KNOWN_CAPABILITIES constant set.
        { "capability": "x.loudness",        "providers": [{"plugin": "loudness", "status": "available"}] },
        { "capability": "x.structure",       "providers": [{"plugin": "structure", "status": "available"}] },
        { "capability": "x.audio_automation", "providers": [{"plugin": "automation", "status": "available"}] },
        { "capability": "x.scenes",          "providers": [{"plugin": "scenes", "status": "available"}] },
        { "capability": "x.publish_slots",   "providers": [{"plugin": "schedule-optimizer", "status": "available"}] },
        { "capability": "x.tempo",           "providers": [{"plugin": "beat-detect", "status": "available"}] },
        { "capability": "x.voice_gate",      "providers": [{"plugin": "vad", "status": "available"}] },
        { "capability": "x.sidechain",       "providers": [{"plugin": "sidechain", "status": "available"}] },
        { "capability": "x.insert_fx",       "providers": [{"plugin": "insert-fx", "status": "available"}] },
        { "capability": "x.pitch_time",      "providers": [{"plugin": "pitch", "status": "available"}] },
        { "capability": "x.multistream",     "providers": [{"plugin": "multistream", "status": "available"}] },
        { "capability": "x.chat",            "providers": [{"plugin": "chat",        "status": "available"}] },
        { "capability": "x.pipelines_dag",   "providers": [{"plugin": "pipelines-dag", "status": "available"}] },
        { "capability": "x.marketplace",     "providers": [{"plugin": "marketplace",   "status": "available"}] },
    ]);
    Json(matrix)
}

// ── W2: plugin RPC ───────────────────────────────────────────────────

#[cfg(feature = "creator")]
#[derive(Debug, Deserialize, Default)]
struct PluginRpcPayload {
    #[serde(default)]
    selection: Vec<Uuid>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// `POST /api/v1/plugins/<plugin>/<verb>` — dispatch an actions-popup
/// verb to a plugin. Body is `{ selection: [uuid…], payload: any }`.
/// The daemon loads the plugin registry and dispatches `on_verb` over
/// IPC (see `daemon::run_with_plugins`), spawning any returned SpawnTask
/// work headless. The read side of the webui (`routes::plugins`) reads
/// each plugin's SQLite output directly to render results.
#[cfg(feature = "creator")]
async fn plugin_rpc(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((plugin, verb)): Path<(String, String)>,
    body: Option<Json<PluginRpcPayload>>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let Json(body) = body.unwrap_or(Json(PluginRpcPayload::default()));
    let cmd = ClientMessage::PluginRpc {
        plugin: plugin.clone(),
        verb: verb.clone(),
        selection: body.selection,
        payload: body.payload,
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "queued",
                "plugin": plugin,
                "verb": verb,
                "note": "dispatched in the daemon plugin host (W2-phase-3); SpawnTask work runs headless"
            })),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct BulkDownloadPayload {
    channel_name: String,
    platform: PlatformKind,
    /// "start" | "stop"
    action: String,
    #[serde(default)]
    playlist_id: Option<String>,
}

/// `POST /api/v1/channels/{id}/bulk` — start or stop a per-channel bulk
/// back-catalog download. Mirrors the TUI's `b` toggle (#71) and the
/// playlist-scoped Shift+P picker (#73). Progress streams back over
/// `/events` as `bulk-progress`. (W#74.)
async fn bulk_download(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(body): Json<BulkDownloadPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let action = match body.action.as_str() {
        "start" => BulkAction::Start,
        "stop" => BulkAction::Stop,
        other => {
            return crate::problem::Problem::bad_request(format!("unknown action {other:?}"))
                .into_response()
        }
    };
    let cmd = ClientMessage::BulkDownload {
        channel_id,
        channel_name: body.channel_name,
        platform: body.platform,
        action,
        playlist_id: body.playlist_id,
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/channels/{id}/playlists` — request the channel's
/// YouTube playlists for the scope picker. The list arrives over
/// `/events` as `playlist-list`. (W#74 / #73.)
async fn request_playlists(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::ListPlaylists { channel_id })
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(
                json!({"status": "requested", "note": "result arrives via /events playlist-list"}),
            ),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct ChannelVodsPayload {
    platform: PlatformKind,
}

#[derive(Debug, Deserialize)]
struct ResolvePayload {
    platform: PlatformKind,
    query: String,
}

/// `POST /api/v1/channels/resolve` — resolve a human identifier (Twitch
/// login, YouTube/Patreon id) for the Add-Channel wizard (task #19). The
/// result arrives over `/events` as a `ChannelResolved` frame.
async fn resolve_channel(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<ResolvePayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::ResolveChannel {
            platform: body.platform,
            query: body.query,
        })
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "requested", "note": "result arrives via /events ChannelResolved"})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/channels/{id}/vods` — request a channel's recent VODs
/// (live broadcasts + uploads) for the detail pane. The list arrives over
/// `/events` as `channel-vods`. (TUI-style redesign.)
async fn request_channel_vods(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(body): Json<ChannelVodsPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::FetchChannelVods {
            channel_id,
            platform: body.platform,
        })
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "requested", "note": "result arrives via /events channel-vods"})),
        )
            .into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct PatreonPullPayload {
    embed_url: String,
    creator_name: String,
    post_title: String,
}

#[derive(Debug, Deserialize)]
struct VodDownloadPayload {
    url: String,
    channel_name: String,
    platform: PlatformKind,
    #[serde(default)]
    post_title: Option<String>,
}

/// `POST /api/v1/vods/download` — pull a single past-broadcast/VOD on demand
/// from the channel-detail "Past Broadcasts" list. The daemon picks the
/// platform-correct cookies path and builds the output filename from config.
async fn vod_download(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<VodDownloadPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cmd = ClientMessage::DownloadVod {
        url: body.url,
        channel_name: body.channel_name,
        platform: body.platform,
        post_title: body.post_title,
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `POST /api/v1/patreon/pull` — download a single Patreon video post on
/// demand. Webui equivalent of the TUI's `p` on a creator's post (#69).
/// The daemon builds the output path from its config. (#75.)
async fn patreon_pull(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<PatreonPullPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cmd = ClientMessage::PatreonPull {
        embed_url: body.embed_url,
        creator_name: body.creator_name,
        post_title: body.post_title,
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response(),
        Err(e) => crate::problem::Problem::unavailable(e.to_string()).into_response(),
    }
}

/// `GET /api/v1/marketplace/catalog` — the curated third-party plugin
/// catalog. Each entry carries a validated [PluginManifest] + a source
/// tag (`first_party` / `verified` / `community`) + an installed flag.
/// Public surface (not Pro-gated) so free builds preview the upgrade
/// path. Installed detection plugs in when the install endpoint lands;
/// today it always reports `false`.
#[cfg(feature = "creator")]
async fn marketplace_catalog() -> impl IntoResponse {
    let mut catalog = strivo_marketplace::default_catalog();
    // Strip any catalog entries that fail validation — better to hide
    // than to ship a broken row to the SPA.
    catalog
        .entries
        .retain(|e| strivo_marketplace::validate_manifest(&e.manifest).is_ok());
    Json(json!({
        "host_version": strivo_marketplace::HOST_VERSION,
        "catalog": catalog,
    }))
}

/// `GET /api/v1/pipelines/dag` — the canonical DAW-vision pipeline
/// DAG. Public surface (not Pro-gated) so the SPA's Pipelines page
/// renders even on free builds, where it doubles as a roadmap teaser
/// alongside the upgrade card.
#[cfg(feature = "creator")]
fn chains_dir() -> std::path::PathBuf {
    strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("recipe-chains")
}
#[cfg(feature = "creator")]
fn chain_path(id: &str) -> Option<std::path::PathBuf> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(chains_dir().join(format!("{id}.json")))
}

/// `GET /api/v1/pipelines/chains` — every persisted recipe chain.
#[cfg(feature = "creator")]
async fn pipelines_chains_list() -> impl IntoResponse {
    let dir = chains_dir();
    let mut out: Vec<strivo_pipelines_dag::RecipeChain> = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for ent in rd.flatten() {
            if let Ok(s) = std::fs::read_to_string(ent.path()) {
                if let Ok(c) = serde_json::from_str(&s) {
                    out.push(c);
                }
            }
        }
    }
    Json(json!({ "chains": out })).into_response()
}

/// `POST /api/v1/pipelines/chains` — upsert a chain by `id`.
#[cfg(feature = "creator")]
async fn pipelines_chains_save(
    Json(body): Json<strivo_pipelines_dag::RecipeChain>,
) -> impl IntoResponse {
    if let Err(e) = body.validate() {
        return Problem::bad_request(e).into_response();
    }
    let Some(path) = chain_path(&body.id) else {
        return Problem::bad_request("chain id must be alphanumeric/dash/underscore")
            .into_response();
    };
    let dir = chains_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Problem::internal(format!("mkdir: {e}")).into_response();
    }
    match serde_json::to_string_pretty(&body) {
        Ok(s) => match std::fs::write(&path, s) {
            Ok(()) => Json(json!({ "ok": true, "id": body.id })).into_response(),
            Err(e) => Problem::internal(format!("write: {e}")).into_response(),
        },
        Err(e) => Problem::internal(format!("serialise: {e}")).into_response(),
    }
}

/// `DELETE /api/v1/pipelines/chains/<id>` — drop a chain.
#[cfg(feature = "creator")]
async fn pipelines_chains_delete(
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(path) = chain_path(&id) else {
        return Problem::bad_request("chain id must be alphanumeric/dash/underscore")
            .into_response();
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Json(json!({ "ok": true, "missing": true })).into_response()
        }
        Err(e) => Problem::internal(format!("remove: {e}")).into_response(),
    }
}

#[cfg(feature = "creator")]
async fn pipelines_dag() -> impl IntoResponse {
    let pipelines = strivo_pipelines_dag::default_pipelines();
    // Bundle each pipeline with its topological order so the SPA can
    // lay nodes left-to-right deterministically.
    let payload: Vec<serde_json::Value> = pipelines
        .into_iter()
        .map(|p| {
            let order = strivo_pipelines_dag::topo_order(&p).unwrap_or_default();
            json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
                "nodes": p.nodes,
                "edges": p.edges,
                "order": order,
            })
        })
        .collect();
    Json(json!({ "pipelines": payload }))
}

#[cfg(feature = "creator")]
async fn pipeline_runs(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let path = strivo_core::config::AppConfig::data_dir().join("pipelines.json");
    let runs: Vec<strivo_core::pipeline::Pipeline> = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(runs) => runs,
            Err(error) => {
                return Problem::internal(format!("decode pipeline registry: {error}"))
                    .into_response()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Problem::internal(format!("read pipeline registry: {error}")).into_response()
        }
    };
    Json(json!({ "runs": runs })).into_response()
}

#[cfg(feature = "creator")]
#[derive(Debug, Deserialize)]
struct PipelineRunPayload {
    recording_id: Uuid,
    #[serde(default = "default_pipeline_template")]
    template: String,
}

#[cfg(feature = "creator")]
fn default_pipeline_template() -> String {
    "creator_publish".to_string()
}

/// Submit the first fully executable Creator workflow. Crunchr's runner owns
/// extraction, transcription, diarisation, chunking, embedding, and analysis;
/// the daemon owns durability, retries, locking, cancellation, and observability.
#[cfg(feature = "creator")]
async fn pipeline_run(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<PipelineRunPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    match state.ipc.snapshot().await {
        Ok(ServerMessage::StateSnapshot { recordings, .. }) => {
            let Some(recording) = recordings.get(&body.recording_id) else {
                return Problem::not_found("recording not found").into_response();
            };
            if !matches!(
                recording.state,
                strivo_core::recording::job::RecordingState::Finished
            ) {
                return Problem::bad_request("pipeline requires a finished recording")
                    .into_response();
            }
            if !recording.output_path.is_file() {
                return Problem::bad_request("recording media file is missing").into_response();
            }
        }
        Ok(_) => return Problem::internal("unexpected daemon response").into_response(),
        Err(error) => return Problem::unavailable(error.to_string()).into_response(),
    }
    let pipeline = match body.template.as_str() {
        "creator_publish" => {
            strivo_core::pipeline::templates::creator_publish(body.recording_id, "manual")
        }
        "creator_intelligence" => {
            strivo_core::pipeline::templates::creator_intelligence(body.recording_id, "manual")
        }
        _ => {
            return Problem::bad_request(format!(
                "pipeline template '{}' has no executable adapter",
                body.template
            ))
            .into_response()
        }
    };
    let pipeline_id = pipeline.id;
    match state
        .ipc
        .send_command(ClientMessage::SubmitPipeline(pipeline))
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({ "status": "queued", "pipeline_id": pipeline_id })),
        )
            .into_response(),
        Err(error) => Problem::unavailable(error.to_string()).into_response(),
    }
}

#[cfg(feature = "creator")]
async fn pipeline_cancel(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::CancelPipeline { pipeline_id: id })
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => Problem::unavailable(error.to_string()).into_response(),
    }
}

#[cfg(feature = "creator")]
async fn pipeline_stage_retry(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::RetryPipelineStage { stage_id: id })
        .await
    {
        Ok(()) => StatusCode::ACCEPTED.into_response(),
        Err(error) => Problem::unavailable(error.to_string()).into_response(),
    }
}

#[cfg(feature = "creator")]
async fn pipeline_artifact_download(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((pipeline_id, stage_id, index)): Path<(Uuid, Uuid, usize)>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    let registry_path = strivo_core::config::AppConfig::data_dir().join("pipelines.json");
    let bytes = match std::fs::read(&registry_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Problem::not_found(format!("pipeline registry: {error}")).into_response()
        }
    };
    let runs: Vec<strivo_core::pipeline::Pipeline> = match serde_json::from_slice(&bytes) {
        Ok(runs) => runs,
        Err(error) => {
            return Problem::internal(format!("decode pipeline registry: {error}")).into_response()
        }
    };
    let Some(artifact) = runs
        .iter()
        .find(|run| run.id == pipeline_id)
        .and_then(|run| run.stages.iter().find(|stage| stage.id == stage_id))
        .and_then(|stage| stage.artifacts.get(index))
    else {
        return Problem::not_found("pipeline artifact not found").into_response();
    };
    let Some(raw_path) = artifact.get("path").and_then(|value| value.as_str()) else {
        return Problem::not_found("artifact has no local path").into_response();
    };
    let allowed_root = strivo_core::config::AppConfig::data_dir()
        .join("plugins")
        .join("artifacts");
    let canonical_root = match std::fs::canonicalize(&allowed_root) {
        Ok(path) => path,
        Err(error) => return Problem::not_found(format!("artifact root: {error}")).into_response(),
    };
    let canonical_path = match std::fs::canonicalize(raw_path) {
        Ok(path) => path,
        Err(error) => return Problem::not_found(format!("artifact file: {error}")).into_response(),
    };
    if !canonical_path.starts_with(&canonical_root) {
        return Problem::bad_request("artifact path escapes the Creator artifact root")
            .into_response();
    }
    let metadata = match std::fs::metadata(&canonical_path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Problem::bad_request("artifact is not a file").into_response(),
        Err(error) => {
            return Problem::not_found(format!("artifact metadata: {error}")).into_response()
        }
    };
    let file = match tokio::fs::File::open(&canonical_path).await {
        Ok(file) => file,
        Err(error) => return Problem::internal(format!("open artifact: {error}")).into_response(),
    };
    let mime = artifact
        .get("mime")
        .and_then(|value| value.as_str())
        .unwrap_or("application/octet-stream");
    let filename = canonical_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let mut response = axum::response::Response::new(body);
    *response.status_mut() = StatusCode::OK;
    if let Ok(value) = axum::http::HeaderValue::from_str(mime) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, value);
    }
    if let Ok(value) = axum::http::HeaderValue::from_str(&format!(
        "attachment; filename=\"{}\"",
        filename.replace(['"', '\\'], "_")
    )) {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_DISPOSITION, value);
    }
    response.headers_mut().insert(
        axum::http::header::CONTENT_LENGTH,
        axum::http::HeaderValue::from(metadata.len()),
    );
    response
}

pub fn router() -> Router<AppState> {
    #[allow(unused_mut)]
    let mut r = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/health/checks", get(health_checks))
        // CE-Fusion F3: content-free product telemetry. Not gated behind
        // `creator` — serves both editions.
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
        .route("/api/v1/history", get(history))
        .route("/api/v1/backup", post(backup_create))
        .route("/api/v1/backups", get(backups_list))
        .route("/api/v1/backups/{name}/restore", post(backup_restore))
        .route("/api/v1/backups/{name}/download", get(backup_download))
        .route(
            "/api/v1/blocklist",
            get(blocklist_get)
                .post(blocklist_add)
                .delete(blocklist_remove),
        )
        .route(
            "/api/v1/channels/{channel_key}/auto_record",
            put(put_auto_record),
        )
        .route("/api/v1/monitor", get(monitor_state))
        // Task 1: capture-profile CRUD
        .route("/api/v1/capture_profiles", post(create_capture_profile))
        .route(
            "/api/v1/capture_profiles/{name}",
            put(update_capture_profile).delete(delete_capture_profile),
        )
        // Task 4: channel JSON export/import
        .route("/api/v1/channels/export", get(channels_export))
        .route("/api/v1/channels/import", post(channels_import))
        // W5: stream-recorder surfaces
        .route("/api/v1/storage", get(storage))
        .route("/api/v1/gantt", get(gantt))
        // #74: bulk-download controls
        .route("/api/v1/channels/{channel_id}/bulk", post(bulk_download))
        .route(
            "/api/v1/channels/{channel_id}/playlists",
            post(request_playlists),
        )
        .route(
            "/api/v1/channels/{channel_id}/vods",
            post(request_channel_vods),
        )
        // #75: Patreon manual pull
        .route("/api/v1/patreon/pull", post(patreon_pull))
        .route("/api/v1/vods/download", post(vod_download))
        // #19: Add-Channel wizard — resolve a name/id to a channel.
        .route("/api/v1/channels/resolve", post(resolve_channel));

    // Creator Edition routes: plugin/tooling surfaces (archiver tandem,
    // pipelines, marketplace, plugin capabilities + RPC dispatch). Omitted
    // entirely from the pure-PVR build.
    #[cfg(feature = "creator")]
    {
        r = r
            .route("/api/v1/pipelines/dag", get(pipelines_dag))
            .route(
                "/api/v1/pipelines/runs",
                get(pipeline_runs).post(pipeline_run),
            )
            .route("/api/v1/pipelines/runs/{id}/cancel", post(pipeline_cancel))
            .route(
                "/api/v1/pipelines/stages/{id}/retry",
                post(pipeline_stage_retry),
            )
            .route(
                "/api/v1/pipelines/runs/{pipeline_id}/stages/{stage_id}/artifacts/{index}",
                get(pipeline_artifact_download),
            )
            .route(
                "/api/v1/pipelines/chains",
                get(pipelines_chains_list).post(pipelines_chains_save),
            )
            .route(
                "/api/v1/pipelines/chains/{id}",
                axum::routing::delete(pipelines_chains_delete),
            )
            .route("/api/v1/marketplace/catalog", get(marketplace_catalog))
            .route(
                "/api/v1/channels/{channel_key}/archiver_tandem",
                put(put_archiver_tandem),
            )
            .route(
                "/api/v1/channels/{channel_key}/archiver_playlists",
                put(put_archiver_playlists),
            )
            .route("/api/v1/plugins/capabilities", get(plugin_capabilities))
            .route("/api/v1/plugins/{plugin}/{verb}", post(plugin_rpc));
    }

    r
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
    fn backup_name_rejects_traversal() {
        assert!(safe_backup_name("2026-05-26T22-30-00Z"));
        assert!(safe_backup_name("snapshot_1.bak"));
        assert!(!safe_backup_name(""));
        assert!(!safe_backup_name(".."));
        assert!(!safe_backup_name("../etc"));
        assert!(!safe_backup_name("a/b"));
        assert!(!safe_backup_name("with space"));
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
}
