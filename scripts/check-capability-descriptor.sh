#!/usr/bin/env bash
# Cross-check the gate call sites against the capability descriptor
# (plan todo 20). Proves the plan's subset invariant *by shape*:
#
#   every `gate_pro(...)` argument names a declared `CapabilityKey`
#   variant, and the descriptor is built from that same enum, so every
#   gated capability has a descriptor row. The invariant is SUBSET, not
#   equality: the descriptor is strictly larger — it also carries
#   capabilities with no gate at all (`yt-publish`, roadmap only).
#
# WHY AN AST SEARCH AND NOT grep: the thing being checked is the *shape*
# of a call's argument — is it a string literal or an enum path? A text
# search cannot tell those apart, cannot tell a `gate_pro(...)` call from
# the `fn gate_pro` definition, and counts a doc-comment mention as a
# call site. ast-grep parses, so the classification is by shape.
#
# WHY THE ENUM, AND WHY THAT MATTERS: this todo's own refactor replaces
# the 90 string literals at the gate sites with `CapabilityKey::X`
# references. A scan pinned to literals would then find nothing, report
# zero, and look like a clean pass — a vacuous pass. So the scan is
# anchored on `enum CapabilityKey` instead: whatever the call sites are
# written as, the enum is still there, the sites are still counted, and
# a site whose argument the parser cannot recognise is reported as a
# violation rather than skipped.
#
# Usage: scripts/check-capability-descriptor.sh [PATH ...]
#        defaults to crates/strivo-web/src
#
# Exit 0 = clean, 1 = a gate site bypasses the descriptor (or the search
# found nothing at all), 2 = setup problem.
set -euo pipefail

cd "$(dirname "$0")/.."

RULES=scripts/ast-grep
LITERAL_RULE="$RULES/gate-pro-string-literal.yml"
VARIANT_RULE="$RULES/capability-key-variant.yml"
DEFAULT_PATHS=(crates/strivo-web/src)

paths=("$@")
if [ ${#paths[@]} -eq 0 ]; then
  paths=("${DEFAULT_PATHS[@]}")
fi

if ! command -v ast-grep >/dev/null 2>&1; then
  echo "check-capability-descriptor: ast-grep not on PATH" >&2
  echo "  install: npm i -g @ast-grep/cli   (or: cargo install ast-grep --locked)" >&2
  exit 2
fi

for rule in "$LITERAL_RULE" "$VARIANT_RULE"; do
  [ -f "$rule" ] || { echo "check-capability-descriptor: missing $rule" >&2; exit 2; }
done

for p in "${paths[@]}"; do
  [ -e "$p" ] || { echo "check-capability-descriptor: no such path: $p" >&2; exit 2; }
done

# ast-grep `run` exits 1 both when a pattern matched nothing and when the
# tool itself failed, so the exit code cannot discriminate. Parseability
# can: a clean `[]` is a real zero; empty or unparseable output means the
# plumbing broke, and reporting "zero hits" for that would be a check
# that passes by not running.
json_ok() {
  python3 -c '
import json, sys
try:
    findings = json.load(sys.stdin)
except ValueError:
    sys.exit("ast-grep produced no parseable JSON; refusing to report zero hits")
if not isinstance(findings, list):
    sys.exit("ast-grep output was not a list; refusing to report zero hits")
'
}

scan() {
  ast-grep scan -r "$1" --json=compact "${paths[@]}" 2>/dev/null || true
}

run() {
  local err out
  err=$(mktemp)
  out=$(ast-grep run --lang rust --json=compact --pattern "$1" "${paths[@]}" 2>"$err") || true
  if ! printf '%s' "$out" | json_ok; then
    sed 's/^/  /' "$err" >&2
    rm -f "$err"
    exit 2
  fi
  rm -f "$err"
  printf '%s' "$out"
}

# 1. The descriptor's key enum, and every variant it declares. This single
#    pass is also the existence check: the classifier errors out on an
#    empty variant set rather than reporting zero, so a missing
#    `enum CapabilityKey` fails the check instead of emptying it.
variants=$(scan "$VARIANT_RULE" | python3 -c '
import json, re, sys
try:
    findings = json.load(sys.stdin)
except ValueError:
    sys.exit("variant scan produced no parseable JSON; refusing to report zero hits")
names = {}
for f in findings:
    m = re.match(r"\s*([A-Za-z_][A-Za-z0-9_]*)", f.get("text", ""))
    if not m:
        sys.exit("unparseable enum_variant match; refusing to report an empty variant set")
    names[m.group(1)] = "%s:%d" % (f["file"], f["range"]["start"]["line"] + 1)
if not names:
    sys.exit("the CapabilityKey enum declares no variants; refusing to run a vacuous check")
print("\n".join("%s\t%s" % (n, names[n]) for n in sorted(names)))
') || exit 2
n_variants=$(printf '%s\n' "$variants" | grep -c .)
enum_at=$(printf '%s\n' "$variants" | head -1 | cut -f2)
enum_at="$enum_at (first variant)"

# 2. Every gate call site, and what its argument is written as.
sites=$(run 'gate_pro($ARG)' | python3 -c '
import json, re, sys
try:
    findings = json.load(sys.stdin)
except ValueError:
    sys.exit("gate_pro scan produced no parseable JSON; refusing to report zero hits")
for f in sorted(findings, key=lambda f: (f["file"], f["range"]["start"]["line"])):
    text = f.get("text", "")
    where = "%s:%d" % (f["file"], f["range"]["start"]["line"] + 1)
    lit = re.fullmatch(r"gate_pro\(\s*\"([^\"]*)\"\s*\)", text, re.S)
    if lit:
        kind, name = "literal", lit.group(1)
    else:
        m = re.fullmatch(r"gate_pro\(\s*(?:crate::capabilities::)?CapabilityKey::([A-Za-z_][A-Za-z0-9_]*)\s*\)", text, re.S)
        if m:
            kind, name = "key", m.group(1)
        else:
            kind, name = "unrecognised", text
    print("%s\t%s\t%s" % (kind, name, where))
') || exit 2
n_sites=$(printf '%s\n' "$sites" | grep -c . || true)

echo "capability-descriptor shape search — ${paths[*]}"
echo "  CapabilityKey enum                              : $enum_at"
echo "  declared variants                               : $n_variants"
echo "  gate_pro() call sites                            : $n_sites"

# The empty case is the failure this check exists to avoid: a scan whose
# subject vanished reports zero and looks exactly like a clean pass.
if [ "$n_sites" -eq 0 ]; then
  echo
  echo "FAIL: no gate_pro() call site found."
  echo "  Every gate must name a CapabilityKey, so a vanished call site"
  echo "  means the search is measuring nothing — not that it passed."
  exit 1
fi

n_literal=$(printf '%s\n' "$sites" | grep -c $'^literal\t' || true)
n_key=$(printf '%s\n' "$sites" | grep -c $'^key\t' || true)
n_unrec=$(printf '%s\n' "$sites" | grep -c $'^unrecognised\t' || true)
echo "  ...argument is a CapabilityKey path               : $n_key"
echo "  ...argument is a bare string literal (VIOLATION) : $n_literal"
echo "  ...argument shape unrecognised       (VIOLATION) : $n_unrec"

# 3. Subset: every key used at a gate is a declared variant. The
#    descriptor table is built from those same variants, so membership
#    here is membership in the descriptor.
used=$(printf '%s\n' "$sites" | awk -F'\t' '$1=="key"{print $2}' | sort -u)
# A key is undeclared when NO line names it — the exit status of the awk
# is the test, not its output, so an empty match set cannot read as a pass.
undeclared=$(
  for k in $used; do
    if ! printf '%s\n' "$variants" | awk -F'\t' -v k="$k" '$1==k{found=1} END{exit !found}'; then
      printf '%s\n' "$k"
    fi
  done
)
n_used=$(printf '%s' "$used" | grep -c . || true)

echo "  distinct keys gated                             : $n_used"
echo
echo "  gated keys and where they are declared:"
while IFS=$'\t' read -r name at; do
  [ -n "$name" ] || continue
  count=$(printf '%s\n' "$sites" | awk -F'\t' -v k="$name" '$1=="key" && $2==k' | grep -c . || true)
  printf '    %-22s %2s site(s)  %s\n' "$name" "$count" "$at"
done <<EOF
$(for k in $used; do printf '%s\n' "$variants" | awk -F'\t' -v k="$k" '$1==k'; done)
EOF

if [ "$n_literal" -gt 0 ] || [ "$n_unrec" -gt 0 ] || [ -n "$undeclared" ]; then
  echo
  if [ "$n_literal" -gt 0 ]; then
    echo "VIOLATION: gate_pro() called with a bare string literal. It bypasses"
    echo "the descriptor's key type, so the name can be one no row describes:"
    printf '%s\n' "$sites" | awk -F'\t' '$1=="literal"{print "  " $3 "\tgate_pro(\"" $2 "\")"}'
    echo
  fi
  if [ "$n_unrec" -gt 0 ]; then
    echo "VIOLATION: gate_pro() argument shape this check does not recognise."
    echo "A gate must be exactly one CapabilityKey variant; anything else is"
    echo "either a bypass or a call form this check can no longer see, and both"
    echo "mean the count above no longer means what it says:"
    printf '%s\n' "$sites" | awk -F'\t' '$1=="unrecognised"{print "  " $3 "\t" $2}'
    echo
  fi
  if [ -n "$undeclared" ]; then
    echo "VIOLATION: gate names a CapabilityKey variant the enum does not declare:"
    printf '  %s\n' $undeclared
    echo
  fi
  echo "Write the gate as gate_pro(CapabilityKey::X) and add X's row to"
  echo "CAPABILITIES in crates/strivo-web/src/capabilities.rs."
  exit 1
fi

# 4. The scan must actually reach the gates the descriptor's Pro rows
#    depend on. A Pro row with no gate is legitimate (the entitlement set
#    is derived from the descriptor, not the other way round), so this is
#    reported, not asserted.
echo
echo "OK: all $n_sites gate_pro() call sites name a declared CapabilityKey."
echo "    ($n_key keyed, $n_literal literal, $n_unrec unrecognised, $n_used distinct)"
