# Consolidated re-verification register — 2026-09-25

Every finding from all three prior audits, re-checked against the working tree as
it stands at `a8d4f2b`. This document establishes truth and takes no action on
anything it finds. No product code, no prior audit, and no roadmap row was
modified to produce it.

| | |
| --- | --- |
| Prior items re-checked | **84** (34 + 41 + 9) |
| Derived items appended | 6 |
| Total rows | 90 |
| **`open`** | **12** |
| `still-open` | 8 |
| `closed` | 69 |
| `superseded` | 1 |
| **Unacted work still in front of us** | **20** (`open` 12 + `still-open` 8) |

Machine-readable register: [MANIFEST.json](MANIFEST.json) · evidence index:
[EVIDENCE.json](EVIDENCE.json) · validator:
[`validate.mjs`](validate.mjs)

---

## What the three audits are, and why the ids are namespaced

| Namespace | Source | Items | Id form |
| --- | --- | --- | --- |
| `PHASE0/F-nn` | [`PHASE0-EXPERIENCE-AUDIT-2026-07-30.md`](../PHASE0-EXPERIENCE-AUDIT-2026-07-30.md) findings table, lines 60–95 | 34 | `F-01`…`F-39`, gapped — 17, 21, 23, 24 and 31 were never issued |
| `WS/Snn`, `WS/Rnn`, `WS/Vnn`, `WS/CEnn` | [`2026-09-06-web-surface/MANIFEST.json`](../2026-09-06-web-surface/MANIFEST.json) `issues[]` | 41 | `S01`–`S17`, `R01`–`R06`, `V01`–`V12`, `CE01`–`CE06` |
| `CE/nn` | [`2026-09-08-performance-ux/REPORT.md`](../2026-09-08-performance-ux/REPORT.md) numbered findings | 9 | `1`–`9` |
| `DRV-nn` | this register | 6 | fresh ids, appended |

Two of the three audits issue ids in the `R01`–`R07` range, and the
web-surface audit's `CE01`–`CE06` reads exactly like the performance audit's
finding 1. A bare `R04` is therefore ambiguous, and a validator cannot assert
"appears exactly once" over an ambiguous key. Hence the namespaces. **`WS/CE01`
(the web-surface Creator-Edition decoupling item) and `CE/01` (the
performance audit's snapshot-only playback lookup) are different things**, and
the validator fails the register if any bare id is ever claimed by two
namespaces.

## Verdict vocabulary

`closed`, `still-open`, `open`, `superseded`. The last two are separated on
purpose, because "still broken" and "still broken *and its state moved since the
audit wrote it*" need different handling — the first is carried forward, the
second must be re-prioritised rather than blindly re-fixed.

| Verdict | Meaning |
| --- | --- |
| `closed` | Re-checked. The defect no longer exists; the remediation is present and locatable. |
| `still-open` | Re-checked. The defect still exists and no remediation was ever recorded against it. Carry forward unchanged. |
| `open` | Re-checked. The defect still exists **and its state changed** — partly remediated, the cited file moved or was deleted, or a new doc/code disagreement surfaced. Needs a decision, not a repeat of the original ticket. |
| `superseded` | Re-checked. The premise was decided against by a later authoritative decision. Do not action the original text. |

---

## The headline: the prior audits are badly stale, in both directions

Eight weeks separate the oldest audit from this one, and the code moved a great
deal. The single most important structural change: **`assets/spa.js` and
`assets/spa.css` no longer exist.** The SPA is 26+ ordered modules under
`assets/spa/` and 26 CSS modules under `assets/spa-css/`, concatenated at build
time by `crates/strivo-web/build.rs:43-44`. Almost every `file:line` citation in
all three prior audits is therefore dead by construction. That is why this
register re-derives every locator rather than carrying the old ones forward.

### 69 of 84 prior items are closed

That is a genuinely good result and it is *not* a claim that the audits were
wrong. They were right about their defects; the defects got fixed. The 2026-09-06
audit in particular closed 39 of its own 41 and was honest about the two that
were not.

### But two prior claims are now refuted by source, in the same direction

This is the finding that matters most, and it is not in any prior audit:

**`PHASE0/F-11` — the licence dead-click — was declared implemented and is not.**
The 2026-09-06 audit's `Supersession map` lists F-11 under "Six findings from the
2026-07-30 audit are **confirmed implemented** and must not be reopened", citing
`spa.js:8526-8553` and `:9231-9252` as reading `licence.implemented`. In the
current tree the string `implemented` appears **zero times** in every `-creator.js`
module. `renderUpgradeCard` is at `021-creator.js:693`; `renderProUpsell` is at
`021-creator.js:15`; neither reads the flag the backend computes specifically so
the SPA can grey out "Start trial". The original 2026-07-30 defect still stands,
and a prior register told the next person not to look. Any wave that trusted that
supersession map would have skipped a P1 Creator-journey item.

**`PHASE0/F-15` — Demucs — was declared to have no code, and now has some.**
The original evidence was `grep -rln "demucs" crates/` returning nothing.
`crates/multitrack/src/lib.rs:19` now reads `Demucs-style source separation on a
single mixed stereo`. The original premise is refuted. A residual wording
imprecision survives at `docs/STRATEGY-NVIVO-RIVERSIDE.md:114`, which says
"Multitrack and Demucs plugins" (plural) when there is no separate Demucs crate.

The general lesson, and the reason this todo exists: **a supersession map written
by an earlier audit is a claim, not a result.** Two of its six entries did not
survive a source re-read.

### The known-stale items the plan flagged: all re-checked, none assumed

| Flagged as probably closed | Verdict | Where it actually is |
| --- | --- | --- |
| `PHASE0/F-11` licence UI | **`open`** | Refuted. `implemented` is read nowhere. See above. |
| `PHASE0/F-18` reduce-motion | `closed` | `012-pvr.js:48` toggles `reduce-motion` on `<html>`; `:73` reads the setting back. |
| `PHASE0/F-19` REC dot | `closed` | `spa-css/000-pvr.css:1439` and `:1446` suppress it. |
| `PHASE0/F-25` probe cache | `closed` | `server.rs:61`; `ROADMAP.md:249` is now ✅, as the audit asked. |
| `WS/S10` edition boundary | `closed` | The Creator plugin catalog now lives in `assets/spa/020-creator.js`, a `-creator` module, so a PVR build never contains it. `build.rs` no longer strips lines at all. |
| `WS/CE03` spa.css remainder | `closed` | **The blocker the manifest named is gone.** `assets/spa-css/` is 26 edition-split modules and `build.rs:44` assembles them the same way it assembles the JS. |
| `WS/R01`–`WS/R06` | `closed` | All six, each independently located. |
| `WS/S05` constant-time HMAC | `closed` | `auth.rs:59` uses `Mac::verify_slice`. |
| `WS/S13` unauth chain write | `closed` | `api.rs:442`. The handler is no longer *structurally incapable* of authenticating. |
| `ROADMAP.md:120` webhook row | **still stale — opposite direction** | The row says "no SPA settings UI, a DoD gap (audit F-04)". The UI exists at `034-pvr.js:427`. Filed as `DRV-01`. |
| revoy ledger `ROADMAP.md:310-338` | **4 of 6 todos stale** | Filed as `DRV-03`, `-04`, `-05`. Bounds are actually `:306-340`, not `:310-338`. |
| `ROADMAP.md:127-128` canonical CSS | **file does not exist** | Filed as `DRV-02`. |

The webhook row deserves a note, because it is the opposite failure from the one
F-04 recorded. F-04 said "the row claims ✅ but there is no UI". The row has since
been *downgraded* to 🟡 saying there is no UI — and then the UI was built. So the
row is wrong, in the direction that would keep someone from closing a completed
item. **Code is truth.**

---

## The 20 that remain

Twelve `open` and eight `still-open`. Nothing here was fixed, and nothing here
should be actioned from this document.

### Genuinely still-open, unchanged since the original audit (`still-open`, 8)

These are the "needs live measurement or a deliberate decision" items. None has
moved, and none can be honestly closed without work this todo does not do.

| Id | What it is | Why it is still open |
| --- | --- | --- |
| `PHASE0/F-16` | Headless auto-transcribe has no SPA toggle | `crunchr-auto` appears nowhere in the SPA; only Archiver tandem has a control (`039-pvr.js:198`). |
| `PHASE0/F-27` | `cursor` is an integer offset, not a keyset cursor | `persist.rs:608` is still `LIMIT ?1 OFFSET ?2` with a per-page `COUNT(*)` at `:600`. The doc-terminology half was fixed; the shape was not. |
| `PHASE0/F-28` | Per-row `json_extract`, no covering index | `persist.rs:644`. Needs `EXPLAIN QUERY PLAN` before anyone optimises it. |
| `PHASE0/F-30` | One IPC connection per browser tab | `events.rs` is 79 lines and still relays deltas straight through. A scaling note, not a defect. |
| `PHASE0/F-33` | EDL render is not single-pass | `editor/src/lib.rs:299`, `:314`, `:335`. The audit's only ask was that no "one ffmpeg pass" claim be documented; none was found. |
| `PHASE0/F-34` | Per-request `ResearchStore::open` | **Worse, not better: 12 sites became 27** (`plugins.rs:4781` onward). The audit's escape hatch — "the endpoints have no callers" — died with F-14. |
| `PHASE0/F-35` | O(P×S²) registry rescan per wake | `executor.rs:238`. |
| `WS/V05` | Skip-link spec flake | See the honesty note below. |

### Changed state, needs a decision (`open`, 12)

| Id | What changed |
| --- | --- |
| `PHASE0/F-11` | The defect survives and a prior audit's closure claim for it is false. Highest-priority item here. |
| `PHASE0/F-12` | Half-remediated. `PRO_PANES` (`021-creator.js:64`) now describes each signpost honestly — the "honest about indirection" half of the audit's own either/or. The deep-link half did not: **14 of 20 tabs are still `route: null`.** |
| `PHASE0/F-22` | 77 hardcoded hex → 85 hex literals across 26 CSS modules, of which 30 are legitimate `var(--x, #fallback)` fallbacks. Roughly 55 genuine. Reduced, not zeroed. |
| `PHASE0/F-29` | The monolithic file is gone; the served bundle is still one concatenation with no chunking anywhere in the 202-line `build.rs`. Source modules total 1,085,680 bytes. |
| `PHASE0/F-36` | **The cited file no longer exists.** `src/pipeline/templates.rs` is gone — `src/pipeline/` holds only `executor.rs`, `mod.rs`, `runtime.rs`, `stage.rs`. `ResourceLock` moved to `stage.rs:85`, `Gpu` at `:88` is still a bare unit variant against `Cpu { cap: usize }` at `:91`. No transcription-specific cap exists. |
| `WS/V05` | The only row whose `closed` status this register could **not** confirm. The named file has no serial-describe (contrast `WS/S12`, which fixed the sibling file at `player-controller.spec.ts:21`), and `playwright.config.ts:15-17` is still `fullyParallel` with CI-only retries. Whether a flake is gone is a runtime property; this todo ran no browser suite. **Carried as `open` pending a suite run.** |
| `DRV-01`…`DRV-06` | The six new doc/code disagreements below. |

### The six derived findings

Found while re-verifying. Each is a case where documentation and code disagree
and the code is the truth. None was repaired here — that is todo 2.

| Id | Location | Disagreement |
| --- | --- | --- |
| `DRV-01` | `ROADMAP.md:120` | Row says "no SPA settings UI" for webhooks. The UI is at `034-pvr.js:427`. |
| `DRV-02` | `ROADMAP.md:127` | Names `crates/strivo-web/assets/spa.css` as the canonical CSS source. That file does not exist. Canonical source is `assets/spa-css/`, assembled by `build.rs:44`. |
| `DRV-03` | `ROADMAP.md:316` | Ledger todo: "Ship a Docker image — no Dockerfile or compose file exists." Both exist. |
| `DRV-04` | `ROADMAP.md:321` | Ledger todo: "ab-render and submix have zero Rust callers yet `spa.js:7074` routes…". 7 and 17 real callers respectively; `spa.js` does not exist. |
| `DRV-05` | `ROADMAP.md:336` | Ledger todo: "Guard `STRIVO_DEV_UNLOCK_ALL` behind debug_assertions." `src/licence/gate.rs:50` is already `cfg!(debug_assertions) && …`. |
| `DRV-06` | `PHASE0-EXPERIENCE-AUDIT-2026-07-30.md:97` | The audit's own tally says "Total: 33 findings"; its table has 34 rows. `F-19` is marked P2 in the table and appears in no severity bucket. **No id was renumbered and no row removed to reconcile the two.** This register counts 34. |

The revoy ledger's remaining two todos — deploying `licence-backend`
(`licence-backend/wrangler.toml:11` still holds `REPLACE_WITH_D1_ID`, so it is
accurate) and reconciling `/api/v1/pipelines/dag` (`api.rs:493` still returns
`default_pipelines()`, so it is also accurate) — are **not** stale. The
unoccupied-niche positioning claim at `:331` is a market assertion that source
cannot settle either way; it stays open on its own terms.

---

## One item is `superseded`

`WS/CE04` closed by supersession under
[ADR 0002](../../adr/0002-monorepo-boundary.md) and stays that way. Its observed
facts are still true — `strivo-plugins/Cargo.toml:13` does still set
`publish = false` — but the action it recommended (a second repository) was
decided against and must not be attempted. ADR 0001 is immutable; it was
superseded, not rewritten.

## A residual, stated rather than hidden

`WS/CE01`'s stated acceptance criterion was literal: `grep -ri "crunchr\|archiver"
src/` returns nothing. It now returns **one** hit — a doc comment at
`src/config/mod.rs:48`. The code-level acceptance is met (`catalog.rs:50` is a
generic `post_pull_markers: Vec<String>`), the string-level one is not. Recorded
as a residual rather than quietly rounded to "met".

---

## Verification

### The validator

```
node docs/audits/2026-09-25-consolidated/validate.mjs
```

Four assertions: every prior item appears exactly once; every `file:line`
resolves inside its file's length; the verdict vocabulary is closed; the id
namespaces do not collide.

It does **not** read the per-source item counts from this register. It re-parses
the three source audits at run time — the PHASE0 markdown table, the WS
`issues[]` array, and the CE report's numbered headings — and fails if the derived
counts disagree with the claims. A manifest that quietly dropped an item cannot
pass.

Result at this HEAD: `PASS`, 0 failures, 84 prior items re-derived
(34 / 41 / 9), 90 rows, `open_count` 12.

### The non-vacuity probe

A validator that cannot fail is worse than none. A row with id `PHASE0/F-17` and
`file:line` of `012-pvr.js:99999` — against a 564-line file — was injected. The
validator exited **1** with five failures across four independent assertions:

```
- A4 PHASE0/F-17 names an id that the PHASE0 audit never issued
- A1 register has 91 rows; expected 84 prior + 6 derived
- A2 PHASE0/F-17: crates/strivo-web/assets/spa/012-pvr.js:99999 is out of range (file has 564 lines)
- A3 verdict_histogram.open says 12, rows say 13
- A3 open_count says 12, rows say 13
```

(`F-17` is one of the five gapped ids the 2026-07-30 audit never issued, so the
probe tripped a fifth, independent check.) The row was removed and the validator
returned to exit 0. Both outputs are recorded in
`artifacts/w1-01/consolidated-audit.json`.

### What was not run

No `cargo build`, no `cargo test`, no Clippy, no Playwright, no live daemon, no
compression measurement, no benchmark. **Every verdict in this register is judged
against source, not against a build artifact** — `target/` output is not truth
(`CLAUDE.md`). Rows whose original closure rested on a test or a benchmark carry
that caveat in their evidence field, and `EVIDENCE.json` lists every
unperformed check explicitly. The full list is in `EVIDENCE.json` under
`unperformed_checks`.

### The dirty worktree

`git status --porcelain` at entry showed ` M Cargo.lock` and `?? artifacts/`.
Both pre-existed this todo. Neither was modified, staged, or swept. The commit
stages only `docs/audits/2026-09-25-consolidated/`.

---

## Maintenance

**Append-only. Never renumber, reorder, or delete an existing id.** Close an item
by recording its closure evidence against its own id; a closed row stays in the
register with its verdict. New findings append with fresh ids in the `DRV-nn`
namespace. If a `file:line` drifts because the code moved again, update the
locator and the evidence — do not mint a successor id and leave a corpse behind.

When you act on an item, record what you did in `MANIFEST.json` and cite the
executed command in `EVIDENCE.json`. A report that predates the relevant code
change cannot close an item.
