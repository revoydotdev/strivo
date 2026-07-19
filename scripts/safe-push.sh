#!/usr/bin/env bash
# scripts/safe-push.sh [git-push-args...] — operator-side companion to the
# .githooks/pre-push guard for the LAPTOP case (revoy-spec friction F5).
#
# The pre-push hook can only see a LOCAL RUN.lock. When the operator pushes from
# the laptop while a tick runs on daedalus, the lock lives on daedalus, not
# locally. This wrapper checks the canonical daedalus repo's RUN.lock over the
# `daedalus` ssh wrapper before pushing, and refuses if a tick is active.
#
# Usage:  scripts/safe-push.sh origin integration
set -euo pipefail

REMOTE_LOCK='test -f ~/Dev/revelri/project-skinner/.agents/locks/RUN.lock'
if command -v daedalus >/dev/null 2>&1; then
  if daedalus "$REMOTE_LOCK" 2>/dev/null; then
    echo "safe-push: REFUSED — daedalus RUN.lock is held (a tick is running)." >&2
    echo "safe-push: pause skinner-tick.timer or wait for the lock to clear." >&2
    exit 1
  fi
fi
exec git push "$@"
