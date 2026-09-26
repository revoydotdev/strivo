# PVR journey audit — register index, 2026-09-25

Narrative and reasoning: [REPORT.md](REPORT.md) · machine-readable rows:
[MANIFEST.json](MANIFEST.json) · gate: [`validate.mjs`](validate.mjs)

```sh
node docs/audits/2026-09-25-pvr-journey/validate.mjs
```

## What this is

Todo 15: the PVR user journey driven against the **real release binary**
(`/scratch/cargo-targets/strivo-main/release/strivo`, sha256 `38cacd0f…`, not
rebuilt), not `mock-server.mjs`. Audit only — nothing was fixed.

## Counts

| | |
| --- | --- |
| Journey steps driven | **10 of 10** |
| Findings | **10** (`PJ-01`…`PJ-10`) |
| — high | 2 |
| — medium | 3 |
| — low | 5 |
| Observations recorded | 5 (`OBS-A`…`OBS-E`) |
| Unreachable steps, named with the blocking code | 3 |
| Duplicate `closed` findings from todo 1's register | **0** |
| Playwright tests added | 5 files, 15 tests |
| Lane result | **22 passed**, three consecutive runs |
| Manifest ids renumbered or deleted | **0** |

## The three that would change a user's day

| Id | Severity | One line |
| --- | --- | --- |
| **PJ-01** | high | The platform wizard reports "Twitch saved" for credentials that were never validated — the daemon answers `202 {"applied":true,"configured":true}` for junk, and that flag also un-gates the first-run checklist. |
| **PJ-05** | high | `auto_record_channels` and `GET /api/v1/channels` are two different stores; a channel configured for automatic recording is invisible in the rail, on Monitor, and in the watch tiles. |
| **PJ-02** | medium | …and PJ-01 is a one-way door: both un-configure paths return 400, so the false state has no in-app remedy. |

## Why the ids are namespaced

`PJ-nn` is disjoint from todo 1's `PHASE0/F-nn`, `WS/Snn|Rnn|Vnn|CEnn`, `CE/nn`
and `DRV-nn`. `validate.mjs` A4 fails the build if a `PJ-nn` id ever collides
with one of them, and A5 fails if a cited upstream id was never issued.

**A7 is the acceptance criterion, mechanised.** It fails the build if any
finding cross-checks against a `closed` upstream item without a stated reason
it is a different thing. Nine findings touch a closed item; all nine carry
their distinction in `MANIFEST.json`.

## The named QA failure scenario: passed

A deliberately broken recording file surfaces **"Can't play this recording —
this format is not supported by this browser"** with Retry / Download file /
Copy file path and a `Playback error` label. Not a blank tile. A concurrent
ranged `GET` returned 206, proving a decode failure rather than a transport
one. The file was restored byte-for-byte in a `finally`.

## Not verified

Creator Edition surfaces; anything needing real platform credentials; live
playback; the negative direction of the 2 s GET cache; `PJ-06`'s runtime
trigger; large-library behaviour. See `MANIFEST.json` → `not_verified`.

## Maintenance

Append-only. Never renumber, reorder or delete an id. New findings append as
`PJ-nn` / `OBS-nn`. Record closure evidence against the existing id; a closed
row stays visible.
