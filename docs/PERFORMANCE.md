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
  two seconds. Mutations invalidate that cache.
- Recordings and durable history support cursor pagination. The UI incrementally
  loads history and caps each DOM render, preserving responsiveness for large
  libraries.
- Media probe results are fingerprinted by file size and modification time.
  Probe and remux processes share a bounded interactive worker pool.
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
node --check crates/strivo-web/assets/spa.js
```

For representative production measurements, run a release binary against a
copy of a large library and record API latency, resident memory, first-render
time, and active child-process count. Do not benchmark against a live production
database.

## Next profiling thresholds

Profile before adding complexity. Split the SPA into route chunks when compressed
JavaScript exceeds 250 KiB or parse/evaluation exceeds 150 ms on the minimum
supported client. Move history file-existence checks into a maintained catalog
when a 500-row page exceeds the uncached read budget. Add a dedicated transcode
queue when interactive media waits exceed two seconds under normal Creator
loads.
