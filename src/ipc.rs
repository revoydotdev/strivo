use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::events::DaemonEvent;
use crate::platform::{ChannelEntry, PlatformKind};
use crate::recording::job::RecordingJob;
use crate::recording::RecordingCommand;

/// Wire protocol version.  Bump when a backward-incompatible change is made;
/// peers log a warning when the versions differ but continue to operate.
pub const IPC_PROTOCOL_VERSION: u32 = 2;

/// Messages sent from TUI client to daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Request full state snapshot, with protocol version for the handshake.
    /// Older peers that still send the bare `"Hello"` string (unit-variant
    /// wire form) are caught by the `Unknown` catch-all below; the daemon
    /// logs the mismatch and continues.
    Hello {
        /// Protocol version sent by the client.  Old clients (pre-versioning)
        /// omit this field; `#[serde(default)]` gives them version 0.
        #[serde(default)]
        version: u32,
    },
    /// Forward a recording command
    Recording(RecordingCommand),
    /// Trigger immediate channel poll
    PollNow,
    /// Live-update the channel-poll interval without a restart (item 14b).
    /// Seconds; the daemon clamps to a sane minimum.
    SetPollInterval(u64),
    /// Graceful daemon shutdown
    Shutdown,
    /// Dispatch an actions-popup verb to a plugin via the host
    /// `PluginRegistry::dispatch_verb`. (Part 11 W2.)
    PluginRpc {
        plugin: String,
        verb: String,
        /// Recording UUIDs the verb should act on (selection set in
        /// the TUI; cursor row in single-select).
        #[serde(default)]
        selection: Vec<Uuid>,
        /// Optional JSON payload for plugin-specific args. The
        /// plugin parses or ignores; the host doesn't inspect it.
        #[serde(default)]
        payload: serde_json::Value,
    },
    /// Submit an executable Creator DAG to the daemon-owned scheduler.
    SubmitPipeline(crate::pipeline::Pipeline),
    CancelPipeline {
        pipeline_id: crate::pipeline::PipelineId,
    },
    RetryPipelineStage {
        stage_id: crate::pipeline::StageId,
    },
    /// Start or stop a per-channel bulk back-catalog download (task #71).
    BulkDownload {
        channel_id: String,
        channel_name: String,
        platform: PlatformKind,
        action: BulkAction,
        /// Optional YouTube playlist scope (task #73). None = whole channel.
        #[serde(default)]
        playlist_id: Option<String>,
    },
    /// Request the playlists for a YouTube channel, to populate the
    /// bulk-download scope picker (task #73). Answered asynchronously
    /// with DaemonEvent::PlaylistList.
    ListPlaylists {
        channel_id: String,
    },
    /// Pull a single Patreon video post on demand (task #75 — webui
    /// equivalent of the TUI's PullPatreonPost). The daemon builds the
    /// output path from its config, so the webui doesn't have to.
    PatreonPull {
        embed_url: String,
        creator_name: String,
        post_title: String,
    },
    /// Pull a single VOD / past broadcast on demand from the webui's
    /// channel-detail pane. Wraps `RecordingCommand::DownloadVod` so the
    /// daemon picks the platform-correct cookies path and builds the
    /// output path from config (mirrors `PatreonPull`).
    DownloadVod {
        url: String,
        channel_name: String,
        platform: PlatformKind,
        /// Display title for the slug; falls back to channel + date.
        #[serde(default)]
        post_title: Option<String>,
    },
    /// Minimal "start a live capture" envelope used by the webui. The
    /// daemon translates this through `intents::start_recording` so
    /// cookies and the transcode default are resolved against the
    /// daemon's `AppConfig` — the webui has no config in its route
    /// handlers and shouldn't reach for it. Mirrors the `DownloadVod`
    /// shape above: client sends intent, daemon owns policy.
    ///
    /// The fat `Recording(RecordingCommand::Start { … cookies_path,
    /// transcode … })` envelope stays on the wire for the legacy TUI;
    /// it goes away with TUI deletion (task #13).
    Start {
        channel_id: String,
        channel_name: String,
        #[serde(default)]
        display_name: Option<String>,
        platform: PlatformKind,
        #[serde(default)]
        stream_title: Option<String>,
        #[serde(default)]
        thumbnail_url: Option<String>,
        #[serde(default)]
        from_start: bool,
        /// `None` defers to `config.effective_transcode(platform,
        /// channel_id)`. The webui sets `Some(true|false)` based on
        /// the user's checkbox.
        #[serde(default)]
        transcode_override: Option<bool>,
    },
    /// Hard-delete a finished or errored recording: move the file into the
    /// 7-day trash and drop the jobs.db row. Active recordings are rejected;
    /// the webui must Stop them first.
    DeleteRecording {
        job_id: Uuid,
    },
    /// Bulk-delete every recording whose state is `failed` or `interrupted`.
    /// Same trash-then-drop semantics as `DeleteRecording`.
    ClearErroredRecordings,
    /// Request a channel's recent VODs (live broadcasts + uploads) for the
    /// webui channel-detail pane. Answered asynchronously with
    /// DaemonEvent::ChannelVods.
    FetchChannelVods {
        channel_id: String,
        platform: PlatformKind,
    },
    /// Resolve a human-entered identifier (Twitch login, YouTube/Patreon id)
    /// to a channel id for the Add-Channel wizard (task #19). Answered
    /// asynchronously with DaemonEvent::ChannelResolved.
    ResolveChannel {
        platform: PlatformKind,
        query: String,
    },
    /// Forward-compatibility catch-all: an unknown variant from a newer peer
    /// deserializes here so the daemon doesn't hard-error.  Handled as a
    /// logged no-op.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BulkAction {
    Start,
    Stop,
}

/// Messages sent from daemon to TUI client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Full state snapshot (sent in response to Hello)
    StateSnapshot {
        /// Daemon's protocol version.  Old daemons (pre-versioning) omit this
        /// field; `#[serde(default)]` gives them version 0.
        #[serde(default)]
        version: u32,
        channels: Vec<ChannelEntry>,
        recordings: HashMap<Uuid, RecordingJob>,
        twitch_connected: bool,
        youtube_connected: bool,
        patreon_connected: bool,
        pending_auth: Option<(PlatformKind, String, String)>,
        /// Latest Patreon snapshot (creators + their video posts), cached
        /// from the most recent PatreonState event so a client connecting
        /// between polls sees Patreon immediately instead of waiting up to
        /// a full poll interval. Defaults empty for older clients.
        #[serde(default)]
        patreon_creators: Vec<ChannelEntry>,
        #[serde(default)]
        patreon_posts: Vec<crate::platform::patreon::PatreonPost>,
    },
    /// Incremental update event
    Event(DaemonEvent),
    /// Forward-compatibility catch-all: an unknown variant from a newer daemon
    /// deserializes here instead of hard-erroring the client.
    #[serde(other)]
    Unknown,
}

/// Socket path for the daemon
pub fn socket_path() -> std::path::PathBuf {
    crate::config::AppConfig::state_dir().join("strivo.sock")
}

/// PID file path for the daemon
pub fn pid_path() -> std::path::PathBuf {
    crate::config::AppConfig::state_dir().join("strivo.pid")
}

/// Write a message as newline-delimited JSON
pub fn encode_message<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

/// Check if the daemon is running.
///
/// `kill(pid, 0)` alone can produce false positives: PIDs get recycled
/// after a crash, and the recorded PID may belong to an unrelated
/// process. Before trusting the PID we also confirm the Unix socket is
/// bound *and* still accepting connections. A blocking connect with a
/// ~200 ms budget is the cheapest definitive liveness probe; a dead
/// socket rejects `connect(2)` with `ECONNREFUSED` almost instantly.
pub fn is_daemon_running() -> bool {
    let pid_file = pid_path();
    let sock_file = socket_path();
    if !pid_file.exists() || !sock_file.exists() {
        return false;
    }
    let Ok(pid_str) = std::fs::read_to_string(&pid_file) else {
        return false;
    };
    let Ok(pid) = pid_str.trim().parse::<i32>() else {
        return false;
    };
    // Safety: kill(pid, 0) is the canonical reachability probe — no signal
    // is delivered.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    // Cross-check: actually connect to the socket. If the daemon crashed
    // and the PID got recycled, the socket file may still sit on disk
    // but nothing is accept(2)ing on it.
    match std::os::unix::net::UnixStream::connect(&sock_file) {
        Ok(stream) => {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            true
        }
        Err(_) => false,
    }
}

/// Remove stale pid + socket files left by a previous daemon that
/// crashed. Safe to call at the start of every `daemon` command.
pub fn sweep_stale_files() {
    let pid_file = pid_path();
    let sock_file = socket_path();
    let stale_pid = match std::fs::read_to_string(&pid_file) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(pid) => unsafe { libc::kill(pid, 0) != 0 },
            Err(_) => true,
        },
        Err(_) => !pid_file.exists(),
    };
    if stale_pid {
        let _ = std::fs::remove_file(&pid_file);
        let _ = std::fs::remove_file(&sock_file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_client_message_deserializes_to_unknown() {
        // A new *unit* variant from a future release must not crash an older
        // peer.  In serde's externally-tagged JSON encoding, a unit variant
        // is a bare JSON string (`"PeerReady"`), not a map.  The
        // `#[serde(other)]` catch-all captures this case; struct variants
        // with a non-null payload are not caught and are an inherent
        // limitation of externally-tagged enums.
        let json = r#""FutureUnitVariant""#;
        let msg: ClientMessage =
            serde_json::from_str(json).expect("should parse unknown unit variant as Unknown");
        assert!(
            matches!(msg, ClientMessage::Unknown),
            "unexpected variant: {msg:?}"
        );
    }

    #[test]
    fn unknown_server_message_deserializes_to_unknown() {
        // Same as above for ServerMessage.
        let json = r#""FutureUnitVariant""#;
        let msg: ServerMessage =
            serde_json::from_str(json).expect("should parse unknown unit variant as Unknown");
        assert!(
            matches!(msg, ServerMessage::Unknown),
            "unexpected variant: {msg:?}"
        );
    }

    #[test]
    fn hello_version_roundtrip() {
        let msg = ClientMessage::Hello {
            version: IPC_PROTOCOL_VERSION,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let decoded: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(decoded, ClientMessage::Hello { version: v } if v == IPC_PROTOCOL_VERSION),
            "decoded: {decoded:?}"
        );
    }

    #[test]
    fn hello_missing_version_defaults_to_zero() {
        // An old client sends {"Hello":{}} with no version field; the new
        // daemon must parse this as version 0.
        let json = r#"{"Hello":{}}"#;
        let msg: ClientMessage = serde_json::from_str(json).expect("struct variant with no fields");
        assert!(
            matches!(msg, ClientMessage::Hello { version: 0 }),
            "expected Hello {{ version: 0 }}, got: {msg:?}"
        );
    }

    #[test]
    fn state_snapshot_missing_version_defaults_to_zero() {
        // An old daemon omits the version field; the new client parses it as 0.
        let json = r#"{"StateSnapshot":{"version":0,"channels":[],"recordings":{},"twitch_connected":false,"youtube_connected":false,"patreon_connected":false,"pending_auth":null,"patreon_creators":[],"patreon_posts":[]}}"#;
        let msg: ServerMessage = serde_json::from_str(json).expect("deserialize snapshot");
        assert!(
            matches!(msg, ServerMessage::StateSnapshot { version: 0, .. }),
            "expected version 0, got: {msg:?}"
        );
    }
}
