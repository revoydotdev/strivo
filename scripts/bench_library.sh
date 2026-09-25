#!/usr/bin/env bash
# Reproducible performance baseline harness (plan todo 9).
#
# docs/PERFORMANCE.md:80-83 asks for exactly this: run a RELEASE binary against
# a copy of a large library and record API latency, resident memory,
# first-render time, and active child-process count — and forbids benchmarking
# against a live production database. Every open performance question in the
# three prior audits was deferred because no such harness existed. This is it.
#
# What it does, in order:
#   1. REFUSES to run against a real installation (see the guard below). This
#      is the one rule stated in capitals in the todo, so it is the first thing
#      the script does, before anything is created.
#   2. Creates a throwaway XDG/HOME sandbox and a synthetic library: N
#      recordings spread over a pool of REAL media files, one group per
#      characteristic the plan names (multi-audio, silent, VFR, mixed codec,
#      long, Unicode path). ffprobe's own reading of each file lands in the
#      baseline, so "multi-audio" is two audio streams because ffprobe said
#      so, not because we named a class that way.
#   3. Boots `strivo daemon` + `strivo serve` inside the sandbox, exactly the
#      two-process shape crates/strivo-web/e2e/real-server.sh uses, so every
#      strivo path resolves under the sandbox and no real data is reachable.
#   4. Measures through the real HTTP surface and writes one machine-readable
#      JSON document (scripts/bench_probe.mjs). Latency percentiles come from
#      the `Server-Timing` header the server already emits — the same signal
#      the budget table in docs/PERFORMANCE.md is written against.
#
# Usage:
#   cargo build --release -p strivo-bin
#   scripts/bench_library.sh target/release/strivo [rows] [out.json]
#
# Knobs (all optional, all conservative by default — this runs on a shared
# workstation and must not become a load test):
#   BENCH_REQUESTS=60        requests per workload (concurrency is always 1
#                            unless BENCH_CONCURRENCY is set explicitly)
#   BENCH_CONCURRENCY=1
#   BENCH_OFFSETS=0,1000,5000,10000
#   BENCH_MEDIA_PER_CLASS=2  real media files encoded per characteristic
#   BENCH_PORT=8391
#   BENCH_HOME=<dir>         use this sandbox instead of a fresh mktemp one;
#                            it must be a throwaway directory (see the guard)
#   KEEP_SANDBOX=1           leave the sandbox on disk for inspection
#
# Exit codes: 0 baseline written | 2 refused (unsafe target) | 1 run failed.
set -euo pipefail

BIN="${1:?usage: bench_library.sh <release-strivo-binary> [rows] [out.json]}"
ROWS="${2:-10000}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="${BENCH_PORT:-8391}"
API_KEY="perf-baseline-key"
REQUESTS="${BENCH_REQUESTS:-60}"
CONCURRENCY="${BENCH_CONCURRENCY:-1}"
OFFSETS="${BENCH_OFFSETS:-0,1000,5000,10000}"
MEDIA_PER_CLASS="${BENCH_MEDIA_PER_CLASS:-2}"
OUT="${3:-artifacts/bench-library-$(date -u +%Y%m%dT%H%M%SZ).json}"
REAL_HOME="${HOME:-}"
NOT_CAPTURED=()

refuse() {
  cat >&2 <<EOF
bench_library.sh: REFUSING TO RUN.

  $1

A benchmark against a real installation is forbidden by docs/PERFORMANCE.md:83
and would read a live production database. Point this at a throwaway
directory instead:

  scripts/bench_library.sh $BIN $ROWS $OUT

which creates a fresh sandbox under \${TMPDIR:-/tmp} and deletes it on exit.
EOF
  exit 2
}

[ -x "$BIN" ] || { echo "bench_library.sh: not an executable binary: $BIN" >&2; exit 1; }
# The plan says RELEASE. A debug build's numbers are not a baseline; refuse
# rather than quietly publishing them, with an explicit override for the one
# case where a debug build is all you have.
case "$(dirname "$BIN")" in
  *release) ;;
  *) [ -n "${STRIVO_BENCH_ALLOW_NONRELEASE:-}" ] ||
       refuse "'$BIN' is not a release build. These budgets are release gates;
  a debug build is not comparable. Pass STRIVO_BENCH_ALLOW_NONRELEASE=1 if
  that is genuinely what you want to measure." ;;
esac

SANDBOX_ROOT="${TMPDIR:-/tmp}"
case "$SANDBOX_ROOT" in /*) ;; *) SANDBOX_ROOT="/$SANDBOX_ROOT" ;; esac
if [ -n "${BENCH_HOME:-}" ]; then
  WORKDIR="$BENCH_HOME"
  case "$WORKDIR" in
    "$SANDBOX_ROOT"/*) ;;
    *) refuse "BENCH_HOME='$WORKDIR' is not under the throwaway root '$SANDBOX_ROOT'." ;;
  esac
  [ "$WORKDIR" != "$REAL_HOME" ] ||
    refuse "BENCH_HOME is your real \$HOME ($WORKDIR)."
  [ ! -e "$WORKDIR/.config/strivo/config.toml" ] ||
    refuse "BENCH_HOME contains a real StriVo configuration
  ($WORKDIR/.config/strivo/config.toml) — that is a live installation."
  mkdir -p "$WORKDIR"
else
  # A short path on purpose: the daemon's Unix socket lives under
  # $XDG_STATE_HOME and a long path can overflow SUN_LEN (the same fix
  # scripts/bench_jobs_db.sh carries).
  mkdir -p "$SANDBOX_ROOT/strivo-perf-bench"
  WORKDIR="$(mktemp -d "$SANDBOX_ROOT/strivo-perf-bench/r.XXXXXX")"
fi

export HOME="$WORKDIR"
export XDG_CONFIG_HOME="$WORKDIR/.c"
export XDG_DATA_HOME="$WORKDIR/.d"
export XDG_STATE_HOME="$WORKDIR/.s"
export XDG_CACHE_HOME="$WORKDIR/.ca"
# The sandbox redirects XDG_CACHE_HOME, which is also where Playwright looks
# for Chromium; point it back at the real browser cache so first-render can
# still be measured. Browsers are read-only tooling, not strivo state.
export PLAYWRIGHT_BROWSERS_PATH="${PLAYWRIGHT_BROWSERS_PATH:-$REAL_HOME/.cache/ms-playwright}"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

DAEMON_PID=""
SERVE_PID=""
cleanup() {
  trap - EXIT INT TERM
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [ -n "${KEEP_SANDBOX:-}" ]; then
    echo "bench_library.sh: sandbox kept at $WORKDIR" >&2
  else
    rm -rf "$WORKDIR"
  fi
}
trap cleanup EXIT INT TERM

echo "bench_library.sh: binary   $BIN" >&2
echo "bench_library.sh: version  $("$BIN" --version 2>&1 | head -1)" >&2
echo "bench_library.sh: sandbox  $WORKDIR (throwaway; removed on exit)" >&2
echo "bench_library.sh: library  $ROWS rows, $MEDIA_PER_CLASS real media file(s) per characteristic" >&2

# Seed BEFORE the daemon starts, at the exact path strivo-core's
# AppConfig::data_dir() resolves to under this sandboxed XDG root.
DATA_DIR="$XDG_DATA_HOME/strivo"
MEDIA_DIR="$WORKDIR/media"
mkdir -p "$DATA_DIR" "$MEDIA_DIR"
python3 "$SCRIPT_DIR/seed_bench_library.py" "$DATA_DIR" "$MEDIA_DIR" \
  "$ROWS" "$MEDIA_PER_CLASS"

"$BIN" daemon &
DAEMON_PID=$!
daemon_up=0
for _ in $(seq 1 150); do
  if "$BIN" status >/dev/null 2>&1; then daemon_up=1; break; fi
  kill -0 "$DAEMON_PID" 2>/dev/null || break
  sleep 0.2
done
if [ "$daemon_up" -ne 1 ]; then
  echo "bench_library.sh: daemon never came up; last log lines:" >&2
  find "$XDG_STATE_HOME" -name 'strivo.*.log' -exec tail -n 60 {} \; >&2 || true
  exit 1
fi

"$BIN" serve --bind "127.0.0.1:$PORT" --api-key "$API_KEY" &
SERVE_PID=$!
BASE_URL="http://127.0.0.1:$PORT"
up=0
for _ in $(seq 1 150); do
  # Any HTTP response proves the listener and the jobs_db init path are alive;
  # only a connection failure means "not up yet".
  if curl -s -o /dev/null "$BASE_URL/api/v1/health" 2>/dev/null; then up=1; break; fi
  sleep 0.2
done
if [ "$up" -ne 1 ]; then
  echo "bench_library.sh: web server never came up on $BASE_URL" >&2
  exit 1
fi
echo "bench_library.sh: serving  $BASE_URL (pid $SERVE_PID, daemon pid $DAEMON_PID)" >&2

mkdir -p "$(dirname "$OUT")"
BIN_SHA="$(sha256sum "$BIN" | cut -d' ' -f1)"
STRIVO_BENCH_BIN="$BIN" node "$SCRIPT_DIR/bench_probe.mjs" \
  --base-url "$BASE_URL" \
  --api-key "$API_KEY" \
  --serve-pid "$SERVE_PID" \
  --daemon-pid "$DAEMON_PID" \
  --requests "$REQUESTS" \
  --concurrency "$CONCURRENCY" \
  --offsets "$OFFSETS" \
  --binary-sha "$BIN_SHA" \
  --library-manifest "$WORKDIR/library-manifest.json" \
  --out "$OUT"
echo "bench_library.sh: baseline -> $OUT" >&2
