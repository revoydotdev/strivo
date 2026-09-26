# Performance architecture

Strivo's performance policy favors bounded work and observable degradation over
unlimited concurrency. These budgets are release gates and tuning targets, not
claims about every host.

## Budgets

| Surface | Budget |
| --- | --- |
| API application time, cached reads | p95 under 100 ms |
| API application time, uncached local reads | p95 under 500 ms |
| Initial recordings/history DOM | at most 200 rows |
| History API page | 200 rows by default |
| Recordings API page | 500 rows by default |
| Concurrent interactive media probes/remuxes | 2 |
| Concurrent Creator CPU/disk-heavy stages | 2 per resource |
| Browser background polling | paused while the page is hidden |

Every HTTP response emits a `Server-Timing` application duration and the server
logs structured route, status, and duration fields. Monitor polls log elapsed
time and channel count. Use those signals to find regressions before increasing
limits.

## Implemented controls

- Brotli/gzip response compression and a compact SVG application mark reduce
  transfer size.
- Identical short-lived GET requests are coalesced in the browser and cached for
  two seconds. Mutations and lifecycle events invalidate affected resources;
  reads from an older generation retry through the current coalesced request.
  Progress ticks patch shared records without repeatedly retiring pending reads.
- Navigation keeps the application shell mounted and rejects abandoned route
  commits. Shell hydration runs independently; Home's Patreon, schedule, and
  health requests do not block its core content.
- Recording progress patches affected keyed rows/cards. Table browsing uses a
  200-row window; loading more data does not grow the rendered table indefinitely.
  Lifecycle refreshes preserve loaded pages, unrelated menus, focus, and selection.
- Recordings and durable history support bounded offset pagination. The API
  field is named `cursor`, but it carries a numeric offset; it is not a
  keyset cursor. The UI incrementally loads history and caps each DOM render,
  preserving responsiveness for large libraries. Measure high-offset queries
  before replacing this shape.
- Recording list/detail/playback resolve the durable journal and overlay live
  snapshot state, so daemon memory eviction does not make archives unplayable.
  Snapshot-only active jobs participate in the numeric pagination offset.
- Media probe results are fingerprinted by file size and modification time.
  Probe, remux, and thumbnail processes share a two-slot worker pool. Thumbnails
  additionally have a one-slot limit, coalesce by recording ID, and cache failures
  briefly. Probe and individual thumbnail attempts have 30-second deadlines;
  remux deadlines allow 120 seconds plus file size at 4 MiB/s. Cancelled media
  processes are killed, and thumbnail temporary files are cleaned up.
- Creator stages declare API, CPU, and disk resource locks. Generated artifacts
  are reused when source and transcript fingerprints still match.
- Capture records every interruption it survives, and a cross-check holds that
  record to the media. `recording::gaps::GapLedger` stores the measured interval
  and a reason (`recorder-exit`, `stall`, `restart`, `disk-pressure`) for each
  gap-resume retry, the stall stop, a daemon restart, or a deliberate disk stop.
  `GapLedger::audit` differences that ledger against the timeline — wall clock
  elapsed versus the stream duration `ffprobe` measures on the finished media —
  and reports both directions: an *unrecovered* interval the ledger does not
  explain, and a *spurious* one the media does not support. The media side is
  measured from the bytes rather than from a counter the recorder also wrote, so
  a short encode or a lossy `mkvmerge` join shows up instead of cancelling out.
  The tolerance is a stated 5 s per session, three orders of magnitude below the
  5–45 s outages the recorder actually produces.
- Rendering yields to capture on disk pressure. A `Disk` stage lock is refused
  before the permit is taken when free space on the recording volume is not
  *above* `monitor_limits.disk_budget_reserved_gb`, which is the same value the
  monitor's auto-record gate already uses — one number, two consumers. The
  refusal is `LockAcquireError::CaptureReserve`, which carries both byte counts
  and reaches `StageState::Failed`/`last_error`, so a stopped render says why
  rather than retrying silently against a full volume. It is deliberately not
  routed down the transient lock-timeout path. Capture may use the whole volume;
  only render is bounded. A reserve of 0 leaves the check inert, matching that
  config field's existing convention. This is a second bound on a different
  resource (bytes, not recorder slots) and does not alter the PVR concurrency
  gate in `ROADMAP.md`.
- `scripts/soak_capture_pressure.sh` rehearses the three together. It encodes six
  hours of real stream time, merges each session's parts through
  `recording::segments::merge_segments`, measures every part with `ffprobe` and
  **fully decodes** it rather than size-checking, and audits the result. Its
  disk-pressure phase fills a private size-capped tmpfs with real bytes until
  free space crosses the reserve, then asserts capture still writes a decodable
  part while the render is refused through the real `ResourceRegistry`.
  `STRIVO_SOAK_MUTATE=drop-ledger-entry|phantom-ledger-entry` deliberately
  corrupts the graded ledger, and both must turn the run red; a clean mutated run
  is a defect in the rehearsal. **The six hours is six hours of stream time, not
  six hours of wall clock** — the run completes in seconds, so real-time
  behaviour (a wedged child, a slow volume, an hour-five descriptor leak) is not
  proven by a green run. See `.omo/evidence/task-39-strivo-review-and-pro-edition.json`.
- Long-media navigation reads a proxy, never the recording. `src/media/` holds
  the pieces: `proxy.rs` transcodes a source once into a 1280x720
  densely-keyed proxy, `pyramid.rs` builds the waveform pyramid and the
  filmstrip from *that proxy's* audio and frames, and `preview.rs` is the
  bounded interaction surface. The proxy is addressed by the **content
  fingerprint** of the source — `sha256` over the whole file, the rule todo 33
  defines — so a rename keeps the proxy and an edit of any byte loses it. A
  change to the source produces a different cache key, a different artifact, and
  `rebuilt: true` on the resolve, and the tests assert the rebuilt proxy's
  *bytes* differ from the stale one's rather than only that the key moved.
  `preview.rs` is bounded at **`PREVIEW_CACHE_ENTRIES` = 64** decoded frames,
  least-recently-used, and an evicted entry's file is deleted rather than
  orphaned; the test fills the cache to exactly the bound and then one past it.
- The proxy/original time mapping is **measured, not assumed**.
  `proxy::ProxyIndex` holds both files' keyframe presentation times, read from
  container *packets* (`-show_entries packet=pts_time,flags`) rather than
  frames, because `-show_entries frame=` makes ffprobe decode the whole file —
  the cost this control exists to avoid. `proxy_time_for` interpolates
  piecewise between the bracketing keyframes, so a proxy whose timebase drifted
  would be mapped by its own measured keyframes instead of by an identity the
  container does not support. On `fixtures/media/landmarks.mkv` — where second
  *k* is one flat luma value and the seconds are distinguishable from each
  other by that value alone — a seek to each of the eight seconds through the
  proxy decodes a frame whose mean luma matches the luma the **source** shows at
  that second, within **2/255**. The oracle is a direct ffmpeg call in the test,
  sharing no code with the product, because a helper both sides call would
  validate the mapping with the thing being tested.
- "No interaction path spawns an original-resolution decode" is observed, not
  asserted. `media::spawn` records **every** child process these paths start —
  tool, purpose, input path, whether the input is a generated proxy, and the
  picture size it was told to decode — into a ring bounded at 256 entries.
  Recording is always on, so a test cannot pass by recording nothing, and the
  tests assert the record is non-empty and that the frame, waveform and
  filmstrip interactions each appear in it before asserting the absence. The
  routes that make this reachable in the product are
  `GET /api/v1/recordings/{id}/preview` and `.../waveform`: they take a
  **recording id, not a caller-supplied path**, so they need no media-admission
  gate and stay in the PVR edition. A test additionally drives a deliberately
  regressed interaction — a frame decode pointed at the user's file — through
  the same predicate and requires it to be caught, and the whole suite was run
  once with `preview.rs` mutated to do exactly that, which turned the assertion
  red naming all eight offending spawns.
- Concurrency: the interaction paths acquire the **same two slots** and the same
  **30 s** backstop `docs/PERFORMANCE.md` states for interactive media work
  (`media::spawn::MEDIA_SLOTS`, `ACQUIRE_TIMEOUT`), rather than a third pair of
  numbers. The acquisition is bounded **at the site** with
  `acquire_slot_within`, and the refusal names the bound — the fix for the
  unbounded acquisition todo 37 recorded about the web layer's own pools. A test
  holds both permits and requires a third request to be refused rather than
  queued without limit.
- `scripts/bench_media_pool.sh` grew a `proxy` workload (report R02's 500 ms
  p95 target), reusing todo 9's instrument rather than a new benchmark: a
  release binary in a throwaway XDG sandbox, the seeded jobs.db, the
  `Server-Timing: app;dur` signal the budget table is written against, the real
  children sampled out of `/proc`, and machine-readable JSON. The cold proxy
  build is measured and reported **separately** and is never folded into the
  seek percentile, because a transcode is not a seek. Four clauses guard the
  verdict against a vacuous pass: every seek must have answered 200 with a
  timing header, an ffmpeg must actually have been observed, the cold build must
  have answered, and the p95 must be compared to the budget. The harness's
  `host` block now records `cpu_model` and `total_mem_bytes` — the baseline
  todo 9 captured had a CPU *count* and no model, so a latency figure from it
  could not be attributed to a machine. See
  `.omo/evidence/task-50-strivo-review-and-pro-edition.json`.

  **Measured** (`scripts/bench_media_pool.sh`, `proxy` workload, verdict
  `pass`, no problems; `.omo/evidence/task-50-strivo-review-and-pro-edition.json`).
  Host, named because no document in `docs/` declares a baseline machine and a
  latency figure whose host cannot be recovered is not comparable with
  anything: **12th Gen Intel(R) Core(TM) i9-12900K, 24 threads, 62.5 GiB RAM,
  kernel 7.2.6-zen2-1-zen, `loadavg_1m` 18.03 — not an idle host** (other lanes
  were running). Fixture: a six-hour source, 160x90 at 1 fps, 21 600 s of
  stream time, the resolution and the "stream time not wall clock" convention
  `scripts/soak_capture_pressure.sh` already uses. Proxy 1280x720, 60
  sequential seeks spread deterministically across the whole timeline:

  | Measurement | Value |
  | --- | ---: |
  | Seeks answered 200 with a `Server-Timing` header | 60 / 60 |
  | Application time p50 | 226.6 ms |
  | **Application time p95 (the R02 budget)** | **265.55 ms** (budget 500 ms) |
  | Application time max | 1 052.45 ms |
  | Client wall p95, recorded beside it | 266.17 ms |
  | Peak concurrent media children | 1 (declared bound 2) |
  | Cold proxy build, reported separately | 279 054 ms |

  Three things a reader must not misread. **The host was not idle**, so 226 ms
  is a loaded figure and an idle host would be expected to do better; the
  harness records `host.idle` so a future capture cannot be mistaken for a
  clean one. **The cold build is 279 seconds** and is not in the percentile: a
  transcode is not a seek, and folding it in would make a p95 that describes
  nothing. **The 1080p-proxy case the report asks for is not measured**: the
  *proxy* is 1280x720, which is the size
  `docs/CREATOR-EDITION-REPORT-2026-09-07.md:408` names, but the *source* is
  160x90 at 1 fps, at which a one-second keyframe interval makes every frame an
  I-frame — the six-hour proxy is ~700 MB all-intra and the seek cost is
  dominated by opening it. A 30 fps recording would have a 30-frame GOP and a
  much smaller proxy, so this figure is pessimistic on that axis and
  optimistic on resolution. Cold-cache and network-source behaviour, which
  `:408` also asks to be reported separately, is not measured at all.
- SQLite job persistence uses WAL, normal synchronization, a busy timeout, and
  an index matching the paginated history query.
- Poll timers skip missed ticks, and nonessential browser polling pauses while
  the document is hidden.

## Release checks

Run:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo build -p strivo-web --locked
(cd crates/strivo-web/e2e && npm run check:pvr-bundle)
(cd crates/strivo-web/e2e && npm run test:performance)
```

CI also runs the performance regressions against the exact PVR release assets,
preserved before building Creator. Set `STRIVO_E2E_ASSETS_DIR` to that build's
`out/assets` directory to repeat the release-asset lane locally. Results and
measurement caveats for the September 8 remediation are in
[the tranche report](audits/2026-09-08-performance-ux/TRANCHE.md).

For representative production measurements, run a release binary against a
copy of a large library and record API latency, resident memory, first-render
time, and active child-process count. Do not benchmark against a live production
database. `scripts/bench_library.sh` does exactly this against a throwaway
synthetic library and writes the machine-readable baseline; its captured run
and the caveats that apply to reading it are in
[docs/perf](perf/README.md).

## Bundle measurement

The threshold below is on **compressed** JavaScript, so it can only be answered
by a request against a running server: the wire size is chosen by the
`CompressionLayer` in `crates/strivo-web/src/server.rs`, not by anything in the
build. `scripts/measure_bundle.sh` boots a release `strivo` binary in a
throwaway XDG sandbox, confirms by content hash which bundle that binary embeds,
then takes three requests per asset per encoding and asserts the server answered
with the `Content-Encoding` that was asked for. A "brotli" number that is really
gzip because the layer ignored `Accept-Encoding` is the failure mode that check
exists to prevent. `scripts/check-bundle-size.sh` re-measures and fails when a
recorded figure has drifted more than 5%, so this section cannot go stale.

Raw bytes are the uncompressed response body; gzip and brotli are `size_download`
read off the wire. They are different quantities, and only the compressed ones
are compared against the threshold.

| Edition | File | Raw | gzip | brotli | Spread over 3 samples |
| --- | --- | ---: | ---: | ---: | --- |
| PVR 0.8.0 | `spa.js` | 388 381 | 103 139 | 101 362 | none (min = median = max) |
| PVR 0.8.0 | `spa.css` | 92 784 | 17 290 | 18 887 | none (min = median = max) |
| Creator 0.8.0 | `spa.js` | 587 472 | 152 828 | 150 596 | none (min = median = max) |
| Creator 0.8.0 | `spa.css` | 134 576 | 24 159 | 26 359 | none (min = median = max) |

These figures were re-measured after Mediabunny, AntV X6, and wavesurfer.js were
vendored under `crates/strivo-web/assets/vendor/`. Neither bundle grew by the
vendored size, and that is the expected result rather than a lucky one: all three
load through a dynamic `import()` at their point of use, and no surface imports
them yet, so they are embedded bytes a Creator binary carries but not bytes the
initial parse path pays for. A PVR build drops the tree outright, so the
vendoring is byte-free there. The figures above are also 0.3–0.8% above the
pre-vendor measurement, which is the drift of unrelated SPA edits landing in the
same window; `scripts/check-bundle-size.sh` treats 5% as the staleness bound.

**Threshold result, per edition — both are under the compressed-size trigger,
and the decision is NO SPLIT.** 250 KiB is 256 000 bytes. PVR `spa.js` is
393 030 raw / 104 552 gzip / 102 761 brotli, i.e. **0.4084x** the threshold on
gzip. Creator `spa.js` is 650 097 raw / 169 945 gzip / 168 159 brotli, i.e.
**0.6638x** the threshold on gzip — 86 055 bytes of headroom. Neither condition
holds in either edition, so neither bundle is split.

Both conditions were evaluated this time, which is what makes "NO SPLIT" a
decision rather than a deferral. Todo 53 measured the parse/evaluation term
against a named client (headless Chromium, cold cache, 9 runs per edition,
medians and extremes reported) because the project defines no minimum-client
matrix: PVR compiled and executed in an 11.3 ms median against the 150 ms bar.
The Creator bundle has three static imports, so `new Function` cannot take it;
the honest figure there is an **upper bound** — the resource's transfer plus
everything to `DOMContentLoaded`, 70.4 ms median and 74.5 ms worst of nine — so
the parse/eval term inside it is necessarily under the bar too. A split would
not move the Creator figure materially in any case: the transfer is 17.5 ms of
that window, so chunking adds a round trip to hit work already inside it. The
mechanism is available (`build.rs` filters by filename) but a real per-route
split needs a second output file and a second `<script type="module">` in
`assets/spa.html`, which is a change to the shell's shape. See
`.omo/evidence/task-53-strivo-review-and-pro-edition.json`.

Two things a reader must not misread:

- In **both** editions `spa.css` is *smaller* over gzip (17 242 / 24 106) than
  over brotli (18 836 / 26 293). The server picks an encoding from the request's
  `q` values rather than from a size comparison, so at the layer's default
  quality brotli loses on a file this small. Quoting the brotli figure alone for
  the CSS overstates it.
- Every figure above came off the wire. The Creator row in particular required a
  Creator-featured release build, because the two editions share one
  `release/strivo` output path: the PVR binary was copied aside, the Creator
  binary built, measured, and the PVR binary restored and hash-verified. Sizing
  either bundle by compressing the file on disk is the F-29 mistake
  ([PHASE0-EXPERIENCE-AUDIT](audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md)).

<!-- bundle-measurement:begin -->

```json
{
  "threshold_compressed_js_bytes": 256000,
  "tolerance_pct": 5,
  "editions": {
    "PVR": {
      "wire_measured": true,
      "note": "re-measured 2026-09-26 against the strivo 0.8.0 release PVR bundle over HTTP; 3 samples per point, every Content-Encoding asserted; figures are sample medians. build.rs removes assets/vendor/ from a PVR build, so the vendoring is byte-free here. Refreshed by todo 59 from todo 53's measurement after todos 39/40/44/47/50/53 landed",
      "spa.js": { "raw_bytes": 393030, "gzip": 104552, "brotli": 102761 },
      "spa.css": { "raw_bytes": 92784, "gzip": 17290, "brotli": 18887 }
    },
    "Creator": {
      "wire_measured": true,
      "note": "re-measured 2026-09-26 against a Creator-featured strivo 0.8.0 release build over HTTP; 3 samples per point, every Content-Encoding asserted; figures are sample medians. The vendor tree is embedded in a Creator build but nothing imports it yet, so it costs binary size and not parse time. Refreshed by todo 59 from todo 53's measurement: the earlier 152 828 gzip was +11.20% stale, outside the file's own 5% tolerance, which is what made scripts/check-bundle-size.sh fail",
      "spa.js": { "raw_bytes": 650097, "gzip": 169945, "brotli": 168159 },
      "spa.css": { "raw_bytes": 134576, "gzip": 24159, "brotli": 26359 }
    }
  }
}
```

<!-- bundle-measurement:end -->

Reproduce with a release binary and the `out/assets` directory of the same
build — the assets directory is what identifies the edition, by content. The two
editions share one `release/strivo` path, so measure one, copy the binary aside,
build the other, then measure it and restore the first:

```sh
scripts/measure_bundle.sh <release-strivo> <build>/out/assets PVR
scripts/check-bundle-size.sh <release-strivo> <build>/out/assets PVR
# cargo build --release -p strivo-bin --features creator --locked, then:
scripts/measure_bundle.sh <release-strivo> <build>/out/assets Creator
scripts/check-bundle-size.sh <release-strivo> <build>/out/assets Creator
```

A release binary copied out of the target directory no longer lives under a
`release/` path, so the scripts ask for
`STRIVO_BENCH_ALLOW_NONRELEASE=1` to measure a preserved copy. That override is
for a preserved *release* binary only; a debug build is never comparable.

## Recordings/history query shape

Measured 2026-09-25 with the harness from [docs/perf](perf/README.md) against
the `strivo 0.8.0` release PVR binary (sha256 `38cacd0f…`), 60 requests per
workload, `limit=500` (the API's own maximum), concurrency 1. Latency is
`Server-Timing: app;dur`, the signal the budget table above is written against.
**The host was not idle** — `loadavg_1m` was 7.1–7.8 for the two runs below on
24 CPUs, and 13.9–15.1 for the repeat pair; see the caveat below before
treating any delta as a regression.

### p95 by endpoint and offset, 10 000-recording library

Two consecutive runs, both with 60 requests per workload. `rows` is what the
page actually held.

| Endpoint | Offset | rows | run A p50 | run A p95 | run B p50 | run B p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `/api/v1/recordings` | 0 | 500 | 4.51 | 5.98 | 4.27 | 5.86 |
| `/api/v1/recordings` | 1 000 | 500 | 4.08 | 5.51 | 4.73 | 6.44 |
| `/api/v1/recordings` | 5 000 | 500 | 4.27 | 5.73 | 4.85 | 6.34 |
| `/api/v1/recordings` | 10 000 | **0** | 0.68 | 1.11 | 0.73 | 1.15 |
| `/api/v1/history` | 0 | 500 | 2.04 | 3.25 | 2.14 | 3.30 |
| `/api/v1/history` | 1 000 | 500 | 2.16 | 3.37 | 2.16 | 3.55 |
| `/api/v1/history` | 5 000 | 500 | 2.18 | 3.40 | 2.21 | 3.42 |
| `/api/v1/history` | 10 000 | **0** | 0.44 | 0.77 | 0.51 | 0.86 |

**The two offset-10 000 rows are the only numbers in this table that do not
measure a page.** With exactly 10 000 rows, `cursor=10000` is past the end, so
the server returns `200` with an empty array in 0.8–1.2 ms and the "fastest"
figures in the table measure nothing but the empty-page path. `F-27` is
explicitly a finding that refused to conclude without live timing at high
offsets; answering it with a number that never reads a row would repeat the
defect, not settle it, so the real high-offset page was measured separately on a
larger library.

### p95 by endpoint and offset, 11 000-recording library

Same binary, same knobs, one extra thousand rows so `cursor=10000` returns a
full 500-row page. `loadavg_1m` 7.13; a second 11 000-row run at
`loadavg_1m` 15.08 gave the same picture (recordings p95 6.57–6.88, history p95
3.64–4.22).

| Endpoint | Offset | rows | p50 | p95 | vs 500 ms budget |
| --- | ---: | ---: | ---: | ---: | --- |
| `/api/v1/recordings` | 0 | 500 | 4.03 | 5.44 | 92× under |
| `/api/v1/recordings` | 1 000 | 500 | 3.90 | 5.92 | 84× under |
| `/api/v1/recordings` | 5 000 | 500 | 4.23 | 5.71 | 88× under |
| `/api/v1/recordings` | 10 000 | 500 | 4.81 | 6.90 | 72× under |
| `/api/v1/history` | 0 | 500 | 2.17 | 3.30 | 152× under |
| `/api/v1/history` | 1 000 | 500 | 2.06 | 3.27 | 153× under |
| `/api/v1/history` | 5 000 | 500 | 2.09 | 3.02 | 166× under |
| `/api/v1/history` | 10 000 | 500 | 2.28 | 3.65 | 137× under |

### Decision: keep the numeric-offset shape, and do not add a file-existence catalog

**No code changed.** `cursor` keeps its name and its meaning, so there is no API
version note to add. Both alternatives the plan asked to weigh are rejected on
the numbers above:

- **Not a keyset cursor.** The number this rests on is
  `/api/v1/recordings` p95 **6.90 ms at offset 10 000 against 5.44 ms at
  offset 0** — a 27% rise for a 10 000-row deeper seek, on a query whose whole
  budget is 500 ms. A keyset cursor buys a logarithmic seek instead of a linear
  one, and the linear part is not what costs anything here: `EXPLAIN QUERY PLAN`
  on the seeded database reports `SEARCH jobs USING INDEX idx_jobs_kind_updated
  (kind=?)` with no temp B-tree for the `ORDER BY`, so the offset is an
  index-entry skip and the per-page cost is the fixed 500-row work, not the
  skip. Measured straight through `sqlite3` on the same 11 000-row file, one page
  costs 0.45 ms at offset 0, 0.46 ms at 1 000, 0.50 ms at 5 000 and 0.49 ms at
  10 000 — flat. A keyset cursor would add a second cursor format, a
  migration for every client, and a class of "skipped or duplicated row" bug in
  exchange for a saving this budget cannot see.
- **Not a maintained file-existence catalog.** The condition
  [below](#next-profiling-thresholds) sets for that work is a 500-row page
  exceeding the uncached-read budget. The measured worst case is 6.90 ms, 72×
  under it. The per-row `Path::exists()` is real and already `spawn_blocking`-off
  the reactor, and the whole gap between the two endpoints — 4.24 ms p50 for
  `/api/v1/recordings` against 2.15 ms p50 for `/api/v1/history`, which does not
  stat — is about 2.1 ms for 500 rows, roughly 4 µs per row. Reaching 500 ms
  that way needs a ~240× worse per-row cost, not a bigger library: the page is
  already capped at 500 rows server-side, so library size does not make the stat
  loop longer.

This settles [`F-27`](audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md) as
*measured, and no change warranted*: the finding asked for live timing at high
offsets before anyone invests in a keyset cursor, and the timing says the offset
is not what costs anything.

### `F-28`, the monitor's finished-recording count

`F-28`, in the same audit, covers a different query —
`count_finished_recordings` (`src/recording/persist.rs:644`), not the paginated
page — and it refused to conclude without `EXPLAIN QUERY PLAN` plus timing, so
it was measured the same way, straight against the seeded 11 000-row database
(8 800 rows in `finished` state, 500 distinct channels, 22 finished rows per
channel).

```
EXPLAIN QUERY PLAN SELECT COUNT(*) FROM jobs
  WHERE kind='Recording' AND state='finished'
    AND json_extract(payload,'$.channel_id') = ?1
  -> SEARCH jobs USING INDEX idx_jobs_state (state=?)
```

One call: **7.1–8.5 ms**, and the number is flat across channels — a channel
with 22 finished rows and a channel with **zero** finished rows both cost about
the same, because `idx_jobs_state` prunes to every finished row in the library
and `json_extract` then runs on all 8 800 of them. So the audit's estimate is
right about the mechanism and understated the shape: the cost is proportional
to the library's finished-row count **per channel queried**, not to that
channel's own rows. A sweep of all 400 channels holding finished rows took
2.9 s.

**Not changed here, and deliberately.** The budget table above covers HTTP
application time, not a monitor tick, and 8.5 ms of indexed SQLite work per
channel does not breach any budget this document states; the per-call cost is
also flat in channel count, so the thing to watch is
`finished rows × channels with a cutoff profile`. Revisit with an expression
index on `json_extract(payload,'$.channel_id')` when a single
`count_finished_recordings` call passes ~50 ms. At the measured 8.5 ms per
8 800 finished rows that is an extrapolated ~52 000 finished rows, **not a
measured figure** — no library that size was benchmarked.

### What the endpoints do with an unusable cursor

Probed directly, because a benchmark that records a rejection as a fast
response is measuring the wrong thing:

| Request | Result |
| --- | --- |
| `cursor=20000` on 11 000 rows | `200`, 0 rows, `next_cursor: null`, 0.5–0.9 ms |
| `cursor=11000` (exactly the total) | `200`, 0 rows, `next_cursor: null` |
| `cursor=-1` | `400` `cursor: invalid digit found in string` |
| `cursor=abc` | `400` `cursor: invalid digit found in string` |
| `cursor=` (empty) | `400` `cursor: cannot parse integer from empty string` |
| `limit=0` | `200`, clamped to 1 row, `next_cursor: 1` |
| `limit=99999` | `200`, clamped to 500 rows |

A client that walks past the end gets a `200` and an empty page rather than a
cursor error, and `limit` is clamped in both directions, so no request can ask
for an unbounded page. Both are properties worth knowing before a second cursor
format is added; neither is a performance defect.

### Caveat a reader must not skip

Two consecutive 10 000-row runs of this harness agreed to **16.9%** on
recordings p95 and **11.7%** on history p95 — *above* the 10% criterion in
[docs/perf](perf/README.md), on a host that was not idle. Treat single-run
deltas of that size as noise, and re-measure before calling one a regression.

### Reproduce

```sh
# The mandated shape: 10 000 recordings, offsets 0/1000/5000/10000.
BENCH_PORT=8412 BENCH_OFFSETS=0,1000,5000,10000 \
  scripts/bench_library.sh <release-strivo> 10000 /tmp/run-a.json
# The 11 000-row run, so offset 10 000 is a real 500-row page.
BENCH_PORT=8412 BENCH_OFFSETS=0,1000,5000,10000 \
  scripts/bench_library.sh <release-strivo> 11000 /tmp/run-b.json
```

Pick a port no other lane is using. `scripts/bench_library.sh` takes the
offsets as a knob already; no second harness is needed for this question, and
`limit` defaults to the API maximum of 500 inside the probe.

### Failure scenario: the benchmark does see a real regression

To confirm the harness would notice a query-path regression rather than
reporting a flat, meaningless number, `recordings` and `history` were
temporarily reverted to the pre-`1b1ac4d` shape — a fresh
`PersistDb::open(&state.jobs_db_path)` on every request, re-running the
PRAGMA + `CREATE TABLE IF NOT EXISTS` batch per call — the binary was rebuilt,
benchmarked, and the sources restored byte-identically (`md5sum` re-checked).

Two independent baseline/regressed pairs on the 11 000-row library:

| Pair | loadavg_1m (base → reg.) | recordings p95 | history p95 | recordings p50 | history p50 |
| --- | --- | --- | --- | --- | --- |
| B → C | 7.13 → 13.12 | 5.99 → 7.01 ms (+17.0%) | 3.31 → 4.37 ms (+32.1%) | 4.24 → 5.39 ms | 2.15 → 2.85 ms |
| D → E | 15.08 → 13.92 | 6.78 → 7.28 ms (+7.5%) | 3.91 → 4.48 ms (+14.7%) | 5.06 → 5.48 ms | 2.54 → 2.95 ms |

p95 figures are the mean over the four offsets. The honest reading: the
**direction** is unambiguous — all 16 per-workload p50 deltas are positive
across both pairs, and in the D/E pair the regressed binary was measured on the
*less* loaded host (13.92 against 15.08) and was still slower on both endpoints,
which is the signature of a per-request cost rather than of scheduler
contention. The mean per-request p50 penalty is **+0.42 ms** on recordings and
**+0.42 ms** on history in D/E, **+1.14 ms** and **+0.70 ms** in B/C — the
handle open plus its PRAGMA and schema batch. The *magnitude* is not separable
from this host's run-to-run spread on any single run, which is why two pairs
were taken and the sign consistency, not one percentage, is the evidence.

## Cold-media bounds (verified)

The control list above claims six things about ffmpeg/ffprobe work. All six are
real, all six are enforced at runtime rather than merely declared, and each now
names both a code site and a test. Measured 2026-09-25 against the `strivo 0.8.0`
release PVR binary (sha256 `38cacd0f…`) on a 10 000-recording library,
`BENCH_PORT=8413`, `loadavg_1m` 12.0 on 24 CPUs, sampling the web process's real
children out of `/proc` every 20 ms while the media path ran.

**Two of the plan's own pointers were wrong, and re-locating them matters.**
`crates/strivo-web/src/telemetry.rs:426-427` is a `#[cfg(test)]` fixture that
builds an `AppState` for the telemetry tests — it is not the declaration of the
probe cache or the pool. The production declaration is
`crates/strivo-web/src/server.rs:61-71` (fields) and `:175-179` / `:240-244`
(construction). Likewise `recording_thumb` is **not** in `routes/api.rs`; it is
`crates/strivo-web/src/routes/settings.rs:1160`, and so are `recording_probe`
(`:283`) and `remux_recording` (`:1433`).

### Claim-by-claim

| # | Claim, as written above | Code site | Test | Verdict |
| --- | --- | --- | --- | --- |
| 1 | "Probe, remux, and thumbnail processes share a two-slot worker pool" | `server.rs:64,176,241` (`probe_slots`); acquired at `settings.rs:325` (probe), `:1215` (thumb), `:1459` (remux) | `cold_media_routes_bound_the_pool_and_coalesce_by_id` | verified — peak 2 across mixed probe+thumb traffic |
| 2 | "Thumbnails additionally have a one-slot limit" | `server.rs:68,177,242`; acquired at `settings.rs:1209` | same test (asserts the per-name ffmpeg peak) | verified — peak 1 ffmpeg with 2 thumbs + 2 probes in flight |
| 3 | "coalesce by recording ID" | `settings.rs:131-143` (per-id lock map), `:1199-1206` (lock, then re-read the on-disk cache) | `cold_media_routes_bound_the_pool_and_coalesce_by_id` (phase 2) and `thumbnail_lock_keeps_one_lock_while_a_waiter_is_queued` (`settings.rs:2200`) | verified — two concurrent requests for one cold id start exactly **one** extraction |
| 4 | "cache failures briefly" | `settings.rs:155-162` (30 s TTL), `:1224` (cleared on success), `:1229-1233` (recorded, bounded at 1 024) | `queued_thumbnail_rechecks_a_recent_failure_after_its_id_lock` (`settings.rs:2218`) | verified |
| 5 | "Probe and individual thumbnail attempts have 30-second deadlines" | `settings.rs:355` (probe), `:1308` and `:1333` (both thumbnail attempts) | `every_media_child_process_has_a_deadline_and_kill_on_drop` (structural) + the `wedge` workload below (behaviour) | verified — a wedged ffmpeg was killed at 30.0 s |
| 6 | "remux deadlines allow 120 seconds plus file size at 4 MiB/s" | `settings.rs:1480-1481` | same structural test | verified as written; **not measured end-to-end** — no multi-hour archive was remuxed, so the size term is exercised only by the fact that it is the same `tokio::time::timeout` the other three spawns use |

Before this pass, only claims 3 (partially — the existing test covers the lock
map, not that a second request spawns no second process) and 4 had any test at
all. Claims 1, 2, 5 and 6 had none.

Two more sentences in the same paragraph are structural rather than bounded and
are guarded by the same test: "Cancelled media processes are killed"
(`kill_on_drop(true)` at `settings.rs:344,1292,1317,1466`) and "thumbnail
temporary files are cleaned up" (`ThumbnailTemp`'s `Drop`, `settings.rs:1253`).
The probe cache is fingerprinted by size and mtime as claimed
(`settings.rs:316-322`, rechecked after queueing at `:333-341`, written at
`:476`); it has **no** test, and none was added — a second request inside the
TTL is a cache hit only in the sense that it returns the stored JSON, and the
measurement below cannot separate a hit from a fast recompute.

**A recording shorter than 10 s costs two ffmpeg spawns, not one.** The
thumbnail route seeks to `-ss 10` first and only falls back to `-ss 0` when that
attempt produced no file (`settings.rs:1313-1336`), so every cold thumbnail of a
clip shorter than the seek point runs ffmpeg twice. This is correct behaviour,
not a bound violation — the two spawns are sequential and never exceed the
one-slot limit — but it is why the coalescing test's fixtures are 12 s long: on a
2 s fixture "two concurrent requests" looks like a coalescing failure when it is
really one request retrying.

### Measured peaks against the declared bound

| Workload | Requests (concurrency 8) | Peak media children | By name | Window | Result |
| --- | ---: | ---: | --- | --- | --- |
| 24 cold `/thumb` | 24 | **1** | `ffmpeg: 1` | 87–2 164 ms | 24× `200` |
| 24 cold `/probe` | 24 | **2** | `ffprobe: 2` | 114–262 ms | 24× `200` |
| mixed 24 `/thumb` + 24 `/probe` | 48 | **2** | `ffmpeg: 1`, `ffprobe: 1` | 15–2 143 ms | 48× `200` |
| 24 `/thumb`, every ffmpeg SIGKILLed | 24 | 1 | `ffmpeg: 1` | 168–1 656 ms | 42 kills; 22× `404`, 2× `200` |
| `/thumb` on a source that never delivers data | 1 | 1 | `ffmpeg: 1` | 18–**29 993 ms** | `404` at 30 004 ms |

Three of these deserve their own note.

- **The peak is a lower bound on attained concurrency, never a violation.** A
  second run of the same binary put the mixed workload's peak at 1 rather than 2:
  the sampler's 20 ms cadence lost a race it would have caught on a quieter host.
  The bound claim is the `<= 2` side, and that side held on every run.
- **Killing a child does not leak a permit.** After 42 SIGKILLs spread across 24
  concurrent thumbnail requests, a *fresh* cold thumbnail answered `200` in
  112 ms. A leaked `probe_slots`/`thumbnail_slots` permit would have starved the
  pool instead of answering.
- **The deadline is a kill, not a wait.** The wedged source is a FIFO that never
  delivers data, so ffmpeg blocks in `read()`. The request answered `404` at
  30 004 ms and no ffmpeg was running afterwards. One unreaped **zombie**
  (`State: Z`) was still visible 2 s later, which is a killed child awaiting
  reaping, not a running one; the harness and the test both exclude `Z` from the
  count for that reason. Whether tokio's orphan reaper collects it promptly was
  not measured beyond that 2 s window.

### Failure scenario: the harness does detect a wedged ffmpeg

The `timeout` around the first thumbnail attempt (`settings.rs:1308`) was
removed, the release binary rebuilt, and the same workload re-run. The request
never answered — the client gave up at 45 s with the ffmpeg still in its `read()`
(`status: 0`, last media child at **45 032 ms**) — and the harness inverted its
verdict to `wedge_detected`. The pristine binary was copied back over
`release/strivo` afterwards and re-hashed to the same
`63dc01737d7e539d1397ad677d9d82c9` / `38cacd0f…` it had before the build; the
mutated sources were restored with `git checkout` and `git diff HEAD` over
`crates/strivo-web/src/` is empty.

The same recipe makes the Rust tests load-bearing rather than decorative. With
`AppState::test_state`'s pool raised from 2 to 8 the runtime test fails with
"3 media children ran at once"; with one `tokio::time::timeout` deleted the
structural test fails with `left: 4, right: 3`; with the per-id lock at
`settings.rs:1200` removed the coalescing phase fails with "started 2 ffmpeg
extractions". All three mutations were reverted with `git checkout` and
`git diff HEAD` over `crates/strivo-web/src/` is empty.

### Durable archive: the media paths are only reachable if this holds

`docs/PERFORMANCE.md` claims list/detail/playback resolve the durable journal
and overlay live snapshot state, which is what makes a cold thumbnail or probe
possible for a recording the daemon no longer holds in memory. Both halves were
re-checked rather than taken from the tranche report:

- **Evicted from daemon memory** — `durable_archive_entry_lists_details_and_range_downloads_without_snapshot`
  (`crates/strivo-web/tests/routes.rs:1149`) drives the real router with IPC
  deliberately disconnected, and asserts the recording lists, opens its detail,
  and serves a 100-byte range. Green.
- **After a real restart** — nothing in the Rust suite restarts a daemon, so
  `scripts/check_durable_archive.sh` does: it seeds 261 finished rows (the
  oldest written first, against one real media file) into a real `jobs.db`, boots
  the real daemon + web pair in a throwaway XDG sandbox, then stops and starts
  both again. The oldest of 261 — well past `MAX_TERMINAL_RECORDINGS` (200,
  `src/daemon.rs:21`) — listed and served a 100-byte range **before and after**
  the restart. Note the honest limit: after a restart the daemon reloads journal
  rows into its own snapshot, so this half proves durability across the restart,
  not the snapshot-miss fallback; that fallback is the Rust test's job.

The real-server lane (`crates/strivo-web/e2e`, `npm run test:real`) is green
twice in a row — 7 passed, 26.3 s and 5.2 s — against this same binary. Its
`download serves a real finished recording with Range + cache validators` test
covers the Range half over real bytes. `TRANCHE.md:48` records this lane as "6
passed"; it is 7 now, because the CE01-era `#/studio` test was added after the
tranche. That is drift in a count, not a broken claim.

### Reproduce

```sh
scripts/bench_media_pool.sh <release-strivo>/release/strivo 10000 out.json
scripts/bench_media_pool.sh <release-strivo>/release/strivo 10000 out.json --expect-wedge
scripts/check_durable_archive.sh <release-strivo>/release/strivo 260
cargo test -p strivo-web --test media_pool --locked
```

`scripts/bench_media_pool.sh` reuses todo 9's `scripts/seed_bench_library.py`
unchanged and refuses a non-release build or a real `$HOME`, exactly as
`scripts/bench_library.sh` does; it is a separate script rather than an edit to
that harness because the two answer different questions and the captured
baseline must keep its own meaning. It runs on a throwaway filesystem under
`$TMPDIR` with a declared 2 048 MiB ceiling that it asserts — the whole 10 000-row
sandbox, media pool and all, measured **11–12 MiB**. `BENCH_CONCURRENCY` stays at
its default of 1 for the API harness; the concurrency here is a fixed 8
in-flight media requests, the shape a browser wall produces, and it is a knob
(`MEDIA_CONCURRENCY`) rather than a loop.

## Next profiling thresholds

Profile before adding complexity. Split the SPA into route chunks when compressed
JavaScript exceeds 250 KiB or parse/evaluation exceeds 150 ms on the minimum
supported client. The `/api/v1/recordings` history file-existence check
(`augment_recording`'s `Path::exists()`, up to 500 rows per page) already runs
off the async reactor via `spawn_blocking`, so it no longer stalls other
requests while it runs — but it is still one stat syscall per row. Move it
into a maintained catalog (tracked alongside the journal, invalidated on
write) when a 500-row page's wall time exceeds the uncached read budget even
with the blocking-pool dispatch. Add a dedicated transcode queue when
interactive media waits exceed two seconds under normal Creator loads.
