#!/usr/bin/env bash
# Working-tree release-integrity gate. `check-release-version.sh` is the
# tag-time gate (does the pushed tag match Cargo.toml?); this is the always-on
# gate for the claims that only exist in the repository itself. Wired into
# ci.yml's packaging-docs job, which is the no-Rust job for scripts/.
#
#   1. Version agreement -- Cargo.toml's [workspace.package] version, the
#      newest released `## [x.y.z]` heading in CHANGELOG.md, and README.md's
#      "latest release:" claim must be the same string. The 0.8.0 release
#      shipped with no CHANGELOG section at all and the README still named
#      0.6.0, and nothing caught either.
#   2. No source path to the assembled SPA CSS -- `assets/spa.css` is a build
#      output and a served URL, never an in-tree file you can edit. The
#      canonical source is `crates/strivo-web/assets/spa-css/`, assembled by
#      `crates/strivo-web/build.rs`.
#   3. No working claim for roadmap-only surface (report F09) -- `yt-publish`
#      (still `EntryPoint::Roadmap`, `installed: false`), the identity
#      translator, and the incomplete snapshot must not be described as
#      working in a doc or an SPA string.
#
# Reads: Cargo.toml, CHANGELOG.md, README.md, every tracked `*.md` outside the
# immutable record directories, and the SPA asset modules. Later lanes editing
# those files are what this gate watches.
set -euo pipefail
cd "$(dirname "$0")/.."

fail=0
err() { echo "$@" >&2; fail=1; }
ok() { echo "OK: $*"; }

self='scripts/check-version-agreement.sh'
# docs/audits/ and research/ are historical records: they describe what was
# true when written and rewriting them would falsify the audit trail. They are
# excluded from both doc scans, not from existence.
records=(':!docs/audits/**' ':!research/**')

# ---------------------------------------------------------------- 1. version

manifest_version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1 || true)"
if [[ -z "$manifest_version" ]]; then
  err "error: could not read [workspace.package] version from Cargo.toml"
else
  ok "Cargo.toml version ${manifest_version}"
fi

# Newest *released* heading: the first `## [x.y.z]` that is not [Unreleased].
changelog_version="$(awk '
  /^## \[/ {
    v = $0
    sub(/^## \[/, "", v)
    sub(/\].*$/, "", v)
    if (v != "Unreleased") { print v; exit }
  }
' CHANGELOG.md)"
if [[ -z "$changelog_version" ]]; then
  err "error: CHANGELOG.md has no released \`## [x.y.z]\` section at all"
else
  ok "CHANGELOG newest released section ${changelog_version}"
fi

# Only the "latest release" claim is a version assertion. README.md:66's
# "The 0.6.0 release introduced Windows support" is history, not a claim about
# what is current, and must keep naming 0.6.0.
readme_version="$(sed -n 's/.*latest release: \([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' README.md | head -1 || true)"
if [[ -z "$readme_version" ]]; then
  err "error: README.md has no \"latest release: <version>\" claim to compare"
else
  ok "README latest release ${readme_version}"
fi

# Cargo.toml is the pivot, but a changelog and a README that disagree with each
# other are just as broken even if one of them happens to match the manifest.
if [[ -n "$manifest_version" && -n "$changelog_version" && "$manifest_version" != "$changelog_version" ]]; then
  err "error: Cargo.toml says ${manifest_version} but CHANGELOG.md's newest released section is ${changelog_version}"
fi
if [[ -n "$manifest_version" && -n "$readme_version" && "$manifest_version" != "$readme_version" ]]; then
  err "error: Cargo.toml says ${manifest_version} but README.md says ${readme_version}"
fi
if [[ -n "$changelog_version" && -n "$readme_version" && "$changelog_version" != "$readme_version" ]]; then
  err "error: CHANGELOG.md says ${changelog_version} but README.md says ${readme_version}"
fi

# A malformed version is a failure, not a silent pass: a typo in Cargo.toml
# that still round-trips would otherwise satisfy every comparison above.
for v in "$manifest_version" "$changelog_version" "$readme_version"; do
  if [[ -n "$v" && ! "$v" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+].*)?$ ]]; then
    err "error: '${v}' is not a MAJOR.MINOR.PATCH version"
  fi
done

if [[ $fail -eq 0 ]]; then
  ok "Cargo.toml, CHANGELOG.md and README.md agree on the released version"
fi

# --------------------------------------------------------- 2. spa.css source

# 2a. Nobody may name the assembled bundle as an in-tree source file.
spa_path_hits="$(git grep -n 'crates/strivo-web/assets/spa\.css' -- \
  "${records[@]}" ":!${self}" || true)"
if [[ -n "$spa_path_hits" ]]; then
  err "error: tracked file(s) name crates/strivo-web/assets/spa.css as a source file."
  err "       It does not exist -- it is assembled at build time from"
  err "       crates/strivo-web/assets/spa-css/ by crates/strivo-web/build.rs."
  err "$spa_path_hits" >&2
else
  ok "no tracked file names crates/strivo-web/assets/spa.css as a source file"
fi

# 2b. Nobody may call the assembled bundle the canonical/theme source of truth,
#    however they spell the path. Served URLs (`/assets/spa.css`) and build
#    outputs are fine; this is only about *authority over the source*.
spa_claim_hits="$(git grep -n 'spa\.css' -- '*.md' "${records[@]}" \
  | grep -iE 'canonical|source of truth|source file|source-file|in-tree' || true)"
if [[ -n "$spa_claim_hits" ]]; then
  err "error: a document claims spa.css is the canonical CSS source of truth."
  err "       Canonical source: crates/strivo-web/assets/spa-css/*.css,"
  err "       assembled by crates/strivo-web/build.rs."
  err "$spa_claim_hits" >&2
else
  ok "no document claims spa.css is the canonical CSS source of truth"
fi

# ------------------------------------------------------------- 3. F09 claims

mapfile -t docs_to_scan < <(git ls-files '*.md' "${records[@]}")
mapfile -t spa_to_scan < <(git ls-files 'crates/strivo-web/assets/spa/*' 'crates/strivo-web/assets/spa-css/*')
scan=("${docs_to_scan[@]}" "${spa_to_scan[@]}")
if [[ ${#scan[@]} -eq 0 ]]; then
  err "error: no documents or SPA modules found to scan -- refusing to pass vacuously"
fi

f09_hits="$(awk '
  BEGIN { IGNORECASE = 1 }
  {
    subject = ($0 ~ /yt-publish|yt_publish|identity translator|placeholder translator|placeholder translation|snapshot plugin|snapshots plugin/)
    claim   = ($0 ~ /(^|[^A-Za-z])(works|worked|working|functional|operational|production-ready|end-to-end|shipped)([^A-Za-z]|$)/) \
              || ($0 ~ /(^|[^A-Za-z])ready([^A-Za-z]|$)/)
    # Only a hedge on the *same line* excuses a claim. A line that names a
    # roadmap surface and asserts it works, with nothing hedging it, is the
    # defect F09 describes.
    hedge   = ($0 ~ /(^|[^A-Za-z])(not|no|never|non|planned|roadmap|deferred|unimplemented|todo|until|requires|still|lacks|without|remain|placeholder|stub)([^A-Za-z]|$)/)
    if (subject && claim && !hedge) print FILENAME ":" FNR ": " $0
  }
' "${scan[@]}" || true)"
if [[ -n "$f09_hits" ]]; then
  err "error: a doc or SPA string describes roadmap-only surface as working."
  err "       yt-publish is EntryPoint::Roadmap / installed:false; the caption"
  err "       translator is an identity translator; the snapshot surface is"
  err "       incomplete. None of them may be presented as functional."
  err "$f09_hits" >&2
else
  ok "no doc or SPA string claims a roadmap-only surface is working"
fi

if [[ $fail -ne 0 ]]; then
  echo "check-version-agreement: FAILED" >&2
  exit 1
fi
echo "check-version-agreement: all checks passed"
