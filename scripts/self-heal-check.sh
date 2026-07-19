#!/usr/bin/env bash
# scripts/self-heal-check.sh <todo-id> [--cmd '<verify cmd>'] [--record]
#
# Token economy: the "before building a claimed todo, check whether it is already
# done / its artifact already passes" self-heal step (TICK_PROMPT) needs no LLM
# judgment — it is a ledger lookup plus (optionally) re-running the verify cmd.
#
#   - Reports DONE if the todo already has a `done` in the ledger (skip rebuild).
#   - With --cmd, runs the verify command:
#       * exit 0 -> PASS  (and with --record, records the todo done via
#                          `ledger.py done --run`, reconciling a ledger gap free)
#       * nonzero -> NEEDS-WORK (the todo genuinely has to be built)
#   - Without --cmd and not already done -> UNKNOWN (caller must supply the cmd).
#
# Deterministic; the verify cmd is authoritative (provenance-as-a-type, ADR-0022).
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || exit 2
TODO="${1:?usage: self-heal-check.sh <todo-id> [--cmd '<cmd>'] [--record]}"; shift || true

CMD=""; RECORD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --cmd) CMD="${2:-}"; shift 2 ;;
    --record) RECORD=1; shift ;;
    *) shift ;;
  esac
done

# already recorded done?
if python3 scripts/ledger.py status 2>/dev/null | grep -qw "$TODO"; then
  echo "DONE: $TODO already recorded in ledger — skip rebuild"; exit 0
fi

if [ -z "$CMD" ]; then
  echo "UNKNOWN: $TODO not in ledger and no --cmd given — supply the verify cmd"; exit 0
fi

if [ "$RECORD" -eq 1 ]; then
  if python3 scripts/ledger.py done --todo "$TODO" --cmd "$CMD" --run >/dev/null 2>&1; then
    echo "PASS: $TODO verify passed — recorded done (self-healed, no rebuild)"; exit 0
  else
    echo "NEEDS-WORK: $TODO verify failed — must be built"; exit 3
  fi
else
  if bash -c "$CMD" >/dev/null 2>&1; then
    echo "PASS: $TODO artifact already passes (not recorded; pass --record to reconcile)"; exit 0
  else
    echo "NEEDS-WORK: $TODO verify failed — must be built"; exit 3
  fi
fi
