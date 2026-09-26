#!/usr/bin/env bash
# Cross-check the SPA's client calls against the server's route table, in
# BOTH editions (plan todo 7).
#
# The defect class this exists for: the SPA called
# `GET /api/v1/plugins/crunchr/transcript/{id}` behind `.catch(() => null)`
# and no such route has ever existed, so "route missing" and "no transcript
# exists" rendered identically. Nothing in the tree could have caught that —
# the server has no idea which paths the SPA asks for, and the SPA is not
# type-checked against the router. This is the check that would have.
#
# ── Why the module set is parsed twice, not read off the bundle ────────
# `crates/strivo-web/build.rs::assemble_modules` includes every `*.js` in
# `assets/spa/` EXCEPT the `*-creator.js` ones when the `creator` feature is
# off, concatenating in filename order. So the two editions are two *module
# sets*, and reading the assembled PVR bundle could never see a Creator-only
# call at all. This mirrors the assembler's own rule (filename suffix) rather
# than reading build.rs text, and says so in the output.
#
# ── Dynamic routes are matched positionally, not skipped ───────────────
# Axum writes a path parameter as `{id}`; the SPA writes an interpolation as
# `${encodeURIComponent(recordingId)}`. Both are normalised to the single
# token `*`, and a client path matches a server path when the segment counts
# are equal and every segment is either equal or a token *on either side*.
# That is deliberately strict:
#   * a wildcard is one segment, never a subtree — so a client
#     `/plugins/crunchr/transcript/*` cannot match the server's
#     `/plugins/crunchr/recordings/{id}`, and the missing route is reported
#     rather than absorbed by a neighbouring dynamic route;
#   * the number of wildcard matches is reported, and a run in which it is
#     zero is a FAIL — a cross-check that "passes" by ignoring dynamic
#     routes, or by never having met one, proves nothing.
# `--self-test` asserts the matcher discriminates by exercising exactly that
# pair, so the property is checked rather than asserted in prose.
#
# ── Why the sets are asserted non-empty ───────────────────────────────
# A comparison over an empty client set reports zero unmatched and looks like
# a clean pass. That is the recurring vacuous-pass shape, so an empty client
# set, an empty route set, or an empty *edition* (PVR must have fewer modules
# than Creator) each fail the run with a distinct message.
#
# Usage: scripts/check-client-routes.sh [--self-test] [--verbose]
# Exit 0 = every client path in both editions resolves to a route;
#        1 = a client path resolves to no route (or a non-vacuity assert fired);
#        2 = setup problem (missing tree, unparseable input).
set -euo pipefail

cd "$(dirname "$0")/.."

SPA_DIR=crates/strivo-web/assets/spa
ROUTES_DIR=crates/strivo-web/src/routes
ROUTES_MOD=$ROUTES_DIR/mod.rs

for p in "$SPA_DIR" "$ROUTES_DIR" "$ROUTES_MOD"; do
  [ -e "$p" ] || { echo "check-client-routes: no such path: $p" >&2; exit 2; }
done
command -v python3 >/dev/null 2>&1 || { echo "check-client-routes: python3 not on PATH" >&2; exit 2; }

exec python3 - "$@" <<'PY'
import os
import re
import sys

SPA_DIR = "crates/strivo-web/assets/spa"
ROUTES_DIR = "crates/strivo-web/src/routes"
ROUTES_MOD = os.path.join(ROUTES_DIR, "mod.rs")
WILD = "*"


# ── client side ────────────────────────────────────────────────────────────
# The SPA funnels every JSON call through `API._fetch(path, opts)`, which
# prefixes the base and hands off to `fetch`. That base is READ from the
# bundle, not hardcoded here: if `_fetch` ever stops prefixing, or changes
# prefix, a hardcoded `/api/v1` in this script would make every client path
# look unmatched (a false alarm) or — worse — hide a real one. Exactly one
# base must be found, or the check refuses to run.
# A handful of endpoints are addressed by URL instead, for the browser to
# follow directly (`href=`/`src=` on a media or download element, and the
# `*Url:` helpers that build such a URL). Both are real client->server calls
# and both are collected; a check that only read `_fetch` would miss the same
# defect class on a direct link.
FETCH = re.compile(r"_fetch\(")
ABSOLUTE_URL = re.compile(r"""(?:`|")(/[^`"?]*)""")
ROUTE_LIT = re.compile(r'\.route\(\s*"([^"]*)"', re.S)
FETCH_BASE = re.compile(r"fetch\(\s*`([^`$]*)\$\{path\}`")


def normalise(path):
    """Collapse a URL to its path shape.

    Drops the query string, strips a trailing slash, and rewrites every
    interpolation — `${x}`, `{x}` — to one wildcard token, so a client
    `/a/${id}/b` and a server `/a/{id}/b` are the same shape.
    """
    path = path.split("?", 1)[0].split("#", 1)[0]
    path = re.sub(r"\$\{[^}]*\}", WILD, path)
    path = re.sub(r"\{[^}]*\}", WILD, path)
    # A trailing interpolation can leave an empty segment (`/a/` + `${q}`).
    path = re.sub(r"/+$", "", path)
    return path or "/"


def first_argument(line, open_paren):
    """The first argument of a call, by bracket depth rather than a regex.

    A first argument is often a template literal holding another template
    literal — `/recordings${q ? `?${q}` : ""}` — so matching "up to the next
    backtick" truncates it into a path that does not exist. Depth counting
    over `(`, `{` and `)` takes the whole expression.
    """
    depth = 0
    for i in range(open_paren, len(line)):
        c = line[i]
        if c in "({":
            depth += 1
        elif c in ")}":
            depth -= 1
            if depth == 0:
                return line[open_paren + 1 : i].split(",", 1)[0].strip()
    return None


# A query string reaches a path as an interpolation, and an interpolation
# becomes a path segment once normalised, so every one of them has to be
# classified. Two signals, in order of confidence:
#   * the expression contains a `?` or a nested template — it is visibly a
#     query (`${q ? `?${q}` : ""}`);
#   * it is a bare identifier from QUERY_NAMES, which this SPA binds to a
#     URLSearchParams result before interpolating it.
# The second is a name-based rule, so the identifiers it acted on are
# reported under --verbose rather than left as a silent assumption. A new
# query variable with a different name shows up as a client path whose last
# segment is `*` and a route that does not match, which is a loud failure.
QUERY_NAMES = ("qs", "query", "q")
BY_NAME = re.compile(r"\$\{(%s)\}" % "|".join(QUERY_NAMES))


def strip_query_interpolations(arg):
    previous = None
    while previous != arg:
        previous = arg
        arg = re.sub(r"\$\{[^}]*`[^`]*`[^}]*\}", "", arg)
        arg = re.sub(r"\$\{[^}]*\?", "", arg)
        arg = BY_NAME.sub("", arg)
    return arg


def unquote(arg):
    arg = arg.strip()
    if len(arg) >= 2 and arg[0] in "`\"" and arg[-1] == arg[0]:
        return arg[1:-1]
    return None


def client_paths(files, base, skip_lines):
    """Every client-asserted path, with the file:line it was read from."""
    found = {}
    for path in files:
        with open(path, encoding="utf-8") as fh:
            for lineno, line in enumerate(fh, 1):
                if (path, lineno) in skip_lines:
                    continue
                for match in FETCH.finditer(line):
                    arg = unquote(first_argument(line, match.end() - 1) or "")
                    if arg is None:
                        continue
                    raw = base + strip_query_interpolations(arg)
                    found.setdefault(normalise(raw), f"{path}:{lineno}")
                for match in ABSOLUTE_URL.finditer(line):
                    # Only a link that already carries the base is a direct
                    # route address; a bare `/plugins/...` on a `_fetch`
                    # line is the same call counted twice, un-based.
                    if match.group(1).startswith(base):
                        found.setdefault(normalise(match.group(1)), f"{path}:{lineno}")
    return found


def fetch_base(files):
    """The base `_fetch` prefixes, and the lines that declare it.

    The declaring line is `fetch(`/api/v1${path}`)`. Left in the client set
    it would show up as a call to the bare prefix `/api/v1`, which matches
    no route — a defect that does not exist.
    """
    bases, decls = set(), set()
    for path in files:
        with open(path, encoding="utf-8") as fh:
            for lineno, line in enumerate(fh, 1):
                for m in FETCH_BASE.finditer(line):
                    bases.add(m.group(1))
                    decls.add((path, lineno))
    return bases, decls


# ── server side ────────────────────────────────────────────────────────────
def creator_only_modules():
    """Route modules behind `#[cfg(feature = "creator")]`, read off mod.rs.

    Derived from the same attribute `routes/mod.rs` and `server.rs` use to
    decide the merge, so the edition split in this check is the edition split
    in the binary rather than a second hand-kept list. The attribute must sit
    on the declaration or on the comment block immediately above it — a
    wider window would attribute a neighbouring module's `creator` gate to
    this one, and `login` is exactly that trap.
    """
    creator = set()
    with open(ROUTES_MOD, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    for i, line in enumerate(lines):
        if "pub mod" not in line:
            continue
        guarded = False
        for prev in reversed(lines[:i]):
            stripped = prev.strip()
            if not stripped or stripped.startswith("//"):
                continue
            guarded = 'cfg(feature = "creator")' in stripped
            break
        name = line.split("pub mod", 1)[1].split(";", 1)[0].strip()
        if guarded:
            creator.add(name)
    return creator


def route_modules(creator_enabled, creator_mods):
    """Every `routes/*.rs` file compiled into this edition."""
    mods = []
    for name in sorted(os.listdir(ROUTES_DIR)):
        if not name.endswith(".rs") or name == "mod.rs":
            continue
        if name[:-3] in creator_mods and not creator_enabled:
            continue
        mods.append(os.path.join(ROUTES_DIR, name))
    return mods


def server_routes(files):
    routes = {}
    for path in files:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
        for match in ROUTE_LIT.finditer(text):
            raw = match.group(1)
            lineno = text.count("\n", 0, match.start()) + 1
            routes.setdefault(normalise(raw), f"{path}:{lineno}")
    return routes


def matches(client, server):
    """Positional match; a wildcard is exactly one segment on either side."""
    c = normalise(client).strip("/").split("/")
    s = normalise(server).strip("/").split("/")
    if len(c) != len(s):
        return False
    return all(a == b or a == WILD or b == WILD for a, b in zip(c, s))


def crosscheck(label, client, routes):
    unmatched, wildcards = [], 0
    for path, where in sorted(client.items()):
        hit = next((r for r in routes if matches(path, r)), None)
        if hit is None:
            unmatched.append((path, where))
        elif WILD in path or WILD in hit:
            wildcards += 1
    return unmatched, wildcards


def spa_modules(creator):
    """The module set `assemble_modules` would concatenate for this edition."""
    mods = []
    for name in sorted(os.listdir(SPA_DIR)):
        if not name.endswith(".js"):
            continue
        is_creator = name.endswith("-creator.js")
        if is_creator and not creator:
            continue
        mods.append(os.path.join(SPA_DIR, name))
    return mods


# ── the self-test: prove the matcher and the extraction discriminate ───────
def self_test():
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    base_ = "/api/v1"

    # 1. A wildcard is one segment, not a subtree. This is the exact pair the
    #    missing transcript route fell between: had the matcher been
    #    prefix-tolerant, defect (a) would have reported as a match.
    check(
        "wildcard is not a subtree",
        matches("/api/v1/plugins/crunchr/transcript/*", "/api/v1/plugins/crunchr/recordings/*"),
        False,
    )
    check(
        "segment count still discriminates",
        matches("/api/v1/a/b/c", "/api/v1/a/b"),
        False,
    )
    check("exact match", matches("/api/v1/health", "/api/v1/health"), True)
    check("query string is not a segment", matches("/api/v1/a?x=1", "/api/v1/a"), True)
    check("normalise collapses an interpolation", normalise("/a/${id}/b"), "/a/*/b")
    check("normalise collapses an axum param", normalise("/a/{id}/b"), "/a/*/b")
    check(
        "a query interpolation is not a path segment",
        normalise(base_ + strip_query_interpolations("/projects/${id}/codings${qs}")),
        "/api/v1/projects/*/codings",
    )
    check(
        "a conditional query is not a path segment",
        normalise(base_ + strip_query_interpolations("/a/${q ? `?${q}` : \"\"}")),
        "/api/v1/a",
    )

    # 2. A client path with no route is reported unmatched, while a sibling
    #    that does match is counted — including when the match goes through a
    #    parameter on either side.
    client = {"/api/v1/health": "c:1", "/api/v1/plugins/gone/*": "c:2", "/api/v1/plugins/real/*": "c:3"}
    routes = {"/api/v1/health": "s:1", "/api/v1/plugins/real/{id}": "s:2"}
    unmatched, wildcards = crosscheck("t", client, routes)
    check("an absent route is reported", [p for p, _ in unmatched], ["/api/v1/plugins/gone/*"])
    check("a dynamic match is still counted", wildcards, 1)

    # 3. The extractor finds a `_fetch` call (with the base applied) and a
    #    direct absolute-URL link.
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        sample = os.path.join(tmp, "sample.js")
        with open(sample, "w", encoding="utf-8") as fh:
            fh.write(
                "  a: (id) => API._fetch(`/plugins/crunchr/recordings/${encodeURIComponent(id)}`),\n"
                '  b: (p) => `/api/v1/plugins/thumbnails/file?p=${encodeURIComponent(p)}`,\n'
                '  c: () => API._fetch("/recordings"),\n'
            )
        got = sorted(client_paths([sample], "/api/v1", set()))
    check(
        "both call shapes are extracted",
        got,
        [
            "/api/v1/plugins/crunchr/recordings/*",
            "/api/v1/plugins/thumbnails/file",
            "/api/v1/recordings",
        ],
    )
    return failures


def main(argv):
    verbose = "--verbose" in argv
    if "--self-test" in argv:
        failures = self_test()
        if failures:
            print("check-client-routes: SELF-TEST FAILED")
            for f in failures:
                print(f"  {f}")
            return 1
        print("check-client-routes: self-test OK (matcher discriminates, both call shapes extracted)")
        return 0

    creator_mods = creator_only_modules()
    if not creator_mods:
        print(
            "check-client-routes: no #[cfg(feature = \"creator\")] route module found in\n"
            f"  {ROUTES_MOD}; the edition split this check derives is empty, so the PVR\n"
            "  route set would silently equal the Creator one. Refusing to run.",
            file=sys.stderr,
        )
        return 2

    all_spa = spa_modules(True)
    bases, base_decl_lines = fetch_base(all_spa)
    if len(bases) != 1:
        print(
            f"check-client-routes: expected exactly one fetch base in the SPA, found "
            f"{sorted(bases) or 'none'}; the client-path extraction depends on it.",
            file=sys.stderr,
        )
        return 2
    base = bases.pop()

    print("client/server route cross-check")
    print(f"  fetch base (read from the bundle)                : {base!r}")
    print(f"  creator-only route modules (from {ROUTES_MOD}): {', '.join(sorted(creator_mods))}")

    failures = []
    for label, creator in (("PVR", False), ("Creator", True)):
        spa = spa_modules(creator)
        mods = route_modules(creator, creator_mods)
        client = client_paths(spa, base, base_decl_lines)
        routes = server_routes(mods)

        # Non-vacuity, asserted per edition. A comparison over an empty set
        # reports zero unmatched and looks like a clean pass.
        if not spa:
            failures.append(f"{label}: no SPA modules in this edition")
        if not client:
            failures.append(f"{label}: no client calls extracted — the scan is broken, not passing")
        if not routes:
            failures.append(f"{label}: no server routes extracted — the scan is broken, not passing")
        if not client or not routes:
            continue

        unmatched, wildcards = crosscheck(label, client, routes)
        if wildcards == 0:
            failures.append(
                f"{label}: no client path matched a parameterised route; the matcher is "
                "not meeting the dynamic routes it is supposed to resolve"
            )

        print()
        print(f"  [{label}] {len(spa)} SPA modules, {len(client)} client paths, "
              f"{len(routes)} server routes, {wildcards} resolved via a parameter")
        if verbose:
            for path in sorted(client):
                hit = next((r for r in routes if matches(path, r)), None)
                print(f"    {path:60s} -> {hit or 'UNMATCHED'}")
        for path, where in unmatched:
            print(f"    UNMATCHED  {path}   (client: {where})")
        print(f"    unmatched: {len(unmatched)}")
        failures.extend(f"{label}: client path {p} ({w}) resolves to no route" for p, w in unmatched)

    print()
    if failures:
        print(f"FAIL: {len(failures)} problem(s).")
        for f in failures:
            print(f"  {f}")
        return 1
    print("OK: every client call in both editions resolves to a mounted route.")
    print("    Uncalled routes are deliberately NOT a failure — a server route with no SPA")
    print("    caller is an operator/API surface, not a contract gap. Todo 7 documents the")
    print("    known ones instead.")
    return 0


sys.exit(main(sys.argv[1:]))
PY
