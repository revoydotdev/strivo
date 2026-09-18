pub mod patreon;
pub mod twitch;
pub mod twitch_eventsub;
pub mod youtube;
pub mod youtube_websub;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Parse a `Retry-After` (RFC 7231) or `Ratelimit-Reset` header from a
/// response. Accepts seconds (e.g. `Retry-After: 30`) and HTTP-date
/// forms. Returns `None` if the header is absent or unparseable; the
/// caller decides on a default backoff.
pub fn parse_retry_after(resp: &reqwest::Response) -> Option<Duration> {
    if let Some(v) = resp
        .headers()
        .get("Retry-After")
        .and_then(|h| h.to_str().ok())
    {
        if let Ok(secs) = v.trim().parse::<u64>() {
            return Some(Duration::from_secs(secs.min(300)));
        }
        if let Ok(when) = chrono::DateTime::parse_from_rfc2822(v.trim()) {
            let delta = when.with_timezone(&Utc) - Utc::now();
            if let Ok(secs) = delta.num_seconds().try_into() {
                let secs: u64 = secs;
                return Some(Duration::from_secs(secs.min(300)));
            }
        }
    }
    if let Some(v) = resp
        .headers()
        .get("Ratelimit-Reset")
        .and_then(|h| h.to_str().ok())
    {
        if let Ok(epoch) = v.trim().parse::<i64>() {
            let now = chrono::Utc::now().timestamp();
            let delta = epoch.saturating_sub(now);
            if delta > 0 {
                return Some(Duration::from_secs((delta as u64).min(300)));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlatformKind {
    Twitch,
    YouTube,
    Patreon,
}

/// A token-endpoint error that means the token itself was rejected — a
/// revoked/expired refresh token or bad app credentials (RFC 6749 §5.2,
/// `invalid_grant`/`invalid_client`) — as opposed to a transient failure
/// (429/5xx/network). Carries the best human-readable reason we could pull
/// from the response body.
///
/// Deliberately a distinct type (rather than a plain `anyhow!()` string) so
/// callers can `downcast_ref` on it to tell "credentials are dead" apart
/// from "the network hiccuped" without parsing error text.
#[derive(Debug, Clone)]
pub struct RefreshRejected(pub String);

impl std::fmt::Display for RefreshRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RefreshRejected {}

/// Outcome of classifying a token-endpoint HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The endpoint rejected the token/credentials outright — human
    /// intervention (re-authenticate) is needed. Carries the reason.
    Rejected(String),
    /// Rate-limited, a server error, or anything else that says nothing
    /// about whether the credential itself is still good.
    Transient(String),
}

/// Classify a token-endpoint HTTP response per RFC 6749 §5.2.
///
/// - 429 is always `Transient` — it says nothing about the credential.
/// - Any other 4xx is `Rejected`: the best human-readable reason is pulled
///   from the body in priority order: OAuth's `error_description`, then
///   `error` (Google/Patreon shape), then Twitch's `message`, else a
///   generic phrase.
/// - Everything else (2xx-that-somehow-got-here, 5xx, unparsed) is
///   `Transient`.
pub fn classify_token_response(status: u16, body: &str) -> RefreshOutcome {
    if status == 429 {
        return RefreshOutcome::Transient(format!("rate limited (HTTP {status})"));
    }
    if (400..500).contains(&status) {
        #[derive(Deserialize)]
        struct ErrBody {
            error_description: Option<String>,
            error: Option<String>,
            message: Option<String>,
        }
        let reason = serde_json::from_str::<ErrBody>(body)
            .ok()
            .and_then(|b| b.error_description.or(b.error).or(b.message))
            // Providers end `error_description` with a full stop; every
            // caller composes the reason into its own sentence, so strip
            // it here rather than render "revoked.." downstream.
            .map(|s| s.trim().trim_end_matches('.').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "refresh token or app credentials were rejected".to_string());
        return RefreshOutcome::Rejected(reason);
    }
    RefreshOutcome::Transient(format!("HTTP {status}"))
}

/// Where an [`AuthIssue`] came from: an OAuth token refresh that was
/// rejected, or a cookie-jar session yt-dlp reports as no longer accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthSource {
    OAuth,
    Cookies,
}

/// A platform whose credentials need human attention, surfaced end-to-end
/// through the IPC snapshot to the web UI, `strivo status`, and
/// `/api/v1/health/checks`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuthIssue {
    pub kind: PlatformKind,
    pub source: AuthSource,
    pub reason: String,
    pub since: chrono::DateTime<chrono::Utc>,
}

impl std::fmt::Display for PlatformKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformKind::Twitch => write!(f, "Twitch"),
            PlatformKind::YouTube => write!(f, "YouTube"),
            PlatformKind::Patreon => write!(f, "Patreon"),
        }
    }
}

/// The OAuth *application* credentials for one platform — the client id and
/// secret from the platform's developer console, not the user's tokens.
///
/// Every platform holds these behind a lock rather than as plain fields so the
/// web UI can save a new pair while the daemon runs. A lock inside the
/// platform (rather than taking `&mut` through the platform's own `RwLock`) is
/// deliberate: `authenticate()` can hold that outer read lock for the length
/// of a device-code flow — up to 30 minutes — and a writer queued behind it
/// would starve exactly when the user is trying to fix their credentials.
#[derive(Clone)]
pub struct AppCredentials {
    pub client_id: String,
    pub client_secret: String,
}

impl AppCredentials {
    /// The shared form every platform stores.
    pub fn shared(
        client_id: String,
        client_secret: String,
    ) -> std::sync::Arc<tokio::sync::RwLock<Self>> {
        std::sync::Arc::new(tokio::sync::RwLock::new(Self {
            client_id,
            client_secret,
        }))
    }
}

/// Hand-written so the secret can never reach a log through a `{:?}` on this
/// struct — or on any of the platform structs that hold it.
impl std::fmt::Debug for AppCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCredentials")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelEntry {
    pub id: String,
    pub platform: PlatformKind,
    pub name: String,
    pub display_name: String,
    pub is_live: bool,
    pub stream_title: Option<String>,
    pub game_or_category: Option<String>,
    pub viewer_count: Option<u64>,
    pub started_at: Option<DateTime<Utc>>,
    pub thumbnail_url: Option<String>,
    pub auto_record: bool,
    /// Opt-in to auto-downloading this channel's new uploads (Twitch/YouTube:
    /// `config.auto_download_uploads`; Patreon: `config.auto_pull_creators`).
    /// Stamped by the monitor the same way `auto_record` is, so the rail
    /// context menu can read both from one `API.channels()` /
    /// `API.patreon()` call.
    #[serde(default)]
    pub auto_download_uploads: bool,
    /// When StriVo last observed this channel live (for the "last live: N ago"
    /// label on offline rows). Stamped by the monitor from its persisted
    /// last-live tracking; platform builders leave it None.
    #[serde(default)]
    pub last_live_at: Option<DateTime<Utc>>,
    /// The video id of the broadcast currently airing, when the platform
    /// addresses live content that way.
    ///
    /// YouTube needs this and Twitch does not: a Twitch player is pointed at
    /// a channel, but YouTube's IFrame Player API addresses a VIDEO, so the
    /// web UI cannot drive a YouTube tile through the player API from a
    /// channel id alone. Resolved during live detection, where the id is
    /// already in hand, and left None everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_video_id: Option<String>,
}

/// One past video / VOD / video-bearing post returned from a channel's back catalog.
///
/// Common shape across YouTube uploads, Twitch archive videos, and Patreon video posts —
/// just enough for the catalog runner to dedupe and hand a downloadable URL to yt-dlp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VodEntry {
    pub id: String,
    pub platform: PlatformKind,
    pub channel_id: String,
    pub title: String,
    pub published_at: Option<DateTime<Utc>>,
    pub duration: Option<Duration>,
    pub url: String,
    pub thumbnail_url: Option<String>,
    /// Whether this item was a live broadcast (past stream) or a regular
    /// upload. Lets the webui split a channel's "recent live streams" from
    /// "recent uploads" without re-deriving it client-side. Defaults to
    /// Upload for sources that don't distinguish.
    #[serde(default)]
    pub kind: VodKind,
}

/// Distinguishes a past live broadcast from a regular upload (webui channel
/// detail). YouTube sets this from `liveStreamingDetails`; Twitch archives
/// are always past broadcasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VodKind {
    #[default]
    Upload,
    LiveBroadcast,
}

/// A YouTube playlist usable as a bulk-download scope (task #73).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    pub item_count: Option<u64>,
}

#[allow(dead_code)]
#[async_trait::async_trait]
pub trait Platform: Send + Sync {
    fn kind(&self) -> PlatformKind;
    async fn authenticate(&self) -> anyhow::Result<()>;
    async fn fetch_followed_channels(&self) -> anyhow::Result<Vec<ChannelEntry>>;
    async fn check_live_status(&self, channel_ids: &[String]) -> anyhow::Result<Vec<ChannelEntry>>;
    async fn refresh_token(&self) -> anyhow::Result<()>;

    /// True iff this platform has usable credentials in memory. The
    /// monitor uses this to avoid issuing an initial poll before any
    /// platform has actually authenticated (the 10 s timeout can race
    /// authentication and produce an empty first poll).
    async fn is_authenticated(&self) -> bool;

    /// Enumerate a channel's full back catalog. Default returns NotSupported so platforms
    /// can opt in incrementally. `since` filters to entries newer than the given instant
    /// (best-effort — platforms that can't filter server-side may return more and the caller
    /// must filter). `limit` caps the count returned.
    async fn fetch_channel_vods(
        &self,
        _channel_id: &str,
        _since: Option<DateTime<Utc>>,
        _limit: Option<usize>,
    ) -> anyhow::Result<Vec<VodEntry>> {
        anyhow::bail!("catalog enumeration not supported for {}", self.kind())
    }
}

#[cfg(test)]
mod classify_tests {
    use super::*;

    #[test]
    fn google_invalid_grant_is_rejected() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        match classify_token_response(400, body) {
            RefreshOutcome::Rejected(reason) => {
                assert_eq!(reason, "Token has been expired or revoked");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn google_error_only_body_is_rejected() {
        let body = r#"{"error":"invalid_client"}"#;
        match classify_token_response(401, body) {
            RefreshOutcome::Rejected(reason) => assert_eq!(reason, "invalid_client"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn twitch_message_is_rejected() {
        let body = r#"{"status":400,"message":"Invalid refresh token"}"#;
        match classify_token_response(400, body) {
            RefreshOutcome::Rejected(reason) => assert_eq!(reason, "Invalid refresh token"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn patreon_error_shape_is_rejected() {
        let body = r#"{"error":"invalid_grant","error_description":"the code was invalid"}"#;
        match classify_token_response(400, body) {
            RefreshOutcome::Rejected(reason) => assert_eq!(reason, "the code was invalid"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn rate_limited_is_transient() {
        assert!(matches!(
            classify_token_response(429, ""),
            RefreshOutcome::Transient(_)
        ));
    }

    #[test]
    fn server_error_is_transient() {
        assert!(matches!(
            classify_token_response(503, "Service Unavailable"),
            RefreshOutcome::Transient(_)
        ));
    }

    #[test]
    fn unparseable_4xx_body_still_rejected_with_generic_reason() {
        match classify_token_response(400, "not json") {
            RefreshOutcome::Rejected(reason) => {
                assert_eq!(reason, "refresh token or app credentials were rejected");
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }
}
