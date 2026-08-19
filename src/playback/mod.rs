#[cfg(unix)]
use anyhow::bail;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::ipc::{Endpoint, Stream as IpcStream};

pub struct MpvController {
    child: Option<Child>,
    /// mpv's `--input-ipc-server` value: a filesystem socket path on
    /// Unix, a `\\.\pipe\<name>` pipe name on Windows. mpv accepts both
    /// forms verbatim on their respective platforms.
    socket_path: String,
}

impl Default for MpvController {
    fn default() -> Self {
        Self::new()
    }
}

impl MpvController {
    pub fn new() -> Self {
        let pid = std::process::id();
        #[cfg(unix)]
        let socket_path = format!("/tmp/strivo-mpv-{pid}.sock");
        #[cfg(windows)]
        let socket_path = format!(r"\\.\pipe\strivo-mpv-{pid}");
        Self {
            child: None,
            socket_path,
        }
    }

    fn endpoint(&self) -> Endpoint {
        #[cfg(unix)]
        {
            Endpoint::Path(std::path::PathBuf::from(&self.socket_path))
        }
        #[cfg(windows)]
        {
            Endpoint::Pipe(self.socket_path.clone())
        }
    }

    /// Does mpv's IPC endpoint currently exist? A Unix socket has a
    /// filesystem entry to check; a Windows named pipe does not, so the
    /// existence probe there is folded into `send_command`'s connect
    /// attempt instead (see its Windows branch).
    fn socket_exists(&self) -> bool {
        #[cfg(unix)]
        {
            Path::new(&self.socket_path).exists()
        }
        #[cfg(windows)]
        {
            // mpv creates the pipe synchronously at startup, so a quick
            // connect attempt is the only reliable existence check.
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.socket_path)
                .is_ok()
        }
    }

    /// Launch mpv with IPC server, playing the given URL
    pub async fn play(&mut self, url: &str) -> Result<()> {
        // Kill existing instance if any
        self.quit().await.ok();

        // Clean up a stale socket file (Unix only — named pipes have no
        // filesystem entry to remove).
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);

        let child = Command::new("mpv")
            .args([
                &format!("--input-ipc-server={}", self.socket_path),
                "--no-terminal",
                "--force-window=yes",
                "--keep-open=no",
                url,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to launch mpv - is it installed?")?;

        self.child = Some(child);

        // Wait briefly for the IPC endpoint to appear
        for _ in 0..20 {
            if self.socket_exists() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Ok(())
    }

    /// Play a local file
    pub async fn play_file(&mut self, path: &Path) -> Result<()> {
        self.play(&path.to_string_lossy()).await
    }

    /// Like play_file but passes `--start=<sec>` so mpv seeks on load
    /// (M5.2 — transcript-scoped seek).
    pub async fn play_file_at(&mut self, path: &Path, start_secs: f64) -> Result<()> {
        // Kill existing instance if any
        self.quit().await.ok();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);
        let child = Command::new("mpv")
            .args([
                &format!("--input-ipc-server={}", self.socket_path),
                "--no-terminal",
                "--force-window=yes",
                "--keep-open=no",
                &format!("--start={start_secs:.3}"),
                &path.to_string_lossy(),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("Failed to launch mpv - is it installed?")?;
        self.child = Some(child);
        for _ in 0..20 {
            if self.socket_exists() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Send a JSON IPC command to mpv
    async fn send_command(&self, command: &[&str]) -> Result<String> {
        // On Unix we can cheaply check for the socket file before
        // connecting. On Windows there is no filesystem entry to check;
        // `IpcStream::connect` itself is the existence probe there, and
        // its failure surfaces as the same "not found" error below.
        #[cfg(unix)]
        if !self.socket_exists() {
            bail!("mpv IPC socket not found");
        }

        let stream = IpcStream::connect(&self.endpoint())
            .await
            .context("Failed to connect to mpv IPC socket")?;

        let (reader, mut writer) = tokio::io::split(stream);

        // Build JSON command
        let cmd_json = serde_json::json!({
            "command": command
        });
        let mut msg = serde_json::to_string(&cmd_json)?;
        msg.push('\n');

        writer.write_all(msg.as_bytes()).await?;
        writer.flush().await?;

        // Read response
        let mut buf_reader = BufReader::new(reader);
        let mut response = String::new();
        buf_reader.read_line(&mut response).await?;

        Ok(response)
    }

    /// Toggle play/pause
    pub async fn toggle_pause(&self) -> Result<()> {
        self.send_command(&["cycle", "pause"]).await?;
        Ok(())
    }

    /// Seek relative (seconds, can be negative)
    pub async fn seek(&self, seconds: f64) -> Result<()> {
        self.send_command(&["seek", &seconds.to_string(), "relative"])
            .await?;
        Ok(())
    }

    /// Get current playback position
    pub async fn get_position(&self) -> Result<f64> {
        let resp = self.send_command(&["get_property", "time-pos"]).await?;
        let parsed: serde_json::Value = serde_json::from_str(&resp)?;
        parsed["data"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Invalid position response"))
    }

    /// Set volume (0-100)
    pub async fn set_volume(&self, volume: u32) -> Result<()> {
        self.send_command(&["set_property", "volume", &volume.to_string()])
            .await?;
        Ok(())
    }

    /// Set playback speed multiplier.
    pub async fn set_speed(&self, speed: f64) -> Result<()> {
        self.send_command(&["set_property", "speed", &speed.to_string()])
            .await?;
        Ok(())
    }

    /// Quit mpv
    pub async fn quit(&mut self) -> Result<()> {
        // Try IPC quit first (before borrowing self.child mutably)
        if self.socket_exists() {
            self.send_command(&["quit"]).await.ok();
        }

        if let Some(ref mut child) = self.child {
            // Wait briefly for clean exit
            match tokio::time::timeout(std::time::Duration::from_secs(3), child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    child.kill().await.ok();
                }
            }

            self.child = None;
        }

        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);
        Ok(())
    }

    /// Check if mpv is still running
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    false
                }
                Ok(None) => true,
                Err(_) => false,
            }
        } else {
            false
        }
    }
}

impl Drop for MpvController {
    fn drop(&mut self) {
        // Best-effort cleanup
        if let Some(ref mut child) = self.child {
            // Can't do async in drop, just kill
            let _ = child.start_kill();
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
