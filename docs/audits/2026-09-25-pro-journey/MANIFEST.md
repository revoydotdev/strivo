# Pro journey register — 2026-09-26

Plan todo 16. Read-only audit of the Creator-edition journey — licence →
transcribe → pipelines → editor → publish — against a real
`--features creator` binary. **Nothing was fixed.**

Narrative: [REPORT.md](REPORT.md) · machine-readable: [MANIFEST.json](MANIFEST.json) ·
validator: [`validate.mjs`](validate.mjs)

---

## Shape

| | |
| --- | --- |
| Head | `1053082` |
| Binary | `/tmp/opencode/strivo-creator-fresh` |
| sha256 | `0406502bb142c20d0c82b71bbc7ded28…` |
| Edition proven by | GET /api/v1/plugins/descriptor → |
| Findings | 7 |
| Kept promises | 10 |
| Observations | 4 |
| Declared limitations | 5 |

## Why the ids are namespaced

Two prior audits issue ids in the `R01`–`R07` range and the PVR sibling
(todo 15) uses bare `PJ-nn`. A bare `PJ-01` would therefore be ambiguous, and
a validator cannot assert "appears exactly once" over an ambiguous key.
These rows use **`PJ-Cnn`** — the PVR namespace with a `-C` suffix — which is
disjoint from every existing register. `validate.mjs` **A4** fails the build if
that ever stops being true.

## Verdict vocabulary

| Verdict | Meaning |
| --- | --- |
| `confirmed` | Re-checked against the running binary. The defect exists as described. |
| `broken` | A promise the UI makes about backend behaviour that the backend does not keep. Every one carries a reproduction. |
| `kept` | A promise the UI makes that the backend does keep, with the request that proves it. |
| `by-design` | The behaviour is deliberate, cited to the code that decides it, and must not be actioned as a defect. |
| `unreachable` | The state cannot be observed in any build available here, with the blocking code cited. |
| `not-applicable` | An adversarial class that does not apply to this work, with a one-line reason. |

## Findings

### PJ-C01 — The entire Pro journey is unreachable in every build that exists, including a Creator-featured one

`by-design` · severity high · `crates/strivo-web/src/routes/settings.rs:639`

**Promise.** The topnav offers Studio, Analytics, Publish, Pipelines, Plugins, DataViz and Archive as first-class destinations (012-pvr.js:100-103), and the router accepts all seven as deep links (008-pvr.js:319-326).

**Actual.** "creator_enabled": false is hardcoded, so the SPA closes the surfaces twice over: chrome() filters all seven entries out of the nav (012-pvr.js:128-130), and render() hard-redirects all seven routes to #/library, rewriting the hash (008-pvr.js:562-565). Every one of the seven was observed to land on the Home page with location.hash === '#/library'.

**Why by design.** The comment above the line says so: 'Creator work is not publicly available, including in a feature-enabled build. Keep the SPA on the released PVR surface until the product clears its security/release gate.' The same condition is published in docs/LICENCE-BACKEND-DEPLOY.md. Nothing here should be actioned as a defect.

**Consequence.** The finding is the SCOPE, not the flag. A Pro-journey audit on a Creator binary has zero reachable UI, so every pane-content question below is unobservable in the product as shipped.

**Reproduction.** STRIVO_BIN=<creator binary> npx playwright test -c playwright.creator.config.ts -g 'creator_enabled'; the test asserts each of the seven routes bounces to '#/library' and that no nav entry carries the route.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-licence.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `WS/CE02` | closed | WS/CE02 closed the observation that routes::plugins is cfg-gated at the module declaration and therefore unreachable in a PVR build. This is the opposite question in the opposite build: on a binary where the module IS compiled in, the SPA still refuses every Creator route. cfg-gating and a hardcoded-false flag are different mechanisms in different files, and closing CE02 says nothing about either. |
| `WS/CE06` | closed | WS/CE06 closed the fact that routes::licence is cfg-gated and that the refresh loop no longer spawns unconditionally. That is about which code exists; this is about a value the server reports to the SPA, and the SPA's two refusals are in 008-pvr.js and 012-pvr.js, neither of which CE06 touched. |

---

### PJ-C02 — F-12 RE-CONFIRMED, with the register's arithmetic corrected

`confirmed` · severity medium · `crates/strivo-web/assets/spa/021-creator.js:64`

**Promise.** PRO_PANES presents 20 tabs across three panes as the unified Pro app, where each pane is 'a single page with a tab strip across the top' (021-creator.js:58-62).

**Actual.** Of the 20 tabs, 5 carry a real route (:69 editor, :79 insights, :80 viewguard, :84 dataviz, :91 schedule-optimizer) and 15 are declared `route: null`. Two of those 15 — `ab` (:71) and `submix` (:72) — are diverted to a mount function by the `inlinePane` branch at :117, so they render genuine controls. The remaining 13 render only the signpost at :129-136: the 'This tool is reached from inside the Editor view' paragraph and a raw `POST /api/v1/plugins/<slug>/<recording_id>` snippet.

**Corrections to the consolidated register.**

- PHASE0/F-12 records '14 of 20 tabs are still route: null'. It is 15, not 14.
- PHASE0/F-12 records 'Studio 2/4 routed'. Zero Studio tabs carry a `route`; three of four render controls (editor routes, and ab/submix mount inline panes). 'Routed' and 'renders a control' are different facts and the register conflated them.
- The plan's own counts — Studio 3/4, Analytics 3/6, Publish 9/10 as route:null — are exactly right. F-12 is CONFIRMED in substance; only the register's derived numbers were wrong.

**Reproduction.** The spec walks all 20 tabs, asserts each renders exactly one of the three mutually-exclusive body shapes, and asserts the rendered route list equals the five expected slugs. The signpost spec then asserts the signpost text and the exact `POST /api/v1/plugins/<slug>/<recording_id>` string for each of the 13.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-panes.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `PHASE0/F-12` | open | Not a distinct thing — this row RE-CONFIRMS the same defect against current source and the running binary, and corrects its arithmetic. It is recorded as a separate row only because it adds a runtime reproduction and a 20/15/5/13 breakdown the source-only row could not have. |

---

### PJ-C03 — The signpost's API reference is wrong for six of the thirteen tabs it is shown on

`broken` · severity high · `crates/strivo-web/assets/spa/021-creator.js:134`

**Promise.** The `<details>API reference</details>` block tells the user the exact call to make: `POST /api/v1/plugins/<slug>/<recording_id>`.

**Actual.** Followed literally, that call fails or lies for six of thirteen slugs. Wrong METHOD — the real route is a GET, so the advertised POST returns 405 with an empty body: heatmap, casebook, reuse, multitrack, brandsafe. Wrong PATH — clipper's POST lives at /clipper/{id}/analyze, so the advertised bare path is not clipper's route at all. Only scenes, structure, chat-density, thumbnails, chapters, broll and cuepoints (7 of 13) match as printed. There is also no body: the real RPC route requires {selection:[uuid…]}, which the snippet does not mention.

**Control.** CONTROL: on the same server and the same session, the advertised call for the other seven slugs (scenes, structure, chat-density, thumbnails, chapters, broll, cuepoints) is NOT 404 and NOT 405 — it reaches its handler. So the six failures are a per-slug fact about the real routes, not a server that refuses everything. A second control: the same five slugs answer 200 or 404 on GET, where 404 would mean the recording is absent rather than the method wrong.

**Reproduction.** For each of the 7 kept slugs, POST to the advertised path and assert the status is neither 404 nor 405. For the 5 wrong-method slugs, assert POST returns exactly 405 and that the same path with GET does not. For clipper, assert the bare path does not answer as clipper and that /analyze does.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-failure.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `PHASE0/F-13` | closed | F-13 was closed because the B-roll finder's Info-modal trigger exists (029-creator.js:151) and POSTs correctly. It is about a working control. This row is about a different surface — the #/publish/broll tab's signpost — printing a call that 405s. The endpoint F-13 remediated is reachable and correct; the signpost that also names 'broll' is not. A user following the signpost never reaches the control F-13 verified. |

---

### PJ-C04 — A signpost tab explains no state at all — the named failure scenario's answer

`broken` · severity high · `crates/strivo-web/assets/spa/021-creator.js:131`

**Promise.** The plan's QA failure scenario: navigating a Pro tab with no entitlement must explain the state rather than render a signpost.

**Actual.** It depends which tab, and the split is the finding. A Pro-ROUTED tab does explain: #/analytics/insights leads to /plugins/insights, the 402 arrives, and renderPlugins() mounts the upsell card (021-creator.js:445-459) saying 'Creator Edition work is not released or supported' and 'No trial, activation, or purchase path is available'. A SIGPOST tab explains nothing: #/publish/clipper renders a paragraph that contains no licence term, no entitlement term, no availability term and no Creator term. It also cannot be an entitlement story — clipper is Entitlement::Included with EntryPoint::Available, so the gate is not why it is inert. The honest reason is that the tab was never implemented as a pane, and the copy does not say that.

**Control.** CONTROL, in the same test on the same unlicensed machine: #/analytics/insights leads through to /plugins/insights, the 402 arrives, and the upsell card IS rendered and DOES name the release hold and the absence of any purchase path. So the SPA is capable of explaining this state; the signpost branch simply does not try.

**Reproduction.** The spec drives both halves in one test, so the contrast is the evidence: the routed tab's upsell copy is asserted positively, then the signpost tab is asserted to contain none of the four categories of explanation.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-failure.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `PHASE0/F-11` | open | F-11 is that the SPA never reads the backend's `licence.implemented` flag, so the UPGRADE CARD on the plugin hub cannot distinguish 'unavailable because of the hold' from 'unavailable because no service is configured'. This row is about a different renderer — the Pro-pane signpost — which never calls licenceStatus() at all and so has no flag to ignore. Different component, different missing information. |

---

### PJ-C05 — POST /api/v1/plugins/{plugin}/{verb} answers 202 'queued' for a plugin that does not exist

`broken` · severity high · `crates/strivo-web/src/routes/api.rs:847`

**Promise.** The route dispatches a verb to a registered plugin in the daemon plugin host.

**Actual.** It dispatches to the host without checking that the plugin is registered, and answers 202 {"status":"queued","plugin":"<name>"}. A request for plugin 'definitely-not-a-plugin' returns 202 queued. The verb is not validated either: a recording UUID is accepted as a verb name, which is exactly what PJ-C03's signpost snippet tells users to send. So a user who follows the signpost for clipper gets a success response for work that was never enqueued.

**Control.** CONTROL: on the same server the four Pro data routes still answer 402 and the ungated pipeline DAG still answers 200, so 202 is not what this server answers to everything. And the honest counterpart is the same generic route used correctly: the SPA's real calls pass a verb NAME, and those return 200. The false success is specific to an unvalidated plugin or verb name.

**Reproduction.** The spec asserts 202 + status:queued for a fabricated plugin name, and that the same call with a real recording id returns 202 with that id echoed back as the verb.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-failure.spec.ts`

**Cross-check note.** No overlap with the consolidated register. The upstream rows about the plugin RPC surface (WS/CE01-CE06) are all about edition cfg-gating and the descriptor's crate boundary; none of them observes the runtime answer to an unregistered plugin name, and none of them is open on that point. Stated explicitly because an empty cross-check must be a claim, not a gap.

---

### PJ-C06 — The plugin hub hides locked plugins, then tells the user to activate to see them, while advertising them as available underneath

`broken` · severity medium · `crates/strivo-web/src/routes/plugins.rs:3791`

**Promise.** The hub renders 'Activate Strivo Pro above to populate this grid' when the grid is empty (021-creator.js:564-569), and the capability matrix below it lists each contributing plugin with an available/roadmap state (021-creator.js:590-626).

**Actual.** GET /api/v1/plugins filters locked Pro plugins out entirely, so an unlicensed machine gets {"plugins":[]}. The grid is empty and the copy says activating will populate it. On the same paint, the matrix — from the UNGATED /api/v1/plugins/capabilities — names crunchr with status 'available', which is the very capability the gate answers 402 for. The page simultaneously asks for an activation and advertises the unlocked set. The server-side comment at :3782-3784 is honest about hiding them; the SPA copy above the matrix is not.

**Control.** CONTROL: the same matrix is not uniformly optimistic. Its publish_queue row reports reuse as 'available' and yt-publish as 'roadmap', which matches the descriptor exactly (capabilities.rs:451-457) and is the one honest verdict the matrix gives. So the contradiction is between the activation copy and the matrix, not a matrix that claims everything works.

**Reproduction.** Assert the grid is empty and the activation copy present, then assert a matrix chip whose title is exactly 'crunchr · available' is present, and that /api/v1/plugins/crunchr/recordings returns 402 in the same test.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-licence.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `PHASE0/F-11` | open | F-11 is about an unread backend flag. This row is about two rendered facts on one screen that contradict each other, and it holds even if `implemented` were read tomorrow. Different defect, same page. |

---

### PJ-C07 — The licensed half of the Pro journey cannot be observed at all

`unreachable` · severity medium · `crates/strivo-web/src/routes/licence.rs:87`

**Promise.** A Pro machine would transcribe (crunchr), compare streams (insights), score viewers (viewguard) and backfill a catalogue (archiver).

**Actual.** backend_url() returns None, so POST /api/v1/licence/activate and /trial cannot succeed, and todo 27's licence-cache reconnection is off by default. Every Pro data route therefore answers 402 for the whole of this audit. No transcription, comparison, fraud score or backfill was observed to run. Not recorded as passing.

**Reproduction.** Assert activate and trial both refuse and that a rejected key leaves entitled=false, then that all four Pro data routes answer 402 while the ungated DAG answers 200 — so the 402 is the gate, not a blanket refusal.

**Spec.** `crates/strivo-web/e2e/tests-real/creator-journey-licence.spec.ts`

**Cross-checked against the consolidated register.**

| Upstream | Its verdict | Why this is not a duplicate |
| --- | --- | --- |
| `PHASE0/F-16` | still-open | F-16 is that headless auto-transcribe has no SPA control, so the transcribe path cannot be automated even for a licensed user. This row is that no user can be licensed at all in this build, which blocks every Pro path including the manual one F-16 says does exist. F-16's control remains the fix for the licensed case; this row records that the licensed case has no instance. |

---

## Promises the backend keeps

| Path | Promise | Verdict | Proof |
| --- | --- | --- | --- |
| licence | GET /api/v1/licence/status reports the machine's real state. | **kept** | entitled=false, tier=free, trial=null, implemented=false, and both licence entry points refuse. Repeated reads agree, past the 2 s API cache TTL. |
| licence | A Pro capability is refused with 402, and the refusal names the capability. | **kept** | All four Pro data routes answer 402 with a body containing 'Pro plugin'; the ungated DAG answers 200 and an unauthenticated call answers 401, so the status is the gate and not auth. |
| licence | The entitlement set is derived from the descriptor, not hand-kept (021-creator.js:56-62 comment on the single source of truth). | **kept** | The served descriptor yields exactly pro=[crunchr, viewguard, insights, archiver] and 0 pvr-edition rows, and the gate refuses exactly those four. |
| transcribe | The hub's comment states locked Pro plugins never reach the SPA, so it only shows entitled or free ones (021-creator.js:508-510). | **kept** | GET /api/v1/plugins is exactly [] on an unlicensed machine, matching the comment. The comment is the accurate description of the code — the defect is the copy above the matrix, not this one. |
| pipelines | A node with no executor is labelled a blueprint and names the missing capabilities (013-creator.js:75-102). | **kept** | Every rendered node's tooltip says 'blueprint — no executor for …' or is marked runnable, and the rendered node count equals the server's summed node count. |
| pipelines | The DAG is public, not Pro-gated, so the page renders on free builds (api.rs:391). | **kept** | GET /api/v1/pipelines/dag answers 200 on an unlicensed machine, and the matrix endpoint answers 200 with more than twenty rows. |
| editor | The two route:null tabs that DO mount inline panes (ab, submix) render real controls. | **kept** | #/studio/ab renders the two variant slots, a disabled-until-both-slots compare button, and a picker populated with the seeded recording's real stream_title. #/studio/submix renders the track editor behind a selection, adds a track, and shows the composed filter_complex heading. |
| editor | The EDL editor tab routes to a real surface. | **kept** | #/studio/editor's button href is #/recordings and clicking it lands on the Recordings page with the seeded row rendered. |
| publish | The schedule-optimizer tab routes to a real plugin page. | **kept** | Its button href is #/plugins/schedule-optimizer. |
| publish | A Pro-routed tab refuses with an explanation rather than a signpost. | **kept** | Both #/plugins/insights and #/plugins/viewguard mount the upsell card naming the release hold and the absence of any purchase path. |

## Observations

### OBS-C01

playwright.real.config.ts and playwright.creator.config.ts share testDir ./tests-real, and Playwright's default testMatch is recursive, so the creator specs ARE collected by `npm run test:real`. An edition guard (creator-journey-helpers.ts::requireCreatorEdition) makes them skip against a PVR binary. Verified: the PVR lane exits 0 with 22 passed and 16 skipped. The alternative — adding them somewhere else — was rejected because the deliverable specifies this directory and this naming.

**Risk.** If a future lane removes the guard, the PVR lane fails with 16 errors that look like product defects. The guard is asserted, not assumed: the first test of each spec fails on a non-Creator binary instead of skipping.

### OBS-C02

real-server.sh's daemon readiness loop was 50 probes of 0.2 s. A Creator daemon initialises a six-plugin host before binding its socket; that is 0.6 s on an idle box but was measured at 12.4 s, 28 s and 55 s under this host's load average of 15-20, which made the shared harness flake. The loop is now STRIVO_E2E_DAEMON_PROBES, defaulting to the previous 50 so the PVR lane is unchanged, with the Creator config passing 900.

**Fix applied.** Two lines in real-server.sh plus one env entry. Widen-only, so no existing caller can be made to wait less.

### OBS-C03

Playwright kills real-server.sh's process tree with a plain kill, so the script's EXIT trap is best-effort and the daemon and the serve process both survived two runs. Verified and cleaned by PID each time; the residual check is part of this lane's cleanup receipt.

**Fix applied.** None applied. real-server.sh already self-heals stale sandboxes by age; the process sweep is the operator's, and killing by pattern is unsafe here because `pkill -f 'real-server.sh'` matches the shell issuing it.

### OBS-C04

The lane's own first cut asserted a tautology — `expect(labels.length).toBe(dag.body.pipelines.length > 0 ? labels.length : 0)` — which cannot fail. It was replaced with the server's summed node count as the denominator before the lane was ever run. Recorded because a validator that cannot fail is the recurring failure mode in this plan.

## Adversarial classes

| Class | Applies | How it was probed |
| --- | --- | --- |
| misleading success output | yes | Every endpoint verdict is read through Playwright's `request` context, never through the page, so API._fetch's 2 s GET cache cannot turn a broken route green. The plugin-hub 402 and the matrix's 'crunchr · available' are asserted in the same test on the same load. The clipper 202 false success is proved with a fabricated-plugin control that must ALSO return 202 queued. |
| stale state | yes | Licence status is read twice past the 2 s TTL and must agree and stay false. The PVR lane run re-reads a real cache rather than trusting one read. |
| hung or long commands | yes | webServer timeout 120 s with a 900-probe readiness budget; the one genuinely expensive call (clipper /analyze, 55-713 ms idle) has an explicit 180 s test timeout with the measurement recorded. |
| malformed input | yes | A fabricated licence key, a fabricated plugin name, an unknown capability slug, and the near-miss snake_case spelling are each driven and asserted to produce a distinct, explained outcome. |
| dirty worktree | yes | git status checked before and after; the commit carries an explicit path list and Cargo.lock, artifacts/ and .omo/ are never staged. |
| flaky tests | yes | Four consecutive green runs (exit 0, 16/16, ~13.6 s) after two real flakes were found and fixed from source. The PVR lane was then run once to confirm it still exits 0. |
| repeated interruptions | yes | Two runs were interrupted or timed out during development. Each left a daemon and a sandbox behind; both were found by PID-scoped sweep and removed, and the sweep is part of the lane's cleanup receipt. |
| prompt injection | **no** | No untrusted external text is ingested. The inputs are the local binary, its own seeded ffmpeg fixture, and a licence payload the sandbox itself creates. |
| cancel/resume | **no** | An audit adds no resumable user flow. The one stateful client-side structure, API._fetch's run cache, is covered by the stale-state class instead. |

## Limitations — what this audit did NOT verify

- No Pro capability was observed to execute. The licensed half of the journey is unobservable in this build (PJ-C07).
- Pane CONTENT was observed under the one-field instrumentation described above. Reachability, the entitlement gate, the plugin list, and every 402 were observed WITHOUT it.
- The pipeline DAG's nodes were observed as rendered labels and tooltips, not by running a pipeline; the executor needs a licensed machine.
- The A/B compare and sub-mix panes were observed to mount and save-ready their controls; the SSIM/VMAF render itself needs ffmpeg work and a licensed machine.
- clipper /analyze was executed and returned 200, but its findings were not inspected — a two-second synthetic clip has no highlights to find.

## Instrumentation

**What.** creator-journey-helpers.ts::openCreatorSurfaces rewrites ONE field of ONE response: GET /api/v1/settings has creator_enabled forced to true.

**Why.** The shipped product closes every Creator surface (PJ-C01), so no pane and no plugin hub can be observed at all without it.

**Does not change:**

- product source, the served bundle, or the binary
- entitlement — licence.entitled stays false, so every instrumented run is still an UNLICENSED machine
- any backend answer — the panes still meet real routes, real 402s and real data

**Observed without it:** `PJ-C01`, `PJ-C06`, `PJ-C07`

**Reversible.** Lives in the browser context; dies with the page.

