#!/usr/bin/env node
// Measurement half of scripts/bench_library.sh. Boots nothing: it measures an
// ALREADY-RUNNING release binary through its real HTTP surface and writes one
// machine-readable JSON document that a later todo can diff against.
//
// Three rules govern everything here:
//   1. A number appears only if it was measured. Anything not captured is
//      listed in `not_captured` with the reason. Never 0, never estimated.
//   2. Server-side application time comes from the `Server-Timing` header
//      `performance_timing` already emits (crates/strivo-web/src/server.rs:436)
//      — the same signal docs/PERFORMANCE.md's budget table is written
//      against. Client wall-clock is recorded beside it, never instead of it.
//   3. Concurrency is 1 by default. This is a measurement tool on a shared
//      workstation, not a load test; --concurrency is available but the
//      default is a crawl.
//
// Usage (see scripts/bench_library.sh, which owns the sandbox and calls this):
//   node scripts/bench_probe.mjs --base-url URL --api-key KEY \
//        --serve-pid N --daemon-pid N --offsets 0,1000,5000,10000 \
//        --requests 60 --out FILE [--library-manifest FILE]
import { readFileSync, writeFileSync, readdirSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { cpus, totalmem, loadavg, release, arch } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");

// ---------------------------------------------------------------- arguments
const argv = process.argv.slice(2);
const arg = (name, dflt) => {
  const i = argv.indexOf(`--${name}`);
  return i === -1 ? dflt : argv[i + 1];
};
const flag = (name) => argv.includes(`--${name}`);

const baseUrl = arg("base-url");
if (!baseUrl) {
  console.error("bench_probe.mjs: --base-url is required");
  process.exit(2);
}
const apiKey = arg("api-key", "");
const servePid = Number(arg("serve-pid", "0"));
const daemonPid = Number(arg("daemon-pid", "0"));
const requests = Number(arg("requests", "60"));
const concurrency = Number(arg("concurrency", "1"));
const limit = Number(arg("limit", "500"));
const offsets = String(arg("offsets", "0,1000,5000,10000")).split(",").map(Number);
const outPath = arg("out");
const manifestPath = arg("library-manifest");
const notCaptured = [];
if (!outPath) {
  console.error("bench_probe.mjs: --out is required (the JSON baseline)");
  process.exit(2);
}

// ----------------------------------------------------------------- helpers
const pct = (sorted, p) => {
  if (!sorted.length) return null;
  // Nearest-rank: the smallest value at or above the p-th percentile position.
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank - 1))];
};
const round = (v, d = 2) => (v === null || v === undefined ? null : Number(v.toFixed(d)));

function summarise(values) {
  if (!values.length) return null;
  const sorted = [...values].sort((a, b) => a - b);
  return {
    samples: sorted.length,
    min: round(sorted[0]),
    p50: round(pct(sorted, 50)),
    p95: round(pct(sorted, 95)),
    p99: round(pct(sorted, 99)),
    max: round(sorted[sorted.length - 1]),
    mean: round(sorted.reduce((a, b) => a + b, 0) / sorted.length),
  };
}

// `Server-Timing: app;dur=12.34` — the app duration the latency budget is
// written against. Returns null when the header or the metric is absent,
// which is reported as a missing sample rather than as a zero.
function serverTimingMs(res) {
  const raw = res.headers.get("server-timing");
  if (!raw) return null;
  const m = /app;dur=([0-9.]+)/.exec(raw);
  return m ? Number(m[1]) : null;
}

function readStatus(pid) {
  try {
    return readFileSync(`/proc/${pid}/status`, "utf8");
  } catch {
    return null;
  }
}
function rssBytes(pid) {
  const status = readStatus(pid);
  if (!status) return null;
  const m = /^VmRSS:\s+(\d+)\s+kB$/m.exec(status);
  return m ? Number(m[1]) * 1024 : null;
}
function peakRssBytes(pid) {
  const status = readStatus(pid);
  if (!status) return null;
  const m = /^VmHWM:\s+(\d+)\s+kB$/m.exec(status);
  return m ? Number(m[1]) * 1024 : null;
}

// Active child processes of the measured tree, straight out of /proc — the
// same thing docs/PERFORMANCE.md:47-52 bounds for probe/remux/thumbnail
// workers. Direct children only, which is what "active child-process count"
// means for the binary under test; the name breakdown is what makes a
// wedged ffmpeg visible rather than a bare number.
function childProcesses(pids) {
  const wanted = new Set(pids.filter((p) => p > 0));
  const byName = {};
  let total = 0;
  if (!wanted.size) return { total: null, by_name: null, note: "no pid supplied" };
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    const status = readStatus(entry);
    if (!status) continue;
    const ppid = /^PPid:\s+(\d+)$/m.exec(status);
    if (!ppid || !wanted.has(Number(ppid[1]))) continue;
    let comm = entry;
    try {
      comm = readFileSync(`/proc/${entry}/comm`, "utf8").trim();
    } catch { /* raced with exit; pid is enough */ }
    byName[comm] = (byName[comm] || 0) + 1;
    total += 1;
  }
  return { total, by_name: byName };
}

function snapshot(label) {
  const pids = [servePid, daemonPid];
  return {
    label,
    at: new Date().toISOString(),
    rss_bytes: Object.fromEntries(
      pids.filter((p) => p > 0).map((p) => [String(p), rssBytes(p)]),
    ),
    children: childProcesses(pids),
  };
}

async function timedFetch(path) {
  const started = performance.now();
  const res = await fetch(`${baseUrl}${path}`, {
    headers: apiKey ? { "X-Api-Key": apiKey } : {},
  });
  const body = await res.text();
  const wall = performance.now() - started;
  return { wall, status: res.status, app: serverTimingMs(res), bytes: body.length, body };
}

// ------------------------------------------------------------------ edition
// Refuse to publish a baseline for the wrong binary. `/api/v1/licence/status`
// is mounted by the `#[cfg(feature = "creator")]` module `routes/licence`
// (crates/strivo-web/src/routes/mod.rs:25), so on a PVR build it cannot
// resolve; a Creator build answers it. The recordings page is the positive
// control: it must return real seeded rows, or we are timing a stub.
async function assertPvrEdition() {
  const out = { licence_route_status: null, recordings_total: null, version: null,
                recordings_sample: null, ok: false };
  try {
    out.version = execFileSync(serveBinary(), ["--version"], { encoding: "utf8" })
      .trim().split("\n")[0];
  } catch (e) {
    out.version = `unavailable: ${e.message}`;
  }
  const lic = await fetch(`${baseUrl}/api/v1/licence/status`, { headers: { "X-Api-Key": apiKey } })
    .catch((e) => ({ status: () => 0, text: async () => String(e) }));
  out.licence_route_status = lic.status;
  const rec = await timedFetch(`/api/v1/recordings?cursor=0&limit=${Math.min(limit, 5)}`);
  out.recordings_status = rec.status;
  try {
    const parsed = JSON.parse(rec.body);
    out.recordings_total = parsed.total ?? null;
    out.recordings_sample = (parsed.recordings || []).slice(0, 2).map((r) => ({
      id: r.id, state: r.state, file_exists: r.file_exists,
      video_codec: r.video_codec, audio_codec: r.audio_codec,
      output_path_tail: String(r.output_path || "").split("/").slice(-2).join("/"),
    }));
  } catch {
    out.recordings_sample = null;
  }
  out.ok = out.licence_route_status === 404 && rec.status === 200
    && typeof out.recordings_total === "number" && out.recordings_total > 0;
  if (!out.ok) {
    notCaptured.push(
      `edition assertion failed: licence=${out.licence_route_status} ` +
      `recordings=${rec.status} total=${out.recordings_total} — refusing to publish a baseline`,
    );
  }
  return out;
}

function serveBinary() {
  // The binary that owns the port is the one whose version we print. The
  // caller passes --serve-pid; the path comes from the same env the harness
  // booted, so the two cannot drift.
  return process.env.STRIVO_BENCH_BIN || "strivo";
}

// -------------------------------------------------------------- first render
// Time to first real recordings row, in a real browser, against the served
// bundle — not a fetch of spa.js. Playwright resolves from the e2e lane's
// own node_modules (same import measure-tranche.mjs uses); if it or Chromium
// is unavailable the metric is reported as not-captured, never estimated.
async function measureFirstRender(url) {
  const out = { captured: false, reason: null, route: "/app#/recordings" };
  let chromium;
  try {
    const entry = join(root, "crates/strivo-web/e2e/node_modules/playwright/index.mjs");
    ({ chromium } = await import(entry));
  } catch (e) {
    out.reason = `playwright unavailable: ${e.message}`;
    notCaptured.push("first_render_ms (playwright not importable)");
    return out;
  }
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
    await page.goto(`${url}/app#/login`, { waitUntil: "domcontentloaded" });
    await page.locator("#api-key").fill(apiKey);
    const submit = page.locator("#login-form button[type=submit]");
    await submit.click();
    await page.locator("#channel-list").waitFor({ timeout: 30_000 });
    const started = Date.now();
    await page.goto(`${url}/app#/recordings`, { waitUntil: "commit" });
    await page.locator("#rec-body tr[data-rec-row]").first().waitFor({ timeout: 60_000 });
    out.captured = true;
    out.first_row_ms = Date.now() - started;
    out.rows_in_dom = await page.locator("#rec-body tr[data-rec-row]").count();
    out.nav = await page.evaluate(() => {
      const e = performance.getEntriesByType("navigation")[0];
      const fcp = performance.getEntriesByName("first-contentful-paint")[0];
      return {
        dom_content_loaded_ms: e ? Math.round(e.domContentLoadedEventEnd) : null,
        load_ms: e ? Math.round(e.loadEventEnd) : null,
        fcp_ms: fcp ? Math.round(fcp.startTime) : null,
      };
    }).catch(() => null);
  } catch (e) {
    out.reason = e.message.split("\n")[0];
    notCaptured.push(`first_render_ms (${out.reason})`);
  } finally {
    if (browser) await browser.close().catch(() => {});
  }
  return out;
}

// --------------------------------------------------------------------- main
const host = {
  kernel: release(),
  arch: arch(),
  cpus: cpus().length,
  total_mem_bytes: totalmem(),
  loadavg_1m: round(loadavg()[0], 2),
  loadavg_5m: round(loadavg()[1], 2),
};

const edition = await assertPvrEdition();
console.error(`bench_probe: version="${edition.version}" licence_route=${edition.licence_route_status} ` +
  `recordings_total=${edition.recordings_total} pvr_edition_ok=${edition.ok}`);
if (!edition.ok) {
  console.error(JSON.stringify({ edition, not_captured: notCaptured }, null, 2));
  writeFileSync(outPath, JSON.stringify({
    schema: "strivo-perf-baseline/1", captured_at: new Date().toISOString(),
    host, edition, api: null, process: null, first_render: null,
    not_captured: notCaptured,
  }, null, 2));
  process.exit(1);
}

const before = snapshot("before-api-workload");
const api = {};
const childSamples = [before];
for (const path of ["/api/v1/recordings", "/api/v1/history"]) {
  for (const offset of offsets) {
    const url = `${path}?cursor=${offset}&limit=${limit}`;
    const warm = await timedFetch(url); // connection setup + first query, untimed
    if (warm.status !== 200) {
      notCaptured.push(`${path}@${offset} (warm-up returned ${warm.status})`);
      continue;
    }
    const app = [];
    const wall = [];
    let missingHeader = 0;
    let badStatus = 0;
    let total = null;
    let pageRows = null;
    for (let i = 0; i < requests; i += 1) {
      const r = await timedFetch(url);
      if (r.status !== 200) { badStatus += 1; continue; }
      wall.push(r.wall);
      if (r.app === null) missingHeader += 1; else app.push(r.app);
      try {
        const parsed = JSON.parse(r.body);
        total = parsed.total ?? total;
        pageRows = (parsed.recordings ?? parsed.history ?? []).length;
      } catch { /* keep last */ }
      // Bounded crawl: sample the process tree at most every 10th request
      // rather than on every one, so measuring does not itself add load.
      if (i % 10 === 9) childSamples.push(snapshot(`during-${path}-${offset}-${i + 1}`));
    }
    const key = `${path}?cursor=${offset}&limit=${limit}`;
    api[key] = {
      endpoint: path,
      offset,
      limit,
      requests,
      library_total: total,
      rows_returned: pageRows,
      server_app_ms: summarise(app),
      client_wall_ms: summarise(wall),
      responses_without_server_timing: missingHeader,
      non_200_responses: badStatus,
    };
    const p95 = api[key].server_app_ms?.p95;
    console.error(`bench_probe: ${key} server-app p50=${api[key].server_app_ms?.p50}ms ` +
      `p95=${p95}ms p99=${api[key].server_app_ms?.p99}ms (n=${app.length}, ` +
      `missing-timing=${missingHeader}, non-200=${badStatus})`);
    if (p95 === null) notCaptured.push(`${key} server_app_ms (no Server-Timing samples)`);
  }
}

const after = snapshot("after-api-workload");
childSamples.push(after);
const firstRender = await measureFirstRender(baseUrl);

// Peak child-process count across every sample taken during the run, so
// `children_sampled` says how many observations the peak was chosen from.
const childMax = childSamples.map((s) => s.children).reduce(
  (acc, s) => (typeof s.total === "number" && s.total > (acc?.total ?? -1) ? s : acc), null);
const rss = (pid) => {
  const vals = [before, after]
    .map((s) => s.rss_bytes?.[String(pid)])
    .filter((v) => typeof v === "number");
  return vals.length ? { start: vals[0], end: vals[vals.length - 1], peak: peakRssBytes(pid) } : null;
};

if (servePid === 0 || daemonPid === 0) notCaptured.push("process rss (pid not supplied)");

const baseline = {
  schema: "strivo-perf-baseline/1",
  captured_at: new Date().toISOString(),
  host,
  harness: {
    script: "scripts/bench_library.sh + scripts/bench_probe.mjs",
    probe_invocation: process.argv.slice(1).join(" "),
    requests_per_workload: requests,
    concurrency,
    page_limit: limit,
    offsets,
    binary_sha256: arg("binary-sha", null),
    latency_source: "Server-Timing: app;dur (crates/strivo-web/src/server.rs performance_timing)",
  },
  edition,
  library: manifestPath && existsSync(manifestPath)
    ? JSON.parse(readFileSync(manifestPath, "utf8"))
    : (notCaptured.push("library manifest"), null),
  api,
  process: {
    serve_rss: servePid ? rss(servePid) : null,
    daemon_rss: daemonPid ? rss(daemonPid) : null,
    children_max: childMax,
    children_sampled: childSamples.length,
  },
  first_render: firstRender,
  snapshots: [before, after],
  not_captured: notCaptured,
};

writeFileSync(outPath, JSON.stringify(baseline, null, 2));
console.error(`bench_probe: wrote ${outPath}` +
  (notCaptured.length ? ` (not captured: ${notCaptured.join("; ")})` : ""));
