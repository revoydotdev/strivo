# Performance baseline

`library-baseline.json` is a captured run of
[`scripts/bench_library.sh`](../../scripts/bench_library.sh) — the harness
`docs/PERFORMANCE.md` asks for when it says to run a release binary against a
copy of a large library and record API latency, resident memory, first-render
time, and active child-process count. It is the reference a later change
diffs against; regenerate it rather than editing it.

## Run it

```sh
cargo build --release -p strivo-bin
scripts/bench_library.sh "$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/release/strivo" \
  10000 /tmp/baseline.json
```

The script refuses to run against a real `$HOME`, a non-release build, or a
sandbox that contains a StriVo configuration — `docs/PERFORMANCE.md:83`
forbids benchmarking a live production database, and that is the one rule it
will not let you talk it out of. Everything runs under a throwaway
`XDG_*`/`HOME` in `$TMPDIR` and is deleted on exit.

Defaults are deliberately small: 60 requests per workload, concurrency 1, one
real media file pair per characteristic. This is a measurement tool for a
shared workstation, not a load test. `BENCH_REQUESTS`, `BENCH_CONCURRENCY`,
`BENCH_OFFSETS`, `BENCH_MEDIA_PER_CLASS` and `BENCH_PORT` are the knobs.

## Read it

| Field | Meaning |
| --- | --- |
| `edition` | The probe's proof it measured the PVR build: `strivo --version`, the Creator-only `/api/v1/licence/status` route answering 404, and a real seeded `/api/v1/recordings` page. It refuses to write a baseline if any of those fails. |
| `library` | The synthetic library, including **ffprobe's own reading** of every generated media file. "multi-audio" is two audio streams because ffprobe counted two, not because a class was named that. |
| `api.*.server_app_ms` | Percentiles of the `Server-Timing: app;dur` header — the same server-side application time the budget table in `docs/PERFORMANCE.md` is written against. `client_wall_ms` sits beside it. |
| `api.*.rows_returned` | Rows the page actually held. `cursor=10000` returns 0 because the library is exactly 10 000 rows; that workload measures the past-the-end path, not a full page. |
| `process.serve_rss` | Resident and peak RSS of the web process, read from `/proc`. |
| `process.children_max` | Peak direct child processes of the measured tree, by name, from `/proc`. |
| `first_render` | Time to the first real recordings row in headless Chromium against the served bundle. |
| `not_captured` | Anything that could not be measured, with the reason. Empty in this capture. A missing metric is never reported as `0`. |

## Caveats a reader of this baseline must know

- **The capturing host was not idle.** `host.loadavg_1m` was 10.67 on 24
  CPUs — a shared workstation with another lane's load on it. Two consecutive
  runs of this harness agreed to **9.6%** on p95 across all workloads (8.4%
  across the full 500-row pages); a second pair at 150 requests per workload
  reached 10.0% on one workload. The plan's criterion is under 10% *on an idle
  machine*, and this machine was not idle, so treat these numbers as
  "at the criterion under load" rather than a clean pass, and re-capture before
  treating a delta as a regression.
- **Latency here is server application time**, not network time and not
  browser time. It excludes connect and transfer. A slow page in front of
  this server is not visible in these numbers.
- **Child-process count is 0 because listing a library does no media work.**
  It bounds nothing about the probe/remux/thumbnail pools, which only engage
  on cold media. The claim in `docs/PERFORMANCE.md:47-52` is
  `docs/PERFORMANCE.md`'s to verify under load, not this file's.
- **The media pool is small on purpose.** Ten thousand rows share twelve real
  files (two per characteristic), because encoding ten thousand files would
  measure ffmpeg, not StriVo. `BENCH_MEDIA_PER_CLASS` raises the pool.
- **Roughly 5% of rows point at a path that does not exist**, restricted to
  non-finished journal states. The daemon's `prune_missing_recordings`
  (`src/recording/persist.rs:169`) deletes *finished* rows whose file is
  gone, so finished rows with missing files are deliberately not in the
  library: they would be silently deleted before the first request and every
  count in this file would describe a smaller library than the one seeded.
- The served bundle is whatever `build.rs` assembled for that build; bundle
  size is a separate measurement with its own owner.
