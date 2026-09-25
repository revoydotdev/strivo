#!/usr/bin/env bash
# Fail if any std::process::Command is constructed directly in an async fn
# body, i.e. executed on a Tokio worker thread. Audit findings F-32 / the
# `spawn_blocking` migration in CHANGELOG 0.6.0.
#
# WHY AN AST SEARCH AND NOT grep: `grep -n 'Command::new'` cannot tell an
# `async fn` body from a sync helper one, cannot tell `std::process::Command`
# from `tokio::process::Command`, and cannot see that a construction wrapped in
# a `spawn_blocking` closure is off the reactor. It would answer wrongly in
# both directions. ast-grep parses the file, so the classification is by shape.
#
# The rule, in words, and what it deliberately does NOT cover:
#
#   1. `std::process::Command::new(...)` and the `use std::process::Command;`
#      + `Command::new(...)` form. Verified non-overlapping patterns: the bare
#      form does not match a `tokio::process::Command::new` call, so the async
#      process API is never flagged.
#   2. ...lexically inside an `async fn` body (`stopBy: end`, so it reaches the
#      whole body, not just the immediate parent node).
#   3. ...and NOT inside any `spawn_blocking` call.
#
# A `Command::new` in a *sync* helper that is *called from* an async fn is a
# different case and is NOT flagged: the hazard there is the call, not the
# construction, and it is legal exactly when the call is reached through
# `spawn_blocking`. The runner prints those sites separately so they stay
# visible instead of being silently classified as fine.
#
# Usage: scripts/check-no-blocking-command.sh [PATH ...]
#        defaults to the two trees the audit scoped:
#          crates/strivo-web/src/routes  crates/strivo-plugins
#
# Exit 0 = clean, 1 = at least one on-reactor construction, 2 = setup problem.
set -euo pipefail

cd "$(dirname "$0")/.."

RULES=scripts/ast-grep
GATE_RULE="$RULES/std-command-on-reactor.yml"
CLASSIFY_RULE="$RULES/std-command-in-async-fn.yml"
DEFAULT_PATHS=(crates/strivo-web/src/routes crates/strivo-plugins)

paths=("$@")
if [ ${#paths[@]} -eq 0 ]; then
  paths=("${DEFAULT_PATHS[@]}")
fi

if ! command -v ast-grep >/dev/null 2>&1; then
  echo "check-no-blocking-command: ast-grep not on PATH" >&2
  echo "  install: npm i -g @ast-grep/cli   (or: cargo install ast-grep --locked)" >&2
  exit 2
fi

for rule in "$GATE_RULE" "$CLASSIFY_RULE"; do
  [ -f "$rule" ] || { echo "check-no-blocking-command: missing $rule" >&2; exit 2; }
done

for p in "${paths[@]}"; do
  [ -e "$p" ] || { echo "check-no-blocking-command: no such path: $p" >&2; exit 2; }
done

# ast-grep `run` exits 1 both when a pattern matched nothing and when the tool
# itself failed, so the exit code cannot discriminate. Parseability can: a clean
# `[]` is a real zero; empty or unparseable output means the plumbing broke, and
# reporting "zero hits" for that would be a check that passes by not running.
loc() {
  python3 -c '
import json, sys
try:
    findings = json.load(sys.stdin)
except ValueError:
    sys.exit("ast-grep produced no parseable JSON; refusing to report zero hits")
for f in sorted(findings, key=lambda f: (f["file"], f["range"]["start"]["line"])):
    print("%s:%d" % (f["file"], f["range"]["start"]["line"] + 1))
'
}

scan() {
  ast-grep scan -r "$1" --json=compact "${paths[@]}" 2>/dev/null || true
}

# ast-grep refuses `--pattern` twice on one invocation, so the two (verified
# disjoint) construction forms are searched separately and merged.
all_sites=$(
  for pattern in 'std::process::Command::new($$$ARGS)' 'Command::new($$$ARGS)'; do
    err=$(mktemp)
    out=$(ast-grep run --lang rust --json=compact --pattern "$pattern" "${paths[@]}" 2>"$err") || true
    if ! printf '%s' "$out" | loc; then
      sed 's/^/  /' "$err" >&2
      rm -f "$err"
      exit 2
    fi
    rm -f "$err"
  done | sort -u
)
in_async=$(scan "$CLASSIFY_RULE" | loc) || exit 2
on_reactor=$(scan "$GATE_RULE" | loc) || exit 2

n_all=$(printf '%s' "$all_sites" | grep -c . || true)
n_async=$(printf '%s' "$in_async" | grep -c . || true)
n_reactor=$(printf '%s' "$on_reactor" | grep -c . || true)
n_wrapped=$(( n_async - n_reactor ))

# The two classifier passes search the same trees with the same construction
# patterns, so B can never exceed A. If it does, one pass used different
# patterns than the other and every number below is void — fail instead of
# printing a negative remainder.
if [ "$n_async" -gt "$n_all" ]; then
  echo "check-no-blocking-command: internal error — $n_async in-async hits but" >&2
  echo "  only $n_all total sites; the two passes disagree, results are void." >&2
  exit 2
fi

echo "std::process::Command shape search — ${paths[*]}"
echo "  A. blocking constructions (anywhere)          : $n_all"
echo "  B. ...lexically inside an async fn body       : $n_async"
echo "  B1.  ....inside a spawn_blocking closure (OK) : $n_wrapped"
echo "  B2.  ....on the reactor            (VIOLATION): $n_reactor"
echo "  C. in a sync fn body, never in an async body  : $(( n_all - n_async ))"

if [ "$n_reactor" -gt 0 ]; then
  echo
  echo "FAIL: std::process::Command built on a Tokio worker thread:"
  printf '%s\n' "$on_reactor" | sed 's/^/  /'
  echo
  echo "Wrap the whole blocking sequence in tokio::task::spawn_blocking, or"
  echo "switch to tokio::process::Command (async, cancellable)."
  exit 1
fi

# A `Command::new` in a sync helper reached from an async fn is legal only
# when the *call* goes through spawn_blocking. That is a call-graph question,
# not a lexical one, so list the sites rather than pretending they are cleared.
sync_only=$(comm -23 \
  <(printf '%s\n' "$all_sites" | sort) \
  <(printf '%s\n' "$in_async" | sort) || true)
if [ -n "$sync_only" ]; then
  echo
  echo "REVIEW: blocking construction in a sync fn (each must be reached from an"
  echo "async fn only via spawn_blocking, or from a sync caller):"
  printf '%s\n' "$sync_only" | sed 's/^/  /'
fi

echo
echo "OK: zero std::process::Command constructions on the reactor."
