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
