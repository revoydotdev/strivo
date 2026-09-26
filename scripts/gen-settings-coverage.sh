#!/usr/bin/env bash
# Regenerates docs/SETTINGS-COVERAGE.md from the enforced settings allow-list
# (SETTINGS_KEYS in crates/strivo-web/src/routes/settings.rs) and fails when a
# listed setting has no browser control. Run from the repo root; wired into
# ci.yml. Pass --check to assert the committed document is byte-identical to
# the generated one instead of rewriting it.
set -euo pipefail
cd "$(dirname "$0")/.."

DOC="docs/SETTINGS-COVERAGE.md"
SETTINGS_RS="crates/strivo-web/src/routes/settings.rs"
SPA_DIR="crates/strivo-web/assets/spa"
MODE="write"
[ "${1:-}" = "--check" ] && MODE="check"
[ $# -le 1 ] || { echo "usage: $0 [--check]" >&2; exit 2; }

rendered="$(mktemp)" || exit 1
trap 'rm -f "$rendered" "${rendered}.new"' EXIT

# Direct redirection, not `out=$(...)`: the status checked is then python's own,
# and the document is only moved into place after python has succeeded, so an
# interrupted run cannot leave a truncated file a later run calls up to date.
# `set -e` fails the script on a non-zero exit.
python3 - "$SETTINGS_RS" "$SPA_DIR" > "$rendered" <<'PY'
import os, re, sys

settings_rs, spa_dir = sys.argv[1:3]
with open(settings_rs, encoding="utf-8") as f:
    src = f.read()

# --- 1. the allow-list const -------------------------------------------------
m = re.search(r"const SETTINGS_KEYS: &\[SettingsKey\] = &\[", src)
if not m:
    print("error: could not find `const SETTINGS_KEYS: &[SettingsKey] = &[`", file=sys.stderr)
    sys.exit(1)
end = src.find("\n];", m.end())
if end < 0:
    print("error: could not find the end of SETTINGS_KEYS", file=sys.stderr)
    sys.exit(1)
const_body = src[m.end():end]

ENTRY = re.compile(
    r'SettingsKey\s*\{\s*path:\s*"([^"]+)"\s*,\s*restart:\s*(true|false)\s*,?\s*\}'
)
keys = []          # (path, restart, creator_only)
pos = 0
for entry in ENTRY.finditer(const_body):
    # A `#[cfg(feature = "creator")]` between the previous row and this one
    # means the row only exists in a Creator build.
    keys.append((
        entry.group(1),
        entry.group(2) == "true",
        '#[cfg(feature = "creator")]' in const_body[pos:entry.start()],
    ))
    pos = entry.end()

# A zero-key const renders a perfect, empty, wrong document. Refuse it.
if not keys:
    print("error: SETTINGS_KEYS yielded 0 entries; refusing to render", file=sys.stderr)
    sys.exit(1)
paths = [k[0] for k in keys]
if len(set(paths)) != len(paths):
    dupes = sorted({p for p in paths if paths.count(p) > 1})
    print(f"error: SETTINGS_KEYS lists these paths more than once: {dupes}", file=sys.stderr)
    sys.exit(1)

# --- 2. the handler's match arms --------------------------------------------
start = src.find("= match body.path.as_str() {")
if start < 0:
    print("error: could not find update_setting's `match body.path.as_str()`", file=sys.stderr)
    sys.exit(1)
end = src.find("\n    };\n", start)
if end < 0:
    print("error: could not find the end of update_setting's match", file=sys.stderr)
    sys.exit(1)
match_body = src[start:end]
arms = re.findall(r'^\s+"([A-Za-z0-9_.*]+)" =>', match_body, re.MULTILINE)
if not arms:
    print("error: extracted 0 literal match arms; refusing to render", file=sys.stderr)
    sys.exit(1)

# Two directions of drift, both fatal: a listed path with no mutation, and a
# mutation for a path the const no longer admits (the gate would 400 it).
missing_arm = [p for p in paths if "*" not in p and p not in arms]
unlisted_arm = [a for a in arms if a not in paths]
wildcard_rows = [p for p in paths if "*" in p]
wildcard_unwired = [
    p for p in wildcard_rows
    if not all(f'"{piece}"' in match_body for piece in p.split("*"))
]
if missing_arm or unlisted_arm or wildcard_unwired:
    for p in missing_arm:
        print(f"error: SETTINGS_KEYS lists {p!r} but no match arm mutates it", file=sys.stderr)
    for a in unlisted_arm:
        print(f"error: match arm {a!r} is not in SETTINGS_KEYS; the gate refuses it", file=sys.stderr)
    for p in wildcard_unwired:
        print(f"error: wildcard row {p!r} has no guard arm testing its segments", file=sys.stderr)
    sys.exit(1)

# --- 3. which value validator each arm uses ----------------------------------
def validator(arm_text: str) -> str:
    found = re.search(r"\b(take_[a-z0-9_]+)\(", arm_text)
    if not found:
        return "unclassified"
    return {
        "take_bool": "boolean",
        "take_u32": "integer",
        "take_recording_dir": "server directory path",
        "take_nonempty_str": "non-empty string",
        "take_str_in": "string, one of `matroska` / `mp4` / `webm`",
        "take_optional_format_selector": "string, or `null` to clear",
        "take_optional_positive_u32": "integer, or `null` to clear",
        "take_optional_codec": "codec string, or `null` to clear",
        "take_webhook_url": "http(s) URL, or `\"\"` to clear",
    }.get(found.group(1), found.group(1))

def arm_text(arm: str) -> str:
    if "*" in arm:
        at = match_body.find("if path.starts_with(")
        tail = match_body[at:] if at >= 0 else ""
        nxt = re.search(r'^\s+"[A-Za-z0-9_.*]+" =>', tail, re.MULTILINE)
    else:
        at = match_body.find(f'"{arm}" =>')
        if at < 0:
            return ""
        rest = match_body[at + len(f'"{arm}" =>'):]
        nxt = re.search(r'^\s+"[A-Za-z0-9_.*]+" =>', rest, re.MULTILINE)
        tail = rest if nxt is None else rest[: nxt.start()]
    return tail

# --- 4. the browser control, found by scanning the served PVR modules -------
# Mirror build.rs's `assemble_modules`: every .js in the directory, minus the
# creator modules a PVR build does not ship. A narrower filter here would check
# a subset of the served bundle, which is the failure this script exists to stop.
spa_files = sorted(
    os.path.join(spa_dir, n) for n in os.listdir(spa_dir)
    if n.endswith(".js") and not n.endswith("-creator.js")
)
if len(spa_files) < 20:
    print(
        f"error: matched only {len(spa_files)} PVR modules under {spa_dir}; "
        "the module filter is wrong, not the tree",
        file=sys.stderr,
    )
    sys.exit(1)
spa_text = {p: open(p, encoding="utf-8").read() for p in spa_files}

def modules_for(path: str) -> list:
    return [os.path.basename(p) for p in spa_files if path in spa_text[p]]

missing_control = [
    p for p, _, creator in keys
    if not creator and "*" not in p and not modules_for(p)
]

# --- 5. render --------------------------------------------------------------
rows = []
for path, restart, creator in keys:
    mods = modules_for(path)
    if creator:
        control = "Creator Edition pane (absent from a PVR build)"
    elif "*" in path:
        control = "per-plugin toggle; no single control to point at"
    else:
        control = ", ".join(m.removesuffix("-pvr.js") for m in mods) or "**none**"
    rows.append(
        f"| `{path}` | {validator(arm_text(path))}"
        f" | {'yes' if restart else 'no'} | {control} |"
    )

print(f"""# PVR web settings coverage

<!-- GENERATED FILE - DO NOT EDIT. -->
<!-- Source of truth: `SETTINGS_KEYS` in `crates/strivo-web/src/routes/settings.rs`. -->
<!-- Regenerate: `scripts/gen-settings-coverage.sh`   Verify: `scripts/gen-settings-coverage.sh --check` -->
<!-- CI regenerates this document and fails on any difference, so a hand-edit -->
<!-- here is a failing build, not a merge conflict waiting to happen. -->

A setting appears in the table below exactly when `POST
/api/v1/settings/update` will accept it. The list is not maintained by hand: the
handler refuses any path outside the const, so a row that is here is a setting
the API serves, and a setting the API serves is a row here.

| Column | Meaning |
| --- | --- |
| Path | The `path` field of the update request body. |
| Value | The validator the handler applies — JSON `null` clears an optional override. |
| Restart | The response's `restart_required`: the daemon holds a startup config snapshot, so new captures need a restart to see the value. |
| Control | PVR SPA modules that reference the path. |

| Path | Value | Restart | Control |
| --- | --- | --- | --- |
{chr(10).join(rows)}

## What this document does not cover

Two further settings write paths are deliberately out of scope, because they are
not the update allow-list and are not generated from it:
`POST /api/v1/settings/poll-interval` (re-arms the monitor immediately, so it
has its own endpoint) and `POST /api/v1/settings/platform/{{platform}}` (the
credential and cookie fields of the platform wizards). Both are covered by the
route tests in `crates/strivo-web/tests/routes.rs`.

Every configuration field with **no** browser control is TOML-only by design and
is documented in [`config.toml.example`](../config.toml.example), which
`scripts/check-config-example.sh` keeps in step with the `AppConfig` schema.
Recording workers retain their startup configuration, so a restart — not a
browser reload — is what applies a new recording destination or format.

This is the coverage record for the web-writable settings surface, and the
machine-checked half of the claim that the browser UI can reach each one. It
asserts nothing about nested capture-profile and plugin-owned fields beyond the
paths listed, and it does not validate platform or encoder availability.
""", end="")

if missing_control:
    for p in missing_control:
        print(
            f"error: {p!r} is in SETTINGS_KEYS but no PVR SPA module writes it",
            file=sys.stderr,
        )
    print("add a control, or drop the row if the API is the only writer", file=sys.stderr)
    sys.exit(1)
PY

# A misdirected write or a silently empty extraction both land here.
if [ ! -s "$rendered" ] || ! grep -q '^| Path | Value |' "$rendered"; then
    echo "error: the generator produced no coverage table; refusing to use it" >&2
    exit 2
fi

if [ "$MODE" = "check" ]; then
    if ! diff -u "$DOC" "$rendered"; then
        echo "error: $DOC is not what scripts/gen-settings-coverage.sh generates" >&2
        echo "run: scripts/gen-settings-coverage.sh" >&2
        exit 1
    fi
    rows=$(grep -c '^| `' "$rendered" || true)
    echo "OK: $DOC matches SETTINGS_KEYS ($rows listed settings)"
else
    if [ -f "$DOC" ] && cmp -s "$DOC" "$rendered"; then
        echo "OK: $DOC already up to date"
    else
        cp "$rendered" "${rendered}.new" && mv "${rendered}.new" "$DOC"
        echo "wrote $DOC"
    fi
fi
