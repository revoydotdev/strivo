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

use strivo_core::events::DaemonEvent;
use strivo_core::ipc::{self, ClientMessage, Endpoint, ServerMessage, Stream as IpcStream};

pub struct IpcClient {
    endpoint: Endpoint,
}

impl IpcClient {
    pub fn connect_or_err() -> Result<Self> {
        if !ipc::is_daemon_running() {
            return Err(anyhow!("daemon not running; start it with `strivo daemon`"));
        }
        Ok(Self {
            endpoint: Endpoint::current(),
        })
    }

    /// Single-shot snapshot. The route handlers call this each request
    /// Fire-and-forget command — open the socket, write one
    /// [`ClientMessage`], close. The daemon's downstream side-effects
    /// (RecordingStarted, ChannelsUpdated, …) arrive via the
    /// `/events` SSE stream the browser is already subscribed to, so
    /// no per-call response is needed. (W1.)
    pub async fn send_command(&self, msg: ClientMessage) -> Result<()> {
        let mut stream = IpcStream::connect(&self.endpoint)
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
    /// socket, or that are expected to observe an IPC failure (e.g.
    /// `telemetry::telemetry_handler`, or an auth check that must run
    /// before any IPC call).
    ///
    /// `pub(crate)`, not `pub`: this crate's `tests/routes.rs` integration
    /// suite never calls it directly — it goes through the fully-`pub`
    /// `AppState::test_state`, which lives inside this crate and can see
    /// `pub(crate)` items just fine. Keeping this constructor
    /// crate-private stops a test-only need from committing it to the
    /// library's public API.
    pub(crate) fn disconnected() -> Self {
        Self {
            endpoint: Endpoint::Path(PathBuf::from("/nonexistent-strivo-test-socket")),
        }
    }

    /// An `IpcClient` dialling a caller-chosen socket, for a test that stands
    /// up its own daemon. `pub(crate)` for the same reason as
    /// [`Self::disconnected`]: not part of the public API.
    pub(crate) fn at(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    /// — fast enough that we don't bother caching at the web layer.
    pub async fn snapshot(&self) -> Result<ServerMessage> {
        ipc::fetch_snapshot(&self.endpoint)
            .await
            .context("fetch daemon snapshot")
    }

    /// Persistent event stream. Yields one [`DaemonEvent`] per
    /// broadcast. Terminates when the daemon socket closes; callers
    /// (e.g. the SSE endpoint) reconnect as needed.
    pub fn events(&self) -> Pin<Box<dyn Stream<Item = Result<DaemonEvent>> + Send>> {
        let endpoint = self.endpoint.clone();
        Box::pin(try_stream! {
            let stream = IpcStream::connect(&endpoint)
                .await
                .context("connect to daemon socket for /events")?;
            let (reader, mut writer) = tokio::io::split(stream);
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
