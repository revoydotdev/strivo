use anyhow::Result;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::ResolvedFormat;

/// How many trailing stderr lines to keep for diagnostics.
const STDERR_TAIL_LINES: usize = 40;

/// Windows console-signal plumbing shared by this file and `ytdlp.rs`
/// (`crate::recording::ffmpeg::win::…`). Isolated here because it is the
/// one part of the Windows port with a real, documented sharp edge: see
/// `send_ctrl_break`.
#[cfg(windows)]
pub(crate) mod win {
    use std::sync::Mutex;
    use windows_sys::Win32::System::Console::{
        AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleWindow, CTRL_BREAK_EVENT,
    };

    /// Serializes the "borrow a console" dance in `send_ctrl_break`.
    /// Console attachment is per-*process* state, not per-child: if two
    /// recordings are stopped at the same moment on a headless daemon,
    /// unsynchronized `FreeConsole`/`AttachConsole` calls could race and
    /// misdeliver CTRL_BREAK to the wrong child, or fail to attach at
    /// all. Held only across the synchronous FFI calls below, never
    /// across an `.await`.
    static CONSOLE_ATTACH_LOCK: Mutex<()> = Mutex::new(());

    /// `CREATE_NEW_CONSOLE`: give the child its own console instead of
    /// inheriting ours.
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    /// `CREATE_NEW_PROCESS_GROUP`: child becomes the root of its own
    /// process group, so `GenerateConsoleCtrlEvent` can target it
    /// without also hitting us. Windows ignores this flag when combined
    /// with `CREATE_NEW_CONSOLE` (a new console already implies a new
    /// group), so the two flags below are mutually exclusive by
    /// documented behavior, not by choice.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    /// Whether the calling process currently has a console. A Windows
    /// service (what `strivo enable` installs — see `daemon.rs`) starts
    /// with none; `strivo daemon` run from an interactive terminal has
    /// one for its whole lifetime. This is checked once at spawn time
    /// (`creation_flags_for_spawn`) and again at stop time
    /// (`send_ctrl_break`) — both checks observe the same whole-process,
    /// unchanging fact for the life of the daemon, so it is safe to ask
    /// twice instead of threading a flag through `FfmpegProcess`/
    /// `YtDlpProcess`.
    fn has_console() -> bool {
        // SAFETY: no arguments, no output buffer — just reads the
        // calling process's console handle.
        !unsafe { GetConsoleWindow() }.is_null()
    }

    /// Creation flags to spawn ffmpeg/yt-dlp with, so `send_ctrl_break`
    /// can reach them later:
    ///   - We have a console (dev-mode, run from a terminal): let the
    ///     child inherit it (`CREATE_NEW_PROCESS_GROUP` only) — we
    ///     already share a console with it, so `GenerateConsoleCtrlEvent`
    ///     can be called directly at stop time.
    ///   - We have none (the real deployment target — a headless
    ///     service): give the child its own console
    ///     (`CREATE_NEW_CONSOLE`) so there is one to attach to. See
    ///     `send_ctrl_break`.
    pub(crate) fn creation_flags_for_spawn() -> u32 {
        if has_console() {
            CREATE_NEW_PROCESS_GROUP
        } else {
            CREATE_NEW_CONSOLE
        }
    }

    /// Send CTRL_BREAK to the console process group rooted at `pid`
    /// (`pid` doubles as the group id — see `creation_flags_for_spawn`).
    ///
    /// `GenerateConsoleCtrlEvent` only reaches processes that share the
    /// *calling* process's console (Microsoft Learn,
    /// "GenerateConsoleCtrlEvent function": "Only those processes in the
    /// group that share the same console as the calling process receive
    /// the signal"). A headless service has no console at all, so a
    /// direct call fails outright with no way to retry your way out of
    /// it — that was the gap in the first version of this fix: it looked
    /// correct because it was tested from a `strivo daemon` terminal
    /// session, where a console happens to already be shared.
    ///
    /// When we have no console of our own, we borrow the child's for the
    /// duration of one call: `AttachConsole(pid)`, deliver the event,
    /// `FreeConsole()` to give it back immediately after. We are the
    /// child's *parent*, not a member of its process group (the group is
    /// "all processes that are descendants of the root process"), so we
    /// never receive the event ourselves by attaching to its console.
    pub(crate) fn send_ctrl_break(pid: u32) -> std::io::Result<()> {
        let ok = if has_console() {
            // SAFETY: pid is a live child pid we hold via `Child`; no
            // pointers or shared mutable state cross this call.
            unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) }
        } else {
            let _guard = CONSOLE_ATTACH_LOCK.lock().unwrap();
            // SAFETY: serialized by `CONSOLE_ATTACH_LOCK` above so only
            // one thread in this process ever holds a borrowed console
            // attachment at a time; `pid` is a live child pid.
            unsafe {
                FreeConsole();
                let attached = AttachConsole(pid);
                let sent = if attached != 0 {
                    GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid)
                } else {
                    0
                };
                // Always give the console back, even on a failed attach
                // (a no-op FreeConsole is harmless) or failed send, so we
                // never leave ourselves wrongly attached to a child's
                // console.
                FreeConsole();
                if attached == 0 {
                    0
                } else {
                    sent
                }
            }
        };
        if ok == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

pub struct FfmpegProcess {
    child: Child,
    pub output_path: PathBuf,
    stderr_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// ffmpeg's stdin, piped so `stop()` can write `q` (its documented
    /// interactive quit) on Windows — the one graceful-stop path that
    /// needs no console at all. Unix uses SIGINT instead and keeps stdin
    /// closed (`Stdio::null()`), unchanged from before this file grew a
    /// Windows arm.
    #[cfg(windows)]
    stdin: Option<tokio::process::ChildStdin>,
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

        // Unix: stdin is unused (SIGINT does the job), so keep it closed
        // exactly as before. Windows: piped, so `stop()` can write `q` —
        // see the `stdin` field doc on `FfmpegProcess`.
        #[cfg(not(windows))]
        cmd.stdin(std::process::Stdio::null());
        #[cfg(windows)]
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            // Give `stop()` a way to reach ffmpeg with CTRL_BREAK_EVENT
            // regardless of whether we're an interactive `strivo daemon`
            // session or a headless service with no console of our own —
            // see `win::creation_flags_for_spawn`.
            cmd.creation_flags(win::creation_flags_for_spawn());
        }

        let mut child = cmd.spawn()?;

        #[cfg(windows)]
        let stdin = child.stdin.take();

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
            #[cfg(windows)]
            stdin,
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
    /// Windows, in order:
    ///   1. Write `q` to ffmpeg's piped stdin — its documented interactive
    ///      quit. Needs no console at all, so it works the same whether
    ///      we're an interactive `strivo daemon` session or (the primary
    ///      deployment) a headless Windows service with no console. Tried
    ///      first for exactly that reason.
    ///   2. `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT)` as a fallback —
    ///      see `win::send_ctrl_break` for how this is made to work from a
    ///      console-less service too, which the first version of this fix
    ///      got wrong (it only worked from a terminal session, where a
    ///      console happens to already be shared with the child).
    ///   3. `TerminateProcess` (`Child::kill()`) as the last resort. This
    ///      is the bug being fixed — no trailer is written, the file is
    ///      truncated — so reaching it is logged as an error, not a warning:
    ///      an operator needs to know a recording came out damaged.
    ///
    /// `q` only covers ffmpeg — yt-dlp has no equivalent interactive stdin
    /// command, hence step 2 still has to exist and still has to work
    /// headless; `ytdlp.rs::stop()` uses it directly as its only graceful
    /// path.
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
                let mut asked_via_stdin = false;
                if let Some(stdin) = self.stdin.as_mut() {
                    use tokio::io::AsyncWriteExt;
                    asked_via_stdin =
                        stdin.write_all(b"q\n").await.is_ok() && stdin.flush().await.is_ok();
                }
                if asked_via_stdin
                    && wait_for_graceful_exit(&mut self.child, Duration::from_secs(6)).await
                {
                    return Ok(());
                }

                match win::send_ctrl_break(pid) {
                    Ok(()) => {
                        if wait_for_graceful_exit(&mut self.child, Duration::from_secs(4)).await {
                            return Ok(());
                        }
                        tracing::error!(
                            "ffmpeg did not exit after CTRL_BREAK; forcing kill — \
                             output file is truncated (missing Matroska/MP4 trailer)"
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            "CTRL_BREAK_EVENT delivery to ffmpeg failed ({e}); forcing kill — \
                             output file is truncated (missing Matroska/MP4 trailer)"
                        );
                    }
                }
                self.child.kill().await.ok();
                anyhow::bail!(
                    "ffmpeg did not shut down gracefully and was force-killed; \
                     its recording is truncated (no Matroska/MP4 trailer)"
                );
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
