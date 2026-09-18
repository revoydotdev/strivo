pub mod credentials;
pub mod import;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_recording_dir")]
    pub recording_dir: PathBuf,

    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,

    pub twitch: Option<TwitchConfig>,
    pub youtube: Option<YouTubeConfig>,
    pub patreon: Option<PatreonConfig>,

    #[serde(default)]
    pub recording: RecordingConfig,

    #[serde(default)]
    pub theme: ThemeRef,

    /// UI preferences — animation, a11y, verbosity. Everything optional.
    #[serde(default)]
    pub ui: UiConfig,

    #[serde(default)]
    pub auto_record_channels: Vec<AutoRecordEntry>,

    /// Twitch/YouTube channels whose new uploads (not the channel's own
    /// live-capture VODs — see `AutoDownloadUploadsEntry`) the monitor
    /// should fetch and auto-download. Patreon has its own opt-in list,
    /// `auto_pull_creators`, and is deliberately NOT stored here: adding a
    /// second Patreon list would create two sources of truth for the same
    /// per-creator switch. Empty by default — opt-in per channel from the
    /// rail context menu, mirroring `auto_record_channels`.
    #[serde(default)]
    pub auto_download_uploads: Vec<AutoDownloadUploadsEntry>,

    /// Per-channel overrides of the global notification/webhook switches
    /// (`[notifications]` on_go_live / on_recording_finished). Absent
    /// channel = follow the global default unchanged. Applies to both PVR
    /// and Creator editions, so this lives in core rather than behind the
    /// `archiver`/Creator plugin boundary.
    #[serde(default)]
    pub channel_alerts: Vec<ChannelAlertEntry>,

    /// Named capture profiles (roadmap item 21), referenced by
    /// `AutoRecordEntry::profile`.
    #[serde(default)]
    pub capture_profiles: Vec<CaptureProfile>,

    /// Patreon creators whose new video posts should be auto-downloaded
    /// when the monitor sees them. Empty by default — the user has to
    /// opt in per creator (sidebar toggle); the monitor only refreshes
    /// the post cache for unlisted creators. Mirrors the
    /// auto_record_channels shape so the same UX patterns apply.
    #[serde(default)]
    pub auto_pull_creators: Vec<AutoPullEntry>,

    #[serde(default)]
    pub schedule: Vec<ScheduleEntry>,

    /// Config sections owned by plugins/editions outside core (e.g. any
    /// Creator Edition plugin's own settings table). Core has no typed
    /// opinion on their shape — this table exists purely so an existing
    /// `config.toml`'s plugin-owned sections round-trip (parse, and are
    /// written back unchanged) whether or not the owning plugin is even
    /// compiled into this binary. The owning plugin reads/writes its own
    /// slice via [`AppConfig::plugin_section`] / [`AppConfig::set_plugin_section`].
    #[serde(flatten)]
    pub extensions: BTreeMap<String, toml::Value>,

    /// Web UI (`strivo serve`) settings. Generated lazily on first
    /// `serve` invocation; persisted so the API key survives restarts.
    #[serde(default)]
    pub web: WebConfig,

    /// Desktop notification preferences — what state changes the daemon
    /// should fire a notify-rust banner for.
    #[serde(default)]
    pub notifications: NotificationsConfig,

    /// Capture-time safety knobs surfaced on the Monitor page. Zero in
    /// either field disables the corresponding cap; defaults are also
    /// zero so existing users see no behaviour change without explicit
    /// opt-in.
    #[serde(default)]
    pub monitor_limits: MonitorLimits,

    /// Per-plugin enable flags surfaced on Settings → Plugins. Keyed by
    /// plugin name; missing entries default to enabled=true. The runtime
    /// consults this when deciding whether to schedule a plugin's RPC
    /// verb or surface its UI controls — the actual gate is plugin-side.
    #[serde(default)]
    pub plugin_toggles: BTreeMap<String, PluginToggle>,

    /// Tracks the path this config was loaded from, so save() can use it
    #[serde(skip)]
    pub config_path: Option<PathBuf>,
}

/// `[web]` config section. (Part 11.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebConfig {
    /// API key for the `X-Api-Key` header on `/api/v1/*`. Persisted so
    /// repeated `strivo serve` invocations hand the same key to scripts.
    /// `None` means "generate on first run and save".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// HMAC secret used to sign browser-session cookies (W3). Persisted
    /// so cookies survive restarts. `None` means "generate on first
    /// session and save".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_secret: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwitchConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YouTubeConfig {
    pub client_id: String,
    pub client_secret: String,
    pub cookies_path: Option<PathBuf>,
    /// Public HTTPS URL of the WebSub (PubSubHubbub) callback served by
    /// `strivo serve` at `/yt-websub` (e.g. a `tailscale funnel` path). When
    /// set, the daemon subscribes each followed channel's upload feed to
    /// Google's hub so a new video / live broadcast triggers an immediate
    /// poll instead of waiting for the next interval. Unset = polling only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websub_callback_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatreonConfig {
    pub client_id: String,
    pub client_secret: String,
    #[serde(default = "default_patreon_poll_interval")]
    pub poll_interval_secs: u64,
    /// Path to a Netscape cookies.txt for the patron's logged-in Patreon
    /// session. Required to list/download a pledged creator's video posts —
    /// the Patreon API doesn't expose them to patrons, so StriVo falls back
    /// to yt-dlp + these cookies (mirrors the YouTube cookies_path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies_path: Option<PathBuf>,
}

fn default_patreon_poll_interval() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    #[serde(default)]
    pub transcode: bool,

    #[serde(default = "default_filename_template")]
    pub filename_template: String,

    /// Default format/bitrate selection. Per-channel `format` overrides this.
    #[serde(default)]
    pub format: RecordingFormat,

    /// On Twitch recordings, post-process the finished file to drop black
    /// segments left behind when streamlink suppresses ad breaks.
    #[serde(default)]
    pub auto_trim_ads: bool,

    /// Minimum contiguous black duration (seconds) to consider an ad break.
    /// Shorter blackouts (fades, transitions) are preserved.
    #[serde(default = "default_ad_min_secs")]
    pub ad_min_secs: f64,

    /// After a Twitch live recording ends, query the helix /videos endpoint
    /// for the just-finalized archive VOD and download it alongside the
    /// live capture. Saves to `<base>_vod.<ext>`. Useful because the live
    /// HLS pull misses the first ~5 minutes plus any ad-break gaps; the
    /// VOD captures the full broadcast.
    #[serde(default = "default_auto_vod_backfill")]
    pub auto_vod_backfill: bool,

    /// Seconds to wait after the live ends before polling for the VOD —
    /// Twitch takes a few minutes to finalize the archive.
    #[serde(default = "default_vod_backfill_delay_secs")]
    pub vod_backfill_delay_secs: u64,

    /// For Twitch + from_start recordings, attempt the live-from-start
    /// rewind path (helix → GQL → Usher /vod/v2) before falling back to
    /// the standard streamlink live-edge pull. When the streamer has
    /// "Always publish VODs" enabled, this lands at broadcast t=0
    /// instead of the ~5min HLS DVR window.
    #[serde(default = "default_twitch_live_from_start")]
    pub twitch_live_from_start: bool,
}

fn default_ad_min_secs() -> f64 {
    8.0
}

fn default_auto_vod_backfill() -> bool {
    true
}

fn default_vod_backfill_delay_secs() -> u64 {
    300
}

fn default_twitch_live_from_start() -> bool {
    true
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            transcode: false,
            filename_template: default_filename_template(),
            format: RecordingFormat::default(),
            auto_trim_ads: false,
            ad_min_secs: default_ad_min_secs(),
            auto_vod_backfill: default_auto_vod_backfill(),
            vod_backfill_delay_secs: default_vod_backfill_delay_secs(),
            twitch_live_from_start: default_twitch_live_from_start(),
        }
    }
}

/// Format / quality selection for a recording job.
///
/// All fields optional — `RecordingFormat::resolve(channel_override, global)` walks
/// channel → global → built-in defaults: `format = "bv*+ba/b"`, copy-mux into MKV.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecordingFormat {
    /// yt-dlp `-f` selector, e.g. `"bv*+ba/b"`, `"bestvideo[height<=1080]+bestaudio"`,
    /// `"bestaudio"`. Default: `"bv*+ba/b"` — a bare `"best"` only matches a
    /// pre-merged single-file format, which many videos never expose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Target video bitrate in kbps for transcode paths. Ignored when codec is `"copy"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,

    /// Container: `"mkv"` (default — crash-resilient) or `"mp4"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,

    /// `"copy"` (default), `"h264_nvenc"`, `"libx264"`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,

    /// `"copy"` (default), `"aac"`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<String>,
}

impl RecordingFormat {
    /// Merge with precedence: per-channel override → global default → built-in defaults.
    /// Result has every field populated.
    pub fn resolved(channel: Option<&Self>, global: &Self) -> ResolvedFormat {
        let pick = |c: fn(&Self) -> Option<&str>| -> String {
            channel
                .and_then(|x| c(x))
                .or_else(|| c(global))
                .map(String::from)
                .unwrap_or_default()
        };
        let format = if let Some(s) = channel
            .and_then(|x| x.format.as_deref())
            .or(global.format.as_deref())
        {
            s.to_string()
        } else {
            // See QualityTier::Best::format_selector's comment: a bare
            // "best" fails outright on videos with no pre-merged format.
            "bv*+ba/b".to_string()
        };
        let container = pick(|x| x.container.as_deref());
        let container = if container.is_empty() {
            "mkv".into()
        } else {
            container
        };
        let video_codec = pick(|x| x.video_codec.as_deref());
        let video_codec = if video_codec.is_empty() {
            "copy".into()
        } else {
            video_codec
        };
        let audio_codec = pick(|x| x.audio_codec.as_deref());
        let audio_codec = if audio_codec.is_empty() {
            "copy".into()
        } else {
            audio_codec
        };
        let bitrate_kbps = channel.and_then(|x| x.bitrate_kbps).or(global.bitrate_kbps);
        ResolvedFormat {
            format,
            bitrate_kbps,
            container,
            video_codec,
            audio_codec,
        }
    }
}

/// Fully-populated result of `RecordingFormat::resolved`.
#[derive(Debug, Clone)]
pub struct ResolvedFormat {
    pub format: String,
    pub bitrate_kbps: Option<u32>,
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
}

/// Per-channel override of the global live/upload alert switches.
/// `channel_key` is the `Platform:id` string used everywhere else in the
/// web API (`format!("{}:{}", platform, channel_id)`). `None` on either
/// field means "follow the global `[notifications]` default"; only an
/// explicit `Some(false)` suppresses that class of alert for this channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelAlertEntry {
    pub channel_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_live: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_upload: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRecordEntry {
    pub platform: String,
    pub channel_id: String,
    pub channel_name: String,

    /// Per-channel override of recording format/bitrate. Falls back to
    /// `recording.format` global when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<RecordingFormat>,

    /// Name of a `[[capture_profiles]]` entry to apply to this channel
    /// (roadmap item 21). Falls back to the global `[recording]` defaults
    /// when absent. Validated by `config_warnings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// Named quality presets that map to yt-dlp/streamlink format selectors.
/// A `CaptureProfile` carries one of these instead of a raw format string
/// so the UI can offer a stable, human-readable set of choices.
///
/// Tier → yt-dlp `-f` selector:
/// * `Best`      → `"bv*+ba/b"` (best video+audio, merged; falls back to a
///   pre-merged format only if separate streams aren't available)
/// * `P1080`     → `"bestvideo[height<=1080]+bestaudio/best[height<=1080]"`
/// * `P720`      → `"bestvideo[height<=720]+bestaudio/best[height<=720]"`
/// * `P480`      → `"bestvideo[height<=480]+bestaudio/best[height<=480]"`
/// * `AudioOnly` → `"bestaudio/best"` (no video stream; sets `audio_only` semantics)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Best,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
    AudioOnly,
}

/// Streamlink quality selection: stream quality argument plus optional stream
/// sorting excludes ceiling and codec unlock flag.
///
/// Returned by [`QualityTier::streamlink_selection`]; applied by the resolver
/// when building the streamlink command line for a Twitch live stream.
#[derive(Debug, Clone)]
pub struct StreamlinkSelection {
    /// Streamlink stream quality argument. May be a comma-separated fallback
    /// list (e.g. `"best,best-unfiltered"`), tried left-to-right by streamlink
    /// until one of the named qualities is available.
    pub stream: &'static str,
    /// When set, passed as `--stream-sorting-excludes <value>` to cap the
    /// maximum stream quality that `best` will resolve to. Streams ranked above
    /// this threshold are skipped during sorting; the `best-unfiltered` fallback
    /// in the stream arg bypasses the filter if no streams remain after exclusion.
    pub sorting_excludes: Option<&'static str>,
    /// When true, add `--twitch-supported-codecs=h264,h265,av1` so Twitch's
    /// optional h265/av1 streams are visible to streamlink alongside h264.
    /// Only set for tiers where higher-quality codecs matter (Best, P1080).
    pub extended_codecs: bool,
}

impl QualityTier {
    /// Translate to the yt-dlp `-f` format selector string.
    pub fn format_selector(&self) -> &'static str {
        match self {
            // Not a bare "best": that selector only matches a pre-merged
            // single-file format, which many modern YouTube videos (and
            // most past-livestream VODs) never expose at all -- yt-dlp
            // fails outright with "Requested format is not available."
            // bv*+ba/b (bestvideo+bestaudio, falling back to best) is the
            // same selector already validated for YouTube --live-from-start
            // (src/recording/ytdlp.rs), applied consistently here too.
            QualityTier::Best => "bv*+ba/b",
            QualityTier::P1080 => "bestvideo[height<=1080]+bestaudio/best[height<=1080]",
            QualityTier::P720 => "bestvideo[height<=720]+bestaudio/best[height<=720]",
            QualityTier::P480 => "bestvideo[height<=480]+bestaudio/best[height<=480]",
            QualityTier::AudioOnly => "bestaudio/best",
        }
    }

    /// Translate to a streamlink quality selection.
    ///
    /// Streamlink uses named quality levels and an optional sort-excludes
    /// ceiling rather than yt-dlp `-f` selectors. The mapping mirrors the
    /// yt-dlp `[height<=N]` semantics already used by [`format_selector`]:
    ///
    /// * `Best`      → `"best"`, no ceiling, extended codecs enabled.
    /// * `P1080`     → `"best,best-unfiltered"`, ceiling `>1080p60`, extended codecs.
    /// * `P720`      → `"best,best-unfiltered"`, ceiling `>720p60`.
    /// * `P480`      → `"best,best-unfiltered"`, ceiling `>480p`.
    /// * `AudioOnly` → `"audio_only,worst"`, no ceiling.
    ///
    /// The `"best,best-unfiltered"` pattern means: pick the best stream that
    /// isn't excluded by the ceiling; if the exclude filter leaves nothing
    /// (unusual stream list), fall back to `best-unfiltered` which ignores
    /// the sort-excludes entirely. This prevents a silent no-stream error on
    /// channels with non-standard quality names.
    pub fn streamlink_selection(&self) -> StreamlinkSelection {
        match self {
            QualityTier::Best => StreamlinkSelection {
                stream: "best",
                sorting_excludes: None,
                // Unlock h265/av1 Twitch streams for maximum-quality captures.
                extended_codecs: true,
            },
            QualityTier::P1080 => StreamlinkSelection {
                // Twitch's 60fps variant ("1080p60") ranks above the 30fps
                // "1080p" name in streamlink's sort order. The ceiling must be
                // set to the 60fps name so both 1080p and 1080p60 are kept
                // while anything hypothetically above 1080p60 is excluded.
                // In practice nothing is above 1080p60, so this is a safety cap.
                stream: "best,best-unfiltered",
                sorting_excludes: Some(">1080p60"),
                // Twitch's h265/av1 encodes (where available) are most
                // beneficial at 1080p60; unlock them here.
                extended_codecs: true,
            },
            QualityTier::P720 => StreamlinkSelection {
                // "720p60" ranks above "720p"; ceiling at >720p60 keeps both
                // 720p variants in scope and excludes 1080p and above.
                stream: "best,best-unfiltered",
                sorting_excludes: Some(">720p60"),
                extended_codecs: false,
            },
            QualityTier::P480 => StreamlinkSelection {
                // 480p is typically 30fps only on Twitch (no "480p60" variant).
                stream: "best,best-unfiltered",
                sorting_excludes: Some(">480p"),
                extended_codecs: false,
            },
            QualityTier::AudioOnly => StreamlinkSelection {
                // "audio_only" is Twitch's dedicated audio-only track.
                // Fall back to "worst" (lowest-bitrate video stream) for
                // platforms or streams that don't advertise audio_only.
                stream: "audio_only,worst",
                sorting_excludes: None,
                extended_codecs: false,
            },
        }
    }

    /// True when the tier selects no video (audio-only pipeline).
    pub fn is_audio_only(&self) -> bool {
        matches!(self, QualityTier::AudioOnly)
    }
}

/// A named, reusable capture profile (roadmap item 21) — define recording
/// settings once ("1080p60+transcript", "audio-only") and attach to many
/// channels via `AutoRecordEntry::profile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureProfile {
    /// Unique profile name referenced by `AutoRecordEntry::profile`.
    pub name: String,

    /// Named quality tier. When set, the tier's format selector is used as
    /// the effective yt-dlp `-f` argument unless `format.format` is also
    /// set (explicit selector wins over the tier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality_tier: Option<QualityTier>,

    /// Format/bitrate selection for channels using this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<RecordingFormat>,

    /// Transcode the finished capture (overrides the global default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcode: Option<bool>,

    /// Capture audio only (no video).
    #[serde(default)]
    pub audio_only: bool,

    /// Request a transcript for captures using this profile.
    #[serde(default)]
    pub transcript: bool,

    /// Re-capture cutoff: once this many episodes are recorded for a channel
    /// using this profile, StriVo stops auto-capturing it. `None` = no cutoff
    /// (capture indefinitely). `Some(0)` is nonsensical and flagged by
    /// `config_warnings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutoff_episodes: Option<u32>,
}

/// Per-channel opt-in to auto-downloading a Twitch/YouTube channel's new
/// uploads, as distinct from `AutoRecordEntry`'s "capture the live stream
/// itself" switch. Mirrors `AutoRecordEntry`'s shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDownloadUploadsEntry {
    pub platform: String,
    pub channel_id: String,
    pub channel_name: String,
}

/// Per-creator opt-in to Patreon auto-download. Identifying by
/// campaign_id rather than the creator name because creators rename
/// themselves. creator_name is kept around purely for UI display when
/// the monitor hasn't yet refreshed pledged_creators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoPullEntry {
    pub campaign_id: String,
    #[serde(default)]
    pub creator_name: String,
}

/// Schedule-based recording entry.
/// Uses cron syntax for time-based recordings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleEntry {
    pub channel: String,
    pub cron: String,
    #[serde(default = "default_schedule_duration")]
    pub duration: String,
}

fn default_schedule_duration() -> String {
    "4h".to_string()
}

fn default_theme() -> String {
    "neon".to_string()
}

/// How `theme` is expressed in `config.toml`. Accepts either the legacy bare
/// string (`theme = "neon"`) or a rich table with per-slot overrides:
///
/// ```toml
/// [theme]
/// name = "tokyo-night"
/// [theme.colors]
/// primary = "#00E5FF"
/// [theme.ansi]
/// red = "#FF5555"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ThemeRef {
    Named(String),
    Rich(ThemeSpec),
}

impl Default for ThemeRef {
    fn default() -> Self {
        ThemeRef::Named(default_theme())
    }
}

impl ThemeRef {
    pub fn name(&self) -> &str {
        match self {
            ThemeRef::Named(s) => s.as_str(),
            ThemeRef::Rich(s) => s.name.as_str(),
        }
    }

    /// Color slot overrides (keyed by semantic slot name: `bg`, `primary`, …).
    pub fn colors(&self) -> &BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
        match self {
            ThemeRef::Rich(s) => &s.colors,
            ThemeRef::Named(_) => EMPTY.get_or_init(BTreeMap::new),
        }
    }

    /// ANSI slot overrides (keyed by `red`, `blue`, …).
    pub fn ansi(&self) -> &BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
        match self {
            ThemeRef::Rich(s) => &s.ansi,
            ThemeRef::Named(_) => EMPTY.get_or_init(BTreeMap::new),
        }
    }

    /// Replace the theme name, preserving any overrides.
    pub fn set_name(&mut self, new_name: String) {
        match self {
            ThemeRef::Named(s) => *s = new_name,
            ThemeRef::Rich(s) => s.name = new_name,
        }
    }
}

/// Desktop notification preferences. Wired by the daemon's existing
/// notify-rust integration — each flag gates one class of banner.
/// Defaults err on the side of useful-but-not-noisy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Master switch. When false the daemon skips every notify call.
    #[serde(default = "default_true")]
    pub desktop_enabled: bool,
    /// Banner when a tracked channel transitions offline → live.
    #[serde(default = "default_true")]
    pub on_go_live: bool,
    /// Banner when a recording finishes successfully (Twitch / YT live
    /// + Patreon VOD pull). Useful but can be noisy with bulk catalogs.
    #[serde(default = "default_true")]
    pub on_recording_finished: bool,
    /// Banner when a recording dies in the middle. Always default-on:
    /// silent failures are the worst class of bug in a PVR.
    #[serde(default = "default_true")]
    pub on_recording_failed: bool,
    /// Banner when the Twitch auto-VOD-backfill pull lands. Off by
    /// default because most users don't track it manually.
    #[serde(default)]
    pub on_vod_ready: bool,
    /// Outbound webhook — POST JSON on notification-worthy events.
    /// Off by default; set `[notifications.webhook] enabled = true` and
    /// `url = "https://…"` to opt in.
    #[serde(default)]
    pub webhook: WebhookConfig,
}

fn default_true() -> bool {
    true
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            desktop_enabled: true,
            on_go_live: true,
            on_recording_finished: true,
            on_recording_failed: true,
            on_vod_ready: false,
            webhook: WebhookConfig::default(),
        }
    }
}

/// Outbound webhook configuration (`[notifications.webhook]`).
///
/// When enabled, the daemon POSTs a JSON payload to `url` for each
/// notification-worthy `DaemonEvent` (channel live, recording finished /
/// failed, generic `Notification`). The payload shape mirrors streamerREC's
/// webhook contract so existing Make/n8n/Zapier integrations work without
/// adapter changes. Failures are logged and swallowed — the event loop is
/// never blocked.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Whether outbound webhooks are enabled. Off by default to avoid
    /// silent firing when `url` is accidentally set.
    #[serde(default)]
    pub enabled: bool,
    /// Full HTTPS (or HTTP) URL to POST events to. Required when
    /// `enabled = true`; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Per-plugin enable flag. Defaults to true so a missing entry preserves
/// the previous always-on behaviour. Kept as its own struct so future
/// per-plugin knobs (storage cap, RPC quota, …) land here without
/// invalidating the existing config schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToggle {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for PluginToggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Capture-time safety knobs. Both fields default to 0 (= disabled) so
/// existing config files keep the previous unlimited behaviour. Set
/// either to a positive value to opt in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MonitorLimits {
    /// Maximum number of recordings (live + VOD pull combined) StriVo
    /// will run concurrently. New live broadcasts beyond this limit are
    /// surfaced but not started until a slot opens. 0 = unlimited.
    #[serde(default)]
    pub max_concurrent_recordings: u32,

    /// Reserved disk space in GB. When free space on the recording
    /// filesystem drops below this watermark, new captures are deferred.
    /// 0 = no circuit breaker.
    #[serde(default)]
    pub disk_budget_reserved_gb: u32,
}

/// Motion / accessibility / verbosity preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    /// When true, animations snap to their end state instead of tweening.
    /// Mirrors `STRIVO_REDUCE_MOTION`; whichever is set wins.
    #[serde(default)]
    pub reduce_motion: bool,

    /// Verbose status-bar labels for screen readers / low-vision users.
    #[serde(default)]
    pub verbose_status: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeSpec {
    #[serde(default = "default_theme")]
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ansi: BTreeMap<String, String>,
}

fn default_recording_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().join("Videos").join("StriVo"))
        .unwrap_or_else(|| PathBuf::from("./recordings"))
}

fn default_poll_interval() -> u64 {
    60
}

fn default_filename_template() -> String {
    "{channel}_{date}_{title}.mkv".to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            recording_dir: default_recording_dir(),
            poll_interval_secs: default_poll_interval(),
            twitch: None,
            youtube: None,
            patreon: None,
            recording: RecordingConfig::default(),
            theme: ThemeRef::default(),
            ui: UiConfig::default(),
            auto_record_channels: Vec::new(),
            auto_download_uploads: Vec::new(),
            channel_alerts: Vec::new(),
            capture_profiles: Vec::new(),
            auto_pull_creators: Vec::new(),
            schedule: Vec::new(),
            extensions: BTreeMap::new(),
            web: WebConfig::default(),
            notifications: NotificationsConfig::default(),
            monitor_limits: MonitorLimits::default(),
            plugin_toggles: BTreeMap::new(),
            config_path: None,
        }
    }
}

impl AppConfig {
    pub fn config_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "strivo")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// The capture profile attached to an auto-record channel, if any
    /// (roadmap item 21). `platform` is the `Display` form ("Twitch", …).
    pub fn capture_profile_for(&self, platform: &str, channel_id: &str) -> Option<&CaptureProfile> {
        let entry = self
            .auto_record_channels
            .iter()
            .find(|a| a.channel_id == channel_id && a.platform.eq_ignore_ascii_case(platform))?;
        let name = entry.profile.as_ref()?;
        self.capture_profiles.iter().find(|p| &p.name == name)
    }

    /// Effective transcode setting for an auto-record channel: the channel's
    /// capture profile overrides the global `[recording]` default.
    pub fn effective_transcode(&self, platform: &str, channel_id: &str) -> bool {
        self.capture_profile_for(platform, channel_id)
            .and_then(|p| p.transcode)
            .unwrap_or(self.recording.transcode)
    }

    /// Marker filenames a catalog pull should touch in each landed episode
    /// directory, so callers of [`crate::recording::catalog::run_pull`]
    /// don't need to know what optional (e.g. Creator Edition) config, if
    /// any, is compiled in. `suppress` lets a caller opt this pull out (a
    /// CLI flag, say) without needing to know what the markers mean either.
    ///
    /// `registrations` is the caller-supplied `(extension section name,
    /// marker filename)` list: for each entry whose `extensions[section]`
    /// table has `enabled = true`, that marker is included. Core names no
    /// section and no marker itself — it only knows "an enabled extension
    /// section can ask for a marker file", so an unrelated extension
    /// section with its own `enabled = true` flag produces nothing unless
    /// something actually registers that section here (today, nothing
    /// does for any section besides the one Creator Edition's tandem
    /// transcription plugin registers). Empty in the pure-PVR build, which
    /// always passes an empty `registrations` slice.
    ///
    /// (ADR 0001, CE01): the daemon's bulk-download path
    /// (`src/recording/bulk.rs`) calls this synchronously from inside core,
    /// before any plugin instance exists to ask "what markers do you want
    /// written on tandem auto-trigger" — so the registration list has to
    /// reach it as data, supplied by whoever composes plugins into the
    /// running app (`DaemonPluginHost`, populated at daemon startup) rather
    /// than a value core can compute on its own. See
    /// [`crate::daemon::DaemonPluginHost`].
    pub fn post_pull_markers(&self, suppress: bool, registrations: &[(&str, &str)]) -> Vec<String> {
        if suppress {
            return Vec::new();
        }
        registrations
            .iter()
            .filter(|(section, _marker)| {
                self.extensions
                    .get(*section)
                    .and_then(|v| v.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            })
            .map(|(_section, marker)| marker.to_string())
            .collect()
    }

    /// Lint the config for pathological capture-profile / auto-record setups
    /// (roadmap item 21). Returns human-readable warnings; the daemon logs
    /// them at startup. Pure (no I/O) so it's unit-testable.
    pub fn config_warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        // Duplicate profile names — the later one silently wins on lookup.
        let mut seen = std::collections::HashSet::new();
        for p in &self.capture_profiles {
            if !seen.insert(p.name.as_str()) {
                warnings.push(format!(
                    "capture profile '{}' is defined more than once",
                    p.name
                ));
            }
            // A zero cutoff would block all capture for attached channels.
            if p.cutoff_episodes == Some(0) {
                warnings.push(format!(
                    "capture profile '{}' has cutoff_episodes = 0; it will never record",
                    p.name
                ));
            }
        }

        // auto_record entries referencing a profile that doesn't exist.
        for ch in &self.auto_record_channels {
            if let Some(ref prof) = ch.profile {
                if !self.capture_profiles.iter().any(|p| &p.name == prof) {
                    warnings.push(format!(
                        "channel '{}' references unknown capture profile '{}'",
                        ch.channel_name, prof
                    ));
                }
            }
        }

        // Perpetual re-record: a channel that is BOTH auto-recorded (every live
        // session) AND on a cron schedule will double-capture the same content.
        for ch in &self.auto_record_channels {
            if self
                .schedule
                .iter()
                .any(|s| s.channel.eq_ignore_ascii_case(&ch.channel_name))
            {
                warnings.push(format!(
                    "channel '{}' is both auto-recorded and scheduled — likely perpetual re-capture",
                    ch.channel_name
                ));
            }
        }

        warnings
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn cache_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "strivo")
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".cache"))
    }

    pub fn data_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "strivo")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".data"))
    }

    pub fn state_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "strivo")
            .map(|d| d.state_dir().unwrap_or(d.data_dir()).to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".state"))
    }

    /// Deserializes an extension section (e.g. an owning plugin's own
    /// `[<name>]` `config.toml` table, captured generically in
    /// `extensions`) into its typed shape `T`. Defaults when the section
    /// is absent or fails to parse into `T`, so a plugin upgrading its
    /// own config shape never hard-fails the whole config load.
    pub fn plugin_section<T>(&self, name: &str) -> T
    where
        T: Default + serde::de::DeserializeOwned,
    {
        self.extensions
            .get(name)
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default()
    }

    /// Like [`AppConfig::plugin_section`], but for a plugin that has
    /// renamed its own section at some point: tries each name in
    /// `names` in order and returns the first one present, so a
    /// `config.toml` written under the old name keeps working. Core
    /// doesn't need to know what the names mean or which is "current" —
    /// that's entirely the calling plugin's own migration history.
    pub fn plugin_section_aliased<T>(&self, names: &[&str]) -> T
    where
        T: Default + serde::de::DeserializeOwned,
    {
        for name in names {
            if let Some(v) = self.extensions.get(*name) {
                if let Ok(t) = v.clone().try_into() {
                    return t;
                }
            }
        }
        T::default()
    }

    /// Replaces an extension section with `value`'s serialized form, so
    /// the next [`AppConfig::save`] writes it back under `[<name>]`.
    pub fn set_plugin_section<T: Serialize>(&mut self, name: &str, value: &T) -> Result<()> {
        let v = toml::Value::try_from(value).context("serialize plugin config section")?;
        self.extensions.insert(name.to_string(), v);
        Ok(())
    }

    pub fn load(path: Option<&std::path::Path>) -> Result<Self> {
        let path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(Self::config_path);

        if !path.exists() {
            let mut config = Self::default();
            config.config_path = Some(path.clone());
            config.save(Some(&path))?;
            return Ok(config);
        }

        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;

        match toml::from_str::<Self>(&contents) {
            Ok(mut config) => {
                config.config_path = Some(path);
                Ok(config)
            }
            Err(parse_err) => {
                // Live file is corrupt. Try `.backup` (previous known-good),
                // else quarantine the bad file and fall back to defaults so
                // the user is never stranded without a runnable config.
                let backup = backup_path(&path);
                if backup.exists() {
                    if let Ok(bcontents) = std::fs::read_to_string(&backup) {
                        if let Ok(mut config) = toml::from_str::<Self>(&bcontents) {
                            let _ = quarantine(&path);
                            let _ = std::fs::copy(&backup, &path);
                            config.config_path = Some(path);
                            return Ok(config);
                        }
                    }
                }
                let _ = quarantine(&path);
                let mut config = Self::default();
                config.config_path = Some(path.clone());
                let _ = config.save(Some(&path));
                eprintln!(
                    "config: {} was malformed ({}); fell back to defaults",
                    path.display(),
                    parse_err
                );
                Ok(config)
            }
        }
    }

    /// Reset all user-authored values to their defaults, **preserving
    /// credentials** (`twitch`, `youtube`, `patreon`) so the user is
    /// not logged out by a reset. Schedule entries, auto-record
    /// channels, recording defaults, UI prefs, plugin config — all
    /// revert. The caller is responsible for `save()`.
    ///
    /// M2.1.c — defaults-as-preset model. Defaults live in code
    /// (`Default::default()`); the on-disk file is the user's overlay.
    /// "Reset" means "write the empty overlay back."
    pub fn reset_to_defaults(&mut self) {
        let mut fresh = Self::default();
        fresh.config_path = self.config_path.clone();
        fresh.twitch = self.twitch.take();
        fresh.youtube = self.youtube.take();
        fresh.patreon = self.patreon.take();
        *self = fresh;
    }

    pub fn save(&self, path: Option<&std::path::Path>) -> Result<()> {
        let path = path
            .map(|p| p.to_path_buf())
            .or_else(|| self.config_path.clone())
            .unwrap_or_else(Self::config_path);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }

        // Rotate the prior live file into `.backup` before overwriting so
        // a crash mid-write can never wedge the user into a broken state.
        if path.exists() {
            let backup = backup_path(&path);
            let _ = std::fs::copy(&path, &backup);
        }

        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;
        // Write to a sibling temp file then atomically rename into place, so a
        // crash mid-write leaves the live config intact rather than truncated.
        let tmp = {
            let mut s = path.clone().into_os_string();
            s.push(".tmp");
            PathBuf::from(s)
        };
        std::fs::write(&tmp, contents)
            .with_context(|| format!("Failed to write config to {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Failed to persist config to {}", path.display()))?;
        Ok(())
    }
}

fn backup_path(path: &std::path::Path) -> PathBuf {
    let mut s = path.to_path_buf().into_os_string();
    s.push(".backup");
    PathBuf::from(s)
}

fn quarantine(path: &std::path::Path) -> std::io::Result<()> {
    let mut s = path.to_path_buf().into_os_string();
    s.push(".corrupt");
    std::fs::rename(path, PathBuf::from(s))
}

#[cfg(test)]
mod quality_tier_tests {
    use super::*;

    #[test]
    fn tier_format_selectors() {
        assert_eq!(QualityTier::Best.format_selector(), "bv*+ba/b");
        assert_eq!(
            QualityTier::P1080.format_selector(),
            "bestvideo[height<=1080]+bestaudio/best[height<=1080]"
        );
        assert_eq!(
            QualityTier::P720.format_selector(),
            "bestvideo[height<=720]+bestaudio/best[height<=720]"
        );
        assert_eq!(
            QualityTier::P480.format_selector(),
            "bestvideo[height<=480]+bestaudio/best[height<=480]"
        );
        assert_eq!(QualityTier::AudioOnly.format_selector(), "bestaudio/best");
    }

    #[test]
    fn tier_audio_only_flag() {
        assert!(QualityTier::AudioOnly.is_audio_only());
        assert!(!QualityTier::Best.is_audio_only());
        assert!(!QualityTier::P1080.is_audio_only());
    }

    #[test]
    fn streamlink_selection_best() {
        let sel = QualityTier::Best.streamlink_selection();
        assert_eq!(sel.stream, "best");
        assert!(sel.sorting_excludes.is_none());
        assert!(sel.extended_codecs, "Best should unlock extended codecs");
    }

    #[test]
    fn streamlink_selection_1080p() {
        let sel = QualityTier::P1080.streamlink_selection();
        assert_eq!(sel.stream, "best,best-unfiltered");
        assert_eq!(sel.sorting_excludes, Some(">1080p60"));
        assert!(sel.extended_codecs, "P1080 should unlock extended codecs");
    }

    #[test]
    fn streamlink_selection_720p() {
        let sel = QualityTier::P720.streamlink_selection();
        assert_eq!(sel.stream, "best,best-unfiltered");
        assert_eq!(sel.sorting_excludes, Some(">720p60"));
        assert!(!sel.extended_codecs);
    }

    #[test]
    fn streamlink_selection_480p() {
        let sel = QualityTier::P480.streamlink_selection();
        assert_eq!(sel.stream, "best,best-unfiltered");
        assert_eq!(sel.sorting_excludes, Some(">480p"));
        assert!(!sel.extended_codecs);
    }

    #[test]
    fn streamlink_selection_audio_only() {
        let sel = QualityTier::AudioOnly.streamlink_selection();
        assert_eq!(sel.stream, "audio_only,worst");
        assert!(sel.sorting_excludes.is_none());
        assert!(!sel.extended_codecs);
    }

    #[test]
    fn tier_serde_roundtrip() {
        let t = QualityTier::P1080;
        let s = serde_json::to_string(&t).unwrap();
        assert_eq!(s, r#""1080p""#);
        let back: QualityTier = serde_json::from_str(&s).unwrap();
        assert_eq!(back, QualityTier::P1080);
    }

    #[test]
    fn audio_only_serde_name() {
        let s = serde_json::to_string(&QualityTier::AudioOnly).unwrap();
        assert_eq!(s, r#""audio_only""#);
    }

    #[test]
    fn capture_profile_with_tier_is_backward_compat() {
        // A config.toml without quality_tier should deserialise fine (old format).
        let toml = r#"
            [[capture_profiles]]
            name = "legacy"
            audio_only = false
            transcript = false
        "#;
        let cfg: AppConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.capture_profiles.len(), 1);
        assert!(cfg.capture_profiles[0].quality_tier.is_none());
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    fn arc(name: &str, profile: Option<&str>) -> AutoRecordEntry {
        AutoRecordEntry {
            platform: "Twitch".into(),
            channel_id: "1".into(),
            channel_name: name.into(),
            format: None,
            profile: profile.map(String::from),
        }
    }

    #[test]
    fn warns_on_unknown_profile_and_zero_cutoff_and_dupes() {
        let mut cfg = AppConfig::default();
        cfg.capture_profiles = vec![
            CaptureProfile {
                name: "hq".into(),
                quality_tier: None,
                format: None,
                transcode: None,
                audio_only: false,
                transcript: true,
                cutoff_episodes: Some(0),
            },
            CaptureProfile {
                name: "hq".into(), // duplicate
                quality_tier: None,
                format: None,
                transcode: None,
                audio_only: false,
                transcript: false,
                cutoff_episodes: None,
            },
        ];
        cfg.auto_record_channels = vec![arc("Foo", Some("nonexistent"))];
        let w = cfg.config_warnings();
        assert!(
            w.iter().any(|s| s.contains("defined more than once")),
            "{w:?}"
        );
        assert!(w.iter().any(|s| s.contains("never record")), "{w:?}");
        assert!(
            w.iter().any(|s| s.contains("unknown capture profile")),
            "{w:?}"
        );
    }

    #[test]
    fn warns_on_auto_record_plus_schedule() {
        let mut cfg = AppConfig::default();
        cfg.auto_record_channels = vec![arc("LilAggy", None)];
        cfg.schedule = vec![ScheduleEntry {
            channel: "lilaggy".into(),
            cron: "0 20 * * *".into(),
            duration: "4h".into(),
        }];
        let w = cfg.config_warnings();
        assert!(
            w.iter().any(|s| s.contains("perpetual re-capture")),
            "{w:?}"
        );
    }

    #[test]
    fn effective_transcode_honours_profile_override() {
        let mut cfg = AppConfig::default();
        cfg.recording.transcode = false; // global default off
        cfg.capture_profiles = vec![CaptureProfile {
            name: "hq".into(),
            quality_tier: None,
            format: None,
            transcode: Some(true), // profile turns it on
            audio_only: false,
            transcript: false,
            cutoff_episodes: None,
        }];
        cfg.auto_record_channels = vec![AutoRecordEntry {
            platform: "Twitch".into(),
            channel_id: "42".into(),
            channel_name: "Foo".into(),
            format: None,
            profile: Some("hq".into()),
        }];
        // Channel with the profile → override wins.
        assert!(cfg.effective_transcode("Twitch", "42"));
        // Unknown channel → global default.
        assert!(!cfg.effective_transcode("Twitch", "999"));
    }

    #[test]
    fn clean_config_has_no_warnings() {
        let mut cfg = AppConfig::default();
        cfg.capture_profiles = vec![CaptureProfile {
            name: "hq".into(),
            quality_tier: None,
            format: None,
            transcode: Some(true),
            audio_only: false,
            transcript: true,
            cutoff_episodes: Some(5),
        }];
        cfg.auto_record_channels = vec![arc("Foo", Some("hq"))];
        assert!(cfg.config_warnings().is_empty());
    }
}
