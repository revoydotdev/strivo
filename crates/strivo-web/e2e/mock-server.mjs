// Deterministic mock backend for the StriVo webui E2E suite (W7).
// Serves the real SPA assets and stubs /api/v1 + /events so the browser
// tests run without a live daemon or platform auth.
import { createServer } from "node:http";
import { readFile, readdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, isAbsolute, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ASSETS = join(__dirname, "..", "assets");
const PORT = process.env.PORT || 8199;

// `assets/spa.js` no longer exists as a single source file (CE03): it's
// assembled at build time from the ordered modules under `assets/spa/`
// (`<seq>-pvr.js` / `<seq>-creator.js`, concatenated in filename order — see
// build.rs). This mock backend, like the real server before CE03, always
// serves the FULL (Creator-included) source so the suite can exercise the
// SPA's own runtime `creator_enabled` route gating (S10's
// pvr-edition-gating.spec.ts) — it does not test the build-time module
// selection, which check-pvr-bundle.mjs covers against the real artifact.
// `STRIVO_E2E_ASSETS_DIR` overrides both loaders with a PREBUILT bundle
// (i.e. a build.rs output tree). Release builds minify spa.js/spa.css, and
// the source modules this file otherwise assembles are never minified — so
// without this override the 71-test mock lane would give a minified bundle
// zero coverage, and a minifier that broke the SPA would still go green.
const PREBUILT = process.env.STRIVO_E2E_ASSETS_DIR || "";
const PVR_ONLY = process.env.STRIVO_E2E_EDITION === "pvr";

async function readSpaJs() {
  if (PREBUILT) return readFile(join(PREBUILT, "spa.js"));
  const spaDir = join(ASSETS, "spa");
  const names = (await readdir(spaDir)).filter((n) => n.endsWith(PVR_ONLY ? "-pvr.js" : ".js")).sort();
  const parts = await Promise.all(names.map((n) => readFile(join(spaDir, n))));
  return Buffer.concat(parts);
}

// `assets/spa.css` no longer exists as a single source file either (mirrors
// the spa.js split): it's assembled from the ordered modules under
// `assets/spa-css/`. Like readSpaJs above, this always includes every
// module (pvr + creator) so the mock lane exercises the full UI.
async function readSpaCss() {
  if (PREBUILT) return readFile(join(PREBUILT, "spa.css"));
  const cssDir = join(ASSETS, "spa-css");
  const names = (await readdir(cssDir)).filter((n) => n.endsWith(PVR_ONLY ? "-pvr.css" : ".css")).sort();
  const parts = await Promise.all(names.map((n) => readFile(join(cssDir, n))));
  return Buffer.concat(parts);
}

const CHANNELS = [
  {
    id: "UClive0000000000000000aa",
    platform: "YouTube",
    name: "livechan",
    display_name: "Live Channel",
    is_live: true,
    stream_title: "Live test stream",
    game_or_category: "Just Chatting",
    viewer_count: 1234,
    started_at: new Date().toISOString(),
    thumbnail_url: null,
    auto_record: false,
  },
  {
    id: "twitch:offlinechan",
    platform: "Twitch",
    name: "offlinechan",
    display_name: "Offline Channel",
    is_live: false,
    stream_title: null,
    game_or_category: null,
    viewer_count: null,
    started_at: null,
    thumbnail_url: null,
    auto_record: true,
    // ~5 hours ago — exercises the "Xh ago" branch of relTime.
    last_live_at: new Date(Date.now() - 5 * 3600_000).toISOString(),
  },
  {
    id: "UCytoffline00000000aa",
    platform: "YouTube",
    name: "ytoffline",
    display_name: "YouTube Past Stream",
    is_live: false,
    stream_title: null,
    game_or_category: null,
    viewer_count: null,
    started_at: null,
    thumbnail_url: null,
    auto_record: false,
    // ~3 days ago — exercises the "Xd ago" branch.
    last_live_at: new Date(Date.now() - 3 * 86400_000).toISOString(),
  },
];

const RECORDINGS = {
  recordings: [
    {
      id: "11111111-1111-1111-1111-111111111111",
      channel_name: "Alpha",
      stream_title: "Zebra stream",
      state: "Finished",
      started_at: "2026-05-20T10:00:00Z",
      bytes_written: 5_000_000_000,
    },
    {
      id: "22222222-2222-2222-2222-222222222222",
      channel_name: "Bravo",
      stream_title: "Apple stream",
      state: "Recording",
      started_at: "2026-05-26T09:00:00Z",
      bytes_written: 1_000_000_000,
    },
    {
      id: "33333333-3333-3333-3333-333333333333",
      channel_name: "Charlie",
      stream_title: "Mango stream",
      state: "Failed",
      started_at: "2026-05-22T12:00:00Z",
      bytes_written: 200_000_000,
    },
  ],
};
// R01 e2e coverage — push enough filler rows past the default page size
// (500, mirroring the real PageQuery clamp in
// crates/strivo-web/src/routes/api.rs) that /recordings genuinely needs a
// second page. Dated well before the three named fixtures above so they
// always sort (started_at desc, same as the backend) onto page one.
for (let i = 0; i < 499; i++) {
  RECORDINGS.recordings.push({
    id: `filler-${String(i).padStart(4, "0")}`,
    channel_name: "FillerChan",
    stream_title: `Filler recording ${i}`,
    state: "Finished",
    started_at: `2019-01-01T00:00:${String(i % 60).padStart(2, "0")}Z`,
    bytes_written: 1000,
  });
}

// ── Research kernel fixtures (Coding Studio: codebook/corpus/notebook) ──
const RESEARCH_PROJECT_ID = "aaaaaaaa-0000-4000-8000-000000000001";
const RESEARCH_SOURCE_ID = "aaaaaaaa-0000-4000-8000-000000000010";
const RESEARCH_CODE_ID = "aaaaaaaa-0000-4000-8000-000000000020";
const RESEARCH_CODE_CHILD_ID = "aaaaaaaa-0000-4000-8000-000000000021";
const RESEARCH_CODING_ID = "aaaaaaaa-0000-4000-8000-000000000030";
const RESEARCH_CASE_ID = "aaaaaaaa-0000-4000-8000-000000000040";
const RESEARCH_MEMO_ID = "aaaaaaaa-0000-4000-8000-000000000050";
const RESEARCH_REL_ID = "aaaaaaaa-0000-4000-8000-000000000060";
const RESEARCH_SIGNAL_ID = "aaaaaaaa-0000-4000-8000-000000000070";

const RESEARCH_SOURCES = [
  {
    id: RESEARCH_SOURCE_ID,
    project_id: RESEARCH_PROJECT_ID,
    recording_id: null,
    kind: "recording",
    title: "Elden Ring run",
    uri: null,
    duration_ms: 3_600_000,
    attributes: {},
  },
];
const RESEARCH_CODES = [
  { id: RESEARCH_CODE_ID, project_id: RESEARCH_PROJECT_ID, parent_id: null, name: "Onboarding friction", description: "Moments where a new player struggles", color: "#00E5FF" },
  { id: RESEARCH_CODE_CHILD_ID, project_id: RESEARCH_PROJECT_ID, parent_id: RESEARCH_CODE_ID, name: "Signup drop-off", description: "", color: "#FFB020" },
];
const RESEARCH_CODINGS = [
  {
    id: RESEARCH_CODING_ID,
    project_id: RESEARCH_PROJECT_ID,
    source_id: RESEARCH_SOURCE_ID,
    code_id: RESEARCH_CODE_ID,
    start_ms: 1_000,
    end_ms: 5_000,
    excerpt: "the boss fight is confusing at first",
    note: "revisit tutorial pacing",
    author: "Ada",
    origin: "human",
    confidence: null,
  },
];
const RESEARCH_CASES = [
  { id: RESEARCH_CASE_ID, project_id: RESEARCH_PROJECT_ID, name: "Case One", description: "First playthrough cohort", attributes: {} },
];
const RESEARCH_SIGNALS = [
  {
    id: RESEARCH_SIGNAL_ID,
    project_id: RESEARCH_PROJECT_ID,
    source_id: RESEARCH_SOURCE_ID,
    start_ms: 2_000,
    end_ms: 4_000,
    kind: "transcript.utterance",
    label: "hey everyone welcome back",
    payload: {},
    confidence: 0.9,
    provenance_id: null,
  },
];
const RESEARCH_MEMOS = [
  { id: RESEARCH_MEMO_ID, project_id: RESEARCH_PROJECT_ID, source_id: RESEARCH_SOURCE_ID, coding_id: null, title: "Pacing memo", body: "Tutorial pacing needs a second look.", author: "Ada" },
];
const RESEARCH_RELATIONSHIPS = [
  { id: RESEARCH_REL_ID, project_id: RESEARCH_PROJECT_ID, from_kind: "coding", from_id: RESEARCH_CODING_ID, to_kind: "coding", to_id: RESEARCH_CODING_ID, relation: "supports", note: "", author: "Ada" },
];

// ── A/B render compare + sub-mix state ─────────────────────────────
// Mirrors the real strivo-web handlers' persisted-per-recording JSON
// exactly (crates/strivo-web/src/routes/plugins.rs `ab_render_*` /
// `submix_*`), including the composed-value math (strivo-ab-render's
// `audio_filter`/`diff`, strivo-submix's `to_filter_complex`) so a
// fixture drift here can't hide a real contract break.
const abRenderStore = new Map(); // recording_id -> { a, b }
const submixStore = new Map(); // recording_id -> SubMix

// Settings writes are stateful so the mock lane verifies the same persistence
// and restart-required contract as the daemon rather than only request shape.
const DEFAULT_SETTINGS = {
  twitch_configured: true,
  youtube_configured: true,
  patreon_configured: false,
  recording_dir: "/mnt/sda2/strivo",
  recording: {
    format: {
      container: "matroska",
      format: "bestvideo+bestaudio",
      bitrate_kbps: null,
      video_codec: null,
      audio_codec: null,
    },
  },
  auto_record_channels: [],
  poll_interval_secs: 60,
  schedule: [],
  creator_enabled: !PVR_ONLY,
  capture_profiles: [],
  monitor_limits: {
    max_concurrent_recordings: 3,
    disk_budget_reserved_gb: 20,
  },
};
let settingsState = structuredClone(DEFAULT_SETTINGS);

function problem(res, detail) {
  return json(res, 400, {
    type: "about:blank",
    title: "Bad Request",
    status: 400,
    detail,
    instance: null,
  });
}

function updateRestartRequiredSetting(path, value, res) {
  const nullable = new Set([
    "recording.format.format",
    "recording.format.bitrate_kbps",
    "recording.format.video_codec",
    "recording.format.audio_codec",
  ]);
  if (path === "recording_dir") {
    if (typeof value !== "string" || !value.trim()) return problem(res, "recording_dir must be a non-empty string");
    if (/[\u0000-\u001F\u007F]/.test(value)) return problem(res, "recording_dir must not contain control characters");
    // The real daemon probes its host filesystem. The mock exposes two known
    // writable fixtures and uses its own host's path rules, just as the daemon does.
    if (!isAbsolute(value.trim())) return problem(res, "recording_dir must be an absolute path");
    if (!["/mnt/sda2/strivo", "/tmp/strivo-e2e-recordings"].includes(value.trim())) {
      return problem(res, "recording_dir must be an existing writable directory");
    }
    settingsState.recording_dir = value.trim();
  } else if (!nullable.has(path)) {
    return problem(res, `unsupported setting path: ${path}`);
  } else if (value === null) {
    settingsState.recording.format[path.slice("recording.format.".length)] = null;
  } else if (path === "recording.format.bitrate_kbps") {
    if (!Number.isInteger(value) || value < 1 || value > 1_000_000) return problem(res, "bitrate_kbps must be an integer from 1 through 1000000");
    settingsState.recording.format.bitrate_kbps = value;
  } else if (path === "recording.format.format") {
    if (typeof value !== "string" || !value.trim() || value.length > 512 || /[\u0000-\u001F\u007F]/.test(value)) return problem(res, "format selector must be a non-empty string up to 512 characters without control characters");
    settingsState.recording.format.format = value.trim();
  } else {
    if (typeof value !== "string" || !value.trim() || value.length > 128 || !/^[A-Za-z0-9_-]+$/.test(value)) return problem(res, "codec override must be an ASCII alphanumeric, underscore, or hyphen token up to 128 characters");
    settingsState.recording.format[path.slice("recording.format.".length)] = value.trim();
  }
  return json(res, 202, { ok: true, path, restart_required: true });
}

function fmtF(v) {
  if (Math.abs(v - Math.round(v)) < 1e-9) return v.toFixed(1);
  return String(v.toFixed(6)).replace(/0+$/, "").replace(/\.$/, "");
}

function abAudioFilter(v) {
  if (!v) return null;
  const parts = [];
  if (v.pitch_time) {
    const { tempo = 1, pitch = 1, formant_preserve = true } = v.pitch_time;
    const identity = Math.abs(tempo - 1) < 1e-9 && Math.abs(pitch - 1) < 1e-9;
    if (!identity) {
      const t = Math.min(Math.max(tempo, 0.25), 4);
      const p = Math.min(Math.max(pitch, 0.25), 4);
      parts.push(`rubberband=tempo=${fmtF(t)}:pitch=${fmtF(p)}:formants=${formant_preserve ? "preserved" : "shifted"}`);
    }
  }
  if (v.loudness_target_lufs != null) {
    parts.push(`loudnorm=I=${v.loudness_target_lufs}:LRA=7:TP=-1`);
  }
  return parts.join(",");
}

function abDiff(a, b) {
  const out = [];
  if (a.label !== b.label) out.push({ field: "label", a: a.label, b: b.label });
  const an = a.insert_fx ? (a.insert_fx.effects || []).length : 0;
  const bn = b.insert_fx ? (b.insert_fx.effects || []).length : 0;
  if (an !== bn) out.push({ field: "insert_fx_stages", a: String(an), b: String(bn) });
  const at = a.pitch_time?.tempo ?? 1.0;
  const bt = b.pitch_time?.tempo ?? 1.0;
  if (Math.abs(at - bt) > 1e-6) out.push({ field: "tempo", a: `${at.toFixed(3)}×`, b: `${bt.toFixed(3)}×` });
  const alut = a.loudness_target_lufs ?? 0.0;
  const blut = b.loudness_target_lufs ?? 0.0;
  if (Math.abs(alut - blut) > 1e-6) out.push({ field: "loudness_lufs", a: alut.toFixed(1), b: blut.toFixed(1) });
  const ad = a.duck_db ?? 0.0;
  const bd = b.duck_db ?? 0.0;
  if (Math.abs(ad - bd) > 1e-6) out.push({ field: "duck_db", a: ad.toFixed(1), b: bd.toFixed(1) });
  return out;
}

function abRenderStateJson(recordingId, state) {
  const diff = state.a && state.b ? abDiff(state.a, state.b) : [];
  return {
    recording_id: recordingId,
    a: state.a ?? null,
    b: state.b ?? null,
    a_audio_filter: abAudioFilter(state.a),
    b_audio_filter: abAudioFilter(state.b),
    diff,
  };
}

function submixFilterComplex(mix) {
  if (!mix.tracks || mix.tracks.length === 0) return "";
  const parts = [];
  const busLabels = [];
  mix.tracks.forEach((tr, i) => {
    const chain = [];
    if (Math.abs(tr.gain_db || 0) > 1e-6) chain.push(`volume=${tr.gain_db.toFixed(2)}dB`);
    const bus = `bus${i}`;
    if (chain.length === 0) parts.push(`[${tr.input_index}:a]anull[${bus}]`);
    else parts.push(`[${tr.input_index}:a]${chain.join(",")}[${bus}]`);
    busLabels.push(bus);
  });
  const inputs = busLabels.map((b) => `[${b}]`).join("");
  parts.push(`${inputs}amix=inputs=${busLabels.length}:normalize=0[mix]`);
  const master = [];
  if (Math.abs(mix.master_gain_db || 0) > 1e-6) master.push(`volume=${mix.master_gain_db.toFixed(2)}dB`);
  if (master.length === 0) parts.push("[mix]anull[out]");
  else parts.push(`[mix]${master.join(",")}[out]`);
  return parts.join(";");
}

function readBody(req) {
  return new Promise((resolve) => {
    let raw = "";
    req.on("data", (chunk) => (raw += chunk));
    req.on("end", () => {
      try { resolve(raw ? JSON.parse(raw) : {}); } catch { resolve({}); }
    });
  });
}

const CONTENT_TYPES = {
  ".js": "text/javascript", ".css": "text/css", ".html": "text/html",
  ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg",
};

function json(res, code, body) {
  res.writeHead(code, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

// Open SSE connections, so POSTs that resolve asynchronously (vods,
// playlists) can push their answer like the real daemon does.
const sseClients = new Set();
function broadcast(eventObj) {
  const frame = `data: ${JSON.stringify(eventObj)}\n\n`;
  for (const c of sseClients) c.write(frame);
}

const SCHEDULE = [
  {
    channel: "Alpha",
    cron: "0 20 * * *",
    duration: "4h",
    next_fire: new Date(Date.now() + 3600_000).toISOString(),
  },
];

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  const path = url.pathname;

  if (path === "/events") {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(": connected\n\n");
    sseClients.add(res);
    const ka = setInterval(() => res.write(": keepalive\n\n"), 1000);
    req.on("close", () => {
      clearInterval(ka);
      sseClients.delete(res);
    });
    return;
  }

  // Test-only control endpoint: clears every module-level mutable store.
  // playwright.config.ts leaves the server running across `npm test`
  // invocations (reuseExistingServer outside CI), so without this a
  // recording's saved A/B render or sub-mix state from a previous run
  // would leak into the next one via these process-lifetime Maps.
  // global-setup.mjs calls this once before each test run.
  if (path === "/__test__/reset" && req.method === "POST") {
    abRenderStore.clear();
    submixStore.clear();
    settingsState = structuredClone(DEFAULT_SETTINGS);
    return json(res, 200, { status: "ok" });
  }

  // Test-only control endpoint: push an arbitrary event onto the REAL,
  // already-open /events SSE connection (the same `broadcast()` the
  // ChannelVods/PlaylistList/etc. handlers above use). Specs that need to
  // fire a specific lifecycle event (e.g. RecordingsPruned) at a precise
  // moment can't just replace the whole /events route with a canned
  // page.route body — that would also cut off every OTHER event (like the
  // ChannelVods answer a channel-detail visit depends on) the real
  // connection would otherwise deliver. This stays a thin, stateless
  // pass-through so it can't leak state across tests/specs the way
  // mutating RECORDINGS directly would.
  if (path === "/__test__/broadcast" && req.method === "POST") {
    const body = await readBody(req);
    broadcast(body);
    return json(res, 200, { status: "ok" });
  }

  // API surface.
  if (path.startsWith("/api/v1/")) {
    const p = path.slice("/api/v1".length);
    if (p === "/health") return json(res, 200, { status: "ok" });
    if (p === "/history")
      return json(res, 200, {
        history: [
          { id: "1", channel_name: "LilAggy", stream_title: "Elden Ring", platform: "Twitch", state: "Finished", started_at: "2026-05-26T20:00:00Z", bytes_written: 123456789 },
        ],
      });
    if (p === "/blocklist" && req.method === "GET")
      return json(res, 200, {
        blocklist: [
          { platform: "Twitch", channel_id: "ch1", vod_id: "", reason: null, created_at: "2026-05-26T00:00:00Z" },
        ],
      });
    if (p === "/blocklist") return json(res, 201, { status: "ok" });
    if (p === "/backup") return json(res, 201, { name: "2026-05-26T00-00-00Z", files: ["config.toml", "jobs.db"], bytes: 1234 });
    if (p === "/backups")
      return json(res, 200, {
        backups: [{ name: "2026-05-26T00-00-00Z", bytes: 1234, files: ["config.toml", "jobs.db"] }],
      });
    if (p.startsWith("/logs"))
      return json(res, 200, {
        file: "strivo.2026-05-26.log",
        level: "info",
        lines: [
          "2026-05-26T22:00:00Z  INFO strivo_core::daemon: StriVo daemon starting",
          "2026-05-26T22:00:01Z  WARN strivo_core::monitor: example warning",
        ],
      });
    if (p === "/health/checks") {
      // Tier-1 auth e2e coverage (S.E2): a test opts into a degraded
      // Platform Auth domain via an x-e2e-scenario header rather than a
      // new endpoint, so every existing health-checks test (and its
      // "ok" default) is untouched.
      if (req.headers["x-e2e-scenario"] === "auth-revoked") {
        return json(res, 200, {
          status: "error",
          checks: [
            { domain: "Network", name: "Daemon IPC", severity: "ok", message: "Daemon reachable.", fix: "" },
            { domain: "Storage", name: "Disk space", severity: "ok", message: "3 TB free.", fix: "" },
            {
              domain: "Platform Auth",
              name: "YouTube",
              severity: "error",
              message: "YouTube: credentials rejected — Token has been expired or revoked.",
              fix: "Re-authenticate from Settings → Platforms (the daemon will show a device-code prompt).",
            },
            {
              domain: "Platform Auth",
              name: "YouTube cookies",
              severity: "error",
              message: "YouTube cookie session rejected — cookies are no longer valid.",
              fix: "Re-import with: strivo setup cookies youtube --browser <browser>",
            },
          ],
        });
      }
      return json(res, 200, {
        status: "ok",
        checks: [
          { domain: "Network", name: "Daemon IPC", severity: "ok", message: "Daemon reachable.", fix: "" },
          { domain: "Storage", name: "Disk space", severity: "ok", message: "3 TB free.", fix: "" },
        ],
      });
    }
    if (p === "/auth/login") return json(res, 200, { status: "ok" });
    if (p === "/auth/logout") return json(res, 200, { status: "ok" });
    if (p === "/channels") return json(res, 200, { channels: CHANNELS });
    // Live tiles for the multi-view wall. The SPA ignores the server's
    // tile geometry and lays out from its own layout tree, so only the
    // `streams` array matters here. Two live streams, one per platform,
    // so tests can exercise both controller kinds.
    if (p.startsWith("/multistream/tiles"))
      return json(res, 200, {
        streams: [
          {
            stream_id: "Twitch:twitch-live-1",
            channel_name: "twitchlive",
            // Serialised exactly as the Rust side emits it: multistream's
            // Platform is snake_case, unlike PlatformKind on /channels. The
            // fixture used to say "YouTube" here, which is why a build that
            // routed every YouTube stream into the Twitch player still
            // passed its tests.
            platform: "twitch",
            viewer_count: 4321,
            embed_url: "https://player.twitch.tv/?channel=twitchlive&parent=localhost",
          },
          {
            stream_id: "YouTube:UClive0000000000000000aa",
            channel_name: "Live Channel",
            platform: "you_tube",
            viewer_count: 1234,
            video_id: "ytlive123456",
            embed_url: "https://www.youtube.com/embed/live_stream?channel=UClive0000000000000000aa",
          },
        ],
        tiles: [],
      });
    if (p === "/patreon")
      return json(res, 200, {
        creators: [
          {
            id: "camp123",
            platform: "Patreon",
            name: "creatorslug",
            display_name: "Cool Creator",
            is_live: false,
            stream_title: "Premium Tier",
            game_or_category: null,
            viewer_count: null,
            started_at: null,
            thumbnail_url: null,
            auto_record: false,
          },
        ],
        posts: [
          {
            id: "post1",
            campaign_id: "camp123",
            title: "Behind the scenes",
            url: "https://patreon.com/posts/post1",
            published_at: "2026-05-25T00:00:00Z",
            embed_url: "https://example.com/embed/post1",
          },
        ],
      });
    if (p === "/recordings" && req.method === "GET") {
      // R01 — mirror the real PageQuery contract: no `limit` param means
      // the legacy full snapshot; a `limit` opts into cursor pagination,
      // sorted newest-first same as the backend.
      const all = [...RECORDINGS.recordings].sort(
        (a, b) => new Date(b.started_at) - new Date(a.started_at),
      );
      const total = all.length;
      if (!url.searchParams.has("limit")) return json(res, 200, { recordings: all, total });
      const limit = Math.min(Math.max(Number(url.searchParams.get("limit")) || 500, 1), 500);
      const cursor = Math.min(Number(url.searchParams.get("cursor")) || 0, total);
      const end = Math.min(cursor + limit, total);
      return json(res, 200, {
        recordings: all.slice(cursor, end),
        total,
        next_cursor: end < total ? end : null,
      });
    }
    {
      const m = p.match(/^\/recordings\/([0-9a-fA-F-]{8,})$/);
      if (m && req.method === "GET") {
        const id = m[1];
        const found = RECORDINGS.recordings.find((r) => r.id === id);
        if (!found) {
          res.writeHead(404);
          res.end("not found");
          return;
        }
        // Backfill the optional fields the info modal + player expect.
        return json(res, 200, {
          channel_id: "twitch:alpha",
          platform: "Twitch",
          transcode: false,
          duration_secs: 3600,
          output_path: `/mnt/sda2/strivo/${found.channel_name}/${id}.mkv`,
          source_url: null,
          error: found.state === "Failed" ? "synthetic error for tests" : null,
          ...found,
        });
      }
    }
    if (p === "/storage")
      return json(res, 200, {
        bytes_used_by_recordings: 6_200_000_000,
        filesystem_avail_bytes: 900_000_000_000,
      });
    if (p === "/gantt") return json(res, 200, { items: [] });
    if (p === "/schedule") return json(res, 200, { schedule: SCHEDULE });
    if (p === "/monitor")
      return json(res, 200, {
        auto_record: [
          {
            key: "Twitch:twitch:offlinechan",
            channel_id: "twitch:offlinechan",
            channel_name: "Alpha",
            platform: "Twitch",
            format: "",
            profile: "",
          },
        ],
        auto_download: [],
      });
    if (p === "/settings" && req.method === "GET") return json(res, 200, settingsState);
    if (p === "/settings/update" && req.method === "POST") {
      const body = await readBody(req);
      if ([
        "recording_dir",
        "recording.format.format",
        "recording.format.bitrate_kbps",
        "recording.format.video_codec",
        "recording.format.audio_codec",
      ].includes(body.path)) return updateRestartRequiredSetting(body.path, body.value, res);
      // Preserve the mock's historical generic mutation fallback for the
      // unrelated Settings controls covered elsewhere in the suite.
      return json(res, 202, { status: "queued", path: p });
    }
    if (p === "/pipelines/runs" && req.method === "GET")
      return json(res, 200, {
        runs: [{
          id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
          name: "Ultimate creator publish",
          trigger: "recording-finished",
          state: "Done",
          stages: [{
            id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            name: "Assemble Casebook",
            kind: { Custom: "casebook.compose" },
            state: "Done",
            artifacts: [{
              kind: "casebook_markdown",
              path: "/var/lib/strivo/plugins/artifacts/recording/casebook.md",
              mime: "text/markdown",
            }],
          }],
        }],
      });

    // ── Plugin data surface (read-only) ──────────────────────────────
    if (p === "/plugins")
      return json(res, 200, {
        plugins: [
          { name: "crunchr", display: "Crunchr", description: "AI transcription, diarization & analysis", available: true, stats: { recordings: 2, analyzed: 1 }, verbs: [{ verb: "Re-transcribe", scope: "recording", label: "Re-transcribe" }] },
          { name: "insights", display: "Insights", description: "Word frequency, speaker airtime, topics & sentiment", available: true, stats: { words: 42, topics_videos: 1 }, verbs: [] },
          { name: "archiver", display: "Archiver", description: "Back-catalog VOD pulls & download tracking", available: true, stats: { channels: 1, videos: 2, downloaded: 1 }, verbs: [] },
          { name: "viewguard", display: "Viewguard", description: "Viewbot fraud detection — verdicts & viewer signals", available: true, stats: { verdicts: 1, samples: 4 }, verbs: [] },
        ],
      });
    if (p === "/plugins/crunchr/recordings")
      return json(res, 200, {
        available: true,
        recordings: [
          { recording_id: "rec-1", channel_name: "Alpha", title: "Elden Ring run", status: "complete", segment_count: 120, has_analysis: true, created_at: "2026-05-26 20:00:00" },
          { recording_id: "rec-2", channel_name: "Bravo", title: "Just chatting", status: "transcribing", segment_count: 0, has_analysis: false, created_at: "2026-05-27 09:00:00" },
        ],
      });
    {
      const m = p.match(/^\/plugins\/crunchr\/recordings\/([^/]+)$/);
      if (m)
        return json(res, 200, {
          recording_id: decodeURIComponent(m[1]),
          channel_name: "Alpha",
          title: "Elden Ring run",
          status: "complete",
          summary: "A long boss-fight session with commentary.",
          topics: ["elden ring", "bosses"],
          sentiment: "positive",
          segments: [
            { index: 0, start_sec: 0.0, end_sec: 3.5, text: "hey everyone welcome back", speaker: "Alpha", confidence: null },
            { index: 1, start_sec: 3.5, end_sec: 7.0, text: "today we fight the boss", speaker: "Alpha", confidence: null },
          ],
        });
    }
    if (p === "/plugins/crunchr/search") {
      const q = (url.searchParams.get("q") || "").trim();
      if (!q) return json(res, 200, { results: [] });
      return json(res, 200, {
        available: true,
        results: [
          { chunk_id: 1, video_title: "Elden Ring run", channel_name: "Alpha", snippet: `…we fight the ${q}…`, start_sec: 3.5, end_sec: 7.0, score: 0.9 },
        ],
      });
    }
    if (p === "/plugins/archiver/channels")
      return json(res, 200, {
        available: true,
        channels: [
          { id: 1, name: "Alpha", url: "https://twitch.tv/alpha", platform: "Twitch", archive_dir: "/arc/alpha", last_scan: "2026-05-26 12:00:00", video_count: 2, downloaded_count: 1 },
        ],
      });
    {
      const m = p.match(/^\/plugins\/archiver\/channels\/([^/]+)\/videos$/);
      if (m)
        return json(res, 200, {
          available: true,
          videos: [
            { video_id: "v2", title: "Stream Two", upload_date: "20260102", duration: 7200, playlist: null, downloaded: false },
            { video_id: "v1", title: "Stream One", upload_date: "20260101", duration: 3600, playlist: null, downloaded: true },
          ],
        });
    }
    if (p === "/plugins/viewguard/verdicts")
      return json(res, 200, {
        available: true,
        verdicts: [
          { channel_id: "twitch:suspect", stream_started_at: "2026-05-27T10:00:00Z", stream_ended_at: null, final_score: 0.87, band: "fraudulent", contributors: [{ kind: "SpikeShape", score: 0.9 }, { kind: "PlateauVariance", score: 0.7 }] },
          { channel_id: "twitch:clean", stream_started_at: "2026-05-27T08:00:00Z", stream_ended_at: null, final_score: 0.12, band: "clean", contributors: [] },
        ],
      });
    {
      const m = p.match(/^\/plugins\/viewguard\/channels\/([^/]+)\/samples$/);
      if (m)
        return json(res, 200, {
          available: true,
          samples: [
            { ts: "2026-05-27T10:00:00Z", viewers: 100 },
            { ts: "2026-05-27T10:01:00Z", viewers: 5000 },
          ],
        });
    }
    if (p === "/plugins/insights/words")
      return json(res, 200, {
        available: true,
        words: [
          { word: "stream", count: 40 },
          { word: "recording", count: 25 },
        ],
      });
    if (p === "/plugins/insights/topics")
      return json(res, 200, {
        available: true,
        topics: [
          { topic: "elden ring", count: 3, first_seen: "2026-05-20", last_seen: "2026-05-26" },
        ],
      });
    {
      const m = p.match(/^\/plugins\/insights\/recordings\/([^/]+)\/speakers$/);
      if (m)
        return json(res, 200, {
          available: true,
          speakers: [{ speaker: "Alpha", seconds: 1200, segments: 80 }],
          sentiment: "positive",
        });
    }

    // ── R02 — B-roll finder ────────────────────────────────────────────
    {
      const m = p.match(/^\/plugins\/broll\/([^/]+)$/);
      if (m && req.method === "POST") {
        const id = decodeURIComponent(m[1]);
        const body = await readBody(req);
        const assets = (body.library && body.library.assets) || [];
        const suggestions = assets.slice(0, body.top_k || 12).map((a, i) => ({
          time_sec: 10 + i * 5,
          asset_id: a.id,
          asset_path: a.path,
          duration_sec: a.duration_sec,
          score: 0.42,
          matched_tags: a.tags || [],
        }));
        return json(res, 200, {
          recording_id: id,
          suggestions,
          library_size: assets.length,
        });
      }
    }

    // ── A/B render compare ────────────────────────────────────────────
    {
      const m = p.match(/^\/plugins\/ab-render\/([^/]+)$/);
      if (m && req.method === "GET") {
        const id = decodeURIComponent(m[1]);
        const state = abRenderStore.get(id) || { a: null, b: null };
        return json(res, 200, abRenderStateJson(id, state));
      }
    }
    {
      const m = p.match(/^\/plugins\/ab-render\/([^/]+)\/(a|b)$/);
      if (m && req.method === "POST") {
        const id = decodeURIComponent(m[1]);
        const slot = m[2];
        const variant = await readBody(req);
        const state = abRenderStore.get(id) || { a: null, b: null };
        state[slot] = variant;
        abRenderStore.set(id, state);
        return json(res, 200, abRenderStateJson(id, state));
      }
    }
    {
      const m = p.match(/^\/plugins\/ab-render\/([^/]+)\/compare$/);
      if (m && req.method === "POST") {
        const id = decodeURIComponent(m[1]);
        const state = abRenderStore.get(id) || { a: null, b: null };
        if (!state.a || !state.b) {
          return json(res, 400, {
            type: "about:blank",
            title: "Bad Request",
            status: 400,
            detail: "save both slot A and slot B before comparing",
            instance: null,
          });
        }
        return json(res, 200, {
          recording_id: id,
          quality: { vmaf_mean: 95.4218, ssim_all: 0.998234 },
          a_output_path: `/var/lib/strivo/plugins/ab-render/${id}/a.mkv`,
          b_output_path: `/var/lib/strivo/plugins/ab-render/${id}/b.mkv`,
        });
      }
    }

    // ── Sub-mix bus routing ───────────────────────────────────────────
    {
      const m = p.match(/^\/plugins\/submix\/([^/]+)$/);
      if (m && req.method === "GET") {
        const id = decodeURIComponent(m[1]);
        const mix = submixStore.get(id) || { tracks: [], master_chain: null, master_gain_db: 0 };
        return json(res, 200, {
          recording_id: id,
          submix: mix,
          filter_complex: submixFilterComplex(mix),
          output_pad: "out",
        });
      }
      if (m && req.method === "POST") {
        const id = decodeURIComponent(m[1]);
        const mix = await readBody(req);
        submixStore.set(id, mix);
        return json(res, 200, {
          recording_id: id,
          ok: true,
          submix: mix,
          filter_complex: submixFilterComplex(mix),
          output_pad: "out",
        });
      }
    }

    // ── Research kernel (Coding Studio surfaces: codebook/corpus/notebook) ──
    if (p.startsWith("/research/")) {
      const rp = p.slice("/research".length);

      if (rp === "/projects" && req.method === "GET")
        return json(res, 200, { projects: [{ id: RESEARCH_PROJECT_ID, name: "Archive", description: "Default archive workspace" }] });
      if (rp === "/projects" && req.method === "POST")
        return json(res, 201, { project: { id: RESEARCH_PROJECT_ID, name: "Archive", description: "Default archive workspace" } });

      const detailMatch = rp.match(new RegExp(`^/projects/${RESEARCH_PROJECT_ID}$`));
      if (detailMatch && req.method === "GET")
        return json(res, 200, {
          project: {
            id: RESEARCH_PROJECT_ID,
            name: "Archive",
            description: "Default archive workspace",
            sources: RESEARCH_SOURCES,
          },
        });

      const codesMatch = rp.match(/^\/projects\/[^/]+\/codes$/);
      if (codesMatch && req.method === "GET") return json(res, 200, { codes: RESEARCH_CODES });
      if (codesMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { code: body });
      }

      const codingsMatch = rp.match(/^\/projects\/[^/]+\/codings$/);
      if (codingsMatch && req.method === "GET") {
        const codeId = url.searchParams.get("code_id");
        const codings = codeId ? RESEARCH_CODINGS.filter((c) => c.code_id === codeId) : RESEARCH_CODINGS;
        return json(res, 200, { codings });
      }
      if (codingsMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { coding: body });
      }

      const sourcesMatch = rp.match(/^\/projects\/[^/]+\/sources$/);
      if (sourcesMatch && req.method === "GET") return json(res, 200, { sources: RESEARCH_SOURCES });
      if (sourcesMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { source: body });
      }

      const casesMatch = rp.match(/^\/projects\/[^/]+\/cases$/);
      if (casesMatch && req.method === "GET") return json(res, 200, { cases: RESEARCH_CASES });
      if (casesMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { case: body });
      }

      const caseSourceMatch = rp.match(/^\/projects\/[^/]+\/cases\/[^/]+\/sources$/);
      if (caseSourceMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { status: "ok", source_id: body.source_id });
      }

      const signalsMatch = rp.match(/^\/projects\/[^/]+\/signals$/);
      if (signalsMatch && req.method === "GET") {
        const kind = url.searchParams.get("kind");
        const sourceId = url.searchParams.get("source_id");
        let signals = RESEARCH_SIGNALS;
        if (kind) signals = signals.filter((s) => s.kind === kind);
        if (sourceId) signals = signals.filter((s) => s.source_id === sourceId);
        return json(res, 200, { signals });
      }

      const memosMatch = rp.match(/^\/projects\/[^/]+\/memos$/);
      if (memosMatch && req.method === "GET") return json(res, 200, { memos: RESEARCH_MEMOS });
      if (memosMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { memo: body });
      }

      const relsMatch = rp.match(/^\/projects\/[^/]+\/relationships$/);
      if (relsMatch && req.method === "GET") return json(res, 200, { relationships: RESEARCH_RELATIONSHIPS });
      if (relsMatch && req.method === "POST") {
        const body = await readBody(req);
        return json(res, 201, { relationship: body });
      }

      const agreementMatch = rp.match(/^\/projects\/[^/]+\/agreement$/);
      if (agreementMatch && req.method === "GET")
        // Mirrors the real route exactly: envelope + Agreement field names.
        // A fixture that drifts from the server contract hides integration
        // bugs instead of catching them.
        return json(res, 200, {
          agreement: {
            cohens_kappa: 0.82,
            observed_agreement: 0.9,
            expected_agreement: 0.44,
            n: 20,
            author_a: "ana",
            author_b: "ben",
          },
        });

      const exportMatch = rp.match(/^\/projects\/[^/]+\/export$/);
      if (exportMatch && req.method === "GET") {
        const format = url.searchParams.get("format") || "json";
        if (format === "refi") {
          const xml = `<?xml version="1.0" encoding="UTF-8"?><Project name="Archive"/>`;
          res.writeHead(200, { "Content-Type": "application/xml" });
          res.end(xml);
          return;
        }
        return json(res, 200, {
          schema_version: 1,
          project: { id: RESEARCH_PROJECT_ID, name: "Archive", description: "" },
          sources: RESEARCH_SOURCES,
          codes: RESEARCH_CODES,
          signals: RESEARCH_SIGNALS,
          codings: RESEARCH_CODINGS,
        });
      }
    }

    // Channel VODs request → answer asynchronously over SSE, like the daemon.
    const vodsMatch = p.match(/^\/channels\/([^/]+)\/vods$/);
    if (vodsMatch && req.method === "POST") {
      const channelId = decodeURIComponent(vodsMatch[1]);
      setTimeout(() => {
        broadcast({
          ChannelVods: {
            channel_id: channelId,
            vods: [
              {
                id: "stream1",
                platform: "YouTube",
                channel_id: channelId,
                title: "Yesterday's livestream",
                published_at: "2026-05-25T20:00:00Z",
                url: "https://youtu.be/stream1",
                kind: "LiveBroadcast",
              },
              {
                id: "upload1",
                platform: "YouTube",
                channel_id: channelId,
                title: "How I edit my videos",
                published_at: "2026-05-24T12:00:00Z",
                url: "https://youtu.be/upload1",
                kind: "Upload",
              },
            ],
          },
        });
      }, 50);
      return json(res, 202, { status: "requested" });
    }

    // Mutations / verb dispatch — accept and echo queued.
    if (req.method === "POST" || req.method === "PUT" || req.method === "DELETE") {
      return json(res, 202, { status: "queued", path: p });
    }
    return json(res, 200, {});
  }

  // Static assets + SPA shell.
  let file;
  if (path === "/" || path === "/app") file = join(ASSETS, "spa.html");
  else if (path === "/assets/spa.js") file = "spa.js"; // assembled, not read
  else if (path === "/assets/spa.css") file = "spa.css"; // assembled, not read
  else if (path.startsWith("/assets/")) file = join(ASSETS, path.slice("/assets/".length));
  if (file) {
    try {
      const buf = file === "spa.js" ? await readSpaJs()
        : file === "spa.css" ? await readSpaCss()
        : await readFile(file);
      const ext = file === "spa.js" ? ".js" : file === "spa.css" ? ".css" : file.slice(file.lastIndexOf("."));
      res.writeHead(200, { "Content-Type": CONTENT_TYPES[ext] || "application/octet-stream" });
      res.end(buf);
      return;
    } catch {
      res.writeHead(404);
      res.end("not found");
      return;
    }
  }
  res.writeHead(404);
  res.end("not found");
});

server.listen(PORT, () => console.log(`mock server on http://localhost:${PORT}`));
