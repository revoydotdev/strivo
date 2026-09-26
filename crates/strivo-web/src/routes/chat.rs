//! Twitch IRC chat's two server-side routes.
//!
//! Chat itself is a browser-native feature — the SPA speaks the Twitch IRC
//! protocol directly over a WebSocket from the client (`connectChatRoom` in
//! `assets/spa.js`), so it needs no backend route at all to *receive*
//! messages. These two routes are the only server-side pieces it does need:
//! listing the followed channels the SPA can join, and relaying an outbound
//! PRIVMSG (browsers can't open raw TCP sockets, so sending goes through the
//! daemon's process instead).
//!
//! Neither route is Creator/Pro-gated — the descriptor's `chat` row is
//! `Entitlement::Included` — and the SPA already treats
//! `chat`/`viewer` as free routes: they're deliberately absent from
//! `CREATOR_ROUTES` in spa.js, and their `API.chatRooms`/`API.chatSend`
//! methods live in a PVR module under `assets/spa/` (see CE03), so both
//! ship in every edition. This module (unlike `routes::plugins`) is
//! compiled and mounted in every edition so the backend matches what the
//! frontend already assumes.
//!
//! S17: these two used to live in `routes::plugins`, which is merged only
//! under `#[cfg(feature = "creator")]` — a PVR build 404'd them the moment a
//! user opened Chat or the multi-viewer.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::capabilities::CapabilityKey;
use crate::problem::Problem;
use crate::server::AppState;

fn authed(headers: &HeaderMap, state: &AppState) -> Result<(), StatusCode> {
    crate::routes::login::check_dual(headers, &state.api_key, &state.session_secret)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/plugins/chat/rooms", get(chat_rooms))
        .route("/api/v1/chat/send", axum::routing::post(chat_send_message))
}

/// `GET /api/v1/plugins/chat/rooms` — list followed Twitch channels (live
/// first) the SPA can join over IRC. YouTube live chat needs an OAuth
/// flow we haven't built yet, so we surface those rooms tagged
/// `connectable=false`.
async fn chat_rooms(headers: HeaderMap, State(state): State<AppState>) -> impl IntoResponse {
    if authed(&headers, &state).is_err() {
        return Problem::unauthorized().into_response();
    }
    // The descriptor's `chat` row is `Entitlement::Included`, so this
    // always passes today; kept so flipping that row to `Pro` applies here
    // too instead of silently not.
    if !CapabilityKey::Chat.is_entitled() {
        return Problem::payment_required(
            "chat is a Strivo Pro plugin — activate or start a 3-day trial from the Plugins page.",
        )
        .into_response();
    }
    let channels = match state.snapshot().await {
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
struct ChatSendBody {
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
