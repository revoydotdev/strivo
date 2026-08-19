# syntax=docker/dockerfile:1
#
# strivo — self-hosted live-stream PVR
#
# Two stages: a build stage that compiles the release binary, and a slim
# runtime stage that carries only strivo plus the external tools it shells
# out to (ffmpeg/ffprobe, mpv, streamlink, yt-dlp).
#
# EDITION: this image builds the default free PVR edition — plain
# `cargo build --release` with no `--features creator`, matching the
# release tarballs and the AUR package (see Cargo.toml `default-members`).
# Pass `--build-arg EDITION=creator` to build the Creator Edition instead;
# never make that the default tag.
#
# PROCESS MODEL: one container, two supervised processes — `strivo daemon`
# and `strivo serve --bind 0.0.0.0:...`, started and monitored by
# strivo-run.sh. The bare `strivo` (no subcommand) entry point looked like
# the natural single-process choice, since it already spawns the daemon
# in-process and serves the web UI — but it hardcodes the web bind address
# to 127.0.0.1 (see run_default_webui() in crates/strivo-bin/src/main.rs),
# which is unreachable through a published Docker port. `strivo serve
# --bind` takes an explicit address but doesn't spawn a daemon on its own,
# so a routable container needs both processes regardless. They still
# share one container and rendezvous over the daemon's Unix socket under
# the /config volume (src/ipc.rs socket_path()) — no split across
# containers, no socket volume to wire up separately. See strivo-run.sh
# for the supervision (signal forwarding, exit-together) details.

ARG EDITION=pvr

########################################
# Build stage
########################################
FROM rust:1-bookworm AS build

# rusqlite is built with the `bundled` feature (vendors SQLite and compiles
# it with cc), so a C toolchain is required at build time. The `keyring`
# crate's `sync-secret-service` feature links libdbus at build time (see
# Cargo.toml), hence libdbus-1-dev.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libdbus-1-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

ARG EDITION
RUN set -eu; \
    if [ "$EDITION" = "creator" ]; then \
        cargo build --release -p strivo-bin --features creator; \
    else \
        cargo build --release; \
    fi; \
    cp target/release/strivo /build/strivo

########################################
# Runtime stage
########################################
FROM debian:bookworm-slim AS runtime

# External tools strivo shells out to (see `strivo doctor` in
# crates/strivo-bin/src/main.rs for the authoritative required/optional
# list):
#   ffmpeg, ffprobe  — recording, multitrack inspection (required)
#   mpv              — in-browser live playback without downloading first (required)
#   streamlink       — Twitch stream resolution (required)
#   yt-dlp           — YouTube/Patreon stream resolution (required)
# `yt-dlp` and `streamlink` move fast; installing them via pip into an
# isolated venv (rather than Debian's often-stale apt versions) keeps them
# current and keeps pip's dependency tree out of the system Python.
#
# gnome-keyring + dbus provide a headless Secret Service so strivo's OS
# keyring credential storage (src/config/credentials.rs, used for OAuth
# access/refresh tokens) has somewhere to write inside a container that has
# no desktop session — see docs/DOCKER.md "Credentials" for how this is
# unlocked at container start and its limitations.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    mpv \
    python3 \
    python3-venv \
    ca-certificates \
    curl \
    dbus \
    dbus-x11 \
    gnome-keyring \
    gosu \
    tini \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/pyapps \
    && /opt/pyapps/bin/pip install --no-cache-dir streamlink yt-dlp \
    && ln -s /opt/pyapps/bin/streamlink /usr/local/bin/streamlink \
    && ln -s /opt/pyapps/bin/yt-dlp /usr/local/bin/yt-dlp

# Non-root user; the recordings/config/state volumes are chowned to it by
# the entrypoint before dropping from root (root is only needed transiently
# to fix bind-mount ownership — gosu then execs strivo as this user).
RUN useradd --create-home --home-dir /home/strivo --shell /usr/sbin/nologin --uid 1000 strivo

COPY --from=build /build/strivo /usr/local/bin/strivo
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY strivo-run.sh /usr/local/bin/strivo-run.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh /usr/local/bin/strivo-run.sh

# XDG dirs strivo resolves via `directories::ProjectDirs` (see
# src/config/mod.rs config_dir/data_dir/state_dir) — pinned explicitly so
# they land on the mounted volumes regardless of $HOME quirks.
ENV XDG_CONFIG_HOME=/config \
    XDG_DATA_HOME=/config/data \
    XDG_STATE_HOME=/config/state \
    HOME=/home/strivo

VOLUME ["/recordings", "/config"]
EXPOSE 8181

ENTRYPOINT ["tini", "--", "/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/strivo-run.sh"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8181/api/v1/health || exit 1
