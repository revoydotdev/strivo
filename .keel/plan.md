# strivo — Plan

## TL;DR (for humans)

StriVo is a self-hosted stream recorder with a browser UI, and its Creator Edition sits behind a cargo feature. The last fourteen days changed little: nine commits, mostly docs, CI and dependency bumps, plus one Creator pipeline fix. The plan therefore starts with the work already in flight (the pipeline run surface, the template library and cross-archive retrieval), then fixes the one failing test lane found on the way and the repo's own claims. Next come the seventeen open rows of the old revoy ledger. The publishing, analytics and transcript work from the legacy Creator plan is kept verbatim in later milestones, deferred behind the live work, because it has no commits in the window.

It will not add a remote studio, cloud rendering, a public plugin marketplace, a pricing or payment decision, or any test that needs a live platform account. The one act only the owner can do is creating the Cloudflare D1 database for the licence backend, and it is written as an `owner:` line.

## TL;DR (machine)

6 milestones, 19 phases, 129 todos (58 landed and recorded done from their commits, 71 open). Active milestone: M1. Lanes split by path area, 4 to 8 per milestone, and every open todo has a test-named qa that fails today. One owner act (2-A1). Measured over the four-week window: 109 commits, 2% uncovered (2 commits, both dispositioned); the 14-day window has 9 commits and is too small for a fraction. The browser mock lane fails the same 9 tests at HEAD in both of two runs (covered by todos 1-A4, 1-B4 and 1-B5); performance-navigation.spec.ts:9 failed in one run and passed in the other (todo 1-A5). Risks: the Creator publishing and analytics items are large and untested against live platforms.

## Scope

### Must have

**Phase 1 — Review, optimization, design conformance, and Pro groundwork**

- A re-verified findings register covering every prior audit item, with a per-item verdict and evidence. The three existing audits are 2.5–8 weeks stale and a substantial number of their items are already closed — todo 1 enumerates and derives the exact count rather than asserting one — so none may be acted on without a current-source re-check.
- Release integrity repaired: `CHANGELOG.md` gains the 0.8.0 section, `README.md`'s version claim is corrected, and `ROADMAP.md`'s revoy ledger loses its four obsolete todos and gains a re-derived one.
- A measured performance pass with a reproducible baseline harness, not a static read. Bundle size, the per-event refetch storm, database query shape, and cold-media concurrency each get a number and a decision.
- A UX/user-journey review of both editions driven against a real compiled binary, plus a DESIGN.md conformance audit and a WCAG 2.2 AA pass.
- Nine contracts Phase 2 cannot start without: feature-capability descriptor, canonical project/source/composition/variant/artifact identity, one time model, one render contract, one durable job/artifact lifecycle, the editor trust gap closed, the two DAGs reconciled, the entitlement set completed, and the six coexisting edit representations unified.

**Phase 2 — StriVo Pro**

- The full 76-item manifest from `docs/CREATOR-EDITION-REPORT-2026-09-07.md` — every ID, none dropped, with F4's verifier asserting the mapping — sequenced by its own stated critical path: project identity → precise time mapping → complete composition revision → common render plan → validated artifact → approved release package → publishing reconciliation → outcome mapping.
- A real multi-track timeline with frame-accurate editing, keyboard operation, and a browser-native preview that needs no server round-trip.
- A real node graph over the executable pipeline, where the graph the user draws *is* the run, with live status, logs, retry, cancel and artifact inspection.
- A project-centric information architecture replacing today's signpost panes, with one project holding many recordings and edits and one selected revision across every view.
- A truthful distribution loop: release packages, a publication state machine, one real connector verified without credentials, a calendar, and measured outcome ingestion that never labels a derived proxy as a measured metric.
- A commercial-readiness workstream: a deployable licence backend, a designed and tested licence-cache reconnection, a completed `PRO_PLUGINS` set, and the four release-hold preconditions tracked as work.

### Must NOT have (guardrails, anti-slop, scope boundaries)

- No remote guest / WebRTC studio recording. Already a recorded non-goal (`docs/STRATEGY-NVIVO-RIVERSIDE.md:95-110`, §5 "What 'Riverside' means here — and what it does not"); this plan does not re-litigate it.
- No cloud rendering, hosted collaboration, multi-tenant SaaS, or an outbound RTMP/SRT simulcast desk.
- No full colour-grading / VFX suite, and no attempt to become a general research/statistics platform unrelated to creator work.
- No public third-party plugin marketplace.
- No new SPA framework, bundler, or Node toolchain in the **release** pipeline. The release path stays `cargo build` plus **three pre-built third-party libraries**, vendored under `crates/strivo-web/assets/vendor/`, excluded from the PVR build. Each is loaded by a **per-library** mechanism — the plan's "one mechanism for all three" was wrong and is corrected here:

| Library | Artifact | Mechanism | Why |
|---|---|---|---|
| **Mediabunny** | `dist/bundles/mediabunny.min.mjs` (self-contained minified ESM, 684 785 B raw / 174 940 B gz) | `const { … } = await import("/assets/vendor/mediabunny.min.mjs")` — a true ES module | The only one of the three with native ESM exports. **Requires the `.mjs` MIME fix in todo 43.** |
| **AntV X6** | UMD build, **or** the ES build — decided by reading the shipped bundle, see below | `await import("/assets/vendor/x6.umd.js")` then read `globalThis.X6`; **or** `await import("/assets/vendor/x6.es.mjs")` and use the named export | A UMD bundle has **no ESM exports**, so `import { Graph } from "./x6.umd.js"` is a link-time `SyntaxError` ("does not provide an export named 'Graph'") — and because `spa.js` is one concatenated module, that link failure takes down **the whole Creator SPA**. A dynamic side-effect import is legal in ESM. **But see the UMD caveat below — the `this`-form wrapper makes this unviable, and the ES build is the fallback.** |
| **wavesurfer.js** | ESM build (`wavesurfer.esm.js`, or whatever the pinned major actually ships — the plan's earlier `min.js` reference is a v6-era filename) | `const { default: WaveSurfer } = await import("/assets/vendor/wavesurfer.esm.js")` | The UMD build has the same no-ESM-exports problem. |

The precedent is real and the plan verified it rather than assuming it: `assets/spa/001-creator.js:4-6` already does `import { mount as mountCodebook } from "./research/codebook.js"` against a separately-served file, and `crates/strivo-web/assets/spa.html:17` is `<script type="module" src="/assets/spa.js">`. Because `001-creator.js` is a `-creator.js` module, `build.rs:177` skips it entirely for PVR — which is exactly how the new imports get excluded.

**All three load via dynamic `import()`, not a static one.** Todo 31 re-measures the bundle after vendoring and the vendor must not sit in the initial Creator parse path; a static top-level import of ~685 KB raw of Mediabunny would defeat the lazy loading that makes the 250 KiB budget reachable at all.

**Two hard constraints the executor must respect.** (1) If any import is made **static**, it must land at a top-level boundary of the concatenated bundle; fragments split mid-file, so this is not automatic. `build.rs:96-102` already `assert!`s on `parsed.errors` in release, so a bad insertion **fails the build loudly** rather than shipping — rely on that, do not work around it. (2) **The UMD `globalThis` claim is unverified and is wrong for the common wrapper form.** Many UMD bundles end `(function(root, factory){ … })(this, …)`; at ES-module top level `this` is `undefined`, so `root.X6 = …` throws a `TypeError`. Bundles written against `typeof globalThis !== 'undefined' ? globalThis : this` do attach correctly. **So: for X6, the first implementation step is to read the shipped bundle's first line and determine which form it is.** If it is the `this` form, the side-effect import is not viable and the `@antv/x6` ES build must be vendored instead (todo 31 pins whichever is used). Do not assume — this is the one mechanism in the plan that is not settled by evidence.

**Test-only devDependencies in `crates/strivo-web/e2e/` are permitted** (todo 18 adds `@axe-core/playwright`); they never enter the release pipeline.
- No repository split of Creator Edition. ADR 0002 is Accepted and monorepo stays.
- No release, no pricing decision, no payment-provider selection.
- No acceptance criterion that requires a live Google, TikTok, or Meta account or credential.
- No AI or Claude attribution in commits, PRs, or code comments; `.github/workflows/no-ai-attribution.yml` enforces it.
- No renumbering or deletion of an existing audit-manifest ID. New findings append with fresh IDs; closed items record evidence and stay.
- No edits to ADRs 0001–0003. Supersede with a new ADR if a decision genuinely changes.
- No refactor of the PVR capture path for Pro's benefit. The PVR must remain excellent and independently valid (`ROADMAP.md:19-24`).


## Verification

- Test decision: TDD for every open todo; the qa names the test that must fail first. Landed todos are checked by commit ancestry (`git log --format=%s` contains the landing subject), not by re-running their old tests.
- Rust gate, as CI's `check` job runs it (`.github/workflows/ci.yml`): `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo clippy -p strivo-core -p strivo-web -p strivo-bin --features creator --all-targets --locked -- -D warnings`; `cargo test --workspace --release --all-targets --locked`; `cargo test --workspace --all-targets --locked`; `cargo test -p strivo-web -p strivo-bin --features creator --all-targets --locked`.
- Browser gate: `cd crates/strivo-web/e2e && npm test` (the mock lane; its pretest runs check-pvr-bundle.mjs). The real-server lane `npm run test:real` needs the release PVR binary and a sandboxed HOME.
- Licence backend: `cd licence-backend && npm run typecheck && npm test`.
- Local test environment: `.cargo/config.toml` (untracked, git-ignored) sets the target dir to /scratch/cargo-targets/strivo-main. CI scratches on real disk through $RUNNER_TEMP; run the suite with a short TMPDIR, because the socket tests fail under a long one (todo 1-A1).
- Resource rule carried from the legacy plan: a long soak or a disk-pressure rehearsal runs inside a memory-capped scope on a throwaway loopback filesystem with a declared size ceiling, never on the shared workstation's real disk and never against a live library (docs/PERFORMANCE.md:83).
- Deny list: the edition boundary. Creator symbols must not appear in a PVR build (AGENTS.md, Editions; crates/strivo-web/e2e check-pvr-bundle.mjs). State it once here; todos cite it.
- Activity window and floor: the window is 4w because the 14-day window holds 9 commits, and one uncovered commit there is 0.11, so a fraction floor cannot resolve it. Measured over 4w: 2 of 109 commits (0.02) are uncovered by any open todo or landed history, and both are dispositioned under `## Unplanned activity`. The floor is 0.03, which tolerates one more uncovered commit (3 of 109 is 0.028) and fails at four.

activity_floor: 0.03
activity_window: 4w

## Migration notes

- Source: .omo/plans/strivo-review-and-pro-edition.md (104 legacy todos; 58 checked, 46 open), the ROADMAP.md 'Current position and next steps (2026-10-13)' section, and the revoy ledger block in ROADMAP.md (17 rows). The .omo plan and the revoy block are retired and deprecated in place; neither is deleted.
- Legacy todos 1-34, 103, 104 and 35-54, 57, 59 are landed: recorded as [x] under M1-C and M1-D with an ancestry-style qa, touches taken from the landing commit's paths.
- Legacy todo 98 (Wave 9b, real-server Creator journey) was moved from the side waves into M3-C, because todo 69 (Wave 9) depends on it and cross-milestone needs may only point backwards. The later item moved; the earlier one did not.
- Lanes are split by the path area the legacy References already separate: strivo-web assets and SPA, strivo-web server routes, src/ daemon, crates/editor, crates/research and dataviz, plugins and pipelines-dag, src/platform and licence, top-level tests, docs and CI, and other crates. A todo's lane is the area of its first touches entry. Landed todos sit in their own ledger lane (L7 in M1), recorded done. A needs edge was added only where two todos in different lanes have touches that overlap by keel's prefix rule and no needs path already orders them: 95 edges, 70 from an open todo to a landed one and 25 between open todos. Those 25 are listed by todo ID in the report, not in the plan.
- Each open todo's touches are the legacy References paths, remapped where the legacy shorthand no longer resolves (assets/spa/* and routes/* are under crates/strivo-web/; src/edl/* moved to crates/editor/; CLAUDE.md is now AGENTS.md), and reduced to directories as dir/**. Docs-only references are dropped from touches; where a todo had only docs references, its touches were chosen from the code its text names (DOCS_ONLY_TOUCHES in the generator; 13 todos, a reading of the legacy text, not verified against the code it names).
- Legacy commit lines were kept for open todos. For landed todos the commit line is the subject that actually landed; six legacy 'Commit:' lines name a different subject (todos 2, 3, 4, 5, 9, 37).
- Open todos' qa is a ran-guarded test-name filter (`cargo test --workspace <token>` or a Playwright spec path that does not exist yet), so each fails at HEAD and passes only when its work lands. Each token was checked against the 1659 test names at 6a2cd1c. Landed todos' qa is `git merge-base --is-ancestor <full commit hash> HEAD`, so a reword cannot break a done todo.
- Quiet is not cancelled: M3 to M6 have no commits in the window and are carried, deferred behind M1 and M2, with the owner's ratification of the legacy plan cited in M3's epoch.

### M1 — Current position: finish the in-flight pipeline wave and make the repo's own claims true
epoch: The tree at HEAD passes the repo's own gates, the Wave 8 pipeline surface (live node state, template library, cross-archive retrieval) is complete, and the tracked docs state the real gate.
lanes: L1 (strivo-web assets, SPA and e2e), L2 (src/ daemon, pipeline and recording), L3 (crates/research and crates/dataviz), L4 (crates/strivo-plugins and crates/pipelines-dag), L5 (top-level tests/), L6 (docs, tracked top-level docs, CI and packaging), L7 (landed work, recorded done in the ledger)
gates: .keel/gates/M1.json

#### M1-A — Gate repair and claims that must hold at HEAD
exit:
- [ ] The CI check job's test lanes pass at the declared HEAD | proof: `cargo test --workspace --all-targets --locked`
- [ ] CONTRIBUTING.md states the gates CI runs, and CI runs no gate it omits | verify: auditor

todos:
- [ ] 1-A1 — Keep the ipc_handshake socket tests independent of TMPDIR length
  lane: L5
  needs: -
  touches: tests/ipc_handshake.rs
  do: The three socket tests bind a Unix socket under the tempfile directory. Under a TMPDIR longer than about 107 bytes Listener::bind fails with 'path must be shorter than SUN_LEN' (gate run at 6a2cd1c with TMPDIR under the scratchpad: release lane, 3 failed of 4). Put each socket in a short directory the test owns, so the tests pass under any TMPDIR. The qa below is the failing-first check: it must fail on HEAD before the fix.
  must-not: Do not weaken the bind or its SUN_LEN check in the listener, and do not skip the three tests. The production socket path (src/ipc.rs:242, state_dir) has the same limit; it is carried as a finding in M2-C.
  qa: `d="$(mktemp -d)/$(printf 'x%.0s' $(seq 1 100))" && mkdir -p "$d" && TMPDIR="$d" cargo test --locked --test ipc_handshake 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(tests): keep the ipc socket tests under any TMPDIR length

- [ ] 1-A2 — State in CONTRIBUTING.md the gates CI actually runs
  lane: L6
  needs: -
  touches: CONTRIBUTING.md
  do: CONTRIBUTING.md 'Before opening a pull request' lists fmt, clippy and cargo test --workspace. CI's check job (.github/workflows/ci.yml) also runs the release and debug test lanes, the Creator-feature clippy and tests, the licence-backend typecheck and tests, and the Playwright suite in crates/strivo-web/e2e. List them so a red CI run can be reproduced locally.
  must-not: Do not change the CI workflow to match the document.
  qa: `git grep -q -- '--features creator' CONTRIBUTING.md && git grep -q 'npm test' CONTRIBUTING.md`
  commit: docs(contributing): list the gates CI actually runs

- [ ] 1-A3 — Make ROADMAP's gate claim match what CI runs
  lane: L6
  needs: 1-C2
  touches: ROADMAP.md
  do: ROADMAP.md:150-151 says the gate (cargo fmt, clippy, cargo test --workspace) 'is green on main'. The lint premise of the earlier version of this todo already held at HEAD (the CHANGELOG action pins match the workflows, and the packaging lints exit 0), so its qa passed vacuously and it is replaced. The claim is incomplete: CI's browser lanes (ci.yml:239, ci.yml:258) are part of the same gate and the mock lane fails 9 tests at HEAD. Rewrite the sentence to name the gate CI runs, and say which lane is red until 1-A4, 1-B4 and 1-B5 land.
  must-not: Do not claim the gate is green while a browser lane CI runs is red.
  qa: `! git grep -q 'is green on' -- ROADMAP.md`
  commit: docs(roadmap): state the gate CI runs and which lane is red

- [ ] 1-A4 — Stop the composer from disturbing an untouched tile's player
  lane: L1
  needs: 1-C6, 1-C7, 1-C8, 1-C11, 1-C15, 1-C16, 1-C17, 1-C18, 1-C29, 1-C31, 1-C32, 1-D6, 1-D9, 1-D17, 1-D18, 1-D19
  touches: crates/strivo-web/assets/spa/**, crates/strivo-web/e2e/tests/**
  do: The browser mock lane fails crates/strivo-web/e2e/tests/player-controller.spec.ts:336 at 6a2cd1c, in both of two runs: 'composer edits leave an untouched tile's player alone' (the fake player's destroyed log contains the playing id). Find which composer edit reaches the untouched tile and stop it.
  must-not: Do not weaken the assertion that the untouched player is not destroyed.
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/player-controller.spec.ts -g 'composer edits leave'`
  commit: fix(web): keep an untouched multiview tile's player when the composer edits

- [ ] 1-A5 — Pin down the late-recordings race in performance-navigation.spec.ts or prove it flaky
  lane: L1
  needs: 1-C7, 1-C11, 1-C15, 1-C16, 1-C18, 1-C29, 1-C32, 1-D6, 1-D9, 1-D17, 1-D18, 1-D19
  touches: crates/strivo-web/e2e/tests/**
  do: In the first of two runs at 6a2cd1c, crates/strivo-web/e2e/tests/performance-navigation.spec.ts:9 ('a late recordings response cannot replace a newer Settings route') failed; in the second run it passed. Repeat the spec until the failure either reproduces or the repetition count shows it does not. If it reproduces, fix the route guard in the SPA.
  must-not: Do not add a retry to make it pass.
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/performance-navigation.spec.ts --repeat-each=10`
  commit: test(web): repeat the late-response route spec until its race is settled

#### M1-B — Wave 8 in flight: live run graph, template library, cross-archive retrieval
exit:
- [ ] The Wave 8 surfaces pass the workspace suite and the browser suite | proof: `cd crates/strivo-web/e2e && npm test`
- [ ] Each run node shows live state, a retry re-arms its branch, and a lock timeout names the holder | verify: auditor

todos:
- [ ] 1-B1 — A live run graph with per-node status, logs, retry, cancel and artifacts
  lane: L2
  needs: 1-D18, 1-D19, 1-D20, 1-A4, 1-A5, 1-B4, 1-B5, 1-D5
  touches: src/pipeline/executor.rs, src/pipeline/runtime.rs, src/pipeline/stage.rs, crates/strivo-web/assets/spa/013-creator.js, crates/strivo-web/assets/spa/036-pvr.js, crates/strivo-web/e2e/**
  do: The run inspector replaces today's page-refetching list. Each node shows its live `StageState`, attempts, typed failure class, resource lock held, bounded log tail, and produced artifacts, updating from the `PipelineUpdated` stream by patching the affected node — building directly on todo 11's fix. Retry re-arms a failed branch including siblings; cancel cascades and is idempotent, per `src/pipeline/executor.rs:452-500,567`. Acceptance (legacy): (agent-executable): a Playwright test drives a run and asserts each node transitions `Pending → Running → Done` without a full-page refetch; a retry test asserts a failed branch and its siblings re-arm; a lock-timeout test asserts the UI names the holder. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT expose a wedged stage as merely slow; a lock-acquisition timeout must surface as a typed failure naming the holder, which `executor.rs:642`'s `find_lock_holder` already supports
  qa: `cargo test --locked --workspace live_run_graph 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(pipelines): show live per-node status, logs, retry, cancel and artifacts

- [ ] 1-B2 — A template library derived from authored graphs
  lane: L4
  needs: 1-D18, 1-D19, 1-D20, 1-B1, 1-C4, 1-C5, 1-C19, 1-C34, 1-D10, 1-D11, 1-D13, 1-D16, 1-D22
  touches: crates/pipelines-dag/src/lib.rs, crates/strivo-web/src/routes/api.rs, src/pipeline/stage.rs, crates/strivo-plugins/src/pipeline_templates.rs
  do: Turn saved graphs into a browsable, forkable template library with a stated purpose, the capabilities each requires, and its measured cost estimate (`Stage.cost_cents_estimate` already exists). Decision: migrate the existing `RecipeChain` recipe files into this model; do not delete them. They are a working, persisted asset with a live CRUD surface (`crates/strivo-web/src/routes/api.rs:395-462`), and the migration is mechanical — `RecipeTrigger`/`RecipeStep`/`RecipeGate` (`pipelines-dag/src/lib.rs:500-543`) map onto `Stage.kind`, `StageDispatch` and a new `Stage.gate`. Port the existing files, then remove the `RecipeChain` type and its four routes so exactly one authoring system remains. Note that `RecipeChain::validate` at `:560` only rejects empty fields and its own comment (`:557-559`) claims stricter checks "live in the host scheduler", which does not exist — the migration is what makes that comment true. Acceptance (legacy): (agent-executable): the library lists the imported `creator_intelligence` and `creator_publish` templates; every pre-existing `RecipeChain` file round-trips into the new model with its trigger, steps and gates intact; forking a template produces an independent copy; a template requiring an absent executor is labelled not-runnable; no `RecipeChain` type or route remains. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT present a template as runnable when a required executor is absent
  qa: `cargo test --locked --workspace template_library 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(pipelines): build a forkable template library and retire recipe chains

- [ ] 1-B3 — Cross-archive retrieval that lands a moment in the active project
  lane: L3
  needs: 1-C34, 1-D4, 1-D9, 1-D18, 1-D21, 1-A4, 1-B4, 1-B5, 1-D15, 1-D22
  touches: crates/research/src/search.rs, crates/research/src/moments.rs, crates/strivo-web/assets/spa/017-creator.js
  do: CE-P5, and the Query Lab payoff `docs/STRATEGY-NVIVO-RIVERSIDE.md:152-155` names: a hybrid lexical, semantic and temporal query whose hits are renderable — "every moment I talked about the speedrun AND chat density spiked". A query returns playable, provenance-linked intervals across recordings and adds selected moments to the active project without reimport. Builds on the existing FTS5 search at `crates/research/src/search.rs` and the archive surface at `assets/spa/017-creator.js`. Acceptance (legacy): (agent-executable): a cross-recording query returns intervals that each resolve to a source and time range; a selected hit opens at that timecode in the editor; adding it to the active project needs no reimport; an aggregate over a 10 000-hour library is computed server-side within the 500 ms uncached budget. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT round-trip an entire archive's transcripts through the browser to compute an aggregate
  qa: `cargo test --locked --workspace archive_retrieval 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(research): retrieve moments across the archive into the active project

- [ ] 1-B4 — Make the Creator pipelines surface pass the keyboard, axe and UX-budget gates
  lane: L1
  needs: 1-C6, 1-C7, 1-C8, 1-C11, 1-C15, 1-C16, 1-C17, 1-C18, 1-C29, 1-C31, 1-C32, 1-D6, 1-D9, 1-D17, 1-D18, 1-D19
  touches: crates/strivo-web/assets/spa/**, crates/strivo-web/assets/spa-css/**, crates/strivo-web/e2e/tests/**
  do: Deterministic failures at 6a2cd1c on the Creator #/pipelines route, in both of two mock-lane runs: keyboard Tab reaches 70 of 73 focusable controls (a11y-keyboard.spec.ts:131), axe reports critical or serious WCAG 2.2 AA violations (a11y-scan.spec.ts:153), the hit-target floor of 28px is missed (ux-budget.spec.ts:196), and text under 12px outside .micro is present (ux-budget.spec.ts:210). The route is the Wave 8 node graph (todos 52-54), so these belong to the in-flight work.
  must-not: Do not lower the WCAG floor, the 28px target or the 12px text floor in the tests to pass.
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/a11y-keyboard.spec.ts tests/a11y-scan.spec.ts tests/ux-budget.spec.ts -g 'Creator #/pipelines'`
  commit: fix(pipelines): make the node graph reachable by keyboard and inside the UX budget

- [ ] 1-B5 — Make the Creator plugins, archive and dataviz routes pass the UX-budget gates
  lane: L1
  needs: 1-C6, 1-C7, 1-C8, 1-C11, 1-C17, 1-C18, 1-C29, 1-C31, 1-D6, 1-D9, 1-D17, 1-D18, 1-D19
  touches: crates/strivo-web/assets/spa-css/**, crates/strivo-web/assets/spa/**
  do: Deterministic failures at 6a2cd1c in both mock-lane runs: Creator #/plugins has a hit target under 28px (ux-budget.spec.ts:196) and text under 12px (:210); Creator #/archive and #/dataviz have text under 12px outside .micro (:210). Fix the offending rules in the design tokens, not per route.
  must-not: Do not deviate from DESIGN.md without the owner's approval (AGENTS.md, Design system).
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/ux-budget.spec.ts -g 'Creator #/(plugins|archive|dataviz)'`
  commit: style(web): bring the plugins, archive and dataviz routes inside the UX budget

#### M1-C — Landed legacy todos 1-34, 103, 104 (ledger record of work already on main)
exit:
- [ ] Every landed commit named by these todos is an ancestor of HEAD | verify: auditor

todos:
- [x] 1-C1 — Re-verify every prior audit finding against current source and publish one consolidated register
  lane: L7
  needs: -
  touches: docs/audits/**
  do: Landed on main as 38e80f5: docs(audit): consolidate and re-verify all prior findings against current source. Legacy todo 1 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 38e80f5ef4b0fdf77f65572394668ddaec708caa HEAD`
  commit: docs(audit): consolidate and re-verify all prior findings against current source

- [x] 1-C2 — Repair release integrity across CHANGELOG, README, ROADMAP and the revoy ledger
  lane: L7
  needs: 1-C1
  touches: .github/workflows/ci.yml, CHANGELOG.md, Cargo.lock, README.md, ROADMAP.md, scripts/check-version-agreement.sh
  do: Landed on main as c631486: fix(docs): reconcile version, changelog and roadmap ledger with 0.8.0. Legacy todo 2 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c631486e4e0d22360b047f2cad3489b3b3e08868 HEAD`
  commit: fix(docs): reconcile version, changelog and roadmap ledger with 0.8.0

- [x] 1-C3 — Fix the crate-README generator and correct the stale per-crate documentation it produces
  lane: L7
  needs: 1-C1
  touches: crates/ab-render/README.md, crates/automation/README.md, crates/beat-detect/README.md, crates/branding/README.md, crates/brandsafe/README.md, crates/broll/README.md, crates/captions/README.md, crates/casebook/README.md, crates/chapters/README.md, crates/chat-density/README.md, crates/chat/README.md, crates/clipper/README.md, crates/cuepoints/README.md, crates/dataviz/README.md, crates/deadair/README.md, crates/editor/README.md, crates/heatmap/README.md, crates/insert-fx/README.md, crates/insights-compare/README.md, crates/loudness/README.md, crates/marketplace/README.md, crates/multistream/README.md, crates/multitrack/README.md, crates/pipelines-dag/README.md, crates/pitch/README.md, crates/research/README.md, crates/reuse/README.md, crates/scenes/README.md, crates/schedule-optimizer/README.md, crates/sidechain/README.md, crates/strivo-bin/README.md, crates/structure/README.md, crates/submix/README.md, crates/thumbnails/README.md, crates/vad/README.md, crates/viewguard-trend/README.md, scripts/gen-plugin-readmes.sh
  do: Landed on main as 4324ded: fix(docs): correct generated crate README claims. Legacy todo 3 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4324dedf57668e71cd13e50a4a56bdae6fcb1d7c HEAD`
  commit: fix(docs): correct generated crate README claims

- [x] 1-C4 — Correct stale module-level documentation that names deleted systems
  lane: L7
  needs: 1-C1
  touches: crates/strivo-web/README.md, crates/strivo-web/src/**, licence-backend/README.md, src/edl/mod.rs, src/edl/recorder.rs
  do: Landed on main as beefa8e: fix(docs): correct stale module docs naming the removed TUI. Legacy todo 4 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor beefa8e90aeddc126b70075c19df680743d514d1 HEAD`
  commit: fix(docs): correct stale module docs naming the removed TUI

- [x] 1-C5 — Delete the one genuine stub in the workspace: `SearchMode` and its `semantic-search` feature
  lane: L7
  needs: 1-C1
  touches: crates/strivo-plugins/Cargo.toml, crates/strivo-plugins/src/**
  do: Landed on main as 1cc5ef1: refactor(crunchr): drop orphaned SearchMode and empty feature. Legacy todo 5 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 1cc5ef13e2aba5a02882df60301f8aedc9784fe8 HEAD`
  commit: refactor(crunchr): drop orphaned SearchMode and empty feature

- [x] 1-C6 — Reconcile the `pipelines_dag` blueprint surface with real run state, or retire it honestly
  lane: L7
  needs: 1-C1
  touches: crates/pipelines-dag/src/lib.rs, crates/strivo-bin/src/main.rs, crates/strivo-plugins/src/artifacts.rs, crates/strivo-plugins/src/**, crates/strivo-plugins/src/lib.rs, crates/strivo-web/assets/**, crates/strivo-web/src/**, crates/strivo-web/tests/routes.rs
  do: Landed on main as ede68c4: fix(pipelines): report blueprint runnability from the executor capability registry. Legacy todo 6 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor ede68c4cc2f436a476b22f4f3dbf03876c2ff5ac HEAD`
  commit: fix(pipelines): report blueprint runnability from the executor capability registry

- [x] 1-C7 — Close the client/server contract gaps the SPA currently hides
  lane: L7
  needs: 1-C1, 1-C20, 1-C27
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/e2e/**, crates/strivo-web/src/**, scripts/check-client-routes.sh
  do: Landed on main as b536942: fix(web): repair the transcript contract and gate the dataviz route. Legacy todo 7 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b53694228a37db2b363e9a32f9511b6847ec5219 HEAD`
  commit: fix(web): repair the transcript contract and gate the dataviz route

- [x] 1-C8 — Remove inert markup and reconcile the design-token layer
  lane: L7
  needs: 1-C1
  touches: crates/strivo-web/assets/**, crates/strivo-web/assets/spa.html, scripts/check-css-tokens.mjs
  do: Landed on main as a89ef05: fix(css): remove inert theme attribute and close undefined design tokens. Legacy todo 8 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor a89ef053a3ad65e0e85440510dd3ea542e4077c5 HEAD`
  commit: fix(css): remove inert theme attribute and close undefined design tokens

- [x] 1-C9 — Build a reproducible performance baseline harness
  lane: L7
  needs: 1-C1
  touches: docs/PERFORMANCE.md, docs/perf/README.md, docs/perf/library-baseline.json, scripts/bench_library.sh, scripts/bench_probe.mjs, scripts/seed_bench_library.py
  do: Landed on main as bfdecef: build(perf): add a reproducible baseline harness with a captured baseline. Legacy todo 9 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor bfdecef273ab8b5d43efffbcce1c72b0f92d7b09 HEAD`
  commit: build(perf): add a reproducible baseline harness with a captured baseline

- [x] 1-C10 — Measure the shipped bundle and decide whether to split it
  lane: L7
  needs: 1-C9
  touches: .github/workflows/ci.yml, docs/PERFORMANCE.md, scripts/check-bundle-size.sh, scripts/measure_bundle.sh
  do: Landed on main as c0afcf7: perf(web): measure the shipped bundle and record the split decision. Legacy todo 10 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c0afcf70a93f48ec48f88884def7973d9d21d8e4 HEAD`
  commit: perf(web): measure the shipped bundle and record the split decision

- [x] 1-C11 — Fix the pipeline run view's full-page refetch on every stage transition
  lane: L7
  needs: 1-C9
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/**
  do: Landed on main as fc87c1d: perf(web): patch the pipeline run view instead of refetching it per event. Legacy todo 11 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor fc87c1d26e8fdc23ae60ed8ef1ccdd9ef743571c HEAD`
  commit: perf(web): patch the pipeline run view instead of refetching it per event

- [x] 1-C12 — Measure and decide the recordings/history query shape
  lane: L7
  needs: 1-C9
  touches: docs/PERFORMANCE.md
  do: Landed on main as c17244e: perf(db): measure the recordings query shape and record the decision. Legacy todo 12 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c17244e88153dd79181eb39631500ca68b0c3d85 HEAD`
  commit: perf(db): measure the recordings query shape and record the decision

- [x] 1-C13 — Bound, measure and document cold-media work
  lane: L7
  needs: 1-C9
  touches: crates/strivo-web/tests/media_pool.rs, docs/PERFORMANCE.md, scripts/bench_media_pool.sh, scripts/check_durable_archive.sh
  do: Landed on main as 4f39404: perf(media): verify and bound cold thumbnail, probe and remux work. Legacy todo 13 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4f39404270ebb7ff19f29e4ca57d1c6b04765267 HEAD`
  commit: perf(media): verify and bound cold thumbnail, probe and remux work

- [x] 1-C14 — Confirm no blocking subprocess call remains in an async handler
  lane: L7
  needs: 1-C9
  touches: scripts/ast-grep/std-command-in-async-fn.yml, scripts/ast-grep/std-command-on-reactor.yml, scripts/check-no-blocking-command.sh
  do: Landed on main as 0623a0a: perf(concurrency): confirm no blocking subprocess call survives in an async handler. Legacy todo 14 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 0623a0a1fb3b379eff1b5ae6a2b13e7fdfa8988b HEAD`
  commit: perf(concurrency): confirm no blocking subprocess call survives in an async handler

- [x] 1-C15 — Audit the PVR user journey against a real compiled binary
  lane: L7
  needs: 1-C1, 1-C9
  touches: crates/strivo-web/e2e/**, docs/audits/**
  do: Landed on main as 1053082: test(web): audit the PVR journey against the real release binary. Legacy todo 15 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 10530825690089d457350576f4e1f96d27334083 HEAD`
  commit: test(web): audit the PVR journey against the real release binary

- [x] 1-C16 — Audit the Pro user journey against a real Creator-featured binary
  lane: L7
  needs: 1-C1
  touches: crates/strivo-web/e2e/playwright.creator.config.ts, crates/strivo-web/e2e/real-server.sh, crates/strivo-web/e2e/**, docs/audits/**
  do: Landed on main as 9171dd0: test(web): audit the Pro journey against a real Creator-featured binary. Legacy todo 16 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 9171dd0b7e895673e8b49a56fa8ec2c93f66087e HEAD`
  commit: test(web): audit the Pro journey against a real Creator-featured binary

- [x] 1-C17 — Audit DESIGN.md conformance and fix the deviations
  lane: L7
  needs: 1-C15
  touches: crates/strivo-web/assets/**, scripts/check-css-tokens.mjs, scripts/check-design-tokens.mjs
  do: Landed on main as 4ea9f18: fix(design): conform the SPA to the DESIGN.md token table. Legacy todo 17 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4ea9f18c08d08f8683e0800902322ca159780769 HEAD`
  commit: fix(design): conform the SPA to the DESIGN.md token table

- [x] 1-C18 — Run a WCAG 2.2 AA accessibility pass over both editions
  lane: L7
  needs: 1-C15, 1-C17
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/package-lock.json, crates/strivo-web/e2e/package.json, crates/strivo-web/e2e/**
  do: Landed on main as 4b33a3a: fix(a11y): enforce WCAG 2.2 AA checks and close the remaining violations. Legacy todo 18 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4b33a3a830d83bd1d7daf11708494561d33729a3 HEAD`
  commit: fix(a11y): enforce WCAG 2.2 AA checks and close the remaining violations

- [x] 1-C19 — Make the settings-coverage audit self-maintaining
  lane: L7
  needs: 1-C15
  touches: .github/workflows/ci.yml, crates/strivo-web/src/**, crates/strivo-web/tests/routes.rs, docs/SETTINGS-COVERAGE.md, scripts/gen-settings-coverage.sh
  do: Landed on main as 7a6240f: build(docs): generate settings coverage from the enforced allowlist. Legacy todo 19 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 7a6240f3e655080714622b459a0ba8a20a6ffe3b HEAD`
  commit: build(docs): generate settings coverage from the enforced allowlist

- [x] 1-C20 — One typed feature-capability descriptor driving server and SPA
  lane: L7
  needs: 1-C1, 1-C9
  touches: crates/strivo-web/assets/**, crates/strivo-web/src/capabilities.rs, crates/strivo-web/src/lib.rs, crates/strivo-web/src/**, crates/strivo-web/tests/routes.rs, scripts/ast-grep/capability-key-variant.yml, scripts/ast-grep/gate-pro-string-literal.yml, scripts/check-capability-descriptor.sh
  do: Landed on main as eeadbc7: feat(web): add a single feature-capability descriptor for both editions. Legacy todo 20 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor eeadbc7c2b88d0cd41aedef8ab467433ad53dce4 HEAD`
  commit: feat(web): add a single feature-capability descriptor for both editions

- [x] 1-C21 — Canonical creator project, source, composition, variant and artifact identity
  lane: L7
  needs: 1-C20
  touches: crates/research/src/**, crates/research/src/lib.rs, crates/strivo-web/src/creator_schema.rs, crates/strivo-web/src/lib.rs, crates/strivo-web/src/server.rs
  do: Landed on main as 8e0b4b3: feat(research): add the canonical creator project and composition schema. Legacy todo 21 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 8e0b4b3a458ced4a4e2461ab82a09096bf389d9b HEAD`
  commit: feat(research): add the canonical creator project and composition schema

- [x] 1-C22 — One time model: integer media timestamps, one interval map, one ClockMap
  lane: L7
  needs: 1-C20, 1-C21
  touches: Cargo.lock, crates/chat-density/Cargo.toml, crates/chat-density/src/lib.rs, crates/chat-density/src/tests.rs, crates/editor/src/lib.rs, crates/editor/src/**, crates/research/Cargo.toml, crates/research/src/**, crates/strivo-web/src/**
  do: Landed on main as efa08bc: feat(editor): introduce the canonical time model and interval map. Legacy todo 22 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor efa08bca53368354473a3ee57a799917e4bca9f6 HEAD`
  commit: feat(editor): introduce the canonical time model and interval map

- [x] 1-C23 — One render contract: a revision-to-RenderPlan compiler shared by preview and export
  lane: L7
  needs: 1-C20, 1-C21, 1-C22, 1-C24, 1-C25
  touches: crates/editor/src/lib.rs, crates/editor/src/**, crates/pitch/src/lib.rs, crates/research/src/**, crates/strivo-plugins/src/**, crates/strivo-web/src/lib.rs, crates/strivo-web/src/render.rs, crates/strivo-web/src/**, crates/strivo-web/tests/render_effects.rs, crates/strivo-web/tests/render_plan.rs, crates/submix/src/lib.rs
  do: Landed on main as 61d4864: fix(editor): compile one render plan and make every exposed control reach the artifact. Legacy todo 23 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 61d4864f6c85ac9fb5462901f102cdccbef133d2 HEAD`
  commit: fix(editor): compile one render plan and make every exposed control reach the artifact

- [x] 1-C24 — One durable job and artifact lifecycle for creator operations
  lane: L7
  needs: 1-C20, 1-C21, 1-C22
  touches: crates/research/src/**, crates/strivo-web/src/creator_schema.rs
  do: Landed on main as b5c882f: feat(pipeline): give creator operations a durable job and artifact lifecycle. Legacy todo 24 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b5c882fca539d939a7070894db418757e4eb65ac HEAD`
  commit: feat(pipeline): give creator operations a durable job and artifact lifecycle

- [x] 1-C25 — Validate media inputs at the execution boundary
  lane: L7
  needs: 1-C21, 1-C22
  touches: crates/strivo-web/src/**, crates/strivo-web/tests/**, crates/strivo-web/tests/media_input.rs, crates/strivo-web/tests/media_input_spawns.rs
  do: Landed on main as 8ab871e: fix(security): validate media inputs at the render execution boundary. Legacy todo 25 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 8ab871ecd049cf0b5b45ef6ae2ff902b6f6686ac HEAD`
  commit: fix(security): validate media inputs at the render execution boundary

- [x] 1-C26 — Assert in `build.rs` that SPA module sequences are unique and gap-free
  lane: L7
  needs: 1-C20, 1-C23
  touches: crates/strivo-web/build.rs, crates/strivo-web/build/sequence.rs, crates/strivo-web/tests/module_sequence.rs
  do: Landed on main as fd31a5d: build(web): assert SPA module sequences are unique and gap-free. Legacy todo 26 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor fd31a5d135b51da8e7457f39f6aa6d94682fbaa7 HEAD`
  commit: build(web): assert SPA module sequences are unique and gap-free

- [x] 1-C27 — Complete the entitlement set and design the licence-cache reconnection
  lane: L7
  needs: 1-C1
  touches: config.toml.example, crates/strivo-web/tests/licence_entitlement.rs, src/config/mod.rs, src/daemon.rs, src/licence/cache.rs, src/licence/client.rs, src/licence/gate.rs, src/licence/mod.rs, src/licence/reconnect.rs, src/licence/verify.rs
  do: Landed on main as 60a1260: fix(licence): complete the Pro entitlement set and gate the cache reconnection. Legacy todo 27 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 60a1260f8ac2d0b4ecf3d973d3f8dce0722f0c3b HEAD`
  commit: fix(licence): complete the Pro entitlement set and gate the cache reconnection

- [x] 1-C28 — Unify the six coexisting edit representations behind one project revision
  lane: L7
  needs: 1-C21, 1-C24
  touches: crates/editor/src/store.rs, crates/editor/tests/one_source_of_truth.rs, crates/strivo-web/src/**, crates/strivo-web/tests/editor_revision_unification.rs, crates/strivo-web/tests/media_input.rs, crates/strivo-web/tests/media_input_spawns.rs, docs/CREATOR-EDITION-REPORT-2026-09-07.md, src/edl/mod.rs, src/edl/render.rs, src/edl/schema.rs, src/lib.rs
  do: Landed on main as 9b654a5: refactor(editor): unify all edit representations into one project revision. Legacy todo 28 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 9b654a56bdb9f4fdfbd917e7574fe3dbcf1ff11c HEAD`
  commit: refactor(editor): unify all edit representations into one project revision

- [x] 1-C29 — Fix the DataViz transcript contract end to end
  lane: L7
  needs: 1-C7
  touches: crates/dataviz/src/lib.rs, crates/strivo-web/assets/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/e2e/**, crates/strivo-web/src/**, crates/strivo-web/tests/routes.rs
  do: Landed on main as 3c1e8a2: fix(dataviz): serve the real transcript contract with distinguishable failure states. Legacy todo 29 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 3c1e8a2d37b74e72b9be828ed4d82ee4fa3e6450 HEAD`
  commit: fix(dataviz): serve the real transcript contract with distinguishable failure states

- [x] 1-C30 — Isolate every render attempt: unique scratch, immutable output, validated result
  lane: L7
  needs: 1-C9, 1-C23, 1-C24
  touches: crates/editor/src/lib.rs, crates/editor/src/**, crates/strivo-web/src/render.rs, crates/strivo-web/src/**, crates/strivo-web/tests/media_input.rs, crates/strivo-web/tests/render_effects.rs, crates/strivo-web/tests/render_plan.rs
  do: Landed on main as 5aa29f0: fix(editor): isolate render scratch and make outputs immutable per attempt. Legacy todo 30 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 5aa29f0360289048e13e2fe3efad6dd19f4c7fcf HEAD`
  commit: fix(editor): isolate render scratch and make outputs immutable per attempt

- [x] 1-C31 — Vendor the three browser libraries, exclude them from the PVR build, and re-measure the bundle
  lane: L7
  needs: 1-C1
  touches: README.md, crates/strivo-web/Cargo.toml, crates/strivo-web/assets/**, crates/strivo-web/build.rs, crates/strivo-web/src/**, crates/strivo-web/tests/routes.rs, crates/strivo-web/tests/vendor_provenance.rs, docs/PERFORMANCE.md
  do: Landed on main as 588981c: build(web): vendor the browser libraries and keep them out of the PVR binary. Legacy todo 31 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 588981cf733983fb767627180ee521bb9d0e8815 HEAD`
  commit: build(web): vendor the browser libraries and keep them out of the PVR binary

- [x] 1-C32 — Build a representative, redistributable media fixture corpus
  lane: L7
  needs: 1-C9
  touches: crates/strivo-web/e2e/**, fixtures/media/MANIFEST.json, fixtures/media/README.md, fixtures/media/discontinuities.mkv, fixtures/media/fixtures.mjs, fixtures/media/landmarks.mkv, fixtures/media/long-gop.mkv, fixtures/media/mixed-codecs.mkv, fixtures/media/multi-audio.mkv, fixtures/media/silent-video.mkv, "fixtures/media/**, \343\202\273\343\203\203\343\202\267\343\203\247\343\203\263, \360\237\216\254, 2026-09-26.mkv", fixtures/media/variable-frame-rate.mkv, scripts/seed_media_fixtures.py, scripts/verify_media_fixtures.py
  do: Landed on main as ed65cc4: test(media): add a representative redistributable media fixture corpus. Legacy todo 32 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor ed65cc4b4d41ed860ffd1086f5d462803d03a989 HEAD`
  commit: test(media): add a representative redistributable media fixture corpus

- [x] 1-C33 — Local file and folder import with manifests, dedup and relinking
  lane: L7
  needs: 1-C20, 1-C21
  touches: crates/research/src/**, crates/strivo-web/src/**, crates/strivo-web/tests/import_boundary.rs
  do: Landed on main as c4d35e4: feat(import): bring local recordings into the same project as captured streams. Legacy todo 33 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c4d35e442f69605508ef95e8e1cb3febab461cfb HEAD`
  commit: feat(import): bring local recordings into the same project as captured streams

- [x] 1-C34 — OBS multitrack import with track identification and synchronisation
  lane: L7
  needs: 1-C22, 1-C33
  touches: Cargo.lock, crates/multitrack/src/lib.rs, crates/research/Cargo.toml, crates/research/src/**, crates/strivo-web/src/**, crates/strivo-web/tests/multitrack_boundary.rs
  do: Landed on main as 18fa37b: feat(import): import and preserve OBS-style multitrack recordings. Legacy todo 34 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 18fa37bd7a3e6a28488bde3aea19f02b2a68e9f2 HEAD`
  commit: feat(import): import and preserve OBS-style multitrack recordings

- [x] 1-C35 — Convert the agent operating policy from `CLAUDE.md` to a tool-agnostic `AGENTS.md`
  lane: L7
  needs: 1-C1
  touches: .gitignore, config.toml.example, scripts/check-agent-policy-rules.sh, scripts/check-agent-policy.sh
  do: Landed on main as c182310: docs(repo): convert the agent operating policy to a tool-agnostic gitignored AGENTS.md. Legacy todo 103 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c182310cb9aef4abf3cc79e234f021191c038802 HEAD`
  commit: docs(repo): convert the agent operating policy to a tool-agnostic gitignored AGENTS.md

- [x] 1-C36 — Implement `scripts/check-plan-deps.mjs` to the contract and regenerate every derived view
  lane: L7
  needs: -
  touches: scripts/check-plan-deps.mjs
  do: Landed on main as a8d4f2b: build(plan): add check-plan-deps and regenerate all derived dependency views. Legacy todo 104 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor a8d4f2b535eca7a6fef93713557727ad568f412a HEAD`
  commit: build(plan): add check-plan-deps and regenerate all derived dependency views

#### M1-D — Landed legacy todos 35-54, 57, 59 (ledger record of work already on main)
exit:
- [ ] Every landed commit named by these todos is an ancestor of HEAD | verify: auditor

todos:
- [x] 1-D1 — Daemon-owned durable chat acquisition and replay
  lane: L7
  needs: 1-C22, 1-C33, 1-D2
  touches: crates/chat/src/acquire.rs, crates/chat/src/lib.rs, crates/research/src/**, crates/strivo-web/src/chat_capture.rs, crates/strivo-web/src/lib.rs, docs/STRATEGY-NVIVO-RIVERSIDE.md
  do: Landed on main as a302eb0: feat(chat): acquire and replay chat durably in the daemon. Legacy todo 35 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor a302eb05d16124ec50a5132991c4f29e0e8fc04e HEAD`
  commit: feat(chat): acquire and replay chat durably in the daemon

- [x] 1-D2 — Synchronised stream events and viewer samples with coverage
  lane: L7
  needs: 1-C22, 1-C33
  touches: crates/research/src/**
  do: Landed on main as 04da539: feat(research): persist stream events and viewer samples with coverage. Legacy todo 36 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 04da539319380cf14c4886b75d00ffb307a395d2 HEAD`
  commit: feat(research): persist stream events and viewer samples with coverage

- [x] 1-D3 — Ingest readiness checklist: probe, proxies, waveforms, thumbnails
  lane: L7
  needs: 1-C21, 1-C33
  touches: crates/research/src/**
  do: Landed on main as a4e90ec: feat(media): add an ingest readiness checklist with per-step state. Legacy todo 37 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor a4e90ec7108bfa33116e5acc301cf58a11066447 HEAD`
  commit: feat(media): add an ingest readiness checklist with per-step state

- [x] 1-D4 — Source ownership, licence and consent notes with publication eligibility
  lane: L7
  needs: 1-C21, 1-C33
  touches: crates/research/src/**
  do: Landed on main as 05a69cc: feat(research): add source ownership, consent and publication eligibility. Legacy todo 38 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 05a69cc161ebfe32cbc1575910db8f643550995f HEAD`
  commit: feat(research): add source ownership, consent and publication eligibility

- [x] 1-D5 — Capture interruption, segment recovery, disk quota and workload isolation
  lane: L7
  needs: 1-C33, 1-D4
  touches: docs/PERFORMANCE.md, examples/soak_capture.rs, scripts/soak_capture_pressure.sh, src/daemon.rs, src/monitor/mod.rs, src/pipeline/executor.rs, src/pipeline/runtime.rs, src/recording/capacity.rs, src/recording/gaps.rs, src/recording/mod.rs, src/recording/soak.rs
  do: Landed on main as 92a414d: test(media): prove capture survives disconnect, disk pressure and concurrent rendering. Legacy todo 39 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 92a414dda2e2788451608a0d6b0f39348f0d4297 HEAD`
  commit: test(media): prove capture survives disconnect, disk pressure and concurrent rendering

- [x] 1-D6 — Backup, restore, retention and derivative-aware cleanup
  lane: L7
  needs: 1-C21, 1-C33
  touches: crates/research/src/**, crates/strivo-web/assets/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/e2e/**, crates/strivo-web/src/**, crates/strivo-web/src/server.rs
  do: Landed on main as 4e05026: feat(backup): make backup and restore derivative-aware and source-link aware. Legacy todo 40 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4e0502620ce60429dfad89a664693dcc9e8fe926 HEAD`
  commit: feat(backup): make backup and restore derivative-aware and source-link aware

- [x] 1-D7 — A persistent project workspace where every view coexists
  lane: L7
  needs: 1-C20, 1-C21, 1-C22, 1-C23, 1-C24, 1-C25, 1-C28, 1-C30
  touches: crates/research/src/**, crates/strivo-web/src/**, crates/strivo-web/src/server.rs, crates/strivo-web/tests/**, crates/strivo-web/tests/workspace_refusals.rs, crates/strivo-web/tests/workspace_routes.rs
  do: Landed on main as 2dbfefc: feat(web): build the persistent creator project workspace. Legacy todo 41 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 2dbfefc33c9761f29863be96612d911b64a422dd HEAD`
  commit: feat(web): build the persistent creator project workspace

- [x] 1-D8 — Transcript editing linked to media operations
  lane: L7
  needs: 1-C28, 1-D7
  touches: crates/editor/src/lib.rs, crates/editor/src/**, crates/editor/tests/**, crates/editor/tests/transcript_edit.rs, crates/editor/tests/transcript_link.rs, crates/editor/tests/transcript_media.rs
  do: Landed on main as b0b649d: feat(editor): link transcript editing to composition operations without touching media. Legacy todo 42 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b0b649dafaf3a8e766c4c7c880153f2814592eaa HEAD`
  commit: feat(editor): link transcript editing to composition operations without touching media

- [x] 1-D9 — A real multi-track timeline with frame-accurate editing
  lane: L7
  needs: 1-C22, 1-C23, 1-C25, 1-C30, 1-C31, 1-C32, 1-D3, 1-D7
  touches: crates/editor/src/lib.rs, crates/editor/src/**, crates/editor/tests/timeline.rs, crates/editor/tests/timeline_parity.rs, crates/strivo-web/assets/**, crates/strivo-web/e2e/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/src/**, crates/strivo-web/src/server.rs, crates/strivo-web/src/telemetry.rs
  do: Landed on main as cb06ef3: feat(editor): add a frame-accurate multi-track timeline with browser-native preview. Legacy todo 43 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor cb06ef320ae20f2052313f942de3c1fa32470f98 HEAD`
  commit: feat(editor): add a frame-accurate multi-track timeline with browser-native preview

- [x] 1-D10 — An audio mixer and an ordered effect chain
  lane: L7
  needs: 1-C25, 1-C34, 1-D9
  touches: crates/automation/src/lib.rs, crates/editor/src/**, crates/editor/tests/one_render_path.rs, crates/sidechain/src/lib.rs, crates/strivo-web/src/lib.rs, crates/strivo-web/src/mixer.rs, crates/strivo-web/src/render.rs, crates/strivo-web/src/**, crates/strivo-web/tests/**, crates/strivo-web/tests/mixer.rs, crates/strivo-web/tests/mixer_falsifier.rs
  do: Landed on main as cf52636: feat(editor): add an audio mixer whose preview and render share one plan. Legacy todo 44 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor cf5263662f4f3c8c29a51b77422f59ca11d7f6c1 HEAD`
  commit: feat(editor): add an audio mixer whose preview and render share one plan

- [x] 1-D11 — Captions as revisioned timeline content
  lane: L7
  needs: 1-D8, 1-D9, 1-D10
  touches: Cargo.lock, crates/captions/Cargo.toml, crates/captions/src/lib.rs, crates/captions/tests/burn_render.rs, crates/captions/tests/safe_area.rs, crates/captions/tests/sidecar_burn_agreement.rs, crates/captions/tests/timing.rs, crates/editor/src/**, crates/strivo-plugins/src/artifacts.rs, crates/strivo-web/src/ipc_client.rs, crates/strivo-web/src/**, crates/strivo-web/src/server.rs, crates/strivo-web/tests/pipeline_graph_to_run.rs
  do: Landed on main as b3f966d: feat(editor): make captions revisioned timeline content that survives cuts and speed changes. Legacy todo 45 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b3f966dfda51bd2ccd9849ee4713573fd9ce1eac HEAD`
  commit: feat(editor): make captions revisioned timeline content that survives cuts and speed changes

- [x] 1-D12 — Destination reframing and streamer layouts
  lane: L7
  needs: 1-D1, 1-D9
  touches: crates/editor/src/destination.rs, crates/editor/src/lib.rs, crates/editor/tests/destination_layout.rs
  do: Landed on main as 7b1ef32: feat(editor): add editable destination reframing and streamer layouts. Legacy todo 46 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 7b1ef32f4f63d2ca7123b94603e18b0790beb73e HEAD`
  commit: feat(editor): add editable destination reframing and streamer layouts

- [x] 1-D13 — Brand assets, titles, lower thirds and intro/outro templates
  lane: L7
  needs: 1-C32, 1-D9
  touches: crates/branding/src/asset.rs, crates/branding/src/lib.rs, crates/branding/src/library.rs, crates/branding/tests/safe_area.rs, crates/strivo-web/src/render.rs, crates/strivo-web/src/**, crates/strivo-web/tests/branding_preconditions.rs
  do: Landed on main as 2182a8c: feat(branding): make brand assets reusable across variants and fail loudly when missing. Legacy todo 47 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 2182a8ca4bbe70220a70bbeb6551dbd1ec11aee0 HEAD`
  commit: feat(branding): make brand assets reusable across variants and fail loudly when missing

- [x] 1-D14 — Layered B-roll, images, transitions, zoom/crop and basic colour
  lane: L7
  needs: 1-D9
  touches: crates/editor/src/lib.rs, crates/editor/src/**, crates/editor/tests/layered_broll.rs, crates/strivo-web/src/render.rs
  do: Landed on main as 1196d21: feat(editor): add layered B-roll over continuing source audio with transforms. Legacy todo 48 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 1196d2123ad6eb7d087c58ed44d6900619d5a166 HEAD`
  commit: feat(editor): add layered B-roll over continuing source audio with transforms

- [x] 1-D15 — Undo, redo, autosave, revision branching and snapshots
  lane: L7
  needs: 1-C28, 1-D8, 1-D9
  touches: crates/editor/src/history.rs, crates/editor/src/**, crates/editor/src/lib.rs, crates/research/src/**
  do: Landed on main as 4819ae6: feat(editor): add undo, autosave, revision branching and conflict detection. Legacy todo 49 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 4819ae6266eb7fd668aba273ddca74e08ca3046b HEAD`
  commit: feat(editor): add undo, autosave, revision branching and conflict detection

- [x] 1-D16 — Proxies, waveform pyramids, filmstrips and bounded preview caching
  lane: L7
  needs: 1-C23, 1-D9
  touches: crates/strivo-web/src/**, crates/strivo-web/src/server.rs, docs/PERFORMANCE.md, scripts/bench_media_pool.sh, scripts/seed_long_row.py, src/media/mod.rs, src/media/preview.rs, src/media/proxy.rs, src/media/pyramid.rs, src/media/spawn.rs, tests/common/mod.rs, tests/no_original_decode.rs, tests/preview_cache_bound.rs, tests/proxy_invalidation.rs, tests/proxy_mapping.rs
  do: Landed on main as b3fcbc6: perf(media): add proxies, waveform pyramids and bounded preview caching. Legacy todo 50 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b3fcbc65f4ad947d6e752cb2ed3d92d50912a4f1 HEAD`
  commit: perf(media): add proxies, waveform pyramids and bounded preview caching

- [x] 1-D17 — Keyboard-first usability, an accessible timeline and responsive review
  lane: L7
  needs: 1-C18, 1-D9
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/**
  do: Landed on main as fffe934: feat(editor): make the timeline keyboard-operable and review responsive. Legacy todo 51 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor fffe934723eb6960602f174e2b813c71c28c9a59 HEAD`
  commit: feat(editor): make the timeline keyboard-operable and review responsive

- [x] 1-D18 — Replace the blueprint display with a node graph over the real pipeline model
  lane: L7
  needs: 1-C26, 1-C31, 1-D9
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/e2e/**, crates/strivo-web/src/**, crates/strivo-web/tests/pipeline_graph.rs
  do: Landed on main as c0b6af8: feat(pipelines): render the real pipeline as a node graph over the executor model. Legacy todo 52 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor c0b6af8a2d76145ed2b06846c328138f05fa1645 HEAD`
  commit: feat(pipelines): render the real pipeline as a node graph over the executor model

- [x] 1-D19 — Author graphs in the browser: create, edit, validate and save
  lane: L7
  needs: 1-D18
  touches: crates/strivo-web/assets/**, crates/strivo-web/e2e/mock-server.mjs, crates/strivo-web/e2e/**, crates/strivo-web/src/**, crates/strivo-web/tests/pipeline_authoring.rs, src/pipeline/executor.rs, src/pipeline/stage.rs
  do: Landed on main as e12128e: feat(pipelines): let creators author and validate pipeline graphs in the browser. Legacy todo 53 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor e12128e8cfff80dc5b9e9e9be6eab540b61331b0 HEAD`
  commit: feat(pipelines): let creators author and validate pipeline graphs in the browser

- [x] 1-D20 — Make the authored graph the run, not a description of it
  lane: L7
  needs: 1-D18, 1-D19
  touches: crates/strivo-web/src/**, crates/strivo-web/tests/pipeline_graph_to_run.rs, docs/PIPELINE.md, src/pipeline/runtime.rs
  do: Landed on main as 80de53c: feat(pipelines): execute authored graphs directly with preserved identity. Legacy todo 54 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 80de53cb1fe8a2a0dc56fef79a68142b24896fc6 HEAD`
  commit: feat(pipelines): execute authored graphs directly with preserved identity

- [x] 1-D21 — Live clipping during capture
  lane: L7
  needs: 1-C22, 1-D1, 1-D9, 1-D18
  touches: crates/research/src/**, examples/soak_live_clip.rs, scripts/soak_live_clip.sh, src/recording/live_clip.rs, src/recording/mod.rs
  do: Landed on main as b55350d: feat(pipelines): extract and clip during live capture without starving acquisition. Legacy todo 57 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor b55350d89f2d217cc65ef7f07e3da07572806942 HEAD`
  commit: feat(pipelines): extract and clip during live capture without starving acquisition

- [x] 1-D22 — A release package and a truthful publication state machine
  lane: L7
  needs: 1-C21, 1-C24, 1-D4, 1-D6, 1-D15
  touches: crates/research/src/**, crates/strivo-plugins/src/pipeline_templates.rs, crates/strivo-web/tests/pipeline_graph.rs, docs/PERFORMANCE.md
  do: Landed on main as 3dbce4e: feat(publish): add immutable release packages and a truthful publication state machine. Legacy todo 59 in .omo/plans/strivo-review-and-pro-edition.md.
  qa: `git merge-base --is-ancestor 3dbce4ef5df51135fe89e933a6c12ed6d87cc4bf HEAD`
  commit: feat(publish): add immutable release packages and a truthful publication state machine


### M2 — PVR hygiene: the seventeen open revoy ledger rows, re-derived against HEAD
epoch: The revoy ledger's open rows are closed or re-derived against HEAD, and the licence backend deploys once the owner creates the D1 database.
lanes: L1 (strivo-web assets, SPA and e2e), L2 (strivo-web server routes and tests), L3 (src/ daemon, pipeline and recording), L4 (crates/editor render and timeline), L5 (src/platform, src/licence and licence-backend), L6 (docs, tracked top-level docs, CI and packaging)
gates: .keel/gates/M2.json

#### M2-A — Pro licence and upgrade surface
exit:
- [ ] The licence backend deploys from a workflow and the upgrade card reads the entitlement it displays | proof: `git grep -q 'wrangler' -- .github/workflows`
- [ ] The Plugins hub shows a real entitlement verdict for a free and a Pro account | verify: auditor

todos:
- [ ] 2-A1 — Deploy licence-backend through a workflow once the D1 database exists
  lane: L5
  needs: -
  touches: licence-backend/**, .github/workflows/**
  do: licence-backend/wrangler.toml:11 still holds REPLACE_WITH_D1_ID, and no workflow in .github/workflows references wrangler (checked at 6a2cd1c). Once the owner has created the D1 database and recorded its id, replace the placeholder and add a workflow that deploys the Worker, so Pro can be sold. Ledger row: 'Deploy licence-backend to Cloudflare'.
  must-not: Do not put the account id or any token in the repo. The deploy secret lives in the repository's secrets.
  owner: create the Cloudflare D1 database and record its id in licence-backend/wrangler.toml (the placeholder is on line 11); this is the act the todo waits on, and it is not a code change
  qa: `git grep -q 'wrangler' -- .github/workflows && ! git grep -q 'REPLACE_WITH_D1_ID' -- licence-backend/wrangler.toml`
  commit: ci: deploy the licence backend from a workflow

- [ ] 2-A2 — Make the Creator upgrade card read licence.implemented, not only licence.entitled
  lane: L1
  needs: -
  touches: crates/strivo-web/assets/spa/**
  do: Ledger row PHASE0/F-11 (P1). crates/strivo-web/assets/spa/021-creator.js:694 branches on licence.entitled alone, and the trial button calls API.licenceTrial() unconditionally (021-creator.js:32, :713). The backend response carries an implemented flag (crates/strivo-web/src/routes/licence.rs:38, :83 set false), which the SPA never reads. Show a real verdict: not implemented means a stated state, not an upgrade offer.
  must-not: Do not ship a price or a payment flow.
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/licence-upgrade-card.spec.ts`
  commit: fix(web): show the real licence verdict on the upgrade card

- [ ] 2-A3 — Bring the Pro panes off route null, or stop the signposts pointing at them
  lane: L1
  needs: -
  touches: crates/strivo-web/assets/spa/**
  do: Ledger row PHASE0/F-12 (P1). Re-counted at 6a2cd1c: crates/strivo-web/assets/spa/021-creator.js has 20 'route:' entries of which 15 are 'route: null'. The ledger says 14 of 20; reconcile the count when the fix lands and cite the file:line of each remaining null pane.
  must-not: Do not remove a pane that the entitlement tests reference.
  qa: `cd crates/strivo-web/e2e && npx playwright test tests/pro-panes-routed.spec.ts`
  commit: fix(web): give every Pro pane a route or remove its signpost

- [ ] 2-A4 — Make /api/v1/pipelines/dag read real run state, not four fixed templates
  lane: L2
  needs: -
  touches: crates/strivo-web/src/routes/**
  do: Ledger row 'Reconcile /api/v1/pipelines/dag with real run state'. crates/strivo-web/src/routes/api.rs:390 documents the route as the canonical DAW-vision pipeline and api.rs:1022 mounts it; crates/strivo-web/src/routes/pipeline_graph.rs:7 says it still serves the fixed templates unchanged. Serve the executor's real state, or retire the route with its callers. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace dag_reads_run_state 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(pipelines): serve the dag route from real run state

#### M2-B — Pipeline runtime
exit:
- [ ] The executor's shared resource locks, event channel and stage scan match the ledger's claims or the claims are retired | proof: `cargo test --locked --workspace transcription_gpu_cap 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] A stalled consumer and a wedged GPU stage are both bounded and visible | verify: auditor

todos:
- [ ] 2-B1 — Give the pipeline a transcription-specific cap so a long transcribe cannot starve the GPU
  lane: L3
  needs: -
  touches: src/pipeline/**
  do: Ledger row PHASE0/F-36 (P2). src/pipeline/executor.rs:775-782 and :936 hold one process-wide Gpu permit, so a long transcription blocks every other GPU stage. Add a transcription-specific limit, and test that a second GPU stage makes progress while one transcription is running. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace transcription_gpu_cap 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(pipeline): cap transcription separately from the shared GPU permit

- [ ] 2-B2 — Bound the pipeline event channel
  lane: L3
  needs: -
  touches: src/pipeline/**, src/daemon.rs
  do: Ledger row PHASE0/F-38 (P3). src/pipeline/runtime.rs:376 creates the pipeline event channel with mpsc::unbounded_channel, so a stalled consumer grows memory without limit. Bound it and decide the overflow behaviour (drop oldest or back-pressure), and say which in the code. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace bounded_event_channel 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(pipeline): bound the event channel and say what overflow does

- [ ] 2-B3 — Stop the executor rescanning the whole stage registry on every wake
  lane: L3
  needs: -
  touches: src/pipeline/**
  do: Ledger row PHASE0/F-35 (P3). The executor rescans the stage registry on each wake to find ready stages. Keep a ready set updated on completion. Locate the scan at HEAD before changing it; the ledger's line reference is not re-derived here. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace ready_set 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(pipeline): track ready stages instead of rescanning the registry

- [ ] 2-B4 — Render an EDL timeline in one ffmpeg pass, not one process per cut plus a concat
  lane: L4
  needs: -
  touches: crates/editor/**
  do: Ledger row PHASE0/F-33 (P2). The EDL render runs one ffmpeg subprocess per cut and a final concat pass, so a long timeline is N+1 spawns (crates/editor/src/render/invoke.rs and execute.rs). Collapse it to one invocation that the render plan describes, and keep the rendered output byte-for-byte identical to the per-cut result on the fixture corpus. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace edl_single_pass 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(editor): render an edl timeline in one ffmpeg pass

#### M2-C — Web surface and performance
exit:
- [ ] Every ledger row in this phase is closed or re-derived against HEAD with its evidence cited | verify: auditor
- [ ] The served bundle, pagination and SSE fan-out match the measurements in docs/PERFORMANCE.md | proof: `cargo test --locked --workspace no_raw_hex_in_spa_css 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`

todos:
- [ ] 2-C1 — Replace hardcoded hex values in spa-css with design tokens
  lane: L1
  needs: -
  touches: crates/strivo-web/assets/spa-css/**
  do: Ledger row PHASE0/F-22 (P3). Roughly 55 hardcoded hex values remain in crates/strivo-web/assets/spa-css/ (000-pvr.css:1028 is representative; a count at 6a2cd1c finds 10 in 000-pvr.css and 25 across the directory). Replace them with the DESIGN.md tokens; scripts/check-design-tokens.mjs is the gate. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Do not deviate from DESIGN.md without the owner's approval (AGENTS.md, Design system).
  qa: `cargo test --locked --workspace no_raw_hex_in_spa_css 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: style(css): replace the remaining hardcoded hex values with tokens

- [ ] 2-C2 — Replace the offset-plus-COUNT recordings pagination with keyset paging
  lane: L3
  needs: 2-A4
  touches: src/recording/**, crates/strivo-web/src/routes/**
  do: Ledger row PHASE0/F-27 (P3). The recordings list pages with an integer offset and a per-page COUNT(*), so deep pages degrade linearly. The SQL lives in src/recording/persist.rs. Move to keyset paging and keep the cursor the SPA already sends. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace keyset_paging 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(db): page recordings with a keyset cursor

- [ ] 2-C3 — Add a covering index for the recordings json_extract query
  lane: L3
  needs: -
  touches: src/recording/**
  do: Ledger row PHASE0/F-28 (P3). The recordings query runs a per-row json_extract with no covering index. Measure the query with docs/perf/ before and after; record the number, not a claim. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace recordings_query_index 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(db): index the recordings query that filters on json fields

- [ ] 2-C4 — Chunk the served SPA bundle by route
  lane: L2
  needs: 2-A2, 2-A3
  touches: crates/strivo-web/build.rs, crates/strivo-web/assets/spa/**
  do: Ledger row PHASE0/F-29 (P3). The served SPA bundle is one concatenation of about 1.08 MB of modules with no route chunking, against the 250 KiB threshold in docs/PERFORMANCE.md. The bundle size is not re-measured here; scripts/check-bundle-size.sh is the measurement and CI runs it. Keep the Creator vocabulary out of the PVR bundle (crates/strivo-web/e2e pretest). The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace route_chunks 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(web): split the SPA bundle by route

- [ ] 2-C5 — Share one SSE fan-out instead of one IPC connection per browser tab
  lane: L2
  needs: 2-C2
  touches: crates/strivo-web/src/routes/events.rs
  do: Ledger row PHASE0/F-30 (P3). The SSE stream opens one IPC connection per browser tab with no server-side fan-out (crates/strivo-web/src/routes/events.rs). Put one upstream connection behind a broadcast. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace sse_fanout 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(events): fan one upstream IPC connection out to every tab

- [ ] 2-C6 — Stop the plugin endpoints opening per-request resources
  lane: L2
  needs: 2-C2
  touches: crates/strivo-web/src/routes/plugins.rs
  do: Ledger row PHASE0/F-34 (P3). The plugin endpoints open per-request resources 27 times per request (was 12); crates/strivo-web/src/routes/plugins.rs:4934 routes plugin storage size and clear. Count the opens with a test before and after. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace plugin_resources_opened_once 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(plugins): open plugin resources once per request

- [ ] 2-C7 — Give the daemon's IPC socket a path that binds under any state directory
  lane: L3
  needs: -
  touches: src/ipc.rs
  do: src/ipc.rs:242 places the daemon socket at state_dir().join('strivo.sock'). A long state directory makes the Unix listener fail with the same 'path must be shorter than SUN_LEN' error the ipc_handshake tests hit under a long TMPDIR (todo 1-A1). Bind through a short fixed path, or fail at startup with an error that names the length limit and the directory. The test that pins it is socket_path_length. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Do not change the protocol or the socket's permissions.
  qa: `cargo test --locked --workspace socket_path_length 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(ipc): bind the daemon socket under any state directory length

#### M2-D — Tests, docs and claims
exit:
- [ ] The browser lanes run every spec serially where the spec needs it, and the niche claim is removed or tested | proof: `cd crates/strivo-web/e2e && npm test`
- [ ] The auto-transcribe gate has one source of truth and a route test | verify: auditor

todos:
- [ ] 2-D1 — Test the unoccupied-niche claim against the closest tool before repeating it
  lane: L6
  needs: -
  touches: ROADMAP.md, README.md, docs/**
  do: Ledger row 'Test the unoccupied-niche positioning claim against MrBrax/LiveStreamDVR'. ROADMAP.md:330-331 asserts an unoccupied '*arr-grade live-stream PVR niche'. docs/audits/2026-09-25-consolidated/MANIFEST.md:178 already records it as a market assertion the source cannot settle. Write the comparison as a doc with the sources, and keep the claim only if the comparison supports it.
  must-not: Do not repeat the claim in customer-facing copy until this todo is done.
  qa: `git grep -q 'LiveStreamDVR' -- docs`
  commit: docs(roadmap): record the niche comparison against the closest tool

- [ ] 2-D2 — Put the headless auto-transcribe gate behind one source of truth
  lane: L3
  needs: -
  touches: src/config/**
  do: Ledger row PHASE0/F-16 (P2). Headless auto-transcribe is still gated on config-only lists or a marker file, with no SPA control to copy the archiver tandem toggle from. The legacy row names no file; a grep of HEAD for an auto-transcribe symbol found none, so locate the gate before changing it and record where it is. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace headless_auto_transcribe 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(config): gate headless auto-transcribe on one setting with an SPA control

- [ ] 2-D3 — Serialise the skip-link spec that is flaky under parallel workers
  lane: L1
  needs: -
  touches: crates/strivo-web/e2e/tests/**
  do: Ledger row WS/V05. crates/strivo-web/e2e/tests/spa-ux-remediation.spec.ts has no serial-mode configure block, unlike its sibling player-controller.spec.ts:21 (checked at 6a2cd1c: no 'serial' in the file). Add the block and run the spec alone and in the full lane.
  qa: `git grep -q 'mode: "serial"' -- crates/strivo-web/e2e/tests/spa-ux-remediation.spec.ts`
  commit: test(web): run the skip-link spec serially


### M3 — Creator publishing loop (legacy Wave 9, deferred behind M1 and M2)
epoch: Creator publishing works end to end: a vault and capability-driven destinations, a first connector verified without credentials, metadata and calendar, a publishing package, QC and variants, and a release-hold review. Deferred behind M1 and M2: no commits in the last fourteen days touch its paths; carried verbatim from .omo Wave 9 (owner-ratified in the legacy TL;DR, 2026-09-25).
lanes: L1 (strivo-web assets, SPA and e2e), L2 (strivo-web server routes and tests), L3 (src/ daemon, pipeline and recording), L4 (crates/editor render and timeline), L5 (crates/research and crates/dataviz), L6 (crates/strivo-plugins and crates/pipelines-dag), L7 (src/platform, src/licence and licence-backend), L8 (other crates)
gates: .keel/gates/M3.json

#### M3-A — Publishing vault, destinations and metadata
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace publishing_vault 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 3-A1 — A publishing connection vault and capability-driven destinations
  lane: L7
  needs: 1-D22
  touches: src/platform/youtube.rs, crates/reuse/src/lib.rs, src/licence/verify.rs
  do: Report item P02. OAuth, refresh and revoke must work for a test account; secrets must be protected using the existing keyring pattern at `src/platform/youtube.rs:302-304`; unsupported permissions, formats and account capabilities must be explicit. Destination controls are capability-driven, including per-account duration limits — `docs/CREATOR-EDITION-REPORT-2026-09-07.md:214` specifically forbids encoding Reuse's 60-second default as a universal platform rule. Acceptance (legacy): (agent-executable): OAuth, refresh and revoke pass against a local contract-test server; a secret never appears in a log, an error or a support bundle; an unsupported permission is reported as unsupported rather than attempted; a per-account duration limit is read from the destination's declared capabilities. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT infer write access from existing read or capture support, which is the mistake `docs/CREATOR-EDITION-REPORT-2026-09-07.md:216` names
  qa: `cargo test --locked --workspace publishing_vault 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add a connection vault with capability-driven destination controls

- [ ] 3-A2 — First real connector: YouTube upload and reconciliation, verified without credentials
  lane: L8
  needs: 1-C20, 1-C27, 1-D11, 1-D12, 1-D13, 1-D22, 3-A1
  touches: crates/marketplace/src/lib.rs, crates/strivo-web/src/routes/import_export.rs, src/platform/youtube.rs
  do: Report item P03, built to the contract research: `POST https://www.googleapis.com/upload/youtube/v3/videos`, scope `youtube.upload`, 100 `videos.insert` calls/day at 1 unit each, 256 GB max, resumable protocol with a session `Location` then chunked `PUT` returning `308 Resume Incomplete` plus `Range` on interruption. The idempotency mechanism is the session URI — persist the `Location` before sending bytes and, after an ambiguous failure, probe with `Content-Range: bytes */TOTAL`, which returns the *original* response including the video ID; never re-initialise, which would create a duplicate. Poll `status.uploadStatus` and `processingDetails.processingStatus` (owner-only). Because the plan assumes no credentials, the connector is verified against a local contract-test server implementing the same protocol, and the real-account step is recorded as a named, explicitly blocked acceptance item. Document, do not hide: an unverified API project forces every upload to `private`. Implement the resumable protocol directly on `reqwest` 0.13.5 — research found no maintained general-purpose resumable-upload crate. Acceptance (legacy): (agent-executable): against the contract-test server, an interrupted upload resumes from the reported `Range`; an ambiguous response is resolved by the `bytes */TOTAL` probe and yields the **original** video ID with no duplicate; `uploadStatus` and `processingStatus` transitions are reflected in the UI; a documentation note records the forced-private-until-audit behaviour and the 100/day ceiling. No test requires a live account. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace youtube_upload_connector 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add a YouTube upload connector with resumable sessions and reconciliation

- [ ] 3-A3 — Metadata, thumbnail, caption, chapter and disclosure editor
  lane: L6
  needs: 3-A1, 3-A2
  touches: crates/strivo-plugins/src/artifacts.rs, crates/captions/src/lib.rs
  do: Report item P04. Destination preview validates the applicable fields and their accessibility; approved metadata is versioned; unsupported attachments stay downloadable. The existing `docs/CREATOR-EDITION-REPORT-2026-09-07.md:73` finding applies — description and draft statuses must not be presented as proof of upload. Acceptance (legacy): (agent-executable): the destination preview reports every invalid or unsupported field with a reason; approved metadata is versioned and a later edit creates a new version; a draft never renders as published anywhere in the UI. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT present a draft as published
  qa: `cargo test --locked --workspace metadata_disclosure_editor 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add a destination-validating metadata and disclosure editor

- [ ] 3-A4 — A calendar, schedule execution and missed-time recovery
  lane: L3
  needs: 1-D22, 3-A1, 3-A2
  touches: src/recording/schedule.rs, src/state.rs, crates/strivo-web/assets/spa/012g-pvr.js
  do: Report item P05. The correct account and timezone are visible; DST changes and offline-host behaviour are specified; native scheduling is used where the destination supports it and manual posting is labelled where it does not. The PVR already has a 7-day calendar strip (`ROADMAP.md:118`) — this is the publishing calendar, and it must not be confused with it. Acceptance (legacy): (agent-executable): **all rehearsals run against the local contract-test destination from todo 61 — no live account, ever.** The native-scheduling path is exercised with a contract fixture declaring `supports_scheduling: true`; the manual-posting path with one declaring `false`. A DST transition does not fire a scheduled post twice or skip it silently; an offline host defers and reports; a missed window is recovered exactly once. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT double-post after a missed run
  qa: `cargo test --locked --workspace publishing_calendar_dst 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add a publishing calendar with DST and offline recovery

- [ ] 3-A5 — An excellent manual publishing package
  lane: L6
  needs: 1-D11, 1-D13, 1-D22, 3-A1
  touches: crates/strivo-plugins/src/artifacts.rs, crates/reuse/src/lib.rs
  do: Report item P08, and the plan's guarantee that no acceptance criterion requires a live platform account. Download media, captions, thumbnail, copy, chapters and provenance together, and mark manually posted content with an optional remote link so it can later receive results. A successful local export must never be marketed as direct publishing, and a manual post must never be silently recorded as a successful API post. Acceptance (legacy): (agent-executable): the package contains every declared artefact plus a provenance file; a manual post is recorded with its provenance and an optional link; a test asserts no manual post is reported through the connector's status path. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace manual_publishing_package 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): ship a complete manual publishing package with provenance

#### M3-B — Media QC, variants and export
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace final_media_qc 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 3-B1 — Final media QC and a review player
  lane: L2
  needs: 1-C30, 1-D22
  touches: crates/strivo-web/src/telemetry.rs, src/media/**
  do: Report item R05. Detect missing streams, unexpected duration, black or silent faults, clipped audio and caption overflow; the review player plays the exact artifact selected for release, not a re-render. This is the gate before todo 59's approval can be granted. Acceptance (legacy): (agent-executable): each fault class has a fixture that is detected; a clean artifact passes; the review player's bytes hash-match the artifact in the release package. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT approve a package whose artifact fails QC
  qa: `cargo test --locked --workspace final_media_qc 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(media): add final media QC and an exact-artifact review player

- [ ] 3-B2 — Immutable output variants with stale-dependency tracking
  lane: L5
  needs: 1-C23, 1-D9, 1-D15
  touches: crates/research/**, crates/editor/**
  do: Report item R03. A parent revision change marks affected variants and artifacts stale; manual overrides are explicit; an approved release keeps its original revision. This is what stops "the editor saved but my export is old". Acceptance (legacy): (agent-executable): changing a parent revision marks its derived variants stale; an approved release package retains its original revision; an explicit override is recorded as such. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace output_variant_staleness 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(editor): track output variants and stale dependencies against the parent revision

- [ ] 3-B3 — Validated export profiles and a batch queue
  lane: L4
  needs: 1-C25, 1-C30, 1-C34, 1-D9, 1-D10, 1-D11, 1-D12, 1-D14, 1-D15, 3-B1, 3-B2
  touches: crates/editor/src/lib.rs
  do: Report item R04. Render tested MP4, video, audio and subtitle packages with correct dimensions, streams and duration; batch progress and errors are per artifact, not per batch. This closes report item §4.5 — the current stream-copy cuts are not normalised to a common render spec, which risks cut accuracy, mixed media incompatibility, absent audio and inconsistent output. Acceptance (legacy): (agent-executable): each declared profile decodes to the declared dimensions, streams and duration; silent, video-only, mixed-rate and mixed-codec fixtures all pass; a batch reports per-artifact progress and isolates one artifact's failure from the rest. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT document a "single ffmpeg pass" claim for the full EDL render; `docs/audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md:79` (F-33) records that the render is N sequential passes plus a concat, and that framing must not propagate into positioning copy
  qa: `cargo test --locked --workspace export_profile_batch_queue 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(export): add validated export profiles with a per-artifact batch queue

#### M3-C — Security, release hold and real-server journey
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace creator_route_security_review 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 3-C1 — Security, privacy and asset-dependency review with account isolation
  lane: L2
  needs: 1-C27, 1-D4, 1-D22, 3-A1, 3-B1, 3-B3
  touches: crates/strivo-web/src/server.rs, crates/strivo-web/src/auth.rs, crates/strivo-web/tests/routes.rs
  do: Report item O02. Audit routes, jobs, imports, tokens, external processing and render inputs; retain required notices and provenance; exclude unresolved material from release. Builds on todo 25's boundary validation and todo 60's vault. The repo already has a real security posture to preserve — `require_auth` as a default-deny route layer, constant-time HMAC verification, and a mutation-proven route-auth invariant — and this todo must not regress any of it. Acceptance (legacy): (agent-executable): the sweep asserts **zero routes that read or mutate Pro-plugin state lack a `gate_pro()` call, in both editions** — *not* merely zero routes lacking `require_auth`, which `crates/strivo-web/src/server.rs:338` already satisfies globally via a default-deny `route_layer`. A route test proves the sweep goes red if `gate_pro` is removed from `dataviz/run`; a test proves cross-account access is refused; external processing declares its data destination and requires explicit consent; a support bundle redacts every secret. Pair the predicate with the descriptor from todo 20 (subset-of-`gate_pro`-arguments, pinned to `CapabilityKey`). The existing route-auth sweep for `require_auth` stays and is reported separately. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT exclude an unresolved asset from a release silently
  qa: `cargo test --locked --workspace creator_route_security_review 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(security): review creator routes, jobs, tokens and external processing for release

- [ ] 3-C3 — Real-server, real-media Creator contract and journey coverage (O03)
  lane: L1
  needs: 1-C21, 1-D9, 1-D11, 3-B3
  touches: crates/strivo-web/e2e/real-server.sh, crates/strivo-web/e2e/tests-real/real-server.spec.ts
  do: Report item O03 (`:375`), a P0 prerequisite for the release-hold review (todo 69) and the field study (todo 89). Test capture/import → moments → edit → revision recovery → actual rendered outputs, and fail on missing routes, ignored settings and edition leaks. The Sept 6 manifest's S04 established a real-server lane for the PVR; the Creator edition still runs only against the mock. Acceptance (legacy): (agent-executable): a Creator real-server lane boots the compiled `--features creator` binary under a throwaway `HOME` and drives the full journey; it fails if a route 404s, a setting is silently ignored, or a Creator symbol appears in the PVR bundle; the rendered output is `ffprobe`-verified. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT pass on a mock lane
  qa: `cargo test --locked --workspace real_server_creator_journey 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: test(web): add real-server real-media contract coverage for the Creator journey

- [ ] 3-C2 — Creator packaging, entitlement design and the release-hold review
  lane: L7
  needs: 1-C1, 3-C1, 3-C3
  touches: licence-backend/wrangler.toml, src/licence/gate.rs, .github/workflows/release.yml, .github/workflows/docker.yml
  do: Report item O07, the commercial-readiness workstream the user approved. Produce verified installers and documentation for both editions; ensure no debug unlock ships as commercial authorisation (`src/licence/gate.rs:49-54` and its release-profile test at `:78-101` already hold, and must keep holding); add the missing `deploy-licence.yml` workflow that `licence-backend/README.md:85-87` already claims exists; and close the release-hold preconditions in `docs/LICENCE-BACKEND-DEPLOY.md:20-35` — preconditions 1–3 (threat model, entitlement, refund/revocation, machine binding; security review of Creator routes, external tools and data handling; reproducible packaged builds with upgrade/rollback tests and support terms) each closed with a linked evidence path, and precondition 4 — *"explicit operator approval for a specific product, price, payment provider, and public release plan"* (`:30`) — recorded as an OPEN, owner-owned item naming the exact approval artefact required. An agent must never synthesise evidence for precondition 4: doing so is the precise false success the adversarial rule exists to prevent, and it would also violate this plan's own guardrail against choosing a price. The release hold stays in place. The plan does not cut a release and does not choose a price — the user retains that decision. Acceptance (legacy): (agent-executable): both editions build reproducibly and a packaged upgrade-and-rollback rehearsal passes; a release-profile test proves no debug unlock grants entitlement; **preconditions 1–3** of `docs/LICENCE-BACKEND-DEPLOY.md:20-35` are each closed with a linked evidence path; **precondition 4** — *"explicit operator approval for a specific product, price, payment provider, and public release plan"* (`:30`) — **is recorded as an OPEN, owner-owned item** naming the exact approval artifact required, and is *not* marked closed by this plan. The release hold stays in place. The deploy workflow exists and its state is stated truthfully in the README. The PVR edition remains independently valid and shippable throughout. **An agent must never synthesise evidence for precondition 4** — that would be precisely the false success the adversarial rule exists to prevent, and it would also violate this plan's own guardrail against choosing a price. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace release_hold_packaging 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: chore(release): prepare creator packaging, close release-hold preconditions 1-3, record precondition 4 as open owner-owned


### M4 — Measured feedback (legacy Wave 10, deferred behind M3)
epoch: Outcomes from authorised platforms feed the production loop: a metric dictionary, ingestion, a linked source-to-outcome timeline, cohorts and funnels. Deferred behind M3: no commits in the window; carried verbatim from .omo Wave 10.
lanes: L1 (strivo-web server routes and tests), L2 (crates/research and crates/dataviz), L3 (src/platform, src/licence and licence-backend), L4 (other crates)
gates: .keel/gates/M4.json

#### M4-A — Outcome ingestion and linked timeline
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace metric_dictionary 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 4-A1 — A metric dictionary, observation schema, freshness and provenance
  lane: L2
  needs: 1-C7
  touches: crates/research/**
  do: Report items A01 and §8.1. Each metric carries unit, scope, denominator, source, coverage and a definition version. Observed, derived and predicted values must never be confusable — this is the plan's central honesty contract. Record the denominator before deriving any rate; use duration-weighted calculations where appropriate; state whether speaker time includes overlapping speech; distinguish recorded from speaking duration; never merge speaker identities across unrelated recordings without evidence; never sum cross-platform view totals without qualification. Acceptance (legacy): (agent-executable): every metric in the dictionary has unit, scope, denominator, source, coverage and definition version; **a schema test asserts an observed value and a derived value are `distinguishable-by-construction`** — the two occupy different types or are separated by a field the schema requires, so neither can be written where the other belongs; a missing observation is represented as absent, never as zero. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT zero-fill missing data
  qa: `cargo test --locked --workspace metric_dictionary 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(data): add the metric dictionary with freshness, coverage and provenance

- [ ] 4-A2 — Stop labelling derived proxies as measured retention
  lane: L4
  needs: 1-D1, 1-D2, 1-D13, 4-A1
  touches: crates/insights-compare/src/lib.rs, crates/heatmap/src/lib.rs, crates/viewguard-trend/src/lib.rs, crates/chat-density/src/lib.rs, crates/strivo-web/assets/spa/021-creator.js
  do: Report item A02, and the repo's most entrenched honesty gap. `crates/insights-compare/src/lib.rs:195` computes `retention = 0.6·talk + 0.4·action` and its own module doc at `:9-14` admits it "stands in for real audience-retention data". `crates/heatmap/src/lib.rs:145` fuses four self-derived signals. `crates/viewguard-trend/src/lib.rs:88-94` bands another derived score. `crates/chat-density/src/lib.rs:210` computes engagement from a caller-supplied log. Rename and explain every one: activity, not retention; suggested moments, not truth; anomaly with stated uncertainty, not an accusation. `docs/CREATOR-EDITION-REPORT-2026-09-07.md:110` is explicit that viewguard anomalies require uncertainty and review. Acceptance (legacy): (agent-executable): a test asserts no user-facing string labels a derived value as a measured metric; every derived surface shows its formula, its inputs and its confidence; the existing `docs/PERFORMANCE.md`-style "claims, not guarantees" discipline is applied to analytics copy. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT ship any string containing "retention" that refers to a derived value
  qa: `cargo test --locked --workspace derived_proxy_labels 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: fix(analytics): rename derived proxies so no heuristic is labelled a measured metric

- [ ] 4-A3 — Authorized outcome ingestion, starting with YouTube Analytics
  lane: L3
  needs: 1-C7, 1-C20, 1-C21, 3-A2, 4-A1
  touches: src/platform/youtube.rs, crates/research/src/lib.rs
  do: Report item A03, built to the researched contract: `GET https://youtubeanalytics.googleapis.com/v2/reports`. Request both `yt-analytics.readonly` and `youtube.readonly` — Google's own page carries a banner requiring the latter while omitting it from its scope table, an inconsistency the plan routes around rather than guesses at. Persist supported metrics against the remote post and the release revision, and surface authorisation gaps, delay and provider limits. Constraints to document, not hide: the retention dimension returns exactly 100 points, ratios 0.01–1.0, equally spaced, excluding the end of interval — so a 2-hour video resolves at 72 seconds and a 6-hour one at ~3.6 minutes, meaning frame-level outcome measurement is impossible; `audienceWatchRatio` can exceed 1.0 where footage is replayed; and `day`-dimension queries return no rows for the most recent days. Per the approved credential decision, the real-account step is a named, explicitly blocked acceptance item; no test requires a live account. Acceptance (legacy): (agent-executable): a contract-test fixture returns a 100-point retention series and the UI renders its true resolution rather than implying per-second data; a metric above 1.0 renders without being clamped or flagged as corrupt; an authorisation gap is reported as a gap; the resolution limit is stated in the UI. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace youtube_analytics_ingestion 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): ingest authorized YouTube outcomes against release revisions

- [ ] 4-A4 — A linked source, composition and outcome timeline
  lane: L2
  needs: 1-C21, 1-C24, 1-D15, 1-D22, 3-A2, 4-A1, 4-A3, 4-A2
  touches: crates/research/**, crates/strivo-web/assets/spa/**
  do: Report item A04. A measured retention interval opens the corresponding published revision and then the original source spans, respecting the 100-point sampling resolution and repeated footage — a source interval can legitimately map to several outputs, so reverse mapping is many-to-one per todo 22. Acceptance (legacy): (agent-executable): a measured interval opens the exact source span; repeated footage resolves to all contributing spans; the displayed resolution matches the provider's sampling and is stated. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT imply a precision the provider does not offer
  qa: `cargo test --locked --workspace outcome_timeline_linked 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): link measured outcome intervals back to composition and source

#### M4-B — Cohorts, funnel and dashboards
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace comparable_cohorts 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 4-B1 — Comparable cohorts and creator performance dashboards
  lane: L2
  needs: 1-C22, 1-D1, 1-D2, 1-D9, 1-D21, 4-A1, 4-A2, 4-A3, 4-A4
  touches: crates/research/**, crates/dataviz/**
  do: Report item A05. Compare format-, topic-, length- and age-matched posts with sample counts; keep provider-native definitions inspectable; qualify every cross-platform sum. Note that a chat spike may follow a raid or a giveaway rather than content quality, so context must be exposed alongside any recommendation. Acceptance (legacy): (agent-executable): a cohort comparison displays its sample count and its matching criteria; a low-sample cohort reports insufficient evidence rather than a ranking; a cross-platform total is labelled qualified with its sources. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace comparable_cohorts 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): add comparable cohorts with sample counts and qualified totals

- [ ] 4-B2 — A creator production funnel and effort analytics
  lane: L1
  needs: 4-A1, 4-A4
  touches: crates/strivo-web/src/telemetry.rs
  do: Report item A06. Explain where candidates are rejected, where time is spent and where failures occur — source → candidate → accepted → rendered → published. Collection is local or opt-in, with reset and export. Count approved useful outputs, not all generated artifacts; `docs/CREATOR-EDITION-REPORT-2026-09-07.md:23` is explicit that the north-star is creator-approved publish-ready outputs per hour of human editing, not the number of generated clips. Acceptance (legacy): (agent-executable): the funnel counts every stage with its own event definition; no corpus content appears in any collected event; reset and export work and are documented; the headline metric is approved outputs per hour of human editing. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT collect corpus content
  qa: `cargo test --locked --workspace production_funnel_effort 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): add the production funnel and effort analytics


### M5 — Comprehensive Creator product (legacy Wave 11, deferred behind M4)
epoch: The Creator product has dataviz, calendar, portable packages, reproducible exports, collaboration and field-validation evidence. Deferred behind M4: no commits in the window; carried verbatim from .omo Wave 11.
lanes: L1 (strivo-web assets, SPA and e2e), L2 (strivo-web server routes and tests), L3 (src/ daemon, pipeline and recording), L4 (crates/research and crates/dataviz), L5 (other crates)
gates: .keel/gates/M5.json

#### M5-A — Dataviz and saved queries
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace saved_dataviz_queries 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 5-A1 — Saved server-side analytical queries and richer chart series
  lane: L4
  needs: 1-C29
  touches: crates/dataviz/src/lib.rs, crates/strivo-web/assets/spa/015-creator.js
  do: Report item V01. Aggregate a saved source selection without sending every transcript through the browser; include units, dimensions, series identity and query provenance. The existing `{label, value}` points suffice for simple bars but not for multiseries time plots, error bars, scatter, network relations or source-linked brushing, per `docs/CREATOR-EDITION-REPORT-2026-09-07.md:249`. Acceptance (legacy): (agent-executable): a saved query over a 10 000-hour selection aggregates server-side within the 500 ms uncached budget; every series carries units, dimensions and provenance; a test asserts no full transcript set crosses the wire. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT materialise an unbounded collection; paginate or stream
  qa: `cargo test --locked --workspace saved_dataviz_queries 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): add saved server-side queries and richer chart series

- [ ] 5-A2 — Linked brushing, filtering, evidence links and chart annotations
  lane: L1
  needs: 3-B2, 4-B1, 5-A1
  touches: crates/strivo-web/assets/spa/015-creator.js, crates/strivo-web/assets/spa-css/017-creator.css
  do: Report item V02. Selecting a chart interval selects the same media and evidence; filters persist across views; gaps, uncertainty and accessible tables are available. A chart becomes an authoring surface rather than a picture. Acceptance (legacy): (agent-executable): brushing a chart interval selects the corresponding timeline range and evidence; a filter persists across a navigation; every chart has an accessible table equivalent; a gap renders as a gap. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace linked_brushing_annotations 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): link chart brushing, filtering and evidence across views

- [ ] 5-A3 — Reusable creator dashboards and insights boards
  lane: L4
  needs: 4-B1, 5-A1, 5-A2
  touches: crates/dataviz/**, crates/strivo-web/assets/spa/**
  do: Report item V03. Save, pin and share a local board with named filters and pinned evidence; empty, partial and stale states stay clear. Recommendations attach to a new project or template rather than floating free. Acceptance (legacy): (agent-executable): a saved board restores its filters, evidence pins and staleness state; an empty board explains what would populate it; a stale board says so. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace creator_dashboards 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): add reusable creator dashboards and insights boards

- [ ] 5-A4 — Branded chart assets inside compositions
  lane: L4
  needs: 1-D12, 1-D14, 3-B2, 4-B1, 5-A2, 5-A3
  touches: crates/dataviz/src/lib.rs
  do: Report item V05. A chart becomes an editable overlay backed by a frozen query and data snapshot; frame-stable render matches preview; citation text is retained. A later provider correction must mark the chart stale, not silently change a scheduled video. Acceptance (legacy): (agent-executable): a chart overlay renders identically in preview and export; the underlying data snapshot and its query are embedded; a data correction marks the artifact stale rather than re-rendering it; the citation text survives into the render. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT re-render a published figure from changed data
  qa: `cargo test --locked --workspace branded_chart_snapshots 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): add branded chart assets with frozen data snapshots

#### M5-B — Calendar, portable packages and exports
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace calendar_recommendations 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 5-B1 — Calendar recommendations with measured support and cold-start handling
  lane: L5
  needs: 3-A4, 4-B1, 5-A3
  touches: crates/schedule-optimizer/src/lib.rs, crates/strivo-web/assets/spa/023-creator.js, crates/strivo-web/src/routes/plugins.rs
  do: Report item A08. Compare supported historical slots against a baseline, showing sample size, timezone and confounders; a low-data account receives "insufficient evidence". The existing `crates/schedule-optimizer` is metric-agnostic by design (`src/lib.rs:17-20`) and its three real feeders are all self-referential — recording durations, chat density and heatmap's own fused proxy (`assets/spa/023-creator.js:164-266`) — so today "best time to stream" is proxies all the way down. Once todo 72 lands, real outcomes become an input. Acceptance (legacy): (agent-executable): a recommendation states its sample size, timezone and whether its input was measured or derived; an account below the evidence threshold receives "insufficient evidence" rather than a ranking. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT recommend a slot from proxy data without saying it is a proxy
  qa: `cargo test --locked --workspace calendar_recommendations 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): add calendar recommendations with measured support and cold-start handling

- [ ] 5-B2 — Cross-platform review surfaces
  lane: L1
  needs: 1-D2, 1-D12, 4-A1, 4-A2, 5-A1, 5-A3, 5-B1
  touches: crates/strivo-web/assets/spa/**, crates/strivo-web/src/routes/**
  do: Report item A08-adjacent (the manifest has A01–A08; there is no A09) plus `§8.2`'s archive map. What is ready, scheduled, published or blocked, per account and per destination, with per-account failures inspectable. Every row must make clear whether a post went out through a connector or was marked manually by the user — todo 64's provenance carries that distinction. Acceptance (legacy): (agent-executable): every post shows its destination, account, timezone and state; a per-account failure names its cause; a manual post is visually distinct from a connector post. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace cross_platform_review_surfaces 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add cross-platform review surfaces with per-account state

- [ ] 5-B3 — Portable project and media packages
  lane: L4
  needs: 1-D15, 3-B2
  touches: crates/research/src/lib.rs
  do: Report item R06. Export and import a supported subset with sources, edit and caption state and attribution; unsupported transforms are reported with a rendered fallback rather than dropped. This is the "Jellyfin for streaming platforms" promise made portable. Acceptance (legacy): (agent-executable): a round trip restores sources, edit state, captions and attribution; an unsupported transform is reported and rendered as a fallback; a package is self-describing with a version. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace portable_project_packages 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(export): add portable project and media packages

- [ ] 5-B4 — Reproducible data and figure exports
  lane: L1
  needs: 4-B1, 4-B2, 5-A1, 5-A3
  touches: crates/strivo-web/assets/spa/015-creator.js, crates/strivo-web/assets/spa-css/000-pvr.css
  do: Report item V06. Export CSV and JSON plus supported SVG, PNG and PDF with units, filters, snapshot and version, and accessibility notes; reopening reproduces the figure exactly. Acceptance (legacy): (agent-executable): two exports of the same snapshot are byte-identical or pixel-identical; the export carries its query, units, filters and version; an accessible text alternative is included. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT produce a figure that cannot be regenerated identically
  qa: `cargo test --locked --workspace reproducible_figure_exports 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): add reproducible data and figure exports

- [ ] 5-B5 — Outcome-informed recommendations and experiments
  lane: L4
  needs: 4-B2
  touches: crates/research/**
  do: Report item A07. Recommendations show evidence and counterexamples; no unsupported causal or "viral" guarantee; experiment setup distinguishes random assignment from observation. `docs/CREATOR-EDITION-REPORT-2026-09-07.md:23` explicitly forbids optimising for an unvalidated virality score. Acceptance (legacy): (agent-executable): a recommendation states its evidence and at least one counterexample; an experiment records its assignment method and sample size; an observational comparison is labelled observational. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT present an observational comparison as an experiment
  qa: `cargo test --locked --workspace outcome_informed_experiments 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(analytics): add outcome-informed recommendations with evidence and experiments

#### M5-C — Collaboration, performance, onboarding and field validation
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace review_comments_roles 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 5-C1 — Optional small-team roles, revision-safe review comments and review sharing
  lane: L4
  needs: 1-D15, 1-D22, 3-A1, 5-B1, 5-B2
  touches: crates/research/**, crates/strivo-web/src/routes/**
  do: Report items E10 (`:315`, review comments, range notes and approval workflow) and O06 (`:378`, small-team roles). Explicit viewer, editor and publisher permissions; a comment attaches to an exact revision and range; changing an approved revision invalidates the approval; no credential is exposed through sharing. Hosted collaboration remains a separate decision and is out of scope. Acceptance (legacy): (agent-executable): each role is enforced by a test; a comment attaches to an exact revision and range; changing an approved revision invalidates the approval; no credential is exposed by sharing. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT expose raw credentials through sharing
  qa: `cargo test --locked --workspace review_comments_roles 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(collaboration): add optional local small-team roles and revision-safe review

- [ ] 5-C2 — Resource-aware acceleration, cache invalidation and long-job recovery
  lane: L3
  needs: 3-C1, 5-B3
  touches: src/pipeline/executor.rs
  do: Report item R07. Compare CPU and GPU output and throughput on declared hardware; memory, disk and concurrency budgets hold while the PVR records; a stale cache never masquerades as current output. Build on todo 24's content-addressed cache keys and todo 13's media pool. Acceptance (legacy): (agent-executable): a throughput comparison on declared hardware is recorded; a concurrent capture-plus-render soak holds every declared budget; a stale cache entry is detected and rebuilt rather than served. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT silently fall back to a slower path without saying so
  qa: `cargo test --locked --workspace resource_aware_acceleration 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: perf(media): add resource-aware acceleration and long-job recovery

- [ ] 5-C3 — Performance, accessibility and cross-platform qualification
  lane: L5
  needs: 1-C15, 1-C16, 1-C25, 1-C30, 1-D9, 3-B3, 5-B3, 5-B4
  touches: scripts/check-windows.sh
  do: Report item O04. Record hardware, browser and tool versions; meet the agreed interaction and media budgets on supported systems; capture and render soak tests include contention. Note the Safari audio-WebCodecs constraint from todo 43 is a permanent, documented platform qualification item, not a bug to be papered over. Cross-check the Windows target, which `ROADMAP.md:253` records as the real remaining release risk. Acceptance (legacy): (agent-executable): every budget in `docs/CREATOR-EDITION-REPORT-2026-09-07.md:405-415` has a measured value or an explicit, justified exception; the Windows cross-check and the release pipeline's real Windows build both pass (`ROADMAP.md:253-254`); **the Safari audio-WebCodecs gap is qualified as a permanent platform limitation, not a bug** — the degraded-mode contract from todo 43 is unit-tested and its behaviour asserted against that contract, since the Playwright harness is Chromium-only and this plan does not add a WebKit lane. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace qualification_matrix 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: test(perf): qualify performance, accessibility and cross-platform support

- [ ] 5-C4 — Onboarding, a capability doctor and actionable diagnostics
  lane: L3
  needs: 1-C20, 1-C27, 1-D3, 3-A1
  touches: src/lib.rs, src/tools.rs
  do: Report item O05. A new installation reaches a first useful export; missing tools, providers or accounts show the exact next step; support bundles redact secrets. Build on the PVR's existing `strivo doctor` and todo 20's capability descriptor and todo 37's readiness checklist. Acceptance (legacy): (agent-executable): a clean install reaches a first useful export with no terminal step; every missing dependency names itself and the fix; a support bundle contains no secret. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT send a user to a terminal for something the UI can do, which is the exact class of defect F-03 recorded
  qa: `cargo test --locked --workspace capability_doctor 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(onboarding): add a capability doctor and actionable diagnostics

- [ ] 5-C5 — Field validation instrumentation and prioritisation evidence
  lane: L2
  needs: 1-C15, 1-C16, 3-C2, 5-C3, 3-C3
  touches: crates/strivo-web/src/telemetry.rs
  do: Report item O08. `docs/CREATOR-EDITION-REPORT-2026-09-07.md:415` requires five creators completing the first-source-to-approved-output task without engineer intervention, recording time, errors and rework, then repeating after revisions. Build the instrumentation for that study as part of this plan — local, opt-in, content-free, resettable and exportable per `docs/RESEARCH-PLATFORM-ROADMAP.md:76-78` — even though running the study is the user's call, not a todo. This is the last todo because it depends on the whole loop existing. Acceptance (legacy): (agent-executable): the instrumentation records time-to-first-approved-output, suggestions accepted versus rejected, manual editing time, rework and render failures, with no corpus content; export produces a study-ready dataset; reset is clean. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace field_validation_instrumentation 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(insights): add field-validation instrumentation for creator outcome studies


### M6 — Side waves: transcript intelligence, translation, datasets and destinations (legacy 6b-11b, deferred)
epoch: Transcript and moment intelligence, translation aids, real-media contract coverage, external datasets and the extra destinations are in place. Deferred behind M3 and M5: no commits in the window; carried verbatim from .omo side waves 6b-11b.
lanes: L1 (strivo-web assets, SPA and e2e), L2 (strivo-web server routes and tests), L3 (crates/editor render and timeline), L4 (crates/research and crates/dataviz), L5 (crates/strivo-plugins and crates/pipelines-dag), L6 (src/platform, src/licence and licence-backend), L7 (other crates)
gates: .keel/gates/M6.json

#### M6-A — Transcript and moment intelligence
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace canonical_transcript_revisions 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 6-A1 — Canonical word and segment transcript revisions with provider capabilities (I01)
  lane: L5
  needs: 1-C21, 1-C22
  touches: crates/strivo-plugins/src/crunchr/db.rs, crates/strivo-plugins/src/crunchr/transcribe/**, crates/research/src/lib.rs, crates/strivo-web/assets/spa/025-creator.js
  do: Report item I01 (`:293`). Timed words get stable IDs and provenance; providers without word timing are explicitly degraded, never faked; human corrections survive a re-run. The existing backends are Whisper CLI, WhisperX and Voxtral (`crates/strivo-plugins/src/crunchr/transcribe/`) with a 846-line `db.rs` — this makes their output a first-class revisioned object rather than a plugin-private table. Acceptance (legacy): (agent-executable): a fixture proves stable word IDs across two runs of the same input; a segment-only provider renders as degraded with its limitation named; a corrected transcript survives a re-transcribe. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT let a re-transcribe silently discard a human correction
  qa: `cargo test --locked --workspace canonical_transcript_revisions 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(crunchr): make timed transcripts versioned, provenance-bearing objects

- [ ] 6-A2 — Speaker correction, glossary, language selection and identity scope (I02)
  lane: L7
  needs: 6-A1
  touches: crates/insights-compare/src/lib.rs
  do: Report item I02 (`:294`). Rename, merge and split speaker labels without corrupting timing; cross-recording speaker identity requires confirmation or evidence and is never inferred — `docs/CREATOR-EDITION-REPORT-2026-09-07.md:234` is explicit that diarization labels are not globally resolved people. A glossary improves a measured fixture set. Acceptance (legacy): (agent-executable): merge and split preserve every word's timing; a cross-recording merge requires explicit confirmation; a glossary measurably improves transcript output on a fixed fixture set, with the delta recorded. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT carry an identity across recordings on its own
  qa: `cargo test --locked --workspace speaker_correction_glossary 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(crunchr): add speaker correction, glossary and scoped identity

- [ ] 6-A3 — Multimodal moment ranking and narrative boundaries (I03)
  lane: L7
  needs: 6-A1, 6-A2
  touches: crates/clipper/src/lib.rs, crates/cuepoints/src/lib.rs, crates/chat-density/src/lib.rs, crates/research/src/moments.rs
  do: Report item I03 (`:295`). Combine available speech, chat, visual and manual signals; show rationale, context and uncertainty; compare accepted suggestions against a simple baseline. This is the intelligence behind the moment inbox and the clip candidates. Acceptance (legacy): (agent-executable): every ranked moment carries its contributing signals and a confidence; the baseline comparison is recorded; a moment with one signal is visibly weaker-sourced than one with three. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT present a rank as a fact
  qa: `cargo test --locked --workspace multimodal_moment_ranking 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(research): rank moments across speech, chat, visual and manual signals

- [ ] 6-A4 — The moment inbox: accept, reject, handles, tags and deduplication (I04)
  lane: L1
  needs: 6-A1, 6-A2, 6-A3
  touches: crates/strivo-web/assets/spa/017-creator.js, crates/research/src/moments.rs
  do: Report item I04 (`:296`), and the P0 prerequisite for I06 and A06. Accepting a candidate opens an editable source span; rejection is remembered so the same bad suggestion is not re-proposed; overlapping suggestions can be merged or deliberately kept apart. This is the human-in-the-loop surface the strategy calls the moment/clip-review workflow. Acceptance (legacy): (agent-executable): accepting opens the editor at the exact source span; a rejected candidate is not re-proposed; two overlapping candidates can be merged or kept separate, and both outcomes persist. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT silently drop a rejected candidate without recording the rejection
  qa: `cargo test --locked --workspace moment_inbox 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(research): add the moment inbox with remembered rejections

- [ ] 6-A5 — Creator-guided AI rough cut and metadata drafts (I06)
  lane: L3
  needs: 1-D11, 6-A1, 6-A2, 6-A3, 6-A4
  touches: crates/editor/src/lib.rs, crates/reuse/src/lib.rs
  do: Report item I06 (`:298`). A prompt produces an inspectable edit proposal with source-linked choices; applying it is undoable and never directly publishes. Every proposed cut resolves to a source span the user can see before accepting. Acceptance (legacy): (agent-executable): a proposal lists each proposed cut with its source span and reason; applying is one undo step; no proposal path reaches a publish action. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace ai_rough_cut_proposals 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(editor): add inspectable AI rough-cut proposals that never auto-publish

- [ ] 6-A6 — Provider setup, local execution, external-processing consent and cost controls (I07)
  lane: L5
  needs: 6-A1, 6-A2, 6-A3, 6-A4, 6-A5
  touches: crates/strivo-plugins/src/crunchr/transcribe/voxtral_api.rs, crates/strivo-plugins/src/crunchr/transcribe/voxtral_openrouter.rs, src/pipeline/stage.rs
  do: Report item I07 (`:299`), and a P0 prerequisite for E11, O02 and O05. Show prerequisites, the destination of the data, estimated usage and a budget before any external processing; a missing model or key fails actionably; local mode is testable. Builds on the existing five backends in `crates/strivo-plugins/src/crunchr/transcribe/` and `Stage.cost_cents_estimate` (`src/pipeline/stage.rs:143`). This is the consent surface `docs/RESEARCH-PLATFORM-ROADMAP.md:76-78` requires. Acceptance (legacy): (agent-executable): a local-only run makes zero outbound requests (asserted by a network-deny harness); an external run requires a recorded consent decision naming the destination; a missing key fails with the provider and the fix named; the cost estimate is shown before execution. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT send anything to an external provider without recorded consent
  qa: `cargo test --locked --workspace external_processing_consent 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(crunchr): gate external processing behind explicit consent and cost disclosure

- [ ] 6-A7 — Creator-specific evaluation and feedback datasets (I08)
  lane: L1
  needs: 6-A3, 6-A4
  touches: crates/strivo-web/e2e/tests-real/real-server.spec.ts
  do: Report item I08 (`:300`). Measure transcript and alignment error, moment acceptance, harmful context loss, and ranking calibration across speech-heavy and visual streams. This is the evidence that the intelligence is actually working, and it feeds todo 89's field study. Acceptance (legacy): (agent-executable): each of the four metrics has a computed baseline on a fixed corpus; the corpus is committed, reproducible, and free of personal data; ranking calibration is reported as a number, not a claim. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  must-not: Must NOT contain personal data
  qa: `cargo test --locked --workspace creator_eval_datasets 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: test(research): add creator-specific evaluation datasets and baselines

#### M6-B — Translation and datasets
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace translation_aids 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 6-B1 — Translation, optional speech separation and correction aids (E11)
  lane: L7
  needs: 1-D11, 6-A6
  touches: crates/captions/src/lib.rs, crates/multitrack/src/lib.rs
  do: Report item E11 (`:317`). Each enabled backend is evaluated and labelled; translated timings remain editable; synthetic or separated audio is distinguished from captured stems in the UI. Note the current translator is an identity function and must be labelled as such, not presented as translation. `docs/CREATOR-EDITION-REPORT-2026-09-07.md:318` requires the label because a creator must never mistake generated audio for what was captured. Acceptance (legacy): (agent-executable): each backend is evaluated on a fixture and labelled with its measured result; a translated caption's timings are editable and re-export correctly; a separated stem is labelled synthetic in the UI and in the export. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace translation_aids 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(captions): add evaluated translation and separation backends with honest labelling

- [ ] 6-B2 — External dataset import and transforms for visual stories (V04)
  lane: L4
  needs: 3-A2, 5-A1, 5-B1
  touches: crates/dataviz/**
  do: Report item V04 (`:365`). Import CSV and JSON, correct types, units and timezones with the user, validate joins, and retain attribution. Unsafe content is treated as data, never as markup or script — this is the security-relevant half of the todo. This is the P0 prerequisite for todo 79's chart assets, which must carry a frozen external-data snapshot. Acceptance (legacy): (agent-executable): a CSV and a JSON import both round-trip; a wrong timezone is corrected by the user and the correction persists; a malformed join is reported, not silently dropped; a cell containing `<script>` renders as text and is escaped everywhere it appears. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace external_dataset_import 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(dataviz): add safe external dataset import with attribution

#### M6-C — Additional destinations and post lifecycle
exit:
- [ ] The phase's legacy acceptance holds at the commit that lands it | proof: `cargo test --locked --workspace tiktok_instagram_connectors 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
- [ ] Every claim in the phase's todos is shown, not self-reported: named file changed, named command run | verify: auditor

todos:
- [ ] 6-C1 — TikTok and Instagram/Reels connector programs (P06)
  lane: L2
  needs: 1-D22, 3-A1, 3-A2
  touches: crates/strivo-web/src/routes/licence.rs
  do: Report item P06 (`:340`). Verify current API eligibility, audit status and user-control requirements before promising anything, and expose actual direct-post / upload-only / manual capabilities and remote states. The researched contracts: TikTok `POST /v2/post/publish/video/init/` with scope `video.publish`, unaudited apps are SELF_ONLY/private capped at 5 users/24h, max 10 min and 4 GB, 5–64 MB sequential chunks, `upload_url` valid 1 hour. Instagram `instagram_business_content_publish`, with resumable upload only for apps that implemented Facebook Login for Business, Reels 15 min / 300 MB, and Standard vs Advanced access (Advanced needs App Review + Business Verification). Per the approved credential decision, both are verified against local contract-test servers; the real-account step is a named, explicitly blocked acceptance item. Acceptance (legacy): (agent-executable): each destination declares its real capability set; an unsupported permission is reported as unsupported rather than attempted; a per-account duration limit is read from the destination's declared capabilities, never hard-coded; no test requires a live account. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace tiktok_instagram_connectors 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add TikTok and Instagram Reels connector programs

- [ ] 6-C2 — Additional destinations: podcast/hosting, Patreon, blog/newsletter (P07)
  lane: L6
  needs: 1-D22, 3-A1, 3-A2, 6-C1
  touches: src/platform/patreon.rs
  do: Report item P07 (`:341`). Each connector passes its own contract test; a manual-package fallback is labelled as such; read access is never mistaken for publish authority — the exact mistake this plan exists to prevent, since StriVo already reads Patreon for capture. State for each whether it supports direct posting, hosting integration, or a manual package only. Acceptance (legacy): (agent-executable): each destination declares `direct_post | upload_only | manual_package`; a read-scoped account cannot be offered a post action; every manual fallback is labelled in the UI and in the exported package. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace additional_destinations 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add additional destinations with honest capability declarations

- [ ] 6-C3 — Batch operations, policy-controlled automation and post maintenance (P09)
  lane: L2
  needs: 3-A2, 6-C2
  touches: crates/strivo-web/src/routes/**
  do: Report item P09 (`:343`). Preview targets before any bulk action; retries and metadata updates are accountable; removal and replacement limitations are explicit; drafts never auto-post by default — automation requires an explicit, visible policy the user enabled. A scheduled release is never silently altered by a later edit (todo 59's rule binds here). Acceptance (legacy): (agent-executable): a batch previews every target before acting and requires confirmation; a per-item failure is attributed to exactly one item; with automation off, no draft is ever posted; a bulk action is individually reversible or explicitly reported as not reversible. The qa names the test to write first; no test of that name exists at 6a2cd1c (checked against the 1659 test names the workspace lists).
  qa: `cargo test --locked --workspace batch_policy_automation 2>&1|tee /dev/stderr|grep -Eq 'test result: ok\. [1-9][0-9]* passed'`
  commit: feat(publish): add accountable batch operations behind explicit automation policy



## Unplanned activity

Clusters of recent commits that no open todo's `touches` covers and that are not landed history. Each row names its disposition: `plan` (todos follow), `fold <todo>`, or `ignore <reason>`.

- src/monitor/patreon.rs — 2 commits, 1 paths since 4w (under src/monitor) | disposition: ignore shipped PVR auto-download, 757a870 and c5f0d63 (2026-09-18); outside the Creator plan, not in the ledger
- src/monitor/uploads.rs — 2 commits, 1 paths since 4w (under src/monitor) | disposition: ignore shipped PVR auto-download, 757a870 and c5f0d63 (2026-09-18); outside the Creator plan, not in the ledger
- src/webhook.rs — 2 commits, 1 paths since 4w (under src/) | disposition: ignore shipped PVR VOD progress fix, f700f54 (2026-09-18); outside the Creator plan
- Cargo.toml — 1 commits, 1 paths since 4w | disposition: ignore dependabot merges 0389468, 0241129 and 7c05cd6 (2026-09-29); dependency bumps are checked by the CI check job, not by plan todos
- package-lock.json — 1 commits, 1 paths since 4w | disposition: ignore dependabot bump a6b7166 (2026-10-04), marketing-site Next.js; no plan content
- package.json — 1 commits, 1 paths since 4w | disposition: ignore dependabot bump a6b7166 (2026-10-04), marketing-site Next.js; no plan content
- src/events.rs — 1 commits, 1 paths since 4w (under src/) | disposition: ignore shipped PVR recording fix, f700f54 (2026-09-18); outside the Creator plan
- src/intents — 1 commits, 2 paths since 4w | disposition: ignore shipped PVR channel_id threading, c5f0d63 (2026-09-18); outside the Creator plan
- src/platform/mod.rs — 1 commits, 1 paths since 4w (under src/platform) | disposition: ignore shipped PVR monitor work, 757a870 (2026-09-18); the plan's connector todos name no file here
- src/platform/twitch.rs — 1 commits, 1 paths since 4w (under src/platform) | disposition: ignore shipped PVR monitor work, 757a870 (2026-09-18); the plan's connector todos name no file here
- tests/config_roundtrip.rs — 1 commits, 1 paths since 4w (under tests/) | disposition: ignore shipped PVR auto-download config test, 757a870 (2026-09-18)
