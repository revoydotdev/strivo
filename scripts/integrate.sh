#!/usr/bin/env bash
# scripts/integrate.sh — rebase a concern branch onto the integration target,
# run quality gates, fast-forward-merge it in, and log the result.
#
# Usage:
#   integrate.sh <concern-branch> [options]
#
# Options:
#   --target <branch>      Branch to integrate into. Default: integration
#   --repo <path>          Git working tree to operate in. Default: toplevel
#                           of the current directory's git checkout. The
#                           caller MUST cd into (or pass --repo pointing at) a
#                           checkout where both <concern-branch> and <target>
#                           are real branches; this script checks them out in
#                           sequence there. It does not create worktrees.
#   --gate-cmd <cmd>       Shell command to run (via `bash -c`) as the quality
#                           gate, cwd = --repo. Default: "true" (no-op pass) —
#                           override this for a real milestone gate; a gate
#                           that always passes is a placeholder, not a promise.
#                           Also settable via $GATE_CMD.
#   --state-file <path>    Path (relative to --repo) of the ledger to append
#                           to on successful integration. Default: .agents/STATE.md
#   --dry-run              Do the rebase + gate check, then STOP — no
#                           ff-merge, no STATE.md write, branch restored.
#
# Exit codes: 0 on success (dry-run counts as success if rebase+gate pass).
# Nonzero on any failure. On failure the repo is left on (or restored to) the
# branch that was checked out before this script ran, and any failed rebase is
# reset — never left half-applied.
set -euo pipefail

TARGET="integration"
REPO=""
GATE_CMD="${GATE_CMD:-true}"
STATE_FILE=".agents/STATE.md"
DRY_RUN=0
BRANCH=""

usage() {
    cat >&2 <<'EOF'
usage: integrate.sh <concern-branch> [--target <branch>] [--repo <path>]
                     [--gate-cmd <cmd>] [--state-file <path>] [--dry-run]
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target)
            TARGET="$2"; shift 2 ;;
        --repo)
            REPO="$2"; shift 2 ;;
        --gate-cmd)
            GATE_CMD="$2"; shift 2 ;;
        --state-file)
            STATE_FILE="$2"; shift 2 ;;
        --dry-run)
            DRY_RUN=1; shift ;;
        -h|--help)
            usage; exit 0 ;;
        -*)
            echo "integrate.sh: unknown option: $1" >&2
            usage
            exit 1
            ;;
        *)
            if [ -n "$BRANCH" ]; then
                echo "integrate.sh: unexpected extra argument: $1" >&2
                usage
                exit 1
            fi
            BRANCH="$1"
            shift
            ;;
    esac
done

if [ -z "$BRANCH" ]; then
    echo "integrate.sh: <concern-branch> is required" >&2
    usage
    exit 1
fi

if [ -z "$REPO" ]; then
    REPO="$(git rev-parse --show-toplevel)"
fi

_git() {
    git -C "$REPO" "$@"
}

if ! _git rev-parse --verify --quiet "refs/heads/${BRANCH}" >/dev/null; then
    echo "integrate.sh: no such branch: ${BRANCH}" >&2
    exit 1
fi
if ! _git rev-parse --verify --quiet "refs/heads/${TARGET}" >/dev/null; then
    echo "integrate.sh: no such target branch: ${TARGET}" >&2
    exit 1
fi

ORIGINAL_REF="$(_git symbolic-ref --quiet --short HEAD || _git rev-parse HEAD)"
PRE_REBASE_SHA="$(_git rev-parse "refs/heads/${BRANCH}")"

_restore() {
    _git checkout --quiet "$ORIGINAL_REF" 2>/dev/null || true
}

fail() {
    echo "integrate.sh: FAIL — $1" >&2
    _restore
    exit 1
}

echo "integrate.sh: checking out ${BRANCH} in ${REPO}"
_git checkout --quiet "$BRANCH" || fail "could not checkout ${BRANCH}"

echo "integrate.sh: rebasing ${BRANCH} onto ${TARGET}"
if ! _git rebase "$TARGET"; then
    _git rebase --abort >/dev/null 2>&1 || true
    _git reset --quiet --hard "$PRE_REBASE_SHA" >/dev/null 2>&1 || true
    fail "rebase of ${BRANCH} onto ${TARGET} failed (conflicts?)"
fi

echo "integrate.sh: running gate: ${GATE_CMD}"
if ! (cd "$REPO" && bash -c "$GATE_CMD"); then
    _git reset --quiet --hard "$PRE_REBASE_SHA" >/dev/null 2>&1 || true
    fail "gate command failed: ${GATE_CMD}"
fi
echo "integrate.sh: gate passed"

if [ "$DRY_RUN" -eq 1 ]; then
    echo "integrate.sh: --dry-run set — stopping before ff-merge and STATE.md write"
    _restore
    echo "integrate.sh: DRY-RUN OK (rebase + gate verified, no merge performed)"
    exit 0
fi

echo "integrate.sh: fast-forward merging ${BRANCH} into ${TARGET}"
_git checkout --quiet "$TARGET" || fail "could not checkout ${TARGET}"
if ! _git merge --ff-only --quiet "$BRANCH"; then
    fail "ff-merge of ${BRANCH} into ${TARGET} failed (target has diverged?)"
fi

STATE_PATH="${REPO}/${STATE_FILE}"
mkdir -p "$(dirname "$STATE_PATH")"
TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
MERGE_SHA="$(_git rev-parse --short "$TARGET")"
echo "- ${TIMESTAMP} — integrated \`${BRANCH}\` into \`${TARGET}\` at \`${MERGE_SHA}\`" >> "$STATE_PATH"

_git add "$STATE_FILE"
if ! _git diff --cached --quiet; then
    _git commit --quiet -m "chore: integrate ${BRANCH} into ${TARGET}"
else
    echo "integrate.sh: warning — no STATE.md change staged (unexpected)" >&2
fi

echo "integrate.sh: OK — ${BRANCH} integrated into ${TARGET}"
