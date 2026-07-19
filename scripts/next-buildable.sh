#!/usr/bin/env bash
# scripts/next-buildable.sh — decide, WITHOUT an LLM, whether a tick has any
# buildable work, so the driver can skip a full Opus turn on a "nothing to do,
# same external blockers as last tick" tick (token-economics postmortem #7).
#
# Reads the CONTROL block (phase + milestone) from .agents/STATE.md and the
# machine-readable infra-gate manifest .agents/infra-gates.txt (todos blocked on
# external infra the swarm cannot provision). Verdict (last stdout line) + exit:
#
#   RUN:phase=<AUDIT|REMEDIATION>        exit 0  — not a NORMAL tick; must run
#   COMPLETE:<M> no remaining todos      exit 0  — milestone done; run to flip AUDIT
#   BUILDABLE:<n> (<ids>)                exit 0  — real work exists; dispatch
#   BLOCKED:all remaining infra-gated    exit 3  — safe to SKIP the Opus dispatch
#
# FAIL-SAFE: exit 3 (skip) is returned ONLY for the unambiguous all-infra-gated
# case. Any parse error, missing tool, or uncertainty falls through to exit 0.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || { echo "RUN:no-repo"; exit 0; }
STATE=".agents/STATE.md"
GATES=".agents/infra-gates.txt"
DIRECTIVES=".agents/governance/directives.json"

# Operator pause (governance channel): skip the tick entirely if explicitly paused.
# Fail-safe — only an explicit `true` skips; any parse issue falls through to RUN.
if [ -f "$DIRECTIVES" ]; then
  paused="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("paused"))' "$DIRECTIVES" 2>/dev/null)"
  if [ "$paused" = "True" ]; then echo "PAUSED:operator directive (governance)"; exit 3; fi
fi

phase="$(grep -m1 '^- MILESTONE_PHASE:' "$STATE" 2>/dev/null | sed 's/^- MILESTONE_PHASE:[[:space:]]*//')"
ms="$(grep -m1 '^- CURRENT_MILESTONE:' "$STATE" 2>/dev/null | sed 's/^- CURRENT_MILESTONE:[[:space:]]*//')"
phase="${phase%%[[:space:]]*}"; ms="${ms%%[[:space:]]*}"

# Uncertain control state -> run (never skip on ambiguity).
[ -z "$phase" ] && { echo "RUN:phase-unknown"; exit 0; }
[ -z "$ms" ] && { echo "RUN:milestone-unknown"; exit 0; }
[ "$phase" != "NORMAL" ] && { echo "RUN:phase=$phase"; exit 0; }

# Remaining (unclaimed) todos for the milestone, from the ledger source of truth.
mapfile -t remaining < <(python3 scripts/ledger.py next --milestone "$ms" 2>/dev/null | sed -n 's/^[[:space:]]\{2,\}\(M[0-9].*\)$/\1/p') || true
if [ "${#remaining[@]}" -eq 0 ]; then
  echo "COMPLETE:$ms no remaining todos"; exit 0
fi

# Load infra-gate ids (ignore blanks/comments).
declare -A infra=()
if [ -f "$GATES" ]; then
  while read -r line; do
    line="${line%%#*}"; line="${line//[[:space:]]/}"
    [ -n "$line" ] && infra["$line"]=1
  done < "$GATES"
fi

buildable=()
for t in "${remaining[@]}"; do
  [ -n "${infra[$t]:-}" ] || buildable+=("$t")
done

if [ "${#buildable[@]}" -eq 0 ]; then
  echo "BLOCKED:all ${#remaining[@]} remaining infra-gated (${remaining[*]})"; exit 3
fi
echo "BUILDABLE:${#buildable[@]} (${buildable[*]})"; exit 0
