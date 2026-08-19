# strivo

Self-hosted live-stream PVR with a web UI — "Sonarr/Radarr for live streams." Monitor channels across Twitch, YouTube, and Patreon, automatically record when they go live, finalize them into a clean library, and play back in the browser.

strivo ships in two editions from one codebase. The default build is the **pure PVR**. **Creator Edition** (`--features creator`) adds the creator/analytics toolkit — transcription, clip discovery, the EDL editor, and a domain-agnostic stream→signal analytics engine. See [ROADMAP.md](./ROADMAP.md) for the PVR roadmap and the Creator Edition trajectory.

> **TUI removed.** The original ratatui-based TUI was retired; the web UI
> is the only supported frontend. See [CHANGELOG.md](./CHANGELOG.md) for
> the inventory.

[![CI](https://github.com/revoydotdev/strivo/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/revoydotdev/strivo/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/revoydotdev/strivo?sort=semver&display_name=tag)](https://github.com/revoydotdev/strivo/releases)
[![MSRV](https://img.shields.io/badge/MSRV-1.75%2B-orange?logo=rust&logoColor=white)](Cargo.toml)
[![License](https://img.shields.io/github/license/revoydotdev/strivo?color=blue)](LICENSE)
[![AUR](https://img.shields.io/aur/version/strivo?label=AUR&logo=archlinux&logoColor=white)](https://aur.archlinux.org/packages/strivo)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-1f6feb?logo=linux&logoColor=white)](#platform-support)
[![Made with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org)

> **Status: alpha (0.5.0).** The configuration format, daemon IPC protocol, and plugin ABI
> are still unstable and will keep changing until 1.0. Expect to re-edit `config.toml`
> across releases. See [ROADMAP.md](./ROADMAP.md) for the stability timeline and
> [CHANGELOG.md](./CHANGELOG.md) for migration notes.

---


## What it does

strivo runs a background daemon and serves a web UI on `localhost:8181`.
The daemon watches the channels you tell it to; when one goes live, it
records the broadcast through ffmpeg (resolving the playable URL via
streamlink or yt-dlp) and notifies you. You browse recordings, play them
back in the browser, run optional plugins (Whisper transcription, gallery
archiver), and search across your archive — all from the SPA.

`strivo` with no arguments starts the daemon and the webui in one process.
For systemd setups, `strivo daemon` runs the daemon alone and
`strivo serve` runs the webui alone; both talk over the same daemon Unix socket.

### Platform support

| Platform | Auth | Monitoring | Recording | Notes |
|----------|------|------------|-----------|-------|
| Twitch | OAuth device flow | Followed-channel polling | `ffmpeg` + `streamlink` | Sub-only streams via OAuth token passthrough |
| YouTube | OAuth + Data API v3 | Live-broadcast detection | `ffmpeg` + `yt-dlp` | Cookie-based auth for members-only streams |
| Patreon | Membership API | Post / stream detection | `yt-dlp` | Subscription-tier extraction |

### Operating systems

| OS | Web UI | Daemon | Status |
|----|--------|--------|--------|
| Linux (x86_64) | ✅ | ✅ | Primary target; CI-gated |
| macOS (aarch64 / x86_64) | ✅ | ✅ | Builds and runs; manual testing pre-release |
| Windows | ❌ | ❌ | Daemon IPC uses Unix sockets — Windows named-pipe transport is on the roadmap |

## Features

**Core**
- Multi-platform channel monitoring with configurable poll intervals
- Automatic recording when channels go live (per-channel toggle)
- Live playback through mpv without downloading first
- Cron-based recording schedules
- Desktop notifications on go-live events
- Configurable filename templates (`{channel}_{date}_{title}.mkv`)
- Retry with exponential backoff on failed recordings

**Web UI (SPA)**
- Channel rail with auto-record toggles, live-status and platform indicators
- Recordings browser — sortable, filterable, in-browser playback with seek
- Schedule / monitor with capture-limit safety knobs and a disk-free gauge
- Settings panel — edit config and manage plugins without leaving the browser
- Live log viewer (tail-follow, regex filter)
- First-run setup flow for platform credentials
- Multiple color themes

**Daemon mode**
- Background service via Unix-socket IPC
- One or more web clients can attach to a running daemon
- `strivo daemon install` writes a systemd user unit

**Plugins**
- **Crunchr** — Voxtral via OpenRouter (default, $0.003/min) / Mistral direct (diarization) / WhisperX local (self-hosted GPU diarization) / self-hosted Voxtral / Whisper CLI transcription, in-browser speaker editor, SRT/VTT export with `mkvmerge` soft-sub embedding, tandem-mode auto-trigger, SQLite storage
- **Archiver** — organizes recordings by channel, renders gallery views

First-party plugins live in-tree under [`crates/strivo-plugins/`](./crates/strivo-plugins) —
the former separate `strivo-plugins` repo was folded into the workspace.
See [docs/PLUGIN-MANIFEST.md](./docs/PLUGIN-MANIFEST.md) for ABI notes
and the plugin loader contract.

## Tech stack

- **Language:** Rust 1.75+
- **Web UI:** SPA served from `strivo-web` (axum + a vanilla-JS single-file SPA)
- **Recording:** ffmpeg, streamlink, yt-dlp
- **Playback:** mpv
- **Transcription:** Voxtral via OpenRouter (default), Mistral API (with diarization), WhisperX + pyannote (self-hosted GPU diarization, two-stage VRAM unload for 8 GB cards), self-hosted Voxtral (vLLM / RunPod), Whisper CLI
- **Subtitling:** VTT + SRT sidecars, optional `mkvmerge` soft-sub mux back into the recording
- **Storage:** SQLite (bundled via `rusqlite`) for transcripts and journal
- **IPC:** Unix domain sockets (daemon / client)
- **Config & secrets:** TOML on disk, OS keyring for credentials

## Installation

### Prerequisites

- **Rust** 1.75+ to build from source
- **ffmpeg** — recording
- **mpv** — playback
- **streamlink** — Twitch stream resolution
- **yt-dlp** — YouTube / Patreon stream resolution

### Arch Linux (AUR)

```bash
paru -S strivo      # or: yay -S strivo
strivo doctor       # verify ffmpeg / mpv / streamlink / yt-dlp are installed
strivo              # starts daemon + webui on http://127.0.0.1:8181
```

### From source

```bash
git clone https://github.com/revoydotdev/strivo.git
cd strivo
scripts/install.sh --check                 # tailored prerequisite guidance
scripts/install.sh --edition pvr           # focused live-stream PVR
scripts/install.sh --edition creator       # research + transcription edition
strivo doctor
strivo
```

The installer uses `~/.local` by default, never overwrites your configuration,
and supports `--prefix`, `--debug`, and `--uninstall`. The default edition is
the pure PVR. The `creator` feature adds the
transcription/analysis/editor toolkit (the `strivo-plugins` host and the
in-tree tool crates); see [the edition split in ROADMAP.md](./ROADMAP.md#the-edition-split--shipped).

### Dev install (current checkout → `~/.local/bin/strivo`)

For hacking on a clone, `install-dev.sh` remains a shortcut for
`install.sh --edition creator`:

```bash
scripts/install-dev.sh                # release build
scripts/install-dev.sh --debug        # faster iteration build
scripts/install-dev.sh --uninstall    # remove installed bits (config kept)
```

The script:

- builds `strivo-bin`, copies the binary to `~/.local/bin/strivo`,
- ships the `whisperx_diarize.py` orchestrator next to it (auto-discovered
  by the `whisperx-local` backend),
- generates bash/zsh/fish completions and a manpage into
  `~/.local/share/strivo/`.

Override the layout with `--prefix` or `STRIVO_PREFIX` (and the more specific
`STRIVO_BIN_DIR`, `STRIVO_SHARE_DIR`, and `STRIVO_MAN_DIR`).

(The previous `git submodule update --init` step is no longer needed —
the first-party plugins live in `crates/strivo-plugins/` in this repo.)

The binary lands at `target/release/strivo`. Copy it onto your `PATH`.

### Platform credentials

For YouTube or Patreon, Strivo can securely import the session from a browser
you are already signed into. It delegates browser/keyring support to yt-dlp,
stores only a private Netscape cookie jar, and writes its path to your config:

```bash
strivo setup cookies youtube --browser firefox
strivo setup cookies patreon --browser vivaldi --profile Default
```

Close the browser and retry if its cookie database is locked. Use `--force` to
refresh an expired session. Raw cookie values are never printed.

Complete the web UI's setup flow on first launch, or configure manually:

| Platform | How to get credentials |
|----------|------------------------|
| Twitch | Create an app at [dev.twitch.tv/console](https://dev.twitch.tv/console) — need `client_id` and `client_secret` |
| YouTube | Create OAuth credentials at the [Google Cloud Console](https://console.cloud.google.com/) — need `client_id` and `client_secret` |
| Patreon | Uses the membership API via browser cookies |

Credentials are stored in your OS keyring (macOS Keychain, GNOME Keyring /
Secret Service, Windows Credential Manager).

Strivo validates Twitch OAuth hourly and refreshes stale or near-expiry tokens
automatically. `strivo doctor` performs the same repair immediately and reports
whether the app secret or refresh token was rejected. When Twitch requires
fresh user consent, the web UI surfaces its device-code login; this is the one
step Twitch does not permit Strivo to complete on your behalf.

## Usage

### Web UI

```bash
strivo
```

Starts the daemon and serves the SPA on `http://127.0.0.1:8181`. The channel
rail shows monitored channels with live-status indicators; toggle auto-record
per channel, browse and play recordings, and run plugins — all from the browser.

### Daemon

```bash
strivo daemon start     # start the background service
strivo daemon stop      # stop it
strivo daemon status    # report whether it is running
strivo daemon install   # write a systemd user unit
```

When the daemon is running, `strivo` launches as a client that connects to
the Unix socket. See [docs/DAEMON.md](./docs/DAEMON.md) for socket paths,
logging, and lifecycle details.

### CLI

```bash
strivo config list              # show all settings
strivo config get <key>         # read a value
strivo config set <key> <val>   # write a value
strivo config path              # print the config file location
strivo config reset             # restore defaults (keeps credentials)

strivo log tail [-l 100]        # live-tail the log
strivo log path                 # print the log file location
strivo log clear                # wipe the log
```

### Flags

| Flag | Description |
|------|-------------|
| `-c, --config <path>` | Custom config file |
| `-l, --log-level <level>` | `trace`, `debug`, `info`, `warn`, `error` |

`RUST_LOG` is also honoured and overrides `-l` when set.

## Configuration

Config lives at `~/.config/strivo/config.toml` (XDG-compliant — see
`strivo config path` for the resolved location on your system). A fully
annotated reference is checked in at
[`config.toml.example`](./config.toml.example); a minimal working starting
point looks like:

```toml
recording_dir = "/home/you/Videos/strivo"
poll_interval_secs = 60

[twitch]
client_id = "..."
client_secret = "..."

[youtube]
client_id = "..."
client_secret = "..."
cookies_path = "/path/to/cookies.txt"   # optional, for members-only streams

[recording]
transcode = false
filename_template = "{channel}_{date}_{title}.mkv"

[[auto_record_channels]]
platform = "twitch"
channel_id = "12345"
channel_name = "streamer_name"

[[schedules]]
platform = "twitch"
channel_id = "12345"
cron = "0 20 * * 1-5"   # weekdays at 8pm
```

## Architecture

```
Twitch / YouTube / Patreon APIs
              │
              ▼
   ┌─────────────────────────┐
   │        Monitor          │
   │  polling, go-live detect│
   └────────┬────────────────┘
            │
       ┌────▼────┐    ┌──────────┐
       │Recorder │───▶│  Plugin  │
       │ ffmpeg  │    │ Crunchr  │
       │ yt-dlp  │    │ Archiver │
       └────┬────┘    └──────────┘
            │
       ┌────▼────┐    ┌──────────┐
       │Playback │    │ Web UI   │
       │  mpv    │◀──▶│ (SPA)    │
       └─────────┘    └──────────┘
```

```
strivo/                        cargo workspace root
├── src/                       strivo-core (library crate)
│   ├── platform/              Trait-based abstraction (Twitch, YouTube, Patreon)
│   ├── monitor/               Channel polling, go-live detection
│   ├── recording/             Job lifecycle, ffmpeg / yt-dlp process management
│   ├── stream/                URL resolution via streamlink / yt-dlp
│   ├── playback/              mpv controller
│   ├── plugin/                Plugin trait, registry, lifecycle
│   ├── intents/               Recording-intent translators (Start, DownloadVod)
│   ├── events.rs              DaemonEvent — IPC broadcast envelope
│   ├── daemon.rs              Background service, Unix-socket listener
│   ├── ipc.rs                 Client-server protocol
│   └── config/                TOML config, OS-keyring integration
├── crates/strivo-bin/         Binary crate (CLI, main.rs)
└── crates/strivo-plugins/     First-party plugins (Crunchr, Archiver,
                               Insights, Editor, Viewguard)
```

The dependency graph is strictly one-way:
`strivo-core` ← `strivo-plugins` ← `strivo-bin`. The core crate has no
awareness of concrete plugins; the binary pulls both together.

## Design rationale

| Decision | Reasoning |
|----------|-----------|
| Platform trait | Adding a new service means implementing one trait — auth, polling, and recording are decoupled from platform specifics |
| Unix-socket IPC | Zero-overhead daemon / client split; the web UI is just another client and headless recording works standalone |
| Web UI as the frontend | An *arr-style SPA (`strivo serve`) talks to the daemon over the socket; the daemon runs headless so recording and scheduled captures work without a browser attached |
| Plugin event bus | Transcription and archival react to recording events without coupling to the recording pipeline |
| OS keyring | Credentials never touch disk as plaintext — uses platform-native secret storage |

## Known limitations (0.5.0 alpha)

- **Windows support is new and less exercised.** The daemon talks to its
  clients over a named pipe (`\\.\pipe\strivo`) instead of a Unix socket,
  and recordings are stopped with ffmpeg's `q` command rather than SIGINT.
  Both paths are covered by tests that run on Windows, but the platform has far
  less real-world use behind it than Linux and macOS. `strivo enable` installs
  a Task Scheduler logon task there rather than a systemd user service, and has
  no crash-restart equivalent.
- **In-flight recordings are not durable across daemon crashes.** A persisted
  journal exists for status replay, and the daemon marks orphaned jobs
  `interrupted` at startup so nothing looks falsely in flight, but it does not
  resume the ffmpeg process itself — an interrupted capture must be restarted.
- **Transcription jobs cannot be cancelled or retried** after timeout — a
  single failure currently terminates the job.
- **Plugins require same-toolchain compilation** against the exact strivo
  build that loads them. Third-party plugins are not recommended for end
  users in alpha; see
  [docs/PLUGIN-MANIFEST.md](./docs/PLUGIN-MANIFEST.md).

## Documentation

- [docs/FIRST-RUN.md](./docs/FIRST-RUN.md) — log paths, common failure modes
- [docs/DAEMON.md](./docs/DAEMON.md) — daemon lifecycle, systemd integration, socket location
- [docs/PLUGIN-MANIFEST.md](./docs/PLUGIN-MANIFEST.md) — plugin trait, ABI caveats
- [docs/PLUGIN-TEMPLATE.md](./docs/PLUGIN-TEMPLATE.md) — minimal plugin skeleton
- [docs/SETTINGS-COVERAGE.md](./docs/SETTINGS-COVERAGE.md) — which config fields are surfaced in the settings UI

## Contributing

Bug reports and small fixes are welcome — see
[CONTRIBUTING.md](./CONTRIBUTING.md) for the local-build flow and project
conventions. Security issues should be reported privately via
[SECURITY.md](./SECURITY.md), not as public issues.

## Roadmap

Roadmap, milestones, and explicit deferrals live in
[ROADMAP.md](./ROADMAP.md).

## License

[MIT](./LICENSE)

## Credits

The web UI's topbar icons are vendored from
[EliverLara/candy-icons](https://github.com/EliverLara/candy-icons)
(GPL-3.0) by Eliver Lara — see
`crates/strivo-web/assets/icons/candy/` for the upstream LICENSE and
per-icon attribution.
