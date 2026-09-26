#!/usr/bin/env bash
# Staleness gate for the recorded bundle figures in docs/PERFORMANCE.md (todo 10).
#
# docs/PERFORMANCE.md records what the shipped bundle measured like. A recorded
# number that no longer matches a fresh measurement is worse than no number, so
# this re-measures through the real release server (scripts/measure_bundle.sh)
# and fails when any recorded figure has drifted by more than TOLERANCE_PCT.
#
# Usage:
#   scripts/check-bundle-size.sh <release-strivo-binary> <out-assets-dir> <label>
#   scripts/check-bundle-size.sh target/pvr-release/strivo target/pvr-release/assets PVR
#
# Knobs:
#   BUNDLE_PERF_DOC=<path>       document to check (default docs/PERFORMANCE.md)
#   BUNDLE_TOLERANCE_PCT=<n>     allowed drift, percent (default 5)
#   BENCH_PORT                   port for the throwaway server (default 8400)
#
# Exit codes: 0 in tolerance | 1 stale, missing, or unreadable.
set -euo pipefail

BIN="${1:?usage: check-bundle-size.sh <release-strivo-binary> <out-assets-dir> <label>}"
ASSETS_DIR="${2:?usage: check-bundle-size.sh <release-strivo-binary> <out-assets-dir> <label>}"
LABEL="${3:?usage: check-bundle-size.sh <release-strivo-binary> <out-assets-dir> <label>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DOC="${BUNDLE_PERF_DOC:-$SCRIPT_DIR/../docs/PERFORMANCE.md}"
TOL="${BUNDLE_TOLERANCE_PCT:-5}"

[ -f "$DOC" ] || { echo "check-bundle-size.sh: no such document: $DOC" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 ||
  { echo "check-bundle-size.sh: required tool not found: python3" >&2; exit 1; }

MEASURED="$(mktemp)"
trap 'rm -f "$MEASURED"' EXIT INT TERM
BUNDLE_OUT="$MEASURED" "$SCRIPT_DIR/measure_bundle.sh" "$BIN" "$ASSETS_DIR" "$LABEL" >/dev/null

python3 - "$DOC" "$MEASURED" "$LABEL" "$TOL" <<'PY'
import json, re, sys

doc_path, measured_path, label, tol = sys.argv[1:5]
tol = float(tol)
text = open(doc_path, encoding="utf-8").read()

# The record is the fenced json block between explicit markers, so the document
# stays readable as prose and the check reads machine data, not a markdown table.
m = re.search(
    r"<!-- bundle-measurement:begin -->\s*```json\s*(.*?)\s*```\s*<!-- bundle-measurement:end -->",
    text, re.S)
if not m:
    sys.exit(f"check-bundle-size.sh: {doc_path} has no bundle-measurement block; "
             f"nothing can be verified against it.")
try:
    recorded = json.loads(m.group(1))
except json.JSONDecodeError as e:
    sys.exit(f"check-bundle-size.sh: {doc_path} bundle-measurement block is not valid JSON: {e}")

fresh = json.load(open(measured_path, encoding="utf-8"))
want = recorded.get("editions", {}).get(label)
if not want:
    sys.exit(f"check-bundle-size.sh: {doc_path} records no edition named {label!r}; "
             f"the record cannot go stale-checked.")
if not want.get("wire_measured"):
    print(f"check-bundle-size.sh: {label} is recorded as not wire-measured; skipped")
    sys.exit(0)

rows, bad = [], []
# Only the per-asset sub-objects are figures; `note` and `wire_measured` are
# metadata and must not be read as measurements.
assets = {a: v for a, v in want.items() if isinstance(v, dict)}
if not assets:
    sys.exit(f"check-bundle-size.sh: {doc_path} records no asset figures for {label!r}; "
             f"the check would have nothing to verify.")
for asset, rec in sorted(assets.items()):
    got = fresh["assets"].get(asset)
    if got is None:
        bad.append(f"  {label}/{asset}: measured nothing")
        continue
    for field, value in [("raw_bytes", got["raw_bytes"]),
                         ("gzip", got["gzip"]["median"]),
                         ("brotli", got["br"]["median"])]:
        was = rec.get(field)
        if was is None:
            bad.append(f"  {label}/{asset} {field}: not recorded")
            continue
        drift = abs(value - was) / was * 100
        rows.append((label, asset, field, was, value, drift))
        if drift > tol:
            bad.append(f"  {label}/{asset} {field}: recorded {was}, measured {value} "
                       f"({drift:.1f}% drift, tolerance {tol:g}%)")

print(f"check-bundle-size.sh: {doc_path} vs a fresh {fresh['strivo_version']} measurement")
for l, a, f, was, got, d in rows:
    print(f"  {l:<6} {a:<8} {f:<9} recorded {was:>8}  measured {got:>8}  {d:5.2f}% drift")
if bad:
    print(f"check-bundle-size.sh: STALE — {doc_path} no longer matches the shipped bundle:")
    print("\n".join(bad))
    sys.exit(1)
print(f"check-bundle-size.sh: OK — every recorded figure is within {tol:g}% of a fresh measurement")
PY
