//! Caller-facing intent specs.

use std::path::PathBuf;

use uuid::Uuid;

use crate::intents::cookies::CookieSource;
use crate::platform::PlatformKind;

/// Caller's intent to start a new live capture.
///
/// Translated by [`super::start_recording`] into a fully-populated
/// [`crate::recording::RecordingCommand::Start`] (cookies resolved,
/// transcode default applied).
#[derive(Debug, Clone)]
pub struct StartSpec {
    pub channel_id: String,
    pub channel_name: String,
    pub display_name: Option<String>,
    pub platform: PlatformKind,
    pub stream_title: Option<String>,
    pub thumbnail_url: Option<String>,

    /// Ask the platform driver to record from t=0 (Twitch Rewind,
    /// YouTube "live from start"). Ignored when the platform doesn't
    /// support it.
    pub from_start: bool,

    /// Pre-generated UUID. `None` lets the recording manager pick one.
    /// The schedule path uses `Some` so it can correlate the timed
    /// `Stop` with the eventual `RecordingStarted` event.
    pub job_id: Option<Uuid>,

    /// `None` = use `config.effective_transcode(platform, channel_id)`.
    pub transcode_override: Option<bool>,

    pub cookies: CookieSource,
}

/// Caller's intent to pull a single VOD / gated post.
#[derive(Debug, Clone)]
pub struct DownloadVodSpec {
    pub url: String,
    pub channel_name: String,
    pub platform: PlatformKind,
    pub post_title: Option<String>,
    pub cookies: CookieSource,
    pub output_policy: OutputPathPolicy,
    /// Platform channel id, when known. `None` for callers (Patreon pulls,
    /// legacy sites) that don't carry one; threaded through to
    /// `RecordingJob::channel_id`.
    pub channel_id: Option<String>,
}

/// Where the downloaded file ends up on disk.
#[derive(Debug, Clone)]
pub enum OutputPathPolicy {
    /// Build from `config.recording_dir` + slug. Used by Patreon
    /// monitor, daemon translators for `PatreonPull` / `DownloadVod`,
    /// the catalog-pull bulk download path. The default.
    Fresh,

    /// Co-locate with an existing live capture under a `_vod` suffix
    /// (`<base>.<ext>` → `<base>_vod.<ext>`). Used by `vod_backfill`
    /// after a live record finishes.
    AdjacentTo(PathBuf),
}
