//! /api/v1/* JSON surface (webui phase 9).
//!
//! This module now holds the channel-action/VOD-download endpoints
//! (bulk-download, playlists, VOD resolve/download, Patreon pull) plus,
//! under the `creator` feature, the plugin-RPC/pipeline/marketplace
//! surface (W2). The rest of the original `/api/v1/*` handlers were split
//! into sibling `routes::{settings,backup,blocklist,capture_profiles,
//! import_export}` modules; see those modules' doc comments and
//! `routes::mod` for the full picture.
//!
//! Auth: `X-Api-Key: <key>` header. Constant-time compare via
//! `auth::ApiKey::matches`. (See `routes::settings::check_key`, which
//! every `/api/v1/*` handler module — including this one — calls.)

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
#[cfg(feature = "creator")]
use strivo_core::ipc::ServerMessage;
use strivo_core::ipc::{BulkAction, ClientMessage};
use strivo_core::platform::PlatformKind;
// Bare `Problem` is used only by the creator-gated pipelines handlers; PVR
// handlers reference `crate::problem::Problem` by full path.
#[cfg(feature = "creator")]
use crate::problem::Problem;
#[cfg(feature = "creator")]
use uuid::Uuid;
#[cfg(not(feature = "creator"))]
use uuid::Uuid;

use crate::routes::settings::check_key;
use crate::server::AppState;

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
    #[serde(default)]
    operation_id: Option<Uuid>,
    channel_name: String,
    platform: PlatformKind,
    /// "start" | "stop"
    action: String,
    #[serde(default)]
    playlist_id: Option<String>,
    #[serde(default)]
    vod_ids: Option<Vec<String>>,
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
    let operation_id = match action {
        BulkAction::Start => Some(body.operation_id.unwrap_or_else(Uuid::new_v4)),
        // Preserve the channel-scoped cancellation semantics for older
        // callers that do not yet know an operation id.
        BulkAction::Stop => body.operation_id,
    };
    let cmd = ClientMessage::BulkDownload {
        operation_id,
        channel_id,
        channel_name: body.channel_name,
        platform: body.platform,
        action,
        playlist_id: body.playlist_id,
        vod_ids: body.vod_ids,
    };
    match state.ipc.send_command(cmd).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({"status": "queued", "operation_id": operation_id})),
        )
            .into_response(),
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

/// `GET /api/v1/channels/{id}/playlists/{playlist_id}` — fetch playlist
/// items for a read-only thumbnail viewer. This request has no download side
/// effect; clients must explicitly submit a bulk operation for downloading.
async fn request_playlist_items(
    headers: HeaderMap,
    State(state): State<AppState>,
    Path((channel_id, playlist_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return crate::problem::Problem::unauthorized().into_response();
    }
    match state
        .ipc
        .send_command(ClientMessage::ListPlaylistItems {
            channel_id,
            playlist_id,
        })
        .await
    {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(json!({
                "status": "requested",
                "note": "result arrives via /events playlist-items"
            })),
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

/// A YouTube URL addressing a channel tab (its live-now alias, uploads
/// tab, etc.) rather than one specific video. `DownloadVod` pulls a single
/// known VOD; a channel-alias URL either 404s once the broadcast it
/// pointed at has ended, or silently downloads whatever happens to be
/// live at request time — neither is what a caller asking for one VOD
/// wants. Observed in the wild as a malformed `.../@handle/live` request
/// body producing a confusing "yt-dlp ... 404" instead of a clear reason.
fn is_youtube_channel_alias_url(url: &str) -> bool {
    if url.contains("watch?v=") || url.contains("youtu.be/") {
        return false;
    }
    let Some(after_host) = url.split("youtube.com").nth(1) else {
        return false;
    };
    let path = after_host.split(['?', '#']).next().unwrap_or(after_host);
    let path = path.trim_end_matches('/');
    matches!(
        path.rsplit('/').next(),
        Some("live" | "streams" | "videos" | "featured" | "shorts")
    )
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
    if body.platform == PlatformKind::YouTube && is_youtube_channel_alias_url(&body.url) {
        return crate::problem::Problem::bad_request(format!(
            "'{}' addresses a YouTube channel tab, not a specific video. \
             Pass the video's own watch URL instead.",
            body.url
        ))
        .into_response();
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
async fn pipelines_chains_list(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
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
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(body): Json<strivo_pipelines_dag::RecipeChain>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
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
    headers: HeaderMap,
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
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
async fn pipelines_dag(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if check_key(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
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
    Json(json!({ "pipelines": payload })).into_response()
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
    match state.snapshot().await {
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
            strivo_plugins::pipeline_templates::creator_publish(body.recording_id, "manual")
        }
        "creator_intelligence" => {
            strivo_plugins::pipeline_templates::creator_intelligence(body.recording_id, "manual")
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
        // #74: bulk-download controls
        .route("/api/v1/channels/{channel_id}/bulk", post(bulk_download))
        .route(
            "/api/v1/channels/{channel_id}/playlists",
            post(request_playlists),
        )
        .route(
            "/api/v1/channels/{channel_id}/playlists/{playlist_id}",
            get(request_playlist_items),
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

    // Creator Edition routes: plugin/tooling surfaces (pipelines,
    // marketplace, plugin RPC dispatch). Omitted entirely from the
    // pure-PVR build.
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
            .route("/api/v1/plugins/{plugin}/{verb}", post(plugin_rpc));
    }

    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_live_and_other_channel_tab_aliases() {
        assert!(is_youtube_channel_alias_url(
            "https://www.youtube.com/@The Yard/live"
        ));
        assert!(is_youtube_channel_alias_url(
            "https://www.youtube.com/channel/UCGbg3DjQdcqWwqOLHpYHXIg/live"
        ));
        assert!(is_youtube_channel_alias_url(
            "https://www.youtube.com/@somechannel/streams"
        ));
        assert!(is_youtube_channel_alias_url(
            "https://www.youtube.com/@somechannel/videos"
        ));
    }

    #[test]
    fn accepts_real_video_urls() {
        assert!(!is_youtube_channel_alias_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(!is_youtube_channel_alias_url(
            "https://youtu.be/dQw4w9WgXcQ"
        ));
        // "live" as a query param on a real watch URL must not trip the
        // channel-alias check.
        assert!(!is_youtube_channel_alias_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=live"
        ));
    }

    #[test]
    fn vod_download_contract_accepts_uploaded_video_envelope() {
        let payload: VodDownloadPayload = serde_json::from_value(serde_json::json!({
            "url": "https://www.youtube.com/watch?v=abc123",
            "channel_name": "Example creator",
            "platform": "YouTube",
            "post_title": "Uploaded video"
        }))
        .expect("uploaded-video request must match the documented contract");
        assert_eq!(payload.channel_name, "Example creator");
        assert_eq!(payload.platform, PlatformKind::YouTube);
        assert_eq!(payload.post_title.as_deref(), Some("Uploaded video"));
    }
}
