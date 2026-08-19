# Daemon mode

strivo runs as a single foreground TUI by default. For unattended recording
on a server, or to keep monitoring alive while you close the terminal, run
the daemon in the background and attach a TUI client when you want one.

> **Daemon mode is Unix-only in 0.3.0.** Linux and macOS work; Windows
> users can run the TUI as a self-contained foreground process but cannot
> attach to a daemon yet — the IPC layer uses Unix sockets. Named-pipe
> support is on the roadmap.

## Lifecycle

```bash
strivo daemon start      # spawn the background service
strivo status            # report running / not running, pid, socket
strivo daemon stop       # graceful shutdown (SIGTERM)
strivo daemon install    # write a systemd --user unit file
```

`strivo daemon start` forks a background process that opens the IPC socket
and begins polling configured channels. When the daemon is running,
launching `strivo` (no subcommand) connects as a TUI client; multiple
clients can attach concurrently.

Shutdown is graceful: `stop` sends SIGTERM, the daemon finishes any
in-flight network requests, snapshots its recording journal, and unlinks
the socket and pid file. Active ffmpeg processes are **not** killed —
they continue writing to disk, but the post-recording event chain
(transcribe, archive) is interrupted and resumes on the next start.

## Socket and pid

| Resource | Default path |
|----------|--------------|
| Unix socket | `${XDG_RUNTIME_DIR:-~/.cache/strivo}/strivo/daemon.sock` |
| Pid file | `${XDG_RUNTIME_DIR:-~/.cache/strivo}/strivo/daemon.pid` |
| Log | `~/.local/state/strivo/strivo.log` |

Stale sockets and pid files are swept on start; a pid is treated as stale
when `kill(pid, 0)` succeeds but `connect(2)` to the socket fails.

The socket is created with the process umask (typically `0600`), so only
the user that started the daemon can attach.

## systemd integration

```bash
strivo daemon install
systemctl --user enable --now strivo.service
journalctl --user -u strivo -f
```

The generated unit is a `Type=simple` user service that execs
`strivo daemon start` and restarts on failure with a 30-second back-off.
It does **not** require `linger`; if you want the daemon to survive logout,
enable linger separately:

```bash
loginctl enable-linger "$USER"
```

## IPC protocol

The protocol is a length-prefixed JSON stream defined in
[`src/ipc.rs`](../src/ipc.rs). The two top-level frames are
`ClientMessage` (TUI → daemon) and `ServerMessage` (daemon → TUI),
both `#[derive(Serialize, Deserialize)]` so they round-trip via
`serde_json`. There is no version handshake yet; the protocol is
considered unstable until 0.5.0 and may break between alpha releases.
Pin the daemon and the TUI to the same commit if you build either from
source.

## Health checks

```bash
strivo status               # exits 0 if running, 3 if not
```

Use that exit code in monitoring scripts. For Prometheus / observability
integration the recommended pattern today is to tail `strivo.log` and
match on `daemon: ` lines — a metrics endpoint is not yet exposed.

## Troubleshooting

- **`failed to bind socket: Address already in use`** — a previous
  `strivo daemon` crashed without unlinking. Run `strivo daemon stop`
  (it sweeps stale sockets even when the daemon is gone) and retry.
- **TUI shows "daemon disconnected — retrying in 5s"** — the daemon
  exited or its socket disappeared. The TUI reconnects with exponential
  back-off (1 / 2 / 5 / 10 / 30 s); check `journalctl --user -u strivo`
  for the crash reason.
- **`failed to register SIGTERM handler`** — extremely rare; usually a
  seccomp filter blocks `sigaction(2)`. Run the daemon without the
  filter or report the platform.
