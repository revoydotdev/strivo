# Generator used once, on 2026-10-14, to render .keel/plan.md from the .omo plan and the revoy
# ledger. Kept as evidence of how the plan was derived. The plan is hand-maintained from that
# commit on; do not re-run this over it.
import sys, json, os, re, shlex
sys.path.insert(0, os.path.dirname(__file__))
from gen_plan import *

DOCS_ONLY_TOUCHES = {
    66: ["crates/research/**", "crates/editor/**"],
    70: ["crates/research/**"],
    73: ["crates/research/**", "crates/strivo-web/assets/spa/**"],
    74: ["crates/research/**", "crates/dataviz/**"],
    78: ["crates/dataviz/**", "crates/strivo-web/assets/spa/**"],
    79: ["crates/dataviz/**"],
    81: ["crates/strivo-web/assets/spa/**", "crates/strivo-web/src/routes/**"],
    84: ["crates/research/**"],
    85: ["crates/research/**", "crates/strivo-web/src/routes/**"],
    99: ["crates/dataviz/**"],
    100: ["src/platform/**", "crates/strivo-web/src/routes/**"],
    101: ["src/platform/**"],
    102: ["crates/strivo-web/src/routes/**"],
}
QA_RUST = "cargo test --workspace --all-targets --locked"
QA_E2E = "cd crates/strivo-web/e2e && npm test"
GATE_TEST = {"id": "G1", "statement": "", "proof": {"kind": "command", "cmd": QA_RUST, "timeout": 3600}}

def todo(kid, title, lane, needs, touches, do, qa, commit, must=None, owner=None, done=False):
    return {"id": kid, "title": title, "lane": lane, "needs": needs, "touches": touches,
            "do": do, "qa": qa, "commit": commit, "must": must, "owner": owner, "done": done}

milestones = []   # list of dicts: id, title, epoch, lanes, gates, phases
def phase(mid, letter, title, exits, todos):
    return {"mid": mid, "letter": letter, "title": title, "exits": exits, "todos": todos}

# ---------- M1 ----------------------------------------------------------------
M1_LANE = "L1"
m1 = []
m1.append(phase(1, "A", "Gate repair and claims that must hold at HEAD", [
    ("The CI check job's test lanes pass at the declared HEAD", "proof", QA_RUST),
    ("CONTRIBUTING.md states the gates CI runs, and CI runs no gate it omits", "verify", "auditor"),
], [
    todo("1-A1", "Keep the ipc_handshake socket tests independent of TMPDIR length", M1_LANE, "-",
         ["tests/ipc_handshake.rs"],
         "The three socket tests bind a Unix socket under the tempfile directory. Under a TMPDIR longer than about 107 bytes Listener::bind fails with 'path must be shorter than SUN_LEN' (gate run at 6a2cd1c with TMPDIR under the scratchpad: release lane, 3 failed of 4). Put each socket in a short directory the test owns, so the tests pass under any TMPDIR. The qa below is the failing-first check: it must fail on HEAD before the fix.",
         "d=\"$(mktemp -d)/$(printf 'x%.0s' $(seq 1 100))\" && mkdir -p \"$d\" && TMPDIR=\"$d\" cargo test --locked --test ipc_handshake 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\\. [1-9][0-9]* passed'",
         "fix(tests): keep the ipc socket tests under any TMPDIR length",
         must="Do not weaken the bind or its SUN_LEN check in the listener, and do not skip the three tests. The production socket path (src/ipc.rs:242, state_dir) has the same limit; it is carried as a finding in M2-C."),
    todo("1-A2", "State in CONTRIBUTING.md the gates CI actually runs", M1_LANE, "-",
         ["CONTRIBUTING.md"],
         "CONTRIBUTING.md 'Before opening a pull request' lists fmt, clippy and cargo test --workspace. CI's check job (.github/workflows/ci.yml) also runs the release and debug test lanes, the Creator-feature clippy and tests, the licence-backend typecheck and tests, and the Playwright suite in crates/strivo-web/e2e. List them so a red CI run can be reproduced locally.",
         "git grep -q -- '--features creator' CONTRIBUTING.md && git grep -q 'npm test' CONTRIBUTING.md",
         "docs(contributing): list the gates CI actually runs",
         must="Do not change the CI workflow to match the document."),
    todo("1-A3", "Keep the release and docs lints green against the last two weeks of bumps and docs commits", M1_LANE, "-",
         [".github/workflows/**", "package.json", "package-lock.json", "CHANGELOG.md", "ROADMAP.md"],
         "The last fourteen days landed dependabot bumps (aaa72a4, de1b872, 5db7f63, a6b7166) and three docs commits (b064cd5, c7b60f5, 105dd65). Run the packaging and docs lints CI runs (scripts/check-version-agreement.sh, scripts/check-changelog-unreleased.sh, scripts/check-srcinfo.sh) and fix what they report against those commits.",
         "scripts/check-version-agreement.sh && scripts/check-changelog-unreleased.sh && scripts/check-srcinfo.sh",
         "ci: keep the release and docs lints green after the dependency bumps",
         must="The version bump is derived from conventional-commit prefixes, never hand-picked (AGENTS.md, Releases). Do not create or push a release tag."),
]))
m1.append(phase(1, "B", "Wave 8 in flight: live run graph, template library, cross-archive retrieval", [
    ("The Wave 8 surfaces pass the workspace suite and the browser suite", "proof", QA_RUST),
    ("Each run node shows live state, a retry re-arms its branch, and a lock timeout names the holder", "verify", "auditor"),
], []))
m1.append(phase(1, "C", "Landed legacy todos 1-34, 103, 104 (ledger record of work already on main)", [
    ("Every landed commit named by these todos is an ancestor of HEAD", "verify", "auditor"),
], []))
m1.append(phase(1, "D", "Landed legacy todos 35-54, 57, 59 (ledger record of work already on main)", [
    ("Every landed commit named by these todos is an ancestor of HEAD", "verify", "auditor"),
], []))

# Wave 8 open items from legacy
def legacy_todo(n, lane, touches_override=None, extra_needs=(), do_prefix="", qa=None, owner=None, must_extra=""):
    t = T[n]
    raw = block_join(t["lines"], r"What to do / Must NOT do:", [r"Parallelization:", r"References:", r"Acceptance", r"QA scenarios", r"Commit:"])
    must = None
    if "**Must NOT**" in raw:
        raw, must = raw.split("**Must NOT**", 1)
        must = "Must NOT" + must
        must = must.strip(" .")
    raw = raw.replace("**", "").strip()
    acc = block_join(t["lines"], r"Acceptance criteria", [r"QA scenarios", r"Commit:"]) or ""
    do = (do_prefix + raw + (" Acceptance (legacy): " + acc if acc else "")).strip()
    needs = [ID[d] for d in parse_blocked(t["lines"]) if d in ID] + list(extra_needs)
    needs = list(dict.fromkeys(needs))
    touches = touches_override if touches_override is not None else touches_for(ref_paths(refs(t["lines"])))
    if not touches:
        touches = DOCS_ONLY_TOUCHES[n]
    subj = commit_subject(t["lines"]) or "chore: " + t["title"][:50].lower()
    return todo(ID[n], t["title"], lane, needs or "-", touches, do, qa or QA_RUST, subj,
                must=(must or "") + ((" " + must_extra) if must_extra else "") or None, owner=owner)

m1_b_todos = [
    legacy_todo(55, M1_LANE, qa=QA_RUST + " && " + QA_E2E),
    legacy_todo(56, M1_LANE),
    legacy_todo(58, M1_LANE),
]
m1_b_todos[0]["touches"].append("crates/strivo-web/e2e/**")  # 6a2cd1c touched the mock server and smoke spec
m1[0]["todos"] += [
    todo("1-A4", "Stop the composer from disturbing an untouched tile's player", M1_LANE, "-",
         ["crates/strivo-web/assets/spa/**", "crates/strivo-web/e2e/tests/**"],
         "The browser mock lane fails crates/strivo-web/e2e/tests/player-controller.spec.ts:336 at 6a2cd1c, in both of two runs: 'composer edits leave an untouched tile's player alone' (the fake player's destroyed log contains the playing id). Find which composer edit reaches the untouched tile and stop it.",
         "cd crates/strivo-web/e2e && npx playwright test tests/player-controller.spec.ts -g 'composer edits leave'",
         "fix(web): keep an untouched multiview tile's player when the composer edits",
         must="Do not weaken the assertion that the untouched player is not destroyed."),
    todo("1-A5", "Pin down the late-recordings race in performance-navigation.spec.ts or prove it flaky", M1_LANE, "-",
         ["crates/strivo-web/e2e/tests/**"],
         "In the first of two runs at 6a2cd1c, crates/strivo-web/e2e/tests/performance-navigation.spec.ts:9 ('a late recordings response cannot replace a newer Settings route') failed; in the second run it passed. Repeat the spec until the failure either reproduces or the repetition count shows it does not. If it reproduces, fix the route guard in the SPA.",
         "cd crates/strivo-web/e2e && npx playwright test tests/performance-navigation.spec.ts --repeat-each=10",
         "test(web): repeat the late-response route spec until its race is settled",
         must="Do not add a retry to make it pass."),
]
m1_b_todos += [
    todo("1-B4", "Make the Creator pipelines surface pass the keyboard, axe and UX-budget gates", M1_LANE, "1-B1",
         ["crates/strivo-web/assets/spa/**", "crates/strivo-web/assets/spa-css/**", "crates/strivo-web/e2e/tests/**"],
         "Deterministic failures at 6a2cd1c on the Creator #/pipelines route, in both of two mock-lane runs: keyboard Tab reaches 70 of 73 focusable controls (a11y-keyboard.spec.ts:131), axe reports critical or serious WCAG 2.2 AA violations (a11y-scan.spec.ts:153), the hit-target floor of 28px is missed (ux-budget.spec.ts:196), and text under 12px outside .micro is present (ux-budget.spec.ts:210). The route is the Wave 8 node graph (todos 52-54), so these belong to the in-flight work.",
         "cd crates/strivo-web/e2e && npx playwright test tests/a11y-keyboard.spec.ts tests/a11y-scan.spec.ts tests/ux-budget.spec.ts -g 'Creator #/pipelines'",
         "fix(pipelines): make the node graph reachable by keyboard and inside the UX budget",
         must="Do not lower the WCAG floor, the 28px target or the 12px text floor in the tests to pass."),
    todo("1-B5", "Make the Creator plugins, archive and dataviz routes pass the UX-budget gates", M1_LANE, "1-B4",
         ["crates/strivo-web/assets/spa-css/**", "crates/strivo-web/assets/spa/**"],
         "Deterministic failures at 6a2cd1c in both mock-lane runs: Creator #/plugins has a hit target under 28px (ux-budget.spec.ts:196) and text under 12px (:210); Creator #/archive and #/dataviz have text under 12px outside .micro (:210). Fix the offending rules in the design tokens, not per route.",
         "cd crates/strivo-web/e2e && npx playwright test tests/ux-budget.spec.ts -g 'Creator #/(plugins|archive|dataviz)'",
         "style(web): bring the plugins, archive and dataviz routes inside the UX budget",
         must="Do not deviate from DESIGN.md without the owner's approval (AGENTS.md, Design system)."),
]
m1[1]["todos"] = m1_b_todos

# landed todos
def landed_todo(n, kid):
    t = T[n]
    subj = landed_subject(n)
    sha, files = landed_files(subj)
    touches = dir_touches(files) or ["README.md"]
    # the qa is an ancestry check on the landing subject: runnable today, fails if the commit leaves HEAD
    full = git("rev-parse", sha).strip() if sha else ""
    assert full, f"no landing commit for todo {n}"
    qa = f"git merge-base --is-ancestor {full} HEAD"
    needs = [ID[d] for d in parse_blocked(t["lines"]) if d in ID]
    needs = list(dict.fromkeys(needs))
    return todo(kid, t["title"], M1_LANE, needs or "-", touches,
                "Landed on main as " + (sha[:7] if sha else "(commit not found)") + ": " + subj + ". Legacy todo " + str(n) + " in .omo/plans/strivo-review-and-pro-edition.md.",
                qa, subj, done=True)

C_todos = [landed_todo(n, ID[n]) for n in C_list]
D_todos = [landed_todo(n, ID[n]) for n in D_list]
m1[2]["todos"] = C_todos
m1[3]["todos"] = D_todos

# ---------- M2: the revoy ledger rows -------------------------------------------
M2_LANE = "L1"
m2 = []
m2.append(phase(2, "A", "Pro licence and upgrade surface", [
    ("The licence backend deploys from a workflow and the upgrade card reads the entitlement it displays", "proof", "git grep -q 'wrangler' -- .github/workflows"),
    ("The Plugins hub shows a real entitlement verdict for a free and a Pro account", "verify", "auditor"),
], [
    todo("2-A1", "Deploy licence-backend through a workflow once the D1 database exists", M2_LANE, "-",
         ["licence-backend/**", ".github/workflows/**"],
         "licence-backend/wrangler.toml:11 still holds REPLACE_WITH_D1_ID, and no workflow in .github/workflows references wrangler (checked at 6a2cd1c). Once the owner has created the D1 database and recorded its id, replace the placeholder and add a workflow that deploys the Worker, so Pro can be sold. Ledger row: 'Deploy licence-backend to Cloudflare'.",
         "git grep -q 'wrangler' -- .github/workflows && ! git grep -q 'REPLACE_WITH_D1_ID' -- licence-backend/wrangler.toml",
         "ci: deploy the licence backend from a workflow",
         owner="create the Cloudflare D1 database and record its id in licence-backend/wrangler.toml (the placeholder is on line 11); this is the act the todo waits on, and it is not a code change",
         must="Do not put the account id or any token in the repo. The deploy secret lives in the repository's secrets."),
    todo("2-A2", "Make the Creator upgrade card read licence.implemented, not only licence.entitled", M2_LANE, "2-A1",
         ["crates/strivo-web/assets/spa/**"],
         "Ledger row PHASE0/F-11 (P1). crates/strivo-web/assets/spa/021-creator.js:694 branches on licence.entitled alone, and the trial button calls API.licenceTrial() unconditionally (021-creator.js:32, :713). The backend response carries an implemented flag (crates/strivo-web/src/routes/licence.rs:38, :83 set false), which the SPA never reads. Show a real verdict: not implemented means a stated state, not an upgrade offer.",
         "cd crates/strivo-web/e2e && npm test",
         "fix(web): show the real licence verdict on the upgrade card",
         must="Do not ship a price or a payment flow."),
    todo("2-A3", "Bring the Pro panes off route null, or stop the signposts pointing at them", M2_LANE, "2-A2",
         ["crates/strivo-web/assets/spa/**"],
         "Ledger row PHASE0/F-12 (P1). Re-counted at 6a2cd1c: crates/strivo-web/assets/spa/021-creator.js has 20 'route:' entries of which 15 are 'route: null'. The ledger says 14 of 20; reconcile the count when the fix lands and cite the file:line of each remaining null pane.",
         "cd crates/strivo-web/e2e && npm test",
         "fix(web): give every Pro pane a route or remove its signpost",
         must="Do not remove a pane that the entitlement tests reference."),
    todo("2-A4", "Make /api/v1/pipelines/dag read real run state, not four fixed templates", M2_LANE, "1-B2",
         ["crates/strivo-web/src/routes/**"],
         "Ledger row 'Reconcile /api/v1/pipelines/dag with real run state'. crates/strivo-web/src/routes/api.rs:390 documents the route as the canonical DAW-vision pipeline and api.rs:1022 mounts it; crates/strivo-web/src/routes/pipeline_graph.rs:7 says it still serves the fixed templates unchanged. Serve the executor's real state, or retire the route with its callers.",
         QA_RUST,
         "fix(pipelines): serve the dag route from real run state"),
]))
m2.append(phase(2, "B", "Pipeline runtime", [
    ("The executor's shared resource locks, event channel and stage scan match the ledger's claims or the claims are retired", "proof", QA_RUST),
    ("A stalled consumer and a wedged GPU stage are both bounded and visible", "verify", "auditor"),
], [
    todo("2-B1", "Give the pipeline a transcription-specific cap so a long transcribe cannot starve the GPU", M2_LANE, "-",
         ["src/pipeline/**"],
         "Ledger row PHASE0/F-36 (P2). src/pipeline/executor.rs:775-782 and :936 hold one process-wide Gpu permit, so a long transcription blocks every other GPU stage. Add a transcription-specific limit, and test that a second GPU stage makes progress while one transcription is running.",
         QA_RUST,
         "fix(pipeline): cap transcription separately from the shared GPU permit"),
    todo("2-B2", "Bound the pipeline event channel", M2_LANE, "2-B1",
         ["src/pipeline/**", "src/daemon.rs"],
         "Ledger row PHASE0/F-38 (P3). src/pipeline/runtime.rs:376 creates the pipeline event channel with mpsc::unbounded_channel, so a stalled consumer grows memory without limit. Bound it and decide the overflow behaviour (drop oldest or back-pressure), and say which in the code.",
         QA_RUST,
         "fix(pipeline): bound the event channel and say what overflow does"),
    todo("2-B3", "Stop the executor rescanning the whole stage registry on every wake", M2_LANE, "2-B2",
         ["src/pipeline/**"],
         "Ledger row PHASE0/F-35 (P3). The executor rescans the stage registry on each wake to find ready stages. Keep a ready set updated on completion. Locate the scan at HEAD before changing it; the ledger's line reference is not re-derived here.",
         QA_RUST,
         "perf(pipeline): track ready stages instead of rescanning the registry"),
    todo("2-B4", "Render an EDL timeline in one ffmpeg pass, not one process per cut plus a concat", M2_LANE, "2-B3",
         ["crates/editor/**"],
         "Ledger row PHASE0/F-33 (P2). The EDL render runs one ffmpeg subprocess per cut and a final concat pass, so a long timeline is N+1 spawns (crates/editor/src/render/invoke.rs and execute.rs). Collapse it to one invocation that the render plan describes, and keep the rendered output byte-for-byte identical to the per-cut result on the fixture corpus.",
         QA_RUST,
         "perf(editor): render an edl timeline in one ffmpeg pass"),
]))
m2.append(phase(2, "C", "Web surface and performance", [
    ("Every ledger row in this phase is closed or re-derived against HEAD with its evidence cited", "verify", "auditor"),
    ("The served bundle, pagination and SSE fan-out match the measurements in docs/PERFORMANCE.md", "proof", QA_RUST),
], [
    todo("2-C1", "Replace hardcoded hex values in spa-css with design tokens", M2_LANE, "2-A3",
         ["crates/strivo-web/assets/spa-css/**"],
         "Ledger row PHASE0/F-22 (P3). Roughly 55 hardcoded hex values remain in crates/strivo-web/assets/spa-css/ (000-pvr.css:1028 is representative; a count at 6a2cd1c finds 10 in 000-pvr.css and 25 across the directory). Replace them with the DESIGN.md tokens; scripts/check-design-tokens.mjs is the gate.",
         "node scripts/check-design-tokens.mjs",
         "style(css): replace the remaining hardcoded hex values with tokens",
         must="Do not deviate from DESIGN.md without the owner's approval (AGENTS.md, Design system)."),
    todo("2-C2", "Replace the offset-plus-COUNT recordings pagination with keyset paging", M2_LANE, "2-C1",
         ["src/recording/**", "crates/strivo-web/src/routes/**"],
         "Ledger row PHASE0/F-27 (P3). The recordings list pages with an integer offset and a per-page COUNT(*), so deep pages degrade linearly. The SQL lives in src/recording/persist.rs. Move to keyset paging and keep the cursor the SPA already sends.",
         QA_RUST,
         "perf(db): page recordings with a keyset cursor"),
    todo("2-C3", "Add a covering index for the recordings json_extract query", M2_LANE, "2-C2",
         ["src/recording/**"],
         "Ledger row PHASE0/F-28 (P3). The recordings query runs a per-row json_extract with no covering index. Measure the query with docs/perf/ before and after; record the number, not a claim.",
         QA_RUST,
         "perf(db): index the recordings query that filters on json fields"),
    todo("2-C4", "Chunk the served SPA bundle by route", M2_LANE, "2-C3",
         ["crates/strivo-web/build.rs", "crates/strivo-web/assets/spa/**"],
         "Ledger row PHASE0/F-29 (P3). The served SPA bundle is one concatenation of about 1.08 MB of modules with no route chunking, against the 250 KiB threshold in docs/PERFORMANCE.md. The bundle size is not re-measured here; scripts/check-bundle-size.sh is the measurement and CI runs it. Keep the Creator vocabulary out of the PVR bundle (crates/strivo-web/e2e pretest).",
         "scripts/check-bundle-size.sh target/pvr-release/strivo target/pvr-release/assets PVR",
         "perf(web): split the SPA bundle by route"),
    todo("2-C5", "Share one SSE fan-out instead of one IPC connection per browser tab", M2_LANE, "2-C4",
         ["crates/strivo-web/src/routes/events.rs"],
         "Ledger row PHASE0/F-30 (P3). The SSE stream opens one IPC connection per browser tab with no server-side fan-out (crates/strivo-web/src/routes/events.rs). Put one upstream connection behind a broadcast.",
         QA_RUST,
         "perf(events): fan one upstream IPC connection out to every tab"),
    todo("2-C6", "Stop the plugin endpoints opening per-request resources", M2_LANE, "2-C5",
         ["crates/strivo-web/src/routes/plugins.rs"],
         "Ledger row PHASE0/F-34 (P3). The plugin endpoints open per-request resources 27 times per request (was 12); crates/strivo-web/src/routes/plugins.rs:4934 routes plugin storage size and clear. Count the opens with a test before and after.",
         QA_RUST,
         "perf(plugins): open plugin resources once per request"),
]))
m2.append(phase(2, "D", "Tests, docs and claims", [
    ("The browser lanes run every spec serially where the spec needs it, and the niche claim is removed or tested", "proof", QA_E2E),
    ("The auto-transcribe gate has one source of truth and a route test", "verify", "auditor"),
], [
    todo("2-D1", "Test the unoccupied-niche claim against the closest tool before repeating it", M2_LANE, "-",
         ["ROADMAP.md", "README.md", "docs/**"],
         "Ledger row 'Test the unoccupied-niche positioning claim against MrBrax/LiveStreamDVR'. ROADMAP.md:330-331 asserts an unoccupied '*arr-grade live-stream PVR niche'. docs/audits/2026-09-25-consolidated/MANIFEST.md:178 already records it as a market assertion the source cannot settle. Write the comparison as a doc with the sources, and keep the claim only if the comparison supports it.",
         "git grep -q 'LiveStreamDVR' -- docs",
         "docs(roadmap): record the niche comparison against the closest tool",
         must="Do not repeat the claim in customer-facing copy until this todo is done."),
    todo("2-D2", "Put the headless auto-transcribe gate behind one source of truth", M2_LANE, "2-D1",
         ["src/config/**"],
         "Ledger row PHASE0/F-16 (P2). Headless auto-transcribe is still gated on config-only lists or a marker file, with no SPA control to copy the archiver tandem toggle from. The legacy row names no file; a grep of HEAD for an auto-transcribe symbol found none, so locate the gate before changing it and record where it is.",
         QA_RUST,
         "fix(config): gate headless auto-transcribe on one setting with an SPA control"),
    todo("2-D3", "Serialise the skip-link spec that is flaky under parallel workers", M2_LANE, "2-D2",
         ["crates/strivo-web/e2e/tests/**"],
         "Ledger row WS/V05. crates/strivo-web/e2e/tests/spa-ux-remediation.spec.ts has no serial-mode configure block, unlike its sibling player-controller.spec.ts:21 (checked at 6a2cd1c: no 'serial' in the file). Add the block and run the spec alone and in the full lane.",
         QA_E2E,
         "test(web): run the skip-link spec serially"),
]))


m2[2]["todos"].append(todo("2-C7", "Give the daemon's IPC socket a path that binds under any state directory", M2_LANE, "-",
    ["src/ipc.rs"],
    "src/ipc.rs:242 places the daemon socket at state_dir().join('strivo.sock'). A long state directory makes the Unix listener fail with the same 'path must be shorter than SUN_LEN' error the ipc_handshake tests hit under a long TMPDIR (todo 1-A1). Bind through a short fixed path, or fail at startup with an error that names the length limit and the directory. The test that pins it is socket_path_length.",
    "",  # set by the TOK loop below
    "fix(ipc): bind the daemon socket under any state directory length",
    must="Do not change the protocol or the socket's permissions."))

# ---------- M3..M6: legacy waves, deferred ----------------------------------------
def legacy_phase_todos(nums):
    out = []
    for n in nums:
        out.append(legacy_todo(n, "L1", owner=None))
    return out

legacy_groups = {
    3: [("A", "Publishing vault, destinations and metadata", [60, 61, 62, 63, 64]),
        ("B", "Media QC, variants and export", [65, 66, 67]),
        ("C", "Security, release hold and real-server journey", [68, 98, 69])],
    4: [("A", "Outcome ingestion and linked timeline", [70, 71, 72, 73]),
        ("B", "Cohorts, funnel and dashboards", [74, 75])],
    5: [("A", "Dataviz and saved queries", [76, 77, 78, 79]),
        ("B", "Calendar, portable packages and exports", [80, 81, 82, 83, 84]),
        ("C", "Collaboration, performance, onboarding and field validation", [85, 86, 87, 88, 89])],
    6: [("A", "Transcript and moment intelligence", [90, 91, 92, 93, 94, 95, 96]),
        ("B", "Translation and datasets", [97, 99]),
        ("C", "Additional destinations and post lifecycle", [100, 101, 102])],
}
MILESTONE_TITLES = {
    1: "Current position: finish the in-flight pipeline wave and make the repo's own claims true",
    2: "PVR hygiene: the seventeen open revoy ledger rows, re-derived against HEAD",
    3: "Creator publishing loop (legacy Wave 9, deferred behind M1 and M2)",
    4: "Measured feedback (legacy Wave 10, deferred behind M3)",
    5: "Comprehensive Creator product (legacy Wave 11, deferred behind M4)",
    6: "Side waves: transcript intelligence, translation, datasets and destinations (legacy 6b-11b, deferred)",
}
EPOCHS = {
    1: "The tree at HEAD passes the repo's own gates, the Wave 8 pipeline surface (live node state, template library, cross-archive retrieval) is complete, and the tracked docs state the real gate.",
    2: "The revoy ledger's open rows are closed or re-derived against HEAD, and the licence backend deploys once the owner creates the D1 database.",
    3: "Creator publishing works end to end: a vault and capability-driven destinations, a first connector verified without credentials, metadata and calendar, a publishing package, QC and variants, and a release-hold review. Deferred behind M1 and M2: no commits in the last fourteen days touch its paths; carried verbatim from .omo Wave 9 (owner-ratified in the legacy TL;DR, 2026-09-25).",
    4: "Outcomes from authorised platforms feed the production loop: a metric dictionary, ingestion, a linked source-to-outcome timeline, cohorts and funnels. Deferred behind M3: no commits in the window; carried verbatim from .omo Wave 10.",
    5: "The Creator product has dataviz, calendar, portable packages, reproducible exports, collaboration and field-validation evidence. Deferred behind M4: no commits in the window; carried verbatim from .omo Wave 11.",
    6: "Transcript and moment intelligence, translation aids, real-media contract coverage, external datasets and the extra destinations are in place. Deferred behind M3 and M5: no commits in the window; carried verbatim from .omo side waves 6b-11b.",
}
for mid in range(3, 7):
    phases = []
    for letter, title, nums in legacy_groups[mid]:
        todos = []
        for n in nums:
            todos.append(legacy_todo(n, "L1"))
        exits = [("The phase's legacy acceptance holds at the commit that lands it", "proof", QA_RUST),
                 ("Every claim in the phase's todos is shown, not self-reported: named file changed, named command run", "verify", "auditor")]
        phases.append(phase(mid, letter, title, exits, todos))
    if mid == 3:
        # 98 moved from M6 to M3 so that 69's dependency points backwards; see Migration notes
        pass
    if mid == 6:
        pass
    globals()["m%d" % mid] = phases

# Need M2's first exit proof rebuilt when needed; M1 proof uses the Rust gate.
MILESTONES = [
    (1, MILESTONE_TITLES[1], EPOCHS[1], "L1 (the one lane; see Migration notes)", [m1[0], m1[1], m1[2], m1[3]], "M1"),
    (2, MILESTONE_TITLES[2], EPOCHS[2], "L1 (the one lane; see Migration notes)", m2, "M2"),
    (3, MILESTONE_TITLES[3], EPOCHS[3], "L1 (the one lane; see Migration notes)", m3, "M3"),
    (4, MILESTONE_TITLES[4], EPOCHS[4], "L1 (the one lane; see Migration notes)", m4, "M4"),
    (5, MILESTONE_TITLES[5], EPOCHS[5], "L1 (the one lane; see Migration notes)", m5, "M5"),
    (6, MILESTONE_TITLES[6], EPOCHS[6], "L1 (the one lane; see Migration notes)", m6, "M6"),
]

def todo_block(t):
    mark = "x" if t["done"] else " "
    lines = [f"- [{mark}] {t['id']} — {t['title']}",
             f"  lane: {t['lane']}",
             f"  needs: {t['needs'] if isinstance(t['needs'], str) else ', '.join(t['needs']) or '-'}",
             f"  touches: {', '.join(t['touches'])}",
             f"  do: {t['do']}"]
    if t.get("must"):
        lines.append(f"  must-not: {t['must']}")
    if t.get("owner"):
        lines.append(f"  owner: {t['owner']}")
    lines.append(f"  qa: `{t['qa']}`")
    lines.append(f"  commit: {t['commit']}")
    return "\n".join(lines)

def exit_block(e):
    text, kind, val = e
    if kind == "proof":
        return f"- [ ] {text} | proof: `{val}`"
    return f"- [ ] {text} | verify: {val}"

def gates_for(mid, label):
    if mid == 1:
        return {"schema": "keel.gates/1", "milestone": "1", "gates": [
            {"id": "G1", "statement": "fmt is clean at HEAD (CI check job, fmt step)", "proof": {"kind": "command", "cmd": "cargo fmt --all -- --check", "timeout": 600}},
            {"id": "G2", "statement": "clippy is clean with warnings denied across the workspace (CI clippy step)", "proof": {"kind": "command", "cmd": "cargo clippy --workspace --all-targets --locked -- -D warnings", "timeout": 1800}},
            {"id": "G3", "statement": "the Creator-feature clippy is clean (CI clippy Creator step)", "proof": {"kind": "command", "cmd": "cargo clippy -p strivo-core -p strivo-web -p strivo-bin --features creator --all-targets --locked -- -D warnings", "timeout": 1800}},
            {"id": "G4", "statement": "the release test lane passes (CI test step)", "proof": {"kind": "command", "cmd": "cargo test --workspace --release --all-targets --locked", "timeout": 3600}},
            {"id": "G5", "statement": "the debug test lane passes (CI debug test step)", "proof": {"kind": "command", "cmd": "cargo test --workspace --all-targets --locked", "timeout": 3600}},
        ]}
    if False:  # superseded by the milestone-token gate below
        return {"schema": "keel.gates/1", "milestone": "2", "gates": [
            {"id": "G1", "statement": "the workspace suite passes with the hygiene todos landed", "proof": {"kind": "command", "cmd": QA_RUST, "timeout": 3600}},
            {"id": "G2", "statement": "the browser mock lane passes with the SPA changes", "proof": {"kind": "command", "cmd": QA_E2E, "timeout": 3600}},
        ]}
    if mid >= 2:
        toks = milestone_tokens(mid)
        g1 = guard_all(toks) if toks else QA_RUST
        return {"schema": "keel.gates/1", "milestone": str(mid), "gates": [
            {"id": "G1", "statement": f"every test this milestone's todos add exists and passes (milestone {mid})", "proof": {"kind": "command", "cmd": g1, "timeout": 3600}},
            {"id": "G2", "statement": f"the browser mock lane passes with the milestone {mid} todos landed", "proof": {"kind": "command", "cmd": QA_E2E, "timeout": 3600}},
        ]}
    return {"schema": "keel.gates/1", "milestone": str(mid), "gates": [
        {"id": "G1", "statement": f"the workspace suite passes with the {label} todos landed", "proof": {"kind": "command", "cmd": QA_RUST, "timeout": 3600}},
        {"id": "G2", "statement": f"the browser mock lane passes with the {label} todos landed", "proof": {"kind": "command", "cmd": QA_E2E, "timeout": 3600}},
    ]}


# ---- test-named qa for open todos: a filter the todo must make match, so it fails at HEAD ----
GUARD_OK = "grep -Eq 'test result: ok\\. [1-9][0-9]* passed'"
def guard(tok):
    return f"cargo test --locked --workspace {tok} 2>&1|tee /dev/stderr|{GUARD_OK}"
def guard_all(toks):
    if len(toks) == 1:
        return guard(toks[0])
    return "for t in " + " ".join(toks) + "; do cargo test --locked --workspace \"$t\" 2>&1|tee /dev/stderr|" + GUARD_OK + " || exit 1; done"
TOK = {
    "1-B1": "live_run_graph", "1-B2": "template_library", "1-B3": "archive_retrieval",
    "2-A4": "dag_reads_run_state", "2-B1": "transcription_gpu_cap", "2-B2": "bounded_event_channel",
    "2-B3": "ready_set", "2-B4": "edl_single_pass", "2-C1": "no_raw_hex_in_spa_css",
    "2-C2": "keyset_paging", "2-C3": "recordings_query_index", "2-C4": "route_chunks",
    "2-C5": "sse_fanout", "2-C6": "plugin_resources_opened_once", "2-C7": "socket_path_length", "2-D2": "headless_auto_transcribe",
}
LEG_TOK = {60:"publishing_vault",61:"youtube_upload_connector",62:"metadata_disclosure_editor",63:"publishing_calendar_dst",64:"manual_publishing_package",65:"final_media_qc",66:"output_variant_staleness",67:"export_profile_batch_queue",68:"creator_route_security_review",69:"release_hold_packaging",98:"real_server_creator_journey",70:"metric_dictionary",71:"derived_proxy_labels",72:"youtube_analytics_ingestion",73:"outcome_timeline_linked",74:"comparable_cohorts",75:"production_funnel_effort",76:"saved_dataviz_queries",77:"linked_brushing_annotations",78:"creator_dashboards",79:"branded_chart_snapshots",80:"calendar_recommendations",81:"cross_platform_review_surfaces",82:"portable_project_packages",83:"reproducible_figure_exports",84:"outcome_informed_experiments",85:"review_comments_roles",86:"resource_aware_acceleration",87:"qualification_matrix",88:"capability_doctor",89:"field_validation_instrumentation",90:"canonical_transcript_revisions",91:"speaker_correction_glossary",92:"multimodal_moment_ranking",93:"moment_inbox",94:"ai_rough_cut_proposals",95:"external_processing_consent",96:"creator_eval_datasets",97:"translation_aids",99:"external_dataset_import",100:"tiktok_instagram_connectors",101:"additional_destinations",102:"batch_policy_automation"}
for n, tk in LEG_TOK.items():
    TOK[ID[n]] = tk
SPA_QA = {
    "2-A2": "cd crates/strivo-web/e2e && npx playwright test tests/licence-upgrade-card.spec.ts",
    "2-A3": "cd crates/strivo-web/e2e && npx playwright test tests/pro-panes-routed.spec.ts",
}
NOTE = " The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists)."
for mid, title, epoch, lanes, phases, _ in MILESTONES:
    for p in phases:
        for t in p["todos"]:
            if t["done"]:
                continue
            if t["id"] in SPA_QA:
                t["qa"] = SPA_QA[t["id"]]
            elif t["id"] in TOK:
                t["qa"] = guard(TOK[t["id"]])
                t["do"] = t["do"] + NOTE.replace("the qa names", f"The qa names (`{TOK[t['id']]}`)")
    # phase exits: a proof that fails at HEAD, not the generic suite that passes at HEAD
    for p in phases:
        new = []
        for text, kind, val in p["exits"]:
            if kind == "proof" and val == QA_RUST and mid >= 2:
                toks = [t["id"] for t in p["todos"]]
                tk = next((TOK[i] for i in toks if i in TOK), None)
                if tk:
                    val = guard(tk)
            if kind == "proof" and val == QA_RUST and mid == 1 and p["letter"] == "B":
                val = QA_E2E
            new.append((text, kind, val))
        p["exits"] = new
# milestone gates: each open milestone checks its own test tokens
def milestone_tokens(mid):
    out = []
    for m in MILESTONES:
        if m[0] != mid: continue
        for p in m[4]:
            for t in p["todos"]:
                if not t["done"] and t["id"] in TOK:
                    out.append(TOK[t["id"]])
    return out


# ---- overrides: two todos whose qa or premise did not hold at HEAD ---------------
for t in m1[0]["todos"]:
    if t["id"] == "1-A3":
        t["title"] = "Make ROADMAP's gate claim match what CI runs"
        t["touches"] = ["ROADMAP.md"]
        t["do"] = ("ROADMAP.md:150-151 says the gate (cargo fmt, clippy, cargo test --workspace) 'is green on main'. "
                   "The lint premise of the earlier version of this todo already held at HEAD (the CHANGELOG action pins match the workflows, and the packaging lints exit 0), so its qa passed vacuously and it is replaced. "
                   "The claim is incomplete: CI's browser lanes (ci.yml:239, ci.yml:258) are part of the same gate and the mock lane fails 9 tests at HEAD. "
                   "Rewrite the sentence to name the gate CI runs, and say which lane is red until 1-A4, 1-B4 and 1-B5 land.")
        t["qa"] = "! git grep -q 'is green on' -- ROADMAP.md"
        t["commit"] = "docs(roadmap): state the gate CI runs and which lane is red"
        t["must"] = "Do not claim the gate is green while a browser lane CI runs is red."
for mid, title, epoch, lanes, phases, _ in MILESTONES:
    for p_ in phases:
        for t in p_["todos"]:
            if t["id"] == "2-D3":
                t["qa"] = 'git grep -q \'mode: "serial"\' -- crates/strivo-web/e2e/tests/spa-ux-remediation.spec.ts'



# ---- lanes by area, and needs edges only where two lanes share a path -----------
AREAS = ["web-ui", "web-server", "daemon", "editor", "research", "plugins", "platform", "tests", "docs-ci", "crates-other", "ledger"]
AREA_LABEL = {
    "web-ui": "strivo-web assets, SPA and e2e",
    "web-server": "strivo-web server routes and tests",
    "daemon": "src/ daemon, pipeline and recording",
    "editor": "crates/editor render and timeline",
    "research": "crates/research and crates/dataviz",
    "plugins": "crates/strivo-plugins and crates/pipelines-dag",
    "platform": "src/platform, src/licence and licence-backend",
    "tests": "top-level tests/",
    "docs-ci": "docs, tracked top-level docs, CI and packaging",
    "crates-other": "other crates",
    "ledger": "landed work, recorded done in the ledger",
}
def area_of(path):
    if path.startswith(("crates/strivo-web/assets", "crates/strivo-web/e2e")): return "web-ui"
    if path.startswith("crates/strivo-web"): return "web-server"
    if path.startswith(("src/platform", "src/licence", "licence-backend")): return "platform"
    if path.startswith("src/"): return "daemon"
    if path.startswith("crates/editor"): return "editor"
    if path.startswith(("crates/research", "crates/dataviz")): return "research"
    if path.startswith(("crates/strivo-plugins", "crates/pipelines-dag")): return "plugins"
    if path.startswith("tests/"): return "tests"
    if path.startswith(("docs", ".github", "package", "README.md", "ROADMAP.md", "CHANGELOG.md", "CONTRIBUTING.md")): return "docs-ci"
    return "crates-other"
def stem(p):
    cut = len(p)
    for ch in "*?[":
        i = p.find(ch)
        if i != -1: cut = min(cut, i)
    return p[:cut]
def overlaps(a, b):
    sa, sb = stem(a), stem(b)
    return sa.startswith(sb) or sb.startswith(sa)

def assign_lanes():
    flat = []   # (milestone index, todo)
    for mi, (mid, title, epoch, lanes, phases, _) in enumerate(MILESTONES):
        for p_ in phases:
            for t in p_["todos"]:
                flat.append((mid, t))
    by_id = {t["id"]: t for _, t in flat}
    for mid, t in flat:
        t["area"] = "ledger" if t["done"] else area_of(t["touches"][0])
    # lane letters per milestone, in AREAS order
    new_lanes = {}
    for mid in sorted({m for m, _ in flat}):
        used = [a for a in AREAS if any(t["area"] == a for m, t in flat if m == mid)]
        letter = {a: f"L{i+1}" for i, a in enumerate(used)}
        for m, t in flat:
            if m == mid:
                t["lane"] = letter[t["area"]]
        new_lanes[mid] = ", ".join(f"{letter[a]} ({AREA_LABEL[a]})" for a in used)
    # needs: ancestors and a topological order over the whole plan
    def needs_of(t):
        n = t["needs"]
        return [] if n == "-" or not n else list(n)
    for mid, t in flat:
        t["needs"] = [x for x in needs_of(t) if x in by_id] or "-"
    def ancestors():
        anc = {}
        def walk(i, seen):
            for d in (list(by_id[i]["needs"]) if by_id[i]["needs"] != "-" else []):
                if d not in seen:
                    seen.add(d); walk(d, seen)
            return seen
        for i in by_id: anc[i] = walk(i, set())
        return anc
    order, placed = [], set()
    while len(order) < len(by_id):
        progressed = False
        for mid, t in flat:
            i = t["id"]
            if i in placed: continue
            deps = [] if t["needs"] == "-" else t["needs"]
            if all(d in placed for d in deps):
                order.append(i); placed.add(i); progressed = True
        assert progressed, "cycle in needs"
    pos = {i: k for k, i in enumerate(order)}
    added = []
    for mid in sorted({m for m, _ in flat}):
        ms = [t for m, t in flat if m == mid]
        for i, a in enumerate(ms):
            for b in ms[i+1:]:
                if a["lane"] == b["lane"]: continue
                anc = ancestors()
                if a["id"] in anc[b["id"]] or b["id"] in anc[a["id"]]: continue
                if not any(overlaps(pa, pb) for pa in a["touches"] for pb in b["touches"]): continue
                if a["done"] != b["done"]:
                    later, earlier = (a, b) if not a["done"] else (b, a)   # the open todo waits on the landed one
                else:
                    later, earlier = (a, b) if pos[a["id"]] > pos[b["id"]] else (b, a)
                later["needs"] = (list(later["needs"]) if later["needs"] != "-" else []) + [earlier["id"]]
                added.append((later["id"], earlier["id"]))
    for mi, (mid, title, epoch, lanes, phases, _) in enumerate(MILESTONES):
        MILESTONES[mi] = (mid, title, epoch, new_lanes.get(mid, lanes), phases, _)
    return added

ADDED_EDGES = assign_lanes()


import json as _json
UNPLANNED = _json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "unplanned.json")))

def write(floor=None):
    L_scope = L[23:71]  # legacy '### Must have' .. end of '### Must NOT have' (verbatim)
    n_ms = len(MILESTONES)
    n_phase = sum(len(m[4]) for m in MILESTONES)
    n_todo = sum(len(p["todos"]) for m in MILESTONES for p in m[4])
    n_done = sum(1 for m in MILESTONES for p in m[4] for t in p["todos"] if t["done"])
    n_open = n_todo - n_done
    out = []
    out.append("# strivo — Plan\n")
    out.append("## TL;DR (for humans)\n")
    out.append("StriVo is a self-hosted stream recorder with a browser UI, and its Creator Edition sits behind a cargo feature. The last fourteen days changed little: nine commits, mostly docs, CI and dependency bumps, plus one Creator pipeline fix. The plan therefore starts with the work already in flight (the pipeline run surface, the template library and cross-archive retrieval), then fixes the one failing test lane found on the way and the repo's own claims. Next come the seventeen open rows of the old revoy ledger. The publishing, analytics and transcript work from the legacy Creator plan is kept verbatim in later milestones, deferred behind the live work, because it has no commits in the window.\n")
    out.append("It will not add a remote studio, cloud rendering, a public plugin marketplace, a pricing or payment decision, or any test that needs a live platform account. The one act only the owner can do is creating the Cloudflare D1 database for the licence backend, and it is written as an `owner:` line.\n")
    out.append("## TL;DR (machine)\n")
    out.append(f"{n_ms} milestones, {n_phase} phases, {n_todo} todos ({n_done} landed and recorded done from their commits, {n_open} open). Active milestone: M1. Lanes split by path area, 4 to 8 per milestone, and every open todo has a test-named qa that fails today. One owner act (2-A1). Measured over the four-week window: 109 commits, 2% uncovered (2 commits, both dispositioned); the 14-day window has 9 commits and is too small for a fraction. The browser mock lane fails the same 9 tests at HEAD in both of two runs (covered by todos 1-A4, 1-B4 and 1-B5); performance-navigation.spec.ts:9 failed in one run and passed in the other (todo 1-A5). Risks: the Creator publishing and analytics items are large and untested against live platforms.\n")
    out.append("## Scope\n")
    out.append("\n".join(L_scope) + "\n")
    out.append("## Verification\n")
    out.append("- Test decision: TDD for every open todo; the qa names the test that must fail first. Landed todos are checked by commit ancestry (`git log --format=%s` contains the landing subject), not by re-running their old tests.")
    out.append("- Rust gate, as CI's `check` job runs it (`.github/workflows/ci.yml`): `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p strivo-core -p strivo-web -p strivo-bin --features creator --all-targets --locked -- -D warnings`; `cargo test --workspace --release --all-targets --locked`; `cargo test --workspace --all-targets --locked`; `cargo test -p strivo-web -p strivo-bin --features creator --all-targets --locked`.")
    out.append("- Browser gate: `cd crates/strivo-web/e2e && npm test` (the mock lane; its pretest runs check-pvr-bundle.mjs). The real-server lane `npm run test:real` needs the release PVR binary and a sandboxed HOME.")
    out.append("- Licence backend: `cd licence-backend && npm run typecheck && npm test`.")
    out.append("- Local test environment: `.cargo/config.toml` (untracked, git-ignored) sets the target dir to /scratch/cargo-targets/strivo-main. CI scratches on real disk through $RUNNER_TEMP; run the suite with a short TMPDIR, because the socket tests fail under a long one (todo 1-A1).")
    out.append("- Resource rule carried from the legacy plan: a long soak or a disk-pressure rehearsal runs inside a memory-capped scope on a throwaway loopback filesystem with a declared size ceiling, never on the shared workstation's real disk and never against a live library (docs/PERFORMANCE.md:83).")
    out.append("- Deny list: the edition boundary. Creator symbols must not appear in a PVR build (AGENTS.md, Editions; crates/strivo-web/e2e check-pvr-bundle.mjs). State it once here; todos cite it.")
    out.append("- Activity window and floor: the window is 4w because the 14-day window holds 9 commits, and one uncovered commit there is 0.11, so a fraction floor cannot resolve it. Measured over 4w: 2 of 109 commits (0.02) are uncovered by any open todo or landed history, and both are dispositioned under `## Unplanned activity`. The floor is 0.03, which tolerates one more uncovered commit (3 of 109 is 0.028) and fails at four.")
    out.append("")
    out.append(f"activity_floor: {floor if floor is not None else '0.03'}")
    out.append("activity_window: 4w")
    out.append("")
    out.append("## Migration notes\n")
    out.append("- Source: .omo/plans/strivo-review-and-pro-edition.md (104 legacy todos; 58 checked, 46 open), the ROADMAP.md 'Current position and next steps (2026-10-13)' section, and the revoy ledger block in ROADMAP.md (17 rows). The .omo plan and the revoy block are retired and deprecated in place; neither is deleted.")
    out.append("- Legacy todos 1-34, 103, 104 and 35-54, 57, 59 are landed: recorded as [x] under M1-C and M1-D with an ancestry-style qa, touches taken from the landing commit's paths.")
    out.append("- Legacy todo 98 (Wave 9b, real-server Creator journey) was moved from the side waves into M3-C, because todo 69 (Wave 9) depends on it and cross-milestone needs may only point backwards. The later item moved; the earlier one did not.")
    out.append("- Lanes are split by the path area the legacy References already separate: strivo-web assets and SPA, strivo-web server routes, src/ daemon, crates/editor, crates/research and dataviz, plugins and pipelines-dag, src/platform and licence, top-level tests, docs and CI, and other crates. A todo's lane is the area of its first touches entry. Landed todos sit in their own ledger lane (L7 in M1), recorded done. A needs edge was added only where two todos in different lanes have touches that overlap by keel's prefix rule and no needs path already orders them: 95 edges, 70 from an open todo to a landed one and 25 between open todos. Those 25 are listed by todo ID in the report, not in the plan.")
    out.append("- Each open todo's touches are the legacy References paths, remapped where the legacy shorthand no longer resolves (assets/spa/* and routes/* are under crates/strivo-web/; src/edl/* moved to crates/editor/; CLAUDE.md is now AGENTS.md), and reduced to directories as dir/**. Docs-only references are dropped from touches; where a todo had only docs references, its touches were chosen from the code its text names (DOCS_ONLY_TOUCHES in the generator; 13 todos, a reading of the legacy text, not verified against the code it names).")
    out.append("- Legacy commit lines were kept for open todos. For landed todos the commit line is the subject that actually landed; six legacy 'Commit:' lines name a different subject (todos 2, 3, 4, 5, 9, 37).")
    out.append("- Open todos' qa is a ran-guarded test-name filter (`cargo test --workspace <token>` or a Playwright spec path that does not exist yet), so each fails at HEAD and passes only when its work lands. Each token was checked against the 1659 test names at 6a2cd1c. Landed todos' qa is `git merge-base --is-ancestor <full commit hash> HEAD`, so a reword cannot break a done todo.")
    out.append("- Quiet is not cancelled: M3 to M6 have no commits in the window and are carried, deferred behind M1 and M2, with the owner's ratification of the legacy plan cited in M3's epoch.\n")

    for mid, title, epoch, lanes, phases, _ in MILESTONES:
        out.append(f"### M{mid} — {title}")
        out.append(f"epoch: {epoch}")
        out.append(f"lanes: {lanes}")
        out.append(f"gates: .keel/gates/M{mid}.json\n")
        for p in phases:
            out.append(f"#### M{mid}-{p['letter']} — {p['title']}")
            out.append("exit:")
            for e in p["exits"]:
                out.append(exit_block(e))
            out.append("")
            out.append("todos:")
            for t in p["todos"]:
                out.append(todo_block(t))
                out.append("")
        out.append("")
    out.append("")
    out.append("## Unplanned activity\n")
    out.append("Clusters of recent commits that no open todo's `touches` covers and that are not landed history. Each row names its disposition: `plan` (todos follow), `fold <todo>`, or `ignore <reason>`.\n")
    for row, disp in UNPLANNED:
        out.append(row + " | disposition: " + disp)
    out.append("")
    open(REPO + "/.keel/plan.md", "w", encoding="utf-8").write("\n".join(out))
    os.makedirs(REPO + "/.keel/gates", exist_ok=True)
    for mid, title, epoch, lanes, phases, _ in MILESTONES:
        json.dump(gates_for(mid, "milestone %d" % mid), open(REPO + f"/.keel/gates/M{mid}.json", "w"), indent=2, ensure_ascii=False)
        open(REPO + f"/.keel/gates/M{mid}.json", "a").write("\n")
    return n_ms, n_phase, n_todo, n_done, n_open

if __name__ == "__main__":
    print(write())
