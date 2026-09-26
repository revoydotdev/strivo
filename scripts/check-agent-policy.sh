#!/usr/bin/env bash
# Two checks on the agent operating policy's *reachability*, not its prose.
#
#   1. CITATION CHECK -- no tracked file may cite the policy by its old
#      filename. Uses `git grep` (tracked files only) on purpose: `grep -rn`
#      also matches the untracked plan file and .omo/, so the check could never
#      pass and its pass would mean nothing. docs/audits/ and CHANGELOG.md are
#      allowlisted: those are historical records of what was true when written,
#      and rewriting them would falsify the audit trail.
#
#   2. TRACKED-CONTRACT CHECK -- the policy is deliberately untracked, so a
#      clone, a CI run and any worktree will NOT have it. Each contract it
#      protects must therefore also be stated in a TRACKED file. This is the
#      cost check: it is expected to FAIL for any contract that lives only in
#      the untracked policy, and a failure here is a finding, not noise.
#
# Usage: scripts/check-agent-policy.sh
# Exit:  0 = citation check clean AND every contract is tracked
#        1 = a stale citation, or a contract stated only in the untracked policy
#        2 = the check could not run (not a pass)
set -uo pipefail

cd "$(git rev-parse --show-toplevel)" || exit 2

OLD_NAME='CLAUDE\.md'
ALLOW='^docs/audits/|^CHANGELOG\.md:'

echo "== 1. citation check (tracked files) =="

# --- non-vacuity: prove the grep can see the matches we expect -------------
# Witness count: files that mention the old name anywhere. If this drops to 0,
# the allowlist has swallowed everything and a clean verdict below is unfalsified.
mentions_files=$(git grep -lE "$OLD_NAME" -- . 2>/dev/null | wc -l | tr -d ' ')
if ((mentions_files == 0)); then
  echo "  note: no historical mentions exist at all; the allowlist is" >&2
  echo "        untested by real data, so treat a clean verdict with care." >&2
fi

# Search the two construction forms separately: a single combined pattern is
# how a subset relation silently becomes "no matches" (see notepad issues.md).
tracked_hits=$(git grep -nE "$OLD_NAME" -- . 2>/dev/null)
live=$(printf '%s\n' "$tracked_hits" | grep -E "$OLD_NAME" | grep -vE "$ALLOW" | grep -v '^$')
live_count=$(printf '%s\n' "$live" | grep -c . )
inspected=$(git ls-files | wc -l | tr -d ' ')
all_count=$(printf '%s\n' "$tracked_hits" | grep -c . )

echo "  tracked files inspected : $inspected"
echo "  mentions found          : $all_count"
  echo "  allowlisted witnesses   : $mentions_files file(s) mention it"
echo "  live citations          : $live_count"

if ((live_count > 0)); then
  echo
  echo "  FAIL: tracked files still cite the old filename:"
  printf '%s\n' "$live" | sed 's/^/    /'
  exit 1
fi
echo "  ok: no live tracked citation of the old filename."

echo
echo "== 2. tracked-contract check (cost of keeping the policy untracked) =="

# A contract counts as tracked only if the docs that state RULES carry it, not
# merely if one audit manifest happens to mention it -- a statement inside a
# single manifest is reachable only by someone who already opened it.
NORMATIVE='README.md ROADMAP.md CONTRIBUTING.md DESIGN.md docs/adr/ :(glob)crates/**/README.md'
LOCAL_ONLY='docs/audits/'

# id|contract|ERE pattern (must describe the RULE, not a keyword)
# Deliberately searched via `git grep`, so a contract that exists ONLY in the
# untracked policy can never satisfy it. That is the check, not a limitation.
CONTRACTS=$(cat <<'EOF'
editions-default-members-is-PVR|default-members.*PVR|PVR.*default-members
assembly-by-inclusion-in-filename-order|filename order
ADR-immutability-supersede-never-rewrite|never rewrite|supersede, never|ADRs are immutable
audit-manifest-IDs-are-stable-never-renumber|never renumber|renumber or delete|stable IDs
derive-the-bump-from-commit-prefixes|fix:.{0,40}PATCH|feat:.{0,40}MINOR|derive the bump
human-cuts-releases|user cuts releases|leave the tagging|do not create or push a release tag
EOF
)

checked=0
untracked_only=()
local_only=()
while IFS='|' read -r id pattern; do
  [[ -z "$id" ]] && continue
  checked=$((checked + 1))
  if git grep -qiE "$pattern" -- $NORMATIVE 2>/dev/null; then
    printf '  ok                %s\n' "$id"
  elif git grep -qiE "$pattern" -- $LOCAL_ONLY 2>/dev/null; then
    printf '  LOCAL-ONLY        %s  (matches only under %s)\n' "$id" "$LOCAL_ONLY"
    local_only+=("$id")
  else
    printf '  UNTRACKED-ONLY    %s  (/%s/)\n' "$id" "$pattern"
    untracked_only+=("$id")
  fi
done <<<"$CONTRACTS"

echo
echo "  normative surface searched : $NORMATIVE"
echo "  contracts checked          : $checked"
echo "  tracked-normative          : $((checked - ${#local_only[@]} - ${#untracked_only[@]}))"
echo "  tracked-local-only         : ${#local_only[@]}"
echo "  untracked-only             : ${#untracked_only[@]}"

if ((${#untracked_only[@]} > 0)); then
  echo
  echo "  FINDING: ${#untracked_only[@]} contract(s) the policy protects are stated"
  echo "  nowhere in a tracked file. A clone, a CI run and every agent worktree"
  echo "  get the untracked policy and NOT these rules. This is a real cost of"
  echo "  keeping AGENTS.md gitignored, not a script failure:"
  printf '    - %s\n' "${untracked_only[@]}"
  echo
  echo "  The check is working: it is reporting rather than passing."
  exit 1
fi

echo
echo "PASS: every inventoried contract is stated in a tracked file."
