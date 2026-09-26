#!/usr/bin/env bash
# Live-clipping-during-capture harness (plan todo 57, CE-P8).
#
# docs/CREATOR-EDITION-REPORT-2026-09-07.md:284 (C05) asks for a rehearsal in
# which a reviewed clip renders from FINALIZED capture segments while
# recording continues, and does not corrupt or starve acquisition. The
# library half of that is `cargo test` (src/recording/live_clip.rs and
# crates/research/src/creator/{live_clip,tests/live_clip}.rs). This script is
# the half that needs real binaries and a real size-capped volume:
#
#   * every capture part encoded by ffmpeg and fed to the real `LiveCapture`
#     tail exactly as src/recording/mod.rs feeds it;
#   * a live clip rendered MID-CAPTURE through the real `render_live_clip`
#     (mkvmerge join, ffmpeg -c copy), fully decoded and duration-probed;
#   * a request reaching into the part currently being written refused with
#     `LiveClipError::PartStillOpen`, counted and asserted;
#   * six hours of REAL stream time audited against the interruption ledger
#     for zero unreported gaps, with STRIVO_SOAK_MUTATE proving the audit can
#     fail;
#   * a volume filled with real bytes until free space reaches capture's
#     reserve, asserting capture is still admitted and still writes a part
#     that decodes while a live clip is refused WITH A REASON.
#
# It is a separate script from todo 39's soak_capture_pressure.sh rather than
# an edit to it: that one rehearses recovery and disk pressure with no clip
# in the picture, and its phase 2 admits a render through the registry without
# ever reading a segment. This one reads segments mid-capture, which is the
# thing todo 39 could not claim. It reuses todo 39's conventions rather than
# reinventing them: refuse to run against a real install, keep every byte
# under a private directory, exit non-zero rather than skipping.
#
# The volume is a size-capped tmpfs created here and unmounted on every exit
# path, so "fill the disk" cannot reach a real filesystem. Without passwordless
# sudo the tmpfs cannot be created and this script refuses (exit 2) instead of
# running the saturation phase against an unbounded filesystem.
#
# Usage: scripts/soak_live_clip.sh [out.log]
# Env:   STRIVO_SOAK_SEED        decimal seed for the interruption schedule
#        STRIVO_SOAK_SESSIONS    capture sessions across the window (default 6)
#        STRIVO_SOAK_VOLUME_MB   tmpfs size in MB (default 48)
#        STRIVO_SOAK_MUTATE      drop-ledger-entry | phantom-ledger-entry | none
#        STRIVO_SOAK_KEEP        1 to keep the tmpfs and scratch dir
# Exit:  0 every acceptance check passed
#        1 a check failed (the run prints each one)
#        2 refused: no volume, no ffmpeg/ffprobe/mkvmerge, or no sudo
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT="${1:-$REPO_ROOT/artifacts/soak-live-clip-$(date -u +%Y%m%dT%H%M%SZ).log}"
SEED="${STRIVO_SOAK_SEED:-24357}"
SESSIONS="${STRIVO_SOAK_SESSIONS:-6}"
VOLUME_MB="${STRIVO_SOAK_VOLUME_MB:-48}"
KEEP="${STRIVO_SOAK_KEEP:-0}"

SCRATCH_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/strivo-soak-live.XXXXXX")"
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
  echo "         the saturation phase would otherwise fill an unbounded filesystem." >&2
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
  echo "# todo 57 live-clip soak $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# volume: $(df -h "$VOLUME_DIR" | tail -1)"
  echo "# ffmpeg: $(ffmpeg -version | head -1)"
  echo "# seed: $SEED  sessions: $SESSIONS  mutate: ${STRIVO_SOAK_MUTATE:-none}"
} >"$OUT"

# The harness exits 2 rather than skipping when the volume is unset, so
# passing it here is what makes the saturation phase mandatory instead of
# optional.
export STRIVO_SOAK_VOLUME="$VOLUME_DIR"
export STRIVO_SOAK_WORKDIR="$SCRATCH_ROOT/work"
export STRIVO_SOAK_SEED="$SEED"
export STRIVO_SOAK_SESSIONS="$SESSIONS"
export STRIVO_SOAK_MUTATE="${STRIVO_SOAK_MUTATE:-none}"

set +e
CARGO_TARGET_DIR="${STRIVO_SOAK_TARGET_DIR:-/tmp/opencode/t57-pvr}" \
  cargo run --quiet --locked -p strivo-core --example soak_live_clip -- \
  2>&1 | tee -a "$OUT"
STATUS="${PIPESTATUS[0]}"
set -e

# The harness refuses (exit 2) rather than skipping when the volume is absent,
# so a run that reached here did exercise the saturation phase.
if grep -q '^RESULT fail' "$OUT"; then
  STATUS=1
fi
echo "log: $OUT"
exit "$STATUS"
