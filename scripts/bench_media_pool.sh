#!/usr/bin/env bash
# Cold-media bound harness (plan todo 13).
#
# docs/PERFORMANCE.md:47-52 declares six controls over ffmpeg/ffprobe work:
# a two-slot probe/remux/thumbnail pool, a one-slot thumbnail limit, per-ID
# coalescing, brief failure caching, 30 s probe/thumbnail deadlines and
# 120 s + size remux deadlines. `docs/perf/library-baseline.json` cannot
# answer any of them: its own README says its `children_max.total: 0` is
# "because listing a library does no media work" and bounds nothing about the
# pools. This harness drives the media path itself and samples the web
# process's real children out of /proc while it does.
#
# It is deliberately a SEPARATE script from scripts/bench_library.sh (todo 9)
# rather than an edit to it: that harness is committed and its baseline is
# captured, and this one answers a different question with a different
# workload. Everything it shares — the refusal to run against a real install,
# the throwaway XDG sandbox, and the seeded library — is reused, not
# reimplemented: it calls todo 9's scripts/seed_bench_library.py unchanged.
#
# Workloads, all through the real HTTP surface of a real release binary:
#   thumbs  N cold /thumb requests  -> the one-slot thumbnail limit
#   probes  N cold /probe requests  -> the two-slot pool
#   mixed   both at once           -> the shared pool, thumbnails included
#   killed  N thumbs, ffmpeg killed mid-flight -> no leaked permit
#   wedge   one /thumb whose source is a FIFO that never delivers data:
#           ffmpeg blocks, so only the 30 s deadline can end it
#
# Usage: scripts/bench_media_pool.sh <release-strivo-binary> [rows] [out.json]
# Env:   BENCH_PORT (default 8413)  MEDIA_THUMBS/ MEDIA_PROBES  MEDIA_CONCURRENCY
#        --expect-wedge  invert the wedge workload's verdict (for the
#                        deliberately-regressed binary)
# Exit:  0 every workload matched its declared bound | 1 a bound was broken
#        | 2 refused (unsafe target)
set -euo pipefail

BIN="${1:?usage: bench_media_pool.sh <release-strivo-binary> [rows] [out.json]}"
ROWS="${2:-10000}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PORT="${BENCH_PORT:-8413}"
API_KEY="media-pool-bench-key"
THUMBS="${MEDIA_THUMBS:-24}"
PROBES="${MEDIA_PROBES:-24}"
CONCURRENCY="${MEDIA_CONCURRENCY:-8}"
OUT="${3:-artifacts/bench-media-pool-$(date -u +%Y%m%dT%H%M%SZ).json}"
# Declared ceiling on everything this run may write: a 120 s lavfi encode per
# media file, the seeded jobs.db, and one 100-byte range. It is asserted below,
# not assumed, and the sandbox lives on a throwaway filesystem ($TMPDIR) so a
# runaway write can never reach the real disk.
CEILING_MB="${MEDIA_CEILING_MB:-2048}"

expect_wedge=0
for a in "$@"; do
  case "$a" in
    --expect-wedge) expect_wedge=1 ;;
  esac
done

[ -x "$BIN" ] || { echo "bench_media_pool.sh: not an executable binary: $BIN" >&2; exit 1; }
case "$(dirname "$BIN")" in
  *release) ;;
  *) echo "bench_media_pool.sh: refusing a non-release build ($BIN): these budgets are" \
       "release gates. Set STRIVO_BENCH_ALLOW_NONRELEASE=1 if that is genuinely it." >&2
     exit 2 ;;
esac

ROOT="${TMPDIR:-/tmp}"
case "$ROOT" in /*) ;; *) ROOT="/$ROOT" ;; esac
REAL_HOME="${HOME:-}"
mkdir -p "$ROOT/strivo-media-bench"
WORKDIR="$(mktemp -d "$ROOT/strivo-media-bench/r.XXXXXX")"
if [ "$WORKDIR" = "$REAL_HOME" ] || [ -e "$WORKDIR/.config/strivo/config.toml" ]; then
  echo "bench_media_pool.sh: REFUSING: $WORKDIR is a real installation" >&2
  exit 2
fi

export HOME="$WORKDIR"
export XDG_CONFIG_HOME="$WORKDIR/.c"
export XDG_DATA_HOME="$WORKDIR/.d"
export XDG_STATE_HOME="$WORKDIR/.s"
export XDG_CACHE_HOME="$WORKDIR/.ca"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"

DAEMON_PID=""
SERVE_PID=""
cleanup() {
  status=$?
  trap - EXIT INT TERM
  [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null || true
  [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  if [ -n "${KEEP_SANDBOX:-}" ]; then
    echo "bench_media_pool.sh: sandbox kept at $WORKDIR ($(du -sm "$WORKDIR" | cut -f1) MiB)" >&2
  else
    rm -rf "$WORKDIR"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

# The FIFO must exist before the daemon starts: prune_missing_recordings
# deletes finished rows whose file is gone, and a FIFO is not a file ffmpeg
# can ever finish reading, which is the entire point of the wedge workload.
FIFO_DIR="$WORKDIR/wedge"
mkdir -p "$FIFO_DIR" "$XDG_DATA_HOME/strivo"
WEDGE_FIFO="$FIFO_DIR/never-delivers.mkv"
mkfifo "$WEDGE_FIFO"

echo "bench_media_pool.sh: binary  $BIN ($("$BIN" --version 2>&1 | head -1))" >&2
echo "bench_media_pool.sh: sandbox $WORKDIR (throwaway; removed on exit)" >&2
echo "bench_media_pool.sh: ceiling ${CEILING_MB} MiB" >&2
DATA_DIR="$XDG_DATA_HOME/strivo"
MEDIA_DIR="$WORKDIR/media"
mkdir -p "$DATA_DIR" "$MEDIA_DIR"
python3 "$SCRIPT_DIR/seed_bench_library.py" "$DATA_DIR" "$MEDIA_DIR" "$ROWS" 2
du -sm "$WORKDIR" | awk -v c="$CEILING_MB" '{ if ($1 > c) { printf "bench_media_pool.sh: sandbox %d MiB exceeds the declared %d MiB ceiling\n", $1, c > "/dev/stderr"; exit 3 } }'

WEDGE_ID="$(python3 - "$DATA_DIR/jobs.db" "$WEDGE_FIFO" <<'PY'
import json, sqlite3, sys, uuid
from datetime import datetime, timezone
db, fifo = sys.argv[1], sys.argv[2]
now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
job_id = str(uuid.uuid4())
payload = json.dumps({
    "id": job_id, "channel_id": "", "channel_name": "wedge",
    "platform": "Twitch", "state": "Finished", "output_path": fifo,
    "started_at": now, "bytes_written": 4096, "duration_secs": 0.0,
    "transcode": False, "error": None, "stream_title": "wedge",
    "watched": False, "playlist": None, "thumbnail_url": None, "source_url": None,
})
conn = sqlite3.connect(db)
conn.execute("INSERT INTO jobs (id, kind, payload, state, created_at, updated_at,"
             " attempts, last_error, episode_dir)"
             " VALUES (?, 'Recording', ?, 'finished', ?, ?, 0, NULL, NULL)",
             (job_id, payload, now, now))
conn.commit()
conn.close()
print(job_id)
PY
)"

"$BIN" daemon &
DAEMON_PID=$!
for _ in $(seq 1 150); do
  "$BIN" status >/dev/null 2>&1 && break
  kill -0 "$DAEMON_PID" 2>/dev/null || { echo "daemon never came up" >&2; exit 1; }
  sleep 0.2
done
"$BIN" serve --bind "127.0.0.1:$PORT" --api-key "$API_KEY" &
SERVE_PID=$!
BASE="http://127.0.0.1:$PORT"
for _ in $(seq 1 150); do
  curl -s -o /dev/null "$BASE/api/v1/health" 2>/dev/null && break
  sleep 0.2
done
curl -fsS -o /dev/null -H "x-api-key: $API_KEY" "$BASE/api/v1/recordings?limit=1" \
  || { echo "bench_media_pool.sh: recordings endpoint not answering" >&2; exit 1; }
echo "bench_media_pool.sh: serving $BASE (serve pid $SERVE_PID, daemon pid $DAEMON_PID)" >&2

SPEC_FILE="$WORKDIR/media-spec.json"
python3 - "$BASE" "$API_KEY" "$SERVE_PID" "$THUMBS" "$PROBES" "$CONCURRENCY" "$ROWS" > "$SPEC_FILE" <<'PY'
import json, sys
base, key, pid, thumbs, probes, conc, rows = sys.argv[1:8]
print(json.dumps({"base": base, "key": key, "pids": [int(pid)], "rows": int(rows),
                  "thumbs": int(thumbs), "probes": int(probes),
                  "concurrency": int(conc), "interval_ms": 20}))
PY

rc=0
python3 - "$OUT" "$SPEC_FILE" "$WEDGE_ID" "$expect_wedge" "$WORKDIR" <<'PY' || rc=$?
"""Drive the media path and sample the web process's real children."""
import json, os, signal, sys, threading, time, urllib.error, urllib.request
from concurrent.futures import ThreadPoolExecutor

out_path, spec_path, wedge_id, expect_wedge, workdir = sys.argv[1:6]
expect_wedge = int(expect_wedge) == 1
spec = json.load(open(spec_path))
BASE, KEY, PIDS = spec["base"], spec["key"], spec["pids"]
CONC, INTERVAL = spec["concurrency"], spec["interval_ms"] / 1000.0
MEDIA = ("ffmpeg", "ffprobe")

def children():
    """{name: count} and {pid: (name, state)} for the web process's direct
    children.

    The state matters: a SIGKILLed child stays in /proc as a zombie until it
    is reaped, and a zombie is not a wedged process. Only a non-Z child counts
    as still running, which is what distinguishes an enforced deadline from an
    ffmpeg that outlived its request.
    """
    by_name, procs = {}, {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/status") as fh:
                status = fh.read()
            ppid = int(next(l for l in status.splitlines() if l.startswith("PPid:")).split()[1])
            if ppid not in PIDS:
                continue
            with open(f"/proc/{entry}/comm") as fh:
                comm = fh.read().strip()
        except (OSError, StopIteration, ValueError):
            continue
        by_name[comm] = by_name.get(comm, 0) + 1
        procs[int(entry)] = (comm, next(l.split()[1] for l in status.splitlines()
                                        if l.startswith("State:")))
    return by_name, procs

class Sampler(threading.Thread):
    """Samples the media children for the whole workload, and — if asked —
    kills each ffmpeg as soon as it appears (the leaked-permit probe)."""
    def __init__(self, kill=False):
        super().__init__(daemon=True)
        self.samples, self.killed, self.stop = [], 0, False
        self.kill = kill
        self.t0 = time.monotonic()

    def run(self):
        while not self.stop:
            by_name, procs = children()
            self.samples.append((time.monotonic() - self.t0, dict(by_name)))
            if self.kill:
                for pid, (comm, state) in procs.items():
                    if comm == "ffmpeg" and not state.startswith("Z"):
                        try:
                            os.kill(pid, signal.SIGKILL)
                            self.killed += 1
                        except OSError:
                            pass
            time.sleep(INTERVAL)

    def media(self):
        return [(t, n) for t, n in self.samples if any(n.get(m) for m in MEDIA)]

    def summary(self):
        _by_name, procs = children()
        runs = self.media()
        first = runs[0][0] if runs else None
        last = max((t for t, _ in runs), default=None)
        # A single ffmpeg seen in consecutive samples lived at least this long.
        peak_life, prev = 0.0, None
        for t, n in runs:
            if n.get("ffmpeg"):
                peak_life = max(peak_life, (t - prev) if prev is not None else 0.0)
                prev = t
        return {
            "samples": len(self.samples),
            "sample_interval_ms": int(INTERVAL * 1000),
            "media_children_seen": bool(runs),
            "first_media_child_at_ms": None if first is None else round(first * 1000),
            "last_media_child_at_ms": None if last is None else round(last * 1000),
            "peak_media_children": max((sum(n.get(m, 0) for m in MEDIA) for _, n in self.samples),
                                       default=0),
            "peak_by_name": {n: max(s.get(n, 0) for _, s in self.samples)
                             for n in set().union(*(s.keys() for _, s in self.samples))
                             if n in MEDIA} if self.samples else {},
            "longest_observed_ffmpeg_ms": round(peak_life * 1000),
            "media_children_after": {str(pid): state for pid, (comm, state) in procs.items()
                                     if comm in MEDIA},
            "killed": self.killed,
        }

def get(path, timeout=120):
    req = urllib.request.Request(BASE + path, headers={"x-api-Key": KEY})
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as res:
            body = res.read()
            return {"status": res.status, "bytes": len(body), "ms": round((time.monotonic() - started) * 1000)}
    except urllib.error.HTTPError as e:
        e.read()
        return {"status": e.code, "bytes": 0, "ms": round((time.monotonic() - started) * 1000)}
    except Exception as e:  # connection reset by a killed child is a real outcome
        return {"status": 0, "error": type(e).__name__, "ms": round((time.monotonic() - started) * 1000)}

def cold_ids(need):
    """Distinct finished recording ids whose media file exists, walked forward
    page by page — roughly 9% of a seeded page qualifies, so one page is not
    always enough to fill a workload with never-requested ids."""
    found, cursor = [], 0
    while len(found) < need and cursor <= 4 * 500:
        req = urllib.request.Request(f"{BASE}/api/v1/recordings?limit=500&cursor={cursor}",
                                     headers={"x-api-Key": KEY})
        with urllib.request.urlopen(req, timeout=60) as res:
            rows = json.load(res)["recordings"]
        if not rows:
            break
        found += [r["id"] for r in rows
                  if r.get("file_exists") and str(r.get("state", "")).lower() == "finished"]
        cursor += 500
    return found

def run(label, paths, sampler, timeout=120):
    sampler.t0 = time.monotonic()
    with ThreadPoolExecutor(max_workers=CONC) as pool:
        results = list(pool.map(lambda p: get(p, timeout), paths))
    sampler.stop = True
    sampler.join(timeout=5)
    statuses = {}
    for r in results:
        statuses[str(r["status"])] = statuses.get(str(r["status"]), 0) + 1
    return {"label": label, "requests": len(paths), "concurrency": CONC,
            "statuses": statuses,
            "max_response_ms": max((r["ms"] for r in results), default=None),
            **sampler.summary()}

need = 2 * spec["probes"] + 3 * spec["thumbs"] + 1
# The wedge row is journaled with the newest timestamp, so it heads page 1 and
# would otherwise be picked as a cold target: probing or thumbnailing a FIFO
# blocks for the full 30 s deadline and would masquerade as a pool problem.
ids = [i for i in cold_ids(need) if i != wedge_id]
if len(ids) < need:
    sys.exit(f"bench_media_pool: only {len(ids)} cold ids available, need {need}")
probes_a, rest = ids[: spec["probes"]], ids[spec["probes"]:]
thumbs_a, rest = rest[: spec["thumbs"]], rest[spec["thumbs"]:]
thumbs_m, rest = rest[: spec["thumbs"]], rest[spec["thumbs"]:]
probes_m, rest = rest[: spec["probes"]], rest[spec["probes"]:]
killed_ids, rest = rest[: spec["thumbs"]], rest[spec["thumbs"]:]
# One id no workload has touched: a request for an id whose ffmpeg was
# SIGKILLed is answered 404 from the failure cache, which would look like a
# starved pool when it is only the cache doing its job.
spare_id = rest[0]

workloads = []
s = Sampler(); s.start()
workloads.append(run("thumbs", [f"/api/v1/recordings/{i}/thumb" for i in thumbs_a], s))
s = Sampler(); s.start()
workloads.append(run("probes", [f"/api/v1/recordings/{i}/probe" for i in probes_a], s))
s = Sampler(); s.start()
workloads.append(run("mixed", [f"/api/v1/recordings/{i}/thumb" for i in thumbs_m]
                     + [f"/api/v1/recordings/{i}/probe" for i in probes_m], s))
s = Sampler(kill=True); s.start()
workloads.append(run("killed", [f"/api/v1/recordings/{i}/thumb" for i in killed_ids], s))
# The pool must still serve after every child in it was SIGKILLed: a leaked
# permit would make this last request hang instead of answering.
after_kill = get(f"/api/v1/recordings/{spare_id}/thumb", timeout=60)

# The wedge: a source that never delivers data, so only the deadline can end
# it. The request is given deadline + 15 s: on the guarded build it answers at
# 30 s, and on a build whose deadline was removed it is still blocked at 45 s
# with a live ffmpeg, which is the signal the failure demo looks for.
DEADLINE_MS = 30_000
s = Sampler(); s.start()
wedge = run("wedge", [f"/api/v1/recordings/{wedge_id}/thumb"], s,
            timeout=(DEADLINE_MS + 15_000) / 1000)
time.sleep(2)
_, procs_after = children()
survivors = {str(pid): state for pid, (comm, state) in procs_after.items()
             if comm in MEDIA and not state.startswith("Z")}
wedge_summary = s.summary()
deadline_ms = DEADLINE_MS
wedge["deadline_ms"] = deadline_ms
wedge["live_media_children_after_response"] = survivors
wedge["media_children_after_response"] = {str(pid): state for pid, (comm, state)
                                           in procs_after.items() if comm in MEDIA}
# Enforced means: an ffmpeg really was spawned, it did not outlive the
# deadline by more than the grace the sample interval needs to see it, and
# nothing was left RUNNING once the request had answered. A zombie is a
# killed child awaiting reaping, not a wedged one.
wedge["deadline_enforced"] = bool(
    wedge_summary["media_children_seen"]
    and not survivors
    and wedge_summary["last_media_child_at_ms"] is not None
    and wedge_summary["last_media_child_at_ms"] < deadline_ms + 5_000
)
wedge["wedge_detected"] = not wedge["deadline_enforced"]
workloads.append(wedge)

# Verdict. The declared bound is 2 media children at once across the shared
# pool, and 1 for thumbnails alone. `media_children_seen` is asserted first so
# a run where nothing ran can never be reported as a pass.
POOL = 2
THUMB_SLOTS = 1
problems = []
if not any(w["media_children_seen"] for w in workloads):
    problems.append("no ffmpeg/ffprobe child was ever observed: the bound was never exercised")
for w in workloads:
    if w["label"] in ("thumbs", "probes", "mixed", "killed") and w["peak_media_children"] > POOL:
        problems.append(f"{w['label']}: peak {w['peak_media_children']} media children exceeds the declared {POOL}")
    if w["label"] == "thumbs" and w["peak_by_name"].get("ffmpeg", 0) > THUMB_SLOTS:
        problems.append(f"thumbs: {w['peak_by_name']['ffmpeg']} concurrent ffmpeg exceeds the declared {THUMB_SLOTS}")
    if w["label"] == "killed" and w["killed"] == 0:
        problems.append("killed: no ffmpeg was ever seen, so no permit could be proven un-leaked")
if after_kill["status"] != 200:
    problems.append(f"a thumbnail request after every child was SIGKILLed returned {after_kill}")
if expect_wedge:
    if not wedge["wedge_detected"]:
        problems.append("--expect-wedge: the deadline still held, so this build is not the regressed one")
else:
    if not wedge["deadline_enforced"]:
        problems.append("wedge: ffmpeg outlived the 30 s deadline or survived the request")

sandbox_mb = 0
for root, _dirs, files in os.walk(workdir):
    for f in files:
        try:
            sandbox_mb += os.path.getsize(os.path.join(root, f))
        except OSError:
            pass
sandbox_mb //= 1024 * 1024
result = {
    "schema": "strivo-perf-media-pool/1",
    "captured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "host": {"loadavg_1m": round(os.getloadavg()[0], 2), "cpus": os.cpu_count()},
    "settings": {"rows": spec["rows"], "thumbs": spec["thumbs"], "probes": spec["probes"],
                 "concurrency": CONC, "declared_pool": POOL, "declared_thumbnail_slots": THUMB_SLOTS,
                 "declared_probe_thumbnail_deadline_ms": deadline_ms,
                 "expect_wedge": expect_wedge, "sandbox_mb": sandbox_mb},
    "workloads": workloads,
    "after_kill_probe": after_kill,
    "problems": problems,
    "verdict": "pass" if not problems else "fail",
}
with open(out_path, "w") as fh:
    json.dump(result, fh, indent=2)
for w in workloads:
    print(f"{w['label']:>6}: peak_media={w['peak_media_children']} peak_by_name={w['peak_by_name']} "
          f"statuses={w['statuses']} killed={w['killed']} "
          f"first_child_ms={w['first_media_child_at_ms']} last_child_ms={w['last_media_child_at_ms']}",
          file=sys.stderr)
print(f"wedge: deadline_enforced={wedge['deadline_enforced']} "
      f"live_after={wedge['live_media_children_after_response']} "
      f"all_after={wedge['media_children_after_response']} "
      f"max_response_ms={wedge['max_response_ms']}",
      file=sys.stderr)
print(f"after_kill: {after_kill}", file=sys.stderr)
print(f"sandbox: {sandbox_mb} MiB", file=sys.stderr)
print(f"bench_media_pool: {result['verdict']} -> {out_path}", file=sys.stderr)
for p in problems:
    print(f"bench_media_pool: PROBLEM {p}", file=sys.stderr)
sys.exit(0 if not problems else 1)
PY
rc=$?
echo "bench_media_pool.sh: sandbox used $(du -sm "$WORKDIR" | cut -f1) MiB (ceiling ${CEILING_MB} MiB)" >&2
echo "bench_media_pool.sh: result -> $OUT" >&2
exit "$rc"
