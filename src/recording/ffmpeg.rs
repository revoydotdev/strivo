use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::ResolvedFormat;

/// How many trailing stderr lines to keep for diagnostics.
const STDERR_TAIL_LINES: usize = 40;

pub struct FfmpegProcess {
    child: Child,
    pub output_path: PathBuf,
    stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
}

pub struct FfmpegBuilder {
    input_url: String,
    output_path: PathBuf,
    transcode: bool,
    format: Option<ResolvedFormat>,
    from_start: bool,
}

impl FfmpegBuilder {
    pub fn new(input_url: String, output_path: PathBuf) -> Self {
        Self {
            input_url,
            output_path,
            transcode: false,
            format: None,
            from_start: false,
        }
    }

    pub fn transcode(mut self, enabled: bool) -> Self {
        self.transcode = enabled;
        self
    }

    pub fn format(mut self, format: ResolvedFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Start pulling from the first segment in the HLS manifest instead of
    /// the live edge. For Twitch this lands ~5 minutes back (the DVR window);
    /// the closest the protocol gets to "from beginning".
    pub fn from_start(mut self, enabled: bool) -> Self {
        self.from_start = enabled;
        self
    }

    pub fn build(self) -> Result<FfmpegProcess> {
        // Map a container name (config) or output extension (fallback) to the
        // ffmpeg muxer name. Browser-playable picks are kept first.
        fn container_to_muxer(c: &str) -> &'static str {
            match c.trim().to_ascii_lowercase().as_str() {
                "mkv" | "matroska" => "matroska",
                "mp4" | "m4v" | "m4a" => "mp4",
                "webm" => "webm",
                "ts" | "mpegts" => "mpegts",
                "mov" => "mov",
                "wav" => "wav",
                "flac" => "flac",
                "ogg" | "opus" | "oga" => "ogg",
                "mp3" => "mp3",
                "aac" | "adts" => "adts",
                // Unknown → safest default for the webui player.
                _ => "matroska",
            }
        }

        if let Some(parent) = self.output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-y", "-hide_banner", "-loglevel", "warning"]);

        if self.from_start {
            // -99999 lands on the first segment in the current HLS
            // playlist (negative is clamped to 0 after `n_segments +
            // live_start_index`). Plain `0` would target absolute
            // segment index 0, which is never present in a live
            // playlist with rolling EXT-X-MEDIA-SEQUENCE — ffmpeg then
            // 404s every segment and exits.
            cmd.args(["-live_start_index", "-99999"]);
        }

        cmd.args(["-i", &self.input_url]);

        // Resolve codecs: explicit format overrides the legacy `transcode` toggle.
        let (vcodec, acodec, bitrate_kbps) = match (self.format.as_ref(), self.transcode) {
            (Some(f), _) => (f.video_codec.clone(), f.audio_codec.clone(), f.bitrate_kbps),
            (None, true) => ("h264_nvenc".to_string(), "aac".to_string(), None),
            (None, false) => ("copy".to_string(), "copy".to_string(), None),
        };

        if vcodec == "copy" && acodec == "copy" {
            cmd.args(["-c", "copy"]);
            // HLS-from-Twitch carries AAC in ADTS framing; raw AAC in a
            // Matroska/MP4 container needs ASC headers instead, or the
            // file ends up with TS-style packets the browser refuses.
            // No-op on already-ASC sources, so always safe.
            cmd.args(["-bsf:a", "aac_adtstoasc"]);
        } else {
            cmd.args(["-c:v", &vcodec]);
            if vcodec == "h264_nvenc" {
                cmd.args(["-preset", "p4"]);
                if let Some(kbps) = bitrate_kbps {
                    cmd.args(["-b:v", &format!("{kbps}k")]);
                } else {
                    cmd.args(["-cq", "23"]);
                }
            } else if vcodec == "libx264" {
                cmd.args(["-preset", "veryfast"]);
                if let Some(kbps) = bitrate_kbps {
                    cmd.args(["-b:v", &format!("{kbps}k")]);
                } else {
                    cmd.args(["-crf", "23"]);
                }
            }
            cmd.args(["-c:a", &acodec]);
            if acodec != "copy" {
                cmd.args(["-b:a", "192k"]);
            }
        }

        // Pin the muxer explicitly. Without `-f`, ffmpeg infers from the
        // output extension — but for HLS-in / copy-out it can keep the
        // input demuxer's container (TS) intact, which is what produced
        // the in-the-wild `.mkv` files that ffprobe reports as `mpegts`
        // and `<video>` refuses to play. The container name comes from
        // the resolved format if set, otherwise from the output extension.
        let muxer = self
            .format
            .as_ref()
            .map(|f| container_to_muxer(&f.container))
            .or_else(|| {
                self.output_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(container_to_muxer)
            })
            .unwrap_or("matroska");
        cmd.args(["-f", muxer]);

        cmd.arg(&self.output_path);

        // Don't inherit stdin so we can send signals
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            // Spawn ffmpeg into its own console process group so `stop()` can
            // target it alone with CTRL_BREAK_EVENT — without this flag the
            // event would also reach our own process (and any other child
            // sharing our console) since Windows console signals are
            // group-wide, not per-process. See `stop()` below for why
            // CTRL_BREAK is used instead of stdin `q` or TerminateProcess.
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }

        let mut child = cmd.spawn()?;

        // Drain stderr asynchronously: a piped+un-drained stderr fills
        // the kernel pipe buffer and stalls ffmpeg. Also keep the last
        // STDERR_TAIL_LINES so failure paths can surface the real error.
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

        Ok(FfmpegProcess {
            child,
            output_path: self.output_path,
            stderr_tail,
        })
    }
}

impl FfmpegProcess {
    /// Gracefully stop recording so ffmpeg writes the Matroska/MP4 trailer
    /// and cue index before exiting. Without a clean shutdown the output
    /// file is truncated: it may still play, but seeking and duration are
    /// broken.
    ///
    /// Unix: SIGINT, which ffmpeg's own signal handler treats identically to
    /// interactive `q`/Ctrl-C — clean stop.
    ///
    /// Windows: `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)`, targeted at the
    /// process group `build()` placed ffmpeg into. Two other options were
    /// considered and rejected:
    ///   - Writing `q` to ffmpeg's stdin: this is ffmpeg's documented
    ///     interactive quit and would work, but stdin is wired to
    ///     `Stdio::null()` (see `build()` — the comment there already
    ///     explains stdin is closed specifically so signals are used
    ///     instead). It also doesn't generalize to yt-dlp, which has no
    ///     interactive stdin quit command, and this file's escalation logic
    ///     is intentionally kept identical in shape to `ytdlp.rs`'s so the
    ///     two are easy to audit together.
    ///   - `TerminateProcess` (`Child::kill()`): this is the bug being
    ///     fixed — no trailer is written, every Windows recording would be
    ///     truncated.
    ///
    /// CTRL_BREAK_EVENT is what's left: it is delivered like a signal (no
    /// stdin needed) and ffmpeg's console handler treats it as a shutdown
    /// request, same as SIGINT on Unix.
    pub async fn stop(&mut self) -> Result<()> {
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.id() {
                // Send SIGINT for clean shutdown
                unsafe {
                    libc::kill(pid as i32, libc::SIGINT);
                }
                if wait_for_graceful_exit(&mut self.child, Duration::from_secs(10)).await {
                    return Ok(());
                }
                tracing::warn!("ffmpeg didn't stop in 10s, killing");
                self.child.kill().await.ok();
            }
        }

        #[cfg(windows)]
        {
            if let Some(pid) = self.child.id() {
                // SAFETY: FFI call into the Windows API with a valid,
                // still-live process id (we hold `self.child`); no pointers
                // or shared state are involved. `GenerateConsoleCtrlEvent`
                // is documented to signal every process attached to the
                // given console process group — `build()` puts ffmpeg in
                // its own group via CREATE_NEW_PROCESS_GROUP so this
                // doesn't also hit our own process.
                let ok = unsafe {
                    windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent(
                        windows_sys::Win32::System::Console::CTRL_BREAK_EVENT,
                        pid,
                    )
                };
                if ok == 0 {
                    tracing::warn!(
                        "GenerateConsoleCtrlEvent failed: {:?}",
                        std::io::Error::last_os_error()
                    );
                } else if wait_for_graceful_exit(&mut self.child, Duration::from_secs(10)).await {
                    return Ok(());
                } else {
                    tracing::warn!("ffmpeg didn't stop in 10s, killing");
                }
                self.child.kill().await.ok();
            }
        }

        Ok(())
    }

    /// Check if process is still running
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        Ok(self.child.try_wait()?)
    }

    /// Get the output file size in bytes
    pub fn file_size(&self) -> u64 {
        std::fs::metadata(&self.output_path)
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Snapshot of the trailing ffmpeg stderr lines, joined with newlines.
    /// Useful for surfacing the real cause of a non-zero exit.
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
/// the timeout elapsed — the caller is responsible for escalating to a hard
/// kill in that case. This is the escalation logic shared by the Unix and
/// Windows arms of `stop()` (and mirrored in `ytdlp.rs`), pulled out so it
/// can be exercised directly with a real child process on any platform,
/// independent of which OS-specific signal was used to ask for the exit.
async fn wait_for_graceful_exit(child: &mut Child, timeout: Duration) -> bool {
    matches!(tokio::time::timeout(timeout, child.wait()).await, Ok(Ok(_)))
}

impl Drop for FfmpegProcess {
    fn drop(&mut self) {
        // If process already exited, nothing to do
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                // Still running — kill to prevent zombie
                let _ = self.child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise `wait_for_graceful_exit` — the timeout/escalation
    // decision shared by the Unix and Windows arms of `stop()` — against a
    // real child process rather than a mock. What differs between
    // platforms is only *which signal* asks the child to exit (SIGINT vs
    // CTRL_BREAK_EVENT, tested by the human on the Windows VM per the
    // handoff notes); the timeout/escalate decision itself is the same
    // code path on every platform and is what's under test here.

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
        // Clean up so the test doesn't leak a sleeping process.
        let _ = child.kill().await;
    }
}
