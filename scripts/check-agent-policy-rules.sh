#!/usr/bin/env bash
# Rule-inventory diff for the agent operating policy.
#
# The policy file (AGENTS.md) is gitignored, so it has no VCS history to diff
# against. This script embeds the rule inventory extracted from the file it was
# converted from: every rule that must survive the conversion, with one or more
# fixed-string anchors that MUST appear in the current policy. A dropped,
# renamed-away or gutted rule shows up as an unmatched anchor.
#
# The inventory count is asserted against RULES below. Editing the inventory to
# make a failure disappear fails the check instead of passing it, which is the
# point: the number of rules is a floor, not a target.
#
# Usage: scripts/check-agent-policy-rules.sh [path-to-policy-file]
# Exit:  0 = every inventoried rule is present
#        1 = at least one rule dropped, or the policy file is missing/empty
set -uo pipefail

cd "$(git rev-parse --show-toplevel)" || exit 2

POLICY="${1:-AGENTS.md}"
EXPECTED_RULES=25

# id|section|rule|anchor1;;anchor2[;;anchor3...]
RULES=$(cat <<'EOF'
R01|stack|no terminal UI at all; "the UI" means the web SPA|Nothing in this project renders to a;;terminal UI; "the UI" always means the web SPA.
R02|editions|default cargo build is the PVR edition, gated by default-members|restricted by `default-members` in the root `Cargo.toml`.;;the **PVR edition**: core + `strivo-bin` +
R03|editions|Creator is a cargo feature; creator crates stay workspace members but leave the default set|excluded from the default set.;;which pulls in
R04|editions|Creator content is absent from a PVR build by cfg gate, not a runtime if|absent** from a PVR build — not merely;;rather than a runtime `if`.
R05|assets|modules are ordered <seq>-<edition>.<ext>, concatenated in filename order|in **filename order**, including `-creator` files only when the feature is on.;;<seq>-pvr.<ext>` / `<seq>-creator.<ext>`
R06|assets|edit the module files; no checked-in bundle; target/ is build output|There is no checked-in `assets/spa.js`;;any you find under `target/` is build output.
R07|assets|release builds minify; comments survive only in debug|Release builds minify the assembled JS/CSS (oxc + lightningcss).;;survive into the binary in debug builds but not release.
R08|assets|modules are mid-file fragments; per-module parse errors are noise|are **fragments**, split mid-file;;Judge syntax on the assembled bundle.
R09|assets|mime_for() maps only css/js/svg/png; .mjs needs a new branch and a route test|`mime_for()` only knows four extensions.;;a route test asserting the served `Content-Type`
R10|assets|assets/ is embedded wholesale; only research/ is removed, so anything vendored ships in every PVR binary|`assets/` is embedded wholesale, so vendored files ship in every binary.;;ships inside every PVR binary** unless an explicit `remove_dir_all` /
R11|design|DESIGN.md is the visual authority; deviations need approval|Always read DESIGN.md before making any visual or UI decisions.;;In QA mode, flag any code that doesn't match DESIGN.md.
R12|testing|cargo test/clippy --workspace --all-targets is the gate; core-alone config preserved too|cargo clippy --workspace --all-targets` as;;compiles and passes on its own since CE01
R13|testing|browser E2E lives in crates/strivo-web/e2e/|Browser E2E lives in `crates/strivo-web/e2e/`:
R14|testing|npm test is the mock lane; pretest runs the PVR bundle vocabulary check|`npm test` — mock-backend lane;;fails the lane if
R15|testing|npm run test:real boots the real binaries in a sandboxed XDG set|`npm run test:real` — boots the real compiled binaries into a sandboxed;;so it can never touch a real
R16|testing|Playwright resolves from e2e/node_modules; scripts live there too|Playwright resolves from `e2e/node_modules`
R17|docs|ADRs are immutable; supersede, never rewrite; update the status table|**ADRs are immutable.** Supersede, never rewrite; update the status table in;;`docs/adr/README.md`.
R18|docs|audit manifest IDs are stable; never renumber or delete, close and append evidence|**Audit manifests use stable IDs.**;;never renumber or delete an item — close it and append the evidence.
R19|releases|release tags follow SemVer 2.0.0, vMAJOR.MINOR.PATCH|Release tags **must** follow Semantic Versioning 2.0.0 (semver.org):
R20|releases|derive the bump from conventional-commit prefixes since the last tag|**Derive the bump, never hand-pick it.**;;Other prefixes
R21|releases|single source of truth in [workspace.package]; do not bypass the release gate|**Single source of truth.** The version lives once in `[workspace.package]`;;do not bypass that gate.
R22|releases|released versions are immutable; cut the next version|**Released versions are immutable.** Never re-tag, move, or re-publish over a
R23|releases|the user cuts releases; prepare the bump, leave the tagging|**The user cuts releases.**;;leave the tagging to the user.
R24|commits|no AI attribution of any kind; human is sole author of record|Never include AI attribution of any kind;;the code-comment half of the rule
R25|policy|the policy is untracked, so its contracts must also be tracked; tracked file wins|untracked (see `.gitignore`), so a clone, a CI run, and a fresh worktree will;;but where the two
EOF
)

# --- non-vacuity: the policy file must exist and be non-empty ---------------
if [[ ! -f "$POLICY" ]]; then
  echo "FAIL: policy file $POLICY does not exist -- the check did not run." >&2
  exit 1
fi
if [[ ! -s "$POLICY" ]]; then
  echo "FAIL: policy file $POLICY is empty -- a zero-match pass would be vacuous." >&2
  exit 1
fi
policy_lines=$(wc -l <"$POLICY")
if ((policy_lines < 40)); then
  echo "FAIL: policy file $POLICY is only $policy_lines lines; expected a full policy." >&2
  exit 1
fi

# --- non-vacuity: the inventory itself must be intact ----------------------
actual_rules=$(printf '%s\n' "$RULES" | grep -c '|')
if ((actual_rules != EXPECTED_RULES)); then
  echo "FAIL: inventory holds $actual_rules rules, expected $EXPECTED_RULES." >&2
  echo "      Editing the inventory to clear a failure is not a pass." >&2
  exit 1
fi

# --- the diff ---------------------------------------------------------------
checked=0
dropped=0
missing_list=()

while IFS='|' read -r id section rule anchors; do
  [[ -z "$id" ]] && continue
  checked=$((checked + 1))
  rule_missing=()
  IFS=';;' read -r -a anchor_list <<<"$anchors"
  for anchor in "${anchor_list[@]}"; do
    [[ -z "$anchor" ]] && continue
    if ! grep -qF -- "$anchor" "$POLICY"; then
      rule_missing+=("$anchor")
    fi
  done
  if ((${#rule_missing[@]} > 0)); then
    dropped=$((dropped + 1))
    missing_list+=("$id ($section): $rule")
    printf 'DROPPED %s [%s] %s\n' "$id" "$section" "$rule"
    for anchor in "${rule_missing[@]}"; do
      printf '        missing anchor: %s\n' "$anchor"
    done
  else
    printf 'ok      %s [%s] %s\n' "$id" "$section" "$rule"
  fi
done <<<"$RULES"

echo
echo "policy file : $POLICY ($policy_lines lines)"
echo "rules old   : $checked"
echo "rules new   : $checked - $dropped = $((checked - dropped))"
echo "dropped     : $((${#missing_list[@]} == 0 ? 0 : ${#missing_list[@]}))"
if ((dropped > 0)); then
  echo
  echo "FAIL: ${#missing_list[@]} rule(s) from the source policy are absent."
  exit 1
fi
echo
echo "PASS: all $checked inventoried rules are present."
