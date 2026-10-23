#!/usr/bin/env bash
# Fails the release workflow before any platform build starts if the pushed
# tag doesn't match the manifest version. Run from the repo root; reads
# GITHUB_REF_TYPE / GITHUB_REF_NAME the way release.yml's own steps already
# do. A workflow_dispatch rehearsal run has no tag, so it's a deliberate
# no-op there -- rehearsal artifacts are always suffixed "-dev" downstream
# and are never published.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${GITHUB_REF_TYPE:-}" != "tag" ]]; then
  echo "not a tag push (GITHUB_REF_TYPE=${GITHUB_REF_TYPE:-unset}) -- skipping tag/manifest version check"
  exit 0
fi

tag_version="${GITHUB_REF_NAME#v}"
manifest_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

if [[ -z "$manifest_version" ]]; then
  echo "error: could not read [workspace.package] version from Cargo.toml" >&2
  exit 1
fi

if [[ "$tag_version" != "$manifest_version" ]]; then
  echo "error: tag v${tag_version} does not match manifest version ${manifest_version}" >&2
  echo "       (Cargo.toml's [workspace.package] version is the single source of truth --" >&2
  echo "       bump it and land that change before tagging)" >&2
  exit 1
fi

echo "OK: tag v${tag_version} matches manifest version ${manifest_version}"

# The `dev-unlock` feature lets a release binary unlock every Pro capability
# from an environment variable. It is for a lead developer's own machine and
# must never reach a published build.
if grep -n -- 'dev-unlock' .github/workflows/release.yml .github/workflows/docker.yml Dockerfile packaging/aur/PKGBUILD scripts/install.sh 2>/dev/null; then
  echo "error: a published build enables the dev-unlock feature (lines above)" >&2
  exit 1
fi
echo "OK: no published build enables dev-unlock"
