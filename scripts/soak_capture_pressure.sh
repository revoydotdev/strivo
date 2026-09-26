#!/usr/bin/env bash
# Capture interruption / disk-pressure harness (plan todo 39, report C06).
#
# docs/CREATOR-EDITION-REPORT-2026-09-07.md:285,411 asks for a rehearsal in
# which a long-stream disconnect, restart or disk-pressure event preserves
# playable completed parts, reports missing intervals, and never lets
# rendering silently exhaust capture capacity. The library half of that is
# `cargo test` (src/recording/{gaps,capacity,soak}.rs). This script is the
# half that needs real binaries and a real size-capped volume:
#
#   * six hours of REAL stream time encoded by ffmpeg, merged by mkvmerge
#     through recording::segments::merge_segments, measured by ffprobe, and
#     every part fully decoded (not size-checked);
#   * a render admitted through the real pipeline ResourceRegistry, i.e. the
#     same ResourceLock::Disk acquisition src/pipeline/runtime.rs performs;
#   * a volume filled with real bytes until free space reaches capture's
#     reserve, asserting capture is still admitted and the render is refused
#     WITH A REASON.
#
# It is a separate script from todo 9's bench_* harnesses rather than an
# edit to them: those measure a running server's latency and child counts,
# this one measures a recovery rehearsal and needs a writable volume it can
# fill. It reuses todo 9's conventions rather than reinventing them: refuse
# to run against a real install, keep every byte under a private directory,
# write a machine-readable artefact, and exit non-zero rather than skipping.
#
# The volume is a size-capped tmpfs created here and unmounted on every exit
# path, so "fill the disk" cannot reach a real filesystem. Without
# passwordless sudo the tmpfs cannot be created and this script refuses (exit
# 2) instead of running the pressure phase against an unbounded filesystem.
#
# Usage: scripts/soak_capture_pressure.sh [out.json]
# Env:   STRIVO_SOAK_SEED       decimal seed for the interruption schedule
#        STRIVO_SOAK_SESSIONS   capture sessions across the window (default 12)
#        STRIVO_SOAK_VOLUME_MB  tmpfs size in MB (default 48)
#        STRIVO_SOAK_MUTATE     drop-ledger-entry | phantom-ledger-entry | none
#        STRIVO_SOAK_KEEP       1 to keep the tmpfs and scratch dir
# Exit:  0 every acceptance check passed
#        1 a check failed (the run prints each one)
#        2 refused: no volume, no ffmpeg/ffprobe/mkvmerge, or no sudo
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT="${1:-$REPO_ROOT/artifacts/soak-capture-$(date -u +%Y%m%dT%H%M%SZ).log}"
SEED="${STRIVO_SOAK_SEED:-24301}"
SESSIONS="${STRIVO_SOAK_SESSIONS:-12}"
VOLUME_MB="${STRIVO_SOAK_VOLUME_MB:-48}"
KEEP="${STRIVO_SOAK_KEEP:-0}"

# Declared ceiling on everything this run may write. The capture phase needs
# roughly 50 MB of 128x32 H.264 for six hours of stream time; the tmpfs holds
# the pressure phase separately, so the scratch budget is generous but finite.
SCRATCH_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/strivo-soak-scratch.XXXXXX")"
VOLUME_DIR=""
MOUNTED=0

cleanup() {
  local rc=$?
  if [ "$MOUNTED" = 1 ] && [ -n "$VOLUME_DIR" ]; then
    sudo -n umount "$VOLUME_DIR" 2>/dev/null || true
  fi
  if [ "$KEEP" != 1 ]; then
    rm -rf "$SCRATCH_ROOT" "$VOLUME_DIR"
  else
    echo "kept: $SCRATCH_ROOT $VOLUME_DIR" >&2
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

for tool in ffmpeg ffprobe mkvmerge cargo; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "refusing: $tool is not on PATH" >&2
    exit 2
  }
done
if ! sudo -n true 2>/dev/null; then
  echo "refusing: no passwordless sudo, so the size-capped tmpfs cannot be created." >&2
  echo "         the pressure phase would otherwise fill an unbounded filesystem." >&2
  exit 2
fi

mkdir -p "$SCRATCH_ROOT/volume" "$SCRATCH_ROOT/work"
VOLUME_DIR="$SCRATCH_ROOT/volume"
# -o mode=0700 keeps the volume private to this user; nosuid/nodev/noexec stop
# anything the rehearsal writes from being executed.
sudo -n mount -t tmpfs -o "size=${VOLUME_MB}M,mode=0700,nosuid,nodev,noexec" \
  tmpfs "$VOLUME_DIR"
MOUNTED=1
# sudo mounts this root-owned, so mode=0700 alone would lock the invoking user
# out of the very directory the rehearsal has to fill.
sudo -n chown "$(id -u):$(id -g)" "$VOLUME_DIR"

mkdir -p "$(dirname "$OUT")"
{
  echo "# todo 39 capture soak $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# volume: $(df -h "$VOLUME_DIR" | tail -1)"
  echo "# ffmpeg: $(ffmpeg -version | head -1)"
  echo "# seed: $SEED  sessions: $SESSIONS  mutate: ${STRIVO_SOAK_MUTATE:-none}"
} >"$OUT"

# The harness exits 2 rather than skipping when the volume is unset, so
# passing it here is what makes the pressure phase mandatory instead of
# optional.
export STRIVO_SOAK_VOLUME="$VOLUME_DIR"
export STRIVO_SOAK_WORKDIR="$SCRATCH_ROOT/work"
export STRIVO_SOAK_SEED="$SEED"
export STRIVO_SOAK_SESSIONS="$SESSIONS"
export STRIVO_SOAK_MUTATE="${STRIVO_SOAK_MUTATE:-none}"

set +e
CARGO_TARGET_DIR="${STRIVO_SOAK_TARGET_DIR:-/tmp/opencode/t39-pvr}" \
  cargo run --quiet --locked -p strivo-core --example soak_capture -- \
  2>&1 | tee -a "$OUT"
STATUS="${PIPESTATUS[0]}"
set -e

# The harness refuses (exit 2) rather than skipping when the volume is absent,
# so a run that reached here did exercise the pressure phase.
if grep -q '^RESULT fail' "$OUT"; then
  STATUS=1
fi
echo "log: $OUT"
exit "$STATUS"
