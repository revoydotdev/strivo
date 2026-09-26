#!/usr/bin/env bash
# Measure the SHIPPED SPA bundle over real HTTP (plan todo 10).
#
# docs/PERFORMANCE.md:88-96 sets the split threshold at 250 KiB of COMPRESSED
# JavaScript. Measuring a file on disk does not answer that: the wire size is
# decided by the CompressionLayer in crates/strivo-web/src/server.rs, so the
# only honest measurement is a request against a running release server. F-29
# (docs/audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md:90) was exactly this
# mistake — an uncompressed size standing in for a compressed one.
#
# What it does:
#   1. Refuses a non-release binary and a real $HOME (same guard as
#      scripts/bench_library.sh: these budgets are release gates, and a
#      sandbox that could reach a real installation would be a lie).
#   2. Boots `strivo daemon` + `strivo serve` in a throwaway XDG sandbox.
#   3. Confirms BY CONTENT which bundle is embedded: the identity-fetched
#      sha256 of /assets/spa.js and /assets/spa.css must equal the files in
#      the supplied out/assets directory. Naming an edition by a cargo build
#      directory hash is not identification.
#   4. Measures, over HTTP: raw (identity), gzip, and brotli bytes for each
#      asset, SAMPLES times each, and asserts the server actually returned the
#      encoding that was asked for. A "brotli" number that is really gzip
#      because the layer ignored Accept-Encoding is the failure this guards.
#   5. Tears the server down on every exit path and writes one JSON document.
#
# Usage:
#   cargo build --release            # PVR: the default edition
#   scripts/measure_bundle.sh \
#     /scratch/cargo-targets/strivo-main/release/strivo \
#     /scratch/cargo-targets/strivo-main/release/build/strivo-web-*/out/assets \
#     PVR
#
# Knobs:
#   BENCH_PORT=8400        port for the throwaway web server
#   BUNDLE_SAMPLES=3       requests per (asset, encoding) point
#   BUNDLE_OUT=<path>      output JSON (default artifacts/bundle-<label>-<ts>.json)
#
# Exit codes: 0 measured | 2 refused (unsafe target) | 1 measurement failed.
set -euo pipefail

BIN="${1:?usage: measure_bundle.sh <release-strivo-binary> <out-assets-dir> <label>}"
ASSETS_DIR="${2:?usage: measure_bundle.sh <release-strivo-binary> <out-assets-dir> <label>}"
LABEL="${3:?usage: measure_bundle.sh <release-strivo-binary> <out-assets-dir> <label>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="${BENCH_PORT:-8400}"
SAMPLES="${BUNDLE_SAMPLES:-3}"
OUT="${BUNDLE_OUT:-artifacts/bundle-${LABEL}-$(date -u +%Y%m%dT%H%M%SZ).json}"
REAL_HOME="${HOME:-}"
ASSETS=(spa.js spa.css)
ENCODINGS=(gzip br)

refuse() {
  echo "measure_bundle.sh: REFUSING TO RUN." >&2
  echo "  $1" >&2
  echo "Point this at a release binary; the 250 KiB budget in" >&2
  echo "docs/PERFORMANCE.md:91 is a release gate." >&2
  exit 2
}

[ -x "$BIN" ] || { echo "measure_bundle.sh: not an executable binary: $BIN" >&2; exit 1; }
[ -d "$ASSETS_DIR" ] || { echo "measure_bundle.sh: no such assets dir: $ASSETS_DIR" >&2; exit 1; }
case "$(dirname "$BIN")" in
  *release) ;;
  *) refuse "'$BIN' is not a release build; a debug build is not comparable." ;;
esac

SANDBOX_ROOT="${TMPDIR:-/tmp}"
case "$SANDBOX_ROOT" in /*) ;; *) SANDBOX_ROOT="/$SANDBOX_ROOT" ;; esac
mkdir -p "$SANDBOX_ROOT/strivo-bundle"
WORKDIR="$(mktemp -d "$SANDBOX_ROOT/strivo-bundle/m.XXXXXX")"
[ "$WORKDIR" != "$REAL_HOME" ] || refuse "sandbox resolved to your real \$HOME."

export HOME="$WORKDIR"
export XDG_CONFIG_HOME="$WORKDIR/.c"
export XDG_DATA_HOME="$WORKDIR/.d"
export XDG_STATE_HOME="$WORKDIR/.s"
export XDG_CACHE_HOME="$WORKDIR/.ca"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

API_KEY="bundle-measure-key"
DAEMON_PID=""
SERVE_PID=""
cleanup() {
  trap - EXIT INT TERM
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$WORKDIR"
  echo "measure_bundle.sh: stopped $LABEL server, released port $PORT, removed $WORKDIR" >&2
}
trap cleanup EXIT INT TERM

VERSION="$("$BIN" --version 2>&1 | head -1)"

"$BIN" daemon >"$WORKDIR/daemon.log" 2>&1 &
DAEMON_PID=$!
daemon_up=0
for _ in $(seq 1 150); do
  if "$BIN" status >/dev/null 2>&1; then daemon_up=1; break; fi
  kill -0 "$DAEMON_PID" 2>/dev/null || break
  sleep 0.2
done
if [ "$daemon_up" -ne 1 ]; then
  echo "measure_bundle.sh: daemon never came up; last log lines:" >&2
  tail -n 40 "$WORKDIR/daemon.log" >&2 || true
  exit 1
fi

"$BIN" serve --bind "127.0.0.1:$PORT" --api-key "$API_KEY" >"$WORKDIR/serve.log" 2>&1 &
SERVE_PID=$!
BASE="http://127.0.0.1:$PORT"
up=0
for _ in $(seq 1 150); do
  # No -f on purpose: /api/v1/health answers 401 without a key, and -f would
  # read that as "not listening yet" and burn the whole 30 s wait.
  if curl -sS -o /dev/null "$BASE/api/v1/health" 2>/dev/null; then up=1; break; fi
  kill -0 "$SERVE_PID" 2>/dev/null || break
  sleep 0.2
done
if [ "$up" -ne 1 ]; then
  echo "measure_bundle.sh: web server never came up on $BASE" >&2
  tail -n 40 "$WORKDIR/serve.log" >&2 || true
  exit 1
fi

# One (asset, encoding) point. Echoes "min median max" over SAMPLES requests and
# hard-fails if the server answered with a different Content-Encoding.
sample() {
  local url="$1" want="$2"
  local sizes=() hdr body sz got
  for _ in $(seq 1 "$SAMPLES"); do
    hdr="$(mktemp)"; body="$(mktemp)"
    # --fail turns a 404 into a non-zero exit. A missing asset must never be
    # recorded as a small transfer.
    sz="$(curl -fsS -D "$hdr" -o "$body" -H "Accept-Encoding: $want" \
          -w '%{size_download}' "$url")"
    got="$(tr -d '\r' <"$hdr" | awk 'tolower($1)=="content-encoding:"{print $2}' | tail -1)"
    if [ "$got" != "$want" ]; then
      rm -f "$hdr" "$body"
      echo "measure_bundle.sh: $url: asked for '$want' but the server returned" \
           "content-encoding '${got:-<none>}' — refusing to record it as $want." >&2
      exit 1
    fi
    sizes+=("$sz")
    rm -f "$hdr" "$body"
  done
  printf '%s\n' "${sizes[@]}" | sort -n |
    awk '{a[NR]=$1} END{printf "%d %d %d", a[1], a[int((NR+1)/2)], a[NR]}'
}

# Identity fetch, used for the content check and the raw (uncompressed) figure.
raw_bytes() {
  local url="$1" body sz
  body="$(mktemp)"
  sz="$(curl -fsS -o "$body" -w '%{size_download}' "$url")"
  local got
  got="$(sha256sum "$body" | cut -d' ' -f1)"
  rm -f "$body"
  printf '%s %s' "$sz" "$got"
}

echo "measure_bundle.sh: $LABEL  $VERSION  $BIN" >&2
echo "measure_bundle.sh: serving $BASE (pid $SERVE_PID), $SAMPLES samples per point" >&2

JSON_ASSETS=""
FIRST=1
for asset in "${ASSETS[@]}"; do
  url="$BASE/assets/$asset"
  # Same subshell caveat as below: check the status, or a 404 here is recorded
  # as empty bytes and only trips later with a misleading hash message.
  identity="$(raw_bytes "$url")" || exit 1
  read -r raw sha <<<"$identity"
  want_sha="$(sha256sum "$ASSETS_DIR/$asset" | cut -d' ' -f1)"
  if [ "$sha" != "$want_sha" ]; then
    echo "measure_bundle.sh: the server at $url does not serve the bundle in" \
         "$ASSETS_DIR" >&2
    echo "  served sha256    $sha" >&2
    echo "  $ASSETS_DIR/$asset $want_sha" >&2
    echo "Wrong edition, or the binary is older than the asset directory." >&2
    exit 1
  fi
  echo "measure_bundle.sh: $asset content confirmed ($asset sha256 $sha)" >&2
  [ -z "$(find "$ASSETS_DIR" -name '*-creator.*' -print -quit)" ] ||
    echo "measure_bundle.sh: note: $ASSETS_DIR holds -creator modules" >&2

  entry="\"raw_bytes\": $raw"
  for enc in "${ENCODINGS[@]}"; do
    # `sample` runs in a subshell, so its `exit 1` cannot stop this script on its
    # own; the status has to be checked here or a failed assertion is ignored.
    point="$(sample "$url" "$enc")" || exit 1
    read -r mn md mx <<<"$point"
    entry="$entry, \"$enc\": {\"min\": $mn, \"median\": $md, \"max\": $mx}"
    echo "measure_bundle.sh: $asset $enc min=$mn median=$md max=$mx" >&2
  done
  [ "$FIRST" -eq 1 ] || JSON_ASSETS="$JSON_ASSETS,"
  FIRST=0
  JSON_ASSETS="$JSON_ASSETS
    \"$asset\": { $entry }"
done

mkdir -p "$(dirname "$OUT")"
cat >"$OUT" <<JSON
{
  "label": "$LABEL",
  "strivo_version": "$VERSION",
  "measured_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "assets_dir": "$ASSETS_DIR",
  "samples_per_point": $SAMPLES,
  "assets": {$JSON_ASSETS
  }
}
JSON

echo "measure_bundle.sh: wrote $OUT" >&2
echo "$OUT"
