# PVR journey audit — the real binary, 2026-09-25

**Todo 15.** The first pass over the StriVo PVR user journey driven against a
real compiled release binary rather than `mock-server.mjs`.

Machine-readable register: [MANIFEST.json](MANIFEST.json) · validator:
[`validate.mjs`](validate.mjs) (run it; it is the gate, not this prose)

**Nothing in this audit was fixed.** The plan reserves remediation for Phase 2.
No product source, no asset, no mock-lane spec and no CI script was modified.

---

## What was under test

| | |
| --- | --- |
| Binary | `/scratch/cargo-targets/strivo-main/release/strivo` |
| sha256 | `38cacd0f8351c1b19a8d638df2447be04ae2adb6ce678bf23d6c9b2c2fe8aa62` |
| Rebuilt? | **No.** Other lanes measure against this content hash. |
| Edition | PVR, proven by the served bytes |
| Lane | `npm run test:real` — `playwright.real.config.ts`, port 8281 |
| Specs | 5 new files in `crates/strivo-web/e2e/tests-real/`, `pvr-journey-*.ts` |
| Result | **22 passed** on three consecutive end-to-end runs (28.0s / 33.0s / 28.1s) |

The edition is identified by hashing what the server serves, never by the
build directory name and never by `strings` — the lesson todo 10 recorded after
`strings` on this very binary called it a Creator build. `GET /assets/spa.js`
contains `function chrome(` and does not contain `renderProApp`, which is
defined only in `assets/spa/021-creator.js`.

### The two config traps, and what they cost

`playwright.real.config.ts:14` hardcodes `PORT = 8281` and ignores the `PORT`
env var `real-server.sh:17` honours. Left alone deliberately: another lane needs
that port, and changing the config is not this todo's change.

`playwright.real.config.ts:20-23` falls back to `target/release/strivo` and
`target/debug/strivo`. **Neither exists** — `.cargo/config.toml` redirects
`target-dir` to `/scratch/cargo-targets/strivo-main` — so the fallback resolves
to empty and `real-server.sh:16`'s `${STRIVO_BIN:?}` guard aborts the boot. The
lane only runs with the export done by hand:

```sh
export STRIVO_BIN=/scratch/cargo-targets/strivo-main/release/strivo
cd crates/strivo-web/e2e && npm run test:real
```

That is a real onboarding hazard for anyone who reads the config's own comment
("Prefer a release binary … and fall back to a debug build for local runs"),
which describes a fallback that cannot fire in this checkout.

---

## Why these assertions are not the mock wearing a hat

`mock-server.mjs` is a competent stub, and the risk in a journey audit is
asserting something it happens to satisfy. Every assertion was checked against
it. It has **no** route for `/channels/resolve`, `/settings/platform/*`,
`/channels/:key/auto_record`, a backup **download**, or a backup **restore**.
Its fixture literals are all distinct from anything the real server returns:

| Mock literal | Where | Why it matters |
| --- | --- | --- |
| `UClive0000000000000000aa`, `Live Channel`, `is_live:true` | `mock-server.mjs:51,54,59` | A non-empty channel fixture, so **every** "no channels" assertion here fails against the mock |
| `LilAggy` / `Elden Ring` | `mock-server.mjs:454` | A hardcoded history row, so the real journal row cannot be confused with it |
| `2026-05-26T00-00-00Z`, `bytes: 1234` | `mock-server.mjs:464,467` | A fixed backup name and a made-up size |

Three **control assertions** were added on purpose, because without them the
findings they support would be unfalsifiable:

- the Add-channel dialog **does** close on Escape — which is what makes PJ-03 a
  finding about *this* dialog rather than a claim that Escape is broken;
- `#/studio` **does** rewrite the URL on a PVR build — which is what makes PJ-08
  a finding rather than a design choice;
- the platform endpoint **does** return 400 for empty credentials — which is
  what makes PJ-01's 202-for-junk a real false success rather than a
  blanket-success endpoint.

And the honest negative: **nine assertions in these specs failed on their first
execution.** Every one was fixed by reading the source and correcting the
expectation, not by loosening it. Two of those nine were wrong assumptions of
mine that the real binary corrected — see "What I got wrong" below.

---

## The ten steps

`real-server.sh` boots a sandboxed daemon holding exactly one real recording:
a 2-second 320×240 Matroska produced by `ffmpeg`, journalled directly into
`jobs.db`, 19 214 bytes on disk. No platform is configured and no channel is in
the daemon snapshot, so the lane starts in a genuine first-run state.

| # | Step | States reached | Findings |
| --- | --- | --- | --- |
| 1 | First run | loading, empty, partial, stale | **PJ-04** |
| 2 | Platform wizard | loading, empty, partial, failure, stale | **PJ-01**, **PJ-02**, **PJ-03** |
| 3 | Add channel | closed, empty, partial, loading, failure | **PJ-05**; phase 2 **unreachable** |
| 4 | Monitor | populated (live figures), empty | — |
| 5 | Record | **unreachable**; history reachable | — |
| 6 | Library | loading, populated, empty (filtered), empty (cursor) | OBS-A, OBS-E |
| 7 | Watch | empty (named cause), failure, malformed | **PJ-07**, **PJ-08**, **PJ-10**; live tile **unreachable** |
| 8 | Settings | populated ×8 sections, partial, failure (restart-required) | — |
| 9 | Backup / restore | loading, empty, populated, partial, restored | **PJ-09** |
| 10 | System | populated, degraded-capable | OBS-D |

### The two findings that matter

**PJ-01 / PJ-02 — the platform wizard is a one-way door that reports success
while doing it.** `POST /api/v1/settings/platform/twitch` with
`client_id: "not-a-real-client-id"` returns `202 {"ok":true,"applied":true,
"configured":true}`. `openPlatformWizard` branches only on `applied === false`
(`034d-pvr.js:96-104`), so the SPA toasts **"Twitch saved"**, closes the dialog,
and `GET /api/v1/settings` then reports `twitch_configured: true` — for
credentials that were never sent anywhere. The daemon did not contact Twitch.

That flag is also what `renderHome` tests to decide whether setup is finished
(`012b-pvr.js:92-95`), so the lie propagates: the first-run checklist
disappears on a setup that has never worked.

PJ-02 is the half that makes it serious. Both candidate repairs are refused:
`POST /settings/update {"path":"twitch.client_id","value":""}` → 400, and
`POST /settings/platform/twitch` with empty values → 400 *"client_id and
client_secret are required"*. After both, the flag is still `true`. There is no
in-app way back; recovery means hand-editing `config.toml`.

**PJ-05 / PJ-06 — two stores, one channel list.** `GET /api/v1/channels` returns
`ServerMessage::StateSnapshot.channels` and nothing else
(`settings.rs:42-53`). `PUT /api/v1/channels/{key}/auto_record` writes
`config.auto_record_channels` (`blocklist.rs:151,184`) and never touches that
snapshot. Observed, in one breath:

```
PUT /api/v1/channels/Twitch:12345/auto_record {"enabled":true}   -> 200
GET /api/v1/settings   -> auto_record_channels: [{Twitch, 12345}]   ✅
GET /api/v1/channels   -> {"channels": []}                          ❌
```

A channel that is genuinely configured for automatic recording is therefore
absent from the left rail, from Monitor, and from the watch tiles, and there is
no capture button to start it from. The user cannot see or act on a channel
the daemon will in fact record. PJ-06 is the adjacent trap in the same
handler: the fallthrough arm `Ok(_) => {"channels": []}` flattens *any* other
`ServerMessage` variant into a successful empty list, so a daemon that could
not answer is indistinguishable from an account with no channels.

### The named QA failure scenario: **passed**

The seeded capture's bytes were overwritten in place with truncated
non-Matroska garbage. The journey surfaces a real, specific, actionable error
state — not a blank tile:

> **Can't play this recording** — this format is not supported by this browser
> — `Retry` · `Download file` · `Copy file path` — "Playback error" — `0:00 / 0:00`

The `<video>` element is present and points at the real endpoint, and a
concurrent ranged `GET` returned `206` with 16 bytes, which proves the failure
is a **decode** failure and not a transport one. The file was restored
byte-for-byte in a `finally`, verified by both length and content — and the
seven pre-existing `real-server.spec.ts` tests, including its `Range`/`206`
assertions, still pass afterwards.

The one honest wrinkle is **OBS-C**: the recordings table keeps showing
"Finished" and the correct size, because `augment_recording` computes
`file_exists` with `Path::exists()` (`settings.rs:66-80`) and the timeline only
flips to `Failed` when it is `false`. A present-but-undecodable file is
indistinguishable from a good one until you press Play. Recorded as an
observation rather than a finding, because the player handles it well and
surfacing it earlier would mean a media probe on every list render — which
`docs/PERFORMANCE.md:47-52` explicitly argues against. It is a trade-off, and
now it is a recorded decision rather than an oversight.

---

## Unreachable steps, named rather than omitted

Three parts of the journey cannot be reached in this build, and each is
recorded with the code that blocks it.

- **Add-channel phase 2** (`012d-pvr.js:87-96`) renders `#aw-confirm` only when
  a `ChannelResolved` frame carries a `channel_id`. With no platform the daemon
  answers with an error frame instead — observed verbatim as
  *"Not found: twitch section missing in config"*. Asserted: `#aw-confirm` has
  count 0.
- **Starting a capture** (`012c-pvr.js:11,17,25`) renders per channel, and the
  channel detail needs a selected channel. `selectedChannelKey` is null on a
  fresh session and nothing can enter the snapshot without a platform (PJ-05).
  Recording *history* is reachable and is asserted against a real file.
- **A live watch tile.** `#/watch` renders its stage, layout controls and
  recent-recordings strip, but a tile needs a channel. Asserted: 0 iframes.
  **Stated honestly: this is the one state the mock could fake** —
  `mock-server.mjs:59` ships `is_live:true` and would render a live tile.

---

## The cache discipline, in writing

`API._fetch` collapses duplicate GETs behind `_cacheTtlMs: 2000`
(`002-pvr.js:17`). **A DOM read taken inside that window is a cache hit, not a
broken endpoint.** So every empty / partial / stale claim in this register is
cross-checked through Playwright's `request` fixture with an `x-api-key` header
— a separate HTTP request that never touches the page's cache — and no DOM read
alone is ever treated as evidence that an endpoint is empty.

The TTL is asserted out of the **served bundle**, not out of the page:
`assets/spa.html:17` loads `spa.js` as `type="module"`, so the module-scope
`const API` is not on `window` and cannot be read from the page at all. A
release-specific wrinkle worth recording: **the served bundle is minified, so
`_cacheTtlMs: 2000` is emitted as `_cacheTtlMs:2e3`.** An assertion on the
unminified spelling fails against the real release build and passes against the
mock — the worst possible combination for this lane. The spec matches the
identifier and then parses whichever numeric form the minifier chose.

**Deliberately not asserted:** that an immediate re-read is provably stale. That
is wall-clock dependent and would flake under this host's load average. The
positive direction is asserted (a post-TTL read is live and agrees), and the
structural protection is that every claim goes through `request`. Stated as a
limitation rather than quietly dropped.

---

## What I got wrong, because the audit is only as good as its corrections

Four assumptions of mine were falsified by the real binary, and each is a
reminder of what a mock lane structurally cannot teach you:

1. **"All three first-run steps are un-ticked."** Step 2 (recording directory)
   is legitimately complete in this sandbox. A spec asserting "all todo" would
   have been wrong about this build; one asserting nothing would have let a gate
   that marks steps done unconditionally pass unnoticed. It now asserts the
   exact per-step marks.
2. **"The empty state-chip row is a defect."** `012f-pvr.js:165-169` skips the
   row on purpose for a single-state library. My selector was also wrong. This
   is now **OBS-A**, recorded precisely so the next reader does not repeat my
   mistake.
3. **"`POST /backup` returns 200."** It returns 201 — which the mock *also*
   returns. The status code was never a discriminator; the name, the size and
   the gzip magic are. The spec's own comment now says so.
4. **"The heading reads `RECENT RECORDINGS`."** The DOM text is
   `Recent recordings` and CSS uppercases it, so `innerText` returns either form
   depending on stylesheet timing — a 20-second timeout rather than a signal.
   Now matched case-insensitively.

---

## Cross-check against todo 1's register

The [2026-09-25-consolidated](../2026-09-25-consolidated/MANIFEST.md) register
holds 90 rows, **69 of them `closed`**. Re-reporting a closed item is an explicit
failure of this todo, so the check is **mechanised, not asserted**: `validate.mjs`
assertion **A7** fails the build if any finding cross-checks against a `closed`
item without a stated reason it is a different thing, **A6** fails if a recorded
verdict disagrees with the upstream register, and **A5** fails if a cited
upstream id was never issued.

**Result: zero duplicate `closed` findings.** Nine findings touch a closed item
and each carries its distinction; two more touch `open`/`still-open` items and
each carries its distinction.

| Finding | Upstream item | Upstream verdict | Why it is not a re-report |
| --- | --- | --- | --- |
| PJ-01 | `WS/S04` | closed | S04's remedy was the existence of this lane, which is what *found* PJ-01. Re-reporting it would re-report the fix. |
| PJ-02 | `PHASE0/F-14` | closed | F-14 was the unauthenticated chain being unable to write. Both paths here **are** authenticated; the refusal is a payload-validation 400. |
| PJ-03 | `WS/R04` | closed | R04 was Enter/Space on sortable headers. Different component, different key. |
| PJ-04 | `PHASE0/F-04` | closed | F-04 was a doc row claiming a UI that did not exist. PJ-04 is live control-flow ordering in `renderHome`. |
| PJ-05 | `PHASE0/F-30`, `PHASE0/F-34` | still-open | F-30 is one IPC connection per tab; F-34 counts `ResearchStore::open` sites. Neither is a persistence-vs-snapshot divergence. |
| PJ-06 | `WS/V01` | closed | V01 was a specific validator/UI defect already remediated; this is a fallthrough arm in another handler. |
| PJ-07 | `WS/R03` | closed | R03 was the Load-more cursor. PJ-07 is player URL parameter parsing. |
| PJ-08 | `WS/S10` | closed | **S10's closure is the reason PJ-08 is reachable rather than a crash** — the edition boundary is fixed, so the PVR build correctly renders General; only the address bar lies. |
| PJ-09 | `WS/CE02` | closed | CE02 was Creator/PVR feature separation. Unrelated to the backup handler. |
| PJ-10 | `PHASE0/F-39` | closed | Recorded as a fresh observation about hash rewriting, explicitly **not** asserted to be a regression of F-39. |

PJ-01 and PJ-08 are the two that most needed the check: both sit directly on
top of items todo 1 closed, and both would have been easy to file as
"the audit was wrong again". They are not — they are what the fix made visible.

---

## Findings

| Id | Severity | State | Location |
| --- | --- | --- | --- |
| PJ-01 | high | false-success | `assets/spa/034d-pvr.js:96` |
| PJ-05 | high | two-sources-of-truth | `src/routes/settings.rs:42` |
| PJ-02 | medium | unrecoverable-state | `src/routes/settings.rs:1953` |
| PJ-04 | medium | data-hidden | `assets/spa/012b-pvr.js:95` |
| PJ-06 | medium | false-empty | `src/routes/settings.rs:50` |
| PJ-03 | low | accessibility | `assets/spa/034d-pvr.js:44` |
| PJ-07 | low | malformed-input | `assets/spa/019b-pvr.js:61` |
| PJ-08 | low | mislabelled-state | `assets/spa/032-pvr.js:89` |
| PJ-09 | low | misleading-figure | `src/routes/backup.rs:62` |
| PJ-10 | low | stale-url | `assets/spa/008-pvr.js:340` |

Every finding carries a `file:line` and a named reproduction in
[MANIFEST.json](MANIFEST.json), and each is driven by an assertion in
`tests-real/`. `PJ-06` is the one finding whose runtime trigger was **not**
reproduced — forcing a non-`StateSnapshot` variant through the IPC boundary is
outside what this sandbox can do. It is cited and read in source, and the
register says so rather than implying a runtime proof.

---

## Not verified

- Any Creator Edition surface. Out of scope; the binary is the PVR edition.
- Anything requiring working third-party platform credentials — which, per
  PJ-01, cannot be obtained honestly in this build.
- Live stream playback. No tile ever mounts; not simulated.
- The negative direction of the 2 s GET cache, for the flake reason above.
- PJ-06's runtime trigger.
- Large-library or long-running behaviour. One recording, one backup, one
  out-of-range cursor probe.

## Maintenance

Append-only. Never renumber, reorder or delete an existing id. New findings
append in the `PJ-nn` namespace. If a `file:line` drifts because the code moved
again, update the locator and the evidence — do not mint a successor id and
leave a corpse behind. A finding closed by remediation stays in the register
with its closure evidence recorded against its own id.
