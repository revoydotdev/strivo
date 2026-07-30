//! Async wrapper around `strivo_core::ipc` for the web UI.
//!
//! Two surfaces:
//!
//! - [`IpcClient::snapshot`] — open the socket, send Hello, await the
//!   initial [`ServerMessage::StateSnapshot`], close. Used by the
//!   dashboard handler on each request so the page boots from a fresh
//!   snapshot.
//!
//! - [`IpcClient::events`] — open a persistent connection, ignore the
//!   StateSnapshot, return a `Stream<Item = DaemonEvent>` that fires
//!   for every subsequent broadcast. Wired into the `/events` SSE
//!   endpoint so HTMX `hx-sse` clients see live updates.

use std::path::PathBuf;
use std::pin::Pin;

use anyhow::{anyhow, Context, Result};
use async_stream::try_stream;
use futures::Stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use strivo_core::events::DaemonEvent;
use strivo_core::ipc::{self, ClientMessage, ServerMessage};

pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn connect_or_err() -> Result<Self> {
        if !ipc::is_daemon_running() {
            return Err(anyhow!("daemon not running; start it with `strivo daemon`"));
        }
        Ok(Self {
            socket_path: ipc::socket_path(),
        })
    }

    /// Single-shot snapshot. The route handlers call this each request
    /// Fire-and-forget command — open the socket, write one
    /// [`ClientMessage`], close. The daemon's downstream side-effects
    /// (RecordingStarted, ChannelsUpdated, …) arrive via the
    /// `/events` SSE stream the browser is already subscribed to, so
    /// no per-call response is needed. (W1.)
    pub async fn send_command(&self, msg: ClientMessage) -> Result<()> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .context("connect to daemon socket")?;
        let payload = ipc::encode_message(&msg)?;
        stream.write_all(payload.as_bytes()).await?;
        stream.flush().await?;
        stream.shutdown().await.ok();
        Ok(())
    }

    /// Test-only: an `IpcClient` bound to a socket path that never exists,
    /// so router-level tests can build a full `AppState` without a running
    /// daemon. Only safe for handlers under test that never touch the IPC
    /// socket (e.g. `telemetry::telemetry_handler`).
    #[cfg(test)]
    pub(crate) fn disconnected() -> Self {
        Self {
            socket_path: PathBuf::from("/nonexistent-strivo-test-socket"),
        }
    }

    /// — fast enough that we don't bother caching at the web layer.
    pub async fn snapshot(&self) -> Result<ServerMessage> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .context("connect to daemon socket")?;
        let (reader, mut writer) = stream.into_split();
        let payload = ipc::encode_message(&ClientMessage::Hello {
            version: ipc::IPC_PROTOCOL_VERSION,
        })?;
        writer.write_all(payload.as_bytes()).await?;
        writer.flush().await?;

        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("daemon closed socket before snapshot"));
        }
        let msg: ServerMessage =
            serde_json::from_str(line.trim()).context("decode daemon response")?;
        Ok(msg)
    }

    /// Persistent event stream. Yields one [`DaemonEvent`] per
    /// broadcast. Terminates when the daemon socket closes; callers
    /// (e.g. the SSE endpoint) reconnect as needed.
    pub fn events(&self) -> Pin<Box<dyn Stream<Item = Result<DaemonEvent>> + Send>> {
        let path = self.socket_path.clone();
        Box::pin(try_stream! {
            let stream = UnixStream::connect(&path)
                .await
                .context("connect to daemon socket for /events")?;
            let (reader, mut writer) = stream.into_split();
            let payload = ipc::encode_message(&ClientMessage::Hello {
                version: ipc::IPC_PROTOCOL_VERSION,
            })?;
            writer.write_all(payload.as_bytes()).await?;
            writer.flush().await?;

            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            // First message is the StateSnapshot; the live event stream
            // is everything after it. We don't surface the snapshot
            // here — the snapshot() helper covers that case.
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return;
            }

            loop {
                line.clear();
                let n = reader.read_line(&mut line).await?;
                if n == 0 {
                    break; // daemon closed
                }
                let msg: ServerMessage = match serde_json::from_str(line.trim()) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("daemon event decode failed: {e}");
                        continue;
                    }
                };
                if let ServerMessage::Event(de) = msg {
                    yield de;
                }
            }
        })
    }
}
