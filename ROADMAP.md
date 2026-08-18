# StriVo Roadmap

## North star

**StriVo is a self-hosted Live Stream PVR — "Sonarr/Radarr for live streams."**
It monitors Twitch/YouTube (and Patreon) channels, detects when they go live,
records the stream, finalizes it into a clean library, and manages everything
from a TUI-less *arr-style web UI backed by a background daemon. That product —
capture, library, scheduling, monitoring — is the **core and the default build**.

StriVo ships in two editions from one codebase, gated by the `creator` Cargo
feature:

- **StriVo** (default) — the pure PVR. `cargo build`.
- **StriVo Creator Edition** — the PVR **plus** the creator/analytics toolkit
  (transcription, clip discovery, the DAW/EDL editor, cross-recording analytics,
  the pipeline engine). `cargo build -p strivo-bin --features creator`.

The creator toolkit's destination is a **domain-agnostic stream→signal
analytics & content-creation engine** — extract → parse → analyse → visualise →
export, fast enough to keep up with capture, with sports-analytics and
creator-content as templates on top. That ambition is real but it is the
**Creator Edition's** trajectory, not the core PVR's. The PVR must be excellent
and complete on its own first; the engine builds on it for those who opt in.

> **Status legend:** ✅ shipped & wired end-to-end · 🟡 built but not wired /
> shallow · ⬜ not started · ⏸ deferred (with reason).
>
> **Definition of done (non-negotiable):** a milestone is ✅ only when wired
> end-to-end. A pure-data crate with tests is necessary but **not sufficient**.
> Stubs, inert modules, hardcoded paths, and "tested but disconnected" code are
> tracked below as 🟡/⬜ blockers, never presented as shipped.

---

## The edition split (✅ shipped)

The default build is a pure PVR; all creator tooling compiles only under
`--features creator`.

- **strivo-core** — `creator` feature gates the Crunchr/Archiver config sections
  and the `crunchr_auto` tandem handshake on the back-catalog pull path. The
  plugin host trait/registry is generic and unconditional.
- **strivo-web** — all 34 tool-crate deps are `optional`; `routes::plugins` plus
  the marketplace/pipelines/capabilities/archiver routes mount only under the
  feature. The PVR build serves recordings, monitor, schedule, settings,
  system, logs, history, watch.
- **strivo-bin** — `strivo-plugins` is optional; the feature registers the
  first-party plugins and fans out to `strivo-web/creator` + `strivo-core/creator`.
- **workspace** — `default-members` = the PVR crates, so plain `cargo build`/
  `cargo test` is the PVR; `--workspace` and `-p … --features creator` reach the
  rest.

Verified: both editions compile and pass tests (191 PVR tests); a `compile_error!`
probe confirmed the gate fires under `creator` and is absent without it; legacy
configs carrying `[crunchr]`/`[archiver]` sections still load in the PVR build
(unknown fields are ignored).

---

## Where StriVo is today (v0.5.0)

### PVR core — solid ✅
- Web-only frontend (the ratatui TUI was removed in `2ab4e6c`); `strivo`
  launches the SPA + daemon.
- Recording pipeline: live + VOD capture, gap-resume segment merge, Twitch
  ad-trim, MPEG-TS→Matroska remux, deterministic UUIDv5 ids, HTTP-Range seeking.
- Live detection: polling monitor (15 s floor) + Twitch EventSub websocket
  (`stream.online`, ≤10 subs) + optional YouTube WebSub callback.
- Recording dispatch centralised through `src/intents/` (one canonical
  translator). Daemon ↔ SPA over a Unix-socket IPC; SSE event stream.
- Jobs journal in SQLite (`jobs.db`) with orphan recovery on restart; health
  checks page; backup/restore; blocklist; first-run onboarding.
- Desktop notifications now actually fire (✅ this cycle); config + schedule
  state are written atomically (✅ this cycle).

### Creator Edition toolkit — built, partly wired 🟡 (see Creator Edition roadmap)
~34 in-tree crates under `crates/<name>/`, each pure-data with unit tests, most
wired to a Pro-gated HTTP endpoint and surfaced on the SPA. Transcription
(`crunchr`), scene/cue detection (`cuepoints`), chat density (`chat-density`),
and cross-recording aggregation (`insights`) work and are wired. The analytics
spine (`dataviz`, `pipelines-dag` + `src/pipeline/`) is built but the daemon
does not yet drive the executor — the highest-leverage Creator gap.

---

## PVR roadmap (near-term — the product comes first)

Folded in from the 2026-06-25 audit + exemplar comparison (Sonarr/*arr,
streamerREC, Tdarr, Jellyfin). Full detail in `research/analysis/` (SYNTHESIS,
ui-ux, backend) and `research/exemplars/`.

### Robustness & correctness
| Item | State | Notes |
|---|---|---|
| Desktop notifications dispatched (notify-rust was a dead dep) | ✅ | `src/daemon.rs` — per-flag banners on a blocking task |
| Atomic `config.toml` write (tmp+rename) | ✅ | `src/config/mod.rs` |
| Atomic `schedule-state.json` write | ✅ | `src/recording/schedule.rs` |
| **Stall detection** — frozen ffmpeg/yt-dlp stays `Recording` forever, silently | ✅ | `recording/mod.rs` — per-recording growth clock; 120 s no-growth → warn + stop (feeds retry) |
| Concurrency cap is porous — manual `Start` paths bypass `max_concurrent_recordings` | ✅ | gate now in the manager's `Start` handler (all paths) |
| Single-loop monitor — a slow platform delays all live detection that tick | ✅ | `monitor/mod.rs` — per-platform fetches fan out via `join_all`; state mutations stay serial |
| yt-dlp retry falls back to FFmpeg for YouTube live-from-start | ✅ | retry now re-spawns the original process kind (yt-dlp `--live-from-start`) |
| `count_finished_recordings` O(N), undercounts >500/channel | ✅ | `persist.rs` — `SELECT COUNT(*) … WHERE channel_id=? AND state='finished'` |
| `.orig.mkv` safety copies accumulate after every remux | ✅ | deleted after the remuxed file is verified non-empty; kept on failure |
| IPC protocol unversioned — silent drop/deser failure across peer versions | ✅ | `src/ipc.rs` — `Hello {version:u32}`, `StateSnapshot {version:u32}`, `IPC_PROTOCOL_VERSION` constant, roundtrip tests. |
| VOD backfill ignores the daemon CancellationToken (300 s sleep on shutdown) | ✅ | `d784480` — `CancellationToken` threaded into `vod_backfill::spawn`; shutdown no longer blocks 300 s. |

### Web UI — finish the clean PVR split
| Item | State | Notes |
|---|---|---|
| `creator_enabled` exposed in `/api/v1/settings` | ✅ | the enabler for SPA gating |
| SPA hides creator UI in the PVR build | ✅ | consumes `creator_enabled`: filters TOPNAV, bounces creator deep-links, hides the Recording-Info plugin actions, Settings→Plugins pane, and Monitor "Tandem downloads". Chat kept (client-side IRC) |
| Build-time SPA split to drop dead creator JS (~30+ unused API methods) | ✅ | `7776179` — build-time strip confirmed; dead API methods excluded from PVR bundle. |

### PVR feature gaps vs *arr / streamerREC
| Item | State | Notes |
|---|---|---|
| Calendar / upcoming-streams grid | ✅ | 7-day strip on the Schedule page off `next_fire`; full EPG later |
| Per-channel quality/format overrides in the UI | ✅ | Monitor rows expose container/profile selects; `put_auto_record` persists them |
| Outbound webhook / notification connectors | 🟡 | backend works (`src/webhook.rs` off `DaemonEvent`) but TOML-only — no SPA settings UI, a DoD gap (audit F-04). Discord/ntfy presets later |
| Storage gauge in the UI | ✅ | three-segment disk bar on the System page from `/api/v1/storage` |
| Concurrent-slot indicator ("N / M rec") | ✅ | topbar slot pill from `monitor_limits.max_concurrent_recordings` + live count |
| Quality profiles (tiered) | ✅ | `bd75f9c` + `0fa189e` — `QualityTier` enum in `CaptureProfile`; threaded through streamlink and yt-dlp. |
| Filename-token browser, JSON channel import/export | ✅ | `bd75f9c` — token browser SPA pane + JSON import/export routes shipped. |

### DESIGN.md compliance ✅ (resolved — JellySkin is canonical)
DESIGN.md previously mandated ElegantFin while the SPA shipped JellySkin. Owner
decision: **JellySkin is the trajectory.** DESIGN.md §"Web UI Theme" + Typography
were rewritten to JellySkin (tokens mirror `spa.css`), the SPA font is Montserrat,
and `spa.css` font loading moved Google Fonts → Bunny Fonts (privacy). The SPA
uses JellySkin purple/cyan; brand cyan `#00E5FF` stays the TUI/marketing accent.
The stale ElegantFin reference CSS under `docs/reference/` was archived.

---

## Creator Edition roadmap — the analytics engine

The exhaustive execution plan for the stream-native qualitative research
platform now lives in
[`docs/RESEARCH-PLATFORM-ROADMAP.md`](docs/RESEARCH-PLATFORM-ROADMAP.md).
It expands CE-P1 through CE-P8 into versioned delivery phases with TDD,
accessibility, privacy, migration, performance, and release gates.

The phased engine plan, preserved. This is the Creator Edition's destination; it
builds on the PVR substrate and ships only under `--features creator`. Each phase
lists concrete blockers; none is ✅ until wired end-to-end with tests.

### CE-P1 · Unified signal spine ✅ *(foundation)*
Replace fragmented per-plugin SQLite with one canonical, append-only **signal
store** every extractor writes and every analytic reads:
`(recording_id, t_start, t_end, kind, label, payload JSON, confidence, source_plugin)`.
- **Shipped:** canonical project/source/case/code/coding/memo/relationship and
  append-only signal/provenance schema; bounded query/export API; authenticated
  Creator routes; deterministic, checksummed Crunchr/Cuepoints/Viewguard
  adapters; corrected Insights sibling-store and Viewguard plugin paths.
- **Unblocks:** cross-signal joins, the sports event spine (CE-P4), real corpus
  assembly (CE-P2).

### CE-P2 · Corpus-assembly service ⬜
Move corpus assembly server-side: hydrate a `dataviz::Corpus` by
`recording | playlist | channel + date-range` from the CE-P1 store, behind an
endpoint. Today `dataviz_run` exists but the SPA hand-assembles the corpus.

### CE-P3 · Wire the DAG executor into the daemon ✅
The daemon now owns a durable scheduler: validated submissions, dependency
fan-out, plugin capability dispatch, GPU/API/file resource locks, bounded retry
with backoff, cancellation, restart recovery, atomic snapshots, duplicate-run
coalescing, and live `PipelineUpdated` events over SSE. Manual, RPC, and
recording-finished Crunchr triggers share the executable Creator-intelligence
template. The Creator publish template now executes ten durable stages across
transcript intelligence and visual scene mining, with chapters, captions,
brand-safety, highlight scoring, clip export, thumbnails, reuse drafts, and
Casebook convergence. The Pro Pipelines page exposes real run
history/cancel/retry and secured artifact downloads.

### CE-P4 · Extraction adapters — domain-agnostic 🟡→✅
A common `Extractor` contract writing into the CE-P1 store. Have: transcription,
scene/cue, chat density. Missing: timecoded entity/event extraction (the sports
spine), visual/OCR (scoreboards, lower-thirds). Needs per-extractor confidence +
provenance and back-pressure so extraction keeps up with capture (feeds CE-P8).

### CE-P5 · Analytics over real corpora ⬜
Experiment registry over `dataviz`; cross-signal experiments (transcript ×
events × chat); incremental/streaming aggregation over SSE.

### CE-P6 · Visualisation & composer UI ⬜
Pick corpus → pick experiment → render via `chart_hint` → export CSV/JSON/PNG. A
general composer (not per-plugin pages); chart-type auto-selection; saved views.

### CE-P7 · Clip & export pipeline ✅
The CE-P3 Creator DAG now drives
*scene extraction → highlight selection → clip export + thumbnail generation*
alongside the caption branch, and converges both into publish drafts and
Casebook. Outputs are durable, inspectable, and downloadable from run history.

### CE-P8 · Real-time — "as fast as it is recorded" ⬜ *(headline promise)*
Streaming incremental extraction *during* capture: extractors tail the live
segment, write signals, analytics/visualisation update live over SSE.

### CE-Capstone · Domain templates ⬜
On the domain-agnostic core, ship two configs (not codebases): a **Sports**
template (event taxonomy + MLB-style box-score rollups) and a **Creator**
template (highlight/retention rollups + publish-ready clips).

---

## CE-Fusion · NVivo meets Riverside (dual-value bridge)

Positioning and rationale live in
[`docs/STRATEGY-NVIVO-RIVERSIDE.md`](docs/STRATEGY-NVIVO-RIVERSIDE.md) (Round 1).
Standing rule adopted from that confab: **every research-platform phase ships at
least one creator-visible outcome.** The kernel keeps canonical research names;
creator surfaces render their own vocabulary (coding → moment, code → tag,
corpus → collection). One schema, one API, per-surface display strings — never a
fork.

| Item | State | Notes |
|---|---|---|
| **F1 · Archive search** — lexical search over kernel `transcript.utterance` signals; every hit resolves to source + time range | ✅ | `crates/research/src/search.rs` (temp FTS5 index, phrase-quoted queries, deterministic `(source_id, start_ms, id)` order) + `GET /api/v1/research/projects/{id}/search` |
| **F2 · Moments projection** — codings and high-confidence detections exposed as creator-vocabulary "moments"; creating a moment writes a coding | ✅ | `crates/research/src/moments.rs` + GET/POST `/api/v1/research/projects/{id}/moments`; detections = `visual.scene_change` + `audience.anomaly` (chat-density joins once it writes kernel signals) |
| **F3 · Content-free product telemetry** — per-route latency/reliability aggregation, local only, no corpus content | ✅ | `crates/strivo-web/src/telemetry.rs` (matched-route template + status + duration only, both editions) + authed `GET /api/v1/telemetry` |
| **F4 · SPA Archive surface** — search + moments pages in creator vocabulary, keyboard operable, `creator`-gated | ✅ | `#/archive` route: workspace bootstrap, index-my-archive (migrate endpoints w/ live report), debounced paginated search, moments list/create; PVR build verified free of it (build-time strip) |
| **F5 · Search → Editor journey** — a search hit or moment opens the Editor at that source/time | ✅ | hit/moment → `openRecordingInfo(recordingId, {seekSec})` → EDL editor at timecode; unlinked sources get a non-blocking explanation; also fixed stale `CREATOR_ENABLED` after fresh login (edition now re-resolved on sign-in) |
| **F6 · Phase 0 experience audit** — UX/user-journey, PVR + Pro performance, optimization and efficacy audit | ✅ | `docs/audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md` — 33 findings (6 P1); P1 remediation tracked below |
| **F7 · Audit P1 remediation** | 🟡 | F-32 ✅ (nine handlers onto `spawn_blocking`, five beyond the audit's four) · F-37 ✅ (600 s lock-acquisition timeout → existing transient-retry class, holder named in warn) · F-11/F-18/F-04 in flight · F-12 (creator-nav IA restructure) deferred to owner review |
| Pricing tier for the research platform | ⏸ | owner decision (strategy §8); constraint carried: no subscription for locally-running code |
| Local multitrack import seam | ⏸ | owner decision on phase ownership (strategy §9) |
| Remote guest studio recording | ⏸ | explicit non-goal (strategy §5); recorded to stop re-litigation |

---

## Cross-cutting blockers & hardening

| Item | State | Disposition |
|---|---|---|
| SPA edition awareness (creator UI hidden in PVR build) | ✅ | `creator_enabled` boot probe gates routes, nav, and actions |
| Daemon doesn't drive the pipeline executor | ✅ | durable daemon runtime + executable Crunchr adapter |
| Per-plugin SQLite fragmentation; canonical cross-signal exchange | ✅ | **CE-P1** research spine + migration adapters |
| `viewguard` `data_dir` double-nest | ✅ | Corrected; nested path retained read-only for migration |
| Corpus assembled client-side, not server-side | 🟡 | **CE-P2** |
| Licence JWT ES256 signature and claims verified | ✅ | ES256 signature, issuer, machine, expiry, licence expiry, and tier fail closed |
| Crunchr headless auto-transcribe | ✅ | recording-finished events reach resident plugins and enqueue the same durable DAG used by manual runs |
| ffprobe results uncached — re-analyses on every `/probe` | ✅ | `probe_cache` in `crates/strivo-web/src/routes/api.rs` (audit F-25 found this row stale; `docs/PERFORMANCE.md` already documented the fix) |
| Dynamic cdylib plugin loading coded but never triggered; no hot-reload | ⬜ | Deferred until third-party plugins are real |
| `yt-publish` marketplace entry needs YouTube OAuth | ⏸ | Deferred — needs Google Cloud creds |
| Creator Edition clippy gate | ✅ | CI runs `-D warnings` with the Creator feature |
| Windows build | ⬜ | The workspace does not compile on Windows: `tokio::net::Unix{Listener,Stream}` is used unconditionally in `src/daemon.rs`, `crates/strivo-web/src/ipc_client.rs`, `src/playback/mod.rs`, and `src/ipc.rs:236`. Needs a transport abstraction (named pipes), not a packaging fix |
| Release pipeline | 🟡 | Linux proven end-to-end via `workflow_dispatch` rehearsal (run 32185451150: tarball + completions + manpage + verified sha256). macOS/Windows blocked on offline self-hosted runners |

### Adversarial-review wounds (from the 2026-05-29 review, folded in)
1. **Identity collapse** — resolved: the PVR is now the product, the engine is
   the Creator Edition trajectory, and the edition split makes the boundary
   concrete in code.
2. **Architectural straddle (TUI + web)** — resolved; the TUI is gone (`2ab4e6c`).
3. **No recording service** — resolved by `src/intents/`.
4. **Doctrine without enforcement** — partially open; the DoD above is the gate.
5. **No customer / forcing function** — the PVR has direct competitors
   (streamerREC, CLI recorders) and an unoccupied "*arr-grade live-stream PVR"
   niche; the Creator Edition engine is the upsell thesis. A founder-level call.

---

## Appendix · shipped history (condensed)

- **0.1.0** (2026-03-14) — initial release: monitoring (Twitch/YouTube/Patreon),
  ffmpeg recording, playback, daemon, CLI, TOML config + keyring. *(TUI; removed.)*
- **0.2.0 – 0.3.0** (2026-04-19) — Tier-1 UX + P0/P1 quality.
- **0.3.0 → 0.4.0** — DAW phase-1 closeout (iters 21–53) + E2E audit + SPA polish.
- **0.4.0 → 0.5.0** — TUI removed (web-only), `strivo-plugins` folded into the
  workspace, `ab-render` + `submix`, backend integration batches (iters 54–84).
- **0.5.0 (post)** — **PVR / Creator Edition split** (the `creator` feature);
  sweep fixes (notify dispatch, atomic config/schedule writes, `creator_enabled`).

---

## Conventions

- Commit prefixes: `feat:` `fix:` `chore:` `refactor:` `ci:` `docs:` `test:` `perf:`.
- **No AI attribution** in commits, PRs, or code comments (per project CLAUDE.md).
- **Editions:** default build = PVR; `--features creator` = Creator Edition. Keep
  the PVR build free of creator deps; gate new creator surfaces behind the feature.
- A PVR slice is: change + tests + daemon/web wiring + SPA surface + E2E verify.
  A Creator slice adds: signal-store/contract change (where relevant) + pure-data
  crate + capability/marketplace registration. The wiring step separates 🟡 from ✅.

---

## revoy ledger block

Machine-readable current phase for the revoy cross-project ledger. Tracks the
outstanding (⬜) items of the near-term PVR phase (the product comes first); roll
to the next phase when these land. Keep in sync with the tables above.

<!-- revoy:begin -->
```toml
phase = "v0.6.0 release hardening"

[[todo]]
line = "Port the daemon/web IPC off unconditional tokio::net::Unix* onto a transport abstraction (named pipes on Windows) so the workspace compiles on Windows at all"
difficulty = 60
priority = "HIGH"

[[todo]]
line = "Bring the macos-sonoma and win11-ci self-hosted runners back online; both are offline and the release workflow cannot build those platforms without them"
difficulty = 20
priority = "HIGH"

[[todo]]
line = "Reconcile CHANGELOG: write the missing [0.4.0] and [0.5.0] sections, promote [Unreleased] to [0.6.0], bump the workspace version"
difficulty = 20
priority = "MED"

[[todo]]
line = "Update packaging/aur/PKGBUILD from 0.3.0 and decide whether the AUR package keeps --all-features (it currently ships Creator Edition, unlike the release tarballs)"
difficulty = 15
priority = "MED"

[[todo]]
line = "Marketplace catalog lists speculative plugins with author 'Chorosyne' and repo URLs that do not resolve; decide the brand string and whether the catalog ships at all"
difficulty = 15
priority = "LOW"
```
<!-- revoy:end -->
