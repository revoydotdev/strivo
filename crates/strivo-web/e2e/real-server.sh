#!/usr/bin/env bash
# S04 — boots the REAL compiled strivo-web server (daemon + web process,
# same two binaries production runs) for the "real-server" Playwright
# lane, as opposed to tests/*.spec.ts's Node mock backend.
#
# Every strivo path that identifies "the" daemon on this machine
# (config dir, state dir incl. the PID file + IPC socket, data dir, cache
# dir) comes from `directories::ProjectDirs::from("", "", "strivo")`,
# which resolves relative to $HOME/$XDG_*_HOME — NOT from --config. So a
# naive `strivo daemon` here would either attach to a real daemon already
# running under this account, or clobber its config/state. We sandbox
# every one of those XDG roots under a fresh temp dir instead, so this
# lane can never see or touch a real installation.
set -euo pipefail

STRIVO_BIN="${STRIVO_BIN:?set STRIVO_BIN to the compiled strivo binary path}"
PORT="${PORT:-8281}"
API_KEY="${STRIVO_E2E_API_KEY:-strivo-e2e-real-server-test-key}"
# Readiness budget, in 0.2 s probes of `strivo status`. The default is
# unchanged, so the PVR lane behaves exactly as it did. It is a knob only
# because a Creator-featured build needs a bigger one: it initialises a
# plugin host with six plugins before the IPC listener binds, measured at
# 12.4 s on this host under load average ~15, against the 10 s the 50-probe
# default allows. playwright.creator.config.ts raises it; nothing else needs
# to. Widen-only, so no existing caller can be made to wait less.
DAEMON_PROBES="${STRIVO_E2E_DAEMON_PROBES:-50}"

SANDBOX_ROOT="${TMPDIR:-/tmp}"
# Playwright tears this script's process tree down with a plain kill (not
# always a trappable signal we get to react to), so the EXIT trap below is
# best-effort, not guaranteed. Self-heal instead of relying on it: sweep
# sandboxes from runs that didn't clean up, once, before making a new one.
find "$SANDBOX_ROOT" -maxdepth 1 -name 'strivo-e2e-sandbox.*' -mmin +10 -exec rm -rf {} + 2>/dev/null || true

WORKDIR="$(mktemp -d "$SANDBOX_ROOT/strivo-e2e-sandbox.XXXXXX")"
export HOME="$WORKDIR"
export XDG_CONFIG_HOME="$WORKDIR/.config"
export XDG_DATA_HOME="$WORKDIR/.local/share"
export XDG_STATE_HOME="$WORKDIR/.local/state"
export XDG_CACHE_HOME="$WORKDIR/.cache"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

# Seed one finished recording so the real lane exercises download/Range
# (B-04/B-08) against an actual HTTP response, not just the mock lane's
# synthetic bytes. `scan_existing_recordings` (daemon startup) would pick up
# a file dropped into recording_dir, but it only lands in the daemon's
# in-memory snapshot — GET /api/v1/recordings only lists it if it's also in
# the durable jobs.db (see routes/*.rs `recordings()`: a live Finished/Failed
# entry not already in the db is never added to the page). So write the
# journal row directly instead, the same shape `RecordingFinished` would
# have written (see `PersistedJob`/`upsert_job` in src/recording/persist.rs)
# — same technique crates/strivo-web/tests/routes.rs's
# durable_archive_entry_lists_details_and_range_downloads_without_snapshot
# test uses, just via python3's stdlib sqlite3 module against the real
# compiled binaries instead of calling PersistDb from Rust.
SEED_ID=""
SEED_FILE=""
if command -v ffmpeg >/dev/null 2>&1 && command -v python3 >/dev/null 2>&1; then
  SEED_DIR="$WORKDIR/Videos/StriVo"
  mkdir -p "$SEED_DIR"
  SEED_FILE="$SEED_DIR/e2eseed_2024-01-01_real-lane-range-check.mkv"
  ffmpeg -y -loglevel error \
    -f lavfi -i testsrc=duration=2:size=320x240:rate=10 \
    -f lavfi -i sine=frequency=440:duration=2 \
    -shortest \
    "$SEED_FILE"
  SEED_ID="$(cat /proc/sys/kernel/random/uuid 2>/dev/null || python3 -c 'import uuid; print(uuid.uuid4())')"
else
  echo "real-server.sh: ffmpeg or python3 not found on PATH; skipping seeded recording" >&2
fi

DAEMON_PID=""
SERVE_PID=""

cleanup() {
  status=$?
  trap - EXIT INT TERM
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$WORKDIR"
  exit "$status"
}
trap cleanup EXIT INT TERM

"$STRIVO_BIN" daemon &
DAEMON_PID=$!

daemon_up=0
for _ in $(seq 1 "$DAEMON_PROBES"); do
  if "$STRIVO_BIN" status >/dev/null 2>&1; then
    daemon_up=1
    break
  fi
  if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
    break
  fi
  sleep 0.2
done

if [ "$daemon_up" -ne 1 ]; then
  echo "real-server.sh: daemon never came up; recent log:" >&2
  find "$XDG_STATE_HOME" -name 'strivo.*.log' -exec tail -n 100 {} \; >&2 || true
  exit 1
fi

# jobs.db exists once the daemon is up (PersistDb::open runs before the IPC
# listener binds, which `strivo status` above already proved is live).
# directories::ProjectDirs::from("", "", "strivo").data_dir() is
# $XDG_DATA_HOME/strivo on Linux.
if [ -n "$SEED_ID" ] && [ -f "$SEED_FILE" ]; then
  JOBS_DB="$XDG_DATA_HOME/strivo/jobs.db"
  SEED_BYTES="$(stat -c%s "$SEED_FILE" 2>/dev/null || stat -f%z "$SEED_FILE")"
  SEED_NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  SEED_PAYLOAD="$(python3 - "$SEED_ID" "$SEED_FILE" "$SEED_BYTES" "$SEED_NOW" <<'PY'
import json
import sys

job_id, path, size, now = sys.argv[1:5]
print(json.dumps({
    "id": job_id,
    "channel_id": "",
    "channel_name": "e2eseed",
    "platform": "Twitch",
    "state": "Finished",
    "output_path": path,
    "started_at": now,
    "bytes_written": int(size),
    "duration_secs": 2.0,
    "transcode": False,
    "error": None,
    "stream_title": "real lane range check",
    "watched": False,
    "playlist": None,
    "thumbnail_url": None,
    "source_url": None,
}))
PY
  )"
  if [ -f "$JOBS_DB" ]; then
    python3 - "$JOBS_DB" "$SEED_ID" "$SEED_PAYLOAD" "$SEED_NOW" <<'PY'
import sqlite3
import sys

db_path, job_id, payload, now = sys.argv[1:5]
conn = sqlite3.connect(db_path)
conn.execute(
    "INSERT INTO jobs (id, kind, payload, state, created_at, updated_at, attempts, last_error, episode_dir)"
    " VALUES (?, 'Recording', ?, 'finished', ?, ?, 0, NULL, NULL)",
    (job_id, payload, now, now),
)
conn.commit()
conn.close()
PY
  else
    echo "real-server.sh: $JOBS_DB not found after daemon startup; skipping seeded recording" >&2
  fi
fi

"$STRIVO_BIN" serve --bind "127.0.0.1:$PORT" --api-key "$API_KEY" &
SERVE_PID=$!
wait "$SERVE_PID"
