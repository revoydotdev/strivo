#!/usr/bin/env bash
# scripts/install-hooks.sh — activate the repo's tracked git hooks (M1G1).
#
# Sets core.hooksPath to the RELATIVE path ".githooks" so that it resolves
# within each worktree's own committed .githooks/ directory rather than a
# single absolute path shared (and possibly wrong) across worktrees.
# core.hooksPath lives in the shared .git/config, so running this once from
# any worktree activates hook enforcement for the whole repo, including all
# other worktrees.
#
# Idempotent: safe to run repeatedly; re-running just re-asserts the config.
#
# Usage:
#   scripts/install-hooks.sh            Install/activate the hooks path.
#   scripts/install-hooks.sh --verify   Check activation only; exit nonzero
#                                        with a clear message if not exactly
#                                        ".githooks".
set -euo pipefail

HOOKS_PATH=".githooks"

cmd_verify() {
    local current
    current="$(git config --get core.hooksPath || true)"
    if [ "${current}" != "${HOOKS_PATH}" ]; then
        echo "install-hooks.sh: FAIL — core.hooksPath is '${current:-<unset>}', expected '${HOOKS_PATH}'" >&2
        exit 1
    fi
    echo "install-hooks.sh: OK — core.hooksPath is '${HOOKS_PATH}'"
}

cmd_install() {
    git config core.hooksPath "${HOOKS_PATH}"
    echo "install-hooks.sh: core.hooksPath set to '${HOOKS_PATH}'"
}

case "${1:-}" in
    --verify)
        cmd_verify
        ;;
    "")
        cmd_install
        ;;
    *)
        echo "usage: $0 [--verify]" >&2
        exit 2
        ;;
esac
