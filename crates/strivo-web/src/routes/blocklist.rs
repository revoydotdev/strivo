//! Blocklist (roadmap item 17), per-channel auto-record + alerts, and
//! (Creator Edition) archiver tandem/playlist toggles. Split out of
//! `routes::api`.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use strivo_core::ipc::ClientMessage;
use strivo_core::platform::PlatformKind;
#[cfg(feature = "creator")]
use strivo_plugins::archiver::types::ArchiverConfig;

use crate::routes::settings::check_key;
use crate::server::AppState;

pub(crate) fn parse_platform(s: &str) -> Option<PlatformKind> {
    match s.to_ascii_lowercase().as_str() {
        "twitch" => Some(PlatformKind::Twitch),
        "youtube" => Some(PlatformKind::YouTube),
        "patreon" => Some(PlatformKind::Patreon),
        _ => None,
    }
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
    let db = match state.jobs_db().await {
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
    let db = match state.jobs_db().await {
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
    let db = match state.jobs_db().await {
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
            let display_name = match state.snapshot().await {
                Ok(strivo_core::ipc::ServerMessage::StateSnapshot { channels, .. }) => channels
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
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    let _ = state.ipc.send_command(ClientMessage::PollNow).await;
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "enabled": body.enabled})),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct AutoDownloadUploadsPayload {
    enabled: bool,
}

/// `PUT /api/v1/channels/<channel_key>/auto_download_uploads` — toggle
/// auto-downloading a channel's new uploads. Mirrors `put_auto_record`'s
/// shape, but Patreon is special-cased: it has no entry of its own here
/// (core has no second source of truth for the Patreon opt-in) — instead
/// this writes/reads `config.auto_pull_creators`, keyed by campaign_id
/// (which is exactly what the rail's Patreon `channel_id` already is —
/// see `PatreonCreator::to_channel_entry`). Twitch/YouTube write
/// `config.auto_download_uploads`.
async fn put_auto_download_uploads(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
    Json(body): Json<AutoDownloadUploadsPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let Some((plat_str, ch_id)) = channel_key.split_once(':') else {
        return crate::problem::Problem::bad_request("channel_key must be Platform:id")
            .into_response();
    };
    let Some(platform) = parse_platform(plat_str) else {
        return crate::problem::Problem::bad_request(format!("unknown platform: {plat_str}"))
            .into_response();
    };

    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };

    if platform == PlatformKind::Patreon {
        let already_in = cfg
            .auto_pull_creators
            .iter()
            .any(|e| e.campaign_id == ch_id);
        match (body.enabled, already_in) {
            (true, false) => {
                let creator_name = match state.snapshot().await {
                    Ok(strivo_core::ipc::ServerMessage::StateSnapshot {
                        patreon_creators, ..
                    }) => patreon_creators
                        .iter()
                        .find(|c| c.id == ch_id)
                        .map(|c| c.display_name.clone())
                        .unwrap_or_else(|| ch_id.to_string()),
                    _ => ch_id.to_string(),
                };
                cfg.auto_pull_creators
                    .push(strivo_core::config::AutoPullEntry {
                        campaign_id: ch_id.to_string(),
                        creator_name,
                    });
            }
            (false, true) => {
                cfg.auto_pull_creators.retain(|e| e.campaign_id != ch_id);
            }
            _ => {}
        }
    } else {
        let already_in = cfg
            .auto_download_uploads
            .iter()
            .any(|c| format!("{}:{}", c.platform, c.channel_id) == channel_key);
        match (body.enabled, already_in) {
            (true, false) => {
                let display_name = match state.snapshot().await {
                    Ok(strivo_core::ipc::ServerMessage::StateSnapshot { channels, .. }) => channels
                        .iter()
                        .find(|c| c.id == ch_id)
                        .map(|c| c.display_name.clone())
                        .unwrap_or_else(|| ch_id.to_string()),
                    _ => ch_id.to_string(),
                };
                cfg.auto_download_uploads
                    .push(strivo_core::config::AutoDownloadUploadsEntry {
                        platform: format!("{platform:?}"),
                        channel_id: ch_id.to_string(),
                        channel_name: display_name,
                    });
            }
            (false, true) => {
                cfg.auto_download_uploads
                    .retain(|c| format!("{}:{}", c.platform, c.channel_id) != channel_key);
            }
            _ => {}
        }
    }

    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    let _ = state.ipc.send_command(ClientMessage::PollNow).await;
    (
        StatusCode::OK,
        Json(json!({"status": "ok", "enabled": body.enabled})),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ChannelAlertsPayload {
    #[serde(default)]
    on_live: Option<bool>,
    #[serde(default)]
    on_upload: Option<bool>,
}

/// `GET /api/v1/channels/<channel_key>/alerts` — current per-channel alert
/// override, if any. Both fields are `null` when the channel follows the
/// global `[notifications]` defaults unchanged.
async fn get_channel_alerts(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let cfg = match state.config().await {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e).into_response(),
    };
    let entry = cfg
        .channel_alerts
        .iter()
        .find(|c| c.channel_key == channel_key);
    Json(json!({
        "channel_key": channel_key,
        "on_live": entry.and_then(|e| e.on_live),
        "on_upload": entry.and_then(|e| e.on_upload),
    }))
    .into_response()
}

/// `PUT /api/v1/channels/<channel_key>/alerts` — set (or clear) a
/// per-channel override of the global live/upload alert switches. `None`
/// on both `on_live` and `on_upload` removes any existing override for the
/// channel, reverting it to the global default; otherwise the entry is
/// upserted with whatever fields were supplied (the other field stays
/// `None`, i.e. still follows global default).
async fn put_channel_alerts(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path(channel_key): Path<String>,
    Json(body): Json<ChannelAlertsPayload>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    let _config_guard = state.config_write_lock.lock().await;
    let mut cfg = match strivo_core::config::AppConfig::load(state.config_path()) {
        Ok(c) => c,
        Err(e) => return crate::problem::Problem::internal(e.to_string()).into_response(),
    };
    if body.on_live.is_none() && body.on_upload.is_none() {
        cfg.channel_alerts.retain(|c| c.channel_key != channel_key);
    } else if let Some(entry) = cfg
        .channel_alerts
        .iter_mut()
        .find(|c| c.channel_key == channel_key)
    {
        entry.on_live = body.on_live;
        entry.on_upload = body.on_upload;
    } else {
        cfg.channel_alerts
            .push(strivo_core::config::ChannelAlertEntry {
                channel_key: channel_key.clone(),
                on_live: body.on_live,
                on_upload: body.on_upload,
            });
    }
    if let Err(e) = cfg.save(state.config_path()) {
        return crate::problem::Problem::internal(e.to_string()).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "channel_key": channel_key,
            "on_live": body.on_live,
            "on_upload": body.on_upload,
        })),
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
    let mut archiver: ArchiverConfig = cfg.plugin_section("archiver");
    let already_in = archiver.tandem_channels.iter().any(|c| c == &channel_key);
    match (body.enabled, already_in) {
        (true, false) => archiver.tandem_channels.push(channel_key.clone()),
        (false, true) => archiver.tandem_channels.retain(|c| c != &channel_key),
        _ => {}
    }
    let _ = cfg.set_plugin_section("archiver", &archiver);
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
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
    let mut archiver: ArchiverConfig = cfg.plugin_section("archiver");
    archiver
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
            archiver.tandem_playlists.push(entry);
        }
    }
    let _ = cfg.set_plugin_section("archiver", &archiver);
    let path = cfg.config_path.clone();
    if let Err(e) = cfg.save(path.as_deref()) {
        return crate::problem::Problem::internal(format!("save config: {e}")).into_response();
    }
    if let Err(e) = state.refresh_config().await {
        tracing::warn!("config cache refresh failed: {e}");
    }
    Json(json!({ "ok": true })).into_response()
}

pub fn router() -> Router<AppState> {
    #[allow(unused_mut)]
    let mut r = Router::new()
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
        .route(
            "/api/v1/channels/{channel_key}/auto_download_uploads",
            put(put_auto_download_uploads),
        )
        .route(
            "/api/v1/channels/{channel_key}/alerts",
            get(get_channel_alerts).put(put_channel_alerts),
        );

    #[cfg(feature = "creator")]
    {
        r = r
            .route(
                "/api/v1/channels/{channel_key}/archiver_tandem",
                put(put_archiver_tandem),
            )
            .route(
                "/api/v1/channels/{channel_key}/archiver_playlists",
                put(put_archiver_playlists),
            );
    }

    r
}
