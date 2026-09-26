# StriVo

**A self-hosted live-stream PVR for the channels you follow.**

StriVo watches Twitch, YouTube, and Patreon, records the broadcasts you choose,
and keeps the resulting library available in a local web interface. It is built
for a durable personal archive: monitoring, scheduled capture, back-catalog
pulls, browser playback, and the practical repair tools around a real media
library—not a hosted streaming service.

[![CI](https://github.com/revoydotdev/strivo/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/revoydotdev/strivo/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/revoydotdev/strivo?sort=semver&display_name=tag)](https://github.com/revoydotdev/strivo/releases)
[![MSRV](https://img.shields.io/badge/MSRV-1.88%2B-orange?logo=rust&logoColor=white)](Cargo.toml)
[![License](https://img.shields.io/github/license/revoydotdev/strivo?color=blue)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS%20%7C%20Windows-1f6feb?logo=linux&logoColor=white)](#platforms)

> **Alpha software — latest release: 0.8.0.** Configuration, daemon IPC, and
> the dynamic-plugin ABI are not yet stable. Read the [changelog](CHANGELOG.md)
> before upgrading, and keep a copy of your configuration and recordings.

## Release boundary

The default build is the focused **PVR edition**. It contains the capture,
library, scheduling, monitoring, playback, and web-control surfaces.

The repository contains an experimental Creator Edition code path for internal
development and test coverage. It is **not released or supported**: no Creator
binary, Docker image, licence service, activation, trial, or purchase path is
available. Tagged archives and the Docker image are PVR-only. Do not rely on
Creator functionality for production work.

## What you can do today

- Monitor configured Twitch, YouTube, and Patreon channels and record when
  they are live; use per-channel auto-recording, capture profiles, schedules,
  and concurrency / disk safeguards.
- Pull a creator's back catalog into the same library, with deduplication.
- Browse, filter, seek, and play completed recordings in the local web UI.
  The recording pipeline persists its journal, recovers interrupted jobs on
  restart, and can normalize browser-hostile MPEG-TS captures to Matroska.
- Keep capture operations running as a foreground process or a user service,
  and check the daemon, media tools, paths, and configured platforms from the
  command line.
- Creator/research tooling is not available in a released build.

## Platforms

| Service | Discovery and access | Capture path |
| --- | --- | --- |
| Twitch | Configured account credentials; polling and EventSub live notifications | `streamlink` and `ffmpeg` |
| YouTube | Configured OAuth credentials; optional WebSub callback and browser cookies for restricted content | `yt-dlp` and `ffmpeg` |
| Patreon | Configured credentials and a signed-in browser session for pledged-creator video posts | `yt-dlp` |

Access to subscriber-, member-, or pledge-only material depends on the account
and credentials you supply. StriVo does not provide access to streams you are
not entitled to watch.

### Operating systems

| OS | Runtime support | Service integration |
| --- | --- | --- |
| Linux | Supported | `systemd --user` via `strivo enable` |
| macOS | Supported | `launchd` LaunchAgent via `strivo enable` |
| Windows | Supported | Task Scheduler logon task via `strivo enable` |

The 0.6.0 release introduced Windows support, including named-pipe daemon IPC
and graceful recording finalization. It has had less field use than the Unix
paths; see [Known limitations](#known-limitations).

## Quick start

Install the external tools first, then let StriVo verify them:

| Tool | Used for |
| --- | --- |
| `ffmpeg` and `ffprobe` | Recording, finalization, and inspection |
| `streamlink` | Twitch stream resolution |
| `yt-dlp` | YouTube and Patreon resolution |
| `mpv` | Live playback |

### From a release

Download the archive for your platform from [GitHub Releases](https://github.com/revoydotdev/strivo/releases), verify its accompanying SHA-256 manifest, put the executable on your `PATH`, then run:

```bash
strivo doctor
strivo
```

Release archives are produced for x86_64 Linux, macOS, and Windows. The Linux
and macOS archives also include shell completions and a man page.

### From source

The source build requires Rust 1.88 or newer, in addition to the external
tools above.

```bash
git clone https://github.com/revoydotdev/strivo.git
cd strivo
cargo build --release --locked
./target/release/strivo doctor
./target/release/strivo
```

To install from a checkout under `~/.local`, use the maintained installer:

```bash
scripts/install.sh --check
scripts/install.sh --edition pvr
```

It supports `--prefix`, `--debug`, and `--uninstall`; it does not remove user
configuration when uninstalling. See `scripts/install.sh --help` for the full
contract.

### Docker

The official image packages the PVR edition and its required media tools.

```bash
docker run -d --name strivo \
  -p 8181:8181 \
  -v strivo_recordings:/recordings \
  -v strivo_config:/config \
  ghcr.io/revoydotdev/strivo:latest
```

Open <http://localhost:8181>. The image runs the daemon and web server as two
supervised processes and exposes a health endpoint. For credentials, volumes,
the healthcheck, read [the Docker guide](docs/DOCKER.md).

## First launch and everyday use

Running `strivo` starts the daemon when needed and serves the web UI at
<http://127.0.0.1:8181>. Use the browser to configure platforms, add channels,
set recording preferences, and work with the library.

For member-only YouTube or Patreon content, the CLI can create a private cookie
jar from a signed-in browser session:

```bash
strivo setup cookies youtube --browser firefox
strivo setup cookies patreon --browser firefox --profile Default
```

The command delegates browser-cookie extraction to `yt-dlp`; close the browser
and retry if its cookie database is locked. Do not commit or share the generated
cookie files.

### Daemon and service modes

```bash
strivo                     # daemon + local web UI in one process
strivo daemon              # foreground daemon only
strivo serve --bind 127.0.0.1:8181  # web UI for an already-running daemon

strivo enable              # install and start a per-user service
strivo status              # exits 3 when the daemon is not running
strivo disable             # stop and remove the installed service
```

On Linux, `enable` writes and starts a `systemd --user` unit. macOS does not
ship systemd, so manage `strivo` or `strivo daemon` with your own supervisor.
On Windows, `enable` registers a Task Scheduler logon task; unlike the systemd
unit, that task does not restart a crashed daemon. `strivo enable --daemon-only`
keeps the Unix service to the daemon alone for a split deployment.

### Useful commands

```bash
strivo doctor                         # external tools and configured platforms
strivo config path                    # resolved configuration location
strivo config list                    # inspect settings
strivo log tail --lines 100           # follow the log
strivo search "launch stream"         # search the library
strivo pull youtube:UCxxxxxxxx        # pull a configured creator's catalog
strivo repair containers              # preview container repairs
strivo repair containers --apply      # perform them
```

Run `strivo --help` for the complete CLI, including import, chapter, thumbnail,
merge, Twitch Rewind, shell-completion, and man-page commands.

## Data, configuration, and privacy

StriVo is self-hosted: recordings, the local journal, configuration, and the
web interface run on your machine or server. It still contacts the platforms
and media tools needed for the operations you request. The web API uses an API
key and browser session controls; the Docker health endpoint is deliberately
unauthenticated so a container orchestrator can probe readiness.

Use `strivo config path` and `strivo log path` rather than assuming a path.
On a standard Linux installation, configuration is under
`~/.config/strivo/`, logs and runtime state follow the XDG directories, and
recordings default to `~/Videos/StriVo`. The detailed path map, first-run flow,
and troubleshooting live in [First run](docs/FIRST-RUN.md). For every
configuration field and default, start with the annotated
[`config.toml.example`](config.toml.example).

## Architecture

```text
Twitch / YouTube / Patreon
             │
             ▼
  monitor + schedules ──► recording manager ──► local media library
             │                    │                       │
             │                    └── journal / recovery   ├── browser playback
             │                                             └── search / repair
             ▼
        daemon IPC ◄──────────── web UI

```

The Rust workspace keeps `strivo-core` (platforms, configuration, monitoring,
recording, IPC) independent from the binary and optional first-party creator
plugins. The web UI communicates with the daemon over its local IPC transport:
Unix sockets on Unix-like systems and a named pipe on Windows.

## Known limitations

- This is alpha software. Configuration, IPC, and the third-party dynamic
  plugin ABI may change before 1.0.
- Windows support is new and less exercised. Its Task Scheduler integration
  does not provide crash restart; use an external supervisor if that matters.
- A daemon crash marks an in-flight recording as interrupted on the next
  startup, but does not resume the former capture process.
- Dynamic third-party plugins must be compiled with the exact toolchain and
  StriVo build that loads them. They are not recommended for end users during
  the alpha; see [the plugin manifest documentation](docs/PLUGIN-MANIFEST.md).

## Documentation and contribution

- [First run](docs/FIRST-RUN.md) — setup, paths, logs, and common failures
- [Docker](docs/DOCKER.md) — container process model, credentials, and volumes
- [Creator Edition release hold](docs/LICENCE-BACKEND-DEPLOY.md) — unavailable
  experimental work and the conditions required to reopen it
- [Project roadmap](ROADMAP.md) and [changelog](CHANGELOG.md) — shipped work, constraints, and migrations

Contributions are welcome; start with [CONTRIBUTING.md](CONTRIBUTING.md).
Please report vulnerabilities privately according to [SECURITY.md](SECURITY.md),
not in a public issue.

## License and credits

StriVo is released under the [MIT License](LICENSE).

The web UI's topbar icons are vendored from
[EliverLara/candy-icons](https://github.com/EliverLara/candy-icons)
(GPL-3.0) by Eliver Lara. Their upstream license and per-icon attribution are
kept in `crates/strivo-web/assets/icons/candy/`.

Three browser libraries are vendored pre-built under
`crates/strivo-web/assets/vendor/` and loaded on demand — there is no bundler or
JavaScript build step. Versions, source URLs, SHA-256 sums, and the full license
texts are in
[`crates/strivo-web/assets/vendor/PROVENANCE.md`](crates/strivo-web/assets/vendor/PROVENANCE.md).

| Library | Version | License | Used by |
|---|---|---|---|
| [mediabunny](https://www.npmjs.com/package/mediabunny) | 1.60.0 | MPL-2.0 | Creator timeline media handling |
| [@antv/x6](https://www.npmjs.com/package/@antv/x6) | 3.1.8 | MIT | Creator pipeline node graph |
| [wavesurfer.js](https://www.npmjs.com/package/wavesurfer.js) | 8.0.1 | BSD-3-Clause | Creator audio waveform |

They are Creator-only dependencies: `build.rs` keeps them out of PVR builds
entirely, so a released PVR binary neither embeds nor serves them.
