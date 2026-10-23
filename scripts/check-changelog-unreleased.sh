#!/usr/bin/env bash
# Fails when feat:/fix: commits have landed since the last tag but
# CHANGELOG.md's ## [Unreleased] section is empty -- catching the exact
# drift this repo already had once (197 commits, only two recorded). Run
# from the repo root; wired into ci.yml.
set -euo pipefail
cd "$(dirname "$0")/.."

# Release tags only: non-release tags such as the vendored-deps mirror
# (vendored-deps-YYYY-MM-DD) would otherwise count as "the last release".
last_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)"
if [[ -z "$last_tag" ]]; then
  echo "skip: no tags found -- nothing to compare Unreleased against"
  exit 0
fi

commit_count="$(git log "${last_tag}..HEAD" --grep='^feat' --grep='^fix' -E --oneline | wc -l)"
if [[ "$commit_count" -eq 0 ]]; then
  echo "OK: no feat/fix commits since ${last_tag} -- nothing required in Unreleased"
  exit 0
fi

# Extract the ## [Unreleased] section body (everything up to the next ## heading).
section="$(awk '/^## \[Unreleased\]/{flag=1; next} /^## /{flag=0} flag' CHANGELOG.md)"
# Non-empty means at least one real content line -- a bullet, sub-heading, or
# prose -- not just blank lines.
if [[ -z "$(printf '%s' "$section" | tr -d '[:space:]')" ]]; then
  # A release commit empties Unreleased into a section for the manifest
  # version before that version's tag exists; the entries are recorded there.
  manifest_version="$(awk '/^\[workspace\.package\]/{f=1; next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/, "", $0); sub(/^version=/, "", $0); print; exit}' Cargo.toml)"
  newest_section="$(awk -F'[][]' '/^## \[[0-9]/{print $2; exit}' CHANGELOG.md)"
  if [[ -n "$manifest_version" && "$newest_section" == "$manifest_version" ]] \
    && ! git rev-parse -q --verify "refs/tags/v${manifest_version}" >/dev/null; then
    echo "OK: ${commit_count} feat/fix commit(s) since ${last_tag}, recorded under the untagged [${manifest_version}] release section"
    exit 0
  fi
  echo "error: ${commit_count} feat/fix commit(s) landed since ${last_tag} but CHANGELOG.md's" >&2
  echo "       ## [Unreleased] section is empty -- add an entry." >&2
  exit 1
fi

echo "OK: ${commit_count} feat/fix commit(s) since ${last_tag}, Unreleased is non-empty"
