#!/usr/bin/env bash
# scripts/preflight.sh — one-call, structured tick preflight (token economy).
#
# Replaces the turn-by-turn reasoned git-plumbing the supervisor re-derives every
# tick (TICK_PROMPT STEP 2) with a single deterministic classifier. It fetches,
# fast-forwards local main/integration if cleanly behind origin, and classifies
# the tree. It NEVER clobbers, force-updates, or deletes anything — recovery
# itself stays a supervisor decision; this only reports what is true.
#
# Output (last line is the verdict the caller branches on):
#   CLEAN                          — safe to proceed with a normal tick
#   RECOVER:<reason>[;<reason>...] — killed-tick residue (orphans / stale lock /
#                                    divergence); supervisor should run recovery
#   DIRTY:<n> files                — foreign uncommitted changes present
#
# Exit code mirrors the verdict: 0 CLEAN, 3 RECOVER, 4 DIRTY (so a driver can
# branch on $? without parsing). Read-mostly; the only writes are safe ff-merges.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo .)"
cd "$ROOT" || { echo "DIRTY:0 files (no repo)"; exit 4; }
LOCK=".agents/locks/RUN.lock"
reasons=()

git fetch -q origin 2>/dev/null || true

# fast-forward local refs that are cleanly behind origin (never non-ff)
for br in main integration; do
  if git show-ref -q --verify "refs/heads/$br" && git show-ref -q --verify "refs/remotes/origin/$br"; then
    if git merge-base --is-ancestor "$br" "origin/$br" 2>/dev/null && \
       ! git merge-base --is-ancestor "origin/$br" "$br" 2>/dev/null; then
      cur="$(git rev-parse --abbrev-ref HEAD)"
      if [ "$cur" = "$br" ]; then git merge --ff-only -q "origin/$br" 2>/dev/null || true
      else git update-ref "refs/heads/$br" "origin/$br" 2>/dev/null || true; fi
    fi
  fi
done

# stale lock?
if [ -f "$LOCK" ]; then
  age=$(( $(date +%s) - $(stat -c %Y "$LOCK" 2>/dev/null || echo 0) ))
  if [ "$age" -gt 1500 ]; then reasons+=("stale-lock(${age}s)"); fi
fi

# orphaned extra worktrees / leftover concern branches
if [ "$(git worktree list --porcelain 2>/dev/null | grep -c '^worktree ')" -gt 1 ]; then
  reasons+=("extra-worktrees")
fi
if git for-each-ref --format='%(refname:short)' 'refs/heads/concern/*' 2>/dev/null | grep -q .; then
  reasons+=("concern-branches")
fi

# integration diverged from main (non-ff both ways)
if git show-ref -q --verify refs/heads/main && git show-ref -q --verify refs/heads/integration; then
  if ! git merge-base --is-ancestor main integration 2>/dev/null; then
    reasons+=("integration-behind-main")
  fi
fi

# foreign uncommitted changes
dirty="$(git status --porcelain 2>/dev/null | grep -v '^?? .agents/' | wc -l | tr -d ' ')"

if [ "$dirty" -gt 0 ]; then
  echo "DIRTY:$dirty files"; exit 4
fi
if [ "${#reasons[@]}" -gt 0 ]; then
  IFS=';'; echo "RECOVER:${reasons[*]}"; exit 3
fi
echo "CLEAN"; exit 0
