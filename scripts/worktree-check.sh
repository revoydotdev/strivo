#!/usr/bin/env bash
# scripts/worktree-check.sh [--prune-salvage] — deterministic worktree/branch
# orphan classifier (revoy-spec ADR-084 taxonomy, distilled; friction F1).
#
# Killed ticks leave worktrees, `concern/*` branches, and `.agents/salvage/*`
# dirs behind; today the supervisor re-derives the same classification by hand in
# prose every time. This labels each once:
#   orphaned-landed   — commits already on `integration` -> safe to remove
#   orphaned-unlanded — has commits NOT on `integration` -> SALVAGE before removal
#   active            — the primary/main worktree (leave alone)
# and classifies `concern/*` branches as merged (safe delete) or unmerged.
#
# Salvage dirs are only REPORTED by default. With --prune-salvage it removes
# .agents/salvage/* dirs older than 24h (the residue is a copy; its commits, if
# any, already landed or are preserved on their branch) — opt-in, never automatic.
#
# Exit 0 if nothing needs attention; exit 3 if any orphaned-unlanded worktree or
# unmerged concern branch exists (a human/supervisor should look).
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || exit 0
PRUNE=0; [ "${1:-}" = "--prune-salvage" ] && PRUNE=1
attention=0
INT="integration"

echo "== worktrees =="
main_wt="$(git rev-parse --show-toplevel)"
while IFS= read -r wt; do
  [ -z "$wt" ] && continue
  if [ "$wt" = "$main_wt" ]; then
    echo "  active            $wt"
    continue
  fi
  head="$(git -C "$wt" rev-parse HEAD 2>/dev/null || echo '?')"
  if [ "$head" != "?" ] && git merge-base --is-ancestor "$head" "$INT" 2>/dev/null; then
    echo "  orphaned-landed   $wt ($head on $INT — safe: git worktree remove)"
  else
    echo "  orphaned-unlanded $wt ($head NOT on $INT — SALVAGE first)"; attention=1
  fi
done < <(git worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p')

echo "== concern/* branches =="
found_branch=0
while IFS= read -r br; do
  [ -z "$br" ] && continue
  found_branch=1
  tip="$(git rev-parse "$br" 2>/dev/null || echo '?')"
  if git merge-base --is-ancestor "$br" "$INT" 2>/dev/null; then
    echo "  merged            $br ($tip — safe: git branch -d)"
  else
    echo "  unmerged          $br ($tip — NOT on $INT)"; attention=1
  fi
done < <(git for-each-ref --format='%(refname:short)' 'refs/heads/concern/*' 2>/dev/null)
[ "$found_branch" -eq 0 ] && echo "  (none)"

echo "== .agents/salvage =="
if [ -d .agents/salvage ] && [ -n "$(ls -A .agents/salvage 2>/dev/null)" ]; then
  for d in .agents/salvage/*/; do
    [ -d "$d" ] || continue
    age=$(( ($(date +%s) - $(stat -c %Y "$d" 2>/dev/null || echo 0)) / 3600 ))
    if [ "$PRUNE" -eq 1 ] && [ "$age" -ge 24 ]; then
      # salvage dirs may be tracked — git rm if so, else plain rm.
      git rm -rq -r "$d" 2>/dev/null || rm -rf "$d"
      echo "  pruned            $d (${age}h old)"
    else
      echo "  present           $d (${age}h old${d:+; --prune-salvage removes if >=24h})"
    fi
  done
else
  echo "  (none)"
fi

exit "$([ "$attention" -eq 1 ] && echo 3 || echo 0)"
