# The Pro journey, against a real Creator-featured binary — 2026-09-26

Plan todo 16. A read-only audit of the Creator-edition user journey —
**licence → transcribe → pipelines → editor → publish** — driven against a
binary compiled with `--features creator`. Nothing was fixed. The rebuild is
Phase 2 (todo 41, Waves 7/8).

Machine-readable register: [MANIFEST.json](MANIFEST.json) · human-readable
[MANIFEST.md](MANIFEST.md) · validator: [`validate.mjs`](validate.mjs)

| | |
| --- | --- |
| Head audited | `1053082` |
| Binary under test | `/tmp/opencode/strivo-creator-fresh`, sha256 `0406502bb142c20d…` |
| Edition verified by | served content, not `strings` |
| Journey steps audited | 5 of 5 |
| Findings | 7 (`by-design` 1, `confirmed` 1, `broken` 4, `unreachable` 1) |
| Kept promises | 10 |
| Upstream rows cross-checked | 6, of which 3 against `closed` items |
| Duplicate `closed` items re-reported | **0** |

---

## The edition was verified by content, and the first binary was stale

`GET /api/v1/plugins/descriptor` on the binary this lane booted:

```json
{ "edition": "creator", "version": 1, "capabilities": [ …39 rows… ] }
```

with `entitlement: pro` on exactly `crunchr`, `viewguard`, `insights`,
`archiver`; `entry_point: roadmap` on exactly `yt-publish`; and **zero**
`edition: pvr` rows.

A Creator binary already existed at `/tmp/opencode/strivo-creator-0.8.0`. It
answered **404** on the descriptor route. Its mtime is 21:02; commit
`eeadbc7` ("a single feature-capability descriptor for both editions") landed
at 21:48. The binary was 46 minutes older than the route it was being used to
test, so it was rebuilt. **A stale binary does not fail loudly — it 404s, and
a 404 reads like a finding.**

The rebuild went to `/tmp/opencode/creator-target`, never to the shared
`/scratch` target dir. The PVR binary other lanes use is byte-identical after
all of this work: `38cacd0f8351c1b1…` before and after.

## F-12: CONFIRMED, with the register's arithmetic corrected

F-12 is real and still live. Derived from `PRO_PANES` and then re-derived
from the rendered DOM:

| | count |
| --- | --- |
| tabs across the three panes | **20** |
| carrying a real `route` | **5** — `editor`, `insights`, `viewguard`, `dataviz`, `schedule-optimizer` |
| declared `route: null` | **15** |
| …of which mount real inline controls | **2** — `ab`, `submix`, via `inlinePane` at `021-creator.js:117` |
| …of which render the signpost | **13** |

The plan's counts (Studio 3/4, Analytics 3/6, Publish 9/10 as `route: null`)
are **exactly right**. Two numbers in the consolidated register's `PHASE0/F-12`
row are not:

- it says "**14** of 20 tabs are still `route: null`" — it is **15**;
- it says "Studio **2/4** routed" — **zero** Studio tabs carry a `route`. Three
  of four render controls, but one does it by routing and two by mounting an
  inline pane. "Routed" and "renders a control" are different facts, and the
  row conflated them.

The signpost itself is unchanged at `021-creator.js:129-136`: the
"This tool is reached from inside the Editor view" paragraph at `:131` and the
raw `POST /api/v1/plugins/<slug>/<recording_id>` snippet at `:134`.

## The scope finding: the whole Pro journey is unreachable

`GET /api/v1/settings` returns `"creator_enabled": false`, **hardcoded**, at
`crates/strivo-web/src/routes/settings.rs:639`. The same response still
carries the `archiver` config section, which proves the feature is compiled in
and this is a deliberate `false`.

The SPA closes the surfaces twice over:

- `012-pvr.js:128-130` — `chrome()` filters all seven Creator nav entries out;
- `008-pvr.js:562-565` — `render()` hard-redirects all seven Creator routes to
  `#/library`, rewriting the hash.

All seven were observed to land on Home with `location.hash === "#/library"`.
A bookmarked Pro deep link cannot reach its pane.

This is **by design** and the comment says so: *"Creator work is not publicly
available, including in a feature-enabled build. Keep the SPA on the released
PVR surface until the product clears its security/release gate."* Same
condition as `docs/LICENCE-BACKEND-DEPLOY.md`. Nothing here should be actioned
as a defect.

**The consequence is the audit's scope.** On a Creator binary there is no
reachable Pro UI at all, so every pane-content question is unobservable in the
product as shipped. That is why the register distinguishes what was observed
**without** instrumentation from what was observed **with** it.

## The named failure scenario: the answer is "it depends, and that is the finding"

> *navigate a Pro tab with no entitlement and confirm it explains the state
> rather than rendering a signpost*

- **A Pro-routed tab does explain.** `#/analytics/insights` → `#/plugins/insights`
  → the 402 arrives → `renderPlugins()` mounts the upsell card
  (`021-creator.js:445-459`): *"Creator Edition work is not released or
  supported"* and *"No trial, activation, or purchase path is available."*
- **A signpost tab explains nothing.** `#/publish/clipper` renders a paragraph
  with no licence term, no entitlement term, no availability term and no
  Creator term. It cannot be an entitlement story — `clipper` is
  `Entitlement::Included` with `EntryPoint::Available`, so the gate is not why
  it is inert. The honest reason is that the tab was never implemented as a
  pane, and the copy does not say that.

Both halves are driven in one test, so the contrast is the evidence.

## The five journey paths

| Path | States observed | Promises kept | Promises broken |
| --- | --- | --- | --- |
| **licence** | unlicensed; release hold; bad key; trial refused; activate refused | licence status is truthful; the 402 gate is real and names the capability; the Pro set is derived, not hand-kept | the hub says "Activate to populate this grid" while the matrix underneath advertises the gated plugins as `available` (PJ-C06) |
| **transcribe** | 402 on all four Pro data routes; the hub's list is `[]` | the hub's own comment is accurate — locked plugins really are filtered out server-side | not observable licensed at all (PJ-C07) |
| **pipelines** | DAG 200, ungated; nodes rendered `runnable` / `blueprint` | the blueprint verdict is honest and names the missing capabilities; the ungated design is as documented | the pane itself is unreachable in the shipped product (PJ-C01) |
| **editor** | `#/studio/editor` → Recordings; `#/studio/scenes` → signpost; `#/studio/ab`, `#/studio/submix` → real inline panes | both inline panes mount genuine, populated controls; the EDL tab routes to a real surface | the `scenes` signpost names an API by guesswork (PJ-C03) |
| **publish** | 1 of 10 tabs routes; 9 render signposts | `schedule-optimizer` routes to a real plugin page | 9 signposts; **6 of the 13 signpost API claims are wrong**; the generic RPC route answers `202 queued` for work never enqueued (PJ-C05) |

## What the signpost's "API reference" actually costs you

The `<details>API reference</details>` block shows one uniform call:
`POST /api/v1/plugins/<slug>/<recording_id>`. Followed literally:

| Slug | Real route | Method | Verdict |
| --- | --- | --- | --- |
| `scenes`, `structure`, `chat-density`, `thumbnails`, `chapters`, `broll`, `cuepoints` | `/<slug>/{id}` | POST | **kept** (7) |
| `heatmap`, `casebook`, `reuse`, `multitrack`, `brandsafe` | `/<slug>/{id}` | **GET** | **broken** — the advertised POST returns 405 with an empty body (5) |
| `clipper` | `/clipper/{id}/analyze` | POST | **broken** — the advertised bare path is not clipper's route |

No body is shown either; the real RPC route requires `{selection:[uuid…]}`.

And the trap behind it: `POST /api/v1/plugins/{plugin}/{verb}`
(`routes/api.rs:847`) does not check that the plugin is registered. A request
for `definitely-not-a-plugin` returns **202 `{"status":"queued"}`**. The verb
is not validated either — a recording UUID is accepted as a verb name, which
is exactly what the signpost tells users to send. A user who follows the
signpost for `clipper` gets a success response for work that was never
enqueued.

## Instrumentation, and what it does not change

`creator-journey-helpers.ts::openCreatorSurfaces` rewrites **one field of one
response**: `GET /api/v1/settings` with `creator_enabled` forced to `true`.

It does **not** change the product source, the served bundle or the binary; it
does not change entitlement — `licence.entitled` stays `false`, so every
instrumented run is still an unlicensed machine; and it changes no backend
answer, since the panes still meet real routes, real 402s and real data.

Observed **without** it: `PJ-C01` (reachability), `PJ-C05` (the RPC false
success), `PJ-C06` (the hub contradiction). Observed **with** it: the pane
contents, F-12's tab arithmetic, and the signpost copy.

## What was NOT verified

- No Pro capability was observed to **execute**. The licensed half of the
  journey has no instance in this build.
- Pipeline nodes were observed as rendered labels and tooltips, not by running
  a pipeline.
- The A/B compare and sub-mix panes were observed to mount; the SSIM/VMAF
  render was not run.
- `clipper /analyze` returned 200, but its findings were not inspected — a
  two-second synthetic clip has no highlights to find.

## The cross-check against the consolidated register

Six upstream rows were cross-checked, three of them against a `closed` item.
**No `closed` item is re-reported.** Each closed overlap carries a stated
reason it is a different thing, and `validate.mjs` fails the build if one is
missing (A7), if a recorded verdict disagrees with the register (A6), or if an
id the register never issued is cited (A5).

| This row | Upstream | Verdict | Why it is not a duplicate |
| --- | --- | --- | --- |
| PJ-C01 | `WS/CE02` | closed | CE02 closed *cfg-gating* in a **PVR** build. This is the opposite question in the opposite build: where the module **is** compiled in, the SPA still refuses every route. Different mechanism, different file. |
| PJ-C01 | `WS/CE06` | closed | CE06 closed *which code exists* for the licence router. This is a **value the server reports** to the SPA, and the refusals are in `008-pvr.js` / `012-pvr.js`, which CE06 never touched. |
| PJ-C03 | `PHASE0/F-13` | closed | F-13 was closed because the B-roll trigger **works** (`029-creator.js:151`). This is a different surface — the `#/publish/broll` signpost — printing a call that 405s. A user following the signpost never reaches the control F-13 verified. |
| PJ-C02 | `PHASE0/F-12` | open | Not a distinct thing: this row **re-confirms** the same defect and corrects its arithmetic. |
| PJ-C04, PJ-C06 | `PHASE0/F-11` | open | F-11 is the SPA ignoring the backend's `licence.implemented` flag on the **upgrade card**. These are different renderers, and PJ-C06 holds even if `implemented` were read tomorrow. |
| PJ-C07 | `PHASE0/F-16` | still-open | F-16 is that auto-transcribe has no SPA control, which remains the fix for the licensed case. This row records that the licensed case has no instance. |

## The validator fails when it should

`validate.mjs` asserts nine things. Eleven deliberate violations were injected
to prove each one fires — and **two of them initially did not**, because my
injection appended to a local map instead of the findings list, so `A1` and
`A4` were never actually exercised. That is the plan's recurring failure mode
in its purest form: *an assertion that has never run is not a passing check.*

| Assertion | Violation injected | Fired |
| --- | --- | --- |
| A1 | malformed id `PRO-3`; duplicate `PJ-C06` | fired |
| A2 | `file:line` 999999 into a 733-line file | fired |
| A3 | verdict `probably-fine` | fired |
| A4 | finding reusing the id `PHASE0/F-12` | fired |
| A5 | cross-check against `WS/NOPE-99` | fired |
| A6 | recorded `closed` for an `open` upstream row | fired |
| A7 | `closed` overlap with no stated reason | fired |
| A7 | empty cross-check with no note explaining the absence | fired |
| A9 | `broken` finding with its control removed | fired |
| A9 | `kept_promises` emptied | fired |
| A9 | `limitations` emptied | fired |

**A9 is the addition over todo 15's validator.** This audit's subject is
kept-vs-broken, so a `broken` row with no control cannot distinguish "this is
broken" from "everything is broken". The manifest was restored from a copy
taken before the probe and is byte-identical (sha256
`72b8673aff3662ad3370c86bc39367558cbea712001e540405407d46f003820c`); the
validator exits 0.

## Two harness problems this audit had to fix to run at all

1. **The shared readiness budget was too short for a Creator build.**
   `real-server.sh` waited 50 × 0.2 s. A Creator daemon initialises a
   six-plugin host before binding its socket: 0.6 s on an idle box, but
   measured at 12.4 s, 28 s and 55 s under this host's load average of 15-20.
   The loop is now `STRIVO_E2E_DAEMON_PROBES`, **defaulting to the previous
   50** so the PVR lane is unchanged; the Creator config passes 900.
   Widen-only, so no existing caller can be made to wait less.
2. **The two lanes share a `testDir`.** `playwright.real.config.ts` and
   `playwright.creator.config.ts` both point at `./tests-real`, and
   Playwright's default `testMatch` is recursive, so the Creator specs **are**
   collected by `npm run test:real`. Rather than break todo 15's lane, every
   Creator spec skips when the binary reports a non-`creator` edition —
   a true statement about that binary, and not a vacuous pass, because the
   first test in each spec *fails* on a non-Creator binary instead of skipping.
   Verified: the PVR lane exits 0 with **22 passed and 16 skipped**.

If a future lane removes that guard, the PVR lane fails with 16 errors that
read like product defects. That is `OBS-C01` in the register.

## Flake probe

Four consecutive green runs, **exit 0, 16/16, ~13.6 s each**, after two real
flakes were found and fixed. Both fixes came from reading source:

- a reload followed immediately by a hash navigation is a race —
  `render()` reads `CREATOR_ENABLED` before `fetchEdition()` resolves, so the
  pane bounced to Home intermittently. The helper now waits for the settings
  response;
- `clipper /analyze` (55-713 ms idle) exceeded the 45 s default once under
  load. Its test now has an explicit 180 s timeout; **the expectation is
  unchanged.**

Ten of my own first-run assertions failed, and every fix came from reading the
source. **None came from loosening an expectation** — with one exception worth
naming: the one assertion I *did* relax was a clock, and its measurement is
recorded in the code and in `PJ-C03`'s repro.

One tautology was caught before it ever ran: the pipeline node count was
originally `expect(labels.length).toBe(... ? labels.length : 0)`, which cannot
fail. It now takes its denominator from the API.
