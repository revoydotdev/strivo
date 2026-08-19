pub mod adtrim;
pub mod bulk;
pub mod catalog;
pub mod chapters;
pub mod container;
pub mod ffmpeg;
pub mod job;
pub mod persist;
pub mod remux;
pub mod scan;
pub mod schedule;
pub mod segments;
pub mod thumbnail;
pub mod trash;
pub mod vod_backfill;
pub mod ytdlp;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{AppConfig, QualityTier, RecordingFormat, ResolvedFormat};
use crate::events::DaemonEvent;
use crate::platform::PlatformKind;
use crate::recording::ffmpeg::{FfmpegBuilder, FfmpegProcess};
use crate::recording::job::{RecordingJob, RecordingState};
use crate::recording::ytdlp::YtDlpProcess;
use crate::stream::resolver;

/// Resolve the format/quality settings for a recording, walking
/// per-channel override → capture-profile tier → global → built-in defaults.
///
/// Resolution order for the yt-dlp `-f` selector:
/// 1. Per-channel `format.format` (explicit override on the auto-record entry)
/// 2. Capture profile `format.format` (explicit override on the profile)
/// 3. Capture profile `quality_tier` (named preset → format selector)
/// 4. Global `recording.format.format`
/// 5. Built-in default `"best"`
pub fn resolve_format(
    config: &AppConfig,
    channel_id: &str,
    platform: PlatformKind,
) -> ResolvedFormat {
    let platform_str = platform.to_string();
    let entry = config
        .auto_record_channels
        .iter()
        .find(|c| c.channel_id == channel_id && c.platform == platform_str);

    let channel_override = entry.and_then(|c| c.format.as_ref());

    // Build an effective "intermediate" format from the capture profile:
    // if the profile has an explicit format, use it; if it has a quality_tier
    // but no explicit format, synthesise one from the tier's selector.
    // We use a local to own any synthesised RecordingFormat so the reference
    // to it (`profile_fmt`) stays valid for the merge below.
    let synthesised_tier: Option<RecordingFormat> = entry
        .and_then(|e| e.profile.as_ref())
        .and_then(|name| config.capture_profiles.iter().find(|p| &p.name == name))
        .and_then(|profile| {
            if profile.format.is_some() {
                None // explicit format on profile wins; handled below
            } else {
                profile.quality_tier.as_ref().map(|tier| RecordingFormat {
                    format: Some(tier.format_selector().to_string()),
                    ..Default::default()
                })
            }
        });

    let profile_fmt: Option<&RecordingFormat> = entry
        .and_then(|e| e.profile.as_ref())
        .and_then(|name| config.capture_profiles.iter().find(|p| &p.name == name))
        .and_then(|profile| profile.format.as_ref().or(synthesised_tier.as_ref()));

    // 3-level merge: channel_override > profile_fmt > global.
    // We collapse profile_fmt + global into a single RecordingFormat first
    // (profile fills gaps in global), then apply channel_override on top.
    let effective_global = if let Some(pf) = profile_fmt {
        // Build a merged RecordingFormat from profile + global so channel
        // override has a single RecordingFormat to sit on top of.
        let r = RecordingFormat::resolved(Some(pf), &config.recording.format);
        RecordingFormat {
            format: Some(r.format),
            bitrate_kbps: r.bitrate_kbps,
            container: Some(r.container),
            video_codec: Some(r.video_codec),
            audio_codec: Some(r.audio_codec),
        }
    } else {
        config.recording.format.clone()
    };

    RecordingFormat::resolved(channel_override, &effective_global)
}

/// Resolve the effective `QualityTier` for a recording, if one is set.
///
/// Walks: channel auto-record entry → capture profile → `quality_tier`.
/// Returns `None` when no profile is attached or the profile has no tier,
/// which preserves the pre-tier `"best"` fallback behaviour.
fn resolve_quality_tier(
    config: &AppConfig,
    channel_id: &str,
    platform: PlatformKind,
) -> Option<QualityTier> {
    let platform_str = platform.to_string();
    let entry = config
        .auto_record_channels
        .iter()
        .find(|c| c.channel_id == channel_id && c.platform == platform_str)?;
    let profile_name = entry.profile.as_ref()?;
    let profile = config
        .capture_profiles
        .iter()
        .find(|p| &p.name == profile_name)?;
    profile.quality_tier.clone()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum RecordingCommand {
    Start {
        channel_id: String,
        channel_name: String,
        /// Human-readable channel name for filename slugs (YT-1). The
        /// recording manager prefers this over `channel_name` when
        /// building the output path. Falls back to `channel_name` for
        /// older callers (e.g. schedule-fired starts).
        #[serde(default)]
        display_name: Option<String>,
        platform: PlatformKind,
        transcode: bool,
        cookies_path: Option<PathBuf>,
        stream_title: Option<String>,
        from_start: bool,
        /// If provided, the recording manager uses this ID instead of generating a new one.
        /// Used by the schedule manager to track job IDs for timed Stop commands.
        job_id: Option<Uuid>,
        /// Source stream thumbnail URL (snapshotted to local cache at start).
        #[serde(default)]
        thumbnail_url: Option<String>,
    },
    Stop {
        job_id: Uuid,
    },
    StopAll,
    DownloadVod {
        url: String,
        channel_name: String,
        platform: PlatformKind,
        output_path: PathBuf,
        cookies_path: Option<PathBuf>,
        post_title: Option<String>,
    },
}

/// Recordings that have not grown in bytes for this long are considered stalled.
/// A stalled ffmpeg/yt-dlp process stays `Recording` forever without this guard.
/// Value is intentionally a const (not a config field) — 2 min is a safe global
/// minimum; a per-channel override would belong to the monitor layer.
const STALL_TIMEOUT_SECS: u64 = 120;

/// Unified recorder process — either FFmpeg or yt-dlp
enum RecorderProcess {
    Ffmpeg(FfmpegProcess),
    YtDlp(YtDlpProcess),
}

impl RecorderProcess {
    async fn stop(&mut self) -> Result<()> {
        match self {
            Self::Ffmpeg(p) => p.stop().await,
            Self::YtDlp(p) => p.stop().await,
        }
    }

    fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        match self {
            Self::Ffmpeg(p) => p.try_wait(),
            Self::YtDlp(p) => p.try_wait(),
        }
    }

    fn file_size(&self) -> u64 {
        match self {
            Self::Ffmpeg(p) => p.file_size(),
            Self::YtDlp(p) => p.file_size(),
        }
    }

    /// Parsed yt-dlp `[download]` progress for VOD pulls. Ffmpeg-driven
    /// live captures have no known total, so their progress is always None.
    fn download_progress(&self) -> crate::recording::ytdlp::DownloadProgress {
        match self {
            Self::YtDlp(p) => p.progress(),
            Self::Ffmpeg(_) => crate::recording::ytdlp::DownloadProgress::default(),
        }
    }

    fn stderr_tail(&self) -> String {
        match self {
            Self::Ffmpeg(p) => p.stderr_tail(),
            Self::YtDlp(p) => p.stderr_tail(),
        }
    }
}

struct ActiveRecording {
    job: RecordingJob,
    process: Option<RecorderProcess>,
    retry_count: u32,
    cookies_path: Option<PathBuf>,
    /// All on-disk segments produced for this recording so far. Element 0
    /// is always the original output path; subsequent retries append
    /// `_partN.mkv` paths via `segments::segment_path`. On Finished the
    /// orchestrator merges them back into the base path via mkvmerge
    /// (M5.5).
    segments: Vec<PathBuf>,
    /// Stall detection: bytes observed on the previous poll tick.
    last_bytes: u64,
    /// Stall detection: wall-clock instant when `bytes_written` last grew.
    /// Reset each time the state transitions to `Recording` so a slow
    /// resolve phase does not eat into the stall budget.
    last_growth_at: std::time::Instant,
    /// For YouTube `--live-from-start` recordings, the resolved
    /// `watch?v=…` URL to pass to `YtDlpProcess` on gap-resume retries.
    /// `None` for FFmpeg-driven captures (Twitch, plain YouTube, etc.).
    from_start_watch_url: Option<String>,
}

/// Run the post-completion pipeline for one recording: optional segment
/// merge, ad-trim, and container-normalisation remux, then publish
/// `RecordingFinished`. Spawned because all three steps shell out to
/// ffmpeg/mkvmerge and must not block the manager loop.
///
/// Called from both the natural-exit path (the poll-interval `finished`
/// list) and the explicit `Stop`/`StopAll` IPC handlers — so a
/// user-stopped Twitch capture goes through the same merge/trim/remux
/// pipeline as one that ended on its own.
fn finalize_completion(
    id: Uuid,
    final_state: RecordingState,
    error: Option<String>,
    rec: Option<ActiveRecording>,
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
) {
    let needs_merge = matches!(
        (final_state, rec.as_ref()),
        (RecordingState::Finished, Some(r)) if r.segments.len() > 1
    );
    let trim_ads = config.recording.auto_trim_ads
        && matches!(final_state, RecordingState::Finished)
        && rec
            .as_ref()
            .map(|r| r.job.platform == PlatformKind::Twitch)
            .unwrap_or(false);
    let ad_min_secs = config.recording.ad_min_secs;
    // Every successful completion gets a container check — yt-dlp's
    // hls-native downloader leaves MPEG-TS bytes inside a .mkv filename,
    // which Chromium-based browsers refuse to play. `normalise_container`
    // is a cheap noop for already-good EBML / MP4.
    let needs_remux = matches!(final_state, RecordingState::Finished) && rec.is_some();

    if !(needs_merge || trim_ads || needs_remux) {
        let _ = event_tx.send(DaemonEvent::RecordingFinished {
            job_id: id,
            final_state,
            error,
            new_path: None,
        });
        return;
    }
    let Some(r) = rec else {
        let _ = event_tx.send(DaemonEvent::RecordingFinished {
            job_id: id,
            final_state,
            error,
            new_path: None,
        });
        return;
    };
    let etx = event_tx;
    let job_id = id;
    let base = r.segments[0].clone();
    let segs = r.segments.clone();
    tokio::spawn(async move {
        let mut warn: Option<String> = None;

        if needs_merge {
            // M5.5: merge gap-resume parts back into the base path via
            // mkvmerge before any further processing.
            let parent = base
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            let stem = base
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("recording")
                .to_string();
            let ext = base
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("mkv")
                .to_string();
            let temp = parent.join(format!(".{stem}.merging.{ext}"));
            let segs_for_merge = segs.clone();
            let temp_for_merge = temp.clone();
            let merged = tokio::task::spawn_blocking(move || {
                segments::merge_segments(&segs_for_merge, &temp_for_merge)
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("merge task join: {e}")));
            match merged {
                Ok(()) => {
                    if let Err(e) = tokio::fs::rename(&temp, &base).await {
                        tracing::warn!(error = %e, "rename merged file failed");
                        let _ = tokio::fs::remove_file(&temp).await;
                        let _ = etx.send(DaemonEvent::RecordingFinished {
                            job_id,
                            final_state: RecordingState::Finished,
                            error: Some(format!("merged segments preserved as {}", temp.display())),
                            new_path: None,
                        });
                        return;
                    }
                    for s in segs.iter().skip(1) {
                        let _ = tokio::fs::remove_file(s).await;
                    }
                    tracing::info!(job_id = %job_id, "merged {} segments", segs.len());
                }
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "merge failed; keeping segments");
                    warn = Some(format!("merge failed: {e}"));
                }
            }
        }

        if trim_ads && warn.is_none() {
            match adtrim::trim_in_place(&base, ad_min_secs).await {
                Ok(adtrim::TrimOutcome::Trimmed {
                    removed_secs,
                    ranges,
                }) => {
                    tracing::info!(
                        job_id = %job_id,
                        ranges,
                        removed_secs = format!("{removed_secs:.1}"),
                        "ad-trim removed black segments"
                    );
                }
                Ok(adtrim::TrimOutcome::NoBlackFound) => {
                    tracing::debug!(job_id = %job_id, "ad-trim: no black segments");
                }
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "ad-trim failed; file untouched");
                }
            }
        }

        // Browser-playable container check runs last so it sees the
        // merged + ad-trimmed bytes. A remux failure is logged but
        // doesn't poison the `RecordingFinished` event — the file is
        // still usable in mpv/VLC, and the user can retry via the
        // `/remux` endpoint.
        if needs_remux && warn.is_none() {
            match remux::normalise_container(&base).await {
                Ok(remux::Outcome::Remuxed { kept_original }) => {
                    if let Some(orig) = kept_original {
                        tracing::info!(
                            job_id = %job_id,
                            kept = %orig.display(),
                            "container remuxed to Matroska (was MPEG-TS); .orig safety copy kept"
                        );
                    } else {
                        tracing::info!(
                            job_id = %job_id,
                            "container remuxed to Matroska (was MPEG-TS); .orig cleaned up"
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(job_id = %job_id, error = %e, "post-record remux failed; file untouched");
                }
            }
        }

        // Last of all, make the name honest. The steps above may have
        // remuxed MPEG-TS into Matroska, but a capture can equally land as
        // MP4 or (for an audio-only pull) MP3 while the filename template
        // still says .mkv. Correcting the extension here is what stops the
        // library drifting away from what is actually on disk; the new path
        // rides back on the event so the journal follows the file.
        let renamed = match container::normalize_extension(&base) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(job_id = %job_id, error = %e, "could not correct container extension; leaving the name as it is");
                None
            }
        };

        let _ = etx.send(DaemonEvent::RecordingFinished {
            job_id,
            final_state: RecordingState::Finished,
            error: warn,
            new_path: renamed,
        });
    });
}

pub async fn run_manager(
    config: AppConfig,
    twitch: Option<std::sync::Arc<tokio::sync::RwLock<crate::platform::twitch::TwitchPlatform>>>,
    mut cmd_rx: mpsc::UnboundedReceiver<RecordingCommand>,
    event_tx: mpsc::UnboundedSender<DaemonEvent>,
    cancel: CancellationToken,
) {
    let mut active: HashMap<Uuid, ActiveRecording> = HashMap::new();
    let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(2));

    // Channels whose Twitch rewind (record-from-start at broadcast t=0) is
    // known sub-gated. Skipping the helix + GQL round-trip for these on every
    // subsequent record turns a recurring WARN log into one INFO line per
    // daemon lifetime. TTL is short enough that an unsub / un-gate naturally
    // re-probes.
    let rewind_forbidden: std::sync::Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    const REWIND_FORBIDDEN_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

    // Channel for spawned resolve tasks to send back results.
    // Tuple layout: (job_id, Result<(process, renamed_path, watch_url), err>)
    //   - renamed_path: YT-5 path rename when title was resolved.
    //   - watch_url: for YouTube --live-from-start jobs, the resolved
    //     `watch?v=…` URL so gap-resume retries can re-spawn YtDlpProcess
    //     instead of falling back to FFmpeg (Task 2 / M5.5 yt-dlp retry).
    // YT-5: the YouTube from-start spawn resolves the broadcast title in
    // the same round-trip it resolves the video id, and rebuilds the
    // filename so a fresh-start auto-record no longer lands as
    // `UCxxxx_2026-…_stream.mkv`. Other call sites pass None.
    let (resolve_tx, mut resolve_rx) = mpsc::unbounded_channel::<(
        Uuid,
        Result<(RecorderProcess, Option<PathBuf>, Option<String>), String>,
    )>();

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    RecordingCommand::Start { channel_id, channel_name, display_name, platform, transcode, cookies_path, stream_title, from_start, job_id: requested_id, thumbnail_url } => {
                        // Check if already recording this channel
                        let already = active.values().any(|r| {
                            r.job.channel_id == channel_id
                                && !matches!(r.job.state, RecordingState::Finished | RecordingState::Failed)
                        });
                        if already {
                            let _ = event_tx.send(DaemonEvent::Error(
                                format!("Already recording {channel_name}")
                            ));
                            continue;
                        }

                        // Enforce max_concurrent_recordings for ALL start paths
                        // (manual IPC, schedule, and auto-record). The monitor
                        // also checks at auto-record trigger time, but manual
                        // starts bypass the monitor entirely.  0 = unlimited.
                        let cap = config.monitor_limits.max_concurrent_recordings;
                        if cap > 0 {
                            let active_count = active.values()
                                .filter(|r| !matches!(r.job.state, RecordingState::Finished | RecordingState::Failed))
                                .count();
                            if active_count >= cap as usize {
                                let _ = event_tx.send(DaemonEvent::Notification {
                                    title: "Recording limit reached".to_string(),
                                    body: format!(
                                        "Cannot start {channel_name}: {cap} concurrent recording cap is active"
                                    ),
                                });
                                continue;
                            }
                        }

                        // YT-1 — human-readable channel slug for the
                        // filename. For YouTube the channel_name is
                        // a UC… ID; display_name is the @handle the
                        // user actually recognises. Falls back when
                        // older callers (schedule fires) don't supply.
                        let filename_channel =
                            display_name.as_deref().unwrap_or(&channel_name);
                        let output_path = build_output_path(&config, filename_channel, platform, stream_title.as_deref());
                        let mut job = RecordingJob::new(
                            channel_id.clone(),
                            channel_name.clone(),
                            platform,
                            output_path.clone(),
                            transcode,
                            stream_title,
                        );
                        if let Some(id) = requested_id {
                            job.id = id;
                        }
                        job.thumbnail_url = thumbnail_url.clone();
                        let job_id = job.id;
                        // Snapshot the source thumbnail to a local cache so it
                        // survives the upstream URL expiring (Twitch live
                        // previews go stale once the stream ends).
                        if let Some(url) = thumbnail_url {
                            let dest = thumbnail_cache_path(&config, job_id);
                            tokio::spawn(async move { snapshot_thumbnail(&url, &dest).await; });
                        }
                        let _ = event_tx.send(DaemonEvent::RecordingStarted { job: job.clone() });

                        active.insert(job_id, ActiveRecording {
                            job,
                            process: None,
                            retry_count: 0,
                            cookies_path: cookies_path.clone(),
                            segments: vec![output_path.clone()],
                            last_bytes: 0,
                            last_growth_at: std::time::Instant::now(),
                            from_start_watch_url: None,
                        });

                        let resolved_format = resolve_format(&config, &channel_id, platform);

                        // YouTube + from_start: use yt-dlp directly (no URL resolution needed)
                        if platform == PlatformKind::YouTube && from_start {
                            let rtx = resolve_tx.clone();
                            // YT-2 — resolve the channel /live alias
                            // to /watch?v=<id> first. yt-dlp's
                            // --live-from-start works most reliably
                            // against a stable video URL; the channel
                            // alias path was observed to land at the
                            // join-time slice instead of t=0.
                            let alias_url = if channel_name.starts_with("UC")
                                && channel_name.len() == 24
                            {
                                format!("https://www.youtube.com/channel/{channel_name}/live")
                            } else {
                                format!("https://www.youtube.com/@{channel_name}/live")
                            };
                            let cookies = cookies_path.clone();
                            let fmt = resolved_format.clone();
                            let log_channel = channel_name.clone();
                            let cfg_clone = config.clone();
                            let filename_channel_owned = filename_channel.to_string();
                            let pre_resolved_title = active
                                .get(&job_id)
                                .and_then(|r| r.job.stream_title.clone());
                            tokio::spawn(async move {
                                // YT-5: fetch id + title in one round-trip.
                                // The title is what makes the filename
                                // human-readable; falling back to "stream"
                                // produces the alphanumeric-only filenames
                                // the user has been seeing.
                                let (watch_url, resolved_title, resolved_uploader) =
                                    match ytdlp::resolve_live_fields(
                                        &alias_url,
                                        cookies.as_deref(),
                                    )
                                    .await
                                {
                                    Ok(fields) => {
                                        let url = format!(
                                            "https://www.youtube.com/watch?v={}",
                                            fields.video_id
                                        );
                                        tracing::info!(
                                            channel = %log_channel,
                                            video_id = %fields.video_id,
                                            title = ?fields.title,
                                            uploader = ?fields.uploader,
                                            "yt-dlp: resolved /live → /watch?v= for live-from-start"
                                        );
                                        (url, fields.title, fields.uploader)
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            channel = %log_channel,
                                            error = %e,
                                            "yt-dlp: live-id resolve failed; falling back to /live alias"
                                        );
                                        (alias_url, None, None)
                                    }
                                };

                                // If the host handed us a UC… channel id
                                // (schedule fires, older saved auto-record
                                // entries, manual starts where display_name
                                // was lost), prefer yt-dlp's resolved
                                // uploader name for the filename's channel
                                // slot. Otherwise keep what the caller said.
                                let filename_channel_final = if ytdlp::looks_like_uc_id(
                                    &filename_channel_owned,
                                ) && resolved_uploader
                                    .as_ref()
                                    .is_some_and(|u| !u.is_empty())
                                {
                                    resolved_uploader.clone().unwrap()
                                } else {
                                    filename_channel_owned.clone()
                                };

                                let title_for_filename = resolved_title
                                    .clone()
                                    .or(pre_resolved_title);
                                let new_output_path = build_output_path(
                                    &cfg_clone,
                                    &filename_channel_final,
                                    PlatformKind::YouTube,
                                    title_for_filename.as_deref(),
                                );
                                let path_changed = new_output_path != output_path;

                                match YtDlpProcess::with_options(
                                    &watch_url,
                                    new_output_path.clone(),
                                    cookies.as_deref(),
                                    Some(&fmt),
                                    true,
                                ) {
                                    Ok(process) => {
                                        let rename = if path_changed {
                                            Some(new_output_path)
                                        } else {
                                            None
                                        };
                                        // Carry the resolved watch URL so
                                        // gap-resume retries can re-spawn
                                        // YtDlpProcess instead of falling
                                        // back to FFmpeg (yt-dlp retry fix).
                                        let _ = rtx.send((
                                            job_id,
                                            Ok((RecorderProcess::YtDlp(process), rename, Some(watch_url))),
                                        ));
                                    }
                                    Err(e) => {
                                        let _ = rtx.send((job_id, Err(format!("yt-dlp failed: {e}"))));
                                    }
                                }
                            });
                        } else {
                            // Normal path: resolve URL then spawn FFmpeg.
                            // For Twitch + from_start, try the rewind path
                            // first (helix → GQL → Usher /vod/v2 — starts
                            // at broadcast t=0); on failure fall back to
                            // streamlink + ffmpeg `-live_start_index` which
                            // lands at the ~5min HLS DVR window start.
                            let rtx = resolve_tx.clone();
                            let etx = event_tx.clone();
                            let fmt = resolved_format.clone();
                            let twitch_handle = twitch.clone();
                            let twitch_live_from_start = config.recording.twitch_live_from_start;
                            let twitch_id = channel_id.clone();
                            let rewind_cache = rewind_forbidden.clone();
                            let quality_tier =
                                resolve_quality_tier(&config, &channel_id, platform);
                            tokio::spawn(async move {
                                // Twitch rewind eligibility: opt-in via config, only
                                // when the user asked for from-start, and only when
                                // we haven't recently confirmed this channel is
                                // sub-gated.
                                let mut rewind_url = if from_start
                                    && platform == PlatformKind::Twitch
                                    && twitch_live_from_start
                                {
                                    twitch_handle.as_ref().map(|tw| {
                                        let tw = tw.clone();
                                        (tw, twitch_id.clone())
                                    })
                                } else {
                                    None
                                };
                                if rewind_url.is_some() {
                                    let mut g = rewind_cache.lock().await;
                                    if let Some(at) = g.get(&twitch_id).copied() {
                                        if at.elapsed() < REWIND_FORBIDDEN_TTL {
                                            tracing::debug!(
                                                channel = %channel_name,
                                                "twitch rewind: skipping — channel cached as sub-gated"
                                            );
                                            rewind_url = None;
                                        } else {
                                            g.remove(&twitch_id);
                                        }
                                    }
                                }

                                let mut resolved_url: Option<String> = None;
                                if let Some((tw, cid)) = rewind_url {
                                    let oauth = match tw.read().await.fresh_access_token().await {
                                        Ok(token) => token,
                                        Err(error) => {
                                            tracing::warn!(
                                                %error,
                                                "could not validate Twitch OAuth before rewind"
                                            );
                                            None
                                        }
                                    };
                                    let r = crate::stream::twitch_rewind::RewindResolver::new(tw, oauth);
                                    match r.resolve(&cid).await {
                                        Ok(s) => {
                                            tracing::info!(
                                                channel = %channel_name,
                                                video_id = %s.video_id,
                                                "twitch rewind: pulling from broadcast t=0"
                                            );
                                            resolved_url = Some(s.master_url);
                                        }
                                        // Sub-gated is steady-state, not a failure:
                                        // we can't rewind without a sub. Cache the
                                        // verdict so subsequent records skip the
                                        // round-trip silently.
                                        Err(crate::stream::twitch_rewind::RewindError::Forbidden) => {
                                            rewind_cache
                                                .lock()
                                                .await
                                                .insert(cid.clone(), std::time::Instant::now());
                                            tracing::info!(
                                                channel = %channel_name,
                                                "twitch rewind: channel is sub-gated; recording from live DVR window instead"
                                            );
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                channel = %channel_name,
                                                error = %e,
                                                "twitch rewind failed; falling back to streamlink + live DVR"
                                            );
                                        }
                                    }
                                }

                                let stream_url = if let Some(u) = resolved_url {
                                    u
                                } else {
                                    match resolver::resolve_stream_url(platform, &channel_name, cookies_path.as_deref(), quality_tier.as_ref()).await {
                                        Ok(info) => info.url,
                                        Err(e) => {
                                            let _ = rtx.send((job_id, Err(format!("Resolve failed: {e}"))));
                                            return;
                                        }
                                    }
                                };

                                let _ = etx.send(DaemonEvent::StreamUrlResolved {
                                    channel_id: channel_id.clone(),
                                    url: stream_url.clone(),
                                });
                                match FfmpegBuilder::new(stream_url, output_path)
                                    .transcode(transcode)
                                    .format(fmt)
                                    .from_start(from_start)
                                    .build()
                                {
                                    Ok(process) => {
                                        let _ = rtx.send((job_id, Ok((RecorderProcess::Ffmpeg(process), None, None))));
                                    }
                                    Err(e) => {
                                        let _ = rtx.send((job_id, Err(format!("FFmpeg failed: {e}"))));
                                    }
                                }
                            });
                        }
                    }
                    RecordingCommand::Stop { job_id } => {
                        if let Some(mut rec) = active.remove(&job_id) {
                            rec.job.state = RecordingState::Stopping;
                            if let Some(ref mut proc) = rec.process {
                                if let Err(e) = proc.stop().await {
                                    tracing::error!("Failed to stop recorder: {e}");
                                }
                            }
                            rec.job.state = RecordingState::Finished;
                            // Same finalize path as a natural exit: any
                            // gap-resume segments get merged, ad-trim runs
                            // if configured, MPEG-TS is remuxed to MKV.
                            finalize_completion(
                                job_id,
                                RecordingState::Finished,
                                None,
                                Some(rec),
                                &config,
                                event_tx.clone(),
                            );
                        }
                    }
                    RecordingCommand::StopAll => {
                        let ids: Vec<Uuid> = active.keys().copied().collect();
                        for id in ids {
                            if let Some(mut rec) = active.remove(&id) {
                                if matches!(rec.job.state, RecordingState::Recording | RecordingState::ResolvingUrl) {
                                    rec.job.state = RecordingState::Stopping;
                                    if let Some(ref mut proc) = rec.process {
                                        proc.stop().await.ok();
                                    }
                                    rec.job.state = RecordingState::Finished;
                                    finalize_completion(
                                        id,
                                        RecordingState::Finished,
                                        None,
                                        Some(rec),
                                        &config,
                                        event_tx.clone(),
                                    );
                                }
                            }
                        }
                        let _ = event_tx.send(DaemonEvent::AllRecordingsStopped);
                    }
                    RecordingCommand::DownloadVod { url, channel_name, platform, output_path, cookies_path, post_title } => {
                        let mut job = RecordingJob::new(
                            String::new(),
                            channel_name,
                            platform,
                            output_path.clone(),
                            false,
                            post_title,
                        );
                        // Stamp the source URL so the webui can mark the
                        // matching VOD pill as Downloaded by exact match
                        // (no FIFO-by-channel heuristic).
                        job.source_url = Some(url.clone());
                        let job_id = job.id;
                        let _ = event_tx.send(DaemonEvent::RecordingStarted { job: job.clone() });

                        active.insert(job_id, ActiveRecording {
                            job,
                            process: None,
                            retry_count: 0,
                            cookies_path: cookies_path.clone(),
                            segments: vec![output_path.clone()],
                            last_bytes: 0,
                            last_growth_at: std::time::Instant::now(),
                            from_start_watch_url: None,
                        });

                        let rtx = resolve_tx.clone();
                        let fmt = resolve_format(&config, "", platform);
                        tokio::spawn(async move {
                            match YtDlpProcess::with_options(&url, output_path, cookies_path.as_deref(), Some(&fmt), false) {
                                Ok(process) => {
                                    let _ = rtx.send((job_id, Ok((RecorderProcess::YtDlp(process), None, None))));
                                }
                                Err(e) => {
                                    let _ = rtx.send((job_id, Err(format!("yt-dlp VOD download failed: {e}"))));
                                }
                            }
                        });
                    }
                }
            }
            Some((job_id, result)) = resolve_rx.recv() => {
                if let Some(rec) = active.get_mut(&job_id) {
                    match result {
                        Ok((process, renamed_path, watch_url)) => {
                            if let Some(new_path) = renamed_path {
                                rec.job.output_path = new_path.clone();
                                // segments[0] is the base path other code derives
                                // resume segments from; keep it in sync so a crash
                                // resume still writes next-to the renamed file.
                                if !rec.segments.is_empty() {
                                    rec.segments[0] = new_path;
                                }
                            }
                            // Remember the watch URL for yt-dlp retries.
                            if watch_url.is_some() {
                                rec.from_start_watch_url = watch_url;
                            }
                            rec.process = Some(process);
                            rec.job.state = RecordingState::Recording;
                            rec.job.started_at = chrono::Utc::now();
                            // Reset stall clock so the resolve phase doesn't
                            // eat into the 2-minute stall budget.
                            rec.last_bytes = 0;
                            rec.last_growth_at = std::time::Instant::now();
                            let _ = event_tx.send(DaemonEvent::RecordingStarted { job: rec.job.clone() });
                        }
                        Err(e) => {
                            rec.job.state = RecordingState::Failed;
                            rec.job.error = Some(e.clone());
                            let _ = event_tx.send(DaemonEvent::RecordingFinished { job_id, final_state: RecordingState::Failed, error: Some(e.clone()), new_path: None });
                            let _ = event_tx.send(DaemonEvent::Error(e));
                        }
                    }
                }
            }
            _ = cancel.cancelled() => {
                tracing::info!("Recording manager shutting down, stopping all recordings");
                let ids: Vec<Uuid> = active.keys().copied().collect();
                for id in ids {
                    if let Some(mut rec) = active.remove(&id) {
                        if matches!(rec.job.state, RecordingState::Recording | RecordingState::ResolvingUrl) {
                            if let Some(ref mut proc) = rec.process {
                                proc.stop().await.ok();
                            }
                        }
                    }
                }
                break;
            }
            _ = poll_interval.tick() => {
                let mut finished = Vec::new();
                for (id, rec) in active.iter_mut() {
                    if rec.job.state != RecordingState::Recording {
                        continue;
                    }
                    if let Some(ref mut proc) = rec.process {
                        rec.job.bytes_written = proc.file_size();
                        rec.job.duration_secs = (chrono::Utc::now() - rec.job.started_at)
                            .num_seconds() as f64;
                        // Stall detection: advance the growth clock whenever
                        // new bytes arrive so the threshold starts from the
                        // last genuine write activity.
                        if rec.job.bytes_written > rec.last_bytes {
                            rec.last_bytes = rec.job.bytes_written;
                            rec.last_growth_at = std::time::Instant::now();
                        }
                        let dp = proc.download_progress();

                        let _ = event_tx.send(DaemonEvent::RecordingProgress {
                            job_id: *id,
                            bytes_written: rec.job.bytes_written,
                            duration_secs: rec.job.duration_secs,
                            download_pct: dp.pct,
                            download_eta_secs: dp.eta_secs,
                            download_rate_bps: dp.rate_bps,
                        });

                        match proc.try_wait() {
                            Ok(Some(status)) => {
                                if status.success() {
                                    rec.job.state = RecordingState::Finished;
                                    finished.push((*id, RecordingState::Finished, None));
                                } else if rec.retry_count < 3 {
                                    // M5.5 gap-resume: keep the prior segment on
                                    // disk and write the next chunk to
                                    // `<base>_partN.mkv`. After Finished the
                                    // segments merge back into the base file.
                                    rec.retry_count += 1;
                                    let wait_secs = 2u64.pow(rec.retry_count);
                                    tracing::warn!(
                                        "Recorder exited with {status}, resume segment {}/3 in {wait_secs}s for {}",
                                        rec.retry_count,
                                        rec.job.channel_name
                                    );
                                    rec.job.state = RecordingState::ResolvingUrl;
                                    rec.process = None;

                                    // Segment N path derives from the original
                                    // base (segments[0]).
                                    let base = rec.segments[0].clone();
                                    let segment_path = segments::segment_path(&base, rec.retry_count + 1);
                                    rec.segments.push(segment_path.clone());
                                    rec.job.output_path = segment_path;

                                    // Re-resolve and restart.
                                    // For YouTube --live-from-start jobs the
                                    // original process was YtDlp; re-spawn it
                                    // with the same watch URL instead of
                                    // resolving via streamlink + FFmpeg.
                                    let rtx = resolve_tx.clone();
                                    let job = rec.job.clone();
                                    let jid = *id;
                                    let retry_cookies = rec.cookies_path.clone();
                                    let retry_fmt = resolve_format(&config, &job.channel_id, job.platform);
                                    let retry_quality_tier =
                                        resolve_quality_tier(&config, &job.channel_id, job.platform);
                                    let maybe_watch_url = rec.from_start_watch_url.clone();
                                    tokio::spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                                        if let Some(watch_url) = maybe_watch_url {
                                            // yt-dlp from-start retry: re-spawn
                                            // with the same resolved watch URL.
                                            match YtDlpProcess::with_options(
                                                &watch_url,
                                                job.output_path,
                                                retry_cookies.as_deref(),
                                                Some(&retry_fmt),
                                                true,
                                            ) {
                                                Ok(p) => { let _ = rtx.send((jid, Ok((RecorderProcess::YtDlp(p), None, Some(watch_url))))); }
                                                Err(e) => { let _ = rtx.send((jid, Err(format!("{e}")))); }
                                            }
                                        } else {
                                            match resolver::resolve_stream_url(
                                                job.platform,
                                                &job.channel_name,
                                                retry_cookies.as_deref(),
                                                retry_quality_tier.as_ref(),
                                            )
                                            .await
                                            {
                                                Ok(info) => {
                                                    match FfmpegBuilder::new(info.url, job.output_path)
                                                        .transcode(job.transcode)
                                                        .format(retry_fmt)
                                                        .build()
                                                    {
                                                        Ok(p) => { let _ = rtx.send((jid, Ok((RecorderProcess::Ffmpeg(p), None, None)))); }
                                                        Err(e) => { let _ = rtx.send((jid, Err(format!("{e}")))); }
                                                    }
                                                }
                                                Err(e) => {
                                                    let _ = rtx.send((jid, Err(format!("{e}"))));
                                                }
                                            }
                                        }
                                    });
                                } else {
                                    let stderr_tail = proc.stderr_tail();
                                    let stderr_excerpt = if stderr_tail.is_empty() {
                                        String::new()
                                    } else {
                                        // Keep the message short for the UI; full
                                        // tail goes to tracing.
                                        let last = stderr_tail
                                            .lines()
                                            .rev()
                                            .find(|l| !l.trim().is_empty())
                                            .unwrap_or("");
                                        format!(" — {}", last)
                                    };
                                    tracing::error!(
                                        job_id = %id,
                                        channel = %rec.job.channel_name,
                                        retries = rec.retry_count,
                                        status = %status,
                                        "Recorder failed; stderr tail:\n{stderr_tail}"
                                    );
                                    let error_msg = format!(
                                        "Recorder exited: {status} after {} retries{stderr_excerpt}",
                                        rec.retry_count
                                    );
                                    rec.job.state = RecordingState::Failed;
                                    rec.job.error = Some(error_msg.clone());
                                    finished.push((*id, RecordingState::Failed, Some(error_msg)));
                                }
                            }
                            Ok(None) => {
                                // Process is still alive — check for a stall.
                                // A stall means no new bytes for STALL_TIMEOUT_SECS;
                                // a frozen ffmpeg/yt-dlp would otherwise stay
                                // `Recording` forever.  Stopping here lets the
                                // gap-resume retry path kick in (or finalize if
                                // retries are exhausted).
                                let stalled = rec.last_growth_at.elapsed().as_secs()
                                    >= STALL_TIMEOUT_SECS;
                                if stalled {
                                    tracing::warn!(
                                        job_id = %id,
                                        channel = %rec.job.channel_name,
                                        bytes = rec.job.bytes_written,
                                        stall_secs = STALL_TIMEOUT_SECS,
                                        "recorder stall detected — no new bytes; stopping for retry/finalize"
                                    );
                                    let _ = event_tx.send(DaemonEvent::Notification {
                                        title: format!("Recording stalled: {}", rec.job.channel_name),
                                        body: format!(
                                            "No new data for {STALL_TIMEOUT_SECS}s — stopping to retry"
                                        ),
                                    });
                                    // Stop the process; the non-zero exit will
                                    // trigger the gap-resume retry path (or
                                    // finalize as Failed after max retries).
                                    if let Some(ref mut p) = rec.process {
                                        p.stop().await.ok();
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to check recorder status: {e}");
                            }
                        }
                    }
                }
                for (id, final_state, error) in finished {
                    let rec = active.remove(&id);
                    finalize_completion(id, final_state, error, rec, &config, event_tx.clone());
                }
            }
        }
    }
}

/// Local cache path for a recording's source thumbnail (item: recording
/// cover art). Keyed by job id under `data_dir/thumbs/`.
pub fn thumbnail_cache_path(_config: &AppConfig, job_id: uuid::Uuid) -> PathBuf {
    AppConfig::data_dir()
        .join("thumbs")
        .join(format!("{job_id}.jpg"))
}

/// Download the source thumbnail to `dest` (best-effort). Snapshotting a local
/// copy means the webui cover art survives the upstream URL expiring.
async fn snapshot_thumbnail(url: &str, dest: &std::path::Path) {
    if let Some(parent) = dest.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match reqwest::get(url).await {
        Ok(resp) if resp.status().is_success() => match resp.bytes().await {
            Ok(bytes) => {
                if let Err(e) = tokio::fs::write(dest, &bytes).await {
                    tracing::warn!("thumbnail cache write failed: {e}");
                }
            }
            Err(e) => tracing::warn!("thumbnail download failed: {e}"),
        },
        Ok(resp) => tracing::warn!("thumbnail fetch {}: {}", url, resp.status()),
        Err(e) => tracing::warn!("thumbnail fetch failed: {e}"),
    }
}

pub fn build_output_path(
    config: &AppConfig,
    channel_name: &str,
    platform: PlatformKind,
    stream_title: Option<&str>,
) -> PathBuf {
    let now = chrono::Local::now();
    let date = now.format("%Y-%m-%d_%H%M%S");
    let platform_str = match platform {
        PlatformKind::Twitch => "twitch",
        PlatformKind::YouTube => "youtube",
        PlatformKind::Patreon => "patreon",
    };

    // Sanitize stream title for filesystem safety
    let title = stream_title.unwrap_or("stream");
    let safe_title: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    let safe_title = if safe_title.is_empty() {
        "stream".to_string()
    } else {
        safe_title
    };
    // Truncate to avoid excessively long filenames
    let safe_title: String = safe_title.chars().take(80).collect();

    let filename = config
        .recording
        .filename_template
        .replace("{channel}", channel_name)
        .replace("{date}", &date.to_string())
        .replace("{title}", &safe_title)
        .replace("{platform}", platform_str);

    disambiguate_path(config.recording_dir.join(filename))
}

/// Compute the per-episode output directory for catalog-pull and structured recordings.
///
/// Layout: `{root}/{platform}/{channel}/{YYYY-MM-DD}_{title}/`
///
/// Both `channel` and `title` are filesystem-sanitized. The result is *not*
/// disambiguated — a re-run that lands on the same date+title will reuse the
/// directory; the catalog index in §5 is what guarantees we don't re-download.
pub fn episode_dir(
    root: &std::path::Path,
    platform: PlatformKind,
    channel: &str,
    date: chrono::DateTime<chrono::Utc>,
    title: &str,
) -> PathBuf {
    let platform_str = match platform {
        PlatformKind::Twitch => "twitch",
        PlatformKind::YouTube => "youtube",
        PlatformKind::Patreon => "patreon",
    };
    let date_str = date.format("%Y-%m-%d").to_string();
    let leaf = format!("{date_str}_{}", sanitize_path_component(title));
    root.join(platform_str)
        .join(sanitize_path_component(channel))
        .join(leaf)
}

/// Strip filesystem-hostile characters and clamp length so deeply-nested paths
/// don't exceed PATH_MAX on any platform.
pub fn sanitize_path_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let truncated: String = trimmed.chars().take(80).collect();
    if truncated.is_empty() {
        "untitled".to_string()
    } else {
        truncated
    }
}

/// Per-episode metadata sidecar. Written next to `video.mkv` after a catalog-pull
/// recording finishes so downstream tools (Crunchr, archiver, etc.) have provenance
/// without parsing filenames.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EpisodeMetadata {
    pub platform: String,
    pub channel_id: String,
    pub channel_name: String,
    pub vod_id: String,
    pub title: String,
    pub source_url: String,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub recorded_at: chrono::DateTime<chrono::Utc>,
    pub duration_secs: Option<f64>,
    pub format: String,
    pub container: String,
    pub video_codec: String,
    pub audio_codec: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Serialize EpisodeMetadata to `{episode_dir}/metadata.json`, creating the dir
/// if needed. Best-effort: errors are returned but never panic.
pub fn write_metadata_json(episode_dir: &std::path::Path, meta: &EpisodeMetadata) -> Result<()> {
    std::fs::create_dir_all(episode_dir)?;
    let path = episode_dir.join("metadata.json");
    let json = serde_json::to_string_pretty(meta)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// If `path` already exists, return `stem_1.ext`, `stem_2.ext`, ... until a
/// free slot is found. Guards against two concurrent recordings that resolve
/// to the same template-rendered filename silently stomping each other.
fn disambiguate_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let parent = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    for n in 1u32.. {
        let candidate_name = match &ext {
            Some(e) => format!("{stem}_{n}.{e}"),
            None => format!("{stem}_{n}"),
        };
        let candidate = parent.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RecordingFormat;

    #[test]
    fn format_resolution_precedence() {
        let global = RecordingFormat {
            format: Some("bestvideo+bestaudio".into()),
            container: Some("mp4".into()),
            ..Default::default()
        };
        let channel = RecordingFormat {
            format: Some("worst".into()),
            ..Default::default()
        };
        let r = RecordingFormat::resolved(Some(&channel), &global);
        assert_eq!(r.format, "worst", "channel wins on format");
        assert_eq!(r.container, "mp4", "global fills missing container");
        assert_eq!(r.video_codec, "copy", "built-in default copy");
        assert_eq!(r.audio_codec, "copy");
    }

    #[test]
    fn format_resolution_uses_builtin_default_when_empty() {
        let r = RecordingFormat::resolved(None, &RecordingFormat::default());
        assert_eq!(r.format, "best");
        assert_eq!(r.container, "mkv");
        assert_eq!(r.video_codec, "copy");
        assert_eq!(r.audio_codec, "copy");
    }

    #[test]
    fn episode_dir_layout() {
        let root = std::path::PathBuf::from("/tmp/strivo");
        let date = chrono::DateTime::parse_from_rfc3339("2026-04-12T15:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let dir = episode_dir(
            &root,
            PlatformKind::Patreon,
            "Some Creator",
            date,
            "Episode 1: Hello!",
        );
        assert_eq!(
            dir,
            std::path::PathBuf::from(
                "/tmp/strivo/patreon/Some Creator/2026-04-12_Episode 1_ Hello_"
            )
        );
    }

    #[test]
    fn sanitize_clamps_and_strips() {
        assert_eq!(sanitize_path_component(""), "untitled");
        assert_eq!(sanitize_path_component("...."), "untitled");
        assert_eq!(sanitize_path_component("a/b\\c:d"), "a_b_c_d");
        let long = "x".repeat(200);
        assert_eq!(sanitize_path_component(&long).len(), 80);
    }

    // ── Quality-tier resolution ──────────────────────────────────────
    use crate::config::{AppConfig, AutoRecordEntry, CaptureProfile, QualityTier};

    fn make_arc_with_profile(channel_id: &str, profile: Option<&str>) -> AutoRecordEntry {
        AutoRecordEntry {
            platform: "Twitch".into(),
            channel_id: channel_id.into(),
            channel_name: channel_id.into(),
            format: None,
            profile: profile.map(String::from),
        }
    }

    fn make_profile_with_tier(name: &str, tier: QualityTier) -> CaptureProfile {
        CaptureProfile {
            name: name.into(),
            quality_tier: Some(tier),
            format: None,
            transcode: None,
            audio_only: false,
            transcript: false,
            cutoff_episodes: None,
        }
    }

    #[test]
    fn quality_tier_1080p_resolves_correctly() {
        let mut cfg = AppConfig::default();
        cfg.capture_profiles = vec![make_profile_with_tier("hd", QualityTier::P1080)];
        cfg.auto_record_channels = vec![make_arc_with_profile("streamer1", Some("hd"))];
        let r = resolve_format(&cfg, "streamer1", PlatformKind::Twitch);
        assert_eq!(
            r.format,
            "bestvideo[height<=1080]+bestaudio/best[height<=1080]"
        );
    }

    #[test]
    fn quality_tier_audio_only() {
        let mut cfg = AppConfig::default();
        cfg.capture_profiles = vec![make_profile_with_tier("ao", QualityTier::AudioOnly)];
        cfg.auto_record_channels = vec![make_arc_with_profile("streamer2", Some("ao"))];
        let r = resolve_format(&cfg, "streamer2", PlatformKind::Twitch);
        assert_eq!(r.format, "bestaudio/best");
    }

    #[test]
    fn channel_explicit_format_wins_over_tier() {
        let mut cfg = AppConfig::default();
        cfg.capture_profiles = vec![make_profile_with_tier("hd", QualityTier::P720)];
        cfg.auto_record_channels = vec![AutoRecordEntry {
            platform: "Twitch".into(),
            channel_id: "streamer3".into(),
            channel_name: "streamer3".into(),
            format: Some(RecordingFormat {
                format: Some("worst".into()),
                ..Default::default()
            }),
            profile: Some("hd".into()),
        }];
        let r = resolve_format(&cfg, "streamer3", PlatformKind::Twitch);
        assert_eq!(
            r.format, "worst",
            "per-channel explicit format must win over tier"
        );
    }

    #[test]
    fn no_profile_falls_back_to_global() {
        let mut cfg = AppConfig::default();
        cfg.recording.format.format = Some("bestvideo+bestaudio".into());
        cfg.auto_record_channels = vec![make_arc_with_profile("streamer4", None)];
        let r = resolve_format(&cfg, "streamer4", PlatformKind::Twitch);
        assert_eq!(r.format, "bestvideo+bestaudio");
    }

    #[test]
    fn unknown_channel_falls_back_to_global() {
        let mut cfg = AppConfig::default();
        cfg.recording.format.format = Some("global_selector".into());
        let r = resolve_format(&cfg, "not_in_list", PlatformKind::Twitch);
        assert_eq!(r.format, "global_selector");
    }
}
