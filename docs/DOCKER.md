# Running strivo in Docker

strivo ships an official image, published to GHCR on every tagged release
(`.github/workflows/docker.yml`). This page covers the quick start, what
lives in the image, and the two things Docker changes about a normal
install: process supervision and OS-keyring credential storage.

## Quick start

```bash
git clone https://github.com/revoydotdev/strivo.git
cd strivo
docker compose up -d
```

Or without cloning, using the published image:

```bash
docker run -d --name strivo \
  -p 8181:8181 \
  -v strivo_recordings:/recordings \
  -v strivo_config:/config \
  ghcr.io/revoydotdev/strivo:latest
```

Then open `http://localhost:8181`. The first-run setup flow is the same
one described in [FIRST-RUN.md](./FIRST-RUN.md); the container's console
log (`docker logs strivo`) prints the generated `X-Api-Key` the SPA needs
on first load, the same as running `strivo serve` outside a container.

```bash
docker compose logs strivo | grep X-Api-Key
```

## What's in the image

Multi-stage build (`Dockerfile`): a `rust:1-bookworm` build stage compiles
the release binary, and a `debian:bookworm-slim` runtime stage carries only
what's needed to run it:

- the `strivo` binary itself — **default (free) PVR edition**, i.e. plain
  `cargo build --release` with no `--features creator`, matching the
  release tarballs and the AUR package. Build with
  `--build-arg EDITION=creator` for the Creator Edition (transcription/
  analytics toolkit); the published GHCR tags carry this as a `-creator`
  suffix, never as `latest` — see the workflow.
- `ffmpeg` / `ffprobe`, `mpv`, `streamlink`, `yt-dlp` — the external tools
  `strivo doctor` checks for (`crates/strivo-bin/src/main.rs`). `yt-dlp`
  and `streamlink` are installed via `pip` into an isolated venv rather
  than Debian's apt packages, which lag behind upstream noticeably for
  both (stream-site breakage is common and gets fixed upstream fast).
- `gnome-keyring` + `dbus` — a headless Secret Service for OS-keyring
  credential storage; see "Credentials" below.
- `tini` as PID 1 (proper signal forwarding/zombie reaping) and `gosu` to
  drop from root to the `strivo` user after fixing volume ownership.

Image size (pvr edition, uncompressed): **~1.1 GB** — ffmpeg and mpv
account for most of it; there's no way around carrying both since both are
required by `strivo doctor`.

## Process model: one container, two processes

The container runs `strivo daemon` and `strivo serve --bind 0.0.0.0:8181`
as two supervised processes (`strivo-run.sh`), not the bare `strivo`
(no-subcommand) entry point you'd use outside Docker. That entry point
spawns the daemon in-process and serves the web UI together, but it
hardcodes the web UI to bind `127.0.0.1` (`run_default_webui()` in
`crates/strivo-bin/src/main.rs`) — unreachable through a published Docker
port no matter what `-p` mapping you give it. `strivo serve --bind` takes
an explicit address, but doesn't spawn a daemon on its own. Getting both a
routable port and a running daemon needs both processes regardless of how
you slice it, so `strivo-run.sh` runs them side by side in the same
container, rendezvousing over the daemon's Unix socket
(`$XDG_STATE_HOME/strivo/strivo.sock`, i.e. `/config/state/strivo/` in
this image — see `src/ipc.rs`). It:

- waits for the daemon's socket to appear before starting `serve` (`strivo
  serve` fails immediately with `daemon not running` if it races the
  daemon's startup — the bare `strivo` entry point has the same ~3s poll
  built in, which this script reproduces),
- forwards `SIGTERM`/`SIGINT` to both children so `docker stop` (via
  `tini` as PID 1) triggers the daemon's real graceful shutdown — the
  recording journal is snapshotted and the socket unlinked, per
  [DAEMON.md](./DAEMON.md) — rather than both processes being SIGKILLed at
  the container's stop-timeout,
- exits (and lets `docker restart`/`restart: unless-stopped` take over) as
  soon as either process dies, since a web UI with no daemon behind it (or
  vice versa) isn't a useful container.

There was no reason to split this across *two containers* sharing a socket
volume — nothing here needs the daemon and web UI to fail independently,
and a shared-socket-volume setup is strictly more moving parts for the
same result. If your deployment genuinely wants that (e.g. restarting the
web UI without touching in-flight recordings), run the image twice with
`command: ["strivo", "daemon"]` / `command: ["strivo", "serve", "--bind",
"0.0.0.0:8181"]` and a shared named volume mounted at
`$XDG_STATE_HOME/strivo` (`/config/state/strivo` with this image's default
env) on both.

## Healthcheck

`strivo status` (documented in [DAEMON.md](./DAEMON.md) as exiting 3 when
the daemon isn't running) always exits `0` in the current binary
regardless of daemon state — that's a real command, but not a usable
liveness signal, so the container's `HEALTHCHECK` doesn't use it.

Instead it hits `GET /api/v1/health` (`crates/strivo-web/src/routes/api.rs`),
an unauthenticated liveness+readiness probe that round-trips the daemon
over the real IPC socket, opens the jobs DB, and checks free disk on
`recording_dir`:

```bash
curl -fsS http://localhost:8181/api/v1/health
# {"status":"ok","checks":{"daemon":"ok","db":"ok","disk":"ok"}, ...}
```

It returns HTTP 200 when every check passes and 503 otherwise, so
`docker ps` / `docker compose ps` show real health, not just "the process
hasn't crashed."

## Credentials

Twitch/YouTube `client_id`/`client_secret` and Patreon's are plain fields
in `config.toml` (`TwitchConfig`/`YouTubeConfig`/`PatreonConfig` in
`src/config/mod.rs`) — set them through the setup wizard in the SPA on
first launch, or edit `/config/strivo/config.toml` on the `config` volume
directly and restart the container.

The OAuth **access and refresh tokens** minted from those credentials are
a different story: strivo stores them exclusively through the `keyring`
crate (`src/config/credentials.rs`) — there is no plaintext or
environment-variable fallback in the code, despite what
[FIRST-RUN.md](./FIRST-RUN.md)'s `STRIVO_TWITCH_CLIENT_ID`-style env vars
might suggest (those apply to `client_id`/`client_secret` only, not to
tokens). On Linux that means the D-Bus Secret Service — normally backed by
your desktop's keyring daemon, which doesn't exist in a container.

This image solves that by running a **headless `gnome-keyring`**:
`docker-entrypoint.sh` starts a private D-Bus session bus and unlocks a
`gnome-keyring` with an empty passphrase before `exec`-ing strivo. This is
a real Secret Service — `keyring::Entry::get_password`/`set_password`
round-trip through it exactly as they would against GNOME Keyring or
KWallet on a desktop — not a stub. The keyring file lands under
`/home/strivo/.local/share/keyrings` inside the container's writable
layer (not a mounted volume), so:

- **it does not survive `docker compose down -v`** or a container
  recreate — only the config/recordings volumes are meant to persist.
  Tokens are re-minted on next OAuth login, so this is inconvenient, not
  destructive.
- it has no login-session password protecting it (that's the whole point
  of "headless" — there's no login session), so treat it as
  lightly-obfuscated at-rest storage, not a security boundary. Don't rely
  on it for anything more sensitive than the same OAuth tokens a desktop
  install would hold in an unlocked keyring anyway.

If you want tokens to survive container recreation, bind-mount
`/home/strivo/.local/share/keyrings` onto a volume as well; the tradeoff
above still applies to that volume.

## Volumes

| Container path | Purpose |
|---|---|
| `/recordings` | The media library (`recording_dir` in config.toml, pre-seeded to `/recordings` by the entrypoint on first run) |
| `/config` | `config.toml`, plugin manifests, the recording journal, the jobs DB, and logs — see [FIRST-RUN.md](./FIRST-RUN.md) "Where state lives" for what maps where under `$XDG_CONFIG_HOME`/`$XDG_DATA_HOME`/`$XDG_STATE_HOME`, all pinned under `/config` in this image |

Use bind mounts instead of named volumes in production if you want the
recordings library to live on a specific disk/array — swap `recordings:`
in `docker-compose.yml` for a host path.

## Non-root

The container runs `strivo` as a fixed `uid:gid` `1000:1000` user (not
root). `docker-entrypoint.sh` `chown -R`s `/recordings` and `/config` to
that user on startup — transiently as root, before `gosu` drops
privileges — so bind-mounting a host directory owned by a different uid
still works.

## Verified locally

```
$ docker build -t strivo:test .
...
=> naming to docker.io/library/strivo:test

$ docker compose up -d
 Container strivo Started

$ docker ps --filter name=strivo --format "{{.Names}} {{.Status}}"
strivo   Up 10 seconds (healthy)

$ curl -sS -o /dev/null -w '%{http_code}\n' http://localhost:8181/
200

$ curl -sS http://localhost:8181/api/v1/health
{"checks":{"daemon":"ok","db":"ok","disk":"ok"},"disk":{...},"status":"ok","version":"0.6.0"}

$ docker exec strivo strivo doctor
StriVo external tool check
------------------------------------------------------------
  ok      ffmpeg       recording (required)  [/usr/bin/ffmpeg]
  ok      ffprobe      multitrack stream inspection (required)  [/usr/bin/ffprobe]
  ok      mpv          playback (required)  [/usr/bin/mpv]
  ok      streamlink   Twitch stream resolution (required)  [/usr/local/bin/streamlink]
  ok      yt-dlp       YouTube/Patreon resolution (required)  [/usr/local/bin/yt-dlp]
  MISSING whisper      transcription (optional, Crunchr plugin)
All required tools present.

$ docker stop -t 10 strivo   # graceful shutdown, both processes exit cleanly
strivo
$ docker ps -a --filter name=strivo --format "{{.Status}}"
Exited (0) 1 second ago
```
