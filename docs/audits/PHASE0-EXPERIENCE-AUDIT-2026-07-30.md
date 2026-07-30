# CE-Fusion F6 — Phase 0 Experience Audit

Date: 2026-07-30. Scope: UX/UI/user-journey, PVR performance, Pro/Creator
performance and experience, optimization and efficacy. Method: static
read-only review of code, config, and docs — no build, no live measurement.
Every claim below cites `file:line` or a named doc section; where a doc and
the code disagree, the code is treated as truth and the disagreement is
itself logged as a finding. This document is the sole deliverable of
CE-Fusion F6 (`ROADMAP.md` line 224) and feeds the F1–F5 rows of that table.

## Executive summary

The PVR core is in good shape. Monitor concurrency (`src/monitor/mod.rs:293`
`join_all` fan-out, `MissedTickBehavior::Skip`), recording-intent
centralization (`src/intents/`), the daemon's SQLite connection reuse
(`src/recording/persist.rs:79`), the 7-day calendar strip, and the dataviz
page all check out as solidly built and wired. No finding in this audit rises
to P0 — nothing found threatens the core "record what I asked it to record"
promise.

Friction concentrates in three places. First, **Creator/Pro discoverability**:
the Studio/Analytics/Publish top-nav panes are 50–90% placeholder tabs that
point a first-time Creator user at a raw API reference instead of the
feature, while the real controls for most of those same tools live one layer
down in the per-recording Info modal or the Crunchr transcript page — a user
relying on primary nav alone would reasonably conclude most of the toolkit is
vaporware, when it mostly isn't. Second, **the licence/monetization moment
itself is broken by omission**: the SPA never reads the `implemented` flag
the backend computes specifically to gray out "Start trial" before a user
without a licence backend clicks it and fails. Third, **async-runtime
hygiene in Creator render paths**: several handlers (`editor_render`,
`clipper_extract`, `thumbnails_generate`, `reuse_generate`) shell out to
ffmpeg/ffprobe synchronously from inside `async fn` bodies rather than via
`spawn_blocking`, and the pipeline's resource-lock semaphores have no
acquisition timeout — a single wedged external subprocess can silently stall
every other Creator stage waiting on that lock, forever.

A further theme threading through several findings: two accessibility
affordances StriVo advertises — a `ui.reduce_motion` setting that claims to
"mirror the OS-level prefers-reduced-motion," and the OS-level
`prefers-reduced-motion` media query itself — both fail to cover the
product's own signature REC-dot animation, and the manual toggle has *zero*
wiring anywhere in the codebase. This is worth fixing before any accessibility
claim is repeated in marketing copy.

Two ROADMAP.md entries were found to be stale against the code: the
"ffprobe uncached" cross-cutting blocker (already fixed, contradicts
`docs/PERFORMANCE.md` which correctly describes the fix) and the "Outbound
webhook / notification connectors ✅" PVR row (the backend dispatcher works;
there is no SPA control to enable or configure it, so shipping it silently
resulted in a TOML-only feature marked done under a UI-facing table).
`docs/SETTINGS-COVERAGE.md` is stale in its entirety — it audits the removed
TUI's settings tab, not the current SPA.

## Findings register

Sorted most-severe first. Persona: **PVR** = default-build user, **Creator**
= `--features creator` / Pro user, **Both** = affects the shared substrate.

| ID | Area | Sev | Effort | Persona | Evidence | Recommendation |
|---|---|---|---|---|---|---|
| F-11 | Creator journey | P1 | S | Creator | `crates/strivo-web/src/routes/licence.rs:38` computes `implemented: backend_url().is_some()` specifically so the SPA can disable Activate/Trial buttons cleanly; `spa.js` has zero references to `implemented` — `renderUpgradeCard` (`spa.js:6774-6821`) always renders live buttons that fail with a `Toast.error` only after the click, for every self-hosted user without `STRIVO_LICENCE_URL` set (the default). | Read `licence.entitled`/`licence.implemented` in `renderUpgradeCard`/`renderProUpsell`; when `!implemented`, disable the buttons and show the "backend not configured" hint inline instead of waiting for a failed POST. |
| F-12 | Creator journey / IA | P1 | M | Creator | `PRO_PANES` (`spa.js:6401-6440`) + `renderProApp` (`spa.js:6442-6475`): Studio 3/4, Analytics 3/6, Publish 9/10 tabs are `route: null` and render only a generic "reached elsewhere" message plus a raw `POST /api/v1/plugins/<slug>/<recording_id>` reference (`spa.js:6459-6466`). The real UI for most of those tools (cuepoints, clipper, thumbnails, tracks, reuse, casebook, EDL editor) lives in the per-recording Info modal (`spa.js:8265-8271`); chapters/brand-safety live in the Crunchr transcript page (`spa.js:7358-7359`). Dataviz is the sole exception — fully wired, real loading/empty/failure states (`spa.js:4836-4962`). | Either link each signpost tab directly to its real trigger (deep-link into the Info modal / Crunchr page from the pane), or collapse the three panes into a single "where things live" index page that is honest about indirection instead of implying each tab is its own tool. |
| F-18 | Accessibility / DESIGN.md | P1 | S | Both | Settings row "Reduce motion" (`spa.js:10426-10427`) reads: *"Disables non-essential transitions across the UI. Mirrors the OS-level prefers-reduced-motion."* `ui.reduce_motion` has exactly one reference in the entire SPA — the settings-row toggle itself. No class, `data-` attribute, or conditional logic anywhere in `spa.css`/`spa.js` reads the value back. The setting does nothing. | Wire `ui.reduce_motion` to a root class (e.g. `document.documentElement.dataset.reduceMotion`) and gate the same animations the `prefers-reduced-motion` media query should cover (see F-19), or remove the false claim from the settings copy until it's implemented. |
| F-19 | Accessibility / DESIGN.md | P2 | S | Both | `@keyframes pulse` (`spa.css:162-165`) drives both `.boot-glyph` (`spa.css:160`) and the signature `.live-now h2 .rec-dot` (`spa.css:785`) — DESIGN.md's named "REC dot" animation. Neither of the two `prefers-reduced-motion: reduce` blocks (`spa.css:1142-1145` toast only; `spa.css:1175-1177` button-busy spinner only) covers it; it animates unconditionally. Secondary: DESIGN.md's Motion section specifies "REC dot: 2s ease-in-out pulse" but the CSS implements 1.2s. | Add `.rec-dot`/`.boot-glyph` animation suppression to the existing reduced-motion blocks; reconcile the 1.2s/2s discrepancy with DESIGN.md or update the doc. |
| F-32 | Creator performance | P1 | M | Creator | Synchronous `std::process::Command` shelled directly from `async fn` bodies (not `spawn_blocking`, not `tokio::process`): `probe_duration`/`probe_resolution` (`crates/strivo-web/src/routes/plugins.rs:492,510`, called from `thumbnails_generate` at `:423,443,458` and `reuse_generate` at `:1079`, plus `:1229,1443,1603`); `strivo_clipper::extract_clip` (`crates/clipper/src/lib.rs:185`, called from `clipper_extract` at `plugins.rs:275`); `strivo_editor::render_edl_with_filters` (`crates/editor/src/lib.rs:278,293,314`, called from `editor_render` at `plugins.rs:1717`). The EDL-render path is worst: it blocks a Tokio worker thread for N sequential per-cut ffmpeg passes plus one concat pass — full transcode wall time, not a quick probe. | Wrap all of these in `tokio::task::spawn_blocking`, matching the pattern already used correctly for the 12 `ResearchStore` call sites (`plugins.rs:4357` et al.) and the VAD/beat-detect/loudness handlers, which already use async `tokio::process::Command` (`plugins.rs:2449,2626,3191`). |
| F-37 | Creator performance / reliability | P1 | M | Creator | `ResourceRegistry::acquire` (`src/pipeline/executor.rs:686-717`) ends in `sem.acquire_owned().await` (`:716`) with no `tokio::time::timeout` wrapper anywhere in the call chain; the only error path is the semaphore being explicitly closed. A stage that wedges (a hung whisper/ffmpeg subprocess, not a crash — a crash still drops the permit via `Drop`) holds its Gpu/Cpu/Disk/named-Api/File lock forever, and every other stage waiting on that same resource blocks indefinitely with no visible failure. | Add a bounded `tokio::time::timeout` around `acquire_owned`, surfaced as a typed stage failure (the DAG already has a failure-class/retry policy per `docs/PIPELINE.md`) rather than a silent hang. |
| F-04 | PVR journey / roadmap accuracy | P1 | S–M | PVR | `ROADMAP.md:120` marks "Outbound webhook / notification connectors ✅" and cites `src/webhook.rs` + `[notifications.webhook]`. The dispatcher is real and works, but it is config.toml-only: `grep -i webhook` across all of `spa.js` and `crates/strivo-web/src/routes/api.rs` returns zero matches — there is no Settings UI to enable, configure, or test it. | Either add a minimal Settings→Notifications webhook URL/enable control, or downgrade the ROADMAP row to 🟡 per the repo's own DoD ("stubs, inert modules... tracked as 🟡/⬜, never presented as shipped" — `ROADMAP.md:31-32`) until the SPA surface lands. |
| F-25 | PVR performance / doc accuracy | P2 | S (docs) | PVR | `ROADMAP.md`'s cross-cutting blockers table ("ffprobe results uncached — re-analyses on every /probe call ⬜", line 242) and the revoy ledger todo (lines 296-298) are stale. `crates/strivo-web/src/routes/api.rs:204-374` shows `/api/v1/recordings/{id}/probe` cached by path+size+mtime via `state.probe_cache` since commit `1cf6d8a` ("perf(web): cache ffprobe results by path+mtime to avoid re-probing"), matching `docs/PERFORMANCE.md`'s "Media probe results are fingerprinted by file size and modification time" claim exactly. | Flip the ROADMAP row to ✅ and drop the ledger todo; the real remaining ffprobe gap is F-32/F-33 (Creator render paths), not this endpoint. |
| F-01 | PVR journey | P2 | S | PVR | `renderRecordings()` (`spa.js:2552+`) fetches `API.recordings()` with a default `{limit: 500}` (`spa.js:96`) and never passes a `cursor` or exposes a next-page control (`loadMore`/`nextPage`/`page_size`: zero matches) despite the backend route supporting `cursor`/`limit` (`crates/strivo-web/src/routes/api.rs:115`). Libraries beyond 500 recordings are invisible past client-side filter/sort on the first page. | Add a "load more" control wired to the existing cursor param — the backend contract already supports it. |
| F-03 | PVR journey | P2 | S | PVR | First-run step 1 (`spa.js:1356-1357`) says "Authenticate Twitch / YouTube / Patreon by running `strivo` in a terminal (device-code login)." Settings→Platforms now has a full in-app credential wizard (`openPlatformWizard`, `spa.js:10513-10569`, POSTs via `API.setPlatform`). The onboarding copy is stale and sends new users to a slower path than the one that actually exists. | Update the first-run step-1 copy to point at the in-app Platforms wizard. |
| F-05 | PVR journey | P2 | S | PVR | `recording_dir` is shown read-only in Settings→General (`spa.js:10197`) and first-run explicitly says "Change it in `~/.config/strivo/config.toml` if needed" (`spa.js:1364-1366`). No editable input exists anywhere in the SPA. | Add a path input + validation to Settings→Recording; the backend already supports `settings/update`. |
| F-08 | Docs | P2 | S | Both | `docs/SETTINGS-COVERAGE.md` frames "Today" coverage as "Reachable from the TUI settings tab" (its own header) and is dated before the TUI's removal (`2ab4e6c`, per `ROADMAP.md:67`). Its exposure claims are wrong for the current product: it marks `poll_interval_secs` and `ui.reduce_motion` "hidden," but both are exposed in the current SPA (`spa.js:132,10185,10692,10743` and `spa.js:10426` respectively). | Re-run the settings-coverage audit against the SPA settings pane, not the removed TUI. |
| F-09 | PVR journey | P2 | M | PVR | Backup/restore is otherwise solid (confirm dialog `spa.js:10833`, loading/empty/failure states `spa.js:10679,10810,10847`), but restore explicitly warns "restart the daemon to apply" (`spa.js:10840`) with no in-app restart action anywhere — confirmed via grep, zero restart-daemon routes or buttons exist in `spa.js` or `crates/strivo-web/src/routes/*.rs`. | Either add a daemon-restart route+button (own the tradeoff: it would kill in-flight recordings), or make the warning link to `docs/DAEMON.md`'s manual restart steps. |
| F-13 | Creator journey | P2 | S | Creator | `POST /api/v1/plugins/broll/<recording_id>` is a real, gated endpoint (`crates/strivo-web/src/routes/plugins.rs:1843-1877`, registered `:4123-4124`), listed in the SPA's plugin marketplace catalog (`spa.js:6287,6434`) and in `PLUGIN_ROUTE_REDIRECTS` (`:6495`) — but zero fetch/button wiring exists anywhere in `spa.js`. Fully unreachable, unlike every other cataloged plugin. | Add the missing Info-modal trigger (same pattern as clipper/thumbnails), or unlist it from the marketplace until wired. |
| F-16 | Creator journey | P2 | S | Creator | Headless auto-transcribe (`crates/strivo-plugins/src/crunchr/mod.rs:105-134`, matching `docs/PIPELINE.md`'s "Crunchr headless auto-transcribe") only fires for channels/playlists in `tandem_channels`/`tandem_playlists` or with a `.crunchr-auto` marker file next to the recording. Neither is exposed anywhere in the SPA (`grep -i "crunchr.*tandem\|crunchr-auto"` across `spa.js`: zero matches) — enabling it requires hand-editing `config.toml`. | Expose a per-channel "auto-transcribe" toggle alongside the existing "Archiver tandem" row control (`spa.js:206`), which already has the right UI pattern. |
| F-20 | Accessibility | P2 | S | Both | `chrome()` (`spa.js:1008-1065`) has solid landmark structure — `<header role="banner">` (`:1040`), `<nav aria-label="Main navigation">` (`:1052`), `<nav aria-label="Channels">` (`:1061`), `<main id="content">` (`:1062`) — but no "Skip to content" link exists anywhere (`grep -i "skip.to.content\|skip-link"`: only an unrelated onboarding-tour "Skip tour" button at `:12500`). | Add a visually-hidden, focus-visible skip link as the first focusable element in `chrome()`, targeting `#content`. |
| F-26 | PVR performance | P2 | S–M | PVR | `crates/strivo-web`'s `AppState` (`server.rs:25-44`) holds no shared `PersistDb`/connection; every request-handling call opens `jobs.db` fresh — `open_jobs_db()` (`crates/strivo-web/src/routes/api.rs:1851-1854`, used by `history`/`blocklist_*`), plus direct opens in `health` (`:546-547`) and `remux_recording` (`:1119-1120`) — each re-running the full `PRAGMA`+`CREATE TABLE IF NOT EXISTS` schema batch (`src/recording/persist.rs:90-98`). The daemon side does this correctly (one `Arc<Mutex<Connection>>`, `persist.rs:79`, `daemon.rs:299,904`). | Thread a shared `Arc<Mutex<Connection>>` (or `PersistDb` handle) into `AppState`, mirroring the daemon's own pattern — the fix already exists in the codebase to copy from. |
| F-33 | Creator performance / docs | P2 | M | Creator | The EDL/highlight render path (`editor_render` → `crates/editor/src/lib.rs:242-359`) is **not** single-pass: one ffmpeg subprocess per cut (fade transcode or stream-copy trim, `:278-304`) plus one more for the final concat+branding pass (`:314-354`) — sequential, not one `filter_complex`. Only the simpler `clipper_extract` fast-cut path (`crates/clipper/src/lib.rs:179-199`) is genuinely single-pass. `docs/PERFORMANCE.md`/`docs/PIPELINE.md` do not explicitly claim single-pass rendering, but the audit brief's framing assumption should be corrected for any future doc that does. | If a "single ffmpeg pass" claim is ever documented for the full EDL render, correct it; otherwise no doc change needed — just don't let this assumption propagate into marketing/positioning copy. |
| F-36 | Creator performance | P2 | M | Creator | The crunchr transcription stage declares `ResourceLock::Gpu` + `ResourceLock::Disk{cap:2}` like any other stage (`src/pipeline/templates.rs:24-30,49-55`) — no transcription-specific queue, priority, or concurrency cap. `Gpu` is a single global permit (capacity 1, `executor.rs` `ResourceRegistry`), shared by every GPU-tagged Creator stage, not just transcription. | If transcription throughput becomes a real bottleneck (needs live measurement — see below), give it a named cap distinct from other GPU stages rather than contending on the single global GPU permit. |
| F-39 | Creator performance / reliability | P2 | S | Creator | The EDL renderer's `.edl-temp` scratch directory (`crates/editor/src/lib.rs:255-256`) is only best-effort cleaned up on success (`:356`); a failed multi-cut render leaves sub-clips on disk with no cleanup path, compounding pressure against the same `Disk{cap:2}` lock meant to bound Creator disk usage. | Wrap the render in a cleanup-on-any-exit guard (RAII drop guard or explicit `finally`-style cleanup on the error path). |
| F-02 | PVR journey | P3 | S | PVR | The multistream/watch-route tile player (`spa.js:6213-6216`, raw `<video src=".../download">`) has no `error` event listener, unlike the recordings-table lightbox player which handles it gracefully (`spa.js:9685-9688`: *"Playback failed — your browser may not support this codec. Try Download from the row menu."*). A missing/corrupt file in the watch route falls back to native browser broken-video UI. | Reuse the lightbox player's error handler on the watch-route tile. |
| F-06 | PVR journey | P3 | S | PVR | `poll_interval_secs` is shown read-only in Settings→General (`spa.js:10184-10186`) but is only editable on the separate System page (`spa.js:10690-10694,10735-10738`) — duplicated display, split editing surface. | Move the editable control to Settings, or link the read-only value to the System page control. |
| F-07 | PVR journey | P3 | M | PVR | `recording.format.video_codec`, `audio_codec`, `bitrate_kbps`, and the yt-dlp format selector are not exposed anywhere in the SPA (zero matches for any of these keys). | Low priority; document as an intentional "advanced, TOML-only" tier if that's the product decision, otherwise add to Settings→Recording (advanced section). |
| F-10 | Accessibility | P3 | S | Both | Sortable recordings-table column headers (`th[data-sort]`, `spa.js:2788`) are mouse-only — click handler at `spa.js:2694-2704` has no matching `role="button"`/`tabindex`/keydown anywhere in the file, unlike the compliant `.media-pill` (`spa.js:1564-1565`, has `role="button" tabindex="0"`) and recordings-table rows (`spa.js:2925-2948`, `tabIndex=0` + delegated keydown). | Add `tabindex="0"` + `role="button"` + Enter/Space keydown to `[data-sort]` headers, mirroring the existing `.media-pill` pattern already in the codebase. |
| F-15 | Docs | P3 | — | Creator | `docs/STRATEGY-NVIVO-RIVERSIDE.md:114` ("Multitrack and Demucs plugins already point this direction") overstates the current state: `grep -rln "demucs" crates/` returns nothing — no Demucs code exists anywhere. Multitrack itself is confirmed to be track probing/extraction only (`crates/strivo-web/src/routes/plugins.rs:891-935`), with exactly one working UI trigger (Info-modal "Audio tracks," `spa.js:8268,9501-9551`). | Reword the strategy doc to describe this as future direction, not existing groundwork, so a future contributor doesn't go looking for Demucs code that isn't there. |
| F-22 | DESIGN.md | P3 | M | Both | 77 hardcoded hex/rgb colors in `spa.css` outside the `:root` token block (plus 7 in `spa.js`) — a mix of genuine hardcodes worth fixing (repeated `color: #fff` at e.g. `spa.css:110,201,524,976,1247,1447,1916`) and legitimate `var(--x, #fallback)` CSS-custom-property fallback patterns (e.g. `:686,713,714`) that are not real deviations since `:root` always loads. | Mechanical pass to replace the genuine hardcodes with existing tokens (`--fg`, semantic colors); leave the `var(..., #fallback)` patterns alone. |
| F-27 | PVR performance / docs | P3 | M | PVR | `docs/PERFORMANCE.md`'s "cursor pagination" claim is true only in the loose API-shape sense. `load_recording_jobs_page` (`src/recording/persist.rs:372-410`) uses `LIMIT ?1 OFFSET ?2` (`:387`) plus a separate `COUNT(*)` every page load (`:378-382`) — the SPA's `cursor` param (`spa.js`, `api.rs:98,106,130,1807,1828`) is really an integer offset, not an opaque keyset cursor. `OFFSET` cost scales with the offset value; bounded by the 200/500-row page caps so likely fine today. | Needs live timing at high offsets before deciding whether to invest in a true keyset cursor; at minimum, correct the doc's terminology. |
| F-28 | PVR performance | P3 | M | PVR | `count_finished_recordings` (`src/recording/persist.rs:420-431`) is a genuine `SELECT COUNT(*)` (confirming the ROADMAP fix), but filters on `json_extract(payload,'$.channel_id')` with no covering index — `idx_jobs_state`/`idx_jobs_kind_updated` prune to `finished`/`Recording` rows, but the JSON extraction runs per matching row rather than an indexed equality lookup. Called once per channel with a cutoff profile on every monitor poll tick (`src/monitor/mod.rs:439-464`). | Needs live measurement (`EXPLAIN QUERY PLAN` + timing) before optimizing; only matters at high per-channel finished-job counts. |
| F-29 | PVR performance | P3 | L if triggered | Both | `spa.js` = 605,226 bytes, `spa.css` = 153,992 bytes uncompressed (`wc -c`), single monolithic bundle, no code-splitting (`crates/strivo-web/build.rs:31-37` only strips `@creator-start/@creator-end` blocks for edition gating, not route chunking). `CompressionLayer::new()` is applied globally (`crates/strivo-web/src/server.rs:9,132`) but actual br/gzip compressed size is not knowable statically. | Needs a live compressed-size measurement (`curl -H 'Accept-Encoding: br' ... -w '%{size_download}'`) against `docs/PERFORMANCE.md`'s 250 KiB compressed-JS split threshold before deciding whether to chunk. |
| F-30 | PVR performance | P3 | — | Both | SSE (`crates/strivo-web/src/routes/events.rs:26-75`) has no server-side broadcast/fanout structure — each browser tab opens its own independent Unix-socket `IpcClient::events()` connection to the daemon, each redundantly JSON-decoding/re-encoding the same event stream. Events are per-delta (`DaemonEvent` variants), not full-state snapshots — correctly implemented. `KeepAlive` 15s + `X-Accel-Buffering: no` present. | Fine at PVR/home-user tab counts; note as a scaling consideration only if many concurrent browser sessions become common. |
| F-34 | Creator performance | P3 | S | Creator | 12 `ResearchStore::open()` call sites (`crates/strivo-web/src/routes/plugins.rs:4358,4382,4406,4437,4470,4504,4542,4574,4608,4630,4661,4685`) open a fresh SQLite connection per request inside `spawn_blocking` (non-blocking to the runtime, but no connection reuse/statement caching). Currently zero real traffic hits this cost — the endpoints have no SPA callers (F-14 / CE-Fusion F4). | Low urgency until F-14/F4 lands; fix alongside the SPA Archive surface work, not in isolation. |
| F-35 | Creator performance | P3 | M | Creator | `PipelineRegistry::ready_stages()` (`src/pipeline/executor.rs:220-244`) is a full O(P×S²) scan of every non-terminal pipeline × stage × input-dependency, invoked from `PipelineRuntime::dispatch_ready` (`src/pipeline/runtime.rs:39-56`) on every `Notify`-driven wake (event-driven, not a busy poll) — re-scanning the entire registry (bounded at `MAX_PIPELINE_HISTORY=500`, `executor.rs:21`) on every single stage completion rather than incrementally. | Needs live measurement at realistic concurrent-pipeline counts before optimizing; likely fine at single-digit/low-dozens concurrent Creator pipelines. |
| F-38 | Creator performance | P3 | S | Creator | `event_tx`/`action_tx` in `PipelineRuntime` (`src/pipeline/runtime.rs:19,29`) are unbounded `mpsc` channels — no backpressure if the SSE consumer or plugin-action consumer stalls or falls behind. | Bound the channels with an explicit capacity and a documented overflow policy, or confirm (via live load test) this is a non-issue at expected message rates. |
| F-14 | Creator journey | Informational | — | Creator | Confirmed exactly as CE-Fusion F4 predicts: the 11 research-kernel endpoints (`crates/strivo-web/src/routes/plugins.rs:4146-4186`, under `/api/v1/research/projects/...`) have zero SPA callers — `grep "research"` in `spa.js` only matches unrelated code comments citing roadmap doc sections. | No action from this audit — tracked by CE-Fusion F1–F4 already. |

**Severity totals:** P0: 0 · P1: 6 (F-11, F-12, F-18, F-32, F-37, F-04) · P2: 13 (F-25, F-01, F-03, F-05, F-08, F-09, F-13, F-16, F-20, F-26, F-33, F-36, F-39) · P3: 13 (F-02, F-06, F-07, F-10, F-15, F-22, F-27, F-28, F-29, F-30, F-34, F-35, F-38) · Informational: 1 (F-14). Total: 33 findings.

## Per-journey narratives

### PVR: onboarding → monitor → recording → library → watch

Onboarding is a real 3-step checklist (`renderFirstRun`, `spa.js:1332-1386`)
with done/todo icons (not color-only: ✓/○ glyphs), per-platform pills, and
re-check/continue actions — solid. Its copy is stale on the fastest path to
finishing step 1 (F-03). Note it does not surface the `strivo doctor`
external-tool check (`ffmpeg`/`mpv`/`streamlink`/`yt-dlp`) that
`docs/FIRST-RUN.md` documents as a manual pre-check step — this is
consistent with the doc (doctor is a CLI-only diagnostic, not part of the
in-app checklist), not a gap, but worth knowing the SPA never confirms tool
availability itself.

Adding a channel (`spa.js:2374-2451`) is a clean two-step wizard with
resolve-failure and search-failure states surfaced inline, not swallowed.
Go-live detection fans out concurrently per platform
(`src/monitor/mod.rs:293`) and dispatches through the single canonical
`start_recording` translator (`src/intents/start.rs:16-34`) from all three
trigger sites — monitor auto-record (`monitor/mod.rs:361-375`), manual/API
start (`daemon.rs:1077`), and scheduled start (`recording/schedule.rs:288`)
— which is exactly the architecture `src/intents/mod.rs:9-16`'s own comment
says was built to prevent a real historical bug (webui and TUI silently
diverging on cookie/output-path handling for gated YouTube streams). This is
a genuine strength worth preserving under future changes.

The "Monitor" nav destination (`TOPNAV` route `schedule`, `spa.js:992`) is a
single page serving both channel monitoring and the calendar: the 7-day
strip (`buildCalStrip`, `spa.js:12019-12057`) is confirmed real, matching
`ROADMAP.md:118` — not a doc/code mismatch. Library browsing caps at 500 rows
with no load-more control despite backend cursor support (F-01); playback
error handling is inconsistent between the table-row lightbox (graceful) and
the watch-route tile (silent native fallback, F-02).

### PVR: settings → backup/restore → health

Settings is genuinely broad — 8 sections, full in-app platform credential
wizard, filename templates, container/transcode/quality-tier controls,
desktop-notification toggles — but has three concrete config-surface gaps:
no webhook UI at all (F-04), `recording_dir` is TOML-only despite being the
single most consequential path in the product (F-05), and `poll_interval_secs`
lives on a different page than its read-only display (F-06). Backup/restore
is the best-built destructive-action flow in the app — confirm dialog,
loading/empty/failure states all present — undercut only by the missing
restart-daemon affordance after a restore (F-09). Health checks are wired
(`API.healthChecks()`, `spa.js:1112,10585`) into the System page alongside
the storage gauge and concurrent-slot indicator both confirmed present per
`ROADMAP.md:121-122`.

### Creator: licence → transcription → pipelines → publish

The licence journey is where the product's monetization moment meets its
weakest UX: the backend was built with a specific mechanism
(`implemented`) to avoid exactly the failure mode that ships today — a live,
clickable "Start trial" button that always fails for the majority of
self-hosted installs (F-11). Transcription and the Pipelines page are both
genuinely solid: real search, re-transcribe, chapters/brand-safety triggers,
live SSE-driven run history with cancel/retry/artifact-download. Automatic
transcription exists but is invisible — gated on config-only tandem lists or
marker files with no SPA toggle (F-16).

"Publish" is the pane a Creator user would click to review the DAG's actual
output (chapters/captions/brand-safety/highlights/clips/thumbnails/reuse
drafts/Casebook per `docs/PIPELINE.md`), and it is 90% signpost cards
pointing elsewhere (F-12). The functionality is not vaporware — it mostly
exists, reachable from the Pipelines run-detail artifact downloads and the
per-recording Info modal — but the information architecture actively hides
it from the nav destination named for it. One tool, B-roll finder, really is
fully dead (F-13); the "Demucs" local-multitrack-import direction referenced
in the strategy doc doesn't exist in code at all yet (F-15, informational
for future contributors). Dataviz stands out as the one Creator-nav
destination that is fully built to the standard the others should meet.

## Needs live measurement

The following cannot be honestly assessed from static code and are listed as
future measurement tasks, not guesses. `CE-Fusion F3` (route-level latency
telemetry) is landing concurrently and will supply some of this once merged.

- Real startup time, first-render time, and resident memory under a
  representative large library (`docs/PERFORMANCE.md`'s own stated
  methodology — "run a release binary against a copy of a large library").
- Actual p95 latencies for cached vs. uncached API reads against the
  100ms/500ms budgets in `docs/PERFORMANCE.md`, especially for the
  per-request `PersistDb::open()` paths (F-26) and the `json_extract`-filtered
  `count_finished_recordings` call under high per-channel finished-job counts
  (F-28).
- Actual compressed (br/gzip) size of `spa.js`/`spa.css` against the 250 KiB
  compressed-JS split threshold (F-29) — only the 605 KB/154 KB uncompressed
  sizes are known statically.
- Whether the `OFFSET`-based recordings pagination (F-27) actually degrades
  at realistic offsets, or is masked by the 200/500-row page caps.
- Whether the pipeline executor's O(P×S²) full-registry rescan on every wake
  (F-35) is measurable at realistic concurrent-Creator-pipeline counts, or
  negligible.
- Whether the single global `Gpu` semaphore is the real transcription
  throughput ceiling in practice, or whether the whisper/whisperx/voxtral
  subprocess itself dominates (F-36) — needs an actual transcription run
  under concurrent Creator load.
- Whether any stage-execution timeout exists elsewhere in the plugin
  dispatch path that indirectly bounds the missing lock-acquisition timeout
  (F-37) — none was found in `executor.rs`/`runtime.rs`, but a live hang
  test (kill `-STOP` a transcription subprocess mid-run) would confirm the
  blast radius.
- SSE fanout cost (daemon CPU, per-connection overhead) under many
  simultaneous browser tabs (F-30) — no evidence of a structural problem in
  the code, but the N-independent-IPC-connections pattern has not been load
  tested.
- Whether users actually get stuck at the licence/trial dead-click (F-11) or
  the Publish-pane signposts (F-12) in practice — this audit found the code
  path, not user behavior; a usability session or analytics on the (still
  unbuilt) F3 telemetry would confirm real-world impact.

## Mapping to ROADMAP

**Existing rows this audit confirms, corrects, or extends:**

- `ROADMAP.md:242` (ffprobe uncached, cross-cutting blockers) and the ledger
  todo (`:296-298`) — **stale, should flip to ✅** (F-25). The real remaining
  ffprobe/ffmpeg gap is the blocking-`Command`-in-async-handler pattern
  (F-32), a distinct issue in Creator render paths, not the cached `/probe`
  endpoint the ROADMAP row describes.
- `ROADMAP.md:120` ("Outbound webhook / notification connectors ✅") — **DoD
  gap**: backend done, SPA surface missing (F-04). Recommend 🟡 pending a UI
  task, per the repo's own "wired end-to-end" definition of done
  (`ROADMAP.md:29-32`).
- `ROADMAP.md:118` (calendar strip) — **confirmed accurate**, no change
  needed.
- CE-Fusion F4 (`ROADMAP.md:222`, SPA Archive surface, ⬜) — **confirmed
  correctly not started** (F-14); this audit adds the exact 11-endpoint list
  as a ready-made checklist for F4's implementer.
- `docs/STRATEGY-NVIVO-RIVERSIDE.md:114` (Demucs direction) — **wording
  should be softened** to avoid implying existing groundwork (F-15).
- `docs/SETTINGS-COVERAGE.md` — **needs a full re-audit** against the SPA;
  currently describes the removed TUI (F-08).

**New items recommended for the Cross-cutting blockers & hardening table:**

- Wire or retire the `ui.reduce_motion` setting (F-18); extend
  `prefers-reduced-motion` coverage to the REC-dot animation (F-19).
- Add a bounded timeout to `ResourceRegistry::acquire` (F-37) — currently a
  single wedged Creator subprocess can silently stall the entire pipeline
  system with no visible failure, which is a sharper risk than most existing
  rows in that table.
- Wrap the remaining blocking `Command::new` call sites in Creator handlers
  (`clipper_extract`, `editor_render`, `thumbnails_generate`,
  `reuse_generate`) in `spawn_blocking` (F-32).

**New items recommended for the PVR near-term roadmap:**

- Thread a shared, reused SQLite connection into `strivo-web`'s `AppState`
  (F-26), mirroring the daemon's existing correct pattern.
- Add a "load more" control to the recordings library using the already-
  supported cursor param (F-01).
- Add an editable `recording_dir` field to Settings (F-05).

**New items recommended for the Creator Edition roadmap:**

- Resolve the Studio/Analytics/Publish signpost-tab information architecture
  (F-12) — highest-leverage single UX fix in this audit given it affects the
  first impression of the entire Creator toolkit.
- Fix the licence-activation dead-click (F-11) before any pricing/positioning
  push per `docs/STRATEGY-NVIVO-RIVERSIDE.md` §8.
- Wire or unlist the B-roll finder plugin (F-13).
- Expose per-channel auto-transcribe as a Settings toggle (F-16).

---

*One file written by this audit: `docs/audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md`. No other files were modified.*
