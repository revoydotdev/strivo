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

impl std::fmt::Display for PlatformKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformKind::Twitch => write!(f, "Twitch"),
            PlatformKind::YouTube => write!(f, "YouTube"),
            PlatformKind::Patreon => write!(f, "Patreon"),
        }
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
