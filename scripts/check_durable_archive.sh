#!/usr/bin/env bash
# Durable-archive regression check (plan todo 13).
#
# docs/PERFORMANCE.md claims "Recording list/detail/playback resolve the durable
# journal and overlay live snapshot state, so daemon memory eviction does not
# make archives unplayable", and the September 8 tranche report says the fix is
# what makes the media bounds reachable. The existing Rust test
# (`crates/strivo-web/tests/routes.rs`
# `durable_archive_entry_lists_details_and_range_downloads_without_snapshot`)
# proves the EVICTION half with a disconnected IPC client. It cannot prove the
# RESTART half: nothing in it restarts a daemon.
#
# This script proves the restart half against the REAL compiled binary, in the
# same throwaway XDG sandbox real-server.sh uses, and re-proves both halves
# across a genuine stop/start of both processes:
#
#   1. seed 1 old finished recording + N newer ones into a real jobs.db
#   2. list it and serve a byte range through the real HTTP surface
#   3. kill daemon + serve, start them again against the SAME sandbox
#   4. list it and serve a byte range again
#
# N defaults past src/daemon.rs's MAX_TERMINAL_RECORDINGS (200) so a naive
# snapshot-only lookup would be holding 200 terminal entries while the old
# recording is not among the newest.
#
# Usage: scripts/check_durable_archive.sh <release-strivo-binary> [newer-count]
# Exit:  0 both halves held across the restart | 1 a half failed | 2 refused
set -euo pipefail

BIN="${1:?usage: check_durable_archive.sh <release-strivo-binary> [newer-count]}"
NEWER="${2:-260}"
PORT="${BENCH_PORT:-8413}"
API_KEY="durable-archive-check-key"
ROOT="${TMPDIR:-/tmp}"

if [ ! -x "$BIN" ]; then
  echo "check_durable_archive.sh: not an executable binary: $BIN" >&2
  exit 2
fi
# The daemon identifies "the" daemon through $HOME/$XDG_*, so refuse a real one.
REAL_HOME="${HOME:-}"
mkdir -p "$ROOT/strivo-durable-check"
WORKDIR="$(mktemp -d "$ROOT/strivo-durable-check/run.XXXXXX")"
if [ "$WORKDIR" = "$REAL_HOME" ] || [ -e "$WORKDIR/.config/strivo/config.toml" ]; then
  echo "check_durable_archive.sh: refusing to run against a real installation" >&2
  exit 2
fi

export HOME="$WORKDIR"
export XDG_CONFIG_HOME="$WORKDIR/.c"
export XDG_DATA_HOME="$WORKDIR/.d"
export XDG_STATE_HOME="$WORKDIR/.s"
export XDG_CACHE_HOME="$WORKDIR/.ca"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

DAEMON_PID=""
SERVE_PID=""
KEEP_SANDBOX="${KEEP_SANDBOX:-}"
cleanup() {
  status=$?
  trap - EXIT INT TERM
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [ -n "$KEEP_SANDBOX" ]; then
    echo "check_durable_archive.sh: sandbox kept at $WORKDIR" >&2
  else
    rm -rf "$WORKDIR"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

MEDIA_DIR="$WORKDIR/Videos/StriVo"
mkdir -p "$MEDIA_DIR" "$XDG_DATA_HOME/strivo"
MEDIA="$MEDIA_DIR/durable-archive-check.mkv"
ffmpeg -y -loglevel error -f lavfi -i testsrc=duration=2:size=160x120:rate=10 \
  -f lavfi -i sine=frequency=440:duration=2 -shortest "$MEDIA"
OLD_ID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
JOBS_DB="$XDG_DATA_HOME/strivo/jobs.db"

boot_daemon() {
  "$BIN" daemon &
  DAEMON_PID=$!
  for _ in $(seq 1 100); do
    "$BIN" status >/dev/null 2>&1 && return 0
    kill -0 "$DAEMON_PID" 2>/dev/null || return 1
    sleep 0.2
  done
  return 1
}

boot_serve() {
  "$BIN" serve --bind "127.0.0.1:$PORT" --api-key "$API_KEY" &
  SERVE_PID=$!
  for _ in $(seq 1 100); do
    curl -fsS -o /dev/null "http://127.0.0.1:$PORT/api/v1/health" 2>/dev/null && return 0
    kill -0 "$SERVE_PID" 2>/dev/null || return 1
    sleep 0.2
  done
  return 1
}

stop_all() {
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  SERVE_PID=""
  DAEMON_PID=""
  sleep 0.5
}

echo "check_durable_archive.sh: sandbox $WORKDIR (throwaway; removed on exit)" >&2
boot_daemon || { echo "daemon never came up" >&2; exit 1; }

# One media file, 1 + $NEWER finished journal rows, oldest one written first —
# the shape `RecordingFinished` produces, written straight into the real
# jobs.db the same way e2e/real-server.sh seeds its recording.
python3 - "$JOBS_DB" "$OLD_ID" "$MEDIA" "$NEWER" <<'PY'
import json, os, sqlite3, sys, uuid
from datetime import datetime, timedelta, timezone

db, old_id, media, newer = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
size = os.path.getsize(media)
now = datetime.now(timezone.utc)

def row(job_id, name, started):
    payload = json.dumps({
        "id": job_id, "channel_id": "", "channel_name": name,
        "platform": "Twitch", "state": "Finished", "output_path": media,
        "started_at": started.isoformat().replace("+00:00", "Z"),
        "bytes_written": size, "duration_secs": 2.0, "transcode": False,
        "error": None, "stream_title": name, "watched": False, "playlist": None,
        "thumbnail_url": None, "source_url": None,
    })
    stamp = started.isoformat().replace("+00:00", "Z")
    return (job_id, "Recording", payload, "finished", stamp, stamp, 0, None, None)

conn = sqlite3.connect(db)
rows = [row(old_id, "durable-oldest", now - timedelta(days=30))]
for i in range(newer):
    rows.append(row(str(uuid.uuid4()), f"newer-{i}", now - timedelta(minutes=i + 1)))
conn.executemany(
    "INSERT INTO jobs (id, kind, payload, state, created_at, updated_at, attempts,"
    " last_error, episode_dir) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)", rows)
conn.commit()
conn.close()
print(f"seeded {len(rows)} finished rows; oldest id {old_id}")
PY

boot_serve || { echo "web server never came up on port $PORT" >&2; exit 1; }
BASE="http://127.0.0.1:$PORT"
AUTH=(-H "x-api-key: $API_KEY")

probe() {
  local phase="$1"
  local listed total ranged bytes
  listed="$(curl -fsS "${AUTH[@]}" "$BASE/api/v1/recordings?limit=500" \
    | python3 -c 'import json,sys; print(any(r["id"]==sys.argv[1] for r in json.load(sys.stdin)["recordings"]))' "$OLD_ID")"
  total="$(curl -fsS "${AUTH[@]}" "$BASE/api/v1/recordings?limit=1" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["total"])')"
  read -r status bytes < <(curl -fsS "${AUTH[@]}" -H 'range: bytes=0-99' \
    -o /tmp/durable-range.bin -w '%{http_code} %{size_download}' \
    "$BASE/api/v1/recordings/$OLD_ID/download")
  ranged="$([ "$status" = 206 ] && [ "$bytes" = 100 ] && echo yes || echo "no(status=$status,bytes=$bytes)")"
  printf '%-8s listed=%s library_total=%s range_206_100b=%s\n' "$phase" "$listed" "$total" "$ranged"
  [ "$listed" = True ] && [ "$ranged" = yes ]
}

rc=0
probe before-restart || { echo "FAIL: durable archive unusable BEFORE restart" >&2; rc=1; }
stop_all
boot_daemon || { echo "daemon never came back up after restart" >&2; exit 1; }
boot_serve || { echo "web server never came back up after restart" >&2; exit 1; }
probe after-restart || { echo "FAIL: durable archive unusable AFTER restart" >&2; rc=1; }

if [ "$rc" -eq 0 ]; then
  echo "check_durable_archive.sh: PASS — oldest of $((NEWER + 1)) finished recordings" \
    "listed and served a 100-byte range before and after a real daemon restart" >&2
fi
exit "$rc"
