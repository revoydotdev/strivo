#!/usr/bin/env bash
# Structural verified_by gate over .agents/ledger.jsonl (ADR-0022).
# Default: every `done` todo must carry a verified_by {cmd, exit:0}. Cheap; wired
# into ci.sh so it runs every tick. `--rerun` re-executes live entries (audit use).
exec python3 "$(dirname "$0")/ledger.py" check "$@"
