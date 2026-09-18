use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::ResolvedFormat;

const STDERR_TAIL_LINES: usize = 40;

/// Last-known yt-dlp download progress for VOD pulls. Populated by parsing
/// `[download]  XX.X% of NNN at RR ETA TT` lines on stdout. All fields are
/// optional because yt-dlp reports `Unknown` for rate + ETA early on.
///
/// `pct` is an *overall* 0..100 value across every format yt-dlp downloads
/// for this job (see [`ProgressAggregator`]) — not the raw per-line percent,
/// which regresses every time yt-dlp starts a new format.
#[derive(Debug, Clone, Default)]
pub struct DownloadProgress {
    pub pct: Option<f32>,
    pub eta_secs: Option<u32>,
    pub rate_bps: Option<u64>,
    pub bytes_total: Option<u64>,
    /// Which stage of a VOD pull this progress reflects. `None` for
    /// ffmpeg-driven live captures and before the first yt-dlp line parses.
    pub phase: Option<DownloadPhase>,
}

/// Stage of a yt-dlp VOD pull. `Merging` covers the `[Merger]`/
/// `[VideoRemuxer]`/`[FixupM3u8]` post-processing step yt-dlp runs after all
/// formats have downloaded — there is no further `[download]` percent during
/// that window, so the aggregator freezes `pct` rather than reporting it as
/// stalled or regressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadPhase {
    #[default]
    Downloading,
    Merging,
}

impl DownloadPhase {
    /// Wire value for `DaemonEvent::RecordingProgress::download_phase`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Merging => "merging",
        }
    }
}

/// Parse "1.23GiB" / "456.7MiB" / "789KiB" / "12B" → bytes. yt-dlp uses
/// binary units (KiB/MiB/GiB/TiB).
fn parse_size_bytes(tok: &str) -> Option<u64> {
    let tok = tok.trim_start_matches('~');
    let (num_part, unit) = tok
        .find(|c: char| c.is_ascii_alphabetic())
        .map(|i| tok.split_at(i))?;
    let n = num_part.parse::<f64>().ok()?;
    let mult: f64 = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0_f64.powi(3),
        "TiB" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// "5.50MiB/s" → bytes per second.
fn parse_rate_bps(tok: &str) -> Option<u64> {
    parse_size_bytes(tok.strip_suffix("/s")?)
}

/// "00:42" → 42, "01:23:45" → 5025.
fn parse_eta_secs(tok: &str) -> Option<u32> {
    let parts: Vec<u32> = tok
        .split(':')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    Some(match parts.as_slice() {
        [s] => *s,
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    })
}

/// Parse one yt-dlp stdout line. Returns Some when it's a recognisable
/// `[download]` progress line — `Unknown` rate/ETA become `None` rather than
/// failing the parse, because they're the steady-state at the very start.
pub(crate) fn parse_download_line(line: &str) -> Option<DownloadProgress> {
    let body = line.trim_start().strip_prefix("[download]")?.trim_start();
    let mut toks = body.split_whitespace();
    let pct = toks.next()?.strip_suffix('%')?.parse::<f32>().ok()?;
    if toks.next()? != "of" {
        return None;
    }
    // Fragmented downloads (HLS/DASH — Patreon posts, YouTube live) render
    // the total as an estimate, and yt-dlp pads the tilde into its OWN
    // token: `of ~ 324.23MiB`. Reading that as the size left the next token
    // where "at" was expected, so the whole line failed to parse and every
    // fragmented download reported no progress at all.
    let mut size_tok = toks.next()?;
    if size_tok == "~" {
        size_tok = toks.next()?;
    }
    let bytes_total = parse_size_bytes(size_tok.trim_start_matches('~'));
    if toks.next()? != "at" {
        return None;
    }
    // Rate is either "<num><unit>/s" or "Unknown B/s".
    let rate_tok = toks.next()?;
    let rate_bps = if rate_tok == "Unknown" {
        // Consume "B/s" so the ETA tokens line up.
        let _ = toks.next();
        None
    } else {
        parse_rate_bps(rate_tok)
    };
    if toks.next()? != "ETA" {
        return None;
    }
    let eta_tok = toks.next()?;
    let eta_secs = if eta_tok == "Unknown" {
        None
    } else {
        parse_eta_secs(eta_tok)
    };
    Some(DownloadProgress {
        pct: Some(pct),
        eta_secs,
        rate_bps,
        bytes_total,
        phase: None,
    })
}

/// Parse a `[download] Destination: <path>` line. yt-dlp emits one of these
/// each time it starts writing a new file — once per format for a
/// multi-format pull (`bv*+ba/b`), plus once more for the merged output on
/// single-format/pre-merged selections.
fn parse_destination_line(line: &str) -> Option<PathBuf> {
    let body = line
        .trim_start()
        .strip_prefix("[download] Destination:")?
        .trim();
    if body.is_empty() {
        None
    } else {
        Some(PathBuf::from(body))
    }
}

/// Parse the format count out of yt-dlp's
/// `[info] <id>: Downloading N format(s): 399+251-12` line. The count comes
/// from splitting the id spec after the colon on `+`, not the `N` yt-dlp
/// prints (that number is a labeling quirk, not reliably the format count).
fn format_count_from_info_line(line: &str) -> Option<usize> {
    let after = line.split("Downloading ").nth(1)?;
    let spec = after.split("format(s):").nth(1)?.trim();
    if spec.is_empty() {
        None
    } else {
        Some(spec.split('+').count())
    }
}

/// True for yt-dlp's post-processing lines that follow all formats
/// finishing — no further `[download]` percent lines occur during this
/// window, so the caller should hold `pct` rather than read it as stalled.
fn is_merge_phase_line(line: &str) -> bool {
    let l = line.trim_start();
    l.starts_with("[Merger]") || l.starts_with("[VideoRemuxer]") || l.starts_with("[FixupM3u8]")
}

/// Turns yt-dlp's per-format, regressing `[download]` percent lines into one
/// overall, monotonically-increasing 0..100 progress for the whole job.
///
/// yt-dlp's default VOD selector (`bv*+ba/b`) downloads video then audio
/// sequentially, each restarting its own percent from 0, then merges. A
/// naive pass-through of the raw line (the pre-fix behaviour) regresses
/// (98% → 15.8%) when the second format starts and goes silent during the
/// merge — this aggregator weights each format by its `bytes_total` once
/// known (falling back to an equal share among formats whose size isn't
/// known yet) and never reports a lower overall percent than it already
/// has.
///
/// Pure and side-effect free so the state machine is unit-testable without
/// spawning yt-dlp; `YtDlpProcess`'s stdout reader task owns the only
/// mutable instance.
#[derive(Debug, Clone)]
pub(crate) struct ProgressAggregator {
    total_formats: usize,
    format_index: usize,
    format_totals: Vec<Option<u64>>,
    seen_first_destination: bool,
    current_pct: f32,
    monotonic_pct: f32,
    phase: DownloadPhase,
    last_eta: Option<u32>,
    last_rate: Option<u64>,
}

impl Default for ProgressAggregator {
    fn default() -> Self {
        Self {
            total_formats: 1,
            format_index: 0,
            format_totals: vec![None],
            seen_first_destination: false,
            current_pct: 0.0,
            monotonic_pct: 0.0,
            phase: DownloadPhase::Downloading,
            last_eta: None,
            last_rate: None,
        }
    }
}

impl ProgressAggregator {
    /// Feed one line of yt-dlp stdout.
    pub(crate) fn observe(&mut self, line: &str) {
        if let Some(n) = format_count_from_info_line(line) {
            self.total_formats = n.max(1);
            self.format_totals = vec![None; self.total_formats];
            return;
        }
        if parse_destination_line(line).is_some() {
            if self.seen_first_destination {
                self.format_index =
                    (self.format_index + 1).min(self.total_formats.saturating_sub(1));
                self.current_pct = 0.0;
            } else {
                self.seen_first_destination = true;
            }
            return;
        }
        if is_merge_phase_line(line) {
            self.phase = DownloadPhase::Merging;
            return;
        }
        if let Some(p) = parse_download_line(line) {
            self.phase = DownloadPhase::Downloading;
            self.current_pct = p.pct.unwrap_or(self.current_pct);
            self.last_eta = p.eta_secs;
            self.last_rate = p.rate_bps;
            if let Some(bt) = p.bytes_total {
                if self.format_totals.len() <= self.format_index {
                    self.format_totals.resize(self.format_index + 1, None);
                }
                self.format_totals[self.format_index] = Some(bt);
            }
        }
    }

    /// Weighted overall percent across every format, using known
    /// `bytes_total`s where available and the average of the known ones as
    /// a stand-in for formats not yet started.
    fn overall_pct(&self) -> f32 {
        if self.total_formats <= 1 {
            return self.current_pct;
        }
        let known: Vec<u64> = self.format_totals.iter().filter_map(|t| *t).collect();
        let avg = if known.is_empty() {
            1.0
        } else {
            known.iter().sum::<u64>() as f64 / known.len() as f64
        };
        let mut total_weight = 0.0f64;
        let mut done_weight = 0.0f64;
        for i in 0..self.total_formats {
            let weight = self
                .format_totals
                .get(i)
                .copied()
                .flatten()
                .map(|b| b as f64)
                .unwrap_or(avg);
            total_weight += weight;
            match i.cmp(&self.format_index) {
                std::cmp::Ordering::Less => done_weight += weight,
                std::cmp::Ordering::Equal => {
                    done_weight += weight * (self.current_pct as f64 / 100.0)
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        if total_weight <= 0.0 {
            0.0
        } else {
            ((done_weight / total_weight) * 100.0) as f32
        }
    }

    /// Current overall progress. Monotonic: never reports a lower `pct` than
    /// a previous call. Always capped below 100 — literal 100% is reserved
    /// for the actual `RecordingFinished` event, never this in-progress
    /// snapshot, which is what keeps the merge phase (no percent of its
    /// own) from reading as "done" before the job actually is. Capping
    /// unconditionally (not just once merging starts) is what keeps this
    /// monotonic: a value already delivered pre-merge can never need to be
    /// walked back down once the merge phase begins.
    pub(crate) fn snapshot(&mut self) -> DownloadProgress {
        let raw = self.overall_pct().clamp(0.0, 100.0);
        self.monotonic_pct = self.monotonic_pct.max(raw).min(99.9);
        DownloadProgress {
            pct: Some(self.monotonic_pct),
            eta_secs: if self.phase == DownloadPhase::Merging {
                None
            } else {
                self.last_eta
            },
            rate_bps: if self.phase == DownloadPhase::Merging {
                None
            } else {
                self.last_rate
            },
            bytes_total: self.format_totals.get(self.format_index).copied().flatten(),
            phase: Some(self.phase),
        }
    }
}

/// YT-2 — resolve a YouTube `/live` channel URL to the underlying
/// `/watch?v=<id>` URL of the active broadcast.
///
/// Why: `yt-dlp --live-from-start` against `/channel/UC.../live` or
/// `/@handle/live` works only when yt-dlp's extractor follows the
/// redirect cleanly. In practice we've observed the live stream
/// starting at the join-time slice when the URL form is the channel
/// alias — the extractor races the redirect and falls back to the
/// stream's live-edge cursor. Resolving to `/watch?v=<id>` first gives
/// `--live-from-start` a stable video URL it can replay against.
///
/// Implementation: shell out to `yt-dlp --print id --no-warnings
/// --no-download --no-playlist <url>` with a short timeout. Returns
/// the resolved video ID; caller composes the watch URL.
pub async fn resolve_live_video_id(
    channel_live_url: &str,
    cookies_path: Option<&std::path::Path>,
) -> Result<String> {
    Ok(resolve_live_fields(channel_live_url, cookies_path)
        .await?
        .video_id)
}

#[derive(Debug, Clone)]
pub struct LiveFields {
    pub video_id: String,
    pub title: Option<String>,
    /// The broadcaster's display name as YouTube knows it
    /// (`%(uploader)s` — what shows under the video player). Falls back
    /// to `%(channel)s` if uploader is missing. Lets the host build a
    /// human-readable filename even when the schedule fired with only a
    /// `UC…` channel id on hand.
    pub uploader: Option<String>,
}

/// One round-trip that returns both the video id and the broadcast title.
/// Used so the host can build a semantic filename (`{channel}_{date}_{title}.mkv`)
/// before yt-dlp ever opens the manifest — previously the host fell back to
/// "stream" when the monitor hadn't polled the channel yet.
pub async fn resolve_live_fields(
    channel_live_url: &str,
    cookies_path: Option<&std::path::Path>,
) -> Result<LiveFields> {
    let mut cmd = Command::new(crate::tools::resolve_tool("yt-dlp"));
    cmd.args([
        "--print",
        "%(id)s\t%(title)s\t%(uploader,channel)s",
        "--no-warnings",
        "--no-download",
        "--no-playlist",
        "--socket-timeout",
        "20",
    ]);
    if let Some(cookies) = cookies_path {
        cmd.args(["--cookies", &cookies.to_string_lossy()]);
    }
    cmd.arg(channel_live_url);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = tokio::time::timeout(std::time::Duration::from_secs(30), async move {
        cmd.output().await
    })
    .await
    .context("yt-dlp --print id timed out after 30 s")?
    .context("yt-dlp --print id failed to spawn")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "yt-dlp --print id exit {}: {}",
            output.status,
            stderr
                .lines()
                .last()
                .unwrap_or("(no stderr)")
                .chars()
                .take(200)
                .collect::<String>()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_print_line(&stdout)
}

/// Pure parser for the `--print '%(id)s\t%(title)s\t%(uploader,channel)s'`
/// output. Split out from `resolve_live_fields` so the regex-free string
/// handling can be unit-tested without invoking yt-dlp.
fn parse_print_line(stdout: &str) -> Result<LiveFields> {
    let line = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("yt-dlp --print returned empty output"))?;
    let mut parts = line.splitn(3, '\t');
    let video_id = parts
        .next()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| anyhow::anyhow!("yt-dlp --print missing id"))?;
    let title = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "NA");
    let uploader = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "NA");

    if video_id.len() != 11
        || !video_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        anyhow::bail!("yt-dlp --print returned unexpected id shape: {video_id:?}");
    }
    Ok(LiveFields {
        video_id,
        title,
        uploader,
    })
}

/// YT-5 guard: should the host substitute yt-dlp's uploader for the
/// filename's channel slot? True when the host-supplied name is empty
/// or a bare `UC…` YouTube channel id (24 chars, base64). Live broadcasts
/// from those callers (schedule fires, older saved auto-records) would
/// otherwise land as `UCxxxxxxxxxxxxxxxxxxxxxxxx_<date>_<title>.mkv`.
pub fn looks_like_uc_id(name: &str) -> bool {
    if name.is_empty() {
        return true;
    }
    name.len() == 24
        && name.starts_with("UC")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

pub struct YtDlpProcess {
    child: Child,
    pub output_path: PathBuf,
    stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// Latest parsed `[download]` line. Updated continuously by the stdout
    /// reader task spawned in `with_options`; read each poll tick by the
    /// recording manager to feed `RecordingProgress` events.
    progress: Arc<Mutex<DownloadProgress>>,
    /// Every `[download] Destination: <path>` yt-dlp has printed so far —
    /// one per format for a multi-format pull. `file_size()` sums whichever
    /// of these still exist on disk while the final merged `output_path`
    /// hasn't been created yet, so `bytes_written` reflects real progress
    /// instead of reading 0 for the whole download+merge window.
    destinations: Arc<Mutex<Vec<PathBuf>>>,
}

impl YtDlpProcess {
    pub fn new(
        url: &str,
        output_path: PathBuf,
        cookies_path: Option<&std::path::Path>,
    ) -> Result<Self> {
        Self::with_options(url, output_path, cookies_path, None, true)
    }

    /// Spawn yt-dlp with an explicit format selector and optional `--live-from-start`.
    /// `format` of `None` means use built-in default `"best"`.
    pub fn with_options(
        url: &str,
        output_path: PathBuf,
        cookies_path: Option<&std::path::Path>,
        format: Option<&ResolvedFormat>,
        live_from_start: bool,
    ) -> Result<Self> {
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut cmd = Command::new(crate::tools::resolve_tool("yt-dlp"));
        if live_from_start {
            cmd.arg("--live-from-start");
            // YT-3 — grace period when the stream is just coming
            // online. Default is fail-fast, which loses the first
            // 30 s of many recordings to user reaction time.
            cmd.args(["--wait-for-video", "60"]);
            // YT-4 (corrected 2026-05-23): an earlier iteration
            // forced `protocol=m3u8_native` thinking live-from-start
            // was HLS-only. The reverse is true on YouTube — when
            // `--live-from-start` is set, yt-dlp surfaces *DASH*
            // formats (`dashG`), because DASH is what supports the
            // back-replay from t=0. Forcing HLS-native therefore
            // made the selector match nothing and recording failed
            // outright. Validated empirically: with `-f best` plus
            // `--live-from-start`, a 60s wall-clock pull of LofiGirl
            // produced 2355s of video (frag 470/26401), proving the
            // rewind path actually engages.
        }
        cmd.arg("--continue");
        cmd.args(["--no-part"]);

        // YT-4b: when `--live-from-start` is set, yt-dlp surfaces only
        // DASH-split formats (video-only + audio-only — there is no
        // pre-merged "best"). `-f best` works for streams that happen
        // to expose a combined fallback (LofiGirl) but fails outright
        // for those that don't (Sky News: "Requested format is not
        // available"). yt-dlp's own no-`-f` default is `bv*+ba/b`,
        // which picks the best video + best audio and merges, then
        // falls back to any pre-merged variant. Use that explicitly
        // when live_from_start is on. Validated 2026-05-23 against
        // LofiGirl, Sky News, and NASA — all produced multi-minute
        // rewinds (LofiGirl: 60s wall → 2355s pulled; Sky: 60s →
        // 1200s).
        let default_format = if live_from_start { "bv*+ba/b" } else { "best" };
        let format_str = format.map(|f| f.format.as_str()).unwrap_or(default_format);
        cmd.args(["-f", format_str]);
        if let Some(container) = format.map(|f| match f.container.to_ascii_lowercase().as_str() {
            "mp4" => "mp4",
            "webm" => "webm",
            "mkv" => "mkv",
            _ => "mkv",
        }) {
            // Force yt-dlp to publish one merged file in the configured
            // container; otherwise DASH selection can leave separate video
            // and audio outputs beside the advertised target.
            cmd.args([
                "--merge-output-format",
                container,
                "--remux-video",
                container,
                // Do not leave the video-only/audio-only intermediates beside
                // the final output.  A logical VOD must result in one file.
                "--no-keep-video",
            ]);
        }

        // Bitrate hint for format selection sort.
        if let Some(kbps) = format.and_then(|f| f.bitrate_kbps) {
            cmd.args(["-S", &format!("vbr~{kbps}")]);
        }

        cmd.arg("-o");
        cmd.arg(&output_path);

        if let Some(cookies) = cookies_path {
            cmd.args(["--cookies", &cookies.to_string_lossy()]);
        }

        cmd.arg(url);

        #[cfg(windows)]
        {
            // Give `stop()` a way to reach yt-dlp with CTRL_BREAK_EVENT
            // whether we're an interactive `strivo daemon` session or a
            // headless service with no console — see
            // `ffmpeg::win::creation_flags_for_spawn` (shared with
            // `ffmpeg.rs`, which has the same requirement).
            cmd.creation_flags(crate::recording::ffmpeg::win::creation_flags_for_spawn());
        }

        cmd.stdin(std::process::Stdio::null());
        // Capture stdout so the `[download]` progress lines can be parsed
        // (was Stdio::null() — file-size polling is fine for live captures
        // but VOD pulls have a known total + ETA that the webui surfaces).
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Default yt-dlp progress is a carriage-return-overwritten single
        // line; `--newline` turns each tick into its own line so the BufReader
        // sees it.
        cmd.arg("--newline");
        // Throttle progress emission so we don't drown the channel.
        cmd.args(["--progress-delta", "0.5"]);

        let mut child = cmd.spawn()?;

        let stderr_tail = Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(
            STDERR_TAIL_LINES,
        )));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let mut t = tail.lock().unwrap();
                    if t.len() >= STDERR_TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            });
        }

        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        let destinations: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            let progress = progress.clone();
            let destinations = destinations.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout).lines();
                let mut agg = ProgressAggregator::default();
                while let Ok(Some(line)) = reader.next_line().await {
                    if let Some(path) = parse_destination_line(&line) {
                        if let Ok(mut d) = destinations.lock() {
                            d.push(path);
                        }
                    }
                    let prev_format_index = agg.format_index;
                    let prev_phase = agg.phase;
                    agg.observe(&line);
                    // Phase-transition breadcrumbs only — never the raw line
                    // (which can carry stream titles/paths) or cookies.
                    if agg.format_index != prev_format_index {
                        tracing::debug!(
                            format = agg.format_index + 1,
                            total = agg.total_formats,
                            "yt-dlp: next format download started"
                        );
                    }
                    if agg.phase != prev_phase && agg.phase == DownloadPhase::Merging {
                        tracing::debug!("yt-dlp: merge/remux phase started");
                    }
                    if let Ok(mut g) = progress.lock() {
                        *g = agg.snapshot();
                    }
                }
            });
        }

        Ok(Self {
            child,
            output_path,
            stderr_tail,
            progress,
            destinations,
        })
    }

    /// Gracefully stop the download. Unix: SIGINT, as in `ffmpeg.rs`.
    /// Windows: `ffmpeg::win::send_ctrl_break` — yt-dlp has no interactive
    /// stdin quit command (that's ffmpeg's `q`, not yt-dlp's), so
    /// CTRL_BREAK_EVENT is the only graceful path here, and it has to
    /// work from a console-less headless service, not just from a
    /// `strivo daemon` terminal session. See `ffmpeg.rs::win` for why the
    /// naive direct call doesn't and what makes it work anyway, and
    /// `FfmpegProcess::stop()` for the sibling three-step escalation.
    pub async fn stop(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                unsafe {
                    libc::kill(pid as i32, libc::SIGINT);
                }
                if wait_for_graceful_exit(&mut self.child, Duration::from_secs(15)).await {
                    return Ok(());
                }
                tracing::warn!("yt-dlp didn't stop in 15s, killing");
                self.child.kill().await.ok();
            }
        }

        #[cfg(windows)]
        {
            if let Some(pid) = self.child.id() {
                match crate::recording::ffmpeg::win::send_ctrl_break(pid) {
                    Ok(()) => {
                        if wait_for_graceful_exit(&mut self.child, Duration::from_secs(15)).await {
                            return Ok(());
                        }
                        tracing::error!(
                            "yt-dlp did not exit after CTRL_BREAK; forcing kill — \
                             the recording is left truncated/incomplete"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "CTRL_BREAK_EVENT delivery to yt-dlp failed ({e}); forcing kill — \
                             the recording is left truncated/incomplete"
                        );
                    }
                }
                self.child.kill().await.ok();
                anyhow::bail!(
                    "yt-dlp did not shut down gracefully and was force-killed; \
                     its recording is left truncated/incomplete"
                );
            }
        }

        Ok(())
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        Ok(self.child.try_wait()?)
    }

    /// Bytes on disk for this download. The final merged `output_path` only
    /// exists once yt-dlp finishes merging, so while a multi-format VOD
    /// pull is still in progress this sums whatever per-format
    /// `Destination` files are (still) present instead of reporting 0 for
    /// the whole download+merge window. `--no-keep-video` removes the
    /// per-format files once the merge lands, so there's no double-count
    /// once `output_path` exists and the sources are gone.
    pub fn file_size(&self) -> u64 {
        let output = std::fs::metadata(&self.output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let parts: u64 = self
            .destinations
            .lock()
            .map(|d| {
                d.iter()
                    .filter_map(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0);
        output + parts
    }

    /// Snapshot the latest `[download]` line yt-dlp emitted. Returns
    /// `DownloadProgress::default()` (all-None) until the first tick.
    pub fn progress(&self) -> DownloadProgress {
        self.progress.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr_tail
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Wait for `child` to exit within `timeout` after a graceful-stop signal
/// has already been sent. Returns `true` if it exited in time, `false` if
/// the timeout elapsed — the caller escalates to a hard kill in that case.
/// Mirrors `ffmpeg.rs::wait_for_graceful_exit` (kept as a small duplicate
/// rather than a shared module, since it's two call sites); see that
/// file's doc comment for why this specific piece is what's unit-tested.
async fn wait_for_graceful_exit(child: &mut Child, timeout: Duration) -> bool {
    matches!(tokio::time::timeout(timeout, child.wait()).await, Ok(Ok(_)))
}

/// Phrases yt-dlp's stderr uses when the session it was given (cookies —
/// OAuth failures are a separate signal, handled in `platform::*`) is no
/// longer accepted by the platform: expired session, signed-out browser,
/// or an age/bot challenge the cookies used to clear.
///
/// Best-effort and yt-dlp-version-dependent: these are plain substring
/// matches against upstream's human-readable error text, not a stable API.
/// yt-dlp releases can reword them; when a real failure stops matching,
/// widen this list rather than assume the cookie jar is fine.
const COOKIE_REJECTION_PHRASES: &[&str] = &[
    "sign in to confirm you're not a bot",
    "sign in to confirm your age",
    "available to this channel's members",
    "cookies are no longer valid",
    "please sign in",
    "login required",
];

/// Classify a yt-dlp stderr tail as a rejected cookie session, if it
/// matches one of the known phrases. Returns the matched phrase (for the
/// `AuthIssue` reason) rather than the whole tail, which can be long and
/// full of unrelated noise.
pub fn classify_recorder_auth_failure(stderr_tail: &str) -> Option<String> {
    let lower = stderr_tail.to_lowercase();
    COOKIE_REJECTION_PHRASES
        .iter()
        .find(|phrase| lower.contains(*phrase))
        .map(|phrase| phrase.to_string())
}

impl Drop for YtDlpProcess {
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                let _ = self.child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn graceful_exit_within_timeout_is_detected() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 0.05"])
            .spawn()
            .expect("spawn sh");
        let exited = wait_for_graceful_exit(&mut child, Duration::from_secs(2)).await;
        assert!(
            exited,
            "child that exits quickly should be seen as graceful"
        );
    }

    #[tokio::test]
    async fn slow_exit_past_timeout_is_reported_for_escalation() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5"])
            .spawn()
            .expect("spawn sh");
        let exited = wait_for_graceful_exit(&mut child, Duration::from_millis(50)).await;
        assert!(
            !exited,
            "child still running past the deadline must be reported so the caller kills it"
        );
        let _ = child.kill().await;
    }

    #[test]
    fn parse_full_line() {
        let s = "dQw4w9WgXcQ\tNever Gonna Give You Up\tRickAstleyVEVO\n";
        let f = parse_print_line(s).unwrap();
        assert_eq!(f.video_id, "dQw4w9WgXcQ");
        assert_eq!(f.title.as_deref(), Some("Never Gonna Give You Up"));
        assert_eq!(f.uploader.as_deref(), Some("RickAstleyVEVO"));
    }

    #[test]
    fn parse_missing_uploader() {
        // yt-dlp's `--print` emits "NA" when a field isn't available.
        let s = "abc12345678\tA Title\tNA\n";
        let f = parse_print_line(s).unwrap();
        assert_eq!(f.uploader, None);
    }

    #[test]
    fn parse_title_with_tabs_is_truncated_at_uploader() {
        // Real-world stream titles do not contain tabs (yt-dlp escapes
        // them), but be defensive: splitn(3) preserves anything past
        // the second tab in the uploader slot, which is harmless.
        let s = "abc12345678\tWeird title\tWeirder uploader\n";
        let f = parse_print_line(s).unwrap();
        assert_eq!(f.title.as_deref(), Some("Weird title"));
        assert_eq!(f.uploader.as_deref(), Some("Weirder uploader"));
    }

    #[test]
    fn parse_rejects_bad_id() {
        let r = parse_print_line("not-an-id\tT\tU\n");
        assert!(r.is_err(), "should reject ids that are not 11 base64 chars");
    }

    #[test]
    fn parse_skips_blank_leading_lines() {
        let s = "\n\nabc12345678\tT\tU\n";
        let f = parse_print_line(s).unwrap();
        assert_eq!(f.video_id, "abc12345678");
    }

    #[test]
    fn uc_id_detection() {
        // Real UC id pulled from the user's recordings dir.
        assert!(looks_like_uc_id("UCrPseYLGpNygVi34QpGNqpA"));
        assert!(looks_like_uc_id(""));
        assert!(!looks_like_uc_id("hasanabi"));
        assert!(!looks_like_uc_id("UCshort"));
        assert!(!looks_like_uc_id("UC with spaces in the middle!"));
        // Twitch login names happen to be ≤ 25 chars; make sure we
        // don't accidentally clobber a real human-readable name.
        assert!(!looks_like_uc_id("xqc"));
        assert!(!looks_like_uc_id("LinusTechTips_official"));
    }

    #[test]
    fn parse_download_steady_state() {
        let p = parse_download_line("[download]  45.2% of 1.23GiB at 5.50MiB/s ETA 00:42").unwrap();
        assert!((p.pct.unwrap() - 45.2).abs() < 0.01);
        assert_eq!(p.eta_secs, Some(42));
        assert!(p.rate_bps.unwrap() > 5_700_000 && p.rate_bps.unwrap() < 5_800_000);
        assert!(p.bytes_total.unwrap() > 1_300_000_000);
    }

    #[test]
    fn parse_download_early_unknowns() {
        // First ticks before yt-dlp has a rate/ETA estimate.
        let p = parse_download_line("[download]   0.0% of ~1.23GiB at Unknown B/s ETA Unknown")
            .unwrap();
        assert_eq!(p.pct, Some(0.0));
        assert_eq!(p.eta_secs, None);
        assert_eq!(p.rate_bps, None);
        assert!(p.bytes_total.unwrap() > 0);
    }

    #[test]
    fn parse_download_long_eta() {
        let p =
            parse_download_line("[download]  3.1% of 4.20GiB at 1.10MiB/s ETA 01:23:45").unwrap();
        assert_eq!(p.eta_secs, Some(5025));
    }

    /// Captured verbatim from `yt-dlp --newline --progress-delta 0.5`
    /// against a fragmented HLS source. The tilde is its own token here;
    /// the hand-written `~1.23GiB` fixture below never exercised that, which
    /// is how fragmented downloads shipped with progress permanently stuck.
    #[test]
    fn parse_download_fragmented_estimate_with_padded_tilde() {
        let p = parse_download_line(
            "[download]   0.0% of ~ 324.23MiB at      0.00B/s ETA Unknown (frag 0/64)",
        )
        .expect("real fragmented progress line must parse");
        assert_eq!(p.pct, Some(0.0));
        assert!(
            p.bytes_total.is_some(),
            "estimated total should still parse"
        );
        assert_eq!(p.eta_secs, None, "ETA Unknown");

        let p = parse_download_line(
            "[download]   0.3% of ~ 356.16MiB at  336.35KiB/s ETA 14:45 (frag 0/64)",
        )
        .expect("real fragmented progress line must parse");
        assert_eq!(p.pct, Some(0.3));
        assert_eq!(p.eta_secs, Some(14 * 60 + 45));
        assert!(p.rate_bps.unwrap() > 0);
    }

    /// Also captured verbatim — a plain (non-fragmented) download, which
    /// pads fields with runs of spaces.
    #[test]
    fn parse_download_real_plain_progress_line() {
        let p = parse_download_line("[download]   0.6% of   20.65MiB at  299.70KiB/s ETA 01:10")
            .expect("real plain progress line must parse");
        assert_eq!(p.pct, Some(0.6));
        assert_eq!(p.eta_secs, Some(70));
    }

    #[test]
    fn parse_download_non_progress_lines_ignored() {
        assert!(parse_download_line("[info] foo").is_none());
        assert!(parse_download_line("ERROR: nope").is_none());
        // Completion line uses "in" instead of "ETA" — out of scope, ignored.
        assert!(parse_download_line("[download] 100% of 1.23GiB in 04:23").is_none());
    }

    #[test]
    fn parses_destination_line() {
        let p = parse_destination_line("[download] Destination: /r/out.f399.mp4").unwrap();
        assert_eq!(p, PathBuf::from("/r/out.f399.mp4"));
        assert!(
            parse_destination_line("[download]  45.2% of 1.23GiB at 5.50MiB/s ETA 00:42").is_none()
        );
    }

    #[test]
    fn parses_format_count_from_info_line() {
        // Real line (see module doc): the "1" in "1 format(s)" is a
        // labeling quirk — the actual count comes from splitting the id
        // spec on '+'.
        assert_eq!(
            format_count_from_info_line("[info] -iyIne7c3DM: Downloading 1 format(s): 399+251-12"),
            Some(2)
        );
        assert_eq!(
            format_count_from_info_line("[info] abc: Downloading 1 format(s): 137"),
            Some(1)
        );
        assert_eq!(
            format_count_from_info_line("[download] Destination: x"),
            None
        );
    }

    #[test]
    fn detects_merge_phase_lines() {
        assert!(is_merge_phase_line(
            "[Merger] Merging formats into \"/r/out.mkv\""
        ));
        assert!(is_merge_phase_line("[VideoRemuxer] Remuxing video"));
        assert!(is_merge_phase_line("[FixupM3u8] Fixing up \"out.mp4\""));
        assert!(!is_merge_phase_line("[download] Destination: x"));
    }

    /// Replays the real yt-dlp transcript from a YouTube VOD pull (video
    /// then audio sequentially, then merge) and asserts the aggregated
    /// overall percent never regresses — the bug this fixes had it go
    /// 98.0% → 15.8% when the audio format started, then freeze at 89.7%
    /// with no visible merge phase.
    #[test]
    fn aggregator_overall_pct_is_monotonic_across_a_real_transcript() {
        let lines = [
            "[youtube] Extracting URL: https://www.youtube.com/watch?v=-iyIne7c3DM",
            "[youtube] -iyIne7c3DM: Downloading webpage",
            "[info] -iyIne7c3DM: Downloading 1 format(s): 399+251-12",
            "[download] Destination: /path/out.f399.mp4",
            "[download]   0.0% of  989.48MiB at  Unknown B/s ETA Unknown",
            "[download]   2.1% of  989.48MiB at   40.11MiB/s ETA 00:24",
            "[download]  50.0% of  989.48MiB at   40.11MiB/s ETA 00:12",
            "[download]  98.0% of  989.48MiB at   20.30MiB/s ETA 00:01",
            "[download] Destination: /path/out.f251-12.webm",
            "[download]  15.8% of   80.12MiB at   10.00MiB/s ETA 00:05",
            "[download]  89.7% of   80.12MiB at   10.00MiB/s ETA 00:01",
            "[Merger] Merging formats into \"/path/out.mkv\"",
            "Deleting original file /path/out.f399.mp4 (pass -k to keep)",
            "Deleting original file /path/out.f251-12.webm (pass -k to keep)",
        ];
        let mut agg = ProgressAggregator::default();
        let mut last_pct = 0.0f32;
        for line in lines {
            agg.observe(line);
            let snap = agg.snapshot();
            let pct = snap
                .pct
                .expect("pct always populated once observing starts");
            assert!(
                pct >= last_pct,
                "pct regressed: {last_pct} -> {pct} on line {line:?}"
            );
            last_pct = pct;
        }
        assert_eq!(agg.phase, DownloadPhase::Merging);
        assert!(
            last_pct < 100.0,
            "pct must stay below 100 until the job actually finishes, got {last_pct}"
        );
        assert!(
            last_pct > 50.0,
            "final pct should reflect the large video format being fully done, got {last_pct}"
        );
    }

    #[test]
    fn aggregator_single_format_download_uses_raw_percent() {
        let mut agg = ProgressAggregator::default();
        agg.observe("[download] Destination: /r/out.mp4");
        agg.observe("[download]  45.0% of 100.00MiB at 5.00MiB/s ETA 00:10");
        let snap = agg.snapshot();
        assert_eq!(snap.pct, Some(45.0));
        assert_eq!(snap.phase, Some(DownloadPhase::Downloading));
    }

    #[test]
    fn aggregator_merge_phase_holds_and_caps_below_100() {
        let mut agg = ProgressAggregator::default();
        agg.observe("[download] Destination: /r/out.mp4");
        agg.observe("[download] 100.0% of 100.00MiB at 5.00MiB/s ETA 00:00");
        let before = agg.snapshot().pct.unwrap();
        assert!(
            before < 100.0,
            "must cap below 100 even pre-merge, got {before}"
        );
        agg.observe("[Merger] Merging formats into \"/r/out.mkv\"");
        let held = agg.snapshot().pct.unwrap();
        assert!(
            held < 100.0,
            "merge phase must cap pct below 100, got {held}"
        );
        assert!(held >= before, "merge phase must not regress");
        // No further download lines: stays held on repeated snapshots.
        let held_again = agg.snapshot().pct.unwrap();
        assert_eq!(held, held_again);
    }
}

#[cfg(test)]
mod cookie_rejection_tests {
    use super::classify_recorder_auth_failure;

    #[test]
    fn detects_bot_challenge() {
        let tail = "ERROR: [youtube] abc123: Sign in to confirm you're not a bot.";
        assert_eq!(
            classify_recorder_auth_failure(tail),
            Some("sign in to confirm you're not a bot".to_string())
        );
    }

    #[test]
    fn detects_age_gate() {
        let tail = "ERROR: Sign in to confirm your age. This video may be inappropriate.";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn detects_members_only() {
        let tail = "ERROR: This video is available to this channel's members on level: ...";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn detects_stale_cookies() {
        let tail = "WARNING: The provided cookies are no longer valid.";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn detects_please_sign_in() {
        let tail = "ERROR: Please sign in.";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn private_video_is_not_a_cookie_failure() {
        // A creator can simply have made the VOD private; that says nothing
        // about our session, so it must not raise a cookies issue.
        let tail = "ERROR: This video is private.";
        assert!(classify_recorder_auth_failure(tail).is_none());
    }

    #[test]
    fn detects_login_required() {
        let tail = "ERROR: Login required to access this content.";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn is_case_insensitive() {
        let tail = "ERROR: SIGN IN TO CONFIRM YOU'RE NOT A BOT";
        assert!(classify_recorder_auth_failure(tail).is_some());
    }

    #[test]
    fn unrelated_failure_is_none() {
        let tail = "ERROR: unable to download webpage: HTTP Error 503: Service Unavailable";
        assert_eq!(classify_recorder_auth_failure(tail), None);
    }

    #[test]
    fn empty_tail_is_none() {
        assert_eq!(classify_recorder_auth_failure(""), None);
    }
}
