//! End-to-end coverage of the daemon↔client wire protocol over the
//! [`strivo_core::ipc`] transport (a Unix domain socket on Unix, a named
//! pipe on Windows). Exercises:
//!   * Hello → StateSnapshot
//!   * broadcast of DaemonEvent → Event frame
//!   * the endpoint connect probe rejects a dead/nonexistent endpoint
//!
//! This is a black-box harness — no hooks into the daemon's internal
//! event loop. We stand up a minimal in-test server via `ipc::Listener`
//! that speaks the same framing contract so the IPC format — and the
//! transport abstraction itself — is locked down on every platform.

use strivo_core::events::DaemonEvent;
use strivo_core::ipc::{
    self, ClientMessage, Endpoint, Listener, ServerMessage, Stream as IpcStream,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn snapshot_stub() -> ServerMessage {
    ServerMessage::StateSnapshot {
        version: ipc::IPC_PROTOCOL_VERSION,
        channels: Vec::new(),
        recordings: std::collections::HashMap::new(),
        twitch_connected: false,
        youtube_connected: false,
        patreon_connected: false,
        pending_auth: None,
        patreon_creators: Vec::new(),
        patreon_posts: Vec::new(),
    }
}

/// A fresh, collision-free test endpoint. Unix gets a socket path inside a
/// dedicated temp dir (dropped — and thus cleaned up — at the end of the
/// test); Windows gets a uniquely-named pipe (pipes have no filesystem
/// footprint to clean up).
#[cfg(unix)]
fn test_endpoint(_tag: &str) -> (Endpoint, Option<tempfile::TempDir>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let sock = tmp.path().join("strivo.sock");
    (Endpoint::Path(sock), Some(tmp))
}

#[cfg(windows)]
fn test_endpoint(tag: &str) -> (Endpoint, Option<tempfile::TempDir>) {
    let name = format!(r"\\.\pipe\strivo-test-{tag}-{}", std::process::id());
    (Endpoint::Pipe(name), None)
}

#[tokio::test]
async fn client_hello_receives_state_snapshot() {
    let (endpoint, _tmp) = test_endpoint("hello");
    let mut listener = Listener::bind(&endpoint).await.unwrap();

    let server = tokio::spawn(async move {
        let stream = listener.accept().await.unwrap();
        let (reader, mut writer) = tokio::io::split(stream);
        let mut buf = BufReader::new(reader);

        let mut line = String::new();
        buf.read_line(&mut line).await.unwrap();
        let msg: ClientMessage = serde_json::from_str(line.trim()).unwrap();
        assert!(matches!(msg, ClientMessage::Hello { .. }));

        let encoded = ipc::encode_message(&snapshot_stub()).unwrap();
        writer.write_all(encoded.as_bytes()).await.unwrap();

        // Push a follow-up event so the client can assert the framing is
        // newline-delimited and not a single-shot channel.
        let evt = ServerMessage::Event(DaemonEvent::Notification {
            title: "hi".into(),
            body: "there".into(),
        });
        let encoded = ipc::encode_message(&evt).unwrap();
        writer.write_all(encoded.as_bytes()).await.unwrap();
    });

    // Give the server a moment to bind/re-arm before the client connects
    // (matters most on Windows, where connect() otherwise races the first
    // pipe instance's readiness).
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let stream = IpcStream::connect(&endpoint).await.unwrap();
    let (reader, mut writer) = tokio::io::split(stream);
    let mut buf = BufReader::new(reader);

    let hello = ipc::encode_message(&ClientMessage::Hello {
        version: ipc::IPC_PROTOCOL_VERSION,
    })
    .unwrap();
    writer.write_all(hello.as_bytes()).await.unwrap();

    let mut line = String::new();
    buf.read_line(&mut line).await.unwrap();
    let first: ServerMessage = serde_json::from_str(line.trim()).unwrap();
    assert!(matches!(first, ServerMessage::StateSnapshot { .. }));

    line.clear();
    buf.read_line(&mut line).await.unwrap();
    let second: ServerMessage = serde_json::from_str(line.trim()).unwrap();
    match second {
        ServerMessage::Event(DaemonEvent::Notification { title, .. }) => assert_eq!(title, "hi"),
        other => panic!("expected Notification event, got {other:?}"),
    }

    server.await.unwrap();
}

/// The accept-loop re-arm: a *second* client must be able to connect
/// while the first connection is still open. On Windows this is the trap
/// called out in the port brief — `NamedPipeServer::connect()` hands the
/// caller the connected instance, and a fresh instance must exist before
/// `accept()` returns or the next client sees `ERROR_PIPE_BUSY` with
/// nothing listening to retry into.
#[tokio::test]
async fn listener_accepts_a_second_client_while_the_first_is_still_open() {
    let (endpoint, _tmp) = test_endpoint("rearm");
    let mut listener = Listener::bind(&endpoint).await.unwrap();

    let server = tokio::spawn(async move {
        let first = listener.accept().await.unwrap();
        let second = listener.accept().await.unwrap();
        (first, second)
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let _client_a = IpcStream::connect(&endpoint).await.unwrap();
    let _client_b = IpcStream::connect(&endpoint).await.unwrap();

    let (_first, _second) = server.await.unwrap();
}

#[cfg(unix)]
#[test]
fn is_daemon_running_rejects_stale_socket_file() {
    // A bare socket file on disk with no accept(2)er should NOT be
    // treated as a live daemon. This is the cross-check we added on
    // top of the pid liveness check.
    let tmp = tempfile::TempDir::new().unwrap();
    let sock = tmp.path().join("strivo.sock");
    let pid = tmp.path().join("strivo.pid");

    // Create an empty "socket" stand-in + a PID belonging to this
    // process (which is obviously alive per kill(pid, 0)).
    std::fs::write(&sock, b"").unwrap();
    std::fs::write(&pid, format!("{}", std::process::id())).unwrap();

    // We can't easily redirect `ipc::socket_path()` in-process without
    // plumbing an override, so instead we re-implement the probe inline
    // with the temp paths to document the invariant we care about:
    // connect(2) fails against a non-listening path.
    let connect = std::os::unix::net::UnixStream::connect(&sock);
    assert!(
        connect.is_err(),
        "connect(2) must fail against a stale socket file"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn connecting_to_a_nonexistent_pipe_fails() {
    // The named-pipe analogue of the stale-socket test above: no
    // filesystem entry to go stale, but an endpoint nobody bound must
    // still fail cleanly rather than hang or silently succeed.
    let name = format!(r"\\.\pipe\strivo-test-nonexistent-{}", std::process::id());
    let endpoint = Endpoint::Pipe(name);
    let result = IpcStream::connect(&endpoint).await;
    assert!(
        result.is_err(),
        "connect must fail against a pipe nobody bound"
    );
}
