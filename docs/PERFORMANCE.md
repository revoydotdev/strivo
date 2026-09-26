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
| PVR 0.8.0 | `spa.js` | 386 058 | 102 295 | 100 705 | none (min = median = max) |
| PVR 0.8.0 | `spa.css` | 92 405 | 17 242 | 18 836 | none (min = median = max) |
| Creator 0.8.0 | `spa.js` | 584 259 | not measured | not measured | — |
| Creator 0.8.0 | `spa.css` | 134 197 | not measured | not measured | — |

**PVR threshold result: under.** Compressed `spa.js` is 102 295 bytes as gzip
and 100 705 as brotli, both below the 256 000-byte (250 KiB) threshold. Whether
to split the bundle is a separate decision; it is not made here.

Two things a reader must not misread:

- `spa.css` is *smaller* over gzip (17 242) than over brotli (18 836). The
  server picks an encoding from the request's `q` values rather than from a size
  comparison, so at the layer's default quality brotli loses on a file this
  small. Quoting the brotli figure alone for the CSS overstates it.
- The Creator rows are **not wire-measured**. They are the release, minified
  Creator assembly on disk, and no Creator-featured 0.8.0 release binary existed
  on the measuring host — the single 0.8.0 release binary there embeds the PVR
  bundle, which the script establishes by content hash rather than by name. The
  compressed Creator figures are therefore recorded as `null` and the Creator
  threshold verdict is **unmeasured, not "under"**. Deriving a compressed Creator
  number from the file on disk is the F-29 mistake
  ([PHASE0-EXPERIENCE-AUDIT](audits/PHASE0-EXPERIENCE-AUDIT-2026-07-30.md)).

<!-- bundle-measurement:begin -->

```json
{
  "threshold_compressed_js_bytes": 256000,
  "tolerance_pct": 5,
  "editions": {
    "PVR": {
      "wire_measured": true,
      "note": "measured 2026-09-26 against the strivo 0.8.0 release PVR bundle over HTTP; 3 samples per point, every Content-Encoding asserted; figures are sample medians",
      "spa.js": { "raw_bytes": 386058, "gzip": 102295, "brotli": 100705 },
      "spa.css": { "raw_bytes": 92405, "gzip": 17242, "brotli": 18836 }
    },
    "Creator": {
      "wire_measured": false,
      "note": "no Creator-featured 0.8.0 release binary was available to measure through; raw sizes are the release minified Creator assembly on disk and are NOT wire figures",
      "spa.js": { "raw_bytes": 584259, "gzip": null, "brotli": null },
      "spa.css": { "raw_bytes": 134197, "gzip": null, "brotli": null }
    }
  }
}
```

<!-- bundle-measurement:end -->

Reproduce with a release binary and the `out/assets` directory of the same
build — the assets directory is what identifies the edition, by content:

```sh
scripts/measure_bundle.sh <release-strivo> <build>/out/assets PVR
scripts/check-bundle-size.sh <release-strivo> <build>/out/assets PVR
```

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
