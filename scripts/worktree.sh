#!/usr/bin/env bash
# scripts/worktree.sh — manage per-concern git worktrees for Project Skinner.
#
# Subcommands:
#   create <concern-tag> [base]   Create /home/revelri/Desktop/skinner-wt/<tag>
#                                  on branch concern/<tag>, off [base] (default: integration).
#   list                          List all worktrees (git worktree list passthrough).
#   destroy <tag>                 Remove the worktree at .../skinner-wt/<tag> and
#                                  delete its concern/<tag> branch.
#   --self-test                   Create + assert + destroy a throwaway worktree.
#
# Hard rules honored:
#  - Never touches main/integration/concern/* branches except via explicit
#    create/destroy of the tag the caller names.
#  - Self-test uses a unique throwaway tag (embeds $$ + timestamp) and cleans
#    up unconditionally via trap.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKTREE_ROOT="/home/revelri/Desktop/skinner-wt"

# Resolve the shared repo root. We must be inside one of the worktrees when
# this script runs; `git rev-parse --git-common-dir` gives the shared .git dir
# regardless of which worktree we're in.
_git_common_dir() {
    git rev-parse --git-common-dir
}

_repo_root_from_common_dir() {
    local common_dir
    common_dir="$(_git_common_dir)"
    # common_dir is typically <repo>/.git — its parent is the repo root.
    (cd "$common_dir/.." && pwd)
}

usage() {
    cat >&2 <<'EOF'
usage: worktree.sh create <concern-tag> [base-branch]
       worktree.sh list
       worktree.sh destroy <tag>
       worktree.sh --self-test
EOF
}

cmd_create() {
    local tag="${1:?concern-tag required}"
    local base="${2:-integration}"
    local dest="${WORKTREE_ROOT}/${tag}"
    local branch="concern/${tag}"

    if [ -e "$dest" ]; then
        echo "worktree.sh: destination already exists: $dest" >&2
        return 1
    fi

    if git rev-parse --verify --quiet "refs/heads/${branch}" >/dev/null; then
        echo "worktree.sh: branch already exists: ${branch}" >&2
        return 1
    fi

    if ! git rev-parse --verify --quiet "refs/heads/${base}" >/dev/null \
        && ! git rev-parse --verify --quiet "refs/remotes/${base}" >/dev/null; then
        echo "worktree.sh: base branch not found: ${base}" >&2
        return 1
    fi

    git worktree add -b "$branch" "$dest" "$base"
    echo "worktree.sh: created $dest on branch $branch (from $base)"

    # Activate commit-msg enforcement (ADR-0017 / M1G1), resolved relative to
    # this script's own location (not _repo_root_from_common_dir, which
    # points at the main worktree checkout and may lag the branch currently
    # running worktree.sh). core.hooksPath lives in the shared .git/config,
    # so this also activates it for the calling worktree and every other
    # existing worktree, not just the new one.
    "${SCRIPT_DIR}/install-hooks.sh"
}

cmd_list() {
    git worktree list
}

cmd_destroy() {
    local tag="${1:?tag required}"
    local dest="${WORKTREE_ROOT}/${tag}"
    local branch="concern/${tag}"

    if [ ! -e "$dest" ]; then
        echo "worktree.sh: no such worktree dir: $dest" >&2
        return 1
    fi

    git worktree remove --force "$dest"

    if git rev-parse --verify --quiet "refs/heads/${branch}" >/dev/null; then
        git branch -D "$branch"
    fi
    echo "worktree.sh: destroyed $dest and branch $branch"
}

self_test() {
    local tag="selftest-$$-$(date +%s)"
    local dest="${WORKTREE_ROOT}/${tag}"
    local branch="concern/${tag}"
    local failed=0

    cleanup() {
        # Best-effort teardown regardless of outcome.
        git worktree remove --force "$dest" >/dev/null 2>&1 || true
        git branch -D "$branch" >/dev/null 2>&1 || true
    }
    trap cleanup EXIT

    echo "worktree.sh self-test: creating $tag"
    if ! cmd_create "$tag" integration; then
        echo "worktree.sh self-test: FAIL create" >&2
        exit 1
    fi

    if ! git worktree list | grep -qF "$dest"; then
        echo "worktree.sh self-test: FAIL — worktree not found in 'git worktree list' after create" >&2
        failed=1
    else
        echo "worktree.sh self-test: PASS — worktree present after create"
    fi

    if [ ! -d "$dest/.git" ] && [ ! -f "$dest/.git" ]; then
        echo "worktree.sh self-test: FAIL — $dest/.git missing" >&2
        failed=1
    fi

    echo "worktree.sh self-test: destroying $tag"
    if ! cmd_destroy "$tag"; then
        echo "worktree.sh self-test: FAIL destroy" >&2
        exit 1
    fi

    if git worktree list | grep -qF "$dest"; then
        echo "worktree.sh self-test: FAIL — worktree still listed after destroy" >&2
        failed=1
    else
        echo "worktree.sh self-test: PASS — worktree absent after destroy"
    fi

    if git rev-parse --verify --quiet "refs/heads/${branch}" >/dev/null; then
        echo "worktree.sh self-test: FAIL — branch ${branch} still exists after destroy" >&2
        failed=1
    else
        echo "worktree.sh self-test: PASS — branch ${branch} absent after destroy"
    fi

    trap - EXIT

    if [ "$failed" -ne 0 ]; then
        echo "worktree.sh self-test: OVERALL FAIL" >&2
        exit 1
    fi

    echo "worktree.sh self-test: OVERALL PASS"
}

main() {
    local sub="${1:-}"
    case "$sub" in
        create)
            shift
            cmd_create "$@"
            ;;
        list)
            shift
            cmd_list "$@"
            ;;
        destroy)
            shift
            cmd_destroy "$@"
            ;;
        --self-test|self-test)
            self_test
            ;;
        -h|--help|"")
            usage
            [ "$sub" = "" ] && exit 1
            ;;
        *)
            echo "worktree.sh: unknown subcommand: $sub" >&2
            usage
            exit 1
            ;;
    esac
}

main "$@"
