//! Multi-stream tile layout for the watch player.
//!
//! This is a core player capability — the watch view uses it to discover
//! which followed channels are currently live, get a ready-to-mount embed
//! URL per stream, and (for the multi-view mode) a tile geometry for the
//! container. It is NOT a Pro plugin, so it lives outside the creator-gated
//! `plugins` module and ships in the pure-PVR build.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::problem::Problem;
use crate::server::AppState;

fn authed(headers: &HeaderMap, state: &AppState) -> Result<(), axum::http::StatusCode> {
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

#[derive(Debug, Deserialize)]
struct MultistreamQuery {
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

/// `GET /api/v1/multistream/tiles?container_w=…&container_h=…&host=…` — fetch
/// the list of currently live followed channels from the daemon and emit the
/// tile layout for the given container, plus a ready-to-mount embed URL per
/// stream.
async fn multistream_tiles(
    headers: HeaderMap,
    State(state): State<AppState>,
    Query(q): Query<MultistreamQuery>,
) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
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
                video_id: c.live_video_id,
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
                "video_id": s.video_id,
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

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/multistream/tiles", get(multistream_tiles))
}
