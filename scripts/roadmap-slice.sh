#!/usr/bin/env bash
# scripts/roadmap-slice.sh <milestone> — print ONLY the given milestone's section
# of ROADMAP.md (its `# M<n> —` H1 heading through the line before the next H1).
#
# Token economy (ADR-0022 lineage): a tick advancing M7 has no need to re-ingest
# all 8 milestones of the 45KB ROADMAP every 20 minutes. This slices out the one
# live milestone (its phases, stages, sub-tasks, and `M#G#` gate list, which all
# live inside that H1 section). Read-only; deterministic; no LLM.
#
# Usage:  scripts/roadmap-slice.sh M7
set -euo pipefail

MS="${1:?usage: roadmap-slice.sh <milestone, e.g. M7>}"
ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
ROADMAP="$ROOT/ROADMAP.md"
[ -f "$ROADMAP" ] || ROADMAP="ROADMAP.md"

awk -v ms="$MS" '
  /^# M[0-9]+([ ]|$|—)/ || /^# M[0-9]+ / {
    if ($2 == ms) { inms = 1; print; next }
    else if (inms) { exit }
    else { next }
  }
  inms { print }
' "$ROADMAP"
