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
    auto_download_uploads: false,
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
  // The three vendored libraries include a true ES module. Served as
  // octet-stream a browser refuses to execute it, and reports that exactly
  // as it reports a 404 — so the mock has to carry the same rule
  // `routes::assets::mime_for` does, or the mock lane would test a bundle
  // the real server never serves.
  ".mjs": "text/javascript",
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

// The authoring surface's store (todo 53). Reset with the other module-level
// stores so a reused server cannot serve a template a previous run saved.
const templateStore = new Map();

// The registry names the editor matches a dispatch against. A FIXTURE, and
// deliberately the same list `/api/v1/plugins` answers with, because in
// production both come from one `PluginRegistry` and a mock that disagreed
// with itself would be a second answer to "can this run".
const REGISTRY_PLUGINS = ["crunchr", "insights", "archiver", "viewguard"];

const A_TRANSCRIBE = "5a5a5a5a-5a5a-4a5a-8a5a-5a5a5a5a5a5a";
const A_PUBLISH = "6b6b6b6b-6b6b-4b6b-8b6b-6b6b6b6b6b6b";

function authorStage(over) {
  return {
    id: A_TRANSCRIBE, name: "Transcribe", kind: { Custom: "crunchr.transcribe" },
    inputs: [], state: "Pending", attempts: 0, max_attempts: 3,
    fallback_provider: null, requires: [], cost_cents_estimate: 0,
    dispatch: { plugin: "crunchr", verb: "transcribe", selection: [], payload: null },
    artifacts: [], ...over,
  };
}

function authorPipeline() {
  return {
    id: "77777777-7777-4777-8777-777777777777",
    name: "Authoring fixture",
    stages: [
      authorStage({}),
      authorStage({
        id: A_PUBLISH, name: "Publish",
        kind: { Custom: "archiver.publish" },
        inputs: [A_TRANSCRIBE],
        dispatch: { plugin: "archiver", verb: "publish", selection: [], payload: null },
      }),
    ],
    state: "Pending", started_at_secs: null, completed_at_secs: null,
    subject_id: null, trigger: "manual", error: null,
  };
}

// The seven rules, so the mock answers a save the way the real route does.
// This is a re-implementation and it is deliberate: the browser lane needs a
// 400 with an `errors` array and a 409 with a `history/conflict` body to
// drive, and the lane that PROVES the rules is
// `crates/strivo-web/tests/pipeline_authoring.rs`, against the real
// `pipeline_templates::save`. This copy is allowed to be wrong; that one is
// not, and a change here cannot make a real save pass.
function authorErrors(pipeline) {
  const out = [];
  const stages = pipeline.stages || [];
  const ids = new Set(stages.map((s) => s.id));
  const seen = new Set();
  for (const s of stages) {
    if (seen.has(s.id)) out.push({ rule: "duplicate-stage-id", reason: "pipeline contains duplicate stage ids", detail: `Two stages share the id ${s.id}.` });
    seen.add(s.id);
  }
  if (!stages.length) out.push({ rule: "empty-stage-list", reason: "pipeline must contain at least one stage", detail: null });
  for (const s of stages) {
    for (const i of s.inputs || []) {
      if (!ids.has(i)) out.push({ rule: "dangling-input", reason: "pipeline references an unknown input stage", detail: `${s.name} reads ${i}, which is not a stage here.` });
    }
    if (!s.max_attempts) out.push({ rule: "zero-capacity", reason: "pipeline contains a zero retry or resource capacity", detail: `${s.name} allows 0 attempts.` });
    for (const lock of s.requires || []) {
      const tag = Object.keys(lock)[0];
      if (tag !== "Gpu" && lock[tag].cap === 0) out.push({ rule: "zero-capacity", reason: "pipeline contains a zero retry or resource capacity", detail: `${s.name} locks ${tag} at 0.` });
    }
    const d = s.dispatch;
    if (d && (!String(d.plugin || "").trim() || !String(d.verb || "").trim())) out.push({ rule: "empty-dispatch", reason: "pipeline contains an empty dispatch target", detail: `${s.name} has a dispatch with an empty plugin or verb.` });
    else if (d && !REGISTRY_PLUGINS.includes(String(d.plugin).trim())) out.push({ rule: "unknown-dispatch-target", reason: "pipeline dispatches to a plugin this build does not register", detail: `${s.name} dispatches to ${d.plugin}, which this build does not register.` });
  }
  // A cycle: walk, and report the closed loop in order.
  const byId = new Map(stages.map((s) => [s.id, s]));
  const visited = new Set();
  const walk = (id, stack) => {
    const at = stack.indexOf(id);
    if (at !== -1) return stack.slice(at).concat([id]);
    if (visited.has(id)) return null;
    stack.push(id);
    const s = byId.get(id);
    if (s) for (const i of s.inputs || []) { const f = walk(i, stack); if (f) return f; }
    stack.pop();
    visited.add(id);
    return null;
  };
  for (const s of stages) {
    const f = walk(s.id, []);
    if (f) {
      const names = f.map((id) => (byId.get(id) || {}).name || id);
      out.push({ rule: "acyclic", reason: "pipeline DAG contains a cycle", detail: `${names.join(" \u2192 ")} \u2192 ${names[0]}.` });
      break;
    }
  }
  return out;
}

// The pipeline graph the Pipelines page draws (todo 52).
//
// A FIXTURE, not a derivation: the mock deliberately does not re-implement
// the server's graph builder, because a second implementation in the mock
// would be a second answer to "what does this graph contain" — the exact
// defect `crates/strivo-web/tests/pipeline_graph.rs` exists to rule out on
// the real side. What the browser lane proves is that the surface draws the
// payload it is given, faithfully; what the Rust test proves is that the
// payload is the executor's `Stage` list.
//
// The counts below are the spec the lane asserts against, so they are
// deliberately unequal: 6 nodes of which 5 are runnable, 4 edges, and one
// node whose dispatch names a plugin no Creator build registers.
const GRAPH_RUN = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const STAGE_CASEBOOK = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const STAGE_THUMBS = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const T_TRANSCRIBE = "11111111-1111-4111-8111-111111111111";
const T_CUEPOINTS = "22222222-2222-4222-8222-222222222222";
const T_BRANDSAFE = "33333333-3333-4333-8333-333333333333";
const T_PUBLISH = "44444444-4444-4444-8444-444444444444";

function graphNode(o) {
  return {
    attempts: 0, max_attempts: 3, requires: [], cost_cents_estimate: 0,
    artifacts: 0, fallback_provider: null, capabilities: [], missing: [],
    runnable: true, dispatch: null, ...o,
  };
}

const PIPELINE_GRAPH = {
  totals: { graphs: 2, nodes: 6, edges: 5, runnable: 5, not_runnable: 1 },
  graphs: [
    {
      key: `run:${GRAPH_RUN}`,
      kind: "run",
      run_id: GRAPH_RUN,
      name: "Actual run",
      state: "Running",
      trigger: "recording-finished",
      error: null,
      runnable: true,
      nodes: [
        graphNode({
          stage_id: STAGE_THUMBS, name: "Generate thumbnail candidates",
          kind: "custom(thumbnails.generate)", state: "Done", attempts: 1, artifacts: 1,
          requires: ["file:artifact:rec-1:thumbnails", "cpu:2", "disk:2"],
          dispatch: { plugin: "artifacts", verb: "thumbnails" },
          capabilities: ["thumbnail_ranking"],
        }),
        graphNode({
          stage_id: STAGE_CASEBOOK, name: "Assemble Casebook",
          kind: "custom(casebook.compose)", state: "Done", artifacts: 1,
          requires: ["file:artifact:rec-1:casebook", "cpu:2", "disk:2"],
          dispatch: { plugin: "artifacts", verb: "casebook" },
          capabilities: ["reporting"],
        }),
      ],
      edges: [{ from: STAGE_THUMBS, to: STAGE_CASEBOOK }],
      totals: { nodes: 2, edges: 1, runnable: 2, not_runnable: 0 },
      mapping: { stages: 2, nodes: 2, edges: 1, inputs_links: 1, mismatches: [] },
    },
    {
      key: "template:creator_publish",
      kind: "template",
      run_id: null,
      name: "Ultimate creator publish",
      state: "Pending",
      trigger: "manual",
      error: null,
      runnable: false,
      nodes: [
        graphNode({
          stage_id: T_TRANSCRIBE, name: "Transcribe, diarize & analyze",
          kind: "custom(crunchr.process)", state: "Pending",
          requires: ["gpu", "cpu:2", "disk:2"],
          dispatch: { plugin: "crunchr", verb: "transcribe" },
          capabilities: ["diarisation", "topic_segmentation", "transcription", "word_timestamps"],
        }),
        graphNode({
          stage_id: T_CUEPOINTS, name: "Detect visual cuepoints",
          kind: "custom(cuepoints.extract)", state: "Pending",
          dispatch: { plugin: "artifacts", verb: "cuepoints" },
          capabilities: ["scene_detection"],
        }),
        graphNode({
          stage_id: T_BRANDSAFE, name: "Scan brand safety",
          kind: "analyze(local-rules)", state: "Pending",
          dispatch: { plugin: "artifacts", verb: "brandsafe" },
          capabilities: ["brand_safety"],
        }),
        graphNode({
          // The honest not-runnable case: `reuse` is not a registered
          // plugin, so the server marks this stage unrunnable and names it.
          stage_id: T_PUBLISH, name: "Publish to YouTube",
          kind: "custom(reuse.push)", state: "Pending",
          dispatch: { plugin: "reuse", verb: "push" },
          runnable: false, missing: ["dispatch plugin reuse"],
        }),
      ],
      edges: [
        { from: T_TRANSCRIBE, to: T_CUEPOINTS },
        { from: T_TRANSCRIBE, to: T_BRANDSAFE },
        { from: T_CUEPOINTS, to: T_PUBLISH },
        { from: T_BRANDSAFE, to: T_PUBLISH },
      ],
      totals: { nodes: 4, edges: 4, runnable: 3, not_runnable: 1 },
      mapping: { stages: 4, nodes: 4, edges: 4, inputs_links: 4, mismatches: [] },
    },
  ],
};

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
    templateStore.clear();
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

    // ── The timeline (todo 43) ──────────────────────────────────────────
    // Answers the ONE shape the route answers: a compiled plan plus the
    // request it was compiled from. This mock deliberately does NOT
    // re-implement the edit algebra — the arithmetic is the Rust
    // `strivo_editor::timeline` module's, and a second implementation here
    // would be a second answer to "what does this render as". What the mock
    // does is refuse a stale base_revision and echo the request, so the
    // browser lane can prove the surface sends well-formed intents and
    // draws the plan it is handed. crates/editor/tests/timeline_parity.rs
    // replays the very request bodies this lane produces through the real
    // compiler, which is where the arithmetic is proved.
    if (p === "/creator/timeline/apply" && req.method === "POST") {
      const body = await readBody(req);
      const request = body && body.request;
      if (!request || !Array.isArray(request.clips) || request.clips.length === 0) {
        return json(res, 400, { detail: "timeline/malformed: no clips" });
      }
      if (body.base_revision !== request.revision) {
        return json(res, 409, {
          type: "about:blank", title: "Conflict", status: 409,
          detail: `timeline/stale: the client is editing revision "${request.revision}" but posted "${body.base_revision}"`,
        });
      }
      const clips = request.clips
        .map((c) => ({
          cut: c.cut,
          output: { start: c.span.range.start, end: c.span.range.end },
          span: c.span,
          fade_in: c.fade_in,
          fade_out: c.fade_out,
        }))
        .reduce(
          (acc, c) => {
            const prev = acc[acc.length - 1];
            const start = prev ? prev.output.end : 0;
            acc.push({ ...c, output: { start, end: start + (c.output.end - c.output.start) } });
            return acc;
          },
          [],
        );
      const outputDuration = clips.length ? clips[clips.length - 1].output.end : 0;
      return json(res, 200, {
        plan: {
          subject: request.subject,
          revision: request.revision,
          output_duration: outputDuration,
          clips,
          layers: request.layers,
          transitions: request.transitions,
          tracks: request.tracks,
          audio_chain: [], video_chain: [], loudness: null,
          video_filter_complex: "", colour: null, cues: [],
          caption_mode: "none", caption_style_json: "{}",
          caption_coordinate: "composition", caption_safe_area: true,
          container: request.container, profile: request.profile,
        },
        request,
        clamps: [],
        snap_points_us: [0, ...clips.flatMap((c) => [c.output.start, c.output.end])].sort(
          (a, b) => a - b,
        ),
      });
    }

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
        backups: [
          // A backup carrying a project record, and one that does not — so the
          // "nothing about projects was verified" branch is reachable in this
          // lane rather than asserted only in the model.
          { name: "2026-05-26T00-00-00Z", bytes: 1234, files: ["config.toml", "jobs.db", "research.db", "manifest.json"], projects: 1, sources: 2 },
          { name: "2026-05-27T00-00-00Z", bytes: 1500, files: ["config.toml", "jobs.db", "research.db", "manifest.json"], projects: 1, sources: 1 },
          { name: "2026-05-20T00-00-00Z", bytes: 900, files: ["config.toml", "jobs.db"], projects: 0, sources: 0 },
        ],
      });

    // ── Restore and retention (todo 40) ───────────────────────────────────
    // The restore answers are chosen by the backup name, because the claim
    // under test is which of the three outcomes the UI is willing to call a
    // success. `complete` is reachable here on purpose: without a control that
    // DOES produce the green toast, "no green toast appeared" would pass for
    // any page where toasts are broken.
    if (p.endsWith("/restore") && req.method === "POST") {
      const name = p.split("/")[2];
      if (name === "2026-05-20T00-00-00Z") {
        return json(res, 200, {
          restored: ["config.toml", "jobs.db"],
          report: null,
          note: "This backup carries no project record, so nothing about projects was verified.",
        });
      }
      if (name === "2026-05-26T00-00-00Z") {
        return json(res, 200, {
          restored: ["config.toml", "jobs.db", "research.db"],
          report: {
            verified: true,
            // THE CASE: one of the two recorded sources did not come back.
            complete: false,
            projects: [
              {
                id: "11111111-1111-4111-8111-111111111111",
                name: "launch stream",
                expected: 2,
                restored: 1,
                present: true,
                sources: [
                  { id: "22222222-2222-4222-8222-222222222222", title: "a.mkv", fingerprint: "sha256:a", outcome: "restored", locator: "/archive/a.mkv" },
                  { id: "33333333-3333-4333-8333-333333333333", title: "b.mkv", fingerprint: "sha256:b", outcome: "missing_row", locator: null },
                ],
              },
            ],
          },
          note: "Restart the daemon for the restored config/DB to take effect.",
        });
      }
      return json(res, 200, {
        restored: ["config.toml", "jobs.db", "research.db"],
        report: {
          verified: true,
          complete: true,
          projects: [
            {
              id: "11111111-1111-4111-8111-111111111111",
              name: "launch stream",
              expected: 1,
              restored: 1,
              present: true,
              sources: [
                { id: "22222222-2222-4222-8222-222222222222", title: "a.mkv", fingerprint: "sha256:a", outcome: "restored", locator: "/archive/a.mkv" },
              ],
            },
          ],
        },
        note: "Restart the daemon for the restored config/DB to take effect.",
      });
    }
    if (p === "/creator/retention/plan") {
      return json(res, 200, {
        project: "11111111-1111-4111-8111-111111111111",
        collectable: 1,
        entries: [
          {
            id: "44444444-4444-4444-8444-444444444444",
            kind: "proxy",
            class: "derivative",
            reason: "derived from a composition",
            action: "unreferenced",
            protected: false,
            collected: true,
            explain: "44444444-4444-4444-8444-444444444444: proxy — referenced by nothing, so it can be collected",
            locator: "/archive/proxy.mp4",
            bytes: 1048576,
          },
          {
            id: "55555555-5555-4555-8555-555555555555",
            kind: "render",
            class: "derivative",
            reason: "derived from a composition",
            action: "protected",
            protected: true,
            collected: false,
            explain: "55555555-5555-4555-8555-555555555555: render — referenced by release package 66666666-6666-4666-8666-666666666666",
            locator: "/archive/final.mp4",
            bytes: 8388608,
          },
        ],
      });
    }
    if (p === "/creator/retention/collect" && req.method === "POST") {
      return json(res, 200, {
        plan: { project: "11111111-1111-4111-8111-111111111111", collectable: 0, entries: [] },
        removed: ["44444444-4444-4444-8444-444444444444"],
        bytes_removed: ["/archive/proxy.mp4"],
      });
    }
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
        graph: PIPELINE_GRAPH,
      });

    // ── Authoring (todo 53) ───────────────────────────────────────────
    if (p === "/pipelines/templates" && req.method === "GET") {
      const templates = [{
        id: "authoring_fixture", source: "shipped", revision: "fixture-rev-1",
        pipeline: authorPipeline(),
      }];
      // A graph that already carries a duplicate stage id. The editor mints
      // ids and never types them, so this defect cannot be produced through
      // the editor's own controls — it can only arrive on load, which is why
      // the lane drives it from a payload.
      const dupe = authorPipeline();
      dupe.name = "Duplicate ids";
      dupe.stages[1] = { ...dupe.stages[1], id: dupe.stages[0].id };
      templates.push({
        id: "duplicate_ids", source: "authored", revision: "fixture-rev-2",
        pipeline: dupe,
      });
      for (const stored of templateStore.values()) {
        templates.push({ id: stored.id, source: "authored", revision: stored.revision, pipeline: stored.pipeline });
      }
      return json(res, 200, {
        templates,
        registry: { plugins: REGISTRY_PLUGINS },
        rules: ["acyclic", "empty-stage-list", "duplicate-stage-id", "dangling-input", "zero-capacity", "empty-dispatch", "unknown-dispatch-target"],
        unreadable: [],
      });
    }
    {
      const m = p.match(/^\/pipelines\/templates\/([^/]+)$/);
      if (m && req.method === "PUT") {
        const id = decodeURIComponent(m[1]);
        const body = await readBody(req);
        const pipeline = body && body.pipeline;
        if (!pipeline || !Array.isArray(pipeline.stages)) {
          return json(res, 400, { code: "graph/invalid", status: 400, detail: "malformed request", errors: [] });
        }
        // Validation BEFORE the conflict check and before any write, as the
        // real route does: a graph that breaks a rule is refused on its own
        // merits, so the browser's own refusal is the first line of defence
        // and this is the second.
        const errors = authorErrors(pipeline);
        if (errors.length) {
          return json(res, 400, { code: "graph/invalid", title: "Unprocessable Content", status: 400, detail: (errors[0].detail || errors[0].reason), errors });
        }
        const current = templateStore.get(id);
        if (body.base_revision !== (current ? current.revision : null)) {
          return json(res, 409, {
            code: "history/conflict", title: "Conflict", status: 409,
            detail: `this edit was made against revision ${body.base_revision}, but ${current ? current.revision : "none"} is now current`,
            base: body.base_revision === null ? "new" : body.base_revision,
            current: current ? current.revision : "",
          });
        }
        const revision = `rev-${templateStore.size + 1}-${Buffer.from(JSON.stringify(pipeline)).length}`;
        templateStore.set(id, { id, revision, pipeline });
        return json(res, 200, { ok: true, id, revision, errors: [] });
      }
    }

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
    // POST /api/v1/dataviz/run — the real handler is a pure transform
    // (`strivo_dataviz::run`) with no IO, so the fixture computes the
    // answer from the posted corpus rather than returning a canned series:
    // a canned payload would let the spec pass even if the SPA sent the
    // wrong corpus. `speaker_time` and `word_frequency` are implemented —
    // they are the two experiments that read transcript fields rather than
    // the posting client, which is what makes them worth driving here.
    // Anything else 400s the way an unknown experiment does.
    if (p === "/dataviz/run" && req.method === "POST") {
      const body = await readBody(req);
      const exp = body.experiment || {};
      const eps = (body.corpus && body.corpus.episodes) || [];
      if (exp.kind === "speaker_time") {
        const totals = new Map();
        for (const ep of eps) {
          for (const u of ep.utterances || []) {
            // `end_sec === null` is an unmeasured line, and contributes
            // nothing — the same as `strivo_dataviz::Utterance::measured_seconds`.
            const secs =
              u.end_sec == null ? 0 : Math.max(u.end_sec - (u.start_sec || 0), 0);
            totals.set(u.speaker || "Speaker", (totals.get(u.speaker || "Speaker") || 0) + secs);
          }
        }
        const points = [...totals.entries()]
          .map(([label, sec]) => ({ label, value: sec / 60 }))
          .sort((a, b) => b.value - a.value);
        return json(res, 200, {
          series: { label: "Speaker minutes (across corpus)", chart_hint: "bar", points },
        });
      }
      if (exp.kind === "word_frequency") {
        // Mirrors `strivo_dataviz::word_frequency`: keep alphabetic chars
        // and apostrophes, lowercase, drop <3 chars and stop-words, then
        // sort by count desc and label asc.
        const stop = new Set(
          "the and of to a in is you that it for on with as are i this be or by but we an have not they from at your all was so if what can do just like my me he she his her its their out about up than then there here when how who yeah okay ok um uh".split(
            " ",
          ),
        );
        const counts = new Map();
        for (const ep of eps) {
          for (const u of ep.utterances || []) {
            for (const raw of String(u.text || "").split(/\s+/)) {
              const w = raw.replace(/[^a-zA-Z']/g, "").toLowerCase();
              if (w.length < 3 || stop.has(w)) continue;
              counts.set(w, (counts.get(w) || 0) + 1);
            }
          }
        }
        const take = Math.max(1, exp.top_n || 30);
        const points = [...counts.entries()]
          .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0))
          .slice(0, take)
          .map(([label, c]) => ({ label, value: c }));
        return json(res, 200, {
          series: { label: `Top ${take} words`, chart_hint: "bar", points },
        });
      }
      return json(res, 400, {
        detail: `fixture implements speaker_time and word_frequency only, got ${exp.kind}`,
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
