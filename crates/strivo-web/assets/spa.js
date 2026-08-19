// StriVo SPA — vanilla JS, hash routing, *arr-inspired chrome. (W4 MVP.)
//
// This is the minimum-viable shippable webui that uses the W1+W2+W3
// backend. SvelteKit conversion is the W4 phase 2 follow-up; this
// file deliberately stays small + dependency-free.

/* @creator-start */
// Coding Studio surfaces live in their own ES modules under assets/research/
// so they can be built without editing this file. build.rs deletes that whole
// directory from the PVR build, and this import block is stripped with it.
import { mount as mountCodebook } from "./research/codebook.js";
import { mount as mountCorpus } from "./research/corpus.js";
import { mount as mountNotebook } from "./research/notebook.js";

// Registry the Archive route paints from. Keyed by sub-tab id.
const RESEARCH_SURFACES = {
  codebook: { label: "Codebook", mount: mountCodebook },
  corpus: { label: "Corpus", mount: mountCorpus },
  notebook: { label: "Notebook", mount: mountNotebook },
};
/* @creator-end */

const API = {
  _inflight: new Map(),
  _cache: new Map(),
  _cacheTtlMs: 2000,
  async _fetch(path, opts = {}) {
    const method = String(opts.method || "GET").toUpperCase();
    // Collapse duplicate GETs issued by chrome hydration + the route painter.
    // A short stale window absorbs navigation churn without making live state
    // feel cached; SSE still patches the active screen immediately.
    if (method === "GET" && !opts.__direct) {
      const cached = API._cache.get(path);
      if (cached && performance.now() - cached.at < API._cacheTtlMs) {
        return cached.value;
      }
      if (API._inflight.has(path)) return API._inflight.get(path);
      const pending = API._fetch(path, { ...opts, __direct: true })
        .then((value) => {
          API._cache.set(path, { at: performance.now(), value });
          return value;
        })
        .finally(() => API._inflight.delete(path));
      API._inflight.set(path, pending);
      return pending;
    }
    const direct = { ...opts };
    delete direct.__direct;
    // X-Strivo-CSRF is a custom header browsers can't attach cross-site
    // without a (denied) preflight, so it gates cookie-authed mutations
    // against CSRF. Harmless on GETs. See crates/strivo-web/src/csrf.rs.
    const headers = {
      Accept: "application/json",
      "X-Strivo-CSRF": "1",
      ...(opts.headers || {}),
    };
    if (direct.body && typeof direct.body !== "string") {
      headers["Content-Type"] = "application/json";
      direct.body = JSON.stringify(direct.body);
    }
    const res = await fetch(`/api/v1${path}`, {
      credentials: "same-origin",
      ...direct,
      headers,
    });
    if (res.status === 401) {
      route("login");
      throw new Error("unauthorized");
    }
    if (res.status === 402) {
      // Pro gate — extract the plugin name + detail so callers can
      // render a polished upsell card instead of the raw JSON. Detail
      // shape from problem.rs: { detail, instance, status, title, type }.
      let detail = "Strivo Pro plugin — activate or start a 3-day trial.";
      let plugin = null;
      try {
        const j = await res.json();
        if (j && j.detail) {
          detail = j.detail;
          const m = /^([a-z0-9_-]+) is a Strivo Pro plugin/i.exec(j.detail);
          if (m) plugin = m[1];
        }
      } catch (_) { /* keep defaults */ }
      const err = new Error(detail);
      err.code = 402;
      err.plugin = plugin;
      throw err;
    }
    if (!res.ok) {
      // Try to extract problem+json's `detail` for a clean message; fall
      // back to the raw body when the response isn't JSON.
      const text = await res.text();
      let detail = text;
      try {
        const j = JSON.parse(text);
        if (j && typeof j.detail === "string") detail = j.detail;
      } catch (_) { /* not json */ }
      throw new Error(`HTTP ${res.status}: ${detail}`);
    }
    const value = res.headers.get("content-type")?.includes("json")
      ? res.json()
      : res.text();
    if (method !== "GET") API.invalidate();
    return value;
  },
  invalidate(prefix = "") {
    for (const key of API._cache.keys()) {
      if (!prefix || key.startsWith(prefix)) API._cache.delete(key);
    }
  },
  channels: () => API._fetch("/channels"),
  recordings: (opts = { limit: 500 }) => {
    const p = new URLSearchParams();
    if (opts.cursor != null) p.set("cursor", String(opts.cursor));
    if (opts.limit != null) p.set("limit", String(opts.limit));
    const query = p.toString();
    return API._fetch(`/recordings${query ? `?${query}` : ""}`);
  },
  startRecording: (body) =>
    API._fetch("/recordings", { method: "POST", body }),
  stopRecording: (id) =>
    API._fetch(`/recordings/${id}`, { method: "DELETE" }),
  recordingOne: (id) =>
    API._fetch(`/recordings/${encodeURIComponent(id)}`),
  recordingProbe: (id) =>
    API._fetch(`/recordings/${encodeURIComponent(id)}/probe`),
  deleteRecordingFile: (id) =>
    API._fetch(`/recordings/${encodeURIComponent(id)}/file`, { method: "DELETE" }),
  clearErroredRecordings: () =>
    API._fetch("/recordings/clear_errored", { method: "POST" }),
  toggleAutoRecord: (channelKey, enabled, opts = {}) =>
    API._fetch(`/channels/${encodeURIComponent(channelKey)}/auto_record`, {
      method: "PUT",
      body: { enabled, ...opts },
    }),
  // Update format/profile overrides on an already-enabled auto-record entry.
  setAutoRecordFormat: (channelKey, format, profile) =>
    API._fetch(`/channels/${encodeURIComponent(channelKey)}/auto_record`, {
      method: "PUT",
      body: { enabled: true, format: format || "", profile: profile || "" },
    }),
  pollNow: () => API._fetch("/poll_now", { method: "POST" }),
  health: () => API._fetch("/health"),
  healthChecks: () => API._fetch("/health/checks"),
  logs: (level, lines = 300) =>
    API._fetch(`/logs?level=${encodeURIComponent(level || "trace")}&lines=${lines}`),
  setPollInterval: (secs) =>
    API._fetch("/settings/poll_interval", { method: "POST", body: { secs } }),
  backupCreate: () => API._fetch("/backup", { method: "POST" }),
  backups: () => API._fetch("/backups"),
  backupRestore: (name) =>
    API._fetch(`/backups/${encodeURIComponent(name)}/restore`, { method: "POST" }),
  history: (opts = { limit: 200 }) => {
    const p = new URLSearchParams();
    if (opts.cursor != null) p.set("cursor", String(opts.cursor));
    if (opts.limit != null) p.set("limit", String(opts.limit));
    const query = p.toString();
    return API._fetch(`/history${query ? `?${query}` : ""}`);
  },
  blocklist: () => API._fetch("/blocklist"),
  blockAdd: (body) => API._fetch("/blocklist", { method: "POST", body }),
  blockRemove: (body) => API._fetch("/blocklist", { method: "DELETE", body }),
  storage: () => API._fetch("/storage"),
  settings: () => API._fetch("/settings"),
  patreon: () => API._fetch("/patreon"),
  gantt: () => API._fetch("/gantt"),
  /* @creator-start */
  pluginRpc: (plugin, verb, body) =>
    API._fetch(`/plugins/${encodeURIComponent(plugin)}/${encodeURIComponent(verb)}`, {
      method: "POST",
      body,
    }),
  // ── Plugin data (read-only, served from each plugin's SQLite DB) ──
  plugins: () => API._fetch("/plugins"),
  crunchrRecordings: () => API._fetch("/plugins/crunchr/recordings"),
  crunchrRecording: (id) =>
    API._fetch(`/plugins/crunchr/recordings/${encodeURIComponent(id)}`),
  crunchrSearch: (q) =>
    API._fetch(`/plugins/crunchr/search?q=${encodeURIComponent(q)}`),
  archiverChannels: () => API._fetch("/plugins/archiver/channels"),
  archiverVideos: (channelId) =>
    API._fetch(`/plugins/archiver/channels/${encodeURIComponent(channelId)}/videos`),
  viewguardVerdicts: () => API._fetch("/plugins/viewguard/verdicts"),
  viewguardSamples: (channelId) =>
    API._fetch(`/plugins/viewguard/channels/${encodeURIComponent(channelId)}/samples`),
  insightsWords: (opts = {}) => {
    const p = new URLSearchParams();
    if (opts.scope) p.set("scope", opts.scope);
    if (opts.recording) p.set("recording", opts.recording);
    if (opts.stopwords) p.set("stopwords", "true");
    if (opts.limit) p.set("limit", String(opts.limit));
    return API._fetch(`/plugins/insights/words?${p.toString()}`);
  },
  insightsTopics: () => API._fetch("/plugins/insights/topics"),
  insightsSpeakers: (id) =>
    API._fetch(`/plugins/insights/recordings/${encodeURIComponent(id)}/speakers`),
  /* @creator-end */
  bulkDownload: (channelId, body) =>
    API._fetch(`/channels/${encodeURIComponent(channelId)}/bulk`, {
      method: "POST",
      body,
    }),
  requestPlaylists: (channelId) =>
    API._fetch(`/channels/${encodeURIComponent(channelId)}/playlists`, {
      method: "POST",
    }),
  resolveChannel: (platform, query) =>
    API._fetch("/channels/resolve", { method: "POST", body: { platform, query } }),
  requestChannelVods: (channelId, platform) =>
    API._fetch(`/channels/${encodeURIComponent(channelId)}/vods`, {
      method: "POST",
      body: { platform },
    }),
  schedule: () => API._fetch("/schedule"),
  scheduleAdd: (body) => API._fetch("/schedule", { method: "POST", body }),
  scheduleDelete: (index) =>
    API._fetch(`/schedule/${encodeURIComponent(index)}`, { method: "DELETE" }),
  // Monitor (record-when-live + auto-download new uploads).
  monitor: () => API._fetch("/monitor"),
  /* @creator-start */
  setArchiverTandem: (key, enabled) =>
    API._fetch(`/channels/${encodeURIComponent(key)}/archiver_tandem`, {
      method: "PUT",
      body: { enabled },
    }),
  setArchiverPlaylists: (key, playlists) =>
    API._fetch(`/channels/${encodeURIComponent(key)}/archiver_playlists`, {
      method: "PUT",
      body: { playlists },
    }),
  // DAW-vision capability matrix.
  pluginCapabilities: () => API._fetch("/plugins/capabilities"),
  chaptersGenerate: (recordingId) =>
    API._fetch(`/plugins/chapters/${encodeURIComponent(recordingId)}`, { method: "POST" }),
  cuepointsGenerate: (recordingId) =>
    API._fetch(`/plugins/cuepoints/${encodeURIComponent(recordingId)}`, { method: "POST" }),
  clipperAnalyze: (recordingId) =>
    API._fetch(`/plugins/clipper/${encodeURIComponent(recordingId)}/analyze`, { method: "POST" }),
  clipperExtract: (recordingId, body) =>
    API._fetch(`/plugins/clipper/${encodeURIComponent(recordingId)}/extract`, {
      method: "POST",
      body,
    }),
  clipperListClips: (recordingId) =>
    API._fetch(`/plugins/clipper/${encodeURIComponent(recordingId)}/clips`),
  thumbnailsGenerate: (recordingId, body) =>
    API._fetch(`/plugins/thumbnails/${encodeURIComponent(recordingId)}`, { method: "POST", body }),
  thumbnailsList: (recordingId, stem = "candidate") =>
    API._fetch(`/plugins/thumbnails/${encodeURIComponent(recordingId)}/${encodeURIComponent(stem)}`),
  thumbnailFileUrl: (absPath) =>
    `/api/v1/plugins/thumbnails/file?p=${encodeURIComponent(absPath)}`,
  insightsCompare: (recordingA, recordingB) =>
    API._fetch(`/plugins/insights/compare?recs=${encodeURIComponent(recordingA + "," + recordingB)}`),
  insightsRetention: (recordingId, bucketSecs = 30) =>
    API._fetch(`/plugins/insights/retention/${encodeURIComponent(recordingId)}?bucket_secs=${bucketSecs}`),
  captionsExportUrl: (recordingId, fmt = "srt", lang = "en") =>
    `/api/v1/plugins/captions/${encodeURIComponent(recordingId)}?fmt=${encodeURIComponent(fmt)}&lang=${encodeURIComponent(lang)}`,
  multitrackList: (recordingId) =>
    API._fetch(`/plugins/multitrack/${encodeURIComponent(recordingId)}`),
  multitrackExtract: (recordingId, body) =>
    API._fetch(`/plugins/multitrack/${encodeURIComponent(recordingId)}/extract`, { method: "POST", body }),
  brandsafeScan: (recordingId) =>
    API._fetch(`/plugins/brandsafe/${encodeURIComponent(recordingId)}`),
  reuseGenerate: (recordingId) =>
    API._fetch(`/plugins/reuse/${encodeURIComponent(recordingId)}/generate`, { method: "POST" }),
  reuseList: (recordingId) =>
    API._fetch(`/plugins/reuse/${encodeURIComponent(recordingId)}`),
  casebookFetch: (recordingId) =>
    API._fetch(`/plugins/casebook/${encodeURIComponent(recordingId)}?fmt=json`),
  casebookMarkdownUrl: (recordingId) =>
    `/api/v1/plugins/casebook/${encodeURIComponent(recordingId)}?fmt=markdown`,
  heatmapCompute: (recordingId, bucketSecs = 30) =>
    API._fetch(`/plugins/heatmap/${encodeURIComponent(recordingId)}?bucket_secs=${bucketSecs}`),
  editorLoad: (recordingId) =>
    API._fetch(`/plugins/editor/${encodeURIComponent(recordingId)}`),
  editorSave: (recordingId, edl, label) => {
    const qs = label ? `?label=${encodeURIComponent(label)}` : "";
    return API._fetch(`/plugins/editor/${encodeURIComponent(recordingId)}${qs}`, { method: "POST", body: edl });
  },
  editorRevisions: (recordingId) =>
    API._fetch(`/plugins/editor/${encodeURIComponent(recordingId)}/revisions`),
  editorRevisionRestore: (recordingId, revId) =>
    API._fetch(`/plugins/editor/${encodeURIComponent(recordingId)}/revisions/${encodeURIComponent(revId)}/restore`, { method: "POST" }),
  editorRender: (recordingId) =>
    API._fetch(`/plugins/editor/${encodeURIComponent(recordingId)}/render`, { method: "POST" }),
  datavizRun: (corpus, experiment) =>
    API._fetch(`/dataviz/run`, { method: "POST", body: { corpus, experiment } }),
  crunchrTranscript: (recordingId) =>
    API._fetch(`/plugins/crunchr/transcript/${encodeURIComponent(recordingId)}`).catch(() => null),
  /* @creator-end */
  chatSend: (room, text) =>
    API._fetch(`/chat/send`, {
      method: "POST",
      body: { room, text },
    }),
  /* @creator-start */
  chatDensityCompute: (recordingId, body) =>
    API._fetch(`/plugins/chat-density/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body,
    }),
  scheduleOptimizerRun: (recordingId, body) =>
    API._fetch(`/plugins/schedule-optimizer/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body,
    }),
  scenesList: (recordingId) =>
    API._fetch(`/plugins/scenes/${encodeURIComponent(recordingId)}`),
  scenesCapture: (recordingId, name, thumbnailDataUrl) =>
    API._fetch(`/plugins/scenes/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body: { name, thumbnail_data_url: thumbnailDataUrl || null },
    }),
  scenesRestore: (recordingId, sceneId) =>
    API._fetch(
      `/plugins/scenes/${encodeURIComponent(recordingId)}/${encodeURIComponent(sceneId)}/restore`,
      { method: "POST" },
    ),
  scenesDelete: (recordingId, sceneId) =>
    API._fetch(
      `/plugins/scenes/${encodeURIComponent(recordingId)}/${encodeURIComponent(sceneId)}`,
      { method: "DELETE" },
    ),
  sidechainBuild: (recordingId, body) =>
    API._fetch(`/plugins/sidechain/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body,
    }),
  insertFxLoad: (recordingId) =>
    API._fetch(`/plugins/insert-fx/${encodeURIComponent(recordingId)}`),
  insertFxSave: (recordingId, chain) =>
    API._fetch(`/plugins/insert-fx/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body: chain,
    }),
  insertFxPreset: (recordingId, bus) =>
    API._fetch(
      `/plugins/insert-fx/${encodeURIComponent(recordingId)}/preset/${encodeURIComponent(bus)}`,
      { method: "POST" },
    ),
  pitchLoad: (recordingId) =>
    API._fetch(`/plugins/pitch/${encodeURIComponent(recordingId)}`),
  pitchSave: (recordingId, body) =>
    API._fetch(`/plugins/pitch/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body,
    }),
  pitchFit: (recordingId, sourceSec, targetSec) =>
    API._fetch(`/plugins/pitch/${encodeURIComponent(recordingId)}/fit`, {
      method: "POST",
      body: { source_duration_sec: sourceSec, target_duration_sec: targetSec },
    }),
  abRenderLoad: (recordingId) =>
    API._fetch(`/plugins/ab-render/${encodeURIComponent(recordingId)}`),
  abRenderSave: (recordingId, slot, variant) =>
    API._fetch(
      `/plugins/ab-render/${encodeURIComponent(recordingId)}/${encodeURIComponent(slot)}`,
      { method: "POST", body: variant },
    ),
  abRenderCompare: (recordingId) =>
    API._fetch(`/plugins/ab-render/${encodeURIComponent(recordingId)}/compare`, {
      method: "POST",
    }),
  submixLoad: (recordingId) =>
    API._fetch(`/plugins/submix/${encodeURIComponent(recordingId)}`),
  submixSave: (recordingId, mix) =>
    API._fetch(`/plugins/submix/${encodeURIComponent(recordingId)}`, {
      method: "POST",
      body: mix,
    }),
  pluginStorageSize: (name) =>
    API._fetch(`/plugin-storage/${encodeURIComponent(name)}`),
  pluginStorageClear: (name) =>
    API._fetch(`/plugin-storage/${encodeURIComponent(name)}`, { method: "DELETE" }),
  beatDetectRun: (recordingId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.window_sec != null) p.set("window_sec", opts.window_sec);
    if (opts.min_bpm != null) p.set("min_bpm", opts.min_bpm);
    if (opts.max_bpm != null) p.set("max_bpm", opts.max_bpm);
    if (opts.top_n != null) p.set("top_n", opts.top_n);
    const qs = p.toString();
    return API._fetch(
      `/plugins/beat-detect/${encodeURIComponent(recordingId)}${qs ? "?" + qs : ""}`,
      { method: "POST" },
    );
  },
  vadAnalyze: (recordingId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.window_sec != null) p.set("window_sec", opts.window_sec);
    if (opts.open_db != null) p.set("open_db", opts.open_db);
    if (opts.close_db != null) p.set("close_db", opts.close_db);
    if (opts.min_keep_sec != null) p.set("min_keep_sec", opts.min_keep_sec);
    const qs = p.toString() ? `?${p.toString()}` : "";
    return API._fetch(`/plugins/vad/${encodeURIComponent(recordingId)}${qs}`, { method: "POST" });
  },
  deadairDetect: (recordingId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.noise_db != null) p.set("noise_db", opts.noise_db);
    if (opts.min_span_secs != null) p.set("min_span_secs", opts.min_span_secs);
    if (opts.trim_threshold_secs != null) p.set("trim_threshold_secs", opts.trim_threshold_secs);
    const qs = p.toString() ? `?${p.toString()}` : "";
    return API._fetch(`/plugins/deadair/${encodeURIComponent(recordingId)}${qs}`, { method: "POST" });
  },
  chatRooms: () => API._fetch("/plugins/chat/rooms"),
  chatParseBatch: (lines) =>
    API._fetch("/plugins/chat/parse", { method: "POST", body: { lines: lines.join("\n") } }),
  structureClassify: (recordingId, body) =>
    API._fetch(`/plugins/structure/${encodeURIComponent(recordingId)}`, { method: "POST", body }),
  loudnessMeasure: (recordingId, platform) => {
    const qs = platform ? `?platform=${encodeURIComponent(platform)}` : "";
    return API._fetch(`/plugins/loudness/${encodeURIComponent(recordingId)}${qs}`, { method: "POST" });
  },
  brandingLoad: (recordingId) =>
    API._fetch(`/plugins/branding/${encodeURIComponent(recordingId)}`),
  brandingSave: (recordingId, spec) =>
    API._fetch(`/plugins/branding/${encodeURIComponent(recordingId)}`, { method: "POST", body: spec }),
  viewguardTrend: () => API._fetch("/plugins/viewguard/trend"),
  pipelinesDag: () => API._fetch("/pipelines/dag"),
  pipelineRuns: () => API._fetch("/pipelines/runs"),
  pipelineRun: (recordingId, template = "creator_publish") =>
    API._fetch("/pipelines/runs", {
      method: "POST",
      body: { recording_id: recordingId, template },
    }),
  pipelineCancel: (id) =>
    API._fetch(`/pipelines/runs/${encodeURIComponent(id)}/cancel`, { method: "POST" }),
  pipelineRetryStage: (id) =>
    API._fetch(`/pipelines/stages/${encodeURIComponent(id)}/retry`, { method: "POST" }),
  marketplaceCatalog: () => API._fetch("/marketplace/catalog"),
  // CE-Fusion F4/F5 — Archive surface over the research kernel. Backend
  // lives in crates/strivo-web/src/routes/plugins.rs (research_* handlers),
  // creator-only like every route above.
  researchProjects: () => API._fetch("/research/projects"),
  researchCreateProject: (name, description = "") =>
    API._fetch("/research/projects", { method: "POST", body: { name, description } }),
  researchProjectDetail: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}`),
  researchSearch: (projectId, q, opts = {}) => {
    const p = new URLSearchParams({ q });
    p.set("limit", String(opts.limit ?? 20));
    p.set("offset", String(opts.offset ?? 0));
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/search?${p.toString()}`);
  },
  researchMoments: (projectId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.sourceId) p.set("source_id", opts.sourceId);
    if (opts.minConfidence != null) p.set("min_confidence", String(opts.minConfidence));
    p.set("limit", String(opts.limit ?? 20));
    p.set("offset", String(opts.offset ?? 0));
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/moments?${p.toString()}`);
  },
  researchCreateMoment: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/moments`, { method: "POST", body }),
  researchMigrateCrunchr: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/migrate/crunchr`, { method: "POST" }),
  researchMigrateLegacy: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/migrate/legacy`, { method: "POST" }),
  // Coding Studio surfaces (codebook.js / corpus.js / notebook.js) — the
  // rest of the research kernel surfaced under the Archive route's sub-tabs.
  researchCodes: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/codes`),
  researchCreateCode: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/codes`, { method: "POST", body }),
  researchCodings: (projectId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.codeId) p.set("code_id", opts.codeId);
    const qs = p.toString() ? `?${p.toString()}` : "";
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/codings${qs}`);
  },
  researchCreateCoding: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/codings`, { method: "POST", body }),
  researchSources: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/sources`),
  researchCreateSource: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/sources`, { method: "POST", body }),
  researchCases: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/cases`),
  researchCreateCase: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/cases`, { method: "POST", body }),
  researchAddCaseSource: (projectId, caseId, sourceId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/cases/${encodeURIComponent(caseId)}/sources`, {
      method: "POST",
      body: { source_id: sourceId },
    }),
  researchSignals: (projectId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.sourceId) p.set("source_id", opts.sourceId);
    if (opts.kind) p.set("kind", opts.kind);
    p.set("limit", String(opts.limit ?? 50));
    p.set("offset", String(opts.offset ?? 0));
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/signals?${p.toString()}`);
  },
  researchMemos: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/memos`),
  researchCreateMemo: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/memos`, { method: "POST", body }),
  researchRelationships: (projectId) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/relationships`),
  researchCreateRelationship: (projectId, body) =>
    API._fetch(`/research/projects/${encodeURIComponent(projectId)}/relationships`, { method: "POST", body }),
  researchAgreement: (projectId, opts = {}) => {
    const p = new URLSearchParams();
    if (opts.codeId) p.set("code_id", opts.codeId);
    if (opts.authorA) p.set("author_a", opts.authorA);
    if (opts.authorB) p.set("author_b", opts.authorB);
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/agreement?${p.toString()}`);
  },
  researchExport: (projectId, opts = {}) => {
    const p = new URLSearchParams();
    p.set("format", opts.format || "json");
    if (opts.limit != null) p.set("limit", String(opts.limit));
    if (opts.offset != null) p.set("offset", String(opts.offset));
    return API._fetch(`/research/projects/${encodeURIComponent(projectId)}/export?${p.toString()}`);
  },
  /* @creator-end */
  // Multi-stream tile layout for the watch player. Core (single + multi
  // view), available in the PVR build — kept outside the @creator block so
  // the player isn't broken by a missing API method.
  multistreamTiles: (containerW, containerH, mode, host) => {
    const p = new URLSearchParams({ container_w: containerW, container_h: containerH, host });
    if (mode) p.set("mode", JSON.stringify(mode));
    return API._fetch(`/multistream/tiles?${p.toString()}`);
  },
  patreonPull: (body) =>
    API._fetch("/patreon/pull", { method: "POST", body }),
  vodDownload: (body) =>
    API._fetch("/vods/download", { method: "POST", body }),
  remuxRecording: (id) =>
    API._fetch(`/recordings/${encodeURIComponent(id)}/remux`, { method: "POST" }),
  login: (apiKey) =>
    API._fetch("/auth/login", { method: "POST", body: { api_key: apiKey } }),
  logout: () => API._fetch("/auth/logout", { method: "POST" }),
  // ── Strivo Pro licensing (Phase 1: status only; activate/trial 501) ──
  updateSetting: (path, value) =>
    API._fetch("/settings/update", { method: "POST", body: { path, value } }),
  setPlatform: (name, body) =>
    API._fetch(`/settings/platform/${encodeURIComponent(name)}`, {
      method: "POST",
      body,
    }),
  licenceStatus: () => API._fetch("/licence/status"),
  licenceTrial: () => API._fetch("/licence/trial", { method: "POST" }),
  licenceActivate: (key) =>
    API._fetch("/licence/activate", { method: "POST", body: { key } }),
  licenceTrial: () => API._fetch("/licence/trial", { method: "POST" }),
  // ── Capture-profile CRUD ─────────────────────────────────────────────
  captureProfileCreate: (profile) =>
    API._fetch("/capture_profiles", { method: "POST", body: profile }),
  captureProfileUpdate: (name, profile) =>
    API._fetch(`/capture_profiles/${encodeURIComponent(name)}`, { method: "PUT", body: profile }),
  captureProfileDelete: (name) =>
    API._fetch(`/capture_profiles/${encodeURIComponent(name)}`, { method: "DELETE" }),
  // ── Channel JSON export / import ─────────────────────────────────────
  channelsExport: () => API._fetch("/channels/export"),
  channelsImport: (data) =>
    API._fetch("/channels/import", { method: "POST", body: data }),
};

// ── SSE event stream ─────────────────────────────────────────────────
const events = {
  source: null,
  listeners: new Set(),
  degradedPoll: null,
  start() {
    if (this.source) return;
    this.source = new EventSource("/events", { withCredentials: true });
    this.source.onopen = () => this.setConnected(true);
    this.source.onmessage = (e) => {
      this.setConnected(true);
      try {
        const data = JSON.parse(e.data);
        this.listeners.forEach((fn) => fn(data));
      } catch (_) {}
    };
    this.source.onerror = () => {
      // Make the stale-data state VISIBLE (research §A/§5: silent
      // real-time breakage is the #1 cited gotcha). The browser
      // auto-reconnects on transient errors; meanwhile we show a pill
      // and degrade to a slow poll so list views don't go stale.
      this.setConnected(false);
      // On a hard close (e.g. a 401 before login — /events is now
      // authenticated), EventSource will NOT auto-reconnect. Recreate it on
      // a timer so the stream comes back once the session cookie is set.
      if (this.source && this.source.readyState === EventSource.CLOSED) {
        this.source.close();
        this.source = null;
        setTimeout(() => this.start(), 3000);
      }
    };
  },
  // Show/hide the topbar "reconnecting…" pill and arm/disarm a 10s
  // degraded re-poll of the current data route.
  setConnected(ok) {
    const pill = document.getElementById("conn-status");
    if (pill) pill.hidden = ok;
    if (ok) {
      if (this.degradedPoll) {
        clearInterval(this.degradedPoll);
        this.degradedPoll = null;
      }
    } else if (!this.degradedPoll) {
      this.degradedPoll = setInterval(() => {
        if (document.hidden) return;
        const r = currentRoute();
        if (r === "library") renderHome().catch(() => {});
        else if (r === "recordings") renderRecordings().catch(() => {});
      }, 10000);
    }
  },
  on(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  },
};

// #74 — per-channel bulk-download status, keyed by channel_id:
// { done, total, active }. Fed by the `bulk-progress` SSE event.
const bulkStatus = {};
// #75 — Patreon snapshot, fed by the `patreon-state` SSE event:
// { creators: [ChannelEntry], posts: { campaign_id: [PatreonPost] } }.
const patreonState = { creators: [], posts: {} };
// W4-alt — recordings grid sort/filter state + last-fetched cache.
let recSort = { col: "started", dir: "desc" };
let recFilter = "";
let recCache = [];
// Item 22 — recordings index density (compact|comfortable) + multi-select.
let recDensity = localStorage.getItem("strivo-rec-density") || "comfortable";
let recSelected = new Set();
// State chip filter — Set of state classnames the user has whitelisted
// ("finished", "recording", "downloading", "failed", "file-error"…).
// Empty = no filter (show everything). Persisted across page reloads.
let recStateFilter = new Set(
  (localStorage.getItem("strivo-rec-state-filter") || "")
    .split(",").filter(Boolean),
);
// Group-by toggle — "none" or "channel". Persisted; respects the
// Settings → Layout default when one has been set.
let recGroupBy = localStorage.getItem("strivo-rec-groupby")
  || localStorage.getItem("strivo-layout-rec-groupby")
  || "none";
// Date-range filter on started_at — ISO-prefix bounds, inclusive.
// Empty string = unbounded on that side.
let recDateFrom = "";
let recDateTo = "";
// Anchor for shift+click range selection. Tracks the last row whose
// selection state was toggled by direct interaction (click on checkbox or
// modifier+click on row). Reset when the recordings page re-renders.
let recAnchorId = null;
// TUI-redesign — left-rail channel cache, current selection, per-channel
// VOD cache (channel_id -> [VodEntry]), and the recordings dashboard cache.
let channelCache = [];
let selectedChannelKey = null;
const channelVods = {};
// Per-VOD download state for the Past Broadcasts / Recent uploads pills.
// Keys: VOD URL. Values: "downloading" | "downloaded". Absence = idle.
// Seeded from recCache on every recordings refresh via
// `seedVodDownloadStateFromRecCache()` — correlation is by exact source_url
// match (RecordingJob.source_url, stamped on DownloadVod), so a page reload
// or a previously-finished download both surface correctly without a FIFO
// guess.
const vodDownloadState = {};
let dashRecordings = [];
let dashSchedule = [];
// Cached max-concurrent-recordings limit from /settings; updated by
// setupChromeHandlers on every page render so the topbar slot pill
// stays in sync when the user changes the limit on the Monitor page.
let maxConcurrentSlots = 0;

// True for recording states still in flight.
function isInProgress(state) {
  const s = stateLabel(state).toLowerCase();
  return s.includes("record") || s.includes("resolv") || s.includes("stopp");
}

// ── Toasts (research §D) ──────────────────────────────────────────────
// One singleton with two pre-created ARIA live regions: polite for
// success/info, assertive for errors. Errors are sticky (action-needed);
// success/info auto-dismiss with hover-pause. Toasts are non-interactive
// (message + close only).
const Toast = (() => {
  let polite, assertive;
  function ensure() {
    if (polite && document.body.contains(polite)) return;
    const wrap = document.createElement("div");
    wrap.className = "toast-wrap";
    const mk = (role, live) => {
      const r = document.createElement("div");
      r.className = "toast-region";
      r.setAttribute("role", role);
      r.setAttribute("aria-live", live);
      return r;
    };
    assertive = mk("alert", "assertive");
    polite = mk("status", "polite");
    wrap.append(assertive, polite);
    document.body.appendChild(wrap);
  }
  // Pre-create the live regions at load so screen readers register them
  // BEFORE any message is injected — injecting a region and its content in
  // the same frame is unreliably announced (item 24).
  if (typeof document !== "undefined" && document.body) ensure();
  function show(kind, msg, sticky) {
    ensure();
    const region = kind === "error" ? assertive : polite;
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    el.innerHTML = `<span class="toast-msg"></span><button class="toast-close" aria-label="Dismiss">×</button>`;
    el.querySelector(".toast-msg").textContent = msg;
    const close = () => {
      el.classList.add("out");
      setTimeout(() => el.remove(), 200);
    };
    el.querySelector(".toast-close").addEventListener("click", close);
    region.appendChild(el);
    while (region.children.length > 4) region.firstChild.remove();
    if (!sticky) {
      const ttl = 5000;
      let timer = setTimeout(close, ttl);
      el.addEventListener("mouseenter", () => clearTimeout(timer));
      el.addEventListener("mouseleave", () => (timer = setTimeout(close, ttl)));
    }
    return close;
  }
  return {
    success: (m) => show("success", m, false),
    info: (m) => show("info", m, false),
    error: (m) => show("error", m, true), // sticky — user must see/dismiss
  };
})();

// Focus-trapped confirmation dialog for destructive actions. Resolves
// true/false. (research §D: modals only for irreversible actions.)
function confirmDialog(message, opts = {}) {
  return new Promise((resolve) => {
    const prev = document.activeElement;
    const modal = document.createElement("div");
    modal.className = "kbd-help open confirm-modal";
    modal.innerHTML = `<div class="card" role="alertdialog" aria-modal="true">
      <p class="confirm-msg"></p>
      <div class="confirm-actions">
        <button class="confirm-cancel">${htmlEscape(opts.cancel || "Cancel")}</button>
        <button class="confirm-ok ${opts.danger ? "danger" : "primary"}">${htmlEscape(opts.ok || "Confirm")}</button>
      </div></div>`;
    modal.querySelector(".confirm-msg").textContent = message;
    document.body.appendChild(modal);
    // B1: track modal-open via reference count so confirms stacked
    // alongside other modals don't yank the body lock prematurely.
    bumpModalOpen(+1);
    const ok = modal.querySelector(".confirm-ok");
    const cancel = modal.querySelector(".confirm-cancel");
    const done = (v) => {
      modal.remove();
      bumpModalOpen(-1);
      // C5: only restore focus if prev still exists in the document.
      if (prev && prev.isConnected && prev.focus) prev.focus();
      else document.getElementById("brand-home")?.focus?.();
      resolve(v);
    };
    ok.addEventListener("click", () => done(true));
    cancel.addEventListener("click", () => done(false));
    modal.addEventListener("click", (e) => {
      if (e.target === modal) done(false);
    });
    modal.addEventListener("keydown", (e) => {
      if (e.key === "Escape") { e.preventDefault(); done(false); }
      if (e.key === "Tab") {
        e.preventDefault();
        (document.activeElement === ok ? cancel : ok).focus();
      }
    });
    ok.focus();
  });
}

// B3/B17/B20: reference-counted body.modal-open so stacked modals
// (confirm-over-info, info-over-palette, etc.) don't fight over the
// single class. Every modal-open writer bumps +1 on open and -1 on
// close. The class flips off only when the count hits 0.
let _modalOpenCount = 0;
function bumpModalOpen(delta) {
  _modalOpenCount = Math.max(0, _modalOpenCount + delta);
  document.body.classList.toggle("modal-open", _modalOpenCount > 0);
}

// Run an async action with a busy/debounced button: aria-busy + label
// swap + double-fire guard. Safe even if the handler re-renders the page.
async function withBusy(btn, busyLabel, fn, timeoutMs = 30000) {
  if (btn) {
    if (btn.dataset.busy === "1") return; // debounce double-submit
    btn.dataset.busy = "1";
    btn.setAttribute("aria-busy", "true");
    btn.classList.add("busy");
    if (busyLabel) {
      btn.dataset.prevLabel = btn.textContent;
      btn.textContent = busyLabel;
    }
  }
  // Never strand a spinner: race the work against a timeout so a hung
  // request still tears the busy state down and surfaces an error (item 25).
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("timed out")), timeoutMs);
  });
  try {
    return await Promise.race([fn(), timeout]);
  } finally {
    clearTimeout(timer);
    if (btn && btn.isConnected) {
      btn.dataset.busy = "0";
      btn.removeAttribute("aria-busy");
      btn.classList.remove("busy");
      if (btn.dataset.prevLabel) btn.textContent = btn.dataset.prevLabel;
    }
  }
}

// ── Hash router ──────────────────────────────────────────────────────
const ROUTES = [
  "library",
  "recordings",
  "schedule",
  "watch",
  "studio",
  "analytics",
  "publish",
  "pipelines",
  "viewer",
  "dataviz",
  "archive",
  "plugins",
  "chat",
  "settings",
  "system",
  "logs",
  "history",
  "login",
];

function currentRoute() {
  // Strip any query string ("#/recordings?channel=foo") so the route
  // matcher only sees the path segment.
  const raw = window.location.hash.replace(/^#\/?/, "").split("?")[0];
  const hash = raw || "library";
  // Sub-routes (e.g. #/plugins/crunchr) highlight their base tab.
  const base = hash.split("/")[0];
  return ROUTES.includes(base) ? base : "library";
}

// Path segments after the leading "#/", e.g. #/plugins/crunchr/rec/<id>
// → ["plugins", "crunchr", "rec", "<id>"].
function routeParts() {
  return window.location.hash
    .replace(/^#\/?/, "")
    .split("/")
    .filter(Boolean)
    .map((s) => decodeURIComponent(s));
}

function route(name) {
  window.location.hash = `#/${name}`;
}

window.addEventListener("hashchange", render);

// C1/C15: defensive preventDefault for anchors styled as buttons —
// any <a href="#" data-action / data-seek / data-trace> would otherwise
// navigate to '#' if clicked BEFORE its specific handler wires. The
// per-element click listener still does the actual work; this just
// stops the default navigation flicker from racing it. Document-level
// capture so it runs before per-element handlers.
document.addEventListener("click", (e) => {
  const a = e.target.closest('a[href="#"], a[href="#/"]');
  if (!a) return;
  // Only suppress for the styled-as-button cases — any anchor with
  // one of these data attrs is acting as a click target.
  if (a.matches("[data-action], [data-seek], [data-trace]")) {
    e.preventDefault();
  }
}, true);

// ── Render ───────────────────────────────────────────────────────────
const root = document.getElementById("app");

// Per-route cache requirements. Every entry lists which module-scoped
// caches the route's render needs hydrated before chrome() paints.
// Routes that depend on the chrome's rail (channelCache + recCache)
// MUST list both — direct deep-link entry never runs renderHome() so
// the rail would otherwise be empty.
const ROUTE_HYDRATION = {
  library:     ["channels", "recordings", "schedule", "patreon"],
  recordings:  ["recordings"],
  watch:       ["channels", "recordings"],
  schedule:    ["recordings"], // active-count chip needs it
  pipelines:   ["channels"],
  studio:      ["channels"],
  analytics:   ["channels"],
  publish:     ["channels"],
  viewer:      ["channels", "recordings"],
  dataviz:     ["recordings"],
  archive:     ["channels"],
  plugins:     ["channels"],
  chat:        ["channels"],
  history:     ["recordings"],
};

async function ensureRouteHydration(route) {
  const wants = ROUTE_HYDRATION[route] || ["channels"];
  const jobs = [];
  // A2/A3: track failed hydrations so the UI can surface a toast
  // instead of silently rendering an empty rail / 0-active-count.
  const failures = [];
  if (wants.includes("channels") && !channelCache.length) {
    jobs.push(API.channels()
      .then((r) => { channelCache = r.channels || []; })
      .catch((e) => { failures.push(`channels: ${e.message || e}`); }));
  }
  if (wants.includes("recordings") && !recCache.length) {
    jobs.push(API.recordings()
      .then((r) => { recCache = r.recordings || []; dashRecordings = recCache; })
      .catch((e) => { failures.push(`recordings: ${e.message || e}`); }));
  }
  if (wants.includes("schedule") && !dashSchedule.length) {
    jobs.push(API.schedule()
      .then((r) => { dashSchedule = r.schedule || []; })
      .catch((e) => { failures.push(`schedule: ${e.message || e}`); }));
  }
  if (wants.includes("patreon") && !(patreonState.creators || []).length) {
    jobs.push(typeof seedPatreon === "function" ? seedPatreon().catch(() => {}) : Promise.resolve());
  }
  if (jobs.length) await Promise.allSettled(jobs);
  // Suppress 401 noise (handled by the auth probe a few lines later).
  const real = failures.filter((s) => !/unauthorized/i.test(s));
  if (real.length && typeof Toast !== "undefined") {
    Toast.error?.(`Couldn't load: ${real.join(" · ")}`);
  }
}

// A15: unified lookup across both channel sources. Patreon channels live
// in patreonState.creators, regular channels in channelCache; anywhere
// the code needs to resolve a channel by id OR (platform, id) tuple
// should call this so nothing is missing for Patreon.
function findChannelById(id, platform) {
  if (!id) return null;
  const inMain = channelCache.find((c) => c.id === id && (!platform || c.platform === platform));
  if (inMain) return inMain;
  const inPatreon = (patreonState.creators || []).find((c) => c.id === id && (!platform || c.platform === platform));
  return inPatreon || null;
}

// Centralised per-route teardown — all per-route timers + transient
// UI flags get cleared on every render() call so no route bleed.
function teardownAcrossRoutes() {
  // Player controllers hold live vendor connections; dropping the DOM does
  // not close them.
  if (typeof destroyAllControllers === "function") destroyAllControllers();
  // Modals: kbd-help + body class + every app-modal still in the DOM.
  document.getElementById("kbd-help")?.classList.remove("open");
  // B3: route change always zeroes the modal-open ref count + the
  // class. Any modal that didn't pair its open/close cleanly is
  // forcibly cleaned up at the boundary.
  _modalOpenCount = 0;
  document.body.classList.remove("modal-open");
  try { closeAllAppModals?.(); } catch (_) {}
  try { closeRecordingModals?.(); } catch (_) {}
  // Per-route timers that previously leaked between routes.
  if (typeof cdPosterTimer !== "undefined" && cdPosterTimer) { clearInterval(cdPosterTimer); cdPosterTimer = null; }
  if (playerState && playerState.refreshTimer) { clearInterval(playerState.refreshTimer); playerState.refreshTimer = null; }
  if (typeof _watchRefreshTimer !== "undefined" && _watchRefreshTimer) { clearInterval(_watchRefreshTimer); _watchRefreshTimer = null; }
  // Transient keyboard-prefix + command-palette state.
  if (typeof prefixActive !== "undefined") prefixActive = false;
  if (typeof prefixTimer !== "undefined" && prefixTimer) { clearTimeout(prefixTimer); prefixTimer = null; }
}

async function render() {
  const r = currentRoute();
  // Bounce Creator Edition deep-links to Home in the pure-PVR build (their
  // backend routes don't exist here). The hash change re-enters render().
  if (CREATOR_ROUTES.has(r) && !CREATOR_ENABLED) {
    location.hash = "#/library";
    return;
  }
  teardownAcrossRoutes();
  // P0 perf: tear down per-route long-lived resources before painting
  // the next route. Chat WebSockets, chat buffers, and dataviz resize
  // listeners were accumulating across navigations.
  if (r !== "chat" && typeof chatState !== "undefined") {
    // Only kill sockets that no painter still references — the
    // player-view chat rail keeps its room alive across navigations
    // when open.
    for (const room of Object.keys(chatState.sockets || {})) {
      if (!mountedChatRooms.has(room)) {
        try { disconnectChatRoom(room); } catch (_) {}
        delete chatState.buffers[room];
      }
    }
  }
  if (r !== "dataviz" && typeof teardownDataviz === "function") {
    teardownDataviz();
  }
  if (r !== "archive" && typeof teardownArchive === "function") {
    teardownArchive();
  }
  // Clear any prior per-page hint before the new route paints; it'll be
  // re-mounted (if applicable) by maybeMountPageHint after the route
  // renderer finishes. Avoids stale Library copy bleeding onto Chat etc.
  document.getElementById("page-hint")?.remove();
  // Universal pre-paint hydration. Every chrome-painting route gets
  // its declared cache dependencies fetched in parallel before its
  // renderer runs. Prevents the 'rail empty on deep-link' family of
  // bugs across every Pro / Free / Util route.
  if (r !== "login") {
    await ensureRouteHydration(r);
  }
  // Probe auth — if /health returns 401-ish, we land on login.
  if (r !== "login") {
    try {
      await API.health();
    } catch (e) {
      // health is unauthenticated; this catch means real network/server
      // issue. Surface and continue.
      console.warn(e);
    }
    // The first real call that hits an auth check will redirect to
    // /login on 401 via the API._fetch path.
  }
  switch (r) {
    case "login":
      renderLogin();
      break;
    case "library":
      await renderHome();
      break;
    case "recordings":
      await renderRecordings();
      break;
    case "schedule":
      await renderSchedule();
      break;
    case "pipelines":
      await renderPipelines();
      break;
    case "plugins":
      await renderPlugins();
      break;
    case "studio":
      await renderProApp("studio");
      break;
    case "analytics":
      await renderProApp("analytics");
      break;
    case "publish":
      await renderProApp("publish");
      break;
    case "watch":
      await renderWatch();
      break;
    case "viewer":
      await renderViewer();
      break;
    case "dataviz":
      await renderDataviz();
      break;
    case "archive":
      await renderArchive();
      break;
    case "chat":
      await renderChat();
      break;
    case "settings":
      await renderSettings();
      break;
    case "system":
      await renderSystem();
      break;
    case "logs":
      await renderLogs();
      break;
    case "history":
      await renderHistory();
      break;
  }
  // After whichever route renderer finishes, mount its per-page hint
  // unconditionally. Renderers that already call setupChromeHandlers()
  // (most of them) mounted earlier; this is a belt for the few that
  // bypass it (renderChat, renderWatch). maybeMountPageHint short-
  // circuits when a hint is already present, so the double call is
  // safe + idempotent.
  maybeMountPageHint(r);
}

// ── Edition gating ────────────────────────────────────────────────────
// The SPA bundle is shared by both editions. `creator_enabled` from
// /api/v1/settings says whether this server is the Creator Edition; the
// pure-PVR daemon does not mount the plugin/tooling routes, so those nav
// entries + actions would 404. Default false (a missing flag = PVR/older
// daemon) so we fail closed and hide creator surfaces.
let CREATOR_ENABLED = false;
// Routes whose backend lives behind `--features creator`. (Chat is NOT here:
// it speaks Twitch IRC directly from the browser, so it works in the PVR build.)
const CREATOR_ROUTES = new Set([
  "studio", "analytics", "publish", "pipelines", "plugins", "dataviz", "archive",
]);

// Reduced motion (F-18): the `ui.reduce_motion` setting is a manual override
// on top of the OS-level `prefers-reduced-motion` media query — either one
// being "on" should suppress decorative animation. We track the setting
// value ourselves (settings responses aren't otherwise cached globally) and
// recompute the effective state whenever the setting changes or the OS
// preference flips, so no reload is required either way.
let REDUCE_MOTION_SETTING = false;
const REDUCE_MOTION_QUERY = window.matchMedia
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : null;
function applyReducedMotion() {
  const osReduced = !!(REDUCE_MOTION_QUERY && REDUCE_MOTION_QUERY.matches);
  document.documentElement.classList.toggle("reduce-motion", REDUCE_MOTION_SETTING || osReduced);
}
if (REDUCE_MOTION_QUERY) {
  if (REDUCE_MOTION_QUERY.addEventListener) {
    REDUCE_MOTION_QUERY.addEventListener("change", applyReducedMotion);
  } else if (REDUCE_MOTION_QUERY.addListener) {
    // Safari < 14 fallback.
    REDUCE_MOTION_QUERY.addListener(applyReducedMotion);
  }
}
// Apply the OS preference immediately, before settings have loaded, so a
// reduced-motion browser never sees an unsuppressed boot-glyph pulse.
applyReducedMotion();

async function fetchEdition() {
  try {
    const st = await API.settings();
    CREATOR_ENABLED = !!st.creator_enabled;
    REDUCE_MOTION_SETTING = !!(st.ui && st.ui.reduce_motion);
    applyReducedMotion();
  } catch (_) {
    CREATOR_ENABLED = false;
  }
}

// Top-bar route nav (functional pages). The left rail is the channel
// list now; these icon links reach the management pages.
// Tuple: [route, fallbackGlyph, label, key, iconHref?]
// Eight slots ship Eliver Lara's candy-icons (GPL-3.0, vendored under
// /assets/icons/candy/ with the upstream LICENSE + ATTRIBUTION). History
// keeps its Unicode glyph by the user's choice.
const TOPNAV = [
  // Free panes — capture-loop core.
  ["library", "▣", "Home", "l", "/assets/icons/candy/home.svg"],
  ["recordings", "📁", "Recordings", "r", "/assets/icons/candy/recordings.svg"],
  ["schedule", "📅", "Monitor", "s", "/assets/icons/candy/schedule.svg"],
  ["watch", "▶", "Player", "w", "/assets/icons/candy/watch.svg"],
  // Pro panes — unified app, each pane bundles every contributing
  // plugin's UI under its own tabs. Discrete plugin entries are kept
  // accessible via /plugins → deep-link rows but no longer hold the
  // primary topnav slot.
  ["studio", "🎬", "Studio", "u", "/assets/icons/candy/plugins.svg"],
  ["analytics", "📈", "Analytics", "a", "/assets/icons/sweet-folders/folder-documents.svg"],
  ["publish", "🚀", "Publish", "p", "/assets/icons/candy/pipelines.svg"],
  // No vendored candy icon for Archive yet — falls back to the glyph,
  // same as History's deliberate choice below.
  ["archive", "🗄", "Archive", "v"],
  ["chat", "💬", "Chat", "t", "/assets/icons/candy/chat.svg"],
  ["settings", "⚙", "Settings", "c", "/assets/icons/candy/settings.svg"],
  ["system", "🛠", "System", "y", "/assets/icons/candy/system.svg"],
  ["logs", "📜", "Logs", "o", "/assets/icons/candy/logs.svg"],
  ["history", "🗂", "History", "h", "/assets/icons/candy/history.svg"],
];

function chrome(content) {
  const r = currentRoute();
  // Apply the user's Aeon-style top-nav reorder if any. Unknown
  // entries fall through in their default position so new releases
  // can extend TOPNAV without breaking saved order.
  let layoutOrder;
  try { layoutOrder = JSON.parse(localStorage.getItem("strivo-layout-topnav") || ""); }
  catch { layoutOrder = null; }
  const navItems = Array.isArray(layoutOrder)
    ? [
        ...layoutOrder
          .map((name) => TOPNAV.find((e) => e[0] === name))
          .filter(Boolean),
        ...TOPNAV.filter((e) => !layoutOrder.includes(e[0])),
      ]
    : TOPNAV;
  // Hide Creator Edition routes in the pure-PVR build.
  const visibleNav = CREATOR_ENABLED
    ? navItems
    : navItems.filter((e) => !CREATOR_ROUTES.has(e[0]));
  const nav = visibleNav.map(([route, glyph, label, key, iconHref]) => {
    const inner = iconHref
      ? `<img class="topnav-icon" src="${iconHref}" alt="" />`
      : `<span aria-hidden="true">${glyph}</span>`;
    return `<a class="topnav-link ${route === r ? "active" : ""}"
              href="#/${route}" data-route="${route}" data-key="${key}"
              title="${label}" aria-label="${label}">
            ${inner}
          </a>`;
  }).join("");
  return `
    <div class="chrome">
      <header class="topbar" role="banner">
        <a class="brand" href="#/library" id="brand-home" title="Home">StriVo</a>
        <span id="conn-status" class="conn-status" role="status" hidden
              title="Live updates connection">● reconnecting…</span>
        <a id="health-pill" class="health-pill" href="#/system" hidden
           role="status" title="System health — click for details"></a>
        <span id="live-pill" class="live-pill" style="display:none"
              title="Active recordings"></span>
        <span id="rec-slot-pill" class="storage-pill" style="display:none"
              title="Active recordings / concurrent cap — click to manage"
              role="status"></span>
        <span class="spacer"></span>
        <nav class="topnav" aria-label="Main navigation">${nav}</nav>
        <button id="add-channel" title="Add a channel to monitor"
                aria-label="Add channel">＋ Add</button>
        <button id="poll-now" title="Poke channel monitor (p)"
                aria-label="Trigger immediate channel poll">↻ Poll</button>
        <button id="logout" title="Logout" aria-label="Sign out">
          <img class="topnav-icon" src="/assets/icons/candy/logout.svg" alt="" />
        </button>
      </header>
      <nav class="leftrail" id="channel-list" aria-label="Channels"></nav>
      <main class="content" id="content">${content}</main>
    </div>
  `;
}

function setupChromeHandlers() {
  // Brand → home: clear any selected channel and go to the dashboard.
  document.getElementById("brand-home")?.addEventListener("click", (e) => {
    e.preventDefault();
    selectedChannelKey = null;
    if (currentRoute() === "library") render();
    else route("library");
  });
  document.getElementById("poll-now")?.addEventListener("click", async () => {
    try {
      await API.pollNow();
    } catch (e) {
      console.error(e);
    }
  });
  document.getElementById("add-channel")?.addEventListener("click", () => openAddChannelWizard());
  // Slot pill navigates to Monitor page so users can adjust the cap.
  document.getElementById("rec-slot-pill")?.addEventListener("click", () => route("schedule"));
  document.getElementById("logout")?.addEventListener("click", () => {
    // Quick confirm — one misclick on the topbar shouldn't sign you out.
    if (!confirm("Sign out? You'll need to re-enter the API key to come back.")) return;
    API.logout().catch(() => {}).then(() => route("login"));
  });
  // Health pill — amber/red when any check is degraded (roadmap item 13).
  refreshHealthPill();
  // Populate the concurrent-slot pill with the configured cap. Fire-and-forget;
  // failures silently leave maxConcurrentSlots at its current cached value so
  // a transient /settings error doesn't blank the topbar.
  API.settings().then((s) => {
    maxConcurrentSlots = (s && s.monitor_limits && s.monitor_limits.max_concurrent_recordings) || 0;
    updateLiveCount();
  }).catch(() => {});
  // Channel list lives in the left rail on every page.
  paintChannelList();
  // Per-page first-visit hint banner. No-op when this route's hint has
  // already been dismissed or no hint copy exists for the route.
  maybeMountPageHint(currentRoute());
}

// Topbar health pill: only shown when the worst check is warn/error, so a
// healthy system stays uncluttered. Links to the System page. (Item 13.)
async function refreshHealthPill() {
  const pill = document.getElementById("health-pill");
  if (!pill) return;
  try {
    const h = await API.healthChecks();
    const worst = h.status || "ok";
    if (worst === "ok") {
      pill.hidden = true;
      return;
    }
    const bad = (h.checks || []).filter((c) => c.severity !== "ok");
    pill.className = `health-pill ${worst}`;
    pill.textContent = `${worst === "error" ? "✕" : "▲"} ${bad.length} issue${bad.length === 1 ? "" : "s"}`;
    pill.title = bad.map((c) => `${c.domain}/${c.name}: ${c.message}`).join("\n");
    pill.hidden = false;
  } catch (_) {
    pill.hidden = true;
  }
}

// ── Channel list (left rail) ─────────────────────────────────────────
// Merges /channels (Twitch/YT) with Patreon creators (patreonState),
// live first + bold, then offline. Clicking selects a channel and shows
// its detail in the center (home route only).
// Rail ordering + collapse state. Platform rank keeps Twitch → YouTube →
// Patreon stable regardless of how the daemon happens to return channels.
const RAIL_PLATFORM_ORDER = { Twitch: 0, YouTube: 1, Patreon: 2 };
function byPlatformThenName(a, b) {
  const rank =
    (RAIL_PLATFORM_ORDER[a.platform] ?? 99) - (RAIL_PLATFORM_ORDER[b.platform] ?? 99);
  if (rank !== 0) return rank;
  return (a.display_name || a.name || "").localeCompare(
    b.display_name || b.name || "",
    undefined,
    { sensitivity: "base" },
  );
}

// Rail sort. Alphabetical-within-platform is the default because it makes a
// channel findable by name; the last-live orders answer the other question
// people actually ask of this rail ("who's been on recently / who's gone
// quiet"). Channels StriVo has never seen live sort last in both directions
// rather than pretending to be infinitely old or infinitely recent.
const RAIL_SORT_KEY = "strivo:rail-sort";
const RAIL_SORTS = {
  name: { label: "Name (A–Z)", cmp: byPlatformThenName },
  "live-desc": { label: "Last live (newest)", cmp: (a, b) => byLastLive(a, b, -1) },
  "live-asc": { label: "Last live (oldest)", cmp: (a, b) => byLastLive(a, b, 1) },
};
function byLastLive(a, b, dir) {
  const ta = a.is_live ? Infinity : Date.parse(a.last_live_at || "") || null;
  const tb = b.is_live ? Infinity : Date.parse(b.last_live_at || "") || null;
  if (ta === null && tb === null) return byPlatformThenName(a, b);
  if (ta === null) return 1; // never-seen-live sinks, whichever direction
  if (tb === null) return -1;
  if (ta === tb) return byPlatformThenName(a, b);
  return ta < tb ? dir : -dir;
}
function railSort() {
  try {
    const v = localStorage.getItem(RAIL_SORT_KEY);
    return RAIL_SORTS[v] ? v : "name";
  } catch (_) {
    return "name";
  }
}
function setRailSort(v) {
  try {
    localStorage.setItem(RAIL_SORT_KEY, RAIL_SORTS[v] ? v : "name");
  } catch (_) {
    /* private mode — sort just won't persist */
  }
}

const RAIL_COLLAPSE_KEY = "strivo:rail-collapsed";
function railCollapsedSet() {
  try {
    const raw = JSON.parse(localStorage.getItem(RAIL_COLLAPSE_KEY) || "[]");
    return new Set(Array.isArray(raw) ? raw : []);
  } catch (_) {
    return new Set();
  }
}
function railSectionOpen(id) {
  return !railCollapsedSet().has(id);
}
function setRailSectionOpen(id, open) {
  const set = railCollapsedSet();
  if (open) set.delete(id);
  else set.add(id);
  try {
    localStorage.setItem(RAIL_COLLAPSE_KEY, JSON.stringify([...set]));
  } catch (_) {
    /* private mode — collapse just won't persist */
  }
}

function railSortControlHtml() {
  const current = railSort();
  const opts = Object.entries(RAIL_SORTS)
    .map(
      ([v, { label }]) =>
        `<option value="${v}"${v === current ? " selected" : ""}>${label}</option>`,
    )
    .join("");
  return `<div class="ch-sort">
      <label class="ch-sort-label" for="rail-sort">Sort</label>
      <select id="rail-sort" class="ch-sort-select" data-rail-sort>${opts}</select>
    </div>`;
}

function paintChannelList() {
  const rail = document.getElementById("channel-list");
  if (!rail) return;

  const merged = [...channelCache, ...patreonState.creators];
  // De-dupe by platform:id in case a Patreon creator is also in /channels.
  const seen = new Set();
  const channels = merged.filter((c) => {
    const k = `${c.platform}:${c.id}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });

  // One ordering for both rails: platform groups in a fixed order, then
  // A-Z inside each. Offline used to be split into a header per platform,
  // which cost three headers of vertical space and made a single alphabetical
  // scan impossible. The per-row platform glyph already says which is which.
  const cmp = (RAIL_SORTS[railSort()] || RAIL_SORTS.name).cmp;
  const live = channels.filter((c) => c.is_live).sort(cmp);
  const offline = channels.filter((c) => !c.is_live).sort(cmp);
  updateLiveCount();

  const recordingChannelIds = new Set(
    recCache.filter((r) => isInProgress(r.state)).map((r) => r.channel_id),
  );

  const row = (c) => {
    const key = `${c.platform}:${c.id}`;
    const sel = key === selectedChannelKey ? "sel" : "";
    const isPatreon = c.platform === "Patreon";
    const rec = recordingChannelIds.has(c.id)
      ? '<span class="ch-rec" title="recording">●</span>'
      : "";
    const liveDot = c.is_live ? '<span class="ch-live">◉</span>' : "";
    // Live → viewer count; offline Twitch/YT → "last live: N ago" in the same
    // slot (when StriVo has observed it live at least once).
    let viewers = "";
    if (c.is_live && c.viewer_count) {
      viewers = `<span class="ch-viewers">${formatCount(c.viewer_count)}</span>`;
    } else if (!c.is_live && !isPatreon && c.last_live_at) {
      viewers = `<span class="ch-lastlive" title="last live: ${htmlEscape(lastLiveLong(c.last_live_at))}">${htmlEscape(relTime(c.last_live_at))}</span>`;
    }
    // Patreon rows are visually distinct (item 6): a pledged-tier chip
    // (stored in stream_title) and a patreon-accented platform glyph.
    const tier = isPatreon && c.stream_title
      ? `<span class="ch-tier" title="pledged tier">${htmlEscape(c.stream_title)}</span>`
      : "";
    // Filter Recordings + History by this channel when clicked. Live
    // channels link to the recording dashboard so you can spot the
    // active capture quickly; offline rows go straight to the filtered
    // Recordings page (audit B7/M2).
    const href = c.is_live
      ? "#/library"
      : `#/recordings?channel=${encodeURIComponent(c.display_name || c.name || "")}`;
    // Live rows expose a drag handle to the player stage. The id shape
    // here must match the backend's stream_id format (`PlatformKind:id`)
    // so dropping onto a tile resolves to a known stream.
    const liveStreamId = c.is_live ? `${c.platform}:${c.id}` : "";
    return `
      <a class="ch-row ${c.is_live ? "live" : ""} ${isPatreon ? "patreon" : ""} ${sel}"
         data-channel-key="${key}" data-channel-id="${c.id}"
         data-platform="${c.platform}" data-live-stream-id="${htmlEscape(liveStreamId)}" href="${href}">
        <span class="ch-plat ${c.platform.toLowerCase()}" aria-hidden="true">${platformGlyph(c.platform)}</span>
        <span class="ch-name">${htmlEscape(c.display_name || c.name)}</span>
        ${tier}${viewers}${liveDot}${rec}
      </a>`;
  };

  // Section headers are buttons so they can be collapsed; the open/closed
  // state persists per section so a rail collapsed to just LIVE stays that
  // way across repaints and reloads.
  const section = (id, title, list) =>
    list.length
      ? `<button type="button" class="ch-section-title" data-rail-section="${id}"
                 aria-expanded="${railSectionOpen(id)}" aria-controls="rail-sec-${id}">
           <span class="ch-caret" aria-hidden="true">▾</span>
           <span class="ch-section-label">${title}</span>
           <span class="ch-count">${list.length}</span>
         </button>
         <div class="ch-section-body" id="rail-sec-${id}" data-rail-body="${id}"
              ${railSectionOpen(id) ? "" : "hidden"}>${list.map(row).join("")}</div>`
      : "";

  // Preserve scroll position across repaints (the rail is rebuilt
  // wholesale, which would otherwise jump it to the top on every event).
  const prevScroll = rail.scrollTop;
  rail.innerHTML =
    channels.length === 0
      ? `<div class="ch-empty">No channels yet.<br><br>
           Connect Twitch / YouTube / Patreon and follow channels — they
           appear here automatically.<br>
           <a href="#/settings">Check Settings →</a></div>`
      : railSortControlHtml() +
        section("live", `● LIVE`, live) +
        section("offline", "OFFLINE", offline);

  rail.querySelectorAll(".ch-row").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      selectChannel(el.dataset.channelKey);
    });
  });
  const sortSel = rail.querySelector("[data-rail-sort]");
  if (sortSel) {
    sortSel.addEventListener("change", () => {
      setRailSort(sortSel.value);
      paintChannelList();
    });
  }
  rail.querySelectorAll("[data-rail-section]").forEach((el) => {
    el.addEventListener("click", () => {
      const id = el.dataset.railSection;
      const open = !railSectionOpen(id);
      setRailSectionOpen(id, open);
      el.setAttribute("aria-expanded", String(open));
      const body = rail.querySelector(`[data-rail-body="${id}"]`);
      if (body) body.hidden = !open;
    });
  });
  rail.scrollTop = prevScroll;
}

function platformGlyph(p) {
  return p === "Twitch" ? "🟣" : p === "YouTube" ? "🔴" : "◈";
}

// Seed patreonState from the daemon snapshot (/patreon) so Patreon shows
// immediately on load, instead of only after the next ~5-min poll's
// patreon-state SSE event. Idempotent; refreshed live by SSE thereafter.
async function seedPatreon() {
  try {
    const p = await API.patreon();
    patreonState.creators = p.creators || [];
    patreonState.posts = {};
    for (const post of p.posts || []) {
      (patreonState.posts[post.campaign_id] ||= []).push(post);
    }
    for (const list of Object.values(patreonState.posts)) {
      list.sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""));
    }
  } catch (_) {
    /* non-fatal — SSE still refreshes it */
  }
}

function selectChannel(key) {
  selectedChannelKey = key;
  if (currentRoute() !== "library") {
    route("library"); // hashchange triggers render()
  } else {
    render();
  }
}

// ── Login ────────────────────────────────────────────────────────────
function renderLogin(errorMsg) {
  root.removeAttribute("aria-busy");
  // "Remember me" pre-fills the API key from localStorage on a returning
  // visit. The session cookie itself already persists across reloads via
  // the server; this just spares typing after a browser restart or a
  // dropped cookie (audit M18). Stored under a distinct key per host so
  // sharing a browser across StriVo instances stays clean.
  const remembered = (() => {
    try { return localStorage.getItem("strivo:remembered-api-key") || ""; }
    catch (_) { return ""; }
  })();
  root.innerHTML = `
    <div class="login-screen">
      <form class="login-card" id="login-form">
        <h1>StriVo</h1>
        <p class="subtitle">Sign in to the web console</p>
        <label for="api-key">API Key</label>
        <input type="password" id="api-key" autocomplete="current-password"
               value="${htmlEscape(remembered)}" autofocus />
        <label class="login-remember">
          <input type="checkbox" id="api-remember" ${remembered ? "checked" : ""} />
          <span>Remember on this browser</span>
        </label>
        <button type="submit" class="primary">Sign in</button>
        ${errorMsg ? `<div class="error">${htmlEscape(errorMsg)}</div>` : ""}
        <div class="hint">
          API key lives in <code>~/.config/strivo/config.toml</code> under
          <code>[web]</code>. <br />
          Or run: <code>strivo config get web.api_key</code><br />
          <span class="login-recovery">Lost it? Stop the daemon, edit
          <code>~/.config/strivo/config.toml</code>, replace the
          <code>api_key</code> with anything random, and restart.</span>
        </div>
      </form>
    </div>
  `;
  document.getElementById("login-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const key = document.getElementById("api-key").value.trim();
    if (!key) return;
    const remember = document.getElementById("api-remember").checked;
    try {
      await API.login(key);
      try {
        if (remember) localStorage.setItem("strivo:remembered-api-key", key);
        else localStorage.removeItem("strivo:remembered-api-key");
      } catch (_) {}
      events.start(); // (re)connect the now-authorized SSE stream
      // fetchEdition() only ever ran once at script boot, unauthenticated —
      // /api/v1/settings 401'd and CREATOR_ENABLED stuck false for the rest
      // of the session, hiding every creator route (Archive included) until
      // a manual reload. Re-resolve it now that the session cookie is set.
      await fetchEdition();
      route("library");
      // The first-run tour belongs to the authenticated application
      // chrome. Starting it during the login paint blocks the sign-in
      // form with an overlay and leaves the spotlight without targets.
      setTimeout(startOnboardingTour, 600);
    } catch (err) {
      renderLogin("Invalid API key");
    }
  });
}

// ── Home: channel detail (if selected) + recordings dashboard ─────────
// First-run gate (item 20): a fresh install with no platform connected gets
// a guided setup checklist instead of an empty/half-configured dashboard.
// (Platform auth + config writes happen in the TUI/CLI, not the webui, so
// this screen reports live status and tells the user what to do.)
let firstRunDismissed = false;

function renderFirstRun(setup) {
  root.removeAttribute("aria-busy");
  const step = (done, label, detail) => `
    <div class="fr-step ${done ? "done" : "todo"}">
      <span class="fr-mark">${done ? "✓" : "○"}</span>
      <div class="fr-body">
        <div class="fr-label">${htmlEscape(label)}</div>
        <div class="fr-detail">${detail}</div>
      </div>
    </div>`;
  const plat = (name, ok) =>
    `<span class="fr-pill ${ok ? "ok" : ""}">${ok ? "✓" : "○"} ${htmlEscape(name)}</span>`;
  const anyPlatform =
    setup.twitch_configured || setup.youtube_configured || setup.patreon_configured;
  const recDir = setup.recording_dir || "(unset)";
  const chanCount = (setup.auto_record_channels || []).length;

  root.innerHTML = chrome(`
    <h1 class="page-title">Welcome to StriVo</h1>
    <p class="page-subtitle">Finish setup before the dashboard fills in.</p>
    <div class="cfg-card fr-card">
      ${step(
        anyPlatform,
        "1 · Connect a platform",
        `Authenticate Twitch / YouTube / Patreon by running <code>strivo</code>
         in a terminal (device-code login). Then re-check below.
         <div class="fr-pills">${plat("Twitch", setup.twitch_configured)}
           ${plat("YouTube", setup.youtube_configured)}
           ${plat("Patreon", setup.patreon_configured)}</div>`,
      )}
      ${step(
        !!setup.recording_dir,
        "2 · Recording directory",
        `Where captures are written: <code>${htmlEscape(recDir)}</code>.
         Change it in <code>~/.config/strivo/config.toml</code> if needed.`,
      )}
      ${step(
        chanCount > 0,
        "3 · Pick channels to record",
        `Use the <b>＋ Add</b> button (top bar) to find a channel and enable
         auto-record. ${chanCount} channel(s) configured so far.`,
      )}
      <div class="fr-actions">
        <button id="fr-recheck">↻ Re-check</button>
        <button id="fr-continue" class="primary">${anyPlatform ? "Continue to dashboard" : "Continue anyway"}</button>
      </div>
    </div>
  `);
  setupChromeHandlers();
  document.getElementById("fr-recheck")?.addEventListener("click", () => renderHome());
  document.getElementById("fr-continue")?.addEventListener("click", () => {
    firstRunDismissed = true;
    renderHome();
  });
}

async function renderHome() {
  let setup = null;
  try {
    setup = await API.settings();
  } catch (e) {
    if (e.message.includes("unauthorized")) return;
  }
  const anyPlatform =
    setup &&
    (setup.twitch_configured || setup.youtube_configured || setup.patreon_configured);
  if (setup && !anyPlatform && !firstRunDismissed) {
    renderFirstRun(setup);
    return;
  }
  // Refresh the channel + recordings caches that feed the left rail and
  // the dashboard. Both are cheap snapshots.
  //
  // Use Promise.allSettled so a transient failure on one side (e.g. the
  // daemon socket bouncing) doesn't drop the OTHER side's data into the
  // empty-rail state. Previously Promise.all rejected atomically and we
  // caught at the outer try/catch, leaving both caches stale — visually
  // that surfaced as "rail vanished" because the unauth check at the top
  // already returned for genuine 401s.
  const [chRes, recRes] = await Promise.allSettled([API.channels(), API.recordings()]);
  if (chRes.status === "fulfilled") {
    channelCache = chRes.value.channels || [];
  } else if (chRes.reason && chRes.reason.message && chRes.reason.message.includes("unauthorized")) {
    return;
  }
  if (recRes.status === "fulfilled") {
    recCache = recRes.value.recordings || [];
    dashRecordings = recCache;
    seedVodDownloadStateFromRecCache();
  } else if (recRes.reason && recRes.reason.message && recRes.reason.message.includes("unauthorized")) {
    return;
  }
  await seedPatreon();
  try {
    dashSchedule = (await API.schedule()).schedule || [];
  } catch (_) {
    dashSchedule = [];
  }
  // A14: prune bulkStatus entries whose channel is no longer in the
  // current channelCache. Without this the bulkStatus map grew across
  // weeks of uptime as channels were added + removed.
  if (Object.keys(bulkStatus).length) {
    const liveIds = new Set(channelCache.map((c) => c.id));
    for (const id of Object.keys(bulkStatus)) {
      if (!liveIds.has(id)) delete bulkStatus[id];
    }
  }
  root.removeAttribute("aria-busy");

  const selected = selectedChannelKey
    ? [...channelCache, ...patreonState.creators].find(
        (c) => `${c.platform}:${c.id}` === selectedChannelKey,
      )
    : null;

  // The recordings dashboard (In progress / Recent / Upcoming) lives only on
  // the home view; opening a channel shows just its detail.
  const center = selected
    ? channelDetailHtml(selected)
    : `<div id="dash">${recordingsDashboardHtml(false)}</div>`;

  root.innerHTML = chrome(center);
  setupChromeHandlers();

  if (selected) {
    wireChannelDetail(selected);
    loadChannelDetailData(selected);
  }
  wireDashboard();
}

// Repaint ONLY the recordings dashboard subtree (#dash) — never the chrome,
// left rail, or channel-detail iframe. Driven by high-frequency recording
// events so they don't reload the live preview or reset rail scroll.
function paintDashboard() {
  const el = document.getElementById("dash");
  if (!el) return;
  el.innerHTML = recordingsDashboardHtml(!!selectedChannelKey);
  wireDashboard();
}

// ── Home dashboard (Jellyfin-style horizontal carousels) ─────────────
//
// Rows: Live Now → In Progress → Recently Finished → Upcoming.
// Each row is a horizontal-scroll strip. Recently Finished pills are
// click-to-play (per user request); Live Now cards deep-link to the
// /watch route focused on that stream.
function recordingsDashboardHtml(compact) {
  const inProgress = dashRecordings.filter((r) => isInProgress(r.state));
  // Sort before slicing. This previously took whatever order the daemon
  // returned, so "Recent" was only accidentally chronological and the slice
  // could drop newer rows in favour of older ones.
  const recent = dashRecordings
    .filter((r) => !isInProgress(r.state))
    .sort((a, b) => recordingTime(b) - recordingTime(a))
    .slice(0, compact ? 12 : 24);
  const upcoming = [...dashSchedule]
    .filter((s) => s.next_fire)
    .sort((a, b) => new Date(a.next_fire) - new Date(b.next_fire));
  const liveChannels = (channelCache || []).filter((c) => c.is_live);

  const schedPillEl = (s) => `
    <div class="media-pill">
      <div class="mp-thumb"></div>
      <div class="mp-info">
        <div class="mp-title">${htmlEscape(s.channel)}</div>
        <div class="mp-sub">${htmlEscape(new Date(s.next_fire).toLocaleString())}${s.duration ? ` · ${htmlEscape(s.duration)}` : ""}</div>
      </div>
      <div class="mp-meta"><span class="mp-badge">scheduled</span></div>
    </div>`;

  // Live-now card: thumbnail + channel name + viewer count + LIVE
  // chip. Whole card is a hash link to /watch?focus=<id>.
  const liveCardEl = (c) => {
    const thumb = liveThumbUrl(c);
    const focus = `${c.platform}:${c.id}`;
    const href = `#/watch?mode=focus&focus=${encodeURIComponent(focus)}`;
    const viewers = c.viewer_count != null ? formatCount(c.viewer_count) : "";
    return `
      <a class="live-card" href="${href}" data-live-focus="${htmlEscape(focus)}"
         title="Open ${htmlEscape(c.display_name || c.name)} in the multi-stream viewer">
        <div class="live-card-thumb">${thumb ? `<img loading="lazy" src="${htmlEscape(thumb)}" alt=""/>` : ""}<span class="live-card-badge">LIVE</span></div>
        <div class="live-card-meta">
          <span class="live-card-name">${htmlEscape(c.display_name || c.name)}</span>
          <span class="live-card-sub pg-cap-hint">${htmlEscape(c.platform)}${viewers ? ` · ${viewers}` : ""}</span>
        </div>
      </a>`;
  };

  const rowEl = (title, count, html, empty, klass = "") => `
    <section class="dash-row${klass ? " " + klass : ""}">
      <h2 class="dash-row-title">${title}${count != null ? ` <span class="dash-count">${count}</span>` : ""}</h2>
      <div class="dash-scroll">${html || `<div class="empty sm">${empty}</div>`}</div>
    </section>`;

  const heading = compact ? "" : `<h1 class="page-title">Home</h1>`;
  // Live Now hidden when zero live (avoids "No channels live" noise on
  // dashboards where the rail's offline-only state already conveys
  // that). Same for Upcoming when no schedule.
  const liveRow = liveChannels.length
    ? rowEl("Live Now", liveChannels.length, liveChannels.map(liveCardEl).join(""), "", "live-now-row")
    : "";
  const upcomingRow = upcoming.length
    ? rowEl("Upcoming", upcoming.length, upcoming.map(schedPillEl).join(""), "", "")
    : "";
  return `${heading}
    ${liveRow}
    ${rowEl("In progress", inProgress.length, inProgress.map(recordingPillHtml).join(""), "Nothing recording")}
    ${rowEl("Recent", null, recent.map(recordingPillHtml).join(""), "No recordings yet — start one from the rail.")}
    ${upcomingRow}`;
}

// Shared recording media-pill (used by the home dashboard + History): cover
// thumbnail + title + channel·date + state/size, with a Stop on active rows.
function recordingPillHtml(j) {
  const when = j.started_at ? new Date(j.started_at).toLocaleString() : "—";
  const stop = isInProgress(j.state)
    ? `<button class="danger sm" data-action="stop" data-job-id="${htmlEscape(j.id)}">Stop</button>`
    : "";
  // FILE MISSING overlay on the thumbnail mirrors the Recordings page
  // treatment so the Library dashboard doesn't quietly hide broken
  // rows (audit U2).
  const missingOverlay = j.file_exists === false
    ? '<span class="mp-missing">FILE MISSING</span>'
    : "";
  // Twitch live-pull + auto-VOD-backfill produces two rows per
  // broadcast — surface a small chip when the source is the
  // backfill path so the user can tell them apart at a glance
  // (audit B5). source_url is set when the recording was created
  // via DownloadVod (the backfill path).
  const sourceBadge = j.source_url
    ? '<span class="mp-source" title="From Twitch/YouTube VOD backfill">VOD</span>'
    : "";
  // Finished recordings with a file → click-to-play; in-progress &
  // file-missing rows stay inert (they don't have a playable artefact).
  const playable = !isInProgress(j.state) && j.file_exists !== false;
  const playAttrs = playable
    ? ` data-action="play" data-job-id="${htmlEscape(j.id)}" role="button" tabindex="0"`
    : "";
  // "Finished" is the state of nearly every row here, so rendering a pill
  // for it spends a column of every card restating the default. Only states
  // that actually need attention get a pill; the rest read as unremarkable,
  // which is the point.
  const state = recordingDisplayState(j);
  const statePill = isNoteworthyState(state) ? renderStatePill(state) : "";
  const title = htmlEscape(niceTitle(j.stream_title) || j.channel_name || "(recording)");
  return `
    <div class="media-pill mp-card${j.file_exists === false ? " mp-broken" : ""}${playable ? " mp-clickable" : ""}"${playAttrs}>
      <div class="mp-title" title="${title}">${title} ${sourceBadge}</div>
      <div class="mp-thumb">${missingOverlay}<img class="mp-thumb-img" loading="lazy" alt=""
        src="/api/v1/recordings/${encodeURIComponent(j.id)}/thumb" onerror="this.remove()"></div>
      <div class="mp-foot">
        <span class="mp-channel">${htmlEscape(j.channel_name || "")}</span>
        <span class="mp-when" title="${htmlEscape(when)}">${htmlEscape(shortWhen(j.started_at))}</span>
        <span class="mp-spacer"></span>
        ${statePill}
        <span class="mp-size">${formatBytes(j.bytes_written || 0)}</span>
        ${stop}
      </div>
    </div>`;
}

/// Timestamp a recording sorts by. started_at is the only time the API
/// exposes for every row, so it is the sort key; guard against nulls so a
/// malformed row sinks rather than poisoning the comparator with NaN.
function recordingTime(r) {
  const t = r && r.started_at ? Date.parse(r.started_at) : NaN;
  return Number.isFinite(t) ? t : 0;
}

/// States worth spending pixels on. "Finished" is the expected outcome and
/// is conveyed well enough by the row simply being playable, so it earns no
/// pill. Everything else — failures, missing files, in-flight work — does.
/// Note this takes the object recordingDisplayState() returns, not a string.
function isNoteworthyState(state) {
  return (state && state.className) !== "finished";
}

/// Compact timestamp for dense rows: a time for today, "Wed 14:05" within
/// the week, "30 Jul" beyond it. The full locale string stays in the
/// tooltip. Seconds are never useful here and cost ~4 characters a row.
function shortWhen(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const hhmm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (sameDay) return hhmm;
  const days = (now - d) / 86400000;
  if (days < 7) return `${d.toLocaleDateString([], { weekday: "short" })} ${hhmm}`;
  return d.toLocaleDateString([], { day: "numeric", month: "short" });
}

function wireDashboard() {
  // Click-to-play on finished recording pills. Routes to the Player
  // tab with this recording loaded as the single tile — no inline
  // modal. fresh=1 forces a single-slot reset so a stale multi-tile
  // layout doesn't eat the click.
  document.querySelectorAll('.media-pill[data-action="play"]').forEach((pill) => {
    const open = () => {
      const id = pill.dataset.jobId;
      if (!id) return;
      window.location.hash = `#/watch?recording=${encodeURIComponent(id)}&fresh=1`;
    };
    pill.addEventListener("click", (e) => {
      if (e.target.closest("button, a, input")) return;
      open();
    });
    pill.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); }
    });
  });
  document.querySelectorAll('[data-action="stop"]').forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!(await confirmDialog("Stop this recording?", { ok: "Stop", danger: true })))
        return;
      await withBusy(btn, "Stopping…", async () => {
        await API.stopRecording(btn.dataset.jobId);
        Toast.success("Recording stopped");
        setTimeout(() => render().catch(() => {}), 500);
      }).catch((e) => Toast.error(`Stop failed: ${e.message}`));
    });
  });
}

// ── Channel detail (center) ──────────────────────────────────────────
function channelDetailHtml(c) {
  const key = `${c.platform}:${c.id}`;
  const isPatreon = c.platform === "Patreon";
  const liveBadge = c.is_live
    ? '<span class="status live">LIVE</span>'
    : '<span class="status">offline</span>';
  const actions = `
    <div class="actions">
      ${c.is_live ? `
        <button class="primary" data-action="record" data-channel-id="${c.id}"
                data-channel-name="${htmlEscape(c.name)}"
                data-display-name="${htmlEscape(c.display_name || c.name)}"
                data-platform="${c.platform}"
                data-thumbnail="${htmlEscape(c.thumbnail_url || "")}"
                data-stream-title="${htmlEscape(c.stream_title || "")}">● Record</button>
        <button data-action="record" data-from-start="true" data-channel-id="${c.id}"
                data-channel-name="${htmlEscape(c.name)}"
                data-display-name="${htmlEscape(c.display_name || c.name)}"
                data-platform="${c.platform}"
                data-thumbnail="${htmlEscape(c.thumbnail_url || "")}"
                data-stream-title="${htmlEscape(c.stream_title || "")}">● From start</button>
      ` : ""}
      ${!isPatreon ? `
        <button data-action="auto-record" data-channel-key="${key}"
                data-enabled="${!c.auto_record}">
          ${c.auto_record ? "Disable auto" : "Enable auto"}
        </button>
        ${bulkButton(c)}
        ${c.platform === "YouTube" ? `
          <button data-action="bulk-playlist" data-channel-id="${c.id}"
                  data-channel-name="${htmlEscape(c.display_name || c.name)}">⛁ Playlist…</button>` : ""}
        <button data-action="block-channel" data-channel-id="${c.id}"
                data-platform="${c.platform}"
                data-channel-name="${htmlEscape(c.display_name || c.name)}"
                title="Stop auto-grabbing this channel">⊘ Block</button>
      ` : ""}
    </div>`;

  // Section placeholders filled by loadChannelDetailData (async).
  let sections;
  if (isPatreon) {
    sections = `<div id="cd-posts" class="cd-section"></div>`;
  } else if (c.platform === "YouTube") {
    sections = `
      <div id="cd-playlists" class="cd-section"></div>
      <div id="cd-streams" class="cd-section"></div>
      <div id="cd-uploads" class="cd-section"></div>`;
  } else {
    sections = `<div id="cd-streams" class="cd-section"></div>`;
  }

  return `
    <div class="channel-detail">
      <div class="cd-header">
        <span class="platform-icon ${c.platform.toLowerCase()}">${c.platform}</span>
        <h1 class="cd-name">${htmlEscape(c.display_name || c.name)}</h1>
        ${liveBadge}
        ${c.viewer_count ? `<span class="cd-viewers">${formatCount(c.viewer_count)} viewers</span>` : ""}
        <button class="cd-close" data-action="cd-close" title="Close">×</button>
      </div>
      ${c.stream_title ? `<div class="stream-title">${htmlEscape(c.stream_title)}</div>` : ""}
      ${livePreviewHtml(c)}
      ${actions}
      ${sections}
    </div>`;
}

// Live preview when a live channel is opened (items 4 + 23). Progressive
// model: show a refreshing thumbnail poster first, upgrade to the platform's
// embed player on click (tap-to-play — avoids auto-spinning a player for every
// open and works on mobile). Patreon has no live concept (thumbnail-only).
/// Derive Twitch's `parent=` value from a host string.
///
/// Twitch accepts a HOSTNAME ONLY — a scheme or port produces "embed
/// misconfigured" / "refused to connect". It also rejects bare IPv4, so a
/// LAN address is rewritten to the matching `<ip-dashed>.nip.io`, which
/// resolves to the same IP through wildcard DNS.
///
/// This mirrors `strivo_multistream::embed_url`'s host handling
/// (crates/multistream/src/lib.rs) so every embed surface derives the same
/// parent. Three call sites used to do this independently and disagree:
/// the wall went through the Rust builder, while the channel-detail preview
/// used a bare `location.hostname` (no port stripping, no nip.io) and would
/// break for anyone reaching strivo over a LAN IP.
function embedParentHost(host) {
  const raw = host || location.host || "127.0.0.1";
  const bare = raw
    .replace(/^https?:\/\//, "")
    .split("/")[0]
    .split(":")[0];
  return /^\d+\.\d+\.\d+\.\d+$/.test(bare)
    ? `${bare.replace(/\./g, "-")}.nip.io`
    : bare;
}

/// Single builder for live embed URLs. `muted`/`autoplay` are omitted from
/// the URL entirely when left undefined, so callers that manage playback
/// through a player API do not bake a conflicting state into the src.
function buildEmbedUrl(platform, embedKey, opts = {}) {
  const { host, muted, autoplay } = opts;
  const key = encodeURIComponent(embedKey || "");
  if (platform === "Twitch") {
    let u = `https://player.twitch.tv/?channel=${key}` +
      `&parent=${encodeURIComponent(embedParentHost(host))}`;
    if (muted != null) u += `&muted=${!!muted}`;
    if (autoplay != null) u += `&autoplay=${!!autoplay}`;
    return u;
  }
  if (platform === "YouTube") {
    let u = `https://www.youtube.com/embed/live_stream?channel=${key}`;
    if (muted != null) u += `&mute=${muted ? 1 : 0}`;
    if (autoplay != null) u += `&autoplay=${autoplay ? 1 : 0}`;
    return u + "&playsinline=1";
  }
  return null;
}

function liveEmbedSrc(c) {
  const key = c.platform === "YouTube" ? c.id : c.name;
  return buildEmbedUrl(c.platform, key, { muted: true, autoplay: true });
}

// Substitute Twitch's {width}x{height} placeholders and cache-bust so the
// poster refreshes to a near-live frame.
function liveThumbUrl(c) {
  if (!c.thumbnail_url) return null;
  const sized = c.thumbnail_url
    .replace("{width}", "440")
    .replace("{height}", "248");
  return `${sized}${sized.includes("?") ? "&" : "?"}t=${Date.now()}`;
}

function livePreviewHtml(c) {
  if (!c.is_live) return "";
  const src = liveEmbedSrc(c);
  const thumb = liveThumbUrl(c);
  // Stream id matches the backend's `{Platform:?}:{id}` shape so a
  // ▶ click on this poster routes to the Player and pre-fills the
  // single slot with this channel.
  const focus = `${c.platform}:${c.id}`;
  // No thumbnail but we have an embed → mount the player directly.
  // No `loading="lazy"`: this iframe is the live player. Chromium
  // viewport-throttles lazy iframes during the top-layer transition that
  // fullscreen triggers on cross-origin embeds, which stalls Twitch playback.
  if (!thumb && src) {
    return `<div class="cd-preview" data-embed-src="${htmlEscape(src)}" data-focus="${htmlEscape(focus)}">
      <iframe src="${htmlEscape(src)}" title="Live preview"
              allow="autoplay; fullscreen; picture-in-picture; encrypted-media; clipboard-write" allowfullscreen></iframe>
    </div>`;
  }
  if (!thumb) return "";
  // Poster + (if embeddable) a play overlay that routes to the Player
  // tab with this channel pre-loaded — no in-place upgrade. The user
  // already 'hit play' so we don't make them pick the stream again.
  return `<div class="cd-preview poster" ${src ? `data-embed-src="${htmlEscape(src)}" data-focus="${htmlEscape(focus)}"` : ""}>
    <img id="cd-poster-img" src="${htmlEscape(thumb)}" alt="Live thumbnail" />
    ${src ? `<button class="cd-play" id="cd-play" aria-label="Open in Player">▶</button>` : ""}
  </div>`;
}

let cdPosterTimer = null;
function teardownLivePreview() {
  if (cdPosterTimer) {
    clearInterval(cdPosterTimer);
    cdPosterTimer = null;
  }
}

// Bug fix: cross-origin embed iframes (Twitch / YouTube) freeze when
// fullscreened from inside an .cd-preview parent that has
// overflow:hidden + aspect-ratio. The iframe can't match the parent's
// :fullscreen pseudo (different document scope), so we toggle a class
// on the parent via the document's fullscreenchange event and use
// .is-fullscreen + :has(iframe:fullscreen) in CSS to drop the clip.
function attachFullscreenBugfix(previewEl) {
  if (!previewEl || previewEl.dataset.fsBound === "1") return;
  previewEl.dataset.fsBound = "1";
  const onChange = () => {
    const fsEl = document.fullscreenElement || document.webkitFullscreenElement;
    const ours = !!fsEl && previewEl.contains(fsEl);
    previewEl.classList.toggle("is-fullscreen", ours);
  };
  document.addEventListener("fullscreenchange", onChange);
  document.addEventListener("webkitfullscreenchange", onChange);
  // W6: ESC exits a fullscreened embed cleanly. Browsers handle the
  // native fullscreen ESC themselves, but when the user has NOT
  // fullscreened we let ESC back out of the embed to the channel
  // detail's poster mode (parent toggles handled in wireChannelDetail).
  previewEl.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (document.fullscreenElement) document.exitFullscreen?.();
  });
}

function wireChannelDetail() {
  // Clear any preview refresh timer from a previously-open detail (item 23).
  teardownLivePreview();
  document.querySelector('[data-action="cd-close"]')?.addEventListener("click", () => {
    teardownLivePreview();
    selectedChannelKey = null;
    render();
  });

  // Live preview: refresh the poster thumbnail every 30s, and upgrade to the
  // embed player on click (tap-to-play). Tears down when detail re-renders.
  // Cover BOTH the poster-mode preview and the always-embedded preview
  // — the freeze bug affects either form once the iframe is mounted.
  document.querySelectorAll(".cd-preview").forEach(attachFullscreenBugfix);
  const poster = document.querySelector(".cd-preview.poster");
  if (poster) {
    const img = poster.querySelector("#cd-poster-img");
    if (img) {
      const base = img.src.split(/[?&]t=/)[0];
      cdPosterTimer = setInterval(() => {
        if (document.hidden) return;
        // Only refresh while still on-screen (cheap visibility guard).
        if (!document.body.contains(img)) {
          teardownLivePreview();
          return;
        }
        img.src = `${base}${base.includes("?") ? "&" : "?"}t=${Date.now()}`;
      }, 30000);
    }
    const playBtn = poster.querySelector("#cd-play");
    const focus = poster.dataset.focus;
    if (playBtn && focus) {
      // ▶ on a channel poster routes straight to the Player tab with
      // this channel as the single-slot stream. User already clicked
      // play; don't open the 'pick a stream' picker (audit follow-up).
      playBtn.addEventListener("click", () => {
        teardownLivePreview();
        window.location.hash = `#/watch?focus=${encodeURIComponent(focus)}&fresh=1`;
      });
    }
  }
  document.querySelectorAll("[data-action=record]").forEach((btn) =>
    btn.addEventListener("click", () => startRecordingFromCard(btn.dataset)),
  );
  document.querySelectorAll("[data-action=auto-record]").forEach((btn) =>
    btn.addEventListener("click", () => toggleAutoRecord(btn.dataset)),
  );
  document.querySelectorAll("[data-action=bulk]").forEach((btn) =>
    btn.addEventListener("click", () => toggleBulk(btn.dataset)),
  );
  document.querySelectorAll("[data-action=bulk-playlist]").forEach((btn) =>
    btn.addEventListener("click", () => openPlaylistPicker(btn.dataset)),
  );
  document.querySelectorAll("[data-action=block-channel]").forEach((btn) =>
    btn.addEventListener("click", async () => {
      const d = btn.dataset;
      if (
        !(await confirmDialog(
          `Block ${d.channelName}? StriVo will stop auto-grabbing this channel's VODs.`,
          { ok: "Block", danger: true },
        ))
      )
        return;
      try {
        await API.blockAdd({ platform: d.platform, channel_id: d.channelId });
        Toast.success(`Blocked ${d.channelName}`);
      } catch (e) {
        Toast.error(`Block failed: ${e.message}`);
      }
    }),
  );
}

// Fetch + render the per-channel VOD lists. Patreon uses cached posts;
// YouTube/Twitch request VODs over IPC (result arrives via SSE) and also
// request playlists for YouTube.
function loadChannelDetailData(c) {
  if (c.platform === "Patreon") {
    renderPatreonPosts(c);
    return;
  }
  // Render from cache immediately if we have it, then (re)request.
  paintChannelVods(c.id, c.platform);
  API.requestChannelVods(c.id, c.platform).catch(() => {});
  if (c.platform === "YouTube") {
    API.requestPlaylists(c.id).catch(() => {});
  }
  // Don't hang on "Loading…" forever — if the channel-vods SSE answer
  // hasn't arrived in 15s (slow/failed platform fetch), show an error
  // state for whichever sections are still loading.
  const id = c.id;
  setTimeout(() => {
    if (!channelVods[id] && `${c.platform}:${id}` === selectedChannelKey) {
      for (const sid of ["cd-streams", "cd-uploads"]) {
        const el = document.getElementById(sid);
        if (el && el.textContent.includes("Loading")) {
          const title = sid === "cd-streams"
            ? "Past Broadcasts"
            : "Recent uploads";
          el.innerHTML = `<h2 class="cd-section-title">${title}</h2>` +
            `<div class="empty sm">Still loading VODs from the platform — this can take up to 15 seconds the first time. <a href="#" data-action="cd-retry">Retry now</a></div>`;
        }
      }
      document.querySelector('[data-action="cd-retry"]')?.addEventListener("click", (e) => {
        e.preventDefault();
        loadChannelDetailData(c);
      });
    }
  }, 15000);
}

function paintChannelVods(channelId, platform) {
  const vods = channelVods[channelId];
  const streamsEl = document.getElementById("cd-streams");
  const uploadsEl = document.getElementById("cd-uploads");
  // Look up channel context once so each VOD pill can carry the
  // channel_name + platform the download route needs. A9: also
  // search patreonState.creators so Patreon channels' VOD pills
  // resolve their display name + platform tag instead of falling
  // back to "".
  const channel = channelCache.find((c) => c.id === channelId)
    || (patreonState.creators || []).find((c) => c.id === channelId);
  const ctx = {
    channelName: (channel && (channel.display_name || channel.name)) || "",
    platform: platform || (channel && channel.platform) || "",
  };
  if (!vods) {
    if (streamsEl) streamsEl.innerHTML = vodSectionHtml("Past Broadcasts", null, ctx);
    if (uploadsEl) uploadsEl.innerHTML = vodSectionHtml("Recent uploads", null, ctx);
    return;
  }
  const streams = vods.filter((v) => v.kind === "LiveBroadcast");
  const uploads = vods.filter((v) => v.kind !== "LiveBroadcast");
  if (streamsEl) {
    streamsEl.innerHTML = vodSectionHtml("Past Broadcasts", streams, ctx);
  }
  if (uploadsEl) uploadsEl.innerHTML = vodSectionHtml("Recent uploads", uploads, ctx);
  wireVodDownloadButtons();
}

// Click handler for [data-action=vod-download] buttons inside the media-list
// pills. Optimistically flips to "downloading"; the SSE RecordingFinished
// handler flips to "downloaded" when a matching recording completes.
function wireVodDownloadButtons() {
  document.querySelectorAll("[data-action=vod-download]").forEach((btn) => {
    if (btn.dataset.wired === "1") return;
    btn.dataset.wired = "1";
    btn.addEventListener("click", async (e) => {
      e.preventDefault();
      e.stopPropagation();
      const url = btn.dataset.url;
      const channel_name = btn.dataset.channel;
      const platform = btn.dataset.platform;
      const post_title = btn.dataset.title || null;
      if (!url || vodDownloadState[url] === "downloading" || vodDownloadState[url] === "downloaded") {
        return;
      }
      vodDownloadState[url] = "downloading";
      setVodButtonState(btn, "downloading");
      try {
        // `data-via=patreon` routes through PatreonPull (its IPC arm
        // builds the Patreon-shaped output path + threads the patron
        // cookies); everything else lands on the generic DownloadVod
        // path. Both produce a RecordingJob with `source_url == url`,
        // so the state map + progress bar pipeline are identical.
        if (btn.dataset.via === "patreon") {
          await API.patreonPull({
            embed_url: url,
            creator_name: channel_name,
            post_title: post_title || "",
          });
        } else {
          await API.vodDownload({ url, channel_name, platform, post_title });
        }
        Toast.success(`Downloading: ${post_title || url}`);
        // The RecordingStarted SSE that follows will land in recCache with
        // source_url == this url; seedVodDownloadStateFromRecCache() then
        // confirms our optimistic state. When the recording reaches
        // Finished, the same path flips the pill to Downloaded by exact
        // source_url match — no FIFO guess.
      } catch (err) {
        // Roll back to idle so the user can retry.
        delete vodDownloadState[url];
        setVodButtonState(btn, "idle");
        Toast.error(`Download failed: ${err.message}`);
      }
    });
  });
}

// Walk recCache and reflect each recording whose source_url points at a VOD
// into vodDownloadState. Called whenever recCache is refreshed so the
// channel-detail view (and a fresh page reload) shows correct button state
// without any FIFO guess.
function seedVodDownloadStateFromRecCache() {
  for (const r of recCache) {
    if (!r.source_url) continue;
    if (r.state === "Finished") {
      vodDownloadState[r.source_url] = "downloaded";
    } else if (isInProgress(r.state)) {
      // Don't downgrade a "downloaded" entry if a stale in-progress row
      // sneaks in (rare, but be safe).
      if (vodDownloadState[r.source_url] !== "downloaded") {
        vodDownloadState[r.source_url] = "downloading";
      }
    }
  }
}

function setVodButtonState(btn, state) {
  btn.classList.remove("vod-dl-idle", "vod-dl-downloading", "vod-dl-downloaded");
  btn.classList.add(`vod-dl-${state}`);
  btn.disabled = state !== "idle";
  if (state === "downloading") {
    // Try to seed initial bar from any cached progress on the matching job.
    const url = btn.dataset.url;
    const job = recCache.find((r) => r.source_url === url);
    btn.innerHTML = vodProgressHtml(
      job && job.download_pct,
      job && job.download_eta_secs,
      job && job.download_rate_bps,
    );
  } else {
    btn.textContent = state === "downloaded" ? "Downloaded" : "Download";
  }
}

// Inner HTML for the in-flight download widget: gradient-filled bar +
// "NN% · Xm Ys left · R MB/s" label. Bar gradient runs amber → green so the
// rightmost fill colour shifts greener as the pull completes.
function vodProgressHtml(pct, etaSecs, rateBps) {
  const p = Math.max(0, Math.min(100, Math.round(pct == null ? 0 : pct)));
  const eta = etaSecs == null ? "" : fmtEta(etaSecs);
  const rate = rateBps == null ? "" : `${formatBytes(rateBps)}/s`;
  const meta = [eta && `${eta} left`, rate].filter(Boolean).join(" · ");
  return `
    <span class="vod-dl-bar"><span class="vod-dl-fill" style="width:${p}%"></span></span>
    <span class="vod-dl-label">${p}%${meta ? " · " + meta : ""}</span>
  `;
}

function fmtEta(secs) {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return r ? `${m}m ${r}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const mm = m % 60;
  return mm ? `${h}h ${mm}m` : `${h}h`;
}

// Surgical DOM patch: find every visible VOD pill bound to this job's
// source_url and refresh its progress widget. Skips pills that have
// transitioned to "downloaded" (which the seed function will finalize).
function updateVodProgressDom(job) {
  if (!job || !job.source_url) return;
  if (vodDownloadState[job.source_url] !== "downloading") return;
  const sel = `[data-action=vod-download][data-url="${CSS.htmlEscape(job.source_url)}"]`;
  document.querySelectorAll(sel).forEach((btn) => {
    btn.innerHTML = vodProgressHtml(
      job.download_pct,
      job.download_eta_secs,
      job.download_rate_bps,
    );
  });
}

// Resolve a VOD/stream thumbnail URL, substituting Twitch's templated
// dimension placeholders ({width}/%{width}). VOD thumbnails are static.
function vodThumb(url) {
  if (!url) return null;
  return url
    .replace(/%?\{width\}/g, "440")
    .replace(/%?\{height\}/g, "248");
}
// Compact duration from a serde std::time::Duration ({secs, nanos}) or number.
function fmtDur(d) {
  const s = typeof d === "number" ? d : d && d.secs;
  if (!s || s <= 0) return "";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
}

function vodSectionHtml(title, vods, ctx) {
  // Past Broadcasts gets the larger, centered treatment. Uploads keep the
  // smaller original style.
  const isPast = title === "Past Broadcasts";
  const titleCls = isPast ? "cd-section-title past-broadcasts" : "cd-section-title";
  if (vods === null) {
    return `<h2 class="${titleCls}">${title}</h2><div class="empty sm">Loading…</div>`;
  }
  if (vods.length === 0) {
    return `<h2 class="${titleCls}">${title}</h2><div class="empty sm">None</div>`;
  }
  const channelName = (ctx && ctx.channelName) || "";
  const platform = (ctx && ctx.platform) || "";
  // Jellyseerr/*arr-style horizontal media pills: thumbnail + rich info block,
  // with a sibling download button. The link wraps thumb+info; the button
  // sits next to it so we don't nest interactive elements.
  const rows = vods
    .map((v) => {
      const href = /^https?:\/\//i.test(v.url || "") ? htmlEscape(v.url) : "#";
      const thumb = vodThumb(v.thumbnail_url);
      const date = (v.published_at || "").slice(0, 10);
      const dur = fmtDur(v.duration);
      const live = v.kind === "Live" || v.kind === "live";
      const meta = [date, dur].filter(Boolean).map(htmlEscape).join(" · ");
      const downloadable = !!(v.url && channelName && platform);
      const state = vodDownloadState[v.url] || "idle";
      // For the downloading state, embed a live progress widget instead of
      // plain text. Seed pct/eta/rate from any matching cached job so a
      // re-render between SSE ticks doesn't reset the bar to 0%.
      let inner;
      if (state === "downloading") {
        const job = recCache.find((r) => r.source_url === v.url);
        inner = vodProgressHtml(
          job && job.download_pct,
          job && job.download_eta_secs,
          job && job.download_rate_bps,
        );
      } else if (state === "downloaded") {
        inner = "Downloaded";
      } else {
        inner = "Download";
      }
      const btn = downloadable
        ? `<button class="vod-dl vod-dl-${state}" data-action="vod-download"
              data-url="${htmlEscape(v.url)}"
              data-channel="${htmlEscape(channelName)}"
              data-platform="${htmlEscape(platform)}"
              data-title="${htmlEscape(v.title || "")}"
              ${state !== "idle" ? "disabled" : ""}>${inner}</button>`
        : "";
      return `
    <div class="media-pill">
      <a class="mp-link" href="${href}" target="_blank" rel="noopener">
        <div class="mp-thumb">${thumb ? `<img class="mp-thumb-img" loading="lazy" alt="" src="${htmlEscape(thumb)}" onerror="this.remove()">` : ""}</div>
        <div class="mp-info">
          <div class="mp-title">${htmlEscape(niceTitle(v.title))}</div>
          <div class="mp-sub">${meta}</div>
        </div>
        <div class="mp-meta">${live ? '<span class="mp-badge live">LIVE VOD</span>' : '<span class="mp-badge">Upload</span>'}</div>
      </a>
      ${btn}
    </div>`;
    })
    .join("");
  return `<h2 class="${titleCls}">${title}</h2>
    <div class="media-list">${rows}</div>`;
}

// Patreon channel detail: render cached posts with a pull action.
function renderPatreonPosts(c) {
  const el = document.getElementById("cd-posts");
  if (!el) return;
  const posts = patreonState.posts[c.id] || [];
  const channelName = c.display_name || c.name;
  // Each post pill carries the same `.vod-dl` button the past-broadcasts
  // list uses; state is keyed by embed_url (== source_url on the resulting
  // RecordingJob), so seedVodDownloadStateFromRecCache surfaces in-flight /
  // completed pulls across navigation just like past broadcasts.
  const rows = posts.length
    ? posts
        .map((p) => {
          const thumb = p.thumbnail_url
            ? `<img class="mp-thumb-img" loading="lazy" alt="" src="${htmlEscape(p.thumbnail_url)}" onerror="this.remove()">`
            : "";
          const url = p.embed_url || "";
          const state = vodDownloadState[url] || "idle";
          const cachedJob = recCache.find((r) => r.source_url === url);
          const inner = state === "downloading"
            ? vodProgressHtml(
                cachedJob && cachedJob.download_pct,
                cachedJob && cachedJob.download_eta_secs,
                cachedJob && cachedJob.download_rate_bps,
              )
            : state === "downloaded" ? "Downloaded" : "Download";
          const btn = url
            ? `<button class="vod-dl vod-dl-${state}" data-action="vod-download"
                  data-via="patreon"
                  data-url="${htmlEscape(url)}"
                  data-channel="${htmlEscape(channelName)}"
                  data-platform="Patreon"
                  data-title="${htmlEscape(p.title)}"
                  ${state !== "idle" ? "disabled" : ""}>${inner}</button>`
            : "";
          return `
      <div class="media-pill">
        <div class="mp-link" style="cursor: default;">
          <div class="mp-thumb">${thumb}</div>
          <div class="mp-info">
            <div class="mp-title">${htmlEscape(p.title)}</div>
            <div class="mp-sub">${htmlEscape((p.published_at || "").slice(0, 10))}</div>
          </div>
          <div class="mp-meta"></div>
        </div>
        ${btn}
      </div>`;
        })
        .join("")
    : '<div class="empty sm">No video posts.</div>';
  el.innerHTML = `<h2 class="cd-section-title">Posts</h2><div class="media-list">${rows}</div>`;
  wireVodDownloadButtons();
}

// #74 — start/stop a per-channel bulk download.
async function toggleBulk(ds) {
  const active = ds.bulkActive === "true";
  try {
    await API.bulkDownload(ds.channelId, {
      channel_name: ds.channelName,
      platform: ds.platform,
      action: active ? "stop" : "start",
    });
    // Optimistic: flip local state; SSE bulk-progress will correct it.
    bulkStatus[ds.channelId] = active
      ? { done: 0, total: 0, active: false }
      : { done: 0, total: 0, active: true };
    Toast.success(
      active
        ? `Stopped bulk download — ${ds.channelName}`
        : `Bulk download started — ${ds.channelName}`,
    );
    if (currentRoute() === "library") render();
  } catch (e) {
    Toast.error(`Bulk download failed: ${e.message}`);
  }
}

// #74 / #73 — request the channel's playlists; the picker modal opens
// when the `playlist-list` SSE event arrives.
let pendingPlaylistChannel = null;
async function openPlaylistPicker(ds) {
  pendingPlaylistChannel = { id: ds.channelId, name: ds.channelName };
  try {
    await API.requestPlaylists(ds.channelId);
    showPlaylistModal({ loading: true, name: ds.channelName, playlists: [] });
  } catch (e) {
    Toast.error(`Couldn't load playlists: ${e.message}`);
  }
}

// ── Add-Channel two-phase wizard (item 19) ───────────────────────────
// Phase 1: pick platform + type a name → resolve (live, via SSE).
// Phase 2: show the resolved channel → confirm → enable auto-record.
// Config is deferred until the entity is confirmed.
let addWizard = null; // { platform, query } while a resolve is in flight

function openAddChannelWizard() {
  let modal = document.getElementById("add-channel-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "add-channel-modal";
    modal.className = "app-modal";
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => {
      if (e.target === modal) closeAppModal(modal);
    });
  }
  paintAddWizardSearch(modal);
  modal.classList.add("open");
  document.body.classList.add("modal-open");
}

// One owner for the click-outside / ESC / route-change dismissal of all
// .app-modal dialogs. Built so the keyboard-help (kbd-help) overlay
// stays separate — it has its own toggle and shouldn't be auto-closed
// on navigation.
function closeAppModal(modal) {
  if (!modal) return;
  modal.classList.remove("open");
  // B3: decrement the modal-open counter, which auto-clears the body
  // class only when no other open modal remains. The DOM-query check
  // is kept as a defensive belt for callers that bypass bumpModalOpen.
  bumpModalOpen(-1);
  if (_modalOpenCount === 0 && !document.querySelector(".app-modal.open")) {
    document.body.classList.remove("modal-open");
  }
}
function closeAllAppModals() {
  document.querySelectorAll(".app-modal.open").forEach(closeAppModal);
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeAllAppModals();
  // Global jump-to-recording / channel — Ctrl/Cmd+K or "/". The "/"
  // shortcut is ignored when the user is typing in an existing field
  // (matches GitHub/Linear/Slack conventions). (audit M14)
  const inField =
    document.activeElement &&
    /^(INPUT|TEXTAREA|SELECT)$/i.test(document.activeElement.tagName);
  const isSlash = e.key === "/" && !inField;
  const isCmdK = (e.key === "k" || e.key === "K") && (e.metaKey || e.ctrlKey);
  if (isSlash || isCmdK) {
    e.preventDefault();
    openCommandPalette();
  }
});

// Lightweight command palette — list every recording title + every
// channel name and route the user there on pick. Single-pass filter.
async function openCommandPalette() {
  if (document.getElementById("cmd-palette")) return;
  const dlg = document.createElement("div");
  dlg.id = "cmd-palette";
  dlg.className = "app-modal open";
  dlg.innerHTML = `
    <form class="card cmd-card" role="dialog" aria-label="Quick jump">
      <input id="cmd-q" type="search" placeholder="Search recordings, channels, settings…" autofocus />
      <div id="cmd-results" class="cmd-results">Loading…</div>
      <p class="cmd-hint">↑↓ to navigate · Enter to open · Esc to close</p>
    </form>`;
  document.body.appendChild(dlg);
  bumpModalOpen(+1);
  dlg.addEventListener("click", (e) => { if (e.target === dlg) closeAppModal(dlg); });
  // B6: capture-phase ESC so a recording-info modal opened from the
  // palette doesn't swallow the dismiss.
  const escHandler = (e) => {
    if (e.key === "Escape" && dlg.isConnected) {
      e.preventDefault();
      closeAppModal(dlg);
      document.removeEventListener("keydown", escHandler, true);
    }
  };
  document.addEventListener("keydown", escHandler, true);

  const [recs, chans] = await Promise.all([
    API.recordings().then((r) => r.recordings || []).catch(() => []),
    API.channels().then((r) => r.channels || []).catch(() => []),
  ]);
  // B32: between awaiting the API and painting results, the user may
  // have closed the dialog. Bail without touching dead DOM.
  if (!dlg.isConnected) return;
  const items = [
    ...recs.map((r) => ({
      label: niceTitle(r.stream_title) || "(no title)",
      sub: `${r.channel_name || ""} · recording`,
      href: `#/recordings`,
      hay: `${niceTitle(r.stream_title)} ${r.channel_name}`.toLowerCase(),
    })),
    ...chans.map((c) => ({
      label: c.display_name || c.name,
      sub: `${c.platform} · channel`,
      href: c.is_live ? "#/library" : `#/recordings?channel=${encodeURIComponent(c.display_name || c.name)}`,
      hay: `${c.display_name} ${c.name} ${c.platform}`.toLowerCase(),
    })),
    ...["library", "recordings", "schedule", "plugins", "settings", "system", "logs", "history"].map((r) => ({
      label: r[0].toUpperCase() + r.slice(1),
      sub: "page",
      href: `#/${r}`,
      hay: r,
    })),
  ];
  const out = dlg.querySelector("#cmd-results");
  const q = dlg.querySelector("#cmd-q");
  let active = 0;
  const paint = () => {
    const term = q.value.trim().toLowerCase();
    const hits = term
      ? items.filter((it) => it.hay.includes(term)).slice(0, 25)
      : items.slice(0, 25);
    if (!hits.length) {
      out.innerHTML = '<div class="empty sm">No matches.</div>';
      return;
    }
    active = Math.min(active, hits.length - 1);
    out.innerHTML = hits
      .map(
        (it, i) =>
          `<a class="cmd-item${i === active ? " is-active" : ""}" href="${htmlEscape(it.href)}" data-i="${i}">
            <span class="cmd-label">${htmlEscape(it.label)}</span>
            <span class="cmd-sub">${htmlEscape(it.sub)}</span>
          </a>`,
      )
      .join("");
    out.querySelectorAll(".cmd-item").forEach((el, i) => {
      el.addEventListener("click", (e) => {
        e.preventDefault();
        location.hash = hits[i].href;
        closeAppModal(dlg);
      });
    });
  };
  q.addEventListener("input", paint);
  q.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const visible = [...out.querySelectorAll(".cmd-item")];
      if (!visible.length) return;
      visible[active]?.classList.remove("is-active");
      active = (active + (e.key === "ArrowDown" ? 1 : visible.length - 1)) % visible.length;
      visible[active]?.classList.add("is-active");
      visible[active]?.scrollIntoView({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      out.querySelectorAll(".cmd-item")[active]?.click();
    }
  });
  paint();
}
window.addEventListener("hashchange", closeAllAppModals);

function paintAddWizardSearch(modal, opts = {}) {
  modal = modal || document.getElementById("add-channel-modal");
  if (!modal) return;
  const plat = opts.platform || "Twitch";
  const sel = (p) => (p === plat ? " selected" : "");
  modal.innerHTML = `
    <div class="card">
      <h2>Add channel</h2>
      <p class="wizard-step">Step 1 of 2 — find the channel</p>
      <div class="wizard-row">
        <select id="aw-platform">
          <option value="Twitch"${sel("Twitch")}>Twitch</option>
          <option value="YouTube"${sel("YouTube")}>YouTube</option>
          <option value="Patreon"${sel("Patreon")}>Patreon</option>
        </select>
        <input id="aw-query" type="text" placeholder="Twitch login, or YouTube/Patreon id"
               value="${htmlEscape(opts.query || "")}" autofocus />
        <button id="aw-search" class="primary">Search</button>
      </div>
      <div id="aw-result" class="wizard-result">${htmlEscape(opts.message || "")}</div>
    </div>`;
  const doSearch = async () => {
    const platform = modal.querySelector("#aw-platform").value;
    const query = modal.querySelector("#aw-query").value.trim();
    if (!query) return;
    addWizard = { platform, query };
    modal.querySelector("#aw-result").innerHTML = '<div class="empty sm">Searching…</div>';
    try {
      await API.resolveChannel(platform, query);
    } catch (e) {
      modal.querySelector("#aw-result").innerHTML = `<div class="empty sm">Search failed: ${htmlEscape(e.message)}</div>`;
    }
  };
  modal.querySelector("#aw-search")?.addEventListener("click", doSearch);
  modal.querySelector("#aw-query")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") doSearch();
  });
}

// Phase 2: render the resolved entity for confirmation (called from the
// ChannelResolved SSE handler).
function paintAddWizardConfirm(ev) {
  const modal = document.getElementById("add-channel-modal");
  if (!modal || !modal.classList.contains("open") || !addWizard) return;
  if (ev.platform !== addWizard.platform || ev.query !== addWizard.query) return;
  const result = modal.querySelector("#aw-result");
  if (!result) return;
  if (ev.error || !ev.channel_id) {
    result.innerHTML = `<div class="empty sm">Not found: ${htmlEscape(ev.error || "no match")}</div>`;
    return;
  }
  const name = ev.display_name || ev.channel_id;
  result.innerHTML = `
    <div class="wizard-confirm">
      <p class="wizard-step">Step 2 of 2 — confirm</p>
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(name)}</span>
          <span class="task-cadence">${htmlEscape(ev.platform)} · ${htmlEscape(ev.channel_id)}</span>
        </div>
      </div>
      <button id="aw-confirm" class="primary" data-key="${htmlEscape(ev.platform)}:${htmlEscape(ev.channel_id)}">
        Add &amp; enable auto-record
      </button>
    </div>`;
  result.querySelector("#aw-confirm")?.addEventListener("click", async (e) => {
    const key = e.currentTarget.dataset.key;
    try {
      await API.toggleAutoRecord(key, true);
      Toast.success(`Added ${name} — auto-record on`);
      modal.classList.remove("open");
      addWizard = null;
      if (currentRoute() === "library") render();
    } catch (err) {
      Toast.error(`Add failed: ${err.message}`);
    }
  });
}

function showPlaylistModal(opts) {
  let modal = document.getElementById("playlist-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "playlist-modal";
    modal.className = "app-modal";
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => {
      if (e.target === modal) modal.classList.remove("open");
    });
  }
  const rows = opts.loading
    ? "<div>Loading playlists…</div>"
    : [
        `<div class="pl-row" data-pl=""><b>▣ Whole channel</b> (all uploads)</div>`,
        ...opts.playlists.map(
          (p) =>
            `<div class="pl-row" data-pl="${htmlEscape(p.id)}">≡ ${htmlEscape(p.title)}${
              p.item_count != null ? ` (${p.item_count})` : ""
            }</div>`,
        ),
      ].join("");
  modal.innerHTML = `
    <div class="card">
      <h2>Bulk download — ${htmlEscape(opts.name)}</h2>
      <div class="pl-list">${rows}</div>
    </div>`;
  modal.classList.add("open");
  modal.querySelectorAll(".pl-row").forEach((row) => {
    row.addEventListener("click", async () => {
      const ch = pendingPlaylistChannel;
      if (!ch) return;
      const playlist_id = row.dataset.pl || null;
      try {
        await API.bulkDownload(ch.id, {
          channel_name: ch.name,
          platform: "YouTube",
          action: "start",
          playlist_id,
        });
        bulkStatus[ch.id] = { done: 0, total: 0, active: true };
        modal.classList.remove("open");
        Toast.success(`Bulk download started — ${ch.name}`);
        if (currentRoute() === "library") render();
      } catch (e) {
        Toast.error(`Bulk download failed: ${e.message}`);
      }
    });
  });
}

// #74 — bulk-download toggle button reflecting live SSE progress.
function bulkButton(c) {
  const st = bulkStatus[c.id];
  if (st && st.active) {
    const label = st.total > 0 ? `⇩ ${st.done}/${st.total} — Stop` : "⇩ … — Stop";
    return `<button data-action="bulk" data-bulk-active="true"
              data-channel-id="${c.id}"
              data-channel-name="${htmlEscape(c.display_name || c.name)}"
              data-platform="${c.platform}">${label}</button>`;
  }
  return `<button data-action="bulk" data-bulk-active="false"
            data-channel-id="${c.id}"
            data-channel-name="${htmlEscape(c.display_name || c.name)}"
            data-platform="${c.platform}">⇩ Bulk DL</button>`;
}

async function startRecordingFromCard(d) {
  try {
    await API.startRecording({
      channel_id: d.channelId,
      channel_name: d.channelName,
      display_name: d.displayName,
      platform: d.platform,
      from_start: d.fromStart === "true",
      stream_title: d.streamTitle || null,
      thumbnail_url: d.thumbnail || null,
      transcode: false,
    });
    Toast.success(
      `Recording ${d.fromStart === "true" ? "from start " : ""}— ${d.displayName || d.channelName}`,
    );
  } catch (e) {
    Toast.error(`Start failed: ${e.message}`);
  }
}

async function toggleAutoRecord(d) {
  const enabling = d.enabled === "true";
  try {
    await API.toggleAutoRecord(d.channelKey, enabling);
    Toast.success(enabling ? "Auto-record enabled" : "Auto-record disabled");
    await render();
  } catch (e) {
    Toast.error(`Auto-record toggle failed: ${e.message}`);
  }
}

// ── Recordings table ─────────────────────────────────────────────────
async function renderRecordings() {
  // Allow sidebar links / external bookmarks to seed the search box
  // via #/recordings?channel=NAME (audit M2).
  const hash = window.location.hash || "";
  const qIdx = hash.indexOf("?");
  if (qIdx !== -1) {
    try {
      const params = new URLSearchParams(hash.slice(qIdx + 1));
      const ch = params.get("channel");
      if (ch != null) recFilter = ch;
    } catch (_) {}
  }
  let recordings = [];
  try {
    const data = await API.recordings();
    recordings = data.recordings || [];
  } catch (e) {
    if (e.message.includes("unauthorized")) return;
    root.innerHTML = chrome(
      `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`,
    );
    setupChromeHandlers();
    return;
  }
  root.removeAttribute("aria-busy");
  if (recordings.length === 0) {
    root.innerHTML = chrome(`
      <h1 class="page-title">Recordings</h1>
      <div class="empty">
        <div class="glyph">📁</div>
        No recordings yet. Start one from the Library tab.
      </div>
    `);
    setupChromeHandlers();
    return;
  }
  recCache = recordings;
  seedVodDownloadStateFromRecCache();
  // W4-alt: sortable + filterable data grid. Column headers toggle sort;
  // the filter box narrows by channel/title live without refetching.
  root.innerHTML = chrome(`
    <h1 class="page-title">Recordings</h1>
    <div class="rec-toolbar">
      <input id="rec-filter" class="grid-filter" type="search"
             placeholder="Filter by channel or title… (/)"
             aria-label="Filter recordings" value="${htmlEscape(recFilter)}">
      <label class="rec-daterange" title="Filter recordings by started_at; inclusive.">
        from <input id="rec-from" class="rec-date" type="datetime-local" step="60" value="${htmlEscape(recDateFrom || "")}"/>
        to <input id="rec-to" class="rec-date" type="datetime-local" step="60" value="${htmlEscape(recDateTo || "")}"/>
        <button id="rec-clear-range" class="sm" type="button" title="Clear date range">✕</button>
      </label>
      <button id="rec-groupby" class="sm" title="Group rows by channel">
        ${recGroupBy === "channel" ? "▼ Grouped by channel" : "≣ Group by channel"}
      </button>
      <button id="rec-density" class="sm" title="Toggle row density">
        ${recDensity === "compact" ? "≡ Comfortable rows" : "═ Compact rows"}
      </button>
      ${(() => {
        const errored = recCache.filter((r) => stateClassName(r.state) === "failed" || stateLabel(r.state).toLowerCase().includes("interrupt")).length;
        return errored > 0
          ? `<button id="rec-clear-errored" class="danger sm" title="Trash all failed/interrupted recordings">✕ Clear errored (${errored})</button>`
          : "";
      })()}
    </div>
    <div id="rec-state-chips" class="rec-state-chips" role="group" aria-label="Filter by state"></div>
    <p class="page-subtitle" id="rec-count"></p>
    <div id="rec-massbar" class="massbar"></div>
    <table class="recordings-table ${recDensity === "compact" ? "compact" : ""}">
      <thead>
        <tr>
          <th class="rec-check"><input type="checkbox" id="rec-select-all" aria-label="Select all"></th>
          ${recHeader("state", "State")}
          ${recHeader("channel", "Channel")}
          ${recHeader("title", "Title")}
          ${recHeader("started", "Started")}
          ${recHeader("size", "Size")}
          <th></th>
        </tr>
      </thead>
      <tbody id="rec-body"></tbody>
    </table>
  `);
  setupChromeHandlers();
  paintRecordings();

  document.getElementById("rec-filter")?.addEventListener("input", (e) => {
    recFilter = e.target.value;
    paintRecordings();
  });
  document.getElementById("rec-from")?.addEventListener("change", (e) => {
    recDateFrom = e.target.value;
    paintRecordings();
  });
  document.getElementById("rec-to")?.addEventListener("change", (e) => {
    recDateTo = e.target.value;
    paintRecordings();
  });
  document.getElementById("rec-clear-range")?.addEventListener("click", () => {
    recDateFrom = ""; recDateTo = "";
    const f = document.getElementById("rec-from"); const t = document.getElementById("rec-to");
    if (f) f.value = ""; if (t) t.value = "";
    paintRecordings();
  });
  document.getElementById("rec-density")?.addEventListener("click", () => {
    recDensity = recDensity === "compact" ? "comfortable" : "compact";
    localStorage.setItem("strivo-rec-density", recDensity);
    renderRecordings().catch((e) => Toast.error(e.message));
  });
  document.getElementById("rec-groupby")?.addEventListener("click", () => {
    recGroupBy = recGroupBy === "channel" ? "none" : "channel";
    localStorage.setItem("strivo-rec-groupby", recGroupBy);
    renderRecordings().catch((e) => Toast.error(e.message));
  });
  // Build state chips from the unique states currently in the cache, so
  // we don't paint chips for states that have zero rows. Each chip is a
  // toggle that AND-narrows the visible rows (empty filter = show all).
  paintRecStateChips();
  document.getElementById("rec-clear-errored")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const errored = recCache.filter((r) => {
      const c = stateClassName(r.state);
      const l = stateLabel(r.state).toLowerCase();
      return c === "failed" || l.includes("interrupt");
    });
    if (errored.length === 0) return;
    if (!(await confirmDialog(`Trash ${errored.length} errored recording(s)? Files move to the 7-day trash.`, { ok: "Clear", danger: true })))
      return;
    await withBusy(btn, "Clearing…", async () => {
      await API.clearErroredRecordings();
      Toast.success(`Cleared ${errored.length}`);
      // Optimistic prune; SSE refetch confirms.
      const erroredIds = new Set(errored.map((r) => r.id));
      recCache = recCache.filter((r) => !erroredIds.has(r.id));
      renderRecordings().catch(() => {});
    }).catch((err) => Toast.error(`Clear failed: ${err.message}`));
  });
  document.getElementById("rec-select-all")?.addEventListener("change", (e) => {
    const visible = visibleRecordingIds();
    if (e.target.checked) visible.forEach((id) => recSelected.add(id));
    else visible.forEach((id) => recSelected.delete(id));
    paintRecordings();
  });
  document.querySelectorAll("th[data-sort]").forEach((th) => {
    th.addEventListener("click", () => {
      const col = th.dataset.sort;
      if (recSort.col === col) {
        recSort.dir = recSort.dir === "asc" ? "desc" : "asc";
      } else {
        recSort = { col, dir: "asc" };
      }
      renderRecordings().catch((e) => Toast.error(e.message)); // re-render header arrows + body
    });
  });
}

function paintRecStateChips() {
  const host = document.getElementById("rec-state-chips");
  if (!host) return;
  const counts = new Map();
  for (const r of recCache) {
    const key = stateClassName(r.state);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  if (counts.size <= 1) {
    // Single state in cache → chips add no value; skip the row entirely.
    host.innerHTML = "";
    return;
  }
  const sorted = Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  const chips = sorted
    .map(([state, n]) => {
      const active = recStateFilter.size === 0 || recStateFilter.has(state);
      return `<button class="rec-state-chip state-${htmlEscape(state)} ${active ? "active" : ""}"
                data-state="${htmlEscape(state)}" type="button">
        <span class="rec-state-chip-dot"></span>
        ${htmlEscape(stateChipLabel(state))}
        <span class="rec-state-chip-count">${n}</span>
      </button>`;
    })
    .join("");
  const allActive = recStateFilter.size === 0;
  host.innerHTML = `
    <button class="rec-state-chip rec-state-chip-all ${allActive ? "active" : ""}"
            type="button" title="Show every state">
      <span class="rec-state-chip-dot"></span>All <span class="rec-state-chip-count">${recCache.length}</span>
    </button>
    ${chips}`;
  host.querySelector(".rec-state-chip-all")?.addEventListener("click", () => {
    recStateFilter.clear();
    localStorage.setItem("strivo-rec-state-filter", "");
    paintRecStateChips();
    paintRecordings();
  });
  host.querySelectorAll("[data-state]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const s = btn.dataset.state;
      // Click pattern: starting from "all visible", a click selects ONLY
      // that state. Subsequent clicks toggle additional states (AND-narrow
      // turns into OR-additive — matches gmail's chip behaviour).
      if (recStateFilter.size === 0) {
        recStateFilter = new Set([s]);
      } else if (recStateFilter.has(s)) {
        recStateFilter.delete(s);
      } else {
        recStateFilter.add(s);
      }
      localStorage.setItem(
        "strivo-rec-state-filter",
        Array.from(recStateFilter).join(","),
      );
      paintRecStateChips();
      paintRecordings();
    });
  });
}

// Human-friendly label for a state classname. Falls back to title-case.
function stateChipLabel(cls) {
  switch (cls) {
    case "finished": return "Finished";
    case "recording": return "Recording";
    case "downloading": return "Downloading";
    case "failed": return "Failed";
    case "file-error": return "File missing";
    case "scheduled": return "Scheduled";
    default: return cls.replace(/[-_]/g, " ").replace(/\b\w/g, c => c.toUpperCase());
  }
}

function recHeader(key, label) {
  // Active column shows the direction arrow; inactive sortable columns
  // get a faint ↕ so the affordance is discoverable (R6 audit fix).
  const arrow =
    recSort.col === key
      ? (recSort.dir === "asc" ? " ▲" : " ▼")
      : ' <span class="rec-th-sort-hint" aria-hidden="true">↕</span>';
  return `<th data-sort="${key}" class="rec-th-sortable">${label}${arrow}</th>`;
}

// Apply the live filter + sort to recCache and repaint the table body.
function paintRecordings() {
  const body = document.getElementById("rec-body");
  if (!body) return;
  const q = recFilter.trim().toLowerCase();
  let rows = recCache.filter((r) => {
    if (recStateFilter.size > 0 && !recStateFilter.has(stateClassName(r.state))) return false;
    // Started-at date-range filter. Empty bound = unbounded.
    if (recDateFrom || recDateTo) {
      const sa = (r.started_at || "").slice(0, 19); // YYYY-MM-DDTHH:MM:SS
      if (!sa) return false;
      if (recDateFrom && sa < recDateFrom) return false;
      if (recDateTo && sa > recDateTo) return false;
    }
    if (!q) return true;
    return (
      (r.channel_name || "").toLowerCase().includes(q) ||
      niceTitle(r.stream_title).toLowerCase().includes(q)
    );
  });
  const dir = recSort.dir === "asc" ? 1 : -1;
  const key = (r) => {
    switch (recSort.col) {
      case "state": return stateLabel(r.state).toLowerCase();
      case "channel": return (r.channel_name || "").toLowerCase();
      case "title": return niceTitle(r.stream_title).toLowerCase();
      case "size": return r.bytes_written || 0;
      case "started":
      default: return new Date(r.started_at).getTime() || 0;
    }
  };
  rows.sort((a, b) => {
    const ka = key(a), kb = key(b);
    return ka < kb ? -dir : ka > kb ? dir : 0;
  });
  const totalRows = rows.length;
  const renderRows = rows.slice(0, recRenderLimit);
  recVisible = renderRows;
  if (recGroupBy === "channel") {
    // Cluster rows by channel_name while preserving the active sort order
    // within each cluster. Each cluster gets a heading row spanning every
    // column — sticky-styled via CSS — so the table reads like a grouped
    // ledger without needing a separate render pass per group.
    const order = [];
    const byChannel = new Map();
    for (const r of renderRows) {
      const k = r.channel_name || "(unknown)";
      if (!byChannel.has(k)) { byChannel.set(k, []); order.push(k); }
      byChannel.get(k).push(r);
    }
    const html = order.map((ch) => {
      const list = byChannel.get(ch);
      const totalBytes = list.reduce((a, b) => a + (b.bytes_written || 0), 0);
      return `<tr class="rec-group-head"><td colspan="7">
        <span class="rec-group-name">${htmlEscape(ch)}</span>
        <span class="rec-group-meta">${list.length} recording${list.length === 1 ? "" : "s"} · ${formatBytes(totalBytes)}</span>
      </td></tr>${list.map(recordingRow).join("")}`;
    }).join("");
    body.innerHTML = html;
  } else {
    body.innerHTML = renderRows.map(recordingRow).join("");
  }
  if (renderRows.length < totalRows) {
    body.insertAdjacentHTML("beforeend", `<tr><td colspan="7" class="empty sm">
      <button id="rec-show-more" type="button">Show ${Math.min(200, totalRows - renderRows.length)} more</button>
      <span class="pg-cap-hint"> ${renderRows.length} of ${totalRows} rendered</span>
    </td></tr>`);
    body.querySelector("#rec-show-more")?.addEventListener("click", () => {
      recRenderLimit += 200;
      paintRecordings();
    });
  }
  const count = document.getElementById("rec-count");
  if (count) {
    count.textContent =
      q || totalRows !== recCache.length
        ? `${totalRows} matching · ${renderRows.length} rendered`
        : `${recCache.length} total · ${renderRows.length} rendered`;
  }
  body.querySelectorAll("[data-action=stop]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!(await confirmDialog("Stop this recording?", { ok: "Stop", danger: true })))
        return;
      await withBusy(btn, "Stopping…", async () => {
        await API.stopRecording(btn.dataset.jobId);
        Toast.success("Recording stopped");
        setTimeout(() => render().catch(() => {}), 500);
      }).catch((e) => Toast.error(`Stop failed: ${e.message}`));
    });
  });
  body.querySelectorAll("[data-action=rec-play]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const id = btn.dataset.jobId;
      if (id) window.location.hash = `#/watch?recording=${encodeURIComponent(id)}&fresh=1`;
    });
  });
  body.querySelectorAll("[data-action=rec-info]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openRecordingInfo(btn.dataset.jobId);
    });
  });
  body.querySelectorAll("[data-action=rec-rescan]").forEach((btn) => {
    btn.addEventListener("click", (e) => { e.stopPropagation(); reScanRecording(btn); });
  });
  body.querySelectorAll("[data-action=rec-locate]").forEach((btn) => {
    btn.addEventListener("click", (e) => { e.stopPropagation(); showRecordingPath(btn.dataset.path); });
  });
  body.querySelectorAll("[data-action=rec-delete]").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!(await confirmDialog("Delete this recording? The file moves to the 7-day trash.", { ok: "Delete", danger: true })))
        return;
      await withBusy(btn, "Deleting…", async () => {
        await API.deleteRecordingFile(btn.dataset.jobId);
        Toast.success("Deleted");
        // Optimistic: drop from local cache + repaint; the SSE refetch
        // confirms shortly.
        recCache = recCache.filter((r) => r.id !== btn.dataset.jobId);
        renderRecordings().catch(() => {});
      }).catch((err) => Toast.error(`Delete failed: ${err.message}`));
    });
  });
  // Row click:
  //   plain                           → open Info modal
  //   Shift+click                     → select range (anchor → here)
  //   Ctrl/Cmd+click                  → toggle just this row
  // Buttons/inputs/anchors still get their own handlers (early-return).
  // W4 keyboard nav: rows are tabbable; Enter plays, I opens info, Del
  // confirms delete. Delegated on body so we attach one handler total
  // regardless of N rows (audit P1 perf #4).
  if (!body.dataset.kbBound) {
    body.dataset.kbBound = "1";
    body.tabIndex = -1;
    body.addEventListener("keydown", (e) => {
      const tr = e.target.closest("tr[data-rec-row]");
      if (!tr) return;
      const id = tr.dataset.recRow;
      if (e.key === "Enter") {
        e.preventDefault();
        const playable = tr.querySelector('button[data-action="play-rec"]') ||
                         tr.querySelector('.rec-action-play');
        if (playable) playable.click();
        else if (id) window.location.hash = `#/watch?recording=${encodeURIComponent(id)}&fresh=1`;
      } else if (e.key === "i" || e.key === "I") {
        e.preventDefault();
        const info = tr.querySelector('[data-action="info"], .rec-action-info');
        info?.click();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        const del = tr.querySelector('[data-action="delete"], .rec-action-del');
        del?.click();
      }
    });
  }
  body.querySelectorAll("tr[data-rec-row]").forEach((tr) => {
    if (!tr.hasAttribute("tabindex")) tr.tabIndex = 0;
    tr.addEventListener("click", (e) => {
      if (e.target.closest("button, input, a")) return;
      const id = tr.dataset.recRow;
      if (e.shiftKey && recAnchorId) {
        e.preventDefault();
        const ids = visibleRecordingIds();
        const i = ids.indexOf(recAnchorId);
        const j = ids.indexOf(id);
        if (i >= 0 && j >= 0) {
          const [lo, hi] = i < j ? [i, j] : [j, i];
          for (let k = lo; k <= hi; k++) recSelected.add(ids[k]);
          paintRecordings();
        }
        return;
      }
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        if (recSelected.has(id)) recSelected.delete(id);
        else recSelected.add(id);
        recAnchorId = id;
        paintRecordings();
        return;
      }
      openRecordingInfo(id);
    });
  });
  // Selection model:
  //   Click checkbox            → toggle this row
  //   Shift+click checkbox/row  → select range from anchor to here
  //   Ctrl/Cmd+click row body   → toggle this row (without opening Info)
  //   Plain click row body      → open Info modal (handled below)
  body.querySelectorAll(".rec-row-check").forEach((cb) => {
    // C4/C11/C18: native checkbox toggles on Space BEFORE our click
    // listener can preventDefault, so we'd see the native flip then
    // our handler ran a second toggle. Intercept Space in keydown
    // (with preventDefault) so only the click-path semantics decide.
    cb.addEventListener("keydown", (e) => {
      if (e.key === " " || e.code === "Space") {
        e.preventDefault();
        cb.click();
      }
    });
    // Suppress the native `change`; a `click` handler with
    // `preventDefault` implements range semantics.
    cb.addEventListener("click", (e) => {
      e.preventDefault();
      const id = cb.dataset.jobId;
      if (e.shiftKey && recAnchorId) {
        const ids = visibleRecordingIds();
        const i = ids.indexOf(recAnchorId);
        const j = ids.indexOf(id);
        if (i >= 0 && j >= 0) {
          const [lo, hi] = i < j ? [i, j] : [j, i];
          for (let k = lo; k <= hi; k++) recSelected.add(ids[k]);
        }
      } else {
        if (recSelected.has(id)) recSelected.delete(id);
        else recSelected.add(id);
        recAnchorId = id;
      }
      paintRecordings();
    });
  });
  const all = document.getElementById("rec-select-all");
  if (all) {
    const vis = visibleRecordingIds();
    all.checked = vis.length > 0 && vis.every((id) => recSelected.has(id));
  }
  updateMassbar();
}

// IDs currently visible after filter/sort (for select-all + mass actions).
let recVisible = [];
let recRenderLimit = 200;
function visibleRecordingIds() {
  return recVisible.map((r) => r.id);
}

// Show/hide the multi-select mass-action bar (item 22). Acts on the selection
// intersected with currently-visible rows.
function updateMassbar() {
  const bar = document.getElementById("rec-massbar");
  if (!bar) return;
  const visible = new Set(visibleRecordingIds());
  const sel = recVisible.filter((r) => recSelected.has(r.id) && visible.has(r.id));
  if (sel.length === 0) {
    // Audit fix: persistent toolbar so the bulk affordances are
    // discoverable BEFORE selection. Disabled buttons show what's
    // possible; selecting any row enables them.
    bar.hidden = false;
    bar.classList.add("massbar-empty");
    bar.innerHTML = `
      <span class="massbar-count muted">No rows selected — tick a checkbox to enable bulk actions</span>
      <button class="sm" disabled>Stop active</button>
      <button class="sm" disabled>Re-record</button>
      <button class="sm" disabled>Remux</button>
      <button class="danger sm" disabled>Delete</button>`;
    return;
  }
  bar.classList.remove("massbar-empty");
  const active = sel.filter((r) => stateClassName(r.state) === "recording");
  bar.hidden = false;
  // Pre-compute which selected rows are finished + look browser-broken,
  // so the Remux button is only offered when it could actually help.
  const remuxable = sel.filter((r) => stateClassName(r.state) === "finished" && r.file_exists !== false);
  const deletable = sel.filter((r) => r.file_exists !== false || stateClassName(r.state) !== "recording");
  bar.innerHTML = `
    <span class="massbar-count">${sel.length} selected</span>
    ${active.length ? `<button id="mass-stop" class="danger sm">Stop ${active.length} active</button>` : ""}
    <button id="mass-rerecord" class="sm">Re-record ${sel.length}</button>
    ${remuxable.length ? `<button id="mass-remux" class="sm" title="Remux for browser playback (matroska + aac_adtstoasc)">Remux ${remuxable.length}</button>` : ""}
    ${deletable.length ? `<button id="mass-delete" class="danger sm">Delete ${deletable.length}</button>` : ""}
    <button id="mass-clear" class="sm">Clear</button>`;
  document.getElementById("mass-clear")?.addEventListener("click", () => {
    recSelected.clear();
    paintRecordings();
  });
  document.getElementById("mass-stop")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Stop ${active.length} active recording(s)?`, { ok: "Stop", danger: true })))
      return;
    let ok = 0;
    for (const r of active) {
      try {
        await API.stopRecording(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Stopped ${ok}/${active.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-rerecord")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Re-record ${sel.length} channel(s) now? This starts fresh captures and may collide with any active recording on those channels.`, { ok: "Re-record", danger: true })))
      return;
    let ok = 0;
    for (const r of sel) {
      try {
        await API.startRecording({
          channel_id: r.channel_id,
          channel_name: r.channel_name,
          platform: r.platform,
          from_start: true,
        });
        ok++;
      } catch (_) {}
    }
    Toast.success(`Re-record queued ${ok}/${sel.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-remux")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Remux ${remuxable.length} recording(s) for browser playback? Originals are kept as <name>.orig until success.`, { ok: "Remux" })))
      return;
    let ok = 0;
    for (const r of remuxable) {
      try {
        await API.remuxRecording(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Remuxed ${ok}/${remuxable.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-delete")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Delete ${deletable.length} recording(s)? Files move to the 7-day trash.`, { ok: "Delete", danger: true })))
      return;
    let ok = 0;
    for (const r of deletable) {
      try {
        await API.deleteRecordingFile(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Deleted ${ok}/${deletable.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
}

// Cover thumbnail for a recording. The wrapper renders a channel-initials
// tile coloured by a hash of the channel name; the inner <img> sits on top
// and covers it when /thumb returns a real jpg. On 404 the img self-removes
// and the initials show through, so old recordings (made before the source-
// thumbnail snapshot landed, and missed by ffmpeg fallback on the server)
// still look intentional rather than broken.
function recThumb(r) {
  const initials = (r.channel_name || r.stream_title || "?")
    .trim()
    .replace(/[^\p{L}\p{N} ]/gu, "")
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join("") || "?";
  const hue = thumbHue(r.channel_name || r.id || "");
  // r.file_exists is set by the backend's augment_recording; when false the
  // recording's output_path is gone from disk (moved / deleted / external
  // drive offline) so we surface it as a red-caps overlay over the thumb.
  const missing = r.file_exists === false ? " rec-thumb-missing" : "";
  return `<span class="rec-thumb-wrap${missing}" data-init="${htmlEscape(initials)}"
    style="--ch-hue:${hue}deg">
    <img class="rec-thumb" loading="lazy" alt=""
      src="/api/v1/recordings/${encodeURIComponent(r.id)}/thumb"
      onerror="this.remove()" />
  </span>`;
}

// Stable hash → hue so the same channel always gets the same colour, but
// different channels get different ones across the rail.
function thumbHue(s) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h) % 360;
}

function recordingRow(r) {
  const disp = recordingDisplayState(r);
  const stateClass = disp.className;
  // Active includes both live captures (Recording) and VOD pulls (Downloading);
  // both are in-flight and offer Stop.
  const isActive = stateClass === "recording" || stateClass === "downloading";
  const isFinished = stateClass === "finished";
  // Action set per state. Play sits in slot 1 across every row; when the
  // recording isn't playable yet we render a disabled placeholder so the
  // button columns stay vertically aligned (in-flight downloads + failed
  // captures previously dropped slot 1 and the remaining buttons hopped
  // left).
  const playBtn = isFinished
    ? `<button class="primary sm" data-action="rec-play" data-job-id="${r.id}" title="Open player (Enter)">▶ Play</button>`
    : `<button class="primary sm rec-play-disabled" disabled aria-disabled="true" title="${isActive ? "Playable when capture finishes" : "Recording unavailable"}">▶ Play</button>`;
  const tailBtns = isActive
    ? `<button class="danger sm" data-action="stop" data-job-id="${r.id}">Stop</button>`
    : `<button class="sm" data-action="rec-info" data-job-id="${r.id}" title="Recording details (I)">ⓘ Info</button>
       <button class="danger sm" data-action="rec-delete" data-job-id="${r.id}" title="Delete (Del)">✕</button>`;
  // File-error remediation: re-scan (re-check file_exists, in case the
  // user remounted a drive or restored from backup) + locate (show the
  // absolute path with a copy gesture). Distinct from Failed which is
  // a process error — file-error means the journal-vs-disk drifted.
  const fileErrorBtns = stateClass === "file-error"
    ? `<button class="sm" data-action="rec-rescan" data-job-id="${r.id}" title="Re-check whether the file exists">↻ Re-scan</button>
       <button class="sm" data-action="rec-locate" data-job-id="${r.id}" data-path="${htmlEscape(r.output_path || "")}" title="Show the expected file path">📂 Show path</button>`
    : "";
  const actions = `${playBtn}${fileErrorBtns}${tailBtns}`;
  return `
    <tr class="${recSelected.has(r.id) ? "rec-sel" : ""}" data-rec-row="${htmlEscape(r.id)}">
      <td class="rec-check"><input type="checkbox" class="rec-row-check" data-job-id="${htmlEscape(r.id)}" ${recSelected.has(r.id) ? "checked" : ""} aria-label="Select recording"></td>
      <td>${renderStatePill(disp)}</td>
      <td>${htmlEscape(r.channel_name)}</td>
      <td><div class="rec-title-cell">${recThumb(r)}<span>${htmlEscape(niceTitle(r.stream_title) || "(no title)")}</span></div></td>
      <td>${new Date(r.started_at).toLocaleString()}</td>
      <td>${formatBytes(r.bytes_written || 0)}</td>
      <td class="rec-actions"><div class="rec-actions-inner">${actions}</div></td>
    </tr>
  `;
}

// VOD pulls and live captures both ride `RecordingState::Recording`, but
// "Recording" reads wrong for a yt-dlp-backed VOD pull. Distinguish by
// `source_url`: when set, label + colour as a download instead. Other
// states (Finished/Failed/etc) read the same regardless.
// File-error remediation: refetch /recordings so the backend re-runs
// augment_recording's file_exists probe on the current row. When the
// flag flips back to true (file restored / drive remounted) the next
// render shows it as plain Finished again.
async function reScanRecording(btn) {
  const id = btn.dataset.jobId;
  await withBusy(btn, "Scanning…", async () => {
    try {
      const r = await API.recordingOne(id);
      if (r && r.file_exists !== false) {
        Toast.success("File found — refreshing");
      } else {
        Toast.error("Still missing — file not present at the recorded path");
      }
      // Whichever way it went, repaint the current route so the badge updates.
      render().catch(() => {});
    } catch (err) {
      Toast.error(`Re-scan failed: ${err.message}`);
    }
  });
}

// Pop a tiny copy-friendly modal showing the recording's intended file
// path. Doesn't try to open a native file manager (the SPA can't reach
// the desktop) — instead lets the user copy the path with one click so
// they can paste it into their own shell / finder.
function showRecordingPath(path) {
  if (!path) {
    Toast.error("No path recorded for this row");
    return;
  }
  const overlay = ensureModalContainer("rec-locate-modal");
  overlay.innerHTML = `
    <div class="modal-card rec-locate-card">
      <header class="rec-locate-head">
        <h2>Recording file path</h2>
        <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
      </header>
      <p class="pg-cap-hint">The recording was written here. The SPA can't open your file manager directly — copy the path and open it yourself.</p>
      <div class="rec-locate-row">
        <code class="rec-locate-path">${htmlEscape(path)}</code>
        <button class="primary sm rec-locate-copy">Copy path</button>
      </div>
    </div>`;
  document.body.classList.add("modal-open");
  overlay.addEventListener("click", (e) => { if (e.target === overlay) closeRecLocate(); });
  overlay.querySelector("[data-action=modal-close]").addEventListener("click", closeRecLocate);
  overlay.querySelector(".rec-locate-copy").addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(path);
      Toast.success("Path copied to clipboard");
      closeRecLocate();
    } catch (err) {
      Toast.error(`Copy failed: ${err.message}`);
    }
  });
}
function closeRecLocate() {
  document.getElementById("rec-locate-modal")?.remove();
  document.body.classList.remove("modal-open");
}

function recordingDisplayState(j) {
  const cls = stateClassName(j.state);
  const lbl = stateLabel(j.state);
  // A row whose file is gone overrides every other state — the journal
  // says "Finished" but the recording has no file behind it, so reading
  // that as a green Finished pill misleads.
  if (j && j.file_exists === false) {
    return { label: "File Error", className: "file-error", pct: null };
  }
  if (j && j.source_url && cls === "recording") {
    return { label: "Downloading", className: "downloading", pct: pctForDownload(j) };
  }
  return { label: lbl, className: cls, pct: null };
}

// Project an in-flight VOD pull's percent complete. yt-dlp publishes an
// authoritative `download_pct` whenever the server-side total size is
// known; for HLS / segment-list captures it isn't, so we fall back to a
// projection from the cached VOD's known duration × detected bitrate.
//
// The blend (avg-so-far weighted 0.7, instantaneous 0.3) keeps a brief
// network stall from collapsing the bar mid-download.
function pctForDownload(j) {
  if (!j) return null;
  if (j.download_pct != null && Number.isFinite(j.download_pct)) {
    return Math.max(0, Math.min(100, Number(j.download_pct)));
  }
  const bw = Number(j.bytes_written) || 0;
  const elapsed = Number(j.duration_secs) || 0;
  if (bw < 1 || elapsed < 1) return null;
  const expected = expectedDurationFromVodCache(j);
  if (!expected || expected <= 0) return null;
  const avgBps = bw / elapsed;
  const instBps = Number(j.download_rate_bps) || 0;
  const blendedBps = instBps > 0 ? avgBps * 0.7 + instBps * 0.3 : avgBps;
  const projectedTotal = blendedBps * expected;
  if (projectedTotal <= 0) return null;
  // Cap at 99 — only the Finish event grants 100.
  return Math.max(0, Math.min(99, (bw / projectedTotal) * 100));
}

// Cross-reference the channel-VOD cache to find the expected total
// duration for a recording's source URL. Returns seconds, or null when
// the SPA hasn't visited the source channel's detail page this session
// (no cache entry to consult).
function expectedDurationFromVodCache(j) {
  if (!j || !j.source_url) return null;
  if (typeof channelVods === "undefined" || !channelVods) return null;
  for (const list of Object.values(channelVods)) {
    if (!Array.isArray(list)) continue;
    for (const v of list) {
      if (v && v.url === j.source_url && v.duration_seconds) {
        return Number(v.duration_seconds);
      }
    }
  }
  return null;
}

// Render the state pill, with a progress fill + percentage label when
// `disp.pct` is known. The fill is driven by the `--state-fill` custom
// property so the CSS doesn't need a class per percentage bucket.
function renderStatePill(disp) {
  if (disp.pct == null) {
    return `<span class="state-pill ${disp.className}">${htmlEscape(disp.label)}</span>`;
  }
  const pct = disp.pct;
  const rounded = Math.round(pct);
  return `<span class="state-pill ${disp.className} has-fill" style="--state-fill:${pct.toFixed(1)}%">
    <span class="state-pill-fill" aria-hidden="true"></span>
    <span class="state-pill-label">${rounded}%</span>
  </span>`;
}

function stateLabel(s) {
  if (typeof s === "string") return s;
  if (s && typeof s === "object") return Object.keys(s)[0];
  return "?";
}
function stateClassName(s) {
  const label = stateLabel(s).toLowerCase();
  if (label.includes("record")) return "recording";
  if (label.includes("finish")) return "finished";
  if (label.includes("fail")) return "failed";
  return "";
}

// ── Gantt strip (W5 — last 24h of recordings as horizontal bars) ──────
function renderGantt(items) {
  if (items.length === 0) return "";
  // Bucket by channel for the vertical axis; horizontal axis is the
  // last 24 hours.
  const now = Date.now();
  const windowMs = 24 * 60 * 60 * 1000;
  const start = now - windowMs;
  const byChannel = new Map();
  for (const it of items) {
    const ch = it.channel_name || "(unknown)";
    if (!byChannel.has(ch)) byChannel.set(ch, []);
    byChannel.get(ch).push(it);
  }
  const channels = [...byChannel.keys()];
  if (channels.length === 0) return "";
  const rowH = 22;
  const totalH = channels.length * rowH + 24;
  // SVG width is responsive via 100%; bars use percentage coordinates.
  const bars = channels
    .map((ch, i) => {
      const y = i * rowH + 20;
      const chBars = byChannel
        .get(ch)
        .map((it) => {
          const s = new Date(it.start_at).getTime();
          const e = new Date(it.end_at).getTime();
          const xPct = Math.max(0, ((s - start) / windowMs) * 100);
          const wPct = Math.max(0.3, Math.min(100 - xPct, ((e - s) / windowMs) * 100));
          const stateColor =
            it.state.toLowerCase().includes("record")
              ? "var(--recording)"
              : it.state.toLowerCase().includes("finish")
              ? "var(--live)"
              : it.state.toLowerCase().includes("fail")
              ? "var(--secondary)"
              : "var(--muted)";
          return `<rect x="${xPct}%" y="${y + 3}" width="${wPct}%" height="14"
                     fill="${stateColor}" rx="2"
                     data-title="${htmlEscape(it.stream_title || ch)} · ${formatBytes(it.bytes_written || 0)}"></rect>`;
        })
        .join("");
      return `
        <text x="0" y="${y + 14}" fill="var(--muted)" font-size="11" font-family="ui-monospace, monospace">
          ${htmlEscape(ch.slice(0, 18))}
        </text>
        ${chBars}
      `;
    })
    .join("");
  // Vertical "now" marker at the right edge (100%).
  const nowMarker = `<line x1="100%" x2="100%" y1="20" y2="${totalH - 4}" stroke="var(--primary)" stroke-width="2" stroke-dasharray="2 2"/>`;
  return `
    <div style="background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 1rem; margin-bottom: 2rem;">
      <h2 style="margin: 0 0 0.5rem 0; font-size: 0.875rem; color: var(--muted);">
        24h timeline · ${items.length} recording${items.length === 1 ? "" : "s"}
      </h2>
      <svg viewBox="0 0 100 ${totalH}" preserveAspectRatio="none"
           style="width: 100%; height: ${totalH}px; padding-left: 120px; box-sizing: border-box;"
           role="img" aria-label="24-hour recording timeline">
        ${bars}
        ${nowMarker}
      </svg>
      <div style="display: flex; justify-content: space-between; color: var(--dim); font-size: 0.75rem; padding-left: 120px;">
        <span>24h ago</span><span>12h</span><span>now</span>
      </div>
    </div>
  `;
}

// ── Pipelines (W5 — read PluginRpc dispatch state from daemon) ────────
// Plugins that have a dedicated SPA sub-route. Clicking a node routes
// there; everything else goes to the plugin hub so users land on the
// catalog entry.
const PIPELINE_NODE_ROUTES = new Set([
  "crunchr", "archiver", "viewguard", "insights",
  "schedule-optimizer",
]);

async function renderPipelines() {
  let payload = { pipelines: [] };
  let recs = { recordings: [] };
  let runPayload = { runs: [] };
  try {
    [payload, recs, runPayload] = await Promise.all([
      API.pipelinesDag(),
      API.recordings().catch(() => ({ recordings: [] })),
      API.pipelineRuns().catch(() => ({ runs: [] })),
    ]);
  } catch (_) {}
  root.removeAttribute("aria-busy");
  const pipelines = payload.pipelines || [];
  const runs = (runPayload.runs || []).slice().reverse();
  // Cache finished recordings so the Run-on-… picker can list them.
  const finishedRecs = (recs.recordings || [])
    .filter((r) => stateClassName(r.state) === "finished" && r.file_exists !== false)
    .sort((a, b) => new Date(b.started_at) - new Date(a.started_at));

  const flow = (pipe) => {
    // Layout the nodes left→right by the topological order the server
    // shipped. Edges are encoded as " → " arrows between consecutive
    // nodes that actually connect, with a tag chip.
    const order = pipe.order && pipe.order.length ? pipe.order : pipe.nodes.map((n) => n.id);
    const nodeById = new Map(pipe.nodes.map((n) => [n.id, n]));
    const edges = pipe.edges || [];
    const edgeBetween = (a, b) => edges.find((e) => e.from === a && e.to === b);
    const cells = [];
    for (let i = 0; i < order.length; i++) {
      const node = nodeById.get(order[i]);
      if (!node) continue;
      const statusClass = node.status === "available" ? "is-avail" : "is-roadmap";
      const produces = (node.produces || [])
        .map((c) => `<span class="pl-cap pl-cap-produces">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      const consumes = (node.consumes || [])
        .map((c) => `<span class="pl-cap pl-cap-consumes">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      // Every node is a clickable anchor — routes to the plugin's own
      // sub-page when one exists, else to the plugin-hub catalog. The
      // hub upsell card (iter 26) handles the Pro-gate UX without us
      // having to know entitlement here.
      const href = PIPELINE_NODE_ROUTES.has(node.id)
        ? `#/plugins/${node.id}`
        : `#/plugins`;
      cells.push(`<a class="pl-node ${statusClass}" href="${htmlEscape(href)}"
          title="${htmlEscape(node.blurb)} · click to open ${htmlEscape(node.label)}"
          data-plugin="${htmlEscape(node.id)}">
          <div class="pl-node-head">
            <span class="pl-node-label">${htmlEscape(node.label)}</span>
            <span class="pl-node-status">${htmlEscape(node.status)}</span>
          </div>
          <div class="pl-node-caps">${consumes}${produces}</div>
        </a>`);
      const next = order[i + 1];
      if (next) {
        const eRec = edgeBetween(node.id, next);
        const viaLabel = eRec ? eRec.via.replace(/_/g, " ") : "";
        cells.push(`<div class="pl-arrow${eRec ? "" : " pl-arrow-loose"}" title="${htmlEscape(viaLabel)}">
          <span class="pl-arrow-line"></span>
          ${eRec ? `<span class="pl-arrow-via">${htmlEscape(viaLabel)}</span>` : ""}
          <span class="pl-arrow-tip">▸</span>
        </div>`);
      }
    }
    return cells.join("");
  };

  const cards = pipelines
    .map((p, idx) => {
      const totalNodes = (p.nodes || []).length;
      const availNodes = (p.nodes || []).filter((n) => n.status === "available").length;
      const pct = totalNodes === 0 ? 0 : Math.round((availNodes / totalNodes) * 100);
      return `
    <section class="cfg-card pl-pipe-card">
      <header class="pl-pipe-head">
        <h2 class="cfg-title">${htmlEscape(p.name)} <span class="pg-cap-hint">${htmlEscape(p.description)}</span></h2>
        <div class="pl-pipe-actions">
          <span class="pl-pipe-readiness ${pct === 100 ? "complete" : "partial"}"
                title="${availNodes} of ${totalNodes} stages available">
            ${availNodes}/${totalNodes} ready
          </span>
          <span class="pg-cap-hint">Blueprint</span>
        </div>
      </header>
      <div class="pl-pipe-bar"><span style="width:${pct}%"></span></div>
      <div class="pl-flow">${flow(p)}</div>
    </section>`;
    })
    .join("");

  root.innerHTML = chrome(`
    <h1 class="page-title">Pipelines</h1>
    <p class="page-subtitle">
      Durable, daemon-owned Creator workflows. Runs survive restarts, respect
      resource locks, retry with backoff, and stream every transition live.
    </p>
    <section class="cfg-card pl-pipe-card">
      <header class="pl-pipe-head">
        <div>
          <h2 class="cfg-title">Ultimate creator publish</h2>
          <p class="pg-cap-hint">Transcript intelligence + visual scene mining → captions, chapters, safety, highlights, clips, thumbnails → publish drafts + Casebook. Ten durable stages, real downloadable artifacts.</p>
        </div>
        <button class="btn-primary sm" id="pl-run-ultimate"
                ${finishedRecs.length === 0 ? "disabled title=\"No finished recordings available yet\"" : ""}>▶ New run</button>
      </header>
    </section>
    <section class="cfg-card">
      <h2 class="cfg-title">Runs <span class="pg-cap-hint">${runs.length} durable run${runs.length === 1 ? "" : "s"}</span></h2>
      <div class="pg-list">
        ${runs.slice(0, 20).map((run) => {
          const stages = run.stages || [];
          const done = stages.filter((stage) => stage.state === "Done" || stage.state === "Skipped").length;
          const state = typeof run.state === "string" ? run.state : Object.keys(run.state || {})[0] || "Pending";
          const failed = stages.find((stage) => stage.state && (stage.state.Exhausted || stage.state.Failed));
          const stageRows = stages.map((stage) => {
            const stageState = typeof stage.state === "string"
              ? stage.state
              : Object.keys(stage.state || {})[0] || "Pending";
            const artifacts = (stage.artifacts || []).map((artifact, artifactIndex) => {
              const path = String(artifact.path || "");
              const leaf = path.split(/[\\/]/).pop() || artifact.kind || "artifact";
              const href = `/api/v1/pipelines/runs/${encodeURIComponent(run.id)}/stages/${encodeURIComponent(stage.id)}/artifacts/${artifactIndex}`;
              return `<a class="pl-cap pl-cap-produces" href="${href}" download title="Download ${htmlEscape(path)}">${htmlEscape(artifact.kind || leaf)} · ${htmlEscape(leaf)}</a>`;
            }).join("");
            return `<div class="task-row">
              <div class="task-info">
                <span class="task-name">${htmlEscape(stage.name)}</span>
                <span class="task-cadence">${htmlEscape(stage.kind && stage.kind.Custom ? stage.kind.Custom : stageState)}</span>
                ${artifacts ? `<span class="pl-node-caps">${artifacts}</span>` : ""}
              </div>
              <span class="cfg-badge ${stageState === "Done" ? "ok" : stageState === "Exhausted" ? "bad" : ""}">${htmlEscape(stageState)}</span>
            </div>`;
          }).join("");
          return `<div class="pg-row pl-run-row" data-run-id="${htmlEscape(run.id)}">
            <div class="pg-row-main">
              <strong>${htmlEscape(run.name)}</strong>
              <span class="pg-cap-hint">${done}/${stages.length} stages · ${htmlEscape(run.trigger || "manual")}</span>
              ${failed ? `<span class="error">${htmlEscape((failed.state.Exhausted || failed.state.Failed).error || "stage failed")}</span>` : ""}
              <details class="pl-run-stages">
                <summary>${stages.length} stage${stages.length === 1 ? "" : "s"} · ${(stages.flatMap((stage) => stage.artifacts || [])).length} artifact${stages.flatMap((stage) => stage.artifacts || []).length === 1 ? "" : "s"}</summary>
                ${stageRows}
              </details>
            </div>
            <span class="cfg-badge ${state === "Done" ? "ok" : state === "Failed" ? "bad" : ""}">${htmlEscape(state)}</span>
            ${state === "Running" || state === "Pending" ? `<button class="sm pl-cancel" data-run="${htmlEscape(run.id)}">Cancel</button>` : ""}
            ${failed ? `<button class="sm pl-retry" data-stage="${htmlEscape(failed.id)}">Retry</button>` : ""}
          </div>`;
        }).join("") || '<div class="empty sm">No runs yet. Pick a finished recording to start the first workflow.</div>'}
      </div>
    </section>
    <h2 class="cfg-title">Workflow blueprints <span class="pg-cap-hint">trajectory and capability topology</span></h2>
    ${cards || '<div class="empty">No pipelines defined.</div>'}
  `);
  setupChromeHandlers();

  document.getElementById("pl-run-ultimate")?.addEventListener("click", () => {
    openRecordingPickerForPipeline({ name: "Ultimate creator publish", id: "creator_publish" }, finishedRecs);
  });
  document.querySelectorAll(".pl-cancel").forEach((button) => {
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await API.pipelineCancel(button.dataset.run);
        Toast.success("Pipeline cancellation requested");
        renderPipelines();
      } catch (error) {
        button.disabled = false;
        Toast.error(`Could not cancel pipeline: ${error.message}`);
      }
    });
  });
  document.querySelectorAll(".pl-retry").forEach((button) => {
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await API.pipelineRetryStage(button.dataset.stage);
        Toast.success("Stage re-queued");
        renderPipelines();
      } catch (error) {
        button.disabled = false;
        Toast.error(`Could not retry stage: ${error.message}`);
      }
    });
  });
}

function openRecordingPickerForPipeline(pipe, recs) {
  if (!recs.length) return;
  const overlay = ensureModalContainer("pl-run-picker");
  overlay.innerHTML = `
    <div class="modal-card pl-picker-card">
      <header class="pl-picker-head">
        <h2>Run "${htmlEscape(pipe.name)}" on a recording</h2>
        <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
      </header>
      <p class="pg-cap-hint">Pick a finished recording. The daemon queues the run immediately and keeps it durable across restarts.</p>
      <div class="pl-picker-list">
        ${recs.slice(0, 12).map((r) => `
          <button class="pl-picker-row" data-job-id="${htmlEscape(r.id)}" type="button">
            <span class="pl-picker-channel">${htmlEscape(r.channel_name || "(channel)")}</span>
            <span class="pl-picker-title">${htmlEscape(niceTitle(r.stream_title) || "(no title)")}</span>
            <span class="pl-picker-meta">${htmlEscape(new Date(r.started_at).toLocaleDateString())} · ${formatBytes(r.bytes_written || 0)}</span>
          </button>`).join("")}
      </div>
    </div>`;
  bumpModalOpen(+1);
  let closed = false;
  const escHandler = (e) => { if (e.key === "Escape") { e.preventDefault(); close(); } };
  const close = () => {
    if (closed) return;
    closed = true;
    overlay.remove();
    bumpModalOpen(-1);
    document.removeEventListener("keydown", escHandler, true);
  };
  // B2: capture-phase ESC so a nested modal opening inside this picker
  // can't eat the keystroke before we get to dismiss.
  document.addEventListener("keydown", escHandler, true);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });
  overlay.querySelector("[data-action=modal-close]").addEventListener("click", close);
  overlay.querySelectorAll(".pl-picker-row").forEach((row) => {
    row.addEventListener("click", async () => {
      const id = row.dataset.jobId;
      close();
      try {
        await API.pipelineRun(id, pipe.id || "creator_publish");
        Toast.success(`Queued ${pipe.name}`);
        renderPipelines();
      } catch (error) {
        Toast.error(`Pipeline failed to queue: ${error.message}`);
      }
    });
  });
}

// ── Plugins (W5 — mirror the TUI's Shift+P browser) ────────────────────
// Top-level Plugins route. Sub-routes select a plugin and its sub-views:
//   #/plugins                       → hub
//   #/plugins/crunchr               → transcribed-recordings list + search
//   #/plugins/crunchr/rec/<id>      → transcript + analysis
//   #/plugins/archiver              → archived channels
//   #/plugins/archiver/<channelId>  → channel catalog
//   #/plugins/viewguard             → fraud verdicts
//   #/plugins/insights              → word freq / topics / speakers
// Chat client route — Twitch IRC over anonymous WSS, multi-tab Chatterino-
// style layout. The room list comes from the backend (followed Twitch
// channels live first); each active tab opens its own WS that auto-
// reconnects on close. Filter chips run client-side via the same logic
// shape the host's strivo-chat crate uses.
const chatState = {
  rooms: [],
  active: null,           // active room name (Twitch login)
  buffers: {},            // room → { messages: [], unread, mentions, watched_user }
  sockets: {},            // room → WebSocket
  filters: [],            // [{ kind, needle?, user? }]
  watched_user: null,     // your own twitch login if known (mention highlight)
  paint_timer: null,
};
const CHAT_TWITCH_WS = "wss://irc-ws.chat.twitch.tv:443";
const CHAT_ANON_NICK = () => `justinfan${Math.floor(10000 + Math.random() * 89999)}`;
const CHAT_BUFFER_CAP = 500;

// Chat-paint fan-out. Each visible chat surface (the /chat route's body,
// the player view's right-rail) registers a painter so a single
// schedulePaintChat() updates every surface that's currently mounted.
// `mountedChatRooms` gates the route-leave socket teardown — a room
// still referenced by a painter (e.g. the rail open over /library)
// survives navigation.
const chatPaintListeners = new Set();
const mountedChatRooms = new Set();
function registerChatPainter(fn) {
  chatPaintListeners.add(fn);
  return () => chatPaintListeners.delete(fn);
}
function trackMountedChatRoom(room) {
  mountedChatRooms.add(room);
  return () => mountedChatRooms.delete(room);
}

// Persisted UI state for the /chat tabs panel collapse.
const CHAT_TABS_COLLAPSED_KEY = "strivo-chat-tabs-collapsed";
function loadChatTabsCollapsed() {
  try { return localStorage.getItem(CHAT_TABS_COLLAPSED_KEY) === "1"; } catch (_) { return false; }
}
function saveChatTabsCollapsed(v) {
  try { localStorage.setItem(CHAT_TABS_COLLAPSED_KEY, v ? "1" : "0"); } catch (_) {}
}

function chatPushMsg(room, msg) {
  const buf = chatState.buffers[room] ||= { messages: [], unread: 0, mentions: 0 };
  if (buf.messages.length >= CHAT_BUFFER_CAP) buf.messages.shift();
  buf.messages.push(msg);
  buf.unread += 1;
  if (chatState.watched_user && msgMentionsUser(msg.text, chatState.watched_user)) {
    buf.mentions += 1;
  }
  schedulePaintChat();
}
function msgMentionsUser(text, user) {
  const target = user.replace(/^@/, "").toLowerCase();
  return text.split(/\s+/).some((w) => {
    const cleaned = w.replace(/[.,!?]+$/, "");
    return cleaned.startsWith("@") && cleaned.slice(1).toLowerCase() === target;
  });
}
function schedulePaintChat() {
  if (chatState.paint_timer) return;
  chatState.paint_timer = setTimeout(() => {
    chatState.paint_timer = null;
    // Fan out to every registered chat surface. The /chat route + the
    // player rail each install their own painter so the firehose
    // updates both simultaneously.
    for (const fn of chatPaintListeners) {
      try { fn(); } catch (_) {}
    }
    // Backwards-compat: a /chat route mounted without registering a
    // painter (legacy code path) still gets its body painted directly.
    if (chatPaintListeners.size === 0 && document.getElementById("chat-body")) {
      try { paintChatBody(); } catch (_) {}
    }
  }, 50);
}
// Minimal client-side mirror of strivo-chat's parse_twitch_irc. We could
// round-trip through /plugins/chat/parse to use the host parser, but the
// WS firehose is high-rate and adding network latency per line would lag
// the live feed. Keep parity by reusing the host parser in batched
// previews (e.g. filter test on the recent buffer).
function parseTwitchIrc(line) {
  let rest = line.replace(/[\r\n]+$/, "");
  let tags = {};
  if (rest.startsWith("@")) {
    const sp = rest.indexOf(" ");
    if (sp < 0) return null;
    const raw = rest.slice(1, sp);
    for (const pair of raw.split(";")) {
      const eq = pair.indexOf("=");
      if (eq < 0) continue;
      tags[pair.slice(0, eq)] = pair.slice(eq + 1);
    }
    rest = rest.slice(sp + 1);
  }
  if (!rest.startsWith(":")) return null;
  const sp1 = rest.indexOf(" ");
  if (sp1 < 0) return null;
  const prefix = rest.slice(1, sp1);
  const sender = prefix.split("!")[0];
  rest = rest.slice(sp1 + 1);
  const sp2 = rest.indexOf(" ");
  if (sp2 < 0) return null;
  const verb = rest.slice(0, sp2);
  if (verb !== "PRIVMSG") return null;
  rest = rest.slice(sp2 + 1);
  const colon = rest.indexOf(" :");
  if (colon < 0) return null;
  const channel = rest.slice(0, colon).replace(/^#/, "");
  let text = rest.slice(colon + 2);
  let is_action = false;
  // Twitch wraps /me as CTCP \x01ACTION text\x01.
  const CTCP = String.fromCharCode(1);
  if (text.startsWith(CTCP) && text.endsWith(CTCP)) text = text.slice(1, -1);
  if (text.startsWith("ACTION ")) {
    text = text.slice("ACTION ".length);
    is_action = true;
  }
  // Badges with versions: 'subscriber/12,vip/1' → [{id:'subscriber',v:'12'},…]
  const badges = (tags["badges"] || "")
    .split(",")
    .filter(Boolean)
    .map((b) => {
      const [id, v] = b.split("/");
      return { id, version: v || "1" };
    });
  // Twitch native emote ranges: 'emote_id:start-end,start-end/emote_id:…'.
  // Parsed client-side mirroring strivo-chat::parse_twitch_emotes; the SPA
  // can't round-trip through the host parser cheaply on the live firehose.
  const emote_ranges = parseTwitchEmotes(tags["emotes"] || "");
  return {
    id: tags["id"] || `${channel}-${tags["tmi-sent-ts"] || Date.now()}`,
    room: channel,
    sender: tags["display-name"]?.replace(/\\s/g, " ") || sender,
    sender_color: tags["color"] || null,
    text,
    timestamp_ms: parseInt(tags["tmi-sent-ts"] || "0", 10),
    badges,
    emote_ranges,
    is_action,
    is_system: false,
    deleted: false,
  };
}

function parseTwitchEmotes(raw) {
  if (!raw) return [];
  const out = [];
  for (const group of raw.split("/")) {
    const colon = group.indexOf(":");
    if (colon < 0) continue;
    const id = group.slice(0, colon);
    for (const run of group.slice(colon + 1).split(",")) {
      const dash = run.indexOf("-");
      if (dash < 0) continue;
      const s = parseInt(run.slice(0, dash), 10);
      const e = parseInt(run.slice(dash + 1), 10);
      if (!isFinite(s) || !isFinite(e) || e < s) continue;
      out.push({ id, start: s, end: e });
    }
  }
  out.sort((a, b) => a.start - b.start);
  return out;
}

// BTTV global emotes — fetched once per session, keyed by emote code so
// the per-message tokenizer can substitute them inline. We don't pull
// channel-scoped BTTV/FFZ here (that needs the Twitch user id resolved
// at chat-join time; a future iter).
const bttvCache = { ready: false, map: new Map() };
async function ensureBttvGlobal() {
  if (bttvCache.ready) return bttvCache.map;
  try {
    const r = await fetch("https://api.betterttv.net/3/cached/emotes/global");
    if (!r.ok) throw new Error("bttv fetch failed");
    const list = await r.json();
    for (const e of list) {
      bttvCache.map.set(e.code, `https://cdn.betterttv.net/emote/${e.id}/1x`);
    }
  } catch (_) { /* graceful: chat works without BTTV */ }
  bttvCache.ready = true;
  return bttvCache.map;
}

// FFZ global emotes — same shape as BTTV. Endpoint:
// https://api.frankerfacez.com/v1/set/global
const ffzCache = { ready: false, map: new Map() };
async function ensureFfzGlobal() {
  if (ffzCache.ready) return ffzCache.map;
  try {
    const r = await fetch("https://api.frankerfacez.com/v1/set/global");
    if (!r.ok) throw new Error("ffz fetch failed");
    const j = await r.json();
    for (const setId of j.default_sets || []) {
      const set = (j.sets || {})[setId];
      if (!set) continue;
      for (const e of set.emoticons || []) {
        const url = (e.urls && (e.urls["1"] || e.urls["2"])) || "";
        if (url) {
          // FFZ urls are scheme-relative — coerce to https.
          const full = url.startsWith("//") ? `https:${url}` : url;
          ffzCache.map.set(e.name, full);
        }
      }
    }
  } catch (_) { /* graceful */ }
  ffzCache.ready = true;
  return ffzCache.map;
}

// 7TV global emotes. Endpoint: https://7tv.io/v3/emote-sets/global
const seventvCache = { ready: false, map: new Map() };
async function ensureSeventvGlobal() {
  if (seventvCache.ready) return seventvCache.map;
  try {
    const r = await fetch("https://7tv.io/v3/emote-sets/global");
    if (!r.ok) throw new Error("7tv fetch failed");
    const j = await r.json();
    for (const e of j.emotes || []) {
      const host = e.data?.host;
      if (!host) continue;
      // Prefer the smallest WebP for chat density. host.url is
      // scheme-relative.
      const file = (host.files || []).find((f) => f.name === "1x.webp")
        || (host.files || [])[0];
      if (!file) continue;
      const base = host.url.startsWith("//") ? `https:${host.url}` : host.url;
      seventvCache.map.set(e.name, `${base}/${file.name}`);
    }
  } catch (_) { /* graceful */ }
  seventvCache.ready = true;
  return seventvCache.map;
}

// Merge global third-party emote maps into one EmoteMap-shape Map.
// Precedence: Twitch native (in-message ranges) > BTTV > FFZ > 7TV.
async function ensureThirdPartyEmotes() {
  const [bttv, ffz, stv] = await Promise.all([
    ensureBttvGlobal(),
    ensureFfzGlobal(),
    ensureSeventvGlobal(),
  ]);
  const merged = new Map();
  // Lowest precedence first — later sets overwrite earlier ones.
  for (const [k, v] of stv.entries()) merged.set(k, v);
  for (const [k, v] of ffz.entries()) merged.set(k, v);
  for (const [k, v] of bttv.entries()) merged.set(k, v);
  return merged;
}

function connectChatRoom(room) {
  if (chatState.sockets[room]) return;
  // Kick off per-channel third-party fetches in the background —
  // channel-scoped emotes + sub badges. By the time the first PRIVMSG
  // arrives the caches are usually warm; the tokenizer falls back to
  // globals if not.
  const meta = (chatState.rooms || []).find((r) => r.room === room);
  if (meta?.user_id) {
    ensureChannelEmotes(meta.user_id).then(() => schedulePaintChat());
    ensureChannelBadges(meta.user_id).then(() => schedulePaintChat());
  }
  const ws = new WebSocket(CHAT_TWITCH_WS);
  chatState.sockets[room] = ws;
  ws.onopen = () => {
    ws.send("CAP REQ :twitch.tv/tags twitch.tv/commands");
    ws.send(`NICK ${CHAT_ANON_NICK()}`);
    ws.send(`JOIN #${room.toLowerCase()}`);
  };
  ws.onmessage = (ev) => {
    for (const line of ev.data.split(/\r?\n/)) {
      if (!line) continue;
      if (line.startsWith("PING ")) {
        try { ws.send(line.replace("PING", "PONG")); } catch (_) {}
        continue;
      }
      const m = parseTwitchIrc(line);
      if (m) chatPushMsg(room, m);
    }
  };
  ws.onclose = () => {
    delete chatState.sockets[room];
    // Auto-reconnect with backoff if room is still active.
    if (chatState.active === room) {
      setTimeout(() => connectChatRoom(room), 2500);
    }
  };
  ws.onerror = () => {
    try { ws.close(); } catch (_) {}
  };
}
function disconnectChatRoom(room) {
  const ws = chatState.sockets[room];
  if (ws) try { ws.close(); } catch (_) {}
  delete chatState.sockets[room];
}

function chatRoomMatchesFilters(msg) {
  for (const f of chatState.filters) {
    switch (f.kind) {
      case "keyword_in":
        if (!msg.text.toLowerCase().includes((f.needle || "").toLowerCase())) return false;
        break;
      case "keyword_out":
        if (msg.text.toLowerCase().includes((f.needle || "").toLowerCase())) return false;
        break;
      case "from_user":
        if (msg.sender.toLowerCase() !== (f.user || "").toLowerCase()) return false;
        break;
      case "no_links":
        if (msg.text.includes("http://") || msg.text.includes("https://")) return false;
        break;
      case "no_actions":
        if (msg.is_action) return false;
        break;
      case "mentions_user":
        if (!msgMentionsUser(msg.text, f.user || "")) return false;
        break;
    }
  }
  return true;
}

// Build the HTML for a single message — factored out so both the
// full-repaint and the append-only-diff path can call into it.
function chatMsgHtml(m) {
  const cls = `chat-msg${m.deleted ? " deleted" : ""}${m.is_action ? " action" : ""}`;
  const senderCol = m.sender_color ? `style="color:${htmlEscape(m.sender_color)}"` : "";
  const badges = renderChatBadges(m.badges || [], m.room);
  const tokens = renderChatTokens(m.text, m.emote_ranges || [], m.room);
  const mentioned = chatState.watched_user && msgMentionsUser(m.text, chatState.watched_user)
    ? " mentioned" : "";
  // data-mid lets the diff renderer map back from DOM to message
  // id without re-running filter logic on every repaint.
  return `<div class="${cls}${mentioned}" data-mid="${htmlEscape(m.id)}">
    ${badges}<span class="chat-sender" ${senderCol}>${htmlEscape(m.sender)}</span><span class="chat-sep">:</span> <span class="chat-text">${tokens}</span>
  </div>`;
}

// Mount a chat surface for `room` (Twitch login) into `parentBody`.
// Owns one connection's worth of paint lifecycle for the player rail.
// Reuses the global chatState buffers/sockets — multiple surfaces of
// the same room share the same firehose. Returns { teardown }.
function mountChatRail(parentBody, room) {
  if (!parentBody || !room) return { teardown() {} };
  parentBody.innerHTML = "";
  parentBody.dataset.room = room;

  // Warm caches in the background — chatMsgHtml's badge/emote
  // resolvers will rehydrate once these land.
  ensureGlobalBadges().then(() => schedulePaintChat());
  ensureThirdPartyEmotes().then(() => schedulePaintChat());
  const meta = (chatState.rooms || []).find((r) => r.room === room);
  if (meta?.user_id) {
    ensureChannelEmotes(meta.user_id).then(() => schedulePaintChat());
    ensureChannelBadges(meta.user_id).then(() => schedulePaintChat());
  }

  connectChatRoom(room);
  const unmountRoom = trackMountedChatRoom(room);

  // Mark this room as read while it's actively painted on the rail —
  // tab badges in /chat shouldn't count messages the user is already
  // staring at.
  if (chatState.buffers[room]) {
    chatState.buffers[room].unread = 0;
    chatState.buffers[room].mentions = 0;
  }

  const paint = () => {
    if (!parentBody.isConnected) return;
    const buf = chatState.buffers[room] || { messages: [] };
    const wasAtBottom = parentBody.scrollHeight - parentBody.scrollTop - parentBody.clientHeight < 80;
    const visible = buf.messages.filter(chatRoomMatchesFilters).slice(-200);
    const seen = new Set();
    for (const node of parentBody.children) {
      const id = node.dataset.mid;
      if (id) seen.add(id);
    }
    const fragments = [];
    for (const m of visible) {
      if (!seen.has(m.id)) fragments.push(chatMsgHtml(m));
    }
    if (fragments.length) {
      parentBody.insertAdjacentHTML("beforeend", fragments.join(""));
    }
    while (parentBody.childElementCount > visible.length) {
      parentBody.removeChild(parentBody.firstElementChild);
    }
    if (wasAtBottom) parentBody.scrollTop = parentBody.scrollHeight;
  };
  paint(); // initial
  const unsubPaint = registerChatPainter(paint);
  return {
    teardown() {
      unsubPaint();
      unmountRoom();
      // If no other surface still wants this room, drop the socket.
      // The /chat route's active room is exempt — it owns its own
      // reconnect loop and we don't want to fight it.
      if (!mountedChatRooms.has(room) && chatState.active !== room) {
        try { disconnectChatRoom(room); } catch (_) {}
        delete chatState.buffers[room];
      }
    },
  };
}

// Curated unicode emoji palette for the compose-bar's Emoji tab. Trimmed
// to the most-used per category — full Unicode CLDR is 3000+ glyphs which
// blows up the popup. Users typing Twitch chat reach for these the most.
const UNICODE_EMOJI_CATEGORIES = [
  { key: "smileys", label: "😀", title: "Smileys", emoji: ["😀","😃","😄","😁","😆","😅","🤣","😂","🙂","🙃","😉","😊","😇","🥰","😍","🤩","😘","😗","😚","😙","😋","😛","😜","🤪","😝","🤑","🤗","🤭","🤫","🤔","🤐","🤨","😐","😑","😶","😏","😒","🙄","😬","🤥","😌","😔","😪","🤤","😴","😷","🤒","🤕","🤢","🤮","🤧","🥵","🥶","🥴","😵","🤯","🤠","🥳","😎","🤓","🧐","😕","😟","🙁","☹","😮","😯","😲","😳","🥺","😦","😧","😨","😰","😥","😢","😭","😱","😖","😣","😞","😓","😩","😫","🥱","😤","😡","😠","🤬","😈","👿","💀","☠","💩","🤡","👹","👺","👻","👽","👾","🤖"] },
  { key: "gestures", label: "👋", title: "People & Gestures", emoji: ["👋","🤚","🖐","✋","🖖","👌","🤌","🤏","✌","🤞","🤟","🤘","🤙","👈","👉","👆","🖕","👇","☝","👍","👎","✊","👊","🤛","🤜","👏","🙌","👐","🤲","🤝","🙏","💪","🦾","🤳","💅","🤴","👸","🧑","👤","👥","👶","🧒","👦","👧","🧓","👴","👵"] },
  { key: "animals", label: "🐶", title: "Animals & Nature", emoji: ["🐶","🐱","🐭","🐹","🐰","🦊","🐻","🐼","🐨","🐯","🦁","🐮","🐷","🐽","🐸","🐵","🙈","🙉","🙊","🐒","🐔","🐧","🐦","🐤","🐣","🐥","🦆","🦅","🦉","🦇","🐺","🐗","🐴","🦄","🐝","🐛","🦋","🐌","🐞","🐜","🪳","🪰","🪲","🐢","🐍","🦎","🦂","🦀","🦑","🐙","🦐","🐠","🐟","🐡","🐬","🦈","🐳","🐋","🐊","🐅","🐆","🦓","🦍","🦧","🐘","🦛","🦏","🐪","🐫","🦒","🦘","🐃","🐂","🐄","🐎","🐖","🐏","🐑","🦙","🐐","🦌","🐕","🐩","🦮","🐈","🐓","🦃","🦚","🦜","🦢","🦩","🕊","🐇","🦝","🦨","🦡","🦦","🦥","🐁","🐀","🌲","🌳","🌴","🌱","🌵","🌾","🌿","☘","🍀","🍁","🍂","🍃","🌷","🌹","🥀","🌺","🌸","🌼","🌻","🌞","🌝","🌛","🌜","🌚","🌕","🌖","🌗","🌘","🌑","🌒","🌓","🌔","🌙","🌎","🌍","🌏","🪐","💫","⭐","🌟","✨","⚡","☄","💥","🔥","🌪","🌈","☀","🌤","⛅","🌥","🌦","🌧","⛈","🌩","🌨","❄","☃","⛄","🌬","💨","💧","💦","☔","☂","🌊","🌫"] },
  { key: "food", label: "🍔", title: "Food & Drink", emoji: ["🍏","🍎","🍐","🍊","🍋","🍌","🍉","🍇","🍓","🫐","🍈","🍒","🍑","🥭","🍍","🥥","🥝","🍅","🍆","🥑","🥦","🥬","🥒","🌶","🫑","🌽","🥕","🧄","🧅","🥔","🍠","🥐","🥯","🍞","🥖","🥨","🧀","🥚","🍳","🧈","🥞","🧇","🥓","🥩","🍗","🍖","🦴","🌭","🍔","🍟","🍕","🥪","🥙","🧆","🌮","🌯","🫔","🥗","🥘","🫕","🥫","🍝","🍜","🍲","🍛","🍣","🍱","🥟","🦪","🍤","🍙","🍚","🍘","🍥","🥠","🥮","🍢","🍡","🍧","🍨","🍦","🥧","🧁","🍰","🎂","🍮","🍭","🍬","🍫","🍿","🍩","🍪","🌰","🥜","🍯","🥛","🍼","☕","🫖","🍵","🍶","🍾","🍷","🍸","🍹","🍺","🍻","🥂","🥃","🥤","🧋","🧃","🧉","🧊"] },
  { key: "activities", label: "⚽", title: "Activities", emoji: ["⚽","🏀","🏈","⚾","🥎","🎾","🏐","🏉","🥏","🎱","🪀","🏓","🏸","🏒","🏑","🥍","🏏","🪃","🥅","⛳","🪁","🏹","🎣","🤿","🥊","🥋","🎽","🛹","🛼","🛷","⛸","🥌","🎿","⛷","🏂","🪂","🏋","🤼","🤸","⛹","🤺","🤾","🏌","🏇","🧘","🏄","🏊","🤽","🚣","🧗","🚵","🚴","🏆","🥇","🥈","🥉","🏅","🎖","🏵","🎗","🎫","🎟","🎪","🤹","🎭","🩰","🎨","🎬","🎤","🎧","🎼","🎹","🥁","🪘","🎷","🎺","🪗","🎸","🪕","🎻","🎲","♟","🎯","🎳","🎮","🎰","🧩"] },
  { key: "objects", label: "💻", title: "Objects", emoji: ["⌚","📱","📲","💻","⌨","🖥","🖨","🖱","🖲","🕹","🗜","💽","💾","💿","📀","📼","📷","📸","📹","🎥","📽","🎞","📞","☎","📟","📠","📺","📻","🎙","🎚","🎛","🧭","⏱","⏲","⏰","🕰","⌛","⏳","📡","🔋","🔌","💡","🔦","🕯","🪔","🧯","🛢","💸","💵","💴","💶","💷","🪙","💰","💳","💎","⚖","🪜","🧰","🪛","🔧","🔨","⚒","🛠","⛏","🪚","🔩","⚙","🪤","🧱","⛓","🧲","🔫","💣","🧨","🪓","🔪","🗡","⚔","🛡","🚬","⚰","🪦","⚱","🏺","🔮","📿","🧿","💈","⚗","🔭","🔬","🕳","🩹","🩺","💊","💉","🩸","🧬","🦠","🧫","🧪","🌡","🧹","🪠","🧺","🧻","🚽","🚰","🚿","🛁","🛀","🧼","🪥","🪒","🧽","🪣","🧴","🛎","🔑","🗝","🚪","🪑","🛋","🛏","🛌","🧸","🪆","🖼","🪞","🪟","🛍","🛒","🎁","🎈","🎏","🎀","🪄","🪅","🎊","🎉","🎎","🏮","🎐","🧧","✉","📩","📨","📧","💌","📥","📤","📦","🏷","🪧","📪","📫","📬","📭","📮","📯","📜","📃","📄","📑","🧾","📊","📈","📉","🗒","🗓","📆","📅","🗑","📇","🗃","🗳","🗄","📋","📁","📂","🗂","🗞","📰","📓","📔","📒","📕","📗","📘","📙","📚","📖","🔖","🧷","🔗","📎","🖇","📐","📏","🧮","📌","📍","✂","🖊","🖋","✒","🖌","🖍","📝","✏","🔍","🔎","🔏","🔐","🔒","🔓"] },
  { key: "symbols", label: "❤", title: "Symbols", emoji: ["❤","🧡","💛","💚","💙","💜","🤎","🖤","🤍","💔","❣","💕","💞","💓","💗","💖","💘","💝","💟","☮","✝","☪","🕉","☸","✡","🔯","🕎","☯","☦","🛐","⛎","♈","♉","♊","♋","♌","♍","♎","♏","♐","♑","♒","♓","🆔","⚛","🉑","☢","☣","📴","📳","🈶","🈚","🈸","🈺","🈷","✴","🆚","💮","🉐","㊙","㊗","🈴","🈵","🈹","🈲","🅰","🅱","🆎","🆑","🅾","🆘","❌","⭕","🛑","⛔","📛","🚫","💯","💢","♨","🚷","🚯","🚳","🚱","🔞","📵","🚭","❗","❕","❓","❔","‼","⁉","🔅","🔆","〽","⚠","🚸","🔱","⚜","🔰","♻","✅","🈯","💹","❇","✳","❎","🌐","💠","Ⓜ","🌀","💤","🏧","🚾","♿","🅿","🛗","🈳","🈂","🛂","🛃","🛄","🛅","🚹","🚺","🚼","⚧","🚻","🚮","🎦","📶","🈁","🔣","ℹ","🔤","🔡","🔠","🆖","🆗","🆙","🆒","🆕","🆓","0️⃣","1️⃣","2️⃣","3️⃣","4️⃣","5️⃣","6️⃣","7️⃣","8️⃣","9️⃣","🔟","🔢","#️⃣","*️⃣","⏏","▶","⏸","⏯","⏹","⏺","⏭","⏮","⏩","⏪","⏫","⏬","◀","🔼","🔽","➡","⬅","⬆","⬇","↗","↘","↙","↖","↕","↔","↪","↩","⤴","⤵","🔀","🔁","🔂","🔄","🔃","🎵","🎶","➕","➖","➗","✖","♾","💲","💱","™","©","®","〰","➰","➿","🔚","🔙","🔛","🔝","🔜","✔","☑","🔘","🔴","🟠","🟡","🟢","🔵","🟣","⚫","⚪","🟤","🔺","🔻","🔸","🔹","🔶","🔷","🔳","🔲","▪","▫","◾","◽","◼","◻","⬛","⬜","🟥","🟧","🟨","🟩","🟦","🟪","🟫","🔈","🔇","🔉","🔊","🔔","🔕","📣","📢","💬","💭","🗯","♠","♣","♥","♦","🃏","🎴","🀄","🕐","🕑","🕒","🕓","🕔","🕕","🕖","🕗","🕘","🕙","🕚","🕛","🕜","🕝","🕞","🕟","🕠","🕡","🕢","🕣","🕤","🕥","🕦","🕧"] },
];

// Slash-command quick-pick for the compose bar. Twitch-flavoured today;
// the YouTube path is moderator-only and goes through the Live Chat
// Messages API (separate plumbing), so we surface the same labels and
// let the backend route or reject based on platform.
const CHAT_SLASH_COMMANDS = [
  { cmd: "/me ", label: "/me", hint: "Italicized action message" },
  { cmd: "/announce ", label: "/announce", hint: "Channel announcement (mods)" },
  { cmd: "/timeout ", label: "/timeout", hint: "/timeout <user> <secs> [reason]" },
  { cmd: "/ban ", label: "/ban", hint: "/ban <user> [reason]" },
  { cmd: "/unban ", label: "/unban", hint: "/unban <user>" },
  { cmd: "/mod ", label: "/mod", hint: "/mod <user>" },
  { cmd: "/unmod ", label: "/unmod", hint: "/unmod <user>" },
  { cmd: "/vip ", label: "/vip", hint: "/vip <user>" },
  { cmd: "/unvip ", label: "/unvip", hint: "/unvip <user>" },
  { cmd: "/clear", label: "/clear", hint: "Clear chat (mods)" },
  { cmd: "/slow ", label: "/slow", hint: "/slow <secs>" },
  { cmd: "/slowoff", label: "/slowoff", hint: "Disable slow mode" },
  { cmd: "/subscribers", label: "/subscribers", hint: "Subs-only mode on" },
  { cmd: "/subscribersoff", label: "/subscribersoff", hint: "Subs-only mode off" },
  { cmd: "/emoteonly", label: "/emoteonly", hint: "Emote-only mode on" },
  { cmd: "/emoteonlyoff", label: "/emoteonlyoff", hint: "Emote-only mode off" },
  { cmd: "/followers ", label: "/followers", hint: "/followers <duration>" },
  { cmd: "/followersoff", label: "/followersoff", hint: "Disable followers-only" },
  { cmd: "/raid ", label: "/raid", hint: "/raid <channel>" },
  { cmd: "/unraid", label: "/unraid", hint: "Cancel raid" },
  { cmd: "/shoutout ", label: "/shoutout", hint: "/shoutout <user>" },
  { cmd: "/color ", label: "/color", hint: "/color <hex|name>" },
];

// Chatterino-style compose bar. Owns its textarea, emote palette, slash-
// command menu, char counter, send button. Per-platform accent driven by
// the `data-platform` attribute on the root element — Twitch purple,
// YouTube red, Patreon coral, all swap when setRoom() is called with a
// new (room, platform) pair. Returns a controller object.
function mountChatCompose(parent, opts = {}) {
  if (!parent) return { setRoom() {}, teardown() {} };
  const root = document.createElement("div");
  root.className = "chat-compose-bar";
  root.dataset.platform = opts.platform || "none";
  root.dataset.compact = opts.compact ? "1" : "0";
  root.innerHTML = `
    <div class="cc-popup cc-emote-popup" hidden>
      <div class="cc-popup-tabs" role="tablist">
        <button class="cc-popup-tab active" data-tab="channel" type="button" title="Channel emotes (BTTV / FFZ / 7TV)">⭐</button>
        <button class="cc-popup-tab" data-tab="global" type="button" title="Global emotes">🌐</button>
        <button class="cc-popup-tab" data-tab="emoji" type="button" title="Unicode emoji">😀</button>
      </div>
      <div class="cc-popup-search">
        <input type="text" class="cc-emote-search" placeholder="Search emotes…" autocomplete="off" />
      </div>
      <div class="cc-popup-body cc-emote-body" role="listbox"></div>
    </div>
    <div class="cc-popup cc-cmd-popup" hidden>
      <div class="cc-popup-search">
        <input type="text" class="cc-cmd-search" placeholder="Filter commands…" autocomplete="off" />
      </div>
      <div class="cc-popup-body cc-cmd-body" role="listbox"></div>
    </div>
    <div class="cc-row">
      <button class="cc-tool cc-emote-btn" type="button" title="Emotes (Ctrl+E)" aria-label="Open emote palette" aria-haspopup="true">😀</button>
      <button class="cc-tool cc-cmd-btn" type="button" title="Slash commands (Ctrl+/)" aria-label="Open commands" aria-haspopup="true">/</button>
      <textarea class="cc-input" rows="1" maxlength="500"
                placeholder="${opts.placeholderRoom ? `Send a message to #${opts.placeholderRoom}` : "Send a message…"}"
                aria-label="Chat message"></textarea>
      <div class="cc-meta">
        <span class="cc-count" aria-live="polite">0</span>
        <button class="cc-send" type="button" title="Send (Enter)" aria-label="Send message">▶</button>
      </div>
    </div>
    <span class="cc-hint pg-cap-hint" aria-live="polite"></span>
  `;
  parent.appendChild(root);

  const input = root.querySelector(".cc-input");
  const sendBtn = root.querySelector(".cc-send");
  const count = root.querySelector(".cc-count");
  const hint = root.querySelector(".cc-hint");
  const emoteBtn = root.querySelector(".cc-emote-btn");
  const cmdBtn = root.querySelector(".cc-cmd-btn");
  const emotePop = root.querySelector(".cc-emote-popup");
  const cmdPop = root.querySelector(".cc-cmd-popup");
  const emoteBody = root.querySelector(".cc-emote-body");
  const emoteSearch = root.querySelector(".cc-emote-search");
  const cmdBody = root.querySelector(".cc-cmd-body");
  const cmdSearch = root.querySelector(".cc-cmd-search");
  const emoteTabs = root.querySelectorAll(".cc-popup-tab");

  let currentRoom = opts.room || null;
  let currentPlatform = opts.platform || "none";
  let activeEmoteTab = "channel";
  const channelEmoteMap = new Map(); // populated on setRoom
  const globalEmoteMap = new Map();  // populated lazily

  // Char counter & textarea auto-grow.
  const updateCount = () => {
    const n = input.value.length;
    count.textContent = String(n);
    count.classList.toggle("near-cap", n > 400);
    count.classList.toggle("at-cap", n >= 500);
  };
  const autogrow = () => {
    input.style.height = "auto";
    // Cap at ~6 lines so the rail compose doesn't crowd the messages.
    const max = root.dataset.compact === "1" ? 100 : 160;
    input.style.height = `${Math.min(max, input.scrollHeight)}px`;
  };
  const insertAtCursor = (text) => {
    const start = input.selectionStart ?? input.value.length;
    const end = input.selectionEnd ?? input.value.length;
    const before = input.value.slice(0, start);
    const after = input.value.slice(end);
    // Pad with spaces if the surroundings aren't whitespace, so emote
    // codes are sliced cleanly by Twitch's parser.
    const padBefore = before && !/\s$/.test(before) ? " " : "";
    const padAfter = after && !/^\s/.test(after) ? " " : "";
    input.value = `${before}${padBefore}${text}${padAfter}${after}`;
    const caret = before.length + padBefore.length + text.length + padAfter.length;
    input.setSelectionRange(caret, caret);
    input.focus();
    updateCount();
    autogrow();
  };

  // Send.
  const submit = async () => {
    const text = input.value.trim();
    if (!text || !currentRoom) {
      hint.textContent = currentRoom ? "Empty message" : "No room selected";
      return;
    }
    sendBtn.disabled = true;
    try {
      await API.chatSend(currentRoom, text);
      input.value = "";
      updateCount();
      autogrow();
      hint.textContent = "";
    } catch (err) {
      hint.textContent = (err && err.message) || "Send failed";
    } finally {
      sendBtn.disabled = false;
      input.focus();
    }
  };
  sendBtn.addEventListener("click", submit);
  input.addEventListener("input", () => { updateCount(); autogrow(); });
  input.addEventListener("keydown", (e) => {
    // Enter sends; Shift+Enter inserts newline. Familiar everywhere.
    if (e.key === "Enter" && !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey) {
      e.preventDefault();
      submit();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "e") {
      e.preventDefault();
      togglePopup(emotePop, emoteBtn);
    } else if ((e.ctrlKey || e.metaKey) && e.key === "/") {
      e.preventDefault();
      togglePopup(cmdPop, cmdBtn);
    } else if (e.key === "Escape") {
      hidePopup(emotePop, emoteBtn);
      hidePopup(cmdPop, cmdBtn);
    }
  });

  // Popup helpers — one popup open at a time; outside-click closes both.
  const showPopup = (pop, btn) => {
    pop.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    // Hide the other popup if open.
    const other = pop === emotePop ? cmdPop : emotePop;
    const otherBtn = pop === emotePop ? cmdBtn : emoteBtn;
    hidePopup(other, otherBtn);
    if (pop === emotePop) {
      renderEmoteBody();
      setTimeout(() => emoteSearch.focus(), 0);
    } else {
      renderCmdBody();
      setTimeout(() => cmdSearch.focus(), 0);
    }
  };
  const hidePopup = (pop, btn) => {
    pop.hidden = true;
    btn.setAttribute("aria-expanded", "false");
  };
  const togglePopup = (pop, btn) => {
    if (pop.hidden) showPopup(pop, btn);
    else { hidePopup(pop, btn); input.focus(); }
  };
  emoteBtn.addEventListener("click", () => togglePopup(emotePop, emoteBtn));
  cmdBtn.addEventListener("click", () => togglePopup(cmdPop, cmdBtn));
  const onDocClick = (e) => {
    if (!root.contains(e.target)) {
      hidePopup(emotePop, emoteBtn);
      hidePopup(cmdPop, cmdBtn);
    }
  };
  document.addEventListener("click", onDocClick);

  // Emote-popup tab switching.
  emoteTabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      emoteTabs.forEach((t) => t.classList.toggle("active", t === tab));
      activeEmoteTab = tab.dataset.tab;
      renderEmoteBody();
    });
  });
  emoteSearch.addEventListener("input", renderEmoteBody);

  function renderEmoteBody() {
    const q = (emoteSearch.value || "").trim().toLowerCase();
    const items = [];
    if (activeEmoteTab === "channel") {
      for (const [code, url] of channelEmoteMap.entries()) {
        if (!q || code.toLowerCase().includes(q)) {
          items.push({ code, url, label: code });
        }
      }
      if (items.length === 0 && !q) {
        emoteBody.innerHTML = `<div class="cc-popup-empty pg-cap-hint">
          ${currentRoom ? "No channel emotes — BTTV / FFZ / 7TV haven't returned yet, or this channel has none." : "Pick a stream to load channel emotes."}
        </div>`;
        return;
      }
    } else if (activeEmoteTab === "global") {
      for (const [code, url] of globalEmoteMap.entries()) {
        if (!q || code.toLowerCase().includes(q)) {
          items.push({ code, url, label: code });
        }
      }
    } else {
      // Unicode emoji tab.
      const inSearch = q.length > 0;
      for (const cat of UNICODE_EMOJI_CATEGORIES) {
        for (const e of cat.emoji) {
          if (!inSearch || cat.title.toLowerCase().includes(q)) {
            items.push({ code: e, url: null, label: cat.title });
          }
        }
      }
    }
    items.sort((a, b) => a.code.localeCompare(b.code));
    if (items.length > 240) items.length = 240; // sane cap for popup grid
    emoteBody.innerHTML = items.map((it) => {
      if (it.url) {
        return `<button class="cc-emote-cell" type="button" data-insert="${htmlEscape(it.code)}" title="${htmlEscape(it.code)}">
          <img loading="lazy" src="${htmlEscape(it.url)}" alt="${htmlEscape(it.code)}"></button>`;
      }
      return `<button class="cc-emote-cell cc-emoji-cell" type="button" data-insert="${htmlEscape(it.code)}" title="${htmlEscape(it.label)}">${it.code}</button>`;
    }).join("");
    emoteBody.querySelectorAll(".cc-emote-cell").forEach((cell) => {
      cell.addEventListener("click", () => {
        insertAtCursor(cell.dataset.insert);
      });
    });
  }

  cmdSearch.addEventListener("input", renderCmdBody);
  function renderCmdBody() {
    const q = (cmdSearch.value || "").trim().toLowerCase();
    const items = CHAT_SLASH_COMMANDS.filter((c) =>
      !q || c.label.toLowerCase().includes(q) || c.hint.toLowerCase().includes(q),
    );
    cmdBody.innerHTML = items.map((c) => `
      <button class="cc-cmd-cell" type="button" data-cmd="${htmlEscape(c.cmd)}">
        <span class="cc-cmd-label">${htmlEscape(c.label)}</span>
        <span class="cc-cmd-hint pg-cap-hint">${htmlEscape(c.hint)}</span>
      </button>`).join("");
    cmdBody.querySelectorAll(".cc-cmd-cell").forEach((cell) => {
      cell.addEventListener("click", () => {
        // Replace any leading slash command on the line with this one,
        // else insert at the start.
        const cur = input.value;
        const lineStart = cur.lastIndexOf("\n") + 1;
        const head = cur.slice(0, lineStart);
        const tail = cur.slice(lineStart);
        const cleaned = tail.replace(/^\/\S*\s*/, "");
        input.value = `${head}${cell.dataset.cmd}${cleaned}`;
        input.focus();
        const pos = head.length + cell.dataset.cmd.length;
        input.setSelectionRange(pos, pos);
        hidePopup(cmdPop, cmdBtn);
        updateCount();
        autogrow();
      });
    });
  }

  // setRoom — swap room / platform; refresh palette caches.
  async function setRoom(room, platform) {
    currentRoom = room || null;
    currentPlatform = (platform || "none").toLowerCase();
    root.dataset.platform = currentPlatform;
    input.placeholder = room ? `Send a message to #${room}` : "Send a message…";
    input.disabled = !room;
    sendBtn.disabled = !room;
    channelEmoteMap.clear();
    if (!room) {
      renderEmoteBody();
      return;
    }
    // Prime the global emote map once; persist across room swaps.
    if (globalEmoteMap.size === 0) {
      try {
        const m = await ensureThirdPartyEmotes();
        for (const [k, v] of m.entries()) globalEmoteMap.set(k, v);
      } catch (_) {}
    }
    // Per-channel BTTV/FFZ/7TV.
    const meta = (chatState.rooms || []).find((r) => r.room === room);
    if (meta?.user_id) {
      try {
        const m = await ensureChannelEmotes(meta.user_id);
        for (const [k, v] of m.entries()) channelEmoteMap.set(k, v);
      } catch (_) {}
    }
    renderEmoteBody();
  }

  // Initial state.
  updateCount();
  autogrow();
  if (currentRoom) setRoom(currentRoom, currentPlatform);
  else { input.disabled = true; sendBtn.disabled = true; }

  return {
    setRoom,
    teardown() {
      document.removeEventListener("click", onDocClick);
      root.remove();
    },
    focus() { input.focus(); },
  };
}

// Stable, deterministic hue for a channel name (Twitch login) so
// letter-avatars on the rail keep the same colour every render.
function chatAvatarHue(name) {
  const s = String(name || "");
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h) % 360;
}

function paintChatBody(opts = {}) {
  const body = document.getElementById("chat-body");
  if (!body) return;
  const room = chatState.active;
  if (!room) return;
  const buf = chatState.buffers[room] || { messages: [] };
  const wasAtBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 80;
  const visible = buf.messages.filter(chatRoomMatchesFilters).slice(-200);

  // Full-repaint path: room switch, filter change, or first paint.
  // Marked by the missing data-room attribute or an explicit
  // {full: true} request.
  const needsFull = opts.full
    || body.dataset.room !== room
    || !body.firstElementChild
    || body.childElementCount > visible.length + 20; // sanity: drifted too far
  if (needsFull) {
    body.dataset.room = room;
    body.innerHTML = visible.map(chatMsgHtml).join("");
    if (wasAtBottom) body.scrollTop = body.scrollHeight;
    paintChatTabs();
    return;
  }

  // Diff path: append messages whose id isn't already on the DOM,
  // trim front when we exceed the 200-window, leave existing nodes
  // untouched so the browser doesn't repaint / re-decode emote
  // images. This is what kills the flicker the user reported.
  const seen = new Set();
  for (const node of body.children) {
    const id = node.dataset.mid;
    if (id) seen.add(id);
  }
  const fragments = [];
  for (const m of visible) {
    if (!seen.has(m.id)) fragments.push(chatMsgHtml(m));
  }
  if (fragments.length) {
    body.insertAdjacentHTML("beforeend", fragments.join(""));
  }
  // Trim from the front so the DOM matches the 200-message window.
  while (body.childElementCount > visible.length) {
    body.removeChild(body.firstElementChild);
  }
  if (wasAtBottom) body.scrollTop = body.scrollHeight;
  paintChatTabs();
}

// Twitch badge resolution. The unauthenticated badges.twitch.tv
// endpoint returns a full set/version → image_url map for the
// platform's global badges (broadcaster/moderator/vip/subscriber etc.).
// Channel-scoped sub badges still need an authenticated /helix/chat/
// badges call against the broadcaster's id — left for a follow-up iter.
const badgeCache = { ready: false, map: new Map() }; // key "<id>/<ver>" → image url
async function ensureGlobalBadges() {
  if (badgeCache.ready) return badgeCache.map;
  try {
    const r = await fetch("https://badges.twitch.tv/v1/badges/global/display?language=en");
    if (!r.ok) throw new Error("badge fetch failed");
    const j = await r.json();
    for (const [id, set] of Object.entries(j.badge_sets || {})) {
      for (const [ver, def] of Object.entries(set.versions || {})) {
        const url = def.image_url_1x || def.image_url_2x || def.image_url_4x;
        if (url) badgeCache.map.set(`${id}/${ver}`, url);
      }
    }
  } catch (_) { /* graceful */ }
  badgeCache.ready = true;
  return badgeCache.map;
}
// Channel-scoped third-party emote sets — fetched once per session
// per channel. Keyed by twitch user_id; result map is room-specific
// and consulted by the tokenizer alongside the global maps. The
// classic "subscriber emote shows up as plain text" symptom is
// channel-scoped data falling through; these closures fix it.
const channelEmoteCache = {}; // user_id → Map<code, url>
async function ensureChannelEmotes(userId) {
  if (!userId) return new Map();
  if (channelEmoteCache[userId]) return channelEmoteCache[userId];
  const merged = new Map();
  // BTTV channel — /3/cached/users/twitch/<id>
  try {
    const r = await fetch(`https://api.betterttv.net/3/cached/users/twitch/${userId}`);
    if (r.ok) {
      const j = await r.json();
      for (const e of [...(j.channelEmotes || []), ...(j.sharedEmotes || [])]) {
        merged.set(e.code, `https://cdn.betterttv.net/emote/${e.id}/1x`);
      }
    }
  } catch (_) { /* graceful */ }
  // 7TV channel — /v3/users/twitch/<id>
  try {
    const r = await fetch(`https://7tv.io/v3/users/twitch/${userId}`);
    if (r.ok) {
      const j = await r.json();
      const set = j.emote_set;
      for (const e of (set?.emotes || [])) {
        const host = e.data?.host;
        if (!host) continue;
        const file = (host.files || []).find((f) => f.name === "1x.webp") || (host.files || [])[0];
        if (!file) continue;
        const base = host.url.startsWith("//") ? `https:${host.url}` : host.url;
        merged.set(e.name, `${base}/${file.name}`);
      }
    }
  } catch (_) { /* graceful */ }
  channelEmoteCache[userId] = merged;
  return merged;
}

// Channel-scoped badge set (subscriber tiers, bits, founder, …).
// Uses the legacy unauthenticated endpoint so OAuth isn't required.
const channelBadgeCache = {}; // user_id → Map<"id/ver", url>
async function ensureChannelBadges(userId) {
  if (!userId) return new Map();
  if (channelBadgeCache[userId]) return channelBadgeCache[userId];
  const map = new Map();
  try {
    const r = await fetch(`https://badges.twitch.tv/v1/badges/channels/${userId}/display?language=en`);
    if (r.ok) {
      const j = await r.json();
      for (const [id, set] of Object.entries(j.badge_sets || {})) {
        for (const [ver, def] of Object.entries(set.versions || {})) {
          const url = def.image_url_1x || def.image_url_2x || def.image_url_4x;
          if (url) map.set(`${id}/${ver}`, url);
        }
      }
    }
  } catch (_) { /* graceful */ }
  channelBadgeCache[userId] = map;
  return map;
}

function renderChatBadges(badges, room) {
  const channelBadges = room && chatState.rooms
    ? channelBadgeCache[chatState.rooms.find((r) => r.room === room)?.user_id] || null
    : null;
  return badges.map((b) => {
    const key = `${b.id}/${b.version}`;
    const url = (channelBadges && channelBadges.get(key)) || badgeCache.map.get(key);
    if (url) {
      return `<img class="chat-badge-img" alt="${htmlEscape(b.id)}" title="${htmlEscape(b.id)}/${htmlEscape(b.version)}" src="${htmlEscape(url)}">`;
    }
    return `<span class="chat-badge">${htmlEscape(b.id)}</span>`;
  }).join("");
}

// Token renderer with Twitch emote-range overlay + BTTV global emote
// substitution. Mirrors strivo-chat::tokenize_text_with_ranges so a
// future host parser switch keeps the same shape.
function renderChatTokens(text, ranges = [], room = null) {
  const channelEmotes = room && chatState.rooms
    ? channelEmoteCache[chatState.rooms.find((r) => r.room === room)?.user_id] || null
    : null;
  // Helper that classifies a single whitespace-split run.
  const classifyRun = (run) => {
    if (!run) return "";
    if (run.startsWith("@")) {
      const user = run.replace(/[.,!?]+$/, "").slice(1);
      if (/^[A-Za-z0-9_]+$/.test(user)) {
        return `<span class="chat-mention">@${htmlEscape(user)}</span>`;
      }
    }
    if (/^https?:\/\//.test(run)) {
      return `<a class="chat-link" href="${htmlEscape(run)}" target="_blank" rel="noopener noreferrer">${htmlEscape(run)}</a>`;
    }
    // Third-party emote precedence: channel-scoped wins over globals
    // so a channel's subscriber-only emote always renders.
    // Then BTTV → FFZ → 7TV globally. First hit wins.
    const tpUrl = (channelEmotes && channelEmotes.get(run))
      || bttvCache.map.get(run)
      || ffzCache.map.get(run)
      || seventvCache.map.get(run);
    if (tpUrl) {
      return `<img class="chat-emote" loading="lazy" alt="${htmlEscape(run)}" title="${htmlEscape(run)}" src="${htmlEscape(tpUrl)}">`;
    }
    return htmlEscape(run);
  };
  // Plain text path when there are no Twitch emote ranges.
  const renderPlain = (s) =>
    s.split(/(\s+)/).map((p) => /^\s+$/.test(p) ? p : classifyRun(p)).join("");
  if (!ranges.length) return renderPlain(text);
  // Twitch ranges are in CODE-POINT indices, not byte offsets. Walk by
  // chars so multi-byte codepoints (emoji-prefixed messages) stay aligned.
  const chars = Array.from(text);
  const out = [];
  let cursor = 0;
  for (const r of ranges) {
    if (r.start >= chars.length) continue;
    if (r.start > cursor) out.push(renderPlain(chars.slice(cursor, r.start).join("")));
    const end = Math.min(r.end + 1, chars.length);
    const name = chars.slice(r.start, end).join("");
    const url = `https://static-cdn.jtvnw.net/emoticons/v2/${r.id}/default/dark/1.0`;
    out.push(`<img class="chat-emote" loading="lazy" alt="${htmlEscape(name)}" title="${htmlEscape(name)}" src="${htmlEscape(url)}">`);
    cursor = end;
  }
  if (cursor < chars.length) out.push(renderPlain(chars.slice(cursor).join("")));
  return out.join("");
}

function paintChatTabs() {
  const tabs = document.getElementById("chat-tabs");
  if (!tabs) return;
  tabs.innerHTML = chatState.rooms.map((r) => {
    const buf = chatState.buffers[r.room] || { unread: 0, mentions: 0 };
    const active = r.room === chatState.active ? "active" : "";
    const mentionPill = buf.mentions > 0 ? `<span class="chat-tab-mentions">${buf.mentions}</span>` : "";
    const unreadPill = (buf.unread > 0 && r.room !== chatState.active)
      ? `<span class="chat-tab-unread">${buf.unread}</span>` : "";
    const liveDot = r.is_live ? `<span class="chat-tab-live" title="live">◉</span>` : "";
    const offline = r.connectable === false ? " offline" : "";
    return `<button class="chat-tab ${active}${offline}" data-room="${htmlEscape(r.room)}" ${!r.connectable ? "disabled" : ""}>
      ${liveDot}<span class="chat-tab-name">${htmlEscape(r.display_name)}</span>${mentionPill}${unreadPill}
    </button>`;
  }).join("");
  tabs.querySelectorAll(".chat-tab").forEach((t) => {
    t.addEventListener("click", () => {
      const room = t.dataset.room;
      switchChatRoom(room);
    });
  });
}

function switchChatRoom(room) {
  if (room === chatState.active) return;
  chatState.active = room;
  // Mark prior room as read (snapshot unread counter is preserved in buf,
  // but the badge clears on switch).
  if (chatState.buffers[room]) {
    chatState.buffers[room].unread = 0;
    chatState.buffers[room].mentions = 0;
  }
  connectChatRoom(room);
  paintChatTabs();
  paintChatBody({ full: true });
  const activeLabel = document.getElementById("chat-main-active");
  if (activeLabel) activeLabel.textContent = `#${room}`;
  // Swap the compose bar's emote palette + platform accent to follow
  // the new tab.
  if (chatState.compose) {
    const platform = (chatState.rooms.find((r) => r.room === room)?.platform || "twitch").toLowerCase();
    chatState.compose.setRoom(room, platform);
  }
}

async function renderChat() {
  const tabsCollapsed = loadChatTabsCollapsed();
  chatState.tabsCollapsed = tabsCollapsed;
  root.innerHTML = chrome(`
    <div id="chat-root" class="chat-root${tabsCollapsed ? " tabs-collapsed" : ""}">
      <aside id="chat-tabs" class="chat-tabs" role="tablist"></aside>
      <main class="chat-main">
        <div class="chat-main-head">
          <button class="sm chat-tabs-toggle" id="chat-tabs-toggle"
                  type="button" aria-pressed="${tabsCollapsed ? "true" : "false"}"
                  title="${tabsCollapsed ? "Show channel list" : "Hide channel list"}">
            ${tabsCollapsed ? "☰ Channels" : "◀ Hide channels"}
          </button>
          <span class="chat-main-active pg-cap-hint" id="chat-main-active"></span>
        </div>
        <div class="chat-filters">
          <input id="chat-filter-kw" type="text" placeholder="filter: contains…" />
          <input id="chat-filter-out" type="text" placeholder="filter: hide…" />
          <label class="chat-filter-tog"><input type="checkbox" id="chat-no-links"> no links</label>
          <label class="chat-filter-tog"><input type="checkbox" id="chat-no-actions"> no /me</label>
        </div>
        <div id="chat-body" class="chat-body" role="log" aria-live="polite"></div>
        <div id="chat-compose-host" class="chat-compose-host"></div>
      </main>
    </div>
  `);

  // Tabs toggle — collapses the left channel list. Useful when the
  // window is narrow or the user knows which channel they want and
  // wants more room for messages.
  document.getElementById("chat-tabs-toggle")?.addEventListener("click", () => {
    chatState.tabsCollapsed = !chatState.tabsCollapsed;
    saveChatTabsCollapsed(chatState.tabsCollapsed);
    const rootEl = document.getElementById("chat-root");
    if (rootEl) rootEl.classList.toggle("tabs-collapsed", chatState.tabsCollapsed);
    const btn = document.getElementById("chat-tabs-toggle");
    if (btn) {
      btn.textContent = chatState.tabsCollapsed ? "☰ Channels" : "◀ Hide channels";
      btn.title = chatState.tabsCollapsed ? "Show channel list" : "Hide channel list";
      btn.setAttribute("aria-pressed", chatState.tabsCollapsed ? "true" : "false");
    }
  });

  // Register this route's body as a chat paint surface. Persists for
  // the lifetime of the route; the painter self-unregisters when the
  // chat-body DOM is gone (route navigation removed it).
  const routeUnsub = registerChatPainter(() => {
    if (!document.getElementById("chat-body")) { routeUnsub(); return; }
    paintChatBody();
  });
  // Kick off the third-party emote fetches (BTTV + FFZ + 7TV) in the
  // background; we don't await because the chat firehose should never
  // block on a third party. Caches are merged on demand in the
  // tokenizer via mergedEmoteMap().
  ensureThirdPartyEmotes().then(() => schedulePaintChat());
  // Twitch global badge images — same pattern. No auth required.
  ensureGlobalBadges().then(() => schedulePaintChat());
  let rooms;
  try {
    rooms = (await API.chatRooms()).rooms || [];
  } catch (e) {
    document.getElementById("chat-root").innerHTML =
      `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`;
    return;
  }
  // Live first, then alpha.
  rooms.sort((a, b) => {
    if (a.is_live !== b.is_live) return a.is_live ? -1 : 1;
    return a.display_name.localeCompare(b.display_name);
  });
  chatState.rooms = rooms;
  // Default to the first connectable room.
  const first = rooms.find((r) => r.connectable);
  if (first) chatState.active = first.room;
  paintChatTabs();
  const activeLabel = document.getElementById("chat-main-active");
  if (activeLabel) {
    const r = rooms.find((x) => x.room === chatState.active);
    activeLabel.textContent = r ? `#${r.room}` : "";
  }
  if (chatState.active) {
    connectChatRoom(chatState.active);
    paintChatBody({ full: true });
  } else {
    document.getElementById("chat-body").innerHTML =
      `<div class="empty"><div class="glyph">💬</div>
        <p>No Twitch channels followed yet. Add some in Settings → Channels.</p>
        <p class="pg-cap-hint">YouTube live chat needs an OAuth flow — coming soon.</p></div>`;
  }
  // Filter inputs.
  const applyFilters = () => {
    const kw = document.getElementById("chat-filter-kw").value.trim();
    const out = document.getElementById("chat-filter-out").value.trim();
    const noLinks = document.getElementById("chat-no-links").checked;
    const noActions = document.getElementById("chat-no-actions").checked;
    chatState.filters = [];
    if (kw) chatState.filters.push({ kind: "keyword_in", needle: kw });
    if (out) chatState.filters.push({ kind: "keyword_out", needle: out });
    if (noLinks) chatState.filters.push({ kind: "no_links" });
    if (noActions) chatState.filters.push({ kind: "no_actions" });
    paintChatBody({ full: true });
  };
  document.getElementById("chat-filter-kw").addEventListener("input", applyFilters);
  document.getElementById("chat-filter-out").addEventListener("input", applyFilters);
  document.getElementById("chat-no-links").addEventListener("change", applyFilters);
  document.getElementById("chat-no-actions").addEventListener("change", applyFilters);

  // Chatterino-style compose bar. Mounted once; setRoom() swaps the
  // platform accent + per-channel emote palette whenever the user
  // clicks a different tab. The widget owns its own send call,
  // emote/command popups, and char counter — no per-route plumbing
  // beyond the room-change hook below.
  const composeHost = document.getElementById("chat-compose-host");
  const composePlatform = (chatState.rooms.find((r) => r.room === chatState.active)?.platform || "twitch").toLowerCase();
  chatState.compose = mountChatCompose(composeHost, {
    room: chatState.active,
    platform: composePlatform,
    compact: false,
  });
}

// Data viz / analytics route — research-grade aggregation +
// experiment runner over a corpus of transcribed recordings.
// User picks recordings to assemble a corpus, picks an experiment,
// SPA POSTs to /dataviz/run and renders the returned Series as a
// chart (bar / line / treemap). Pure SVG renderer — no library
// dependency.
const DATAVIZ_EXPERIMENTS = [
  { kind: "word_frequency", label: "Top words", body: { kind: "word_frequency", top_n: 30 } },
  { kind: "speaker_time", label: "Speaker minutes", body: { kind: "speaker_time" } },
  { kind: "speaker_episode_count", label: "Speaker appearances", body: { kind: "speaker_episode_count" } },
  { kind: "episodes_per_month", label: "Episodes per month", body: { kind: "episodes_per_month" } },
  { kind: "episode_durations", label: "Episode durations", body: { kind: "episode_durations" } },
  { kind: "speaker_cooccurrence", label: "Speaker co-occurrence", body: { kind: "speaker_cooccurrence" } },
];

let datavizState = {
  selectedIds: new Set(),
  series: null,
  experimentKind: "word_frequency",
};

async function renderDataviz() {
  const recs = (await API.recordings().catch(() => ({ recordings: [] }))).recordings || [];
  const finished = recs.filter((r) => r.state === "Finished");
  const expBtns = DATAVIZ_EXPERIMENTS.map((e) =>
    `<button class="sm dz-exp ${datavizState.experimentKind === e.kind ? "active" : ""}" data-kind="${e.kind}" type="button">${htmlEscape(e.label)}</button>`
  ).join("");
  const list = finished.map((r) => `
    <label class="dz-rec-row">
      <input type="checkbox" class="dz-rec-pick" data-id="${htmlEscape(r.id)}" ${datavizState.selectedIds.has(r.id) ? "checked" : ""}/>
      <span class="dz-rec-title">${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</span>
      <span class="dz-rec-meta pg-cap-hint">${htmlEscape((r.started_at || "").slice(0, 10))} · ${htmlEscape(r.platform || "")}</span>
    </label>`).join("");
  root.innerHTML = chrome(`
    <h1 class="page-title">📊 Data viz</h1>
    <p class="page-subtitle">Aggregate transcribed/diarised recordings, run experiments, render charts. Pick recordings → run experiment → swap chart type. All runs are local; no telemetry.</p>
    <div class="dz-grid">
      <section class="cfg-card dz-corpus">
        <h2 class="cfg-title">Corpus <span class="pg-cap-hint">${finished.length} finished recording${finished.length === 1 ? "" : "s"} eligible</span></h2>
        <div class="dz-corpus-actions">
          <button class="sm" id="dz-select-all" type="button">Select all</button>
          <button class="sm" id="dz-select-none" type="button">Clear</button>
          <span class="pg-cap-hint" id="dz-count">0 selected</span>
        </div>
        <div class="dz-rec-list">${list || '<div class="empty sm">No finished recordings to analyse yet.</div>'}</div>
      </section>
      <section class="cfg-card dz-exp-card">
        <h2 class="cfg-title">Experiments</h2>
        <div class="dz-exp-buttons">${expBtns}</div>
        <button class="btn-primary" id="dz-run" type="button">▶ Run experiment</button>
        <p class="pg-cap-hint">Crunchr transcripts feed the corpus — make sure each recording has been transcribed first (open it from /recordings → ⓘ Info → Generate subtitles).</p>
      </section>
      <section class="cfg-card dz-chart-card">
        <h2 class="cfg-title" id="dz-chart-title">Result</h2>
        <div id="dz-chart" class="dz-chart"></div>
      </section>
    </div>
  `);
  setupChromeHandlers();
  const updateCount = () => {
    document.getElementById("dz-count").textContent =
      `${datavizState.selectedIds.size} selected`;
  };
  updateCount();
  root.querySelectorAll(".dz-rec-pick").forEach((cb) => {
    cb.addEventListener("change", () => {
      if (cb.checked) datavizState.selectedIds.add(cb.dataset.id);
      else datavizState.selectedIds.delete(cb.dataset.id);
      updateCount();
    });
  });
  document.getElementById("dz-select-all").addEventListener("click", () => {
    root.querySelectorAll(".dz-rec-pick").forEach((cb) => { cb.checked = true; datavizState.selectedIds.add(cb.dataset.id); });
    updateCount();
  });
  document.getElementById("dz-select-none").addEventListener("click", () => {
    root.querySelectorAll(".dz-rec-pick").forEach((cb) => { cb.checked = false; });
    datavizState.selectedIds.clear();
    updateCount();
  });
  root.querySelectorAll(".dz-exp").forEach((btn) => {
    btn.addEventListener("click", () => {
      datavizState.experimentKind = btn.dataset.kind;
      root.querySelectorAll(".dz-exp").forEach((b) => b.classList.toggle("active", b === btn));
    });
  });
  document.getElementById("dz-run").addEventListener("click", async (ev) => {
    if (datavizState.selectedIds.size === 0) { Toast.error("Pick at least one recording first"); return; }
    const exp = DATAVIZ_EXPERIMENTS.find((e) => e.kind === datavizState.experimentKind);
    await withBusy(ev.currentTarget, "Fetching transcripts…", async () => {
      // Build the Corpus client-side from each recording's Crunchr
      // transcript. Skip recordings that have no transcript yet.
      const episodes = [];
      for (const id of datavizState.selectedIds) {
        const r = finished.find((x) => x.id === id);
        const tr = await API.crunchrTranscript(id);
        if (!tr || !tr.utterances) continue;
        episodes.push({
          id,
          title: niceTitle(r?.stream_title) || r?.channel_name || id.slice(0, 8),
          date: r?.started_at || "",
          utterances: tr.utterances.map((u) => ({
            speaker: u.speaker || "Speaker",
            text: u.text || "",
            start_sec: u.start_sec || 0,
            end_sec: u.end_sec || (u.start_sec || 0) + 1,
          })),
        });
      }
      if (episodes.length === 0) {
        Toast.error("None of the selected recordings have Crunchr transcripts yet");
        return;
      }
      const corpus = { label: "selection", episodes };
      const resp = await API.datavizRun(corpus, exp.body);
      datavizState.series = resp.series;
      document.getElementById("dz-chart-title").textContent = resp.series.label;
      renderDatavizChart(resp.series, document.getElementById("dz-chart"));
      Toast.success(`Ran ${exp.label} over ${episodes.length} episode(s)`);
    }).catch((err) => Toast.error(err.message || "Run failed"));
  });
  // Re-render chart on resize so the SVG scales. Listener stored on
  // datavizState so a route change can tear it down — without this
  // each /dataviz visit added a permanent handler (P0 perf #1).
  if (datavizState.resizeHandler) {
    window.removeEventListener("resize", datavizState.resizeHandler);
  }
  datavizState.resizeHandler = () => {
    if (datavizState.series) renderDatavizChart(datavizState.series, document.getElementById("dz-chart"));
  };
  window.addEventListener("resize", datavizState.resizeHandler, { passive: true });
}

// Called from the router when leaving /plugins/dataviz so the resize
// handler doesn't accumulate across navigations.
function teardownDataviz() {
  if (datavizState && datavizState.resizeHandler) {
    window.removeEventListener("resize", datavizState.resizeHandler);
    datavizState.resizeHandler = null;
  }
}

// Pure SVG bar / line / treemap renderer. Takes a Series, emits SVG
// straight into the host element. No external dependency.
function renderDatavizChart(series, host) {
  if (!host || !series) return;
  const points = series.points || [];
  if (!points.length) { host.innerHTML = `<div class="empty sm">No data points</div>`; return; }
  const w = Math.max(400, host.clientWidth || 800);
  const h = 360;
  const max = Math.max(...points.map((p) => p.value));
  const accent = "var(--accent, #b07cff)";
  if (series.chart_hint === "treemap") {
    // Greedy slice-and-dice — single-row layout proportional to value.
    const total = points.reduce((a, p) => a + p.value, 0) || 1;
    let x = 0;
    const cells = points.map((p) => {
      const cw = (p.value / total) * w;
      const cell = `<g transform="translate(${x},0)">
        <rect width="${cw}" height="${h}" fill="${accent}" fill-opacity="${0.3 + 0.6 * (p.value / max)}"/>
        <text x="6" y="20" fill="#fff" font-size="12">${htmlEscape(p.label)}</text>
        <text x="6" y="36" fill="#fff" font-size="11" opacity="0.7">${p.value.toFixed(0)}</text>
      </g>`;
      x += cw;
      return cell;
    }).join("");
    host.innerHTML = `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${cells}</svg>`;
    return;
  }
  if (series.chart_hint === "line") {
    const stepX = w / Math.max(1, points.length - 1);
    const path = points.map((p, i) => `${i === 0 ? "M" : "L"}${(i * stepX).toFixed(1)},${(h - (p.value / max) * (h - 40) - 20).toFixed(1)}`).join(" ");
    const dots = points.map((p, i) =>
      `<circle cx="${(i * stepX).toFixed(1)}" cy="${(h - (p.value / max) * (h - 40) - 20).toFixed(1)}" r="3" fill="${accent}"><title>${htmlEscape(p.label)} · ${p.value.toFixed(1)}</title></circle>`
    ).join("");
    const axis = points.map((p, i) =>
      `<text x="${(i * stepX).toFixed(1)}" y="${h - 4}" fill="#fff" opacity="0.5" font-size="9" text-anchor="middle">${htmlEscape(p.label)}</text>`
    ).join("");
    host.innerHTML = `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
      <path d="${path}" stroke="${accent}" stroke-width="2" fill="none"/>
      ${dots}${axis}
    </svg>`;
    return;
  }
  // Default: horizontal bar chart.
  const rowH = Math.max(18, Math.min(36, Math.floor((h - 20) / points.length)));
  const ww = Math.max(400, w);
  const labelW = 180;
  const barMaxW = ww - labelW - 80;
  const svgH = points.length * rowH + 20;
  const rows = points.map((p, i) => {
    const bw = max > 0 ? (p.value / max) * barMaxW : 0;
    return `<g transform="translate(0,${i * rowH + 10})">
      <text x="0" y="${rowH * 0.65}" fill="#fff" font-size="12" opacity="0.85">${htmlEscape(p.label)}</text>
      <rect x="${labelW}" y="${rowH * 0.2}" width="${bw}" height="${rowH * 0.6}" fill="${accent}" rx="2"/>
      <text x="${labelW + bw + 6}" y="${rowH * 0.65}" fill="#fff" font-size="11" opacity="0.7">${p.value.toFixed(p.value < 10 ? 1 : 0)}</text>
    </g>`;
  }).join("");
  host.innerHTML = `<svg width="${ww}" height="${svgH}" viewBox="0 0 ${ww} ${svgH}">${rows}</svg>`;
}

// ── Archive route (CE-Fusion F4/F5) ───────────────────────────────────
//
// The creator-vocabulary surface over the research kernel: search the
// transcribed archive, review moments (codings + high-confidence
// detections), and jump straight into the Editor at a hit's timecode.
// Strategy §6 vocabulary: Project → "workspace", Source → "recording",
// Coding/Signal → "moment" (never "coding"/"corpus" in this page's copy).
let archiveState = {
  projects: [],
  projectId: null,
  sourcesById: new Map(), // source_id -> strivo_research::Source (has recording_id)
  query: "",
  hits: [],
  hitsOffset: 0,
  hitsLimit: 20,
  hitsHasMore: false,
  moments: [],
  momentsOffset: 0,
  momentsLimit: 20,
  momentsHasMore: false,
  minConfidence: "",
  searchTimer: null,
  surfaceCleanup: null, // cleanup fn returned by a mounted Coding Studio surface (codebook/corpus/notebook)
};

async function renderArchive() {
  let projects;
  try {
    projects = (await API.researchProjects()).projects || [];
  } catch (e) {
    root.removeAttribute("aria-busy");
    if (e.message && e.message.includes("unauthorized")) return;
    if (e.code === 402) {
      root.innerHTML = chrome(
        `${pluginHeader("Archive", "Search your transcribed recordings and review moments.")}<div id="arc-upsell-host"></div>`,
      );
      setupChromeHandlers();
      const host = document.getElementById("arc-upsell-host");
      const plugin = e.plugin || "crunchr";
      const licence = await API.licenceStatus().catch(() => null);
      host.innerHTML = renderProUpsell(plugin, licence);
      wireProUpsell(host, plugin);
      return;
    }
    renderArchiveFailure(e);
    return;
  }
  archiveState.projects = projects;

  const remembered = localStorage.getItem("strivo-archive-project");
  if (remembered && projects.some((p) => p.id === remembered)) {
    archiveState.projectId = remembered;
  } else if (!archiveState.projectId || !projects.some((p) => p.id === archiveState.projectId)) {
    archiveState.projectId = projects[0]?.id || null;
  }

  if (!archiveState.projectId) {
    root.removeAttribute("aria-busy");
    paintArchiveBootstrap();
    return;
  }

  let detail;
  try {
    detail = (await API.researchProjectDetail(archiveState.projectId)).project;
  } catch (e) {
    root.removeAttribute("aria-busy");
    renderArchiveFailure(e);
    return;
  }
  archiveState.sourcesById = new Map((detail.sources || []).map((s) => [s.id, s]));
  root.removeAttribute("aria-busy");
  paintArchiveWorkspace();
}

function teardownArchive() {
  if (archiveState.searchTimer) {
    clearTimeout(archiveState.searchTimer);
    archiveState.searchTimer = null;
  }
  if (typeof archiveState.surfaceCleanup === "function") {
    try { archiveState.surfaceCleanup(); } catch (_) { /* best-effort teardown */ }
  }
  archiveState.surfaceCleanup = null;
}

// Sub-tab strip for the Archive route: the built-in Search/Moments view
// plus every Coding Studio surface registered in RESEARCH_SURFACES
// (codebook.js / corpus.js / notebook.js). Guarded with typeof so this
// still no-ops harmlessly if ever reached before the registry exists —
// build.rs strips the whole @creator-start block from the PVR bundle,
// and "archive" is a CREATOR_ROUTES-gated route so that block is never
// actually missing when this runs.
function archiveTabStripHtml(activeSlug) {
  const surfaces = typeof RESEARCH_SURFACES !== "undefined" ? RESEARCH_SURFACES : {};
  const tabs = [{ slug: "search", label: "Search" }, ...Object.entries(surfaces).map(([slug, s]) => ({ slug, label: s.label }))];
  if (tabs.length <= 1) return "";
  return `<nav class="pro-tabs" role="tablist">${tabs.map((t) => `
    <a class="pro-tab ${t.slug === activeSlug ? "is-active" : ""}" href="#/archive/${t.slug}">${htmlEscape(t.label)}</a>`).join("")}</nav>`;
}

// Builds the ctx object handed to a mounted Coding Studio surface's
// mount(root, ctx). Modules must not reach into spa.js internals beyond
// this contract (see assets/research/README.md).
function archiveSurfaceCtx() {
  return {
    api: API,
    projectId: archiveState.projectId,
    toast: { success: (msg) => Toast.success(msg), error: (msg) => Toast.error(msg) },
    fmt: { escapeHtml: htmlEscape, clock: fmtClock, parseTime: parseTimeInput },
  };
}

function renderArchiveFailure(e) {
  root.innerHTML = chrome(`
    ${pluginHeader("Archive", "Search your transcribed recordings and review moments.")}
    <div class="empty">
      <div class="glyph">⚠</div>
      <p>${htmlEscape(e.message || "Couldn't load the archive.")}</p>
      <button class="sm" id="arc-retry" type="button">Retry</button>
    </div>
  `);
  setupChromeHandlers();
  document.getElementById("arc-retry")?.addEventListener("click", () => renderArchive().catch(() => {}));
}

function paintArchiveBootstrap() {
  root.innerHTML = chrome(`
    ${pluginHeader("Archive", "Search your transcribed recordings and review moments — the creator view over the research kernel.")}
    <div class="cfg-card arc-bootstrap">
      <h2 class="cfg-title">No archive workspace yet</h2>
      <p class="pg-cap-hint">A workspace holds the recordings, transcripts, and moments pulled from your archive. Create one to get started.</p>
      <button class="btn-primary" id="arc-create-workspace" type="button">＋ Create archive workspace</button>
    </div>
  `);
  setupChromeHandlers();
  document.getElementById("arc-create-workspace")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Creating…", async () => {
      const resp = await API.researchCreateProject("Archive", "Default archive workspace");
      archiveState.projectId = resp.project.id;
      localStorage.setItem("strivo-archive-project", resp.project.id);
      Toast.success("Archive workspace created");
      await renderArchive();
    }).catch((err) => Toast.error(`Couldn't create workspace: ${err.message}`));
  });
}

function paintArchiveWorkspace() {
  const parts = routeParts(); // ["archive", <subtab?>]
  const surfaces = typeof RESEARCH_SURFACES !== "undefined" ? RESEARCH_SURFACES : {};
  const subtab = parts[1] && surfaces[parts[1]] ? parts[1] : "search";

  if (archiveState.surfaceCleanup) {
    try { archiveState.surfaceCleanup(); } catch (_) { /* best-effort teardown */ }
    archiveState.surfaceCleanup = null;
  }

  if (subtab !== "search") {
    paintArchiveSurface(subtab, surfaces[subtab]);
    return;
  }

  const sources = [...archiveState.sourcesById.values()];
  const hasSources = sources.length > 0;
  const selector = archiveState.projects.length > 1 ? `
    <label class="arc-ws-picker">
      Workspace
      <select id="arc-ws-select" class="arc-select" aria-label="Archive workspace">
        ${archiveState.projects.map((p) => `<option value="${htmlEscape(p.id)}" ${p.id === archiveState.projectId ? "selected" : ""}>${htmlEscape(p.name)}</option>`).join("")}
      </select>
    </label>` : "";

  root.innerHTML = chrome(`
    ${pluginHeader("Archive", "Search your transcribed recordings and review moments.")}
    ${archiveTabStripHtml(subtab)}
    <div class="arc-toolbar">
      ${selector}
      <button class="sm" id="arc-index-btn" type="button" title="Pull transcripts and detections from Crunchr / cuepoints / Viewguard into this workspace">🔎 Index my archive</button>
      <button class="sm" id="arc-add-moment-btn" type="button">＋ Add moment</button>
    </div>
    <div id="arc-index-report"></div>
    ${hasSources ? "" : `
      <div class="empty arc-empty-sources">
        <div class="glyph">🗄</div>
        <p>This workspace has no recordings indexed yet.</p>
        <p class="pg-cap-hint">Click “Index my archive” above to pull transcripts and detections in.</p>
      </div>`}
    <div class="arc-grid">
      <section class="cfg-card arc-search-card">
        <h2 class="cfg-title">Search</h2>
        <div class="arc-search-row">
          <input id="arc-search-q" class="grid-filter" type="search"
                 placeholder="Search your archive… (Enter to search)"
                 aria-label="Search archive" value="${htmlEscape(archiveState.query)}"/>
          <button class="sm" id="arc-search-btn" type="button">Search</button>
        </div>
        <div id="arc-hits" aria-live="polite"></div>
        <div class="arc-pager" id="arc-hits-pager" hidden>
          <button class="sm" id="arc-hits-prev" type="button">← Prev</button>
          <span id="arc-hits-page-label" class="pg-cap-hint"></span>
          <button class="sm" id="arc-hits-next" type="button">Next →</button>
        </div>
      </section>
      <section class="cfg-card arc-moments-card">
        <h2 class="cfg-title">Moments</h2>
        <div class="arc-moments-filter">
          <label for="arc-min-conf">Min confidence</label>
          <input id="arc-min-conf" class="arc-input" type="number" min="0" max="1" step="0.05"
                 placeholder="any" value="${htmlEscape(archiveState.minConfidence)}"/>
          <button class="sm" id="arc-moments-apply" type="button">Apply</button>
        </div>
        <div id="arc-moments" aria-live="polite"></div>
        <div class="arc-pager" id="arc-moments-pager" hidden>
          <button class="sm" id="arc-moments-prev" type="button">← Prev</button>
          <span id="arc-moments-page-label" class="pg-cap-hint"></span>
          <button class="sm" id="arc-moments-next" type="button">Next →</button>
        </div>
      </section>
    </div>
  `);
  setupChromeHandlers();

  document.getElementById("arc-ws-select")?.addEventListener("change", (e) => {
    archiveState.projectId = e.target.value;
    localStorage.setItem("strivo-archive-project", archiveState.projectId);
    archiveState.hits = [];
    archiveState.hitsOffset = 0;
    archiveState.moments = [];
    archiveState.momentsOffset = 0;
    root.setAttribute("aria-busy", "true");
    renderArchive().catch(() => {});
  });

  document.getElementById("arc-index-btn")?.addEventListener("click", (e) => runArchiveIndex(e.currentTarget));
  document.getElementById("arc-add-moment-btn")?.addEventListener("click", () => openAddMomentModal());

  const qInput = document.getElementById("arc-search-q");
  const runSearch = () => {
    archiveState.query = qInput.value;
    archiveState.hitsOffset = 0;
    loadArchiveHits();
  };
  document.getElementById("arc-search-btn")?.addEventListener("click", runSearch);
  qInput?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); runSearch(); }
  });
  qInput?.addEventListener("input", () => {
    if (archiveState.searchTimer) clearTimeout(archiveState.searchTimer);
    archiveState.searchTimer = setTimeout(runSearch, 400);
  });

  document.getElementById("arc-hits-prev")?.addEventListener("click", () => {
    archiveState.hitsOffset = Math.max(0, archiveState.hitsOffset - archiveState.hitsLimit);
    loadArchiveHits();
  });
  document.getElementById("arc-hits-next")?.addEventListener("click", () => {
    archiveState.hitsOffset += archiveState.hitsLimit;
    loadArchiveHits();
  });

  document.getElementById("arc-moments-apply")?.addEventListener("click", () => {
    archiveState.minConfidence = document.getElementById("arc-min-conf").value.trim();
    archiveState.momentsOffset = 0;
    loadArchiveMoments();
  });
  document.getElementById("arc-moments-prev")?.addEventListener("click", () => {
    archiveState.momentsOffset = Math.max(0, archiveState.momentsOffset - archiveState.momentsLimit);
    loadArchiveMoments();
  });
  document.getElementById("arc-moments-next")?.addEventListener("click", () => {
    archiveState.momentsOffset += archiveState.momentsLimit;
    loadArchiveMoments();
  });

  if (archiveState.query.trim()) loadArchiveHits();
  else paintArchiveHitsEmpty("Type a query and press Enter to search your archive.");
  loadArchiveMoments();
}

// Paints a Coding Studio surface (codebook/corpus/notebook) into the
// Archive route's chrome, alongside the same workspace picker the
// Search tab uses, and hands mount() the ctx contract from
// assets/research/README.md.
function paintArchiveSurface(slug, surface) {
  const selector = archiveState.projects.length > 1 ? `
    <label class="arc-ws-picker">
      Workspace
      <select id="arc-ws-select" class="arc-select" aria-label="Archive workspace">
        ${archiveState.projects.map((p) => `<option value="${htmlEscape(p.id)}" ${p.id === archiveState.projectId ? "selected" : ""}>${htmlEscape(p.name)}</option>`).join("")}
      </select>
    </label>` : "";

  root.innerHTML = chrome(`
    ${pluginHeader("Archive", "Search your transcribed recordings and review moments.")}
    ${archiveTabStripHtml(slug)}
    <div class="arc-toolbar">${selector}</div>
    <div id="arc-surface-mount" class="arc-surface-mount"></div>
  `);
  setupChromeHandlers();

  document.getElementById("arc-ws-select")?.addEventListener("change", (e) => {
    archiveState.projectId = e.target.value;
    localStorage.setItem("strivo-archive-project", archiveState.projectId);
    root.setAttribute("aria-busy", "true");
    renderArchive().catch(() => {});
  });

  const mountRoot = document.getElementById("arc-surface-mount");
  if (!mountRoot || !surface || typeof surface.mount !== "function") return;
  try {
    const cleanup = surface.mount(mountRoot, archiveSurfaceCtx());
    archiveState.surfaceCleanup = typeof cleanup === "function" ? cleanup : null;
  } catch (e) {
    console.error(`Archive surface "${slug}" failed to mount`, e);
    mountRoot.innerHTML = `<div class="empty sm"><div class="glyph">⚠</div>This surface couldn't load: ${htmlEscape(e.message || String(e))}</div>`;
  }
}

async function runArchiveIndex(btn) {
  const reportHost = document.getElementById("arc-index-report");
  await withBusy(btn, "Indexing…", async () => {
    const reports = [];
    try {
      const r1 = await API.researchMigrateCrunchr(archiveState.projectId);
      reports.push(r1.report);
    } catch (e) {
      // No Crunchr database yet is a real, common "nothing to migrate from
      // this source" case — not fatal to the overall index run.
      reports.push({ source: "crunchr", examined: 0, inserted: 0, skipped: 0, errors: [String(e.message || e)], digest: "" });
    }
    const r2 = await API.researchMigrateLegacy(archiveState.projectId);
    reports.push(...(r2.reports || []));
    if (reportHost) reportHost.innerHTML = archiveIndexReportHtml(reports);
    const totalInserted = reports.reduce((a, r) => a + (r.inserted || 0), 0);
    Toast.success(`Indexed ${totalInserted} new signal(s) — refreshing…`);
    await renderArchive();
  }).catch((err) => Toast.error(`Index failed: ${err.message}`));
}

function archiveIndexReportHtml(reports) {
  if (!reports.length) return "";
  const rows = reports.map((r) => `
    <div class="arc-index-row">
      <span class="arc-row-title">${htmlEscape(r.source)}</span>
      <span class="pg-cap-hint">examined ${r.examined} · inserted ${r.inserted} · skipped ${r.skipped}</span>
      ${r.digest ? `<code class="arc-digest" title="content digest ${htmlEscape(r.digest)}">${htmlEscape(r.digest.slice(0, 12))}</code>` : ""}
      ${r.errors && r.errors.length ? `<span class="cfg-badge err" title="${htmlEscape(r.errors.join("; "))}">${r.errors.length} error(s)</span>` : ""}
    </div>`).join("");
  return `<div class="cfg-card arc-index-report">${rows}</div>`;
}

function archiveResolveSource(sourceId) {
  return archiveState.sourcesById.get(sourceId) || null;
}

// Shared "Open in Editor" action (F5) for both hit rows and moment rows.
// A source only resolves to a local recording when it was migrated from
// a per-recording origin (Crunchr transcripts) — channel-level sources
// (e.g. Viewguard audience-anomaly detections) have no `recording_id`,
// so the action is a non-blocking explanation instead of a dead button.
function archiveEditorActionHtml(src, tStartMs) {
  if (src && src.recording_id) {
    return `<button class="sm arc-open-editor" type="button"
              data-recording-id="${htmlEscape(src.recording_id)}"
              data-seek-ms="${tStartMs}">✄ Open in Editor</button>`;
  }
  return `<span class="pg-cap-hint arc-no-editor"
            title="This source isn't linked to a local recording — likely a channel-level detection rather than a specific recording.">
            No local recording linked
          </span>`;
}

function wireArchiveEditorButtons(host) {
  host.querySelectorAll(".arc-open-editor").forEach((btn) => {
    btn.addEventListener("click", () => {
      const recordingId = btn.dataset.recordingId;
      const seekMs = parseInt(btn.dataset.seekMs, 10) || 0;
      openRecordingInfo(recordingId, { seekSec: seekMs / 1000 });
    });
  });
}

function paintArchiveHitsEmpty(msg) {
  const host = document.getElementById("arc-hits");
  if (host) host.innerHTML = `<div class="empty sm">${htmlEscape(msg)}</div>`;
}

async function loadArchiveHits() {
  const host = document.getElementById("arc-hits");
  const pager = document.getElementById("arc-hits-pager");
  if (!host) return;
  const q = archiveState.query.trim();
  if (!q) {
    paintArchiveHitsEmpty("Type a query and press Enter to search your archive.");
    if (pager) pager.hidden = true;
    return;
  }
  host.setAttribute("aria-busy", "true");
  host.innerHTML = `<div class="empty sm">Searching…</div>`;
  try {
    const resp = await API.researchSearch(archiveState.projectId, q, {
      limit: archiveState.hitsLimit,
      offset: archiveState.hitsOffset,
    });
    const hits = resp.hits || [];
    archiveState.hits = hits;
    archiveState.hitsHasMore = hits.length === archiveState.hitsLimit;
    if (!hits.length) {
      host.innerHTML = `<div class="empty sm">No hits for “${htmlEscape(q)}”.</div>`;
    } else {
      host.innerHTML = hits.map((h) => archiveHitRowHtml(h)).join("");
      wireArchiveEditorButtons(host);
    }
    if (pager) {
      pager.hidden = archiveState.hitsOffset === 0 && !archiveState.hitsHasMore;
      document.getElementById("arc-hits-prev").disabled = archiveState.hitsOffset === 0;
      document.getElementById("arc-hits-next").disabled = !archiveState.hitsHasMore;
      document.getElementById("arc-hits-page-label").textContent =
        hits.length ? `${archiveState.hitsOffset + 1}–${archiveState.hitsOffset + hits.length}` : "";
    }
  } catch (e) {
    host.innerHTML = `<div class="empty sm"><div class="glyph">⚠</div>${htmlEscape(e.message)}
      <button class="sm" id="arc-hits-retry" type="button">Retry</button></div>`;
    document.getElementById("arc-hits-retry")?.addEventListener("click", () => loadArchiveHits());
    if (pager) pager.hidden = true;
  } finally {
    host.removeAttribute("aria-busy");
  }
}

function archiveHitRowHtml(h) {
  const src = archiveResolveSource(h.source_id);
  const title = src ? htmlEscape(src.title) : "(unresolved recording)";
  const time = fmtClock(h.t_start_ms / 1000);
  const conf = h.confidence != null ? `<span class="cfg-badge">${Math.round(h.confidence * 100)}%</span>` : "";
  return `
    <div class="arc-row">
      <div class="arc-row-main">
        <span class="arc-row-title">${title}</span>
        <span class="arc-row-time">${time}</span>
        ${conf}
      </div>
      <p class="arc-row-snippet">${htmlEscape(h.snippet)}</p>
      <div class="arc-row-actions">${archiveEditorActionHtml(src, h.t_start_ms)}</div>
    </div>`;
}

async function loadArchiveMoments() {
  const host = document.getElementById("arc-moments");
  const pager = document.getElementById("arc-moments-pager");
  if (!host) return;
  host.setAttribute("aria-busy", "true");
  host.innerHTML = `<div class="empty sm">Loading…</div>`;
  try {
    const raw = archiveState.minConfidence.trim();
    const minConfidence = raw === "" ? undefined : Number(raw);
    if (minConfidence != null && (!isFinite(minConfidence) || minConfidence < 0 || minConfidence > 1)) {
      host.innerHTML = `<div class="empty sm">Min confidence must be between 0 and 1.</div>`;
      if (pager) pager.hidden = true;
      return;
    }
    const resp = await API.researchMoments(archiveState.projectId, {
      minConfidence,
      limit: archiveState.momentsLimit,
      offset: archiveState.momentsOffset,
    });
    const moments = resp.moments || [];
    archiveState.moments = moments;
    archiveState.momentsHasMore = moments.length === archiveState.momentsLimit;
    if (!moments.length) {
      host.innerHTML = `<div class="empty sm">No moments yet. Add one, or lower the confidence filter.</div>`;
    } else {
      host.innerHTML = moments.map((m) => archiveMomentRowHtml(m)).join("");
      wireArchiveEditorButtons(host);
    }
    if (pager) {
      pager.hidden = archiveState.momentsOffset === 0 && !archiveState.momentsHasMore;
      document.getElementById("arc-moments-prev").disabled = archiveState.momentsOffset === 0;
      document.getElementById("arc-moments-next").disabled = !archiveState.momentsHasMore;
      document.getElementById("arc-moments-page-label").textContent =
        moments.length ? `${archiveState.momentsOffset + 1}–${archiveState.momentsOffset + moments.length}` : "";
    }
  } catch (e) {
    host.innerHTML = `<div class="empty sm"><div class="glyph">⚠</div>${htmlEscape(e.message)}
      <button class="sm" id="arc-moments-retry" type="button">Retry</button></div>`;
    document.getElementById("arc-moments-retry")?.addEventListener("click", () => loadArchiveMoments());
    if (pager) pager.hidden = true;
  } finally {
    host.removeAttribute("aria-busy");
  }
}

function archiveMomentRowHtml(m) {
  const src = archiveResolveSource(m.source_id);
  const title = src ? htmlEscape(src.title) : "(unresolved recording)";
  const time = `${fmtClock(m.t_start_ms / 1000)} → ${fmtClock(m.t_end_ms / 1000)}`;
  // MomentOrigin serializes as "coding" | "detection" (crate::MomentOrigin);
  // strategy §6 bans "coding" from creator-surface copy, so it's mapped to
  // "Moment" here rather than shown verbatim.
  const originBadge = m.origin === "detection"
    ? `<span class="cfg-badge">Auto-detected</span>`
    : `<span class="cfg-badge ok">Moment</span>`;
  const conf = m.confidence != null ? `<span class="cfg-badge">${Math.round(m.confidence * 100)}%</span>` : "";
  return `
    <div class="arc-row">
      <div class="arc-row-main">
        <span class="arc-row-title">${title}</span>
        <span class="arc-row-time">${time}</span>
        ${originBadge}${conf}
      </div>
      <p class="arc-row-snippet">${htmlEscape(m.label)} <span class="pg-cap-hint">· ${htmlEscape(m.kind)}</span></p>
      <div class="arc-row-actions">${archiveEditorActionHtml(src, m.t_start_ms)}</div>
    </div>`;
}

function openAddMomentModal() {
  let modal = document.getElementById("arc-moment-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "arc-moment-modal";
    modal.className = "app-modal";
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => { if (e.target === modal) closeAppModal(modal); });
  }
  const sources = [...archiveState.sourcesById.values()];
  modal.innerHTML = `
    <form class="card arc-moment-card" id="arc-moment-form" role="dialog" aria-label="Add moment">
      <h2>Add moment</h2>
      ${sources.length === 0 ? `
        <p class="empty sm">No recordings indexed in this workspace yet — index your archive first.</p>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="arc-mom-cancel">Close</button>
        </div>` : `
        <label class="arc-field">Recording
          <select id="arc-mom-source" class="arc-select" required>
            ${sources.map((s) => `<option value="${htmlEscape(s.id)}">${htmlEscape(s.title)}</option>`).join("")}
          </select>
        </label>
        <label class="arc-field">Start (mm:ss or seconds)
          <input id="arc-mom-start" class="arc-input" type="text" required placeholder="0:00"/>
        </label>
        <label class="arc-field">End (mm:ss or seconds)
          <input id="arc-mom-end" class="arc-input" type="text" required placeholder="0:10"/>
        </label>
        <label class="arc-field">Label
          <input id="arc-mom-label" class="arc-input" type="text" required maxlength="200" placeholder="What happens here?"/>
        </label>
        <label class="arc-field">Tag <span class="pg-cap-hint">(optional)</span>
          <input id="arc-mom-tag" class="arc-input" type="text" maxlength="80" placeholder="Moment"/>
        </label>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="arc-mom-cancel">Cancel</button>
          <button type="submit" class="btn-primary">Add moment</button>
        </div>`}
    </form>`;
  modal.classList.add("open");
  bumpModalOpen(+1);
  const close = () => closeAppModal(modal);
  modal.querySelector("#arc-mom-cancel")?.addEventListener("click", close);
  modal.querySelector("#arc-moment-form")?.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (sources.length === 0) { close(); return; }
    const sourceId = document.getElementById("arc-mom-source").value;
    const startSec = parseTimeInput(document.getElementById("arc-mom-start").value);
    const endSec = parseTimeInput(document.getElementById("arc-mom-end").value);
    const label = document.getElementById("arc-mom-label").value.trim();
    const tag = document.getElementById("arc-mom-tag").value.trim();
    if (!isFinite(startSec) || !isFinite(endSec)) { Toast.error("Couldn't parse start/end time"); return; }
    if (endSec < startSec) { Toast.error("End must be at or after start"); return; }
    if (!label) { Toast.error("Label is required"); return; }
    const btn = e.submitter;
    await withBusy(btn, "Adding…", async () => {
      await API.researchCreateMoment(archiveState.projectId, {
        source_id: sourceId,
        t_start_ms: Math.round(startSec * 1000),
        t_end_ms: Math.round(endSec * 1000),
        label,
        tag: tag || undefined,
      });
      Toast.success("Moment added");
      close();
      archiveState.momentsOffset = 0;
      loadArchiveMoments();
    }).catch((err) => Toast.error(`Couldn't add moment: ${err.message}`));
  });
  modal.querySelector("#arc-mom-source, #arc-mom-cancel")?.focus();
}

// Viewer route — single stream embed + collapsible chat sidepane.
// Reuses the existing chat plumbing (connectChatRoom, paintChatBody,
// emote + badge caches) so the chat in the sidepane is the same
// engine as the standalone /chat route. Channel selection sticks via
// URL hash (?room=<login>).
async function renderViewer() {
  const params = new URLSearchParams(window.location.hash.split("?")[1] || "");
  let room = params.get("room") || "";
  let rooms;
  try { rooms = (await API.chatRooms()).rooms || []; }
  catch (e) {
    root.innerHTML = chrome(`<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`);
    return;
  }
  rooms.sort((a, b) => {
    if (a.is_live !== b.is_live) return a.is_live ? -1 : 1;
    return a.display_name.localeCompare(b.display_name);
  });
  chatState.rooms = rooms;
  if (!room && rooms.length) room = (rooms.find((r) => r.is_live && r.connectable) || rooms.find((r) => r.connectable))?.room || "";
  if (!room) {
    root.innerHTML = chrome(`<div class="empty"><div class="glyph">📺</div>
      <p>No connectable channels. Follow some on Twitch via Settings → Platforms.</p></div>`);
    return;
  }
  chatState.active = room;
  const sidepaneOpen = (localStorage.getItem("strivo-viewer-sidepane") || "open") !== "closed";
  const picker = rooms.filter((r) => r.connectable).map((r) =>
    `<option value="${htmlEscape(r.room)}" ${r.room === room ? "selected" : ""}>${r.is_live ? "● " : ""}${htmlEscape(r.display_name)}</option>`
  ).join("");
  root.innerHTML = chrome(`
    <div id="viewer-root" class="viewer-root ${sidepaneOpen ? "" : "side-collapsed"}" role="main">
      <div class="viewer-toolbar">
        <label>Channel <select id="viewer-channel">${picker}</select></label>
        <button class="sm" id="viewer-toggle-side" type="button" title="Toggle chat sidepane">${sidepaneOpen ? "↦ Hide chat" : "↤ Show chat"}</button>
      </div>
      <div class="viewer-stage">
        <iframe id="viewer-iframe" class="viewer-iframe" allow="autoplay; fullscreen" allowfullscreen frameborder="0"></iframe>
      </div>
      <aside class="viewer-chat" id="viewer-chat">
        <div class="chat-filters">
          <input id="chat-filter-kw" type="text" placeholder="filter: contains…" />
          <input id="chat-filter-out" type="text" placeholder="filter: hide…" />
        </div>
        <div id="chat-body" class="chat-body" role="log" aria-live="polite"></div>
        <form id="chat-compose" class="chat-compose" autocomplete="off">
          <input id="chat-input" type="text" placeholder="Send message… (Enter)" maxlength="500" />
          <button class="sm" type="submit">▶</button>
          <span class="chat-compose-hint pg-cap-hint" id="chat-compose-hint"></span>
        </form>
      </aside>
    </div>
  `);
  setupChromeHandlers();
  // Wire third-party + per-channel caches once.
  ensureThirdPartyEmotes().then(() => schedulePaintChat());
  ensureGlobalBadges().then(() => schedulePaintChat());
  // Mount the embed iframe — Twitch needs parent= hostname.
  // Twitch's embed validator REJECTS bare IP addresses (and 'localhost'
  // works only when accessed via 'localhost'). LAN dogfooding via
  // http://<ip>:8181 fails with 'embed misconfigured'. We detect the
  // bare-IP case, rewrite parent= to a nip.io equivalent
  // (<ip-dashed>.nip.io), and offer the user a one-click banner that
  // navigates the WHOLE page to the matching nip.io URL so the
  // iframe's referer lines up with parent=. nip.io resolves
  // <ip-dashed>.nip.io → that IP via wildcard DNS — no setup needed.
  const rawHost = location.host;
  const hostNoPort = rawHost.split(":")[0];
  const port = rawHost.includes(":") ? rawHost.split(":")[1] : "";
  const isLocalhost = hostNoPort === "localhost" || hostNoPort === "127.0.0.1";
  const isHttps = location.protocol === "https:";
  // Twitch's parent= validator + its embed CSP together require:
  //   * a hostname (no bare IPs)
  //   * either https:// access, OR access via 'localhost' over http
  // Anything else gets the 'embed misconfigured' / CSP-violation
  // error. We rewrite bare IP → nip.io so the parent= validator
  // passes, but the CSP rule still needs https for nip.io.
  const parent = embedParentHost(rawHost);
  const embedBlocked = !isLocalhost && !isHttps;
  const stage = document.querySelector(".viewer-stage");
  if (embedBlocked) {
    const nipUrl = `http://${parent}${port ? ":" + port : ""}${location.pathname}${location.hash}`;
    // Render a fix overlay above the iframe with three working
    // paths so the user can pick whichever is easiest.
    const banner = document.createElement("div");
    banner.className = "viewer-embed-banner";
    banner.innerHTML = `
      <p><strong>Twitch can't embed over plain HTTP on a remote host.</strong>
      Their player CSP requires <code>https://</code> for any parent that isn't <code>localhost</code>. Pick one of these:</p>
      <ol class="viewer-embed-options">
        <li><strong>SSH tunnel (easiest)</strong> — on your laptop run
          <code>ssh -L 8181:localhost:8181 ${htmlEscape(hostNoPort)}</code>
          then open <a href="http://localhost:8181${location.pathname}${location.hash}">http://localhost:8181${htmlEscape(location.pathname + location.hash)}</a> — Twitch whitelists localhost over HTTP.</li>
        <li><strong>HTTPS via nip.io + a cert</strong> — front the serve with Caddy / nginx terminating TLS for
          <code>${htmlEscape(parent)}${port ? ":" + port : ""}</code>, then access via
          <a href="${htmlEscape(nipUrl.replace(/^http:/, "https:"))}">https://${htmlEscape(parent)}${port ? ":" + htmlEscape(port) : ""}</a>.</li>
        <li><strong>SOCKS over the LAN</strong> — proxy the laptop browser through the strivo host (any SOCKS proxy will do) and treat it as localhost.</li>
      </ol>
      <p class="pg-cap-hint">The chat sidepane on the right works regardless — Twitch chat connects via WebSocket without the embed CSP. Use it while you sort out the player path.</p>`;
    stage.prepend(banner);
  }
  document.getElementById("viewer-iframe").src =
    buildEmbedUrl("Twitch", room, { host: rawHost });
  // Sidepane toggle persists.
  document.getElementById("viewer-toggle-side").addEventListener("click", () => {
    const root_ = document.getElementById("viewer-root");
    const open = !root_.classList.contains("side-collapsed");
    if (open) { root_.classList.add("side-collapsed"); localStorage.setItem("strivo-viewer-sidepane", "closed"); }
    else { root_.classList.remove("side-collapsed"); localStorage.setItem("strivo-viewer-sidepane", "open"); }
    document.getElementById("viewer-toggle-side").textContent = open ? "↤ Show chat" : "↦ Hide chat";
  });
  // Channel picker rewrites the URL — render() reruns via hashchange.
  document.getElementById("viewer-channel").addEventListener("change", (ev) => {
    const next = ev.target.value;
    window.location.hash = `#/viewer?room=${encodeURIComponent(next)}`;
  });
  connectChatRoom(room);
  paintChatBody({ full: true });
  // Filter inputs + compose box — reuse the same handlers as renderChat
  // by setting up a tiny applyFilters local.
  const applyFilters = () => {
    const kw = document.getElementById("chat-filter-kw").value.trim();
    const out = document.getElementById("chat-filter-out").value.trim();
    chatState.filters = [];
    if (kw) chatState.filters.push({ kind: "keyword_in", needle: kw });
    if (out) chatState.filters.push({ kind: "keyword_out", needle: out });
    paintChatBody({ full: true });
  };
  document.getElementById("chat-filter-kw").addEventListener("input", applyFilters);
  document.getElementById("chat-filter-out").addEventListener("input", applyFilters);
  document.getElementById("chat-compose").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const text = (document.getElementById("chat-input").value || "").trim();
    if (!text) return;
    try {
      await API.chatSend(room, text);
      document.getElementById("chat-input").value = "";
    } catch (err) {
      document.getElementById("chat-compose-hint").textContent = err.message || "Send failed";
    }
  });
}

// Multi-stream viewer route. Server returns tiles already laid out for
// the requested container size + mode, plus each stream's ready-to-mount
// embed URL. Mode is kept in URL params so refresh / share preserves the
// view.
// Background poll handle so route switches can cancel the previous
// timer before mounting a new one.
// A12: deprecated. playerState.refreshTimer is the canonical handle;
// this dual reference is retained so teardownAcrossRoutes() can keep
// clearing both during a transitional release. New code MUST use
// playerState.refreshTimer.
let _watchRefreshTimer = null;

// Append the muted-state parameter Twitch / YouTube embeds use.
// ── Tile audio ───────────────────────────────────────────────────────
//
// Volume per tile is the source of truth; muted simply means zero. That
// split matters because "which tile is focused" and "which tile is audible"
// were the same flag before, so you could not run a main stream loud with a
// second quietly underneath — the only reachable states were one-audible or
// all-silent.
//
// `playerState.soloPath` survives as the FOCUS marker (it steers the chat
// rail and the focus-aware quality policy); it no longer decides audio.
const PLAYER_VOLUME_KEY = "strivo:tile-volumes";
function loadTileVolumes() {
  try {
    const raw = JSON.parse(localStorage.getItem(PLAYER_VOLUME_KEY) || "{}");
    return raw && typeof raw === "object" ? raw : {};
  } catch (_) {
    return {};
  }
}
function saveTileVolumes() {
  try {
    localStorage.setItem(PLAYER_VOLUME_KEY, JSON.stringify(playerState.volumes || {}));
  } catch (_) {
    /* private mode */
  }
}
/// Everything starts silent: a wall that begins making noise on open is
/// hostile, and it matches the previous mute-all default.
function tileVolumeForKey(key) {
  if (!key) return 0;
  const v = (playerState.volumes || {})[key];
  return typeof v === "number" && Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 0;
}
function tileVolumeAt(path) {
  return tileVolumeForKey(contentKeyOf(getNodeAt(playerState.layout, path)));
}
function setTileVolumeAt(path, vol) {
  const key = contentKeyOf(getNodeAt(playerState.layout, path));
  if (!key) return;
  playerState.volumes = { ...(playerState.volumes || {}), [key]: Math.max(0, Math.min(1, vol)) };
  saveTileVolumes();
}
/// Solo is a preset, not a mode: raise this tile, silence the rest. The
/// viewer is free to nudge any of them afterwards.
function soloTileAt(path) {
  const next = {};
  walkLayout(playerState.layout, (node, p) => {
    const key = contentKeyOf(node);
    if (key) next[key] = p === path ? 1 : 0;
  });
  playerState.volumes = next;
  playerState.soloPath = path;
  saveTileVolumes();
}
function muteAllTiles() {
  const next = {};
  walkLayout(playerState.layout, (node) => {
    const key = contentKeyOf(node);
    if (key) next[key] = 0;
  });
  playerState.volumes = next;
  playerState.soloPath = "";
  saveTileVolumes();
}

/// Muted is derived, never stored: a tile is muted exactly when silent.
function computeMuted(path) {
  return tileVolumeAt(path) === 0;
}

// Single source of truth for a live tile's iframe src.
//
// Autoplay has to be stated explicitly because the two providers disagree
// out of the box: Twitch's player autoplays unless told not to, YouTube's
// embed does the opposite. Leaving it implicit is why opening a 3x3 wall
// used to start nine Twitch streams at once.
//
// The base URL is kept on the element in `data-embed-base` so repaints
// rebuild from it rather than regex-stripping params off a previous src.
function tileSrc(embedUrl, { muted, playing }) {
  if (!embedUrl) return embedUrl;
  const yt = embedUrl.includes("youtube.com");
  const params = [
    yt ? `mute=${muted ? 1 : 0}` : `muted=${muted}`,
    yt ? `autoplay=${playing ? 1 : 0}` : `autoplay=${playing}`,
  ];
  return embedUrl + (embedUrl.includes("?") ? "&" : "?") + params.join("&");
}

// Multi-view quality defaults, per provider.
//
// Deliberately NOT normalised into one cross-platform scale: the same
// "1080p" is a different bitrate on Twitch than on YouTube, and differs
// between streamers on the same platform. The UI therefore offers what each
// provider actually exposes, and says plainly where a provider exposes
// nothing.
const MULTIVIEW_QUALITY_KEY = "strivo:multiview-quality";
const MULTIVIEW_QUALITY_DEFAULTS = { twitch: "best", youtube: "auto" };
const MULTIVIEW_QUALITY_CHOICES = [
  ["best", "Best available"],
  ["auto", "Let the platform decide"],
  ["low", "Lowest (save bandwidth/CPU)"],
  ["focus", "Best on the focused tile, lowest on the rest"],
];
function multiviewQuality() {
  try {
    const raw = JSON.parse(localStorage.getItem(MULTIVIEW_QUALITY_KEY) || "{}");
    return { ...MULTIVIEW_QUALITY_DEFAULTS, ...(raw && typeof raw === "object" ? raw : {}) };
  } catch (_) {
    return { ...MULTIVIEW_QUALITY_DEFAULTS };
  }
}
function setMultiviewQuality(provider, value) {
  const next = { ...multiviewQuality(), [provider]: value };
  try {
    localStorage.setItem(MULTIVIEW_QUALITY_KEY, JSON.stringify(next));
  } catch (_) {
    /* private mode */
  }
}
/// Resolve the effective policy for one tile. "focus" is the only setting
/// that varies per tile; everything else applies uniformly.
function qualityPolicyFor(kind, path) {
  const provider = kind === "youtube" ? "youtube" : "twitch";
  const setting = multiviewQuality()[provider] || "auto";
  if (setting !== "focus") return setting;
  const focused = playerState.soloPath || "";
  return path === focused ? "best" : "low";
}

// ── Player controllers ───────────────────────────────────────────────
//
// A tile's vendor player is owned by a controller keyed on CONTENT
// (`s:<streamId>` / `r:<recordingId>`), held in `playerState.controllers`
// for the whole watch session — deliberately NOT keyed on layout path and
// NOT owned by the DOM.
//
// That distinction is the entire point. Stage HTML is rebuilt wholesale on
// preset switches, composer edits, and Play-all; when the player lived in
// that HTML, every rebuild tore it down and Twitch/YouTube restarted the
// stream. Because the registry outlives any single paint, a rebuild now
// just re-parents the existing element into its new mount point, and a
// stream the user never touched keeps playing.
//
// Every controller is created through a swappable factory so e2e can inject
// a fake and assert on lifecycle without loading a real vendor player.

/// Stable identity for whatever a slot holds. Returns null for empty slots.
function contentKeyOf(node) {
  if (!node) return null;
  if (node.streamId) return `s:${node.streamId}`;
  if (node.recordingId) return `r:${node.recordingId}`;
  return null;
}

/// Today's behaviour, wrapped: a plain cross-origin iframe whose only
/// control surface is its `src`. Mute and playback still cost a reload
/// here — that is inherent to a bare iframe, and is what the vendor-SDK
/// controllers in later phases exist to avoid. This stays as the permanent
/// fallback for when a vendor script is blocked or fails to load, so a
/// missing SDK degrades to exactly the old experience rather than a blank
/// tile.
function makeIframeController(spec) {
  const el = document.createElement("iframe");
  el.className = "watch-tile-iframe ms-iframe";
  el.setAttribute(
    "allow",
    "autoplay; fullscreen; picture-in-picture; encrypted-media; clipboard-write",
  );
  el.setAttribute("allowfullscreen", "");
  el.setAttribute("frameborder", "0");
  let base = spec.embedUrl || "";
  let muted = !!spec.muted;
  const playing = spec.playing !== false;
  el.setAttribute("data-embed-base", base);
  const sync = () => {
    if (!base) return;
    const next = tileSrc(base, { muted, playing });
    // Assigning src reloads the player, so only write on a real change.
    if (el.getAttribute("src") !== next) el.setAttribute("src", next);
  };
  sync();
  return {
    kind: "iframe-fallback",
    root: el,
    mount(container) {
      if (el.parentElement !== container) container.appendChild(el);
    },
    destroy() {
      el.remove();
    },
    setMuted(next) {
      if (next !== muted) {
        muted = next;
        sync();
      }
    },
    setVolume() {
      /* not addressable without a player API — see the Twitch controller */
    },
    setQuality() {
      /* ditto */
    },
    repoint(next) {
      const url = next && next.embedUrl;
      if (url && url !== base) {
        base = url;
        el.setAttribute("data-embed-base", base);
        sync();
      }
    },
    isReady() {
      return true;
    },
  };
}

/// Local recording playback. A `<video>` already has a real API, so this
/// never needed the reload workaround — it is a controller purely so the
/// reconciler can treat every tile the same way, and so a repaint stops
/// dropping playback position on the floor.
function makeRecordingController(spec) {
  const el = document.createElement("video");
  el.className = "watch-tile-iframe ms-video";
  el.controls = true;
  el.playsInline = true;
  el.muted = !!spec.muted;
  const playing = spec.playing !== false;
  el.preload = playing ? "metadata" : "none";
  if (playing) el.autoplay = true;
  if (spec.src) el.src = spec.src;
  return {
    kind: "recording",
    root: el,
    mount(container) {
      if (el.parentElement !== container) container.appendChild(el);
    },
    destroy() {
      try {
        el.pause();
      } catch (_) {
        /* already detached */
      }
      el.removeAttribute("src");
      el.remove();
    },
    setMuted(next) {
      if (el.muted !== next) el.muted = next;
    },
    setVolume(v) {
      el.volume = Math.max(0, Math.min(1, v));
    },
    setQuality() {
      /* the file is the file */
    },
    repoint(next) {
      if (next && next.src && next.src !== el.getAttribute("src")) el.src = next.src;
    },
    isReady() {
      return true;
    },
  };
}

/// Load Twitch's embed SDK once, shared by every tile.
///
/// Memoised on the PROMISE, not a boolean: two tiles starting at the same
/// moment must share one in-flight load rather than injecting the script
/// twice. Rejection is sticky and harmless — the factory simply keeps
/// handing out iframe controllers.
let _twitchSdk = null;
function loadTwitchSdkOnce() {
  if (_twitchSdk) return _twitchSdk;
  _twitchSdk = new Promise((resolve, reject) => {
    if (window.Twitch && window.Twitch.Player) return resolve(window.Twitch);
    const el = document.createElement("script");
    el.src = "https://player.twitch.tv/js/embed/v1.js";
    el.async = true;
    el.onload = () =>
      window.Twitch && window.Twitch.Player
        ? resolve(window.Twitch)
        : reject(new Error("Twitch SDK loaded without a Player"));
    el.onerror = () => reject(new Error("Twitch SDK failed to load"));
    document.head.appendChild(el);
  });
  return _twitchSdk;
}

/// Pull the channel and parent back out of an embed URL we built ourselves.
function parseTwitchEmbed(url) {
  try {
    const u = new URL(url);
    return {
      channel: u.searchParams.get("channel") || "",
      parent: u.searchParams.get("parent") || embedParentHost(),
    };
  } catch (_) {
    return null;
  }
}

/// Twitch tile driven through `Twitch.Player`.
///
/// The whole point: mute, volume and quality are method calls, so none of
/// them reload the stream the way rewriting an iframe `src` does. Quality is
/// the big one on a wall — several 1080p60 decodes is the dominant cost, and
/// background tiles do not need source.
function makeTwitchController(spec) {
  const host = document.createElement("div");
  host.className = "watch-tile-iframe ms-iframe ms-twitch";
  const parsed = parseTwitchEmbed(spec.embedUrl) || { channel: "", parent: embedParentHost() };

  let player = null;
  let ready = false;
  let qualities = [];
  let pendingQualityTier = null;
  let muted = !!spec.muted;
  let volume = typeof spec.volume === "number" ? spec.volume : 1;

  /// Map a policy onto whatever this stream actually offers.
  ///
  /// The quality string vocabulary is NOT documented by Twitch — community
  /// reports resolution-coded names like "1080p60 (source)" — so options are
  /// discovered per player and never hardcoded. A resolution label is also
  /// not a quality measure: the same "1080p" differs in bitrate between
  /// streamers and platforms, which is why nothing here normalises across
  /// providers. Anything unexpected means: do nothing and let Twitch's own
  /// auto stand, which is always a safe answer.
  const applyQuality = (policy) => {
    if (!player || !ready || !policy || policy === "auto") return;
    // getQualities() returns entries with a `group` id and a human `name`.
    // Drop the synthetic "auto" entry; what remains is highest-first per
    // Twitch's ordering, but do not rely on that — pick by explicit ends.
    const usable = qualities.filter((q) => q && q.group && q.group !== "auto");
    if (usable.length < 2) return;
    const pick = policy === "low" ? usable[usable.length - 1] : usable[0];
    if (!pick || !pick.group) return;
    try {
      if (player.getQuality() !== pick.group) player.setQuality(pick.group);
    } catch (_) {
      /* advisory only — never worse than leaving auto alone */
    }
  };

  loadTwitchSdkOnce()
    .then((Twitch) => {
      if (!host.isConnected && !host.parentElement) {
        // Tile was removed while the SDK was loading.
        return;
      }
      player = new Twitch.Player(host, {
        channel: parsed.channel,
        parent: [parsed.parent],
        width: "100%",
        height: "100%",
        muted,
        autoplay: spec.playing !== false,
      });
      player.addEventListener(Twitch.Player.READY, () => {
        ready = true;
        try {
          // getQualities() ordering is not documented either; sort so the
          // highest is first regardless of what Twitch hands back.
          qualities = (player.getQualities() || []).slice();
        } catch (_) {
          qualities = [];
        }
        try {
          player.setMuted(muted);
          player.setVolume(volume);
        } catch (_) {
          /* pre-ready calls can throw on some builds */
        }
        if (pendingQualityTier) applyQuality(pendingQualityTier);
      });
    })
    .catch(() => {
      // SDK blocked or offline: fall back to a plain iframe in place, so
      // the tile still plays rather than sitting empty.
      const fb = makeIframeController(spec);
      host.replaceWith(fb.root);
      player = null;
      controller.root = fb.root;
      controller.setMuted = fb.setMuted;
      controller.setVolume = fb.setVolume;
      controller.repoint = fb.repoint;
      controller.destroy = fb.destroy;
      controller.mount = fb.mount;
    });

  const controller = {
    kind: "twitch",
    root: host,
    mount(container) {
      if (controller.root.parentElement !== container) container.appendChild(controller.root);
    },
    destroy() {
      try {
        // The SDK exposes no documented destroy; dropping the node releases
        // the iframe it created underneath.
        player = null;
      } finally {
        controller.root.remove();
      }
    },
    setMuted(next) {
      muted = next;
      if (player && ready) {
        try {
          player.setMuted(next);
          return;
        } catch (_) {
          /* fall through */
        }
      }
    },
    setVolume(v) {
      volume = Math.max(0, Math.min(1, v));
      if (player && ready) {
        try {
          player.setVolume(volume);
        } catch (_) {
          /* advisory */
        }
      }
    },
    setQuality(tier) {
      pendingQualityTier = tier;
      applyQuality(tier);
    },
    getPlaybackStats() {
      try {
        return player && ready ? player.getPlaybackStats() : null;
      } catch (_) {
        return null;
      }
    },
    repoint(next) {
      const p = next && next.embedUrl ? parseTwitchEmbed(next.embedUrl) : null;
      if (!p || !p.channel || p.channel === parsed.channel) return;
      parsed.channel = p.channel;
      if (player) {
        try {
          // Retarget in place — no teardown, no reload.
          player.setChannel(p.channel);
        } catch (_) {
          /* leave the tile on its current channel rather than blanking it */
        }
      }
    },
    isReady() {
      return ready;
    },
  };
  return controller;
}

function defaultPlayerControllerFactory(kind, spec) {
  if (kind === "recording") return makeRecordingController(spec);
  if (kind === "twitch") return makeTwitchController(spec);
  // YouTube stays on the plain iframe for now: YT.Player addresses content
  // by video id, and the live embed strivo builds addresses a CHANNEL, so
  // there is no video id to hand it without new plumbing.
  return makeIframeController(spec);
}

let _playerControllerFactory = defaultPlayerControllerFactory;
function setPlayerControllerFactory(fn) {
  _playerControllerFactory = fn || defaultPlayerControllerFactory;
}

/// Diff the controller registry against the mount points in freshly-painted
/// stage HTML. Called by BOTH the full repaint and the surgical patch path,
/// so neither needs its own notion of player lifecycle.
function reconcileControllers(stage) {
  if (!stage) return;
  const wanted = new Map();
  stage.querySelectorAll(".ms-mount[data-content-key]").forEach((mount) => {
    wanted.set(mount.dataset.contentKey, mount);
  });

  // Anything no longer on the wall is destroyed. Skipping this leaks a live
  // vendor connection — bandwidth and CPU for a tile that is gone.
  for (const [key, ctl] of playerState.controllers) {
    if (!wanted.has(key)) {
      try {
        ctl.destroy();
      } catch (_) {
        /* a controller that fails to tear down must not block the rest */
      }
      playerState.controllers.delete(key);
    }
  }

  for (const [key, mount] of wanted) {
    const path = mount.dataset.path || "";
    const muted = computeMuted(path);
    let ctl = playerState.controllers.get(key);
    if (!ctl) {
      try {
        ctl = _playerControllerFactory(mount.dataset.kind || "iframe-fallback", {
          embedUrl: mount.dataset.embedBase || "",
          src: mount.dataset.src || "",
          muted,
          playing: mount.dataset.playing !== "0",
        });
      } catch (e) {
        // A factory that throws must not take the wall down with it.
        tracingWarn("player controller failed to construct", e);
        continue;
      }
      playerState.controllers.set(key, ctl);
    } else if (mount.dataset.embedBase || mount.dataset.src) {
      // Same content, different URL (host changed, for instance) — retarget
      // rather than rebuild.
      ctl.repoint({ embedUrl: mount.dataset.embedBase || "", src: mount.dataset.src || "" });
    }
    ctl.mount(mount);
    const vol = tileVolumeForKey(key);
    ctl.setMuted(vol === 0);
    ctl.setVolume(vol);
    ctl.setQuality(qualityPolicyFor(mount.dataset.kind || "", path));
  }
}

/// Drop every controller — used when leaving the watch route entirely.
function destroyAllControllers() {
  for (const [, ctl] of playerState.controllers) {
    try {
      ctl.destroy();
    } catch (_) {
      /* best effort */
    }
  }
  playerState.controllers.clear();
}

function tracingWarn(msg, e) {
  console.warn(`[strivo] ${msg}:`, e);
}

// Tiles start paused. A wall of live streams that all begin playing the
// moment it opens burns bandwidth and CPU on streams the viewer has not
// chosen to watch yet, so the default is a still, cheap grid you press
// play on. Persisted so the preference survives a reload.
const PLAYER_AUTOPLAY_KEY = "strivo-player-autoplay";
function loadPlayerAutoplay() {
  try { return localStorage.getItem(PLAYER_AUTOPLAY_KEY) === "1"; } catch (_) { return false; }
}
function savePlayerAutoplay() {
  try {
    localStorage.setItem(PLAYER_AUTOPLAY_KEY, playerState.autoplay ? "1" : "0");
  } catch (_) { /* private mode */ }
}
/// Is this slot playing? A stream the viewer pressed play on stays playing
/// while the wall default is paused.
///
/// Keyed on CONTENT, not layout path, for the same reason the controller
/// registry is: a tile that moves is still the same stream. Keying on path
/// meant switching preset — which relocates a stream from "" to "a.a" —
/// silently paused it, because the new path had never been started.
function tilePlaying(node) {
  if (playerState.autoplay) return true;
  const key = contentKeyOf(node);
  return !!key && (playerState.playing || []).includes(key);
}
function setTilePlaying(node, on) {
  const key = contentKeyOf(node);
  if (!key) return;
  const set = new Set(playerState.playing || []);
  if (on) set.add(key); else set.delete(key);
  playerState.playing = [...set];
}

// ── Player layout tree (multi-view collapsed into the player) ────────
//
// The viewing stage is a recursive layout tree. Two node kinds:
//   slot:  { kind: "slot", streamId: string|null }
//   split: { kind: "split", dir: "h"|"v", ratio: 0..1, a: node, b: node }
//
// 'h' splits stack left|right, 'v' splits stack top|bottom. The split
// ratio governs how much room the 'a' child gets. Presets always
// create EMPTY slots (per user request) — picking a preset never
// auto-populates streams.
//
// Cap at 9 leaves keeps the iframe count reasonable; beyond that the
// browser starts paging and Twitch/YT rate-limit your IP.

const PLAYER_LEAF_CAP = 9;
const PLAYER_LAYOUT_KEY = "strivo-player-layout";
const PLAYER_PRESET_KEY = "strivo-player-preset";

// A slot can hold ONE of (or neither):
//   streamId      — live channel (rendered as a platform embed iframe)
//   recordingId   — finished recording (rendered as a <video> sourced
//                   from /api/v1/recordings/<id>/download — the /file
//                   route is DELETE-only)
function _slot(streamId = null, recordingId = null) { return { kind: "slot", streamId, recordingId }; }
function _split(dir, ratio, a, b) { return { kind: "split", dir, ratio, a, b }; }
// Row builders for the grid presets — kept as helpers so the trees read as
// rows rather than as nested split soup.
function _row2() { return _split("h", 0.5, _slot(), _slot()); }
function _row3() { return _split("h", 1 / 3, _slot(), _split("h", 0.5, _slot(), _slot())); }

// Strict drag-payload parser. Validates the shape and ID grammar so a
// stray browser URL drag (or a corrupted/old payload) can't slip into
// the layout as a real stream. Returns:
//   { type: "tile",      path: string }
//   { type: "stream",    id:   string }
//   { type: "recording", id:   string }
// or null when the payload doesn't match any known schema.
function parsePlayerDragPayload(text) {
  if (typeof text !== "string" || !text) return null;
  // Path can be empty (root); slug chars only beyond that.
  if (text.startsWith("strivo-tile:")) {
    const path = text.slice("strivo-tile:".length);
    if (path !== "" && !/^[ab](\.[ab])*$/.test(path)) return null;
    return { type: "tile", path };
  }
  if (text.startsWith("strivo-stream:")) {
    const id = text.slice("strivo-stream:".length);
    // Backend stream-id shape: 'PlatformDebug:Id' — alphanumeric +
    // colons/dashes/underscores. Reject anything wilder.
    if (!/^[A-Za-z0-9:_-]+$/.test(id)) return null;
    return { type: "stream", id };
  }
  if (text.startsWith("strivo-recording:")) {
    const id = text.slice("strivo-recording:".length);
    if (!/^[A-Za-z0-9_-]+$/.test(id)) return null;
    return { type: "recording", id };
  }
  return null;
}

const PLAYER_PRESETS = {
  single: () => _slot(),
  "split-screen": () => _split("h", 0.5, _slot(), _slot()),
  // Split / quadrant = 3 streams: left half single, right split top/bottom.
  "split-quadrant": () => _split("h", 0.5, _slot(), _split("v", 0.5, _slot(), _slot())),
  // 2×2 grid.
  quadrant: () => _split("v", 0.5,
    _split("h", 0.5, _slot(), _slot()),
    _split("h", 0.5, _slot(), _slot()),
  ),
  // Grids are built rows-first so a depth-first walk visits tiles in
  // reading order — that walk is what "fill the next empty tile" uses, and
  // a column-major tree makes clicked sources land in a scattered order.
  // 2 columns x 3 rows.
  "grid-6": () => _split("v", 1 / 3,
    _row2(),
    _split("v", 0.5, _row2(), _row2()),
  ),
  // 3x3 wall — 9 leaves, exactly the cap countLeaves enforces.
  "grid-9": () => _split("v", 1 / 3,
    _row3(),
    _split("v", 0.5, _row3(), _row3()),
  ),
  // One large focus tile with a stacked sidebar of three.
  "focus-3": () => _split("h", 0.68,
    _slot(),
    _split("v", 1 / 3, _slot(), _split("v", 0.5, _slot(), _slot())),
  ),
  custom: () => _slot(),
};

const PLAYER_PRESET_LABELS = {
  single: "Single",
  "split-screen": "Split-screen (2)",
  "split-quadrant": "Split / Quadrant (3)",
  quadrant: "Quadrant (4)",
  "focus-3": "Focus + 3",
  "grid-6": "Grid (6)",
  "grid-9": "Wall (9)",
  custom: "Custom",
};

const PLAYER_CHAT_RAIL_KEY = "strivo-player-chat-rail-open";
function loadPlayerChatRailOpen() {
  try { return localStorage.getItem(PLAYER_CHAT_RAIL_KEY) === "1"; } catch (_) { return false; }
}
function savePlayerChatRailOpen() {
  try { localStorage.setItem(PLAYER_CHAT_RAIL_KEY, playerState.chatRailOpen ? "1" : "0"); } catch (_) {}
}

const playerState = {
  layout: null,        // root layout node
  preset: "single",    // last-applied preset name (for the toolbar label)
  soloPath: "",        // path to soloed (audible) slot — "" = mute-all
  refreshTimer: null,
  resizeFx: null,      // gutter drag state
  chatRailOpen: loadPlayerChatRailOpen(),  // right-rail chat persistence
  chatRailRoom: null,  // currently-rendered room (Twitch login) on the rail
  chatRailMount: null, // teardown handle from mountChatRail
  chatRailLastStreams: [], // last streams[] cache for toggle re-reconciliation
  chatRailCompose: null,   // mountChatCompose controller for the rail
  lastPaintedLayout: null, // snapshot used by tryPatchPlayerStage to diff
  autoplay: loadPlayerAutoplay(), // wall-wide default; false = open paused
  playing: [],         // paths the viewer explicitly started while paused
  // contentKey -> PlayerController. Outlives every repaint; see the
  // "Player controllers" block for why ownership is not in the DOM.
  controllers: new Map(),
  volumes: loadTileVolumes(), // contentKey -> 0..1; muted is volume === 0
};

// Path strings are dot-joined sequences of "a"/"b" descending the tree.
// Root = "". Example: "a.b" → root.a.b.
function pathParts(path) { return path ? path.split(".") : []; }
function pathStr(parts) { return parts.join("."); }

// Walk a layout. Calls cb(node, path) for every node (depth-first).
function walkLayout(layout, cb, path = "") {
  cb(layout, path);
  if (layout.kind === "split") {
    walkLayout(layout.a, cb, path ? `${path}.a` : "a");
    walkLayout(layout.b, cb, path ? `${path}.b` : "b");
  }
}

function countLeaves(layout) {
  let n = 0;
  walkLayout(layout, (node) => { if (node.kind === "slot") n++; });
  return n;
}

// Collect every live-Twitch tile in layout order. The chat rail uses
// this to render a PFP/letter-avatar strip the user can tap to swap
// chat rooms. YouTube/Patreon are skipped — only Twitch chat is wired
// end-to-end on the SPA today.
function collectChatableStreams(streams) {
  if (!streams || streams.length === 0 || !playerState.layout) return [];
  const seen = new Set();
  const out = [];
  walkLayout(playerState.layout, (n, path) => {
    if (n.kind !== "slot" || !n.streamId) return;
    if (seen.has(n.streamId)) return;
    const s = streams.find((x) => x.stream_id === n.streamId);
    // Only Twitch chat is wired end-to-end on the SPA. The backend
    // returns the platform tag in lowercase; the rest of the SPA uses
    // the canonical capitalised form. Accept both.
    if (!s) return;
    const plat = (s.platform || "").toLowerCase();
    if (plat !== "twitch") return;
    seen.add(n.streamId);
    out.push({ stream: s, path });
  });
  return out;
}

// Default stream for the rail when the user hasn't explicitly picked
// one (or the previously-picked stream dropped off): solo wins, else
// first chatable tile in layout order.
function derivePlayerChatDefault(streams) {
  const chatable = collectChatableStreams(streams);
  if (chatable.length === 0) return null;
  const soloPath = playerState.soloPath || "";
  if (soloPath) {
    const soloed = chatable.find((c) => c.path === soloPath);
    if (soloed) return soloed.stream;
  }
  return chatable[0].stream;
}

// Reconcile the chat rail against the current layout + streams. Idempotent;
// safe to call after every player paint. Honours the user's explicit room
// pick when it's still in the layout, else falls back to the focus default.
function reconcilePlayerChatRail(streams) {
  const rail = document.getElementById("player-chat-rail");
  if (!rail) return;
  const body = rail.querySelector(".player-chat-rail-body");
  const tabs = rail.querySelector(".player-chat-rail-tabs");
  const roomLabel = rail.querySelector(".player-chat-rail-room");
  if (!body || !tabs) return;

  if (!playerState.chatRailOpen) {
    if (playerState.chatRailMount) {
      try { playerState.chatRailMount.teardown(); } catch (_) {}
      playerState.chatRailMount = null;
    }
    if (playerState.chatRailCompose) {
      try { playerState.chatRailCompose.teardown(); } catch (_) {}
      playerState.chatRailCompose = null;
    }
    playerState.chatRailRoom = null;
    tabs.innerHTML = "";
    body.innerHTML = "";
    if (roomLabel) roomLabel.textContent = "";
    return;
  }

  const chatable = collectChatableStreams(streams);
  // PFP/letter-avatar strip. Click to override follow-focus and switch
  // the rail to that channel's chat.
  tabs.innerHTML = chatable.map(({ stream: s }) => {
    const room = (s.channel_name || "").toLowerCase();
    const display = s.channel_name || room;
    const hue = chatAvatarHue(display);
    const active = room === playerState.chatRailRoom ? " active" : "";
    return `
      <button class="player-chat-rail-tab${active}" type="button"
              data-room="${htmlEscape(room)}"
              data-stream-id="${htmlEscape(s.stream_id)}"
              title="Chat: ${htmlEscape(display)}">
        <span class="player-chat-rail-avatar"
              style="background:hsl(${hue} 55% 32%);">${htmlEscape(display.slice(0, 1).toUpperCase())}</span>
      </button>`;
  }).join("");
  tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      const room = btn.dataset.room || "";
      if (!room || room === playerState.chatRailRoom) return;
      playerState.chatRailRoom = room;
      reconcilePlayerChatRail(playerState.chatRailLastStreams);
    });
  });

  // Resolve target room: respect explicit pick if still valid; else
  // fall back to the default (solo / first leaf).
  const validRooms = new Set(chatable.map((c) => (c.stream.channel_name || "").toLowerCase()));
  let targetRoom = playerState.chatRailRoom;
  if (!targetRoom || !validRooms.has(targetRoom)) {
    const def = derivePlayerChatDefault(streams);
    targetRoom = def ? (def.channel_name || "").toLowerCase() : null;
  }

  // Resolve the platform of the target room — drives the compose-bar
  // per-platform accent. chat-rooms metadata may report `youtube`
  // (rejected for chat today, connectable:false), so we coerce here.
  const targetMeta = (chatState.rooms || []).find((r) => r.room === targetRoom);
  const targetPlatform = (targetMeta?.platform || "twitch").toLowerCase();

  if (targetRoom === (playerState.chatRailMount ? playerState.chatRailRoom : null) && targetRoom) {
    // Already mounted to the right room — just sync UI bits.
    playerState.chatRailRoom = targetRoom;
    if (roomLabel) roomLabel.textContent = `#${targetRoom}`;
    tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.room === targetRoom);
    });
    if (playerState.chatRailCompose) {
      playerState.chatRailCompose.setRoom(targetRoom, targetPlatform);
    }
    return;
  }

  // Different room (or first mount, or no room): drop old, mount new.
  if (playerState.chatRailMount) {
    try { playerState.chatRailMount.teardown(); } catch (_) {}
    playerState.chatRailMount = null;
  }
  playerState.chatRailRoom = targetRoom;
  if (!targetRoom) {
    body.innerHTML = `<div class="player-chat-rail-empty pg-cap-hint">No Twitch stream in the layout.</div>`;
    if (roomLabel) roomLabel.textContent = "—";
    // Tear down compose too — nothing to send to.
    if (playerState.chatRailCompose) {
      try { playerState.chatRailCompose.teardown(); } catch (_) {}
      playerState.chatRailCompose = null;
    }
    return;
  }
  if (roomLabel) roomLabel.textContent = `#${targetRoom}`;
  tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.room === targetRoom);
  });

  // Ensure the compose bar exists — mount once per rail-open session,
  // then call setRoom on each subsequent target change.
  const composeHost = rail.querySelector("#player-chat-rail-compose");
  if (composeHost && !playerState.chatRailCompose) {
    composeHost.innerHTML = "";
    playerState.chatRailCompose = mountChatCompose(composeHost, {
      room: targetRoom,
      platform: targetPlatform,
      compact: true,
    });
  }

  // Mount the message body for the target room.
  const proceed = () => {
    if (playerState.chatRailRoom !== targetRoom) return; // race: another switch landed
    playerState.chatRailMount = mountChatRail(body, targetRoom);
    if (playerState.chatRailCompose) {
      playerState.chatRailCompose.setRoom(targetRoom, targetPlatform);
    }
  };
  // Pre-populate the rooms cache if the rail came up before /chat
  // ever ran (otherwise per-channel emote/badge prefetch can't find
  // user_id).
  if (!chatState.rooms || chatState.rooms.length === 0) {
    API.chatRooms()
      .then((r) => { chatState.rooms = r.rooms || []; proceed(); })
      .catch(() => proceed());
  } else {
    proceed();
  }
}

function getNodeAt(layout, path) {
  let n = layout;
  for (const step of pathParts(path)) n = n[step];
  return n;
}

// Replace the node at path with newNode (immutably-ish — we structuredClone
// the root and patch). Returns the new root.
function setNodeAt(layout, path, newNode) {
  const root = structuredClone(layout);
  if (!path) return newNode;
  const parts = pathParts(path);
  let parent = root;
  for (let i = 0; i < parts.length - 1; i++) parent = parent[parts[i]];
  parent[parts[parts.length - 1]] = newNode;
  return root;
}

function savePlayerLayout() {
  try {
    localStorage.setItem(PLAYER_LAYOUT_KEY, JSON.stringify(playerState.layout));
    localStorage.setItem(PLAYER_PRESET_KEY, playerState.preset);
  } catch (_) {}
}

// Validate a parsed layout tree. Rejects null/undefined, unknown kinds,
// out-of-range ratios, malformed children. Recursive — every node has
// to pass on its own merits.
function validatePlayerLayout(node) {
  if (!node || typeof node !== "object") return false;
  if (node.kind === "slot") {
    // streamId / recordingId either null/undefined or non-empty string.
    if (node.streamId !== null && node.streamId !== undefined && typeof node.streamId !== "string") return false;
    if (node.recordingId !== null && node.recordingId !== undefined && typeof node.recordingId !== "string") return false;
    return true;
  }
  if (node.kind === "split") {
    if (node.dir !== "h" && node.dir !== "v") return false;
    if (typeof node.ratio !== "number" || node.ratio <= 0 || node.ratio >= 1) return false;
    return validatePlayerLayout(node.a) && validatePlayerLayout(node.b);
  }
  return false;
}

function loadPlayerLayout() {
  try {
    const raw = localStorage.getItem(PLAYER_LAYOUT_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (validatePlayerLayout(parsed)) {
        playerState.layout = parsed;
        playerState.preset = localStorage.getItem(PLAYER_PRESET_KEY) || "custom";
        return;
      }
    }
  } catch (_) {}
  // Anything invalid drops back to a single empty slot. The user is
  // never silently stuck with a corrupted layout.
  playerState.layout = PLAYER_PRESETS.single();
  playerState.preset = "single";
}

async function renderWatch() {
  // teardownAcrossRoutes() in render() already cleared any prior refresh
  // poll (playerState.refreshTimer + the legacy _watchRefreshTimer
  // alias). A12 consolidation — don't duplicate the clear here.
  if (!playerState.layout) loadPlayerLayout();

  // Honour URL params from rail / dashboard clicks:
  //   ?focus=<streamId>      → load that LIVE stream into the (empty) single slot
  //   ?recording=<recId>     → load that RECORDING into the (empty) single slot
  // 'fresh=1' forces a single-slot reset before loading so the user
  // doesn't end up dropping a click target into a stale multi-tile layout.
  const params = new URLSearchParams(window.location.hash.split("?")[1] || "");
  const focusId = params.get("focus") || "";
  const recordingId = params.get("recording") || "";
  const fresh = params.get("fresh") === "1";
  const seekTo = parseFloat(params.get("t") || "0") || 0;
  // B12: explicit null/undefined check — a serialised slot with
  // streamId: "" should still be treated as empty here.
  const slotIsEmpty = playerState.layout.kind === "slot"
    && (playerState.layout.streamId == null || playerState.layout.streamId === "")
    && (playerState.layout.recordingId == null || playerState.layout.recordingId === "");
  if ((focusId || recordingId) && (fresh || slotIsEmpty)) {
    playerState.preset = "single";
    if (recordingId) playerState.layout = _slot(null, recordingId);
    else playerState.layout = _slot(focusId);
    savePlayerLayout();
  }

  // Channel + recording caches are guaranteed hydrated by render()'s
  // ensureRouteHydration() before this runs.
  // The watch root holds a `.watch-content` (toolbar + stage) and an
  // `<aside class="player-chat-rail">` that follows the focused tile.
  // The rail can be collapsed via the toggle; its open/closed state
  // persists in localStorage.
  const railOpen = playerState.chatRailOpen ? "true" : "false";
  const railToggleGlyph = playerState.chatRailOpen ? "▶" : "◀";
  const railTitle = playerState.chatRailOpen ? "Collapse chat rail" : "Open chat rail";
  root.innerHTML = chrome(`
    <div id="watch" class="watch-root ${playerState.chatRailOpen ? "has-chat-rail" : ""}" role="main">
      <div class="watch-content"><div class="empty">Loading…</div></div>
      <aside class="player-chat-rail" id="player-chat-rail" data-open="${railOpen}">
        <div class="player-chat-rail-head">
          <button class="player-chat-rail-toggle sm" id="player-chat-rail-toggle"
                  type="button" title="${railTitle}" aria-pressed="${railOpen}">${railToggleGlyph}</button>
          <span class="player-chat-rail-title">Chat</span>
          <span class="player-chat-rail-room"></span>
        </div>
        <div class="player-chat-rail-tabs" role="tablist"
             title="Tap a stream to switch chats"></div>
        <div class="player-chat-rail-body" role="log" aria-live="polite"></div>
        <div class="player-chat-rail-compose" id="player-chat-rail-compose"></div>
      </aside>
    </div>
  `);
  setupChromeHandlers();
  const watch = document.getElementById("watch");
  const watchContent = watch.querySelector(".watch-content");

  // Rail toggle — flip persisted state, sync class + glyph, reconcile.
  document.getElementById("player-chat-rail-toggle")?.addEventListener("click", () => {
    playerState.chatRailOpen = !playerState.chatRailOpen;
    savePlayerChatRailOpen();
    watch.classList.toggle("has-chat-rail", playerState.chatRailOpen);
    const rail = document.getElementById("player-chat-rail");
    if (rail) rail.dataset.open = playerState.chatRailOpen ? "true" : "false";
    const btn = document.getElementById("player-chat-rail-toggle");
    if (btn) {
      btn.textContent = playerState.chatRailOpen ? "▶" : "◀";
      btn.title = playerState.chatRailOpen ? "Collapse chat rail" : "Open chat rail";
      btn.setAttribute("aria-pressed", playerState.chatRailOpen ? "true" : "false");
    }
    reconcilePlayerChatRail(playerState.chatRailLastStreams);
  });

  let resp;
  try {
    // Backend still drives 'which live streams are present + embed URLs';
    // we ignore its tile geometry and lay things out via the layout tree.
    resp = await API.multistreamTiles(800, 450, { mode: "auto" }, window.location.host);
  } catch (e) {
    watchContent.innerHTML = `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`;
    return;
  }
  // `let`, not `const`: the background refresh below updates this baseline
  // in place when the live-set changes, so a one-off go-live/offline doesn't
  // make every subsequent tick compare against a stale set.
  let streams = resp.streams || [];
  playerState.chatRailLastStreams = streams;
  paintPlayerStage(watchContent, streams);
  reconcilePlayerChatRail(streams);
  // Apply ?t=<sec> from in-context tools (Crunchr transcript jump,
  // cuepoints tick, EDL jumps) once the video element has rendered.
  if (seekTo > 0) {
    setTimeout(() => {
      const v = watchContent.querySelector("video.ms-video");
      if (v) {
        const apply = () => { try { v.currentTime = seekTo; v.play?.(); } catch (_) {} };
        if (v.readyState >= 1) apply();
        else v.addEventListener("loadedmetadata", apply, { once: true });
      }
    }, 0);
  }

  // Background refresh: poll the tiles endpoint every 30s and patch the
  // per-tile viewer counts in place. Avoids tearing the iframes (the
  // streams keep playing) but keeps the meta-line fresh.
  playerState.refreshTimer = setInterval(async () => {
    if (document.hidden) return;
    try {
      const r = await API.multistreamTiles(800, 450, { mode: "auto" }, window.location.host);
      const byId = new Map((r.streams || []).map((s) => [s.stream_id, s]));
      const have = new Set(streams.map((s) => s.stream_id));
      const got = new Set([...byId.keys()]);
      const sameSet = have.size === got.size && [...have].every((x) => got.has(x));
      if (!sameSet) {
        // Live-set changed (a followed channel went live/offline — routine
        // with a dozen follows). Repaint via paintPlayerStage, NOT
        // renderWatch: paintPlayerStage's fast path patches only the slots
        // whose contents actually changed and re-attaches the still-playing
        // iframes, so the rail's stream-picker refreshes without reloading
        // the open stream. It also rebuilds the rail itself.
        //
        // Calling renderWatch here was the reload bug: it did a full
        // root.innerHTML reset (remounting every iframe) AND armed a fresh
        // 30s interval without clearing this one. Because each interval's
        // `streams` baseline was frozen, a single set change made it mismatch
        // forever — so it pumped a renderWatch every tick, the intervals
        // multiplied, and the player reloaded on a ~30-90s beat.
        //
        // Update the baseline so the next tick compares against reality.
        streams = r.streams || [];
        playerState.chatRailLastStreams = streams;
        paintPlayerStage(watchContent, streams);
        return;
      }
      // Same set — patch viewer counts in place.
      watchContent.querySelectorAll(".ms-leaf").forEach((tile) => {
        const s = byId.get(tile.dataset.streamId);
        if (!s) return;
        const meta = tile.querySelector('[data-watch-meta="viewers"]');
        if (meta && s.viewer_count != null) meta.textContent = formatCount(s.viewer_count);
      });
    } catch (_) {}
  }, 30000);
  // A12: keep the legacy _watchRefreshTimer in sync so any external
  // teardown (test harness, browser extension) that knew about the
  // old name still works during the deprecation window.
  _watchRefreshTimer = playerState.refreshTimer;
}

// ── Player stage rendering + interactions ────────────────────────────
//
// paintPlayerStage walks the layout tree, emits HTML, then wires every
// interaction (preset menu, split buttons, gutter drag, slot stream
// picker, click-to-swap drag-drop, fullscreen, solo).

// True iff two layouts have an identical tree shape (kind + split dir at
// every node). Slot contents (streamId / recordingId) may differ; that
// is the patchable subset. Anything else (split→slot, dir flip, etc.)
// triggers a full repaint.
function sameLayoutShape(a, b) {
  if (!a || !b) return false;
  if (a.kind !== b.kind) return false;
  if (a.kind === "split") {
    return a.dir === b.dir && sameLayoutShape(a.a, b.a) && sameLayoutShape(a.b, b.b);
  }
  return true; // both slots — shape matches regardless of content
}

// Re-bind interactive handlers on a single freshly-replaced .ms-leaf
// tile. Mirrors the per-tile loops at the bottom of paintPlayerStage so
// surgical patches don't need a full repaint just to wire up drag /
// drop / solo / fs / remove / slot-picker on one tile.
function wireTileHandlers(tile, stage, watch, streams) {
  // Tile-source drag (populated tiles only).
  if (tile.dataset.streamId || tile.dataset.recordingId) {
    tile.draggable = true;
    tile.addEventListener("dragstart", (e) => {
      e.dataTransfer.setData("text/plain", `strivo-tile:${tile.dataset.path || ""}`);
      e.dataTransfer.effectAllowed = "move";
      tile.classList.add("is-dragging");
    });
    tile.addEventListener("dragend", () => tile.classList.remove("is-dragging"));
  }
  // Drop targets — every tile, populated or empty.
  tile.addEventListener("dragover", (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    tile.classList.add("is-drop-target");
  });
  tile.addEventListener("dragleave", () => tile.classList.remove("is-drop-target"));
  tile.addEventListener("drop", (e) => {
    e.preventDefault();
    tile.classList.remove("is-drop-target");
    const parsed = parsePlayerDragPayload(e.dataTransfer.getData("text/plain"));
    const toPath = tile.dataset.path || "";
    if (!parsed) return;
    if (parsed.type === "tile") {
      const fromPath = parsed.path;
      if (fromPath === toPath) return;
      const fromNode = getNodeAt(playerState.layout, fromPath);
      const toNode = getNodeAt(playerState.layout, toPath);
      let next = setNodeAt(
        playerState.layout,
        fromPath,
        _slot(toNode.streamId || null, toNode.recordingId || null),
      );
      next = setNodeAt(
        next,
        toPath,
        _slot(fromNode.streamId || null, fromNode.recordingId || null),
      );
      playerState.layout = next;
      savePlayerLayout();
      paintPlayerStage(watch, streams);
      return;
    }
    if (parsed.type === "stream") {
      playerState.layout = setNodeAt(playerState.layout, toPath, _slot(parsed.id, null));
      savePlayerLayout();
      // If the dropped stream isn't in our current cache (rail showed
      // it as live but the multistream snapshot is stale — race between
      // the channels poll and the tiles poll), refresh before
      // repainting so renderSlot can resolve the embed URL. Without
      // this the dropped tile renders the "Stream offline" pill even
      // though the channel is fine — the bug the user kept hitting.
      if (!streams.find((x) => x.stream_id === parsed.id)) {
        API.multistreamTiles(800, 450, { mode: "auto" }, window.location.host)
          .then((resp) => {
            const fresh = resp.streams || [];
            playerState.chatRailLastStreams = fresh;
            paintPlayerStage(watch, fresh);
          })
          .catch((err) => {
            Toast.error(`Couldn't load dropped stream: ${err && err.message || err}`);
            paintPlayerStage(watch, streams);
          });
        return;
      }
      paintPlayerStage(watch, streams);
      return;
    }
    if (parsed.type === "recording") {
      playerState.layout = setNodeAt(playerState.layout, toPath, _slot(null, parsed.id));
      savePlayerLayout();
      paintPlayerStage(watch, streams);
    }
  });

  // Focus on background click (not on buttons / iframe / picker).
  tile.addEventListener("mousedown", (e) => {
    if (e.target.closest("button, select, iframe")) return;
    stage.querySelectorAll(".ms-leaf.is-focused").forEach((x) => x.classList.remove("is-focused"));
    tile.classList.add("is-focused");
  });

  // Empty-slot stream picker.
  const sel = tile.querySelector("select.ms-slot-pick");
  if (sel) {
    sel.addEventListener("change", () => {
      const path = sel.dataset.path || "";
      const val = sel.value;
      if (!val) return;
      let next;
      if (val.startsWith("rec:")) next = _slot(null, val.slice(4));
      else if (val.startsWith("live:")) next = _slot(val.slice(5), null);
      else next = _slot(val);
      playerState.layout = setNodeAt(playerState.layout, path, next);
      savePlayerLayout();
      paintPlayerStage(watch, streams);
    });
  }

  // Solo / unsolo / fs / remove buttons.
  tile.querySelector(".ms-solo")?.addEventListener("click", (e) => {
    e.stopPropagation();
    soloTileAt(tile.dataset.path || "");
    paintPlayerStage(watch, streams);
  });
  tile.querySelector(".ms-unsolo")?.addEventListener("click", (e) => {
    e.stopPropagation();
    muteAllTiles();
    paintPlayerStage(watch, streams);
  });
  // Per-tile level. Applied straight to the controller so dragging the
  // slider is audible immediately rather than after a repaint.
  const vol = tile.querySelector(".ms-vol");
  if (vol) {
    vol.addEventListener("click", (e) => e.stopPropagation());
    vol.addEventListener("input", () => {
      const path = tile.dataset.path || "";
      const v = Number(vol.value) / 100;
      setTileVolumeAt(path, v);
      const key = contentKeyOf(getNodeAt(playerState.layout, path));
      const ctl = key && playerState.controllers.get(key);
      if (ctl) {
        try {
          ctl.setVolume(v);
          ctl.setMuted(v === 0);
        } catch (_) {
          /* advisory */
        }
      }
      // Raising a tile focuses it, so chat and focus-aware quality follow
      // what you are actually listening to.
      if (v > 0) playerState.soloPath = path;
    });
  }
  tile.querySelector(".ms-fs")?.addEventListener("click", (e) => {
    e.stopPropagation();
    try {
      if (document.fullscreenElement) document.exitFullscreen?.();
      else tile.requestFullscreen?.();
    } catch (err) {
      Toast.error(`Fullscreen denied: ${err && err.message || err}`);
    }
  });
  // Press-to-play on a paused tile. Starting one tile does not start the
  // wall — that is the whole point of opening paused.
  tile.querySelector(".ms-play")?.addEventListener("click", (e) => {
    e.preventDefault();
    e.stopPropagation();
    const path = e.currentTarget.dataset.path || "";
    setTilePlaying(getNodeAt(playerState.layout, path), true);
    // Playing state is not part of the layout tree, so the shape-diffing
    // patch path cannot see this change — force the full paint. That is
    // cheap now: the repaint re-parents existing players instead of
    // rebuilding them, so starting one tile does not disturb the others.
    playerState.lastPaintedLayout = null;
    paintPlayerStage(watch, streams);
  });
  tile.querySelector(".ms-remove")?.addEventListener("click", (e) => {
    e.stopPropagation();
    playerState.layout = setNodeAt(playerState.layout, tile.dataset.path || "", _slot());
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  });
}

// Try to apply a slot-content diff between `prev` and `curr` without
// rebuilding the whole stage. Returns true if the patch succeeded
// (caller can skip the full repaint).
//
// Preserves existing iframes for any slot whose streamId/recordingId is
// unchanged — Twitch embeds pause/reload on DOM detach, so the only
// safe way to "add a stream" without nuking the others is to leave
// their tile DOM entirely alone. Falls back to a full repaint when the
// tree shape changes (split / collapse / preset switch).
function tryPatchPlayerStage(watch, prev, curr, streams) {
  const stage = watch.querySelector(".ms-stage");
  if (!stage || !prev || !sameLayoutShape(prev, curr)) return false;

  // Walk both trees in lockstep, patching only slots whose content
  // changed. The mute state for unchanged tiles is also reapplied
  // (mute is a function of soloPath, not slot content).
  const work = [];
  (function walkParallel(p, c, path) {
    if (p.kind === "split") {
      walkParallel(p.a, c.a, path ? `${path}.a` : "a");
      walkParallel(p.b, c.b, path ? `${path}.b` : "b");
      return;
    }
    work.push({ path, prevNode: p, currNode: c });
  })(prev, curr, "");

  for (const { path, prevNode, currNode } of work) {
    const tile = stage.querySelector(`.ms-leaf[data-path="${cssEscape(path)}"]`);
    if (!tile) return false; // shape thought it was the same but DOM disagrees → bail
    const sameContent =
      (prevNode.streamId || null) === (currNode.streamId || null) &&
      (prevNode.recordingId || null) === (currNode.recordingId || null);
    if (sameContent) {
      // Same content. Mute may have flipped; the controller owns how that
      // is applied, so this path no longer touches media elements at all.
      const muted = computeMuted(path);
      // Swap solo/unsolo button label.
      const soloBtn = tile.querySelector(".ms-solo, .ms-unsolo");
      if (soloBtn) {
        if (muted && !soloBtn.classList.contains("ms-solo")) {
          soloBtn.classList.remove("ms-unsolo");
          soloBtn.classList.add("ms-solo");
          soloBtn.textContent = "🔇";
          soloBtn.title = "Unmute (solo this tile)";
        } else if (!muted && !soloBtn.classList.contains("ms-unsolo")) {
          soloBtn.classList.remove("ms-solo");
          soloBtn.classList.add("ms-unsolo");
          soloBtn.textContent = "🔊";
          soloBtn.title = "Mute (mute-all)";
        }
      }
      continue;
    }
    // Content changed. Render the new tile, swap it in, re-wire it.
    // Existing iframes in OTHER tiles never get detached — only this
    // tile's old element does.
    const tmp = document.createElement("div");
    tmp.innerHTML = renderSlot(currNode, path, streams);
    const newTile = tmp.firstElementChild;
    if (!newTile) return false;
    tile.replaceWith(newTile);
    wireTileHandlers(newTile, stage, watch, streams);
  }

  // Apply mute / mount any tile this patch swapped in. The patch path
  // edits tiles in place, so without this a solo change would update the
  // button label and nothing else.
  reconcileControllers(watch.querySelector(".ms-stage"));

  // Toolbar reflects layout-aware bits (preset label, leaf count,
  // mute-all pressed state). Tree shape didn't change, so the only
  // things that can shift are leaf-count (same — slots unchanged) and
  // the mute-all pressed state.
  const muteAll = watch.querySelector("#watch-mute-all");
  if (muteAll) muteAll.classList.toggle("active", !playerState.soloPath);
  return true;
}

// Robust CSS attribute-selector escaping for data-path values (root is
// "" and paths are alphabetic). Falls back when the browser lacks
// CSS.escape (some older runtimes).
function cssEscape(s) {
  if (typeof CSS !== "undefined" && CSS && typeof CSS.escape === "function") {
    return CSS.escape(s);
  }
  return String(s).replace(/[^A-Za-z0-9_-]/g, (m) => `\\${m}`);
}

function paintPlayerStage(watch, streams) {
  const layout = playerState.layout;
  // Fast path: if the tree shape didn't change, patch only the slots
  // whose contents differ. Unchanged tiles' DOM is never touched, so
  // their iframes never detach — adding / removing / dropping a
  // single stream no longer reloads the others. Falls through to the
  // full repaint when the tree shape changes (split / collapse / preset
  // switch / first paint).
  if (playerState.lastPaintedLayout
      && tryPatchPlayerStage(watch, playerState.lastPaintedLayout, layout, streams)) {
    playerState.lastPaintedLayout = structuredClone(layout);
    playerState.chatRailLastStreams = streams;
    reconcilePlayerChatRail(streams);
    return;
  }
  const leaves = countLeaves(layout);

  // Build the preset menu (rendered as a details element). The current
  // preset's label is the summary; click expands to the option list.
  const presetLabel = PLAYER_PRESET_LABELS[playerState.preset] || "Custom";
  const presetOpts = Object.entries(PLAYER_PRESET_LABELS).map(([k, v]) => `
    <button class="sm ms-preset-opt${k === playerState.preset ? " active" : ""}" type="button" data-preset="${k}">${htmlEscape(v)}</button>`).join("");

  // Toolbar — preset dropdown · split buttons (custom-mode) · mute-all.
  const muteAllPressed = playerState.soloPath ? "" : "active";
  const customTools = playerState.preset === "custom" ? `
    <span class="watch-tb-sep" aria-hidden="true">·</span>
    <span class="pg-cap-hint">Focus a tile, then split:</span>
    <button class="sm ms-split-h" type="button" title="Split focused tile horizontally (side-by-side)">▥ Split H</button>
    <button class="sm ms-split-v" type="button" title="Split focused tile vertically (top + bottom)">▤ Split V</button>
    <button class="sm ms-collapse" type="button" title="Collapse the focused tile back into its sibling">↶ Undo split</button>` : "";
  const toolbar = `
    <div class="watch-toolbar">
      <span class="watch-count pg-cap-hint">${streams.length} live · ${leaves}/${PLAYER_LEAF_CAP} tile${leaves === 1 ? "" : "s"}</span>
      <details class="ms-preset" id="ms-preset-menu">
        <summary class="sm ms-preset-summary" title="Multi-stream layout presets">▦ Multi-stream: ${htmlEscape(presetLabel)} ▾</summary>
        <div class="ms-preset-menu">${presetOpts}</div>
      </details>
      ${customTools}
      <span class="watch-tb-sep" aria-hidden="true">·</span>
      <button class="sm ms-compose-open" id="ms-compose-open" type="button"
              title="Open the multi-view composer — drag streams onto the plane">⊞ Compose</button>
      <button class="sm watch-playall ${playerState.autoplay ? "active" : ""}" id="watch-playall"
              type="button" title="${playerState.autoplay ? "Pause every tile" : "Start every tile"}">${playerState.autoplay ? "⏸ Pause all" : "▶ Play all"}</button>
      <button class="sm watch-mute-all ${muteAllPressed}" id="watch-mute-all" title="Mute every tile">🔇 Mute all</button>
    </div>`;

  // Capture existing iframes/videos before we blow the stage away so
  // we can re-attach them to the new layout intact. Adding a stream,
  // switching presets, splitting tiles, etc. should not reload the
  // streams that were already playing — only newly-added slots
  // actually need fresh media elements. Keyed by stream/recording id
  // because layout paths shift across operations.
  // No detach/reattach dance any more: controllers are not owned by this
  // DOM, so wiping it cannot destroy a player. Rebuild freely, then let
  // reconcileControllers re-parent the survivors.
  const stage = document.createElement("div");
  stage.className = "ms-stage";
  stage.innerHTML = renderLayoutNode(layout, "", streams);

  watch.innerHTML = "";
  watch.insertAdjacentHTML("beforeend", toolbar);
  watch.appendChild(stage);

  reconcileControllers(stage);

  // ── Preset menu ──
  watch.querySelectorAll(".ms-preset-opt").forEach((btn) => {
    btn.addEventListener("click", () => {
      const p = btn.dataset.preset;
      if (!PLAYER_PRESETS[p]) return;
      // Preserve any populated streams across the preset switch. The
      // user expects 'go from single to quadrant' to keep the open
      // stream in the first slot, not to wipe it. Collect every
      // populated slot from the current layout (depth-first, a→b),
      // build the fresh preset, then refill the first N empty slots
      // with the preserved streams in order.
      const preserved = [];
      // Also capture which source path was previously soloed so the
      // user's audio choice survives the layout switch.
      const prevSoloPath = playerState.soloPath || "";
      walkLayout(playerState.layout, (n, path) => {
        if (n.kind === "slot" && (n.streamId || n.recordingId)) {
          preserved.push({ streamId: n.streamId || null, recordingId: n.recordingId || null, wasSoloed: path === prevSoloPath });
        }
      });
      let next = PLAYER_PRESETS[p]();
      let newSoloPath = "";
      if (preserved.length) {
        const slotPaths = [];
        walkLayout(next, (n, path) => {
          if (n.kind === "slot") slotPaths.push(path);
        });
        for (let i = 0; i < Math.min(preserved.length, slotPaths.length); i++) {
          next = setNodeAt(next, slotPaths[i], _slot(preserved[i].streamId, preserved[i].recordingId));
          if (preserved[i].wasSoloed) newSoloPath = slotPaths[i];
        }
      }
      playerState.preset = p;
      playerState.layout = next;
      // Restore soloPath if the soloed source landed in the new layout.
      if (newSoloPath) playerState.soloPath = newSoloPath;
      else if (prevSoloPath) playerState.soloPath = ""; // its source dropped off — fall back to mute-all
      savePlayerLayout();
      paintPlayerStage(watch, streams);
    });
  });

  // ── Custom-mode split / collapse ──
  const splitFocused = (dir) => {
    const focused = stage.querySelector(".ms-leaf.is-focused") || stage.querySelector(".ms-leaf");
    if (!focused) return;
    const path = focused.dataset.path || "";
    if (countLeaves(playerState.layout) >= PLAYER_LEAF_CAP) {
      Toast.error(`Tile cap reached (${PLAYER_LEAF_CAP}) — collapse a tile first.`);
      return;
    }
    const node = getNodeAt(playerState.layout, path);
    if (node.kind !== "slot") return;
    const next = _split(dir, 0.5, _slot(node.streamId), _slot());
    playerState.layout = setNodeAt(playerState.layout, path, next);
    playerState.preset = "custom";
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  };
  watch.querySelector(".ms-split-h")?.addEventListener("click", () => splitFocused("h"));
  watch.querySelector(".ms-split-v")?.addEventListener("click", () => splitFocused("v"));
  watch.querySelector(".ms-collapse")?.addEventListener("click", () => {
    const focused = stage.querySelector(".ms-leaf.is-focused") || stage.querySelector(".ms-leaf");
    if (!focused) return;
    const path = focused.dataset.path || "";
    if (!path) return; // can't collapse the root
    const parts = pathParts(path);
    const parentPath = pathStr(parts.slice(0, -1));
    const parent = getNodeAt(playerState.layout, parentPath);
    if (!parent || parent.kind !== "split") return;
    const siblingKey = parts[parts.length - 1] === "a" ? "b" : "a";
    playerState.layout = setNodeAt(playerState.layout, parentPath, parent[siblingKey]);
    playerState.preset = "custom";
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  });

  // Per-tile interactions (dragstart on populated, drop targets on all,
  // slot picker on empty, solo/fs/remove on populated). Single source
  // of truth — `wireTileHandlers` is also called by the in-place patch
  // path so a surgical tile replacement gets identical wiring.
  stage.querySelectorAll(".ms-leaf").forEach((tile) => {
    wireTileHandlers(tile, stage, watch, streams);
  });

  // Mute-all toolbar button: top-level, not per-tile.
  watch.querySelector("#watch-mute-all")?.addEventListener("click", () => {
    playerState.soloPath = "";
    paintPlayerStage(watch, streams);
  });

  watch.querySelector("#watch-playall")?.addEventListener("click", () => {
    playerState.autoplay = !playerState.autoplay;
    // Pausing the wall also clears per-tile plays, so "Pause all" means it.
    if (!playerState.autoplay) playerState.playing = [];
    savePlayerAutoplay();
    // Full repaint: every tile's src changes, so the patch fast-path has
    // nothing to conserve here.
    playerState.lastPaintedLayout = null;
    paintPlayerStage(watch, streams);
  });

  watch.querySelector("#ms-compose-open")?.addEventListener("click", () => {
    openComposer(watch, streams);
  });

  // Channel-list rail rows are draggable as a stream source. <a> tags
  // are draggable by default (the browser drags the href); our custom
  // dragstart MUST run AND set effectAllowed first so the browser's
  // URL-drag default doesn't win.
  document.querySelectorAll(".ch-row[data-channel-key]").forEach((row) => {
    if (!row.dataset.liveStreamId) return;
    row.draggable = true;
    row.addEventListener("dragstart", (e) => {
      e.dataTransfer.setData("text/plain", `strivo-stream:${row.dataset.liveStreamId}`);
      e.dataTransfer.effectAllowed = "copy";
    });
  });

  // ── Resize gutters ──
  stage.querySelectorAll(".ms-gutter").forEach((gutter) => {
    gutter.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const path = gutter.dataset.path || "";
      const split = getNodeAt(playerState.layout, path);
      if (!split || split.kind !== "split") return;
      const parent = gutter.parentElement;
      const rect = parent.getBoundingClientRect();
      const dir = split.dir;
      playerState.resizeFx = { path, parentRect: rect, dir };
      document.body.classList.add("ms-resizing");
      const onMove = (ev) => {
        const fx = playerState.resizeFx;
        if (!fx) return;
        const pos = fx.dir === "h"
          ? (ev.clientX - fx.parentRect.left) / fx.parentRect.width
          : (ev.clientY - fx.parentRect.top) / fx.parentRect.height;
        const ratio = Math.min(0.9, Math.max(0.1, pos));
        const node = getNodeAt(playerState.layout, fx.path);
        node.ratio = ratio;
        // Live update without full repaint — tweak flex on siblings.
        const a = parent.children[0];
        const b = parent.children[2];
        if (a && b) {
          a.style.flex = `${ratio} ${ratio} 0`;
          b.style.flex = `${1 - ratio} ${1 - ratio} 0`;
        }
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.classList.remove("ms-resizing");
        playerState.resizeFx = null;
        savePlayerLayout();
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    });
  });

  // Snapshot the layout we just painted so the next call can try the
  // fast in-place patch path instead of rebuilding the stage. Stashed
  // *after* the full paint so a mid-paint exception doesn't leave the
  // diff comparing against a partial state.
  playerState.lastPaintedLayout = structuredClone(layout);
  playerState.chatRailLastStreams = streams;
  reconcilePlayerChatRail(streams);
}

// Recursive renderer. Returns an HTML string.
function renderLayoutNode(node, path, streams) {
  if (node.kind === "slot") return renderSlot(node, path, streams);
  // Split: flex container with two children + a gutter between.
  const flexDir = node.dir === "h" ? "row" : "column";
  const r = node.ratio;
  return `
    <div class="ms-split ms-split-${node.dir}" style="flex-direction:${flexDir}" data-path="${htmlEscape(path)}">
      <div class="ms-pane" style="flex:${r} ${r} 0">${renderLayoutNode(node.a, path ? `${path}.a` : "a", streams)}</div>
      <div class="ms-gutter ms-gutter-${node.dir}" data-path="${htmlEscape(path)}" title="Drag to resize"></div>
      <div class="ms-pane" style="flex:${1 - r} ${1 - r} 0">${renderLayoutNode(node.b, path ? `${path}.b` : "b", streams)}</div>
    </div>`;
}

// ── Multi-view composer ──────────────────────────────────────────────
//
// A modal for building the wall: pick a tiling, then fill it by dragging
// sources onto cells or by clicking a cell and then a source. Everything
// applies to the live stage immediately — there is no OK/Cancel, because
// the stage behind the modal *is* the preview.
//
// The mini-map renders the same layout tree the stage uses, so cell paths
// and drop semantics are shared with the real tiles rather than reimplemented.
let composerSelectedPath = null;

function composerSourceChips(streams) {
  const live = (streams || []).map((s) => `
    <button type="button" class="msc-src" draggable="true"
            data-src-kind="stream" data-src-id="${htmlEscape(s.stream_id)}"
            title="${htmlEscape(s.channel_name)} · ${htmlEscape(s.platform)}">
      <span class="msc-src-dot ${htmlEscape(String(s.platform || "").toLowerCase())}"></span>
      <span class="msc-src-name">${htmlEscape(s.channel_name)}</span>
      <span class="msc-src-meta">${s.viewer_count != null ? formatCount(s.viewer_count) : "live"}</span>
    </button>`).join("");
  const recs = (recCache || [])
    .filter((r) => r.state === "Finished" && r.file_exists !== false)
    .sort((a, b) => recordingTime(b) - recordingTime(a))
    .slice(0, 12)
    .map((r) => `
    <button type="button" class="msc-src msc-src-rec" draggable="true"
            data-src-kind="rec" data-src-id="${htmlEscape(r.id)}"
            title="${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id)}">
      <span class="msc-src-dot rec"></span>
      <span class="msc-src-name">${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</span>
      <span class="msc-src-meta">${htmlEscape(shortWhen(r.started_at))}</span>
    </button>`).join("");
  return `
    <div class="msc-src-group">
      <div class="msc-src-head">Live now <span class="ch-count">${(streams || []).length}</span></div>
      <div class="msc-src-list">${live || '<div class="empty sm">No live channels</div>'}</div>
    </div>
    <div class="msc-src-group">
      <div class="msc-src-head">Recent recordings</div>
      <div class="msc-src-list">${recs || '<div class="empty sm">No recordings</div>'}</div>
    </div>`;
}

/// Recursively render the layout tree as a proportional mini-map. Mirrors
/// the stage's own geometry so what you arrange is what you get.
function composerMapHtml(node, path, streams) {
  if (!node) return "";
  if (node.kind === "split") {
    const dir = node.dir === "h" ? "row" : "column";
    const ratio = Math.max(0.1, Math.min(0.9, node.ratio ?? 0.5));
    return `
      <div class="msc-split" style="flex-direction:${dir}">
        <div class="msc-branch" style="flex:${ratio}">${composerMapHtml(node.a, path ? path + ".a" : "a", streams)}</div>
        <div class="msc-branch" style="flex:${1 - ratio}">${composerMapHtml(node.b, path ? path + ".b" : "b", streams)}</div>
      </div>`;
  }
  const sel = composerSelectedPath === path ? " is-selected" : "";
  if (node.streamId) {
    const s = (streams || []).find((x) => x.stream_id === node.streamId);
    const name = s ? s.channel_name : node.streamId;
    const stale = s ? "" : " is-stale";
    return `
      <div class="msc-cell is-filled${sel}${stale}" data-path="${htmlEscape(path)}" draggable="true" tabindex="0">
        <span class="msc-cell-name">${htmlEscape(name)}</span>
        ${s ? "" : '<span class="msc-cell-warn">offline</span>'}
        <button type="button" class="msc-cell-clear" data-clear="${htmlEscape(path)}" title="Clear this tile" aria-label="Clear tile">✕</button>
      </div>`;
  }
  if (node.recordingId) {
    const r = (recCache || []).find((x) => x.id === node.recordingId);
    const name = r ? (niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8)) : node.recordingId.slice(0, 8);
    return `
      <div class="msc-cell is-filled is-rec${sel}" data-path="${htmlEscape(path)}" draggable="true" tabindex="0">
        <span class="msc-cell-name">${htmlEscape(name)}</span>
        <button type="button" class="msc-cell-clear" data-clear="${htmlEscape(path)}" title="Clear this tile" aria-label="Clear tile">✕</button>
      </div>`;
  }
  return `
    <div class="msc-cell${sel}" data-path="${htmlEscape(path)}" tabindex="0">
      <span class="msc-cell-empty">+</span>
    </div>`;
}

function firstEmptyPath(layout) {
  let found = null;
  walkLayout(layout, (node, path) => {
    if (found === null && node.kind === "slot" && !node.streamId && !node.recordingId) found = path;
  });
  return found;
}

function openComposer(watch, streams) {
  const existing = document.getElementById("ms-composer");
  if (existing) existing.remove();
  composerSelectedPath = null;

  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.id = "ms-composer";
  document.body.appendChild(overlay);

  const repaintStage = () => {
    savePlayerLayout();
    playerState.lastPaintedLayout = null;
    paintPlayerStage(watch, streams);
  };

  const draw = () => {
    const presets = Object.entries(PLAYER_PRESET_LABELS)
      .filter(([k]) => k !== "custom")
      .map(([k, v]) => `<button type="button" class="sm msc-preset${k === playerState.preset ? " active" : ""}" data-preset="${k}">${htmlEscape(v)}</button>`)
      .join("");
    overlay.innerHTML = `
      <div class="modal-card msc-card" role="dialog" aria-modal="true" aria-label="Multi-view composer">
        <div class="modal-head">
          <h2 class="msc-title">Multi-view composer</h2>
          <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
        </div>
        <div class="msc-body">
          <aside class="msc-sources">${composerSourceChips(streams)}</aside>
          <section class="msc-plane">
            <div class="msc-presets">${presets}</div>
            <div class="msc-map" id="msc-map">${composerMapHtml(playerState.layout, "", streams)}</div>
            <div class="msc-hint pg-cap-hint">
              Drag a source onto a tile, or click a tile then a source. Drag tile-to-tile to swap.
            </div>
          </section>
        </div>
        <div class="msc-foot">
          <label class="msc-toggle">
            <input type="checkbox" id="msc-paused" ${playerState.autoplay ? "" : "checked"}>
            <span>Open paused</span>
          </label>
          <span class="pg-cap-hint">${countLeaves(playerState.layout)}/${PLAYER_LEAF_CAP} tiles</span>
          <span class="msc-foot-spacer"></span>
          <button type="button" class="sm" id="msc-clear-all">Clear all</button>
          <button type="button" class="sm primary" id="msc-playall">${playerState.autoplay ? "⏸ Pause all" : "▶ Play all"}</button>
        </div>
      </div>`;
    wire();
  };

  const assign = (path, kind, id) => {
    // The root tile's path is "", which is falsy — never test these paths
    // for truthiness or a single-tile layout looks like "no tile at all".
    if (path === null || path === undefined) return;
    playerState.layout = setNodeAt(
      playerState.layout,
      path,
      kind === "rec" ? _slot(null, id) : _slot(id, null),
    );
    repaintStage();
    draw();
  };

  const wire = () => {
    overlay.querySelector('[data-action="modal-close"]')?.addEventListener("click", () => overlay.remove());
    overlay.addEventListener("mousedown", (e) => { if (e.target === overlay) overlay.remove(); });

    overlay.querySelectorAll(".msc-preset").forEach((b) => {
      b.addEventListener("click", () => {
        const k = b.dataset.preset;
        if (!PLAYER_PRESETS[k]) return;
        playerState.preset = k;
        playerState.layout = PLAYER_PRESETS[k]();
        playerState.playing = [];
        composerSelectedPath = null;
        repaintStage();
        draw();
      });
    });

    // Sources: draggable, and click-to-assign into the selected (or first
    // empty) cell so the whole flow works without a drag if you prefer.
    overlay.querySelectorAll(".msc-src").forEach((chip) => {
      chip.addEventListener("dragstart", (e) => {
        const kind = chip.dataset.srcKind === "rec" ? "strivo-rec:" : "strivo-stream:";
        e.dataTransfer.setData("text/plain", kind + chip.dataset.srcId);
        e.dataTransfer.effectAllowed = "copy";
      });
      chip.addEventListener("click", () => {
        const target =
          composerSelectedPath !== null ? composerSelectedPath : firstEmptyPath(playerState.layout);
        if (target === null || target === undefined) {
          Toast.error("No empty tile — pick a bigger layout or clear one.");
          return;
        }
        assign(target, chip.dataset.srcKind, chip.dataset.srcId);
      });
    });

    overlay.querySelectorAll(".msc-cell").forEach((cell) => {
      const path = cell.dataset.path || "";
      cell.addEventListener("click", (e) => {
        if (e.target.closest(".msc-cell-clear")) return;
        composerSelectedPath = composerSelectedPath === path ? null : path;
        draw();
      });
      cell.addEventListener("dragover", (e) => {
        e.preventDefault();
        cell.classList.add("is-drop-target");
      });
      cell.addEventListener("dragleave", () => cell.classList.remove("is-drop-target"));
      cell.addEventListener("drop", (e) => {
        e.preventDefault();
        cell.classList.remove("is-drop-target");
        const raw = e.dataTransfer.getData("text/plain") || "";
        if (raw.startsWith("strivo-rec:")) return assign(path, "rec", raw.slice("strivo-rec:".length));
        const parsed = parsePlayerDragPayload(raw);
        if (!parsed) return;
        if (parsed.type === "stream") return assign(path, "stream", parsed.id);
        if (parsed.type === "tile" && parsed.path !== path) {
          const from = getNodeAt(playerState.layout, parsed.path);
          const to = getNodeAt(playerState.layout, path);
          let next = setNodeAt(playerState.layout, parsed.path, _slot(to.streamId || null, to.recordingId || null));
          next = setNodeAt(next, path, _slot(from.streamId || null, from.recordingId || null));
          playerState.layout = next;
          repaintStage();
          draw();
        }
      });
      if (cell.classList.contains("is-filled")) {
        cell.addEventListener("dragstart", (e) => {
          e.dataTransfer.setData("text/plain", `strivo-tile:${path}`);
          e.dataTransfer.effectAllowed = "move";
        });
      }
    });

    overlay.querySelectorAll(".msc-cell-clear").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        playerState.layout = setNodeAt(playerState.layout, btn.dataset.clear || "", _slot());
        repaintStage();
        draw();
      });
    });

    overlay.querySelector("#msc-clear-all")?.addEventListener("click", () => {
      walkLayout(playerState.layout, (node, path) => {
        if (node.kind === "slot") {
          playerState.layout = setNodeAt(playerState.layout, path, _slot());
        }
      });
      repaintStage();
      draw();
    });

    overlay.querySelector("#msc-paused")?.addEventListener("change", (e) => {
      playerState.autoplay = !e.currentTarget.checked;
      if (!playerState.autoplay) playerState.playing = [];
      savePlayerAutoplay();
      repaintStage();
      draw();
    });

    overlay.querySelector("#msc-playall")?.addEventListener("click", () => {
      playerState.autoplay = !playerState.autoplay;
      if (!playerState.autoplay) playerState.playing = [];
      savePlayerAutoplay();
      repaintStage();
      draw();
    });
  };

  draw();
  const onEsc = (e) => {
    if (e.key === "Escape") { overlay.remove(); document.removeEventListener("keydown", onEsc); }
  };
  document.addEventListener("keydown", onEsc);
}

/// Poster frame for a paused live tile.
///
/// A paused tile deliberately renders NO iframe: a Twitch/YouTube embed
/// downloads and boots an entire player application even when it is not
/// going to play, which on a 9-tile wall is nine player apps for a grid
/// nobody has pressed play on yet. A still frame costs one image.
function tilePosterUrl(s) {
  if (!s) return null;
  const c = (channelCache || []).find((x) => `${x.platform}:${x.id}` === s.stream_id);
  return c ? liveThumbUrl(c) : null;
}

function renderSlot(slot, path, streams) {
  const muted = computeMuted(path);
  const playing = tilePlaying(slot);
  // ─ Recording playback path ─
  if (slot.recordingId) {
    const rec = recCache.find((r) => r.id === slot.recordingId);
    const title = rec ? (niceTitle(rec.stream_title) || rec.channel_name || rec.id.slice(0, 8)) : slot.recordingId.slice(0, 8);
    const channel = rec ? rec.channel_name || "" : "";
    const soloBtn = muted
      ? `<button class="watch-tile-btn ms-solo" title="Unmute this clip" data-path="${htmlEscape(path)}">🔇</button>`
      : `<button class="watch-tile-btn ms-unsolo" title="Mute" data-path="${htmlEscape(path)}">🔊</button>`;
    return `
      <div class="ms-leaf ms-leaf-rec" data-path="${htmlEscape(path)}" data-recording-id="${htmlEscape(slot.recordingId)}">
        <div class="watch-tile-head">
          <span class="watch-tile-name">${htmlEscape(title)}</span>
          <span class="watch-tile-meta">
            <span class="watch-tile-plat pg-cap-hint">${htmlEscape(channel)} · recording</span>
            ${soloBtn}
            <button class="watch-tile-btn ms-fs" title="Fullscreen this tile">⛶</button>
            <button class="watch-tile-btn ms-remove" title="Remove from layout" data-path="${htmlEscape(path)}">✕</button>
          </span>
        </div>
        <div class="ms-mount" data-content-key="r:${htmlEscape(slot.recordingId)}"
             data-kind="recording" data-path="${htmlEscape(path)}"
             data-playing="${playing ? "1" : "0"}"
             data-src="/api/v1/recordings/${encodeURIComponent(slot.recordingId)}/download"></div>
      </div>`;
  }
  // ─ Live stream path ─
  if (slot.streamId) {
    const s = streams.find((x) => x.stream_id === slot.streamId);
    if (!s) {
      return `
        <div class="ms-leaf ms-empty" data-path="${htmlEscape(path)}">
          <div class="ms-empty-pill">Stream offline · drag a live channel here</div>
        </div>`;
    }
    const soloBtn = muted
      ? `<button class="watch-tile-btn ms-solo" title="Solo — raise this tile, silence the rest" data-path="${htmlEscape(path)}">🔇</button>`
      : `<button class="watch-tile-btn ms-unsolo" title="Silence every tile" data-path="${htmlEscape(path)}">🔊</button>`;
    // Per-tile level, so two streams can run at different volumes rather
    // than only one-audible or all-silent.
    const volPct = Math.round(tileVolumeAt(path) * 100);
    const volCtl = `<input class="ms-vol" type="range" min="0" max="100" step="1"
             value="${volPct}" data-path="${htmlEscape(path)}"
             aria-label="Volume for ${htmlEscape(s.channel_name)}"
             title="Volume — ${volPct}%">`;
    return `
      <div class="ms-leaf" data-path="${htmlEscape(path)}" data-stream-id="${htmlEscape(s.stream_id)}">
        <div class="watch-tile-head">
          <span class="watch-tile-name">${htmlEscape(s.channel_name)}</span>
          <span class="watch-tile-meta">
            <span class="watch-tile-plat pg-cap-hint" data-watch-meta="plat">${htmlEscape(s.platform)}${s.viewer_count != null ? ` · <span data-watch-meta="viewers">${formatCount(s.viewer_count)}</span>` : ""}</span>
            ${volCtl}
            ${soloBtn}
            <button class="watch-tile-btn ms-fs" title="Fullscreen this tile">⛶</button>
            <button class="watch-tile-btn ms-remove" title="Remove from layout" data-path="${htmlEscape(path)}">✕</button>
          </span>
        </div>
        ${playing
          ? `<div class="ms-mount" data-content-key="s:${htmlEscape(s.stream_id)}"
                  data-kind="${s.platform === "YouTube" ? "youtube" : "twitch"}"
                  data-path="${htmlEscape(path)}" data-playing="1"
                  data-embed-base="${htmlEscape(s.embed_url)}"></div>`
          : `<div class="ms-poster" data-embed-base="${htmlEscape(s.embed_url)}">
               ${tilePosterUrl(s) ? `<img class="ms-poster-img" loading="lazy" alt="" src="${htmlEscape(tilePosterUrl(s))}" onerror="this.remove()">` : ""}
               <button class="ms-play" data-path="${htmlEscape(path)}" title="Play ${htmlEscape(s.channel_name)}"
                       aria-label="Play ${htmlEscape(s.channel_name)}">▶</button>
             </div>`}
      </div>`;
  }
  // ─ Empty slot — pickable from live channels + recent recordings ─
  const liveOpts = (streams || []).map((s) =>
    `<option value="live:${htmlEscape(s.stream_id)}">▶ LIVE · ${htmlEscape(s.channel_name)} · ${htmlEscape(s.platform)}</option>`
  ).join("");
  const recOpts = (recCache || [])
    .filter((r) => r.state === "Finished" && r.file_exists !== false)
    .slice(0, 24)
    .map((r) => `<option value="rec:${htmlEscape(r.id)}">📁 REC · ${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</option>`)
    .join("");
  return `
    <div class="ms-leaf ms-empty" data-path="${htmlEscape(path)}">
      <div class="ms-empty-pill">
        <span>Select stream</span>
        <select class="ms-slot-pick" data-path="${htmlEscape(path)}" aria-label="Pick a stream or recording for this tile">
          <option value="">— pick a live channel or recording —</option>
          ${liveOpts ? `<optgroup label="Live channels">${liveOpts}</optgroup>` : ""}
          ${recOpts ? `<optgroup label="Recent recordings">${recOpts}</optgroup>` : ""}
        </select>
      </div>
      <div class="ms-empty-hint pg-cap-hint">…or drag a channel from the rail.</div>
    </div>`;
}

function toTitleCase(slug) {
  return slug.split(/[-_]/).map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
}

// Single source of truth for the plugin set across the Settings →
// Plugins manager AND the hub. Every shipped plugin appears once with
// the metadata the SPA needs to render its enable toggle + open link.
const PLUGIN_REGISTRY = [
  // First-party Pro plugins with dedicated SPA sub-routes.
  { name: "crunchr",   label: "Crunchr",   category: "Transcription", route: "#/plugins/crunchr",   proGated: true,  description: "Transcribe every recording — speaker timeline, quote search, exportable subtitles." },
  { name: "archiver",  label: "Archiver",  category: "Archive",       route: "#/plugins/archiver",  proGated: true,  description: "Auto-catalog the full back-catalog of any followed channel." },
  { name: "insights",  label: "Insights",  category: "Analytics",     route: "#/plugins/insights",  proGated: true,  description: "Cross-stream analytics, word frequency, topic shifts, retention proxy." },
  { name: "viewguard", label: "Viewguard", category: "Analytics",     route: "#/plugins/viewguard", proGated: true,  description: "Live fraud-signal scoring during captures + cross-stream trend dashboard." },
  // Editor stack.
  { name: "editor",     label: "EDL editor",   category: "Editor", proGated: true, description: "Non-destructive EDL with split / ripple-delete / dead-air trim / branding overlay + revision history." },
  { name: "deadair",    label: "Dead-air trim", category: "Editor", proGated: true, description: "Silence detection + one-click EDL trim from inside the editor." },
  { name: "branding",   label: "Branding",      category: "Editor", proGated: true, description: "Watermark + intro/outro banner overlay spec, applied via ffmpeg filter_complex on render." },
  { name: "broll",      label: "B-roll finder", category: "Editor", proGated: true, description: "Suggest B-roll cuts from a tagged local library based on transcript topics." },
  { name: "loudness",   label: "Loudness",      category: "Editor", proGated: true, description: "EBU R128 master-bus loudness check with per-platform presets (YouTube/Spotify/Apple/EBU/Twitch)." },
  { name: "structure",  label: "Structure",     category: "Editor", proGated: true, description: "DAW-style section labeller — intro / gameplay / break / outro tiling from chapters + chat density + scene cuepoints." },
  { name: "automation", label: "Automation",    category: "Editor", proGated: true, description: "Volume automation curves — time-keyed gain with linear/cosine/step interpolation, baked via ffmpeg asendcmd." },
  { name: "scenes",     label: "Scene snapshots", category: "Editor", proGated: true, description: "DAW-style session save/recall — bundle every per-recording plugin state as a named scene." },
  { name: "schedule-optimizer", label: "Schedule optimizer", category: "Publish", proGated: true, description: "Publish-slot recommender — engagement samples → top weekly publish times with confidence + plateau coverage." },
  { name: "beat-detect",        label: "Beat detection",     category: "Editor", proGated: true, description: "DAW-style tempo grid — onset detector + BPM autocorrelation for music-sync montage cuts." },
  { name: "vad",                label: "Voice gate",         category: "Editor", proGated: true, description: "DAW-style noise gate — hysteresis VAD that surfaces auto-tighten ripple-deletes for podcast/commentary recordings." },
  { name: "sidechain",          label: "Sidechain compressor", category: "Editor", proGated: true, description: "DAW sidechain — VAD voice intervals → ducking automation curve baked via the existing volume-automation render path." },
  { name: "insert-fx",          label: "Insert FX chain",      category: "Editor", proGated: true, description: "DAW-style ordered insert chain per recording: HP, NR, de-esser, comp, limiter, reverb. Voice + game bus presets. Composes into one ffmpeg -af baked at render." },
  { name: "pitch",              label: "Pitch / time-stretch", category: "Editor", proGated: true, description: "Independent tempo + pitch. Fit a 1h45 stream to a 1h slot without changing voices' pitch, or transpose a stinger without changing tempo. Wraps ffmpeg rubberband, formant-preserving by default." },
  // Asset / analytics / publishing.
  { name: "chapters",         label: "Chapters",         category: "Analytics", proGated: true, description: "Heuristic chapter markers extracted from pacing." },
  { name: "cuepoints",        label: "Cuepoints",        category: "Analytics", proGated: true, description: "Scene-change detection from ffmpeg's select filter." },
  { name: "thumbnails",       label: "Thumbnails",       category: "Analytics", proGated: true, description: "Frame ranking + facecam crop candidates." },
  { name: "clipper",          label: "Clipper",          category: "Editor",    proGated: true, description: "Highlight detection + one-click clip extraction." },
  { name: "captions",         label: "Captions",         category: "Transcription", proGated: true, description: "SRT / VTT / TXT export with translator-trait pluggable backend." },
  { name: "multitrack",       label: "Multitrack",       category: "Editor",    proGated: true, description: "Audio track enumeration + extraction." },
  { name: "brandsafe",        label: "Brand safety",     category: "Publish",   proGated: true, description: "Pre-publish content classifier." },
  { name: "reuse",            label: "Reuse",            category: "Publish",   proGated: true, description: "Cross-format publish-queue drafter." },
  { name: "casebook",         label: "Casebook",         category: "Reports",   proGated: true, description: "Post-stream markdown briefing." },
  { name: "heatmap",          label: "Heatmap",          category: "Analytics", proGated: true, description: "Multi-signal retention overlay." },
  { name: "insights-compare", label: "Compare", category: "Analytics", proGated: true, description: "Stream-vs-stream side-by-side." },
  { name: "viewguard-trend",  label: "Viewguard trend",  category: "Analytics", proGated: true, description: "Cross-stream fraud trend dashboard." },
  { name: "chat-density",     label: "Chat density",     category: "Analytics", proGated: true, description: "Audience-retention proxy from chat rate." },
  // Viewer layer.
  { name: "multistream", label: "Multistream viewer", category: "Viewer", route: "#/watch", proGated: true, description: "Auto-tile any subset of currently-live followed channels." },
  { name: "chat",        label: "Chat client",       category: "Viewer", route: "#/chat",  proGated: true, description: "Chatterino-class IRC + tokenizer + filter pipeline + ring buffer." },
  // Cross-cutting.
  { name: "pipelines-dag", label: "Pipelines DAG", category: "Reports", route: "#/pipelines", proGated: true, description: "Cross-plugin pipeline graph." },
  { name: "marketplace",   label: "Marketplace",   category: "Reports", route: "#/plugins",   proGated: true, description: "Third-party plugin catalog stub." },
];

// Per-plugin pitch lines for the upsell card. Keyed by plugin name so the
// CTA copy stays specific instead of generic. Defaults to the plugin's
// description fetched from the marketplace catalog when present.
const PRO_UPSELL_PITCH = {
  crunchr: "Transcribe every recording, jump-to-quote search, speaker timeline, exportable subtitles.",
  archiver: "Auto-catalog the full back-catalog of any followed channel, dedup VODs, search by title or game.",
  insights: "Cross-stream analytics: word frequency, topic shifts, retention proxy, side-by-side compares.",
  viewguard: "Live fraud-signal scoring during captures; cross-stream trend dashboard.",
  editor: "Non-destructive EDL editor with split / ripple-delete / dead-air trim / branding overlay + revision history.",
  chapters: "Heuristic chapter markers extracted from your stream's pacing.",
  clipper: "Highlight detection + one-click clip extraction from the timeline.",
  captions: "Export SRT / VTT / TXT with a translator-trait pluggable backend.",
};

function renderProUpsell(plugin, licence) {
  const pitch = PRO_UPSELL_PITCH[plugin] || "Unlock this plugin's analytics, automation, and editor features.";
  const trial = licence && licence.trial;
  const hasTrialUsed = trial && trial.used;
  // `implemented` is false only when the server explicitly says no licence
  // backend is configured (STRIVO_LICENCE_URL unset) — self-hosted default.
  const implemented = !licence || licence.implemented !== false;
  const notConfiguredAttrs = ' disabled aria-disabled="true" title="No licence service configured for this install"';
  const trialNote = !implemented
    ? "No licence service configured for this install."
    : hasTrialUsed
    ? "Your 3-day trial has already been used on this machine."
    : "Start a free 3-day trial — no card needed.";
  const trialBtn = !implemented
    ? `<button class="btn-primary"${notConfiguredAttrs}>▶ Start 3-day trial</button>`
    : hasTrialUsed
    ? `<button class="btn-primary" disabled title="trial already used">Trial used</button>`
    : `<button class="btn-primary pg-upsell-trial">▶ Start 3-day trial</button>`;
  const activateBtn = !implemented
    ? `<button class="sm"${notConfiguredAttrs}>Activate</button>`
    : `<button class="sm pg-upsell-activate">Activate</button>`;
  return `
    <div class="pg-upsell-card">
      <div class="pg-upsell-icon">★</div>
      <div class="pg-upsell-body">
        <h2 class="pg-upsell-title">${htmlEscape(toTitleCase(plugin))} is a Strivo Pro plugin</h2>
        <p class="pg-upsell-pitch">${htmlEscape(pitch)}</p>
        <p class="pg-upsell-trial-note pg-cap-hint">${htmlEscape(trialNote)}</p>
        <div class="pg-upsell-actions">
          ${trialBtn}
          <span class="pg-upsell-sep">or</span>
          <input type="text" class="pg-upsell-key" placeholder="paste licence key…" aria-label="licence key"${implemented ? "" : " disabled"}/>
          ${activateBtn}
        </div>
        <p class="pg-upsell-foot pg-cap-hint">
          Already a subscriber? Find your key in your Strivo account.
        </p>
      </div>
    </div>`;
}

function wireProUpsell(host, plugin) {
  host.querySelector(".pg-upsell-trial")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Starting…", async () => {
      try {
        await API.licenceTrial();
        Toast.success(`Trial active — ${toTitleCase(plugin)} unlocked. Refreshing…`);
        setTimeout(() => location.reload(), 800);
      } catch (err) {
        Toast.error(`Trial failed: ${err.message}`);
      }
    });
  });
  host.querySelector(".pg-upsell-activate")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const key = host.querySelector(".pg-upsell-key").value.trim();
    if (!key) { Toast.error("Paste a licence key first."); return; }
    await withBusy(btn, "Activating…", async () => {
      try {
        await API.licenceActivate(key);
        Toast.success(`Activated — ${toTitleCase(plugin)} unlocked. Refreshing…`);
        setTimeout(() => location.reload(), 800);
      } catch (err) {
        Toast.error(`Activate failed: ${err.message}`);
      }
    });
  });
}

// ── Pro App (Studio / Analytics / Publish unified panes) ─────────────
//
// Plugin entries are no longer discrete topnav slots. Instead they
// contribute to one of three Pro panes. Each pane is a single page
// with a tab strip across the top; switching tabs swaps the body.
// Plugins not yet rendered in-pane open via legacy /plugins/<slug>
// — those slugs still work as deep-links.

const PRO_PANES = {
  studio: {
    title: "Studio",
    subtitle: "The editor canvas. Volume automation, branding, captions, loudness, insert-fx, sidechain, pitch, beat-grid, voice gate, and dead-air all live INSIDE the EDL editor and aren't separate tabs anywhere else.",
    tabs: [
      { slug: "editor", label: "EDL editor", route: "#/recordings", description: "The canvas. Open any finished recording → ⓘ Info → ✄ EDL editor. The 14-button toolbar inside (Split · Ripple-delete · Trim dead air · Voice gate · 🦆 Sidechain duck · 🎛 Insert FX · 🎚 Pitch/time · ★ Branding · ♪ Loudness · 🎼 Beat grid · ↺ History · 🎬 Scenes · ♪ I/TP/LRA gauge · ⚡ Render) is where the work happens." },
      { slug: "scenes", label: "Scenes", route: null, description: "Ableton-style session save/recall. Bundles every plugin's per-recording state (EDL + branding + automation + loudness + captions style) into a named snapshot. Open from inside the EDL editor's 🎬 Scenes panel." },
      { slug: "ab", label: "A/B render compare", route: null, description: "Snapshot the render-relevant settings (insert-fx, pitch/time, loudness target, sidechain duck) into two variants and diff before committing. Pure data model — invoked per recording." },
      { slug: "submix", label: "Sub-mix bus", route: null, description: "Per-track InsertChain + master InsertChain routed via ffmpeg filter_complex. Composes multiple audio sources into the master at render." },
    ],
  },
  analytics: {
    title: "Analytics",
    subtitle: "Every analytical lens — viewer-side fraud, audience retention, cross-stream comparison, density.",
    tabs: [
      { slug: "insights",       label: "Insights",         route: "#/plugins/insights",        description: "Cross-stream analytics, word frequency, topic shifts, retention proxy." },
      { slug: "viewguard",      label: "Viewguard",        route: "#/plugins/viewguard",       description: "Live fraud-signal scoring during captures + cross-stream trend dashboard." },
      { slug: "chat-density",   label: "Chat density",     route: null,                        description: "Audience-retention proxy derived from chat rate over the broadcast." },
      { slug: "heatmap",        label: "Heatmap",          route: null,                        description: "Multi-signal retention overlay — talk / action / highlight / brand-safe." },
      { slug: "structure",      label: "Structure",        route: null,                        description: "DAW-style section labeller — intro / gameplay / break / outro tiling." },
      { slug: "dataviz",        label: "Data viz",         route: "#/dataviz",                 description: "Pick recordings → run experiments → chart the result. Open the dedicated page." },
    ],
  },
  publish: {
    title: "Publish",
    subtitle: "Get the cut out the door — clips, chapters, thumbnails, schedule, B-roll, publish queue.",
    tabs: [
      { slug: "schedule-optimizer", label: "Schedule optimizer", route: "#/plugins/schedule-optimizer", description: "Publish-slot recommender — 7×24 heatmap → top weekly times with confidence + plateau coverage." },
      { slug: "clipper",            label: "Clipper",            route: null,                          description: "Highlight detection + clip extraction." },
      { slug: "thumbnails",         label: "Thumbnails",         route: null,                          description: "Frame ranking + facecam crop." },
      { slug: "chapters",           label: "Chapters",           route: null,                          description: "Heuristic chapter generation from pacing." },
      { slug: "casebook",           label: "Casebook",           route: null,                          description: "Post-stream markdown briefing." },
      { slug: "reuse",              label: "Reuse",              route: null,                          description: "Cross-format publish-queue drafter." },
      { slug: "broll",              label: "B-roll finder",      route: null,                          description: "Suggest B-roll cuts from a tagged local library based on transcript topics." },
      { slug: "brandsafe",          label: "Brand safety",       route: null,                          description: "Pre-publish content classifier." },
      { slug: "multitrack",         label: "Multitrack",         route: null,                          description: "Audio track enumeration + extraction." },
      { slug: "cuepoints",          label: "Cue points",         route: null,                          description: "Scene-change detection via ffmpeg select." },
    ],
  },
};

async function renderProApp(paneKey) {
  const pane = PRO_PANES[paneKey];
  if (!pane) { route("library"); return; }

  // Pick the active tab from the hash sub-route (e.g. #/studio/loudness).
  const parts = routeParts();
  const tabSlug = parts[1] || pane.tabs[0]?.slug || "";
  const activeTab = pane.tabs.find((t) => t.slug === tabSlug) || pane.tabs[0];

  const tabStrip = pane.tabs.map((t) => `
    <a class="pro-tab ${t.slug === activeTab.slug ? "is-active" : ""}" href="#/${paneKey}/${t.slug}">${htmlEscape(t.label)}</a>`).join("");

  const inlinePane = paneKey === "studio" && (activeTab.slug === "ab" || activeTab.slug === "submix");

  const body = activeTab.route
    ? `<div class="pro-tab-body">
         <p class="pg-cap-hint">${htmlEscape(activeTab.description)}</p>
         <p><a class="btn-primary sm" href="${htmlEscape(activeTab.route)}">Open ${htmlEscape(activeTab.label)} →</a></p>
       </div>`
    : inlinePane
    ? `<div class="pro-tab-body">
         <p class="pg-cap-hint">${htmlEscape(activeTab.description)}</p>
         <div id="pro-inline-host"></div>
       </div>`
    : `<div class="pro-tab-body">
         <p class="pg-cap-hint">${htmlEscape(activeTab.description)}</p>
         <p class="empty sm">This tool is reached from inside the Editor view (open a recording → ⓘ Info → ✄ EDL editor) or via its per-recording API. The unified pane is the conceptual home; the controls live where the artefact does.</p>
         <details class="pro-tab-detail">
           <summary>API reference</summary>
           <pre>POST /api/v1/plugins/${htmlEscape(activeTab.slug)}/&lt;recording_id&gt;</pre>
         </details>
       </div>`;

  root.innerHTML = chrome(`
    <h1 class="page-title">${htmlEscape(pane.title)}</h1>
    <p class="page-subtitle">${htmlEscape(pane.subtitle)}</p>
    <nav class="pro-tabs" role="tablist">${tabStrip}</nav>
    <section class="cfg-card pro-pane-card">${body}</section>
  `);
  setupChromeHandlers();
  if (inlinePane) {
    const host = document.getElementById("pro-inline-host");
    if (activeTab.slug === "ab") await mountAbRenderPane(host);
    else await mountSubmixPane(host);
  }
}

// ── A/B render compare pane (#/studio/ab) ─────────────────────────────
//
// Per-recording: stash a render-settings snapshot into slot A and slot
// B, diff them, then run the real ffmpeg SSIM compare once both are
// saved. Insert-fx chains stay editable from their own panel (Editor →
// 🎛 Insert FX) — this pane only edits the settings ab-render actually
// models directly (label, loudness target, duck depth, tempo).

const abrState = { recordingId: "", data: null, comparing: false, compareResult: null, compareError: null };

function abrVariantForm(slot, variant) {
  const v = variant || {};
  return `
    <div class="cfg-card abr-slot" data-slot="${slot}">
      <h3 class="cfg-title">Slot ${slot.toUpperCase()}</h3>
      <label>Label
        <input type="text" class="abr-f-label" value="${htmlEscape(v.label || "")}" placeholder="e.g. loud-master"/>
      </label>
      <label>Loudness target (LUFS, blank = leave alone)
        <input type="number" step="0.1" class="abr-f-lufs" value="${v.loudness_target_lufs ?? ""}"/>
      </label>
      <label>Sidechain duck depth (dB, blank = no duck)
        <input type="number" step="0.5" class="abr-f-duck" value="${v.duck_db ?? ""}"/>
      </label>
      <label>Tempo (× speed, 1.0 = identity)
        <input type="number" step="0.01" min="0.25" max="4" class="abr-f-tempo" value="${v.pitch_time?.tempo ?? 1.0}"/>
      </label>
      <p class="pg-cap-hint">Insert-fx chain is edited from the Editor's 🎛 Insert FX panel and carries over automatically once wired to a recording's chain; this form covers the fields ab-render models directly.</p>
      <button class="btn-primary sm abr-save" type="button">Save slot ${slot.toUpperCase()}</button>
      ${v.label !== undefined ? `<pre class="abr-filter">${htmlEscape(v ? (v.audio_filter || "") : "")}</pre>` : ""}
    </div>`;
}

function paintAbRenderPane(host) {
  const d = abrState.data || {};
  const diffRows = (d.diff || [])
    .map((e) => `<tr><td>${htmlEscape(e.field)}</td><td>${htmlEscape(e.a)}</td><td>${htmlEscape(e.b)}</td></tr>`)
    .join("");
  const diffTable = d.a && d.b
    ? `<table class="abr-diff"><thead><tr><th>field</th><th>A</th><th>B</th></tr></thead><tbody>${
        diffRows || `<tr><td colspan="3" class="pg-cap-hint">No differences.</td></tr>`
      }</tbody></table>`
    : `<p class="empty sm">Save both slots to see a diff.</p>`;
  const canCompare = !!(d.a && d.b);
  let compareBlock = "";
  if (abrState.comparing) {
    compareBlock = `<div class="empty sm">Rendering A + B and running ffmpeg SSIM…</div>`;
  } else if (abrState.compareError) {
    compareBlock = `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(abrState.compareError)}</div>`;
  } else if (abrState.compareResult) {
    const q = abrState.compareResult.quality || {};
    compareBlock = `
      <div class="abr-quality">
        <span class="pg-stat"><strong>${q.vmaf_mean != null ? q.vmaf_mean.toFixed(2) : "—"}</strong> VMAF mean</span>
        <span class="pg-stat"><strong>${q.ssim_all != null ? q.ssim_all.toFixed(4) : "—"}</strong> SSIM all</span>
      </div>
      <p class="pg-cap-hint">A: ${htmlEscape(abrState.compareResult.a_output_path || "")}<br/>B: ${htmlEscape(abrState.compareResult.b_output_path || "")}</p>`;
  }
  host.innerHTML = `
    <div class="abr-picker">
      <label>Recording
        <select id="abr-rec"><option value="">— select a recording —</option>${abrState.recOptions || ""}</select>
      </label>
    </div>
    ${abrState.recordingId ? `
    <div class="abr-slots">
      ${abrVariantForm("a", d.a ? { ...d.a, audio_filter: d.a_audio_filter } : null)}
      ${abrVariantForm("b", d.b ? { ...d.b, audio_filter: d.b_audio_filter } : null)}
    </div>
    <div class="cfg-card abr-compare-card">
      <h3 class="cfg-title">Diff</h3>
      ${diffTable}
      <button class="btn-primary sm" id="abr-compare" type="button" ${canCompare ? "" : "disabled"}>▶ Render + compare (VMAF/SSIM)</button>
      ${compareBlock}
    </div>` : `<p class="empty sm">Pick a recording to load or start an A/B compare.</p>`}
  `;
  host.querySelectorAll(".abr-save").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const card = btn.closest(".abr-slot");
      const slot = card.dataset.slot;
      const num = (sel) => {
        const raw = card.querySelector(sel).value.trim();
        return raw === "" ? null : Number(raw);
      };
      const tempo = num(".abr-f-tempo");
      const variant = {
        label: card.querySelector(".abr-f-label").value.trim(),
        insert_fx: null,
        pitch_time: tempo != null && tempo !== 1.0 ? { tempo, pitch: 1.0, formant_preserve: true } : null,
        loudness_target_lufs: num(".abr-f-lufs"),
        duck_db: num(".abr-f-duck"),
        stashed_at: new Date().toISOString(),
      };
      await withBusy(btn, "Saving…", async () => {
        try {
          abrState.data = await API.abRenderSave(abrState.recordingId, slot, variant);
          abrState.compareResult = null;
          abrState.compareError = null;
          Toast.success(`Slot ${slot.toUpperCase()} saved.`);
          paintAbRenderPane(host);
        } catch (err) {
          Toast.error(`Save failed: ${err.message}`);
        }
      });
    });
  });
  host.querySelector("#abr-compare")?.addEventListener("click", async () => {
    abrState.comparing = true;
    abrState.compareError = null;
    paintAbRenderPane(host);
    try {
      abrState.compareResult = await API.abRenderCompare(abrState.recordingId);
    } catch (err) {
      abrState.compareError = err.message;
    } finally {
      abrState.comparing = false;
      paintAbRenderPane(host);
    }
  });
}

async function mountAbRenderPane(host) {
  const recs = (await API.recordings().catch(() => ({ recordings: [] }))).recordings || [];
  const finished = recs.filter((r) => r.state === "Finished");
  abrState.recOptions = finished
    .map((r) => `<option value="${htmlEscape(r.id)}" ${r.id === abrState.recordingId ? "selected" : ""}>${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</option>`)
    .join("");
  paintAbRenderPane(host);
  host.querySelector("#abr-rec").addEventListener("change", async (e) => {
    abrState.recordingId = e.target.value;
    abrState.data = null;
    abrState.compareResult = null;
    abrState.compareError = null;
    if (!abrState.recordingId) { paintAbRenderPane(host); return; }
    try {
      abrState.data = await API.abRenderLoad(abrState.recordingId);
    } catch (err) {
      Toast.error(`Load failed: ${err.message}`);
    }
    paintAbRenderPane(host);
  });
}

// ── Sub-mix bus pane (#/studio/submix) ────────────────────────────────
//
// Per-recording bus routing: N input tracks (label + input index + gain)
// summed into a master bus, composed into one ffmpeg filter_complex via
// strivo-submix. Master/per-track insert-fx chains stay null here (same
// reasoning as the A/B pane) — the composer already handles them when a
// chain is attached via the Insert FX panel's stored state.

const smxState = { recordingId: "", mix: { tracks: [], master_chain: null, master_gain_db: 0 }, filterComplex: "" };

function paintSubmixPane(host) {
  const trackRows = smxState.mix.tracks
    .map((t, i) => `
      <div class="cfg-card smx-track" data-idx="${i}">
        <label>Label <input type="text" class="smx-t-label" value="${htmlEscape(t.label || "")}"/></label>
        <label>Input index <input type="number" min="0" class="smx-t-input" value="${t.input_index ?? 0}"/></label>
        <label>Gain (dB) <input type="number" step="0.5" class="smx-t-gain" value="${t.gain_db ?? 0}"/></label>
        <button class="sm smx-t-remove" type="button">✕ Remove</button>
      </div>`)
    .join("");
  host.innerHTML = `
    <div class="abr-picker">
      <label>Recording
        <select id="smx-rec"><option value="">— select a recording —</option>${smxState.recOptions || ""}</select>
      </label>
    </div>
    ${smxState.recordingId ? `
    <div class="cfg-card smx-tracks-card">
      <h3 class="cfg-title">Tracks</h3>
      <div class="smx-tracks">${trackRows || '<div class="empty sm">No tracks yet.</div>'}</div>
      <button class="sm" id="smx-add-track" type="button">+ Add track</button>
    </div>
    <div class="cfg-card">
      <h3 class="cfg-title">Master</h3>
      <label>Master gain (dB)
        <input type="number" step="0.5" id="smx-master-gain" value="${smxState.mix.master_gain_db ?? 0}"/>
      </label>
      <button class="btn-primary sm" id="smx-save" type="button">Save sub-mix</button>
    </div>
    <div class="cfg-card">
      <h3 class="cfg-title">Composed filter_complex</h3>
      <pre class="abr-filter">${htmlEscape(smxState.filterComplex || "(empty — add at least one track)")}</pre>
    </div>` : `<p class="empty sm">Pick a recording to load or build a sub-mix.</p>`}
  `;
  host.querySelector("#smx-add-track")?.addEventListener("click", () => {
    smxState.mix.tracks.push({ label: `track${smxState.mix.tracks.length}`, input_index: smxState.mix.tracks.length, insert_fx: null, gain_db: 0 });
    paintSubmixPane(host);
  });
  host.querySelectorAll(".smx-t-remove").forEach((btn) => {
    btn.addEventListener("click", () => {
      const idx = Number(btn.closest(".smx-track").dataset.idx);
      smxState.mix.tracks.splice(idx, 1);
      paintSubmixPane(host);
    });
  });
  host.querySelector("#smx-save")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    host.querySelectorAll(".smx-track").forEach((row) => {
      const idx = Number(row.dataset.idx);
      smxState.mix.tracks[idx].label = row.querySelector(".smx-t-label").value.trim();
      smxState.mix.tracks[idx].input_index = Number(row.querySelector(".smx-t-input").value) || 0;
      smxState.mix.tracks[idx].gain_db = Number(row.querySelector(".smx-t-gain").value) || 0;
    });
    smxState.mix.master_gain_db = Number(document.getElementById("smx-master-gain").value) || 0;
    await withBusy(btn, "Saving…", async () => {
      try {
        const resp = await API.submixSave(smxState.recordingId, smxState.mix);
        smxState.mix = resp.submix;
        smxState.filterComplex = resp.filter_complex;
        Toast.success("Sub-mix saved.");
        paintSubmixPane(host);
      } catch (err) {
        Toast.error(`Save failed: ${err.message}`);
      }
    });
  });
}

async function mountSubmixPane(host) {
  const recs = (await API.recordings().catch(() => ({ recordings: [] }))).recordings || [];
  const finished = recs.filter((r) => r.state === "Finished");
  smxState.recOptions = finished
    .map((r) => `<option value="${htmlEscape(r.id)}" ${r.id === smxState.recordingId ? "selected" : ""}>${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</option>`)
    .join("");
  paintSubmixPane(host);
  host.querySelector("#smx-rec").addEventListener("change", async (e) => {
    smxState.recordingId = e.target.value;
    smxState.mix = { tracks: [], master_chain: null, master_gain_db: 0 };
    smxState.filterComplex = "";
    if (!smxState.recordingId) { paintSubmixPane(host); return; }
    try {
      const resp = await API.submixLoad(smxState.recordingId);
      smxState.mix = resp.submix;
      smxState.filterComplex = resp.filter_complex;
    } catch (err) {
      Toast.error(`Load failed: ${err.message}`);
    }
    paintSubmixPane(host);
  });
}

// Map deprecated plugin sub-routes to their new home in the Pro app
// panes. The discrete /plugins/<slug> pages for tools that live
// inside the EDL editor or under a Pro pane are redirected so old
// deep-links keep working.
const PLUGIN_ROUTE_REDIRECTS = {
  // Studio plugins (live inside the EDL editor / Studio pane).
  automation: "#/studio/editor", branding: "#/studio/editor", captions: "#/studio/editor",
  loudness: "#/studio/editor", "insert-fx": "#/studio/editor", sidechain: "#/studio/editor",
  pitch: "#/studio/editor", "beat-detect": "#/studio/editor", vad: "#/studio/editor",
  deadair: "#/studio/editor", scenes: "#/studio/scenes",
  "ab-render": "#/studio/ab", submix: "#/studio/submix",
  // Analytics plugins (Analytics pane).
  "chat-density": "#/analytics/chat-density", heatmap: "#/analytics/heatmap",
  structure: "#/analytics/structure", "viewguard-trend": "#/analytics/viewguard",
  "insights-compare": "#/analytics/insights",
  // Publish plugins (Publish pane).
  clipper: "#/publish/clipper", thumbnails: "#/publish/thumbnails",
  chapters: "#/publish/chapters", casebook: "#/publish/casebook",
  reuse: "#/publish/reuse", broll: "#/publish/broll",
  brandsafe: "#/publish/brandsafe", multitrack: "#/publish/multitrack",
  cuepoints: "#/publish/cuepoints",
};

async function renderPlugins() {
  const parts = routeParts(); // ["plugins", <slug?>, …]
  const slug = parts[1];
  // Redirect deprecated discrete-plugin routes to the unified Pro pane
  // they now contribute to. The five real standalone pages stay.
  if (slug && PLUGIN_ROUTE_REDIRECTS[slug]) {
    window.location.hash = PLUGIN_ROUTE_REDIRECTS[slug];
    return;
  }
  try {
    switch (slug) {
      case "crunchr":
        if (parts[2] === "rec" && parts[3]) return await renderCrunchrRecording(parts[3]);
        return await renderCrunchr();
      case "archiver":
        if (parts[2]) return await renderArchiverVideos(parts[2]);
        return await renderArchiver();
      case "viewguard":
        return await renderViewguard();
      case "insights":
        return await renderInsights();
      case "schedule-optimizer":
        return await renderScheduleOptimizer();
      default:
        return await renderPluginHub();
    }
  } catch (e) {
    if (e.message && e.message.includes("unauthorized")) return;
    root.removeAttribute("aria-busy");
    if (e.code === 402) {
      const plugin = e.plugin || slug || "this plugin";
      root.innerHTML = chrome(
        `${pluginHeader(toTitleCase(plugin), "Strivo Pro")}<div id="pg-upsell-host"></div>`,
      );
      setupChromeHandlers();
      const host = document.getElementById("pg-upsell-host");
      const licence = await API.licenceStatus().catch(() => null);
      host.innerHTML = renderProUpsell(plugin, licence);
      wireProUpsell(host, plugin);
      return;
    }
    root.innerHTML = chrome(
      `${pluginHeader("Plugins", "")}<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`,
    );
    setupChromeHandlers();
  }
}

// Shared page header with an optional "← back to Plugins" trail.
function pluginHeader(title, subtitle, backHref) {
  const back = backHref
    ? `<a class="pg-back" href="${backHref}">← back</a>`
    : "";
  return `
    ${back}
    <h1 class="page-title">${htmlEscape(title)}</h1>
    ${subtitle ? `<p class="page-subtitle">${subtitle}</p>` : ""}
  `;
}

async function renderPluginHub() {
  // Fetch licence + plugins in parallel; licence failure must not block
  // the hub render — it just means we hide the upgrade card this paint.
  const [resp, licence] = await Promise.all([
    API.plugins(),
    API.licenceStatus().catch(() => null),
  ]);
  root.removeAttribute("aria-busy");
  const plugins = (resp && resp.plugins) || [];
  const upgrade = renderUpgradeCard(licence);
  // Plugin first-action hints. Keyed by plugin name; shown when the
  // plugin reports zero data (audit U12). Long-form copy lives here
  // so non-engineers can iterate on it without touching render code.
  const PLUGIN_GETSTARTED = {
    crunchr: "Open a recording's ⓘ Info → Generate subtitles to transcribe it.",
    archiver: "Enable Archiver tandem on a channel from the channel row to start backfilling.",
    insights: "Insights aggregate Crunchr output — transcribe at least one recording.",
    viewguard: "Viewguard scores Twitch viewer signals during live captures.",
  };
  const cards = plugins
    .map((p) => {
      const stats = p.stats || {};
      const totalStats = Object.values(stats).reduce((a, b) => a + (Number(b) || 0), 0);
      const statBits = Object.entries(stats)
        .map(
          ([k, v]) =>
            `<span class="pg-stat"><strong>${formatCount(v)}</strong> ${htmlEscape(k.replace(/_/g, " "))}</span>`,
        )
        .join("");
      // Locked Pro plugins never reach the SPA — the server filters
      // them out of /api/v1/plugins when the gate denies. So this
      // only sees entitled or free plugins.
      const status = p.available
        ? `<span class="cfg-badge ok">ready</span>`
        : `<span class="cfg-badge">idle</span>`;
      const href = p.available ? `#/plugins/${p.name}` : null;
      const cardHref = href || p.route || `#/plugins/${encodeURIComponent(p.name)}`;
      // Get-started guidance fills the stats footprint while there's
      // nothing to count yet — replaces the bland "no data yet" stub.
      const statsHtml = statBits
        ? `<div class="pg-stats">${statBits}</div>`
        : totalStats === 0 && PLUGIN_GETSTARTED[p.name]
          ? `<div class="pg-getstarted"><strong>Get started:</strong> ${htmlEscape(PLUGIN_GETSTARTED[p.name])}</div>`
          : '<div class="pg-stats"><span class="pg-stat muted">no data yet</span></div>';
      const verbs = Array.isArray(p.verbs) && p.verbs.length
        ? `<div class="pg-verbs">${p.verbs
            .map(
              (v) =>
                `<span class="pg-verb-chip" title="${htmlEscape(v.scope ? `Scope: ${v.scope}` : "")}">${htmlEscape(v.label || v.verb)}</span>`,
            )
            .join("")}</div>`
        : "";
      const dataDir = p.data_dir
        ? `<div class="pg-meta"><code title="Plugin data folder">${htmlEscape(p.data_dir)}</code></div>`
        : "";
      const body = `
        <div class="pg-card-head">
          <span class="pg-icon pg-icon-${p.name}" aria-hidden="true">${htmlEscape((p.display || p.name)[0])}</span>
          <a class="pg-card-name" href="${cardHref}">${htmlEscape(p.display || p.name)}</a>
          ${status}
          <a class="pg-card-gear" href="#/settings/plugins"
             title="Open plugin manager"
             onclick="event.stopPropagation()">⚙</a>
        </div>
        <p class="pg-card-desc">${htmlEscape(p.description || "")}</p>
        ${statsHtml}
        ${verbs}
        ${dataDir}`;
      // Idle/locked cards still need to be reachable so users can read
      // the upsell. Route to the plugin's hash anyway; the renderer
      // shows the Pro upsell card for gated routes.
      return href
        ? `<article class="pg-card" data-plugin="${p.name}" data-href="${cardHref}" tabindex="0">${body}</article>`
        : `<article class="pg-card pg-card-idle" data-plugin="${p.name}" data-href="${cardHref}" tabindex="0" title="Open the upsell — this plugin is part of StriVo Pro">${body}<span class="pg-card-lock" aria-hidden="true">🔒</span></article>`;
    })
    .join("");
  // Capability matrix + marketplace both render lazily so the plugin
  // grid paints first.
  API.pluginCapabilities().then(renderCapabilityMatrix).catch(() => {});
  API.marketplaceCatalog().then(renderMarketplaceSection).catch(() => {});
  root.innerHTML = chrome(`
    ${pluginHeader("Plugins", "First-party plugins. Pick one to browse what it has produced.")}
    ${upgrade}
    <div id="pg-capability-matrix"></div>
    <div id="pg-marketplace"></div>
    <div class="pg-grid">${
      cards ||
      (upgrade
        ? '<div class="empty">Activate Strivo Pro above to populate this grid.</div>'
        : '<div class="empty">No plugins loaded.</div>')
    }</div>
  `);
  setupChromeHandlers();
  document.querySelectorAll(".pg-card[data-href]").forEach((card) => {
    const open = () => { location.hash = card.dataset.href.slice(1); };
    card.addEventListener("click", (event) => {
      if (!event.target.closest("a, button, input, select")) open();
    });
    card.addEventListener("keydown", (event) => {
      if ((event.key === "Enter" || event.key === " ") && event.target === card) {
        event.preventDefault();
        open();
      }
    });
  });
  wireUpgradeCard();
}

// Render the DAW-vision capability matrix into #pg-capability-matrix.
// Built lazily so the plugin grid paints first. Groups roadmap vs.
// available providers so the user can see the trajectory at a glance.
function renderCapabilityMatrix(matrix) {
  const host = document.getElementById("pg-capability-matrix");
  if (!host || !Array.isArray(matrix)) return;
  const rows = matrix
    .map((row) => {
      const chips = (row.providers || [])
        .map(
          (p) =>
            // Two visible spans so CSS can give the state badge a pill of
            // its own — without the explicit element, `name+status` ran
            // together visually ("crunchravailable" / "chaptersroadmap").
            `<a class="pg-cap-chip pg-cap-${htmlEscape(p.status)}" href="#/plugins/${htmlEscape(p.plugin)}" title="${htmlEscape(p.plugin)} · ${htmlEscape(p.status)}">
              <span class="pg-cap-name">${htmlEscape(p.plugin)}</span>
              <span class="pg-cap-state pg-cap-state-${htmlEscape(p.status)}">${htmlEscape(p.status)}</span>
            </a>`,
        )
        .join("");
      const label = row.capability.replace(/_/g, " ");
      // audience_retention is the canonical bridge from the analytics
      // bucket world (heatmap, chat-density) into the publish-time
      // recommender. Surface the bridge directly on the row.
      const isRetentionRow = row.capability === "audience_retention";
      const bridgeLink = isRetentionRow
        ? ` <a class="pg-cap-bridge" href="#/plugins/schedule-optimizer" title="Open the schedule-optimizer page so you can feed it any recording's retention buckets via Crunchr → Heatmap → ↘ Send to schedule optimizer.">▶ optimize publish slot</a>`
        : "";
      return `<div class="pg-cap-row">
        <span class="pg-cap-label">${htmlEscape(label)}${bridgeLink}</span>
        <span class="pg-cap-providers">${chips}</span>
      </div>`;
    })
    .join("");
  host.innerHTML = `
    <details class="pg-cap-matrix" open>
      <summary><strong>Capability matrix</strong> <span class="pg-cap-hint">— what each plugin contributes toward the DAW-for-streaming vision</span></summary>
      <div class="pg-cap-grid">${rows}</div>
    </details>`;
}

// Render the marketplace catalog into #pg-marketplace. Renders each
// plugin as a card with status badge (installed / available / coming
// soon), price chip, capability tags, and a primary action (Install
// when entry_point is real, "Watchlist" when roadmap).
function renderMarketplaceSection(payload) {
  const host = document.getElementById("pg-marketplace");
  if (!host || !payload || !payload.catalog || !payload.catalog.entries) return;
  const entries = payload.catalog.entries;
  const sourceColour = {
    first_party: "hsl(280, 60%, 65%)",
    verified: "hsl(140, 60%, 60%)",
    community: "hsl(35, 70%, 60%)",
  };
  const fmtPrice = (cents) => {
    if (cents == null) return '<span class="mk-free">free</span>';
    return `<span class="mk-price">$${(cents / 100).toFixed(2)}</span>`;
  };
  const entryStatus = (ep) => {
    const kind = (ep && ep.kind) || "roadmap";
    if (kind === "roadmap") return { label: "Coming soon", action: "Watchlist" };
    return { label: "Available", action: "Install" };
  };
  const cards = entries
    .map((e) => {
      const m = e.manifest;
      const sColour = sourceColour[e.source] || sourceColour.community;
      const status = entryStatus(m.entry_point);
      const caps = (m.capabilities || [])
        .slice(0, 6)
        .map((c) => `<span class="pl-cap pl-cap-produces" title="provides">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      const consumes = (m.consumes || [])
        .slice(0, 4)
        .map((c) => `<span class="pl-cap pl-cap-consumes" title="needs">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      return `<div class="mk-card" style="--mk-c:${sColour}">
        <div class="mk-card-head">
          <span class="mk-card-name">${htmlEscape(m.name)}</span>
          <span class="mk-source">${htmlEscape(e.source)}</span>
        </div>
        <div class="mk-card-meta">
          <span class="mk-version">v${htmlEscape(m.version)}</span>
          <span class="mk-author">${htmlEscape(m.author)}</span>
          ${fmtPrice(m.price_cents)}
        </div>
        <p class="mk-desc">${htmlEscape(m.description)}</p>
        <div class="mk-caps">${caps}${consumes}</div>
        <div class="mk-card-foot">
          <span class="mk-status">${htmlEscape(status.label)}</span>
          ${m.repository ? `<a class="pg-linkbtn" href="${htmlEscape(m.repository)}" target="_blank" rel="noopener">repository →</a>` : ""}
          <button class="sm" type="button" disabled title="Install endpoint lands in a follow-up">${htmlEscape(status.action)}</button>
        </div>
      </div>`;
    })
    .join("");
  host.innerHTML = `
    <details class="pg-cap-matrix mk-section" open>
      <summary><strong>Marketplace</strong> <span class="pg-cap-hint">third-party plugins · host v${htmlEscape(payload.host_version)}</span></summary>
      <div class="mk-grid">${cards}</div>
    </details>`;
}

// Upgrade card — shown on the Plugins hub when the user is not entitled.
// Activation and trial endpoints report actionable backend errors when
// the deployment has not configured the external licence service.
function renderUpgradeCard(licence) {
  if (!licence || licence.entitled) return ""; // dev unlock + future paid users
  // `implemented` is false only when the server explicitly says no licence
  // backend is configured (STRIVO_LICENCE_URL unset) — self-hosted default.
  const implemented = licence.implemented !== false;
  const notConfiguredAttrs = implemented
    ? ""
    : ' disabled aria-disabled="true" title="No licence service configured for this install"';
  return `
    <section class="upgrade-card" data-tier="${htmlEscape(licence.tier || "free")}">
      <img class="upgrade-logo" src="/assets/img/strivo-mark.svg" alt="StriVo" />
      <div class="upgrade-body">
        <h2 class="upgrade-title">Strivo Pro</h2>
        <p class="upgrade-tagline">Unlock every plugin — Crunchr, Archiver, Viewguard, Insights — and everything we ship next.</p>
        <ul class="upgrade-bullets">
          <li>One-time <strong>$25</strong> — no subscription, no recurring fees.</li>
          <li>Single-machine licence with auto-refresh every 72h (works offline).</li>
          <li>3-day free trial — no card required.</li>
        </ul>
        <div class="upgrade-actions">
          <button class="upgrade-trial btn-primary"${notConfiguredAttrs}>Start 3-day trial</button>
          <button class="upgrade-activate btn-ghost"${notConfiguredAttrs}>I have a key</button>
        </div>
        ${implemented ? "" : `<p class="upgrade-hint">No licence service configured for this install.</p>`}
      </div>
    </section>
  `;
}

function wireUpgradeCard() {
  const trial = document.querySelector(".upgrade-trial");
  const activate = document.querySelector(".upgrade-activate");
  if (trial) {
    trial.addEventListener("click", async () => {
      try {
        await API.licenceTrial();
        location.reload();
      } catch (e) {
        Toast.error(e.message || "Trial unavailable");
      }
    });
  }
  if (activate) {
    activate.addEventListener("click", async () => {
      const key = prompt("Paste your Strivo Pro licence key:");
      if (!key) return;
      try {
        await API.licenceActivate(key.trim());
        location.reload();
      } catch (e) {
        Toast.error(e.message || "Activation failed");
      }
    });
  }
}

// ── Schedule optimizer ────────────────────────────────────────────────
// 7×24 heatmap + top-slot recommender driven by the iter-44 backend.
// The iter ships with a synthetic dataset baked in so users can see the
// renderer work without first plumbing chat-density / Insights output;
// the textarea lets them paste real samples too.
const DAYS_OF_WEEK = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const SAMPLE_DATASET = [
  // Friday afternoon plateau.
  { day_of_week: 4, hour_of_day: 14, score: 70 },
  { day_of_week: 4, hour_of_day: 14, score: 72 },
  { day_of_week: 4, hour_of_day: 14, score: 68 },
  { day_of_week: 4, hour_of_day: 15, score: 75 },
  { day_of_week: 4, hour_of_day: 15, score: 72 },
  { day_of_week: 4, hour_of_day: 16, score: 70 },
  // Tuesday early-hour spike (high score, isolated → low coverage).
  { day_of_week: 1, hour_of_day: 3, score: 80 },
  { day_of_week: 1, hour_of_day: 3, score: 78 },
  // Thursday evening cluster.
  { day_of_week: 3, hour_of_day: 20, score: 65 },
  { day_of_week: 3, hour_of_day: 20, score: 63 },
  { day_of_week: 3, hour_of_day: 21, score: 68 },
  // Sunday singleton.
  { day_of_week: 6, hour_of_day: 18, score: 55 },
];

function heatmapColor(mean, lo, hi) {
  // Cool → warm map. Empty cells stay neutral; we only call this when
  // count > 0 so the gradient endpoints are real numbers.
  if (!isFinite(mean) || hi <= lo) return "rgba(255,255,255,0.04)";
  const t = ((mean - lo) / (hi - lo)).max?.(0)?.min?.(1) ?? Math.max(0, Math.min(1, (mean - lo) / (hi - lo)));
  // Lerp from cyan (low) → amber (mid) → red (high).
  const stops = [
    [0.0, [76, 201, 240]],
    [0.5, [251, 191, 36]],
    [1.0, [239, 68, 68]],
  ];
  let lo_stop = stops[0], hi_stop = stops[stops.length - 1];
  for (let i = 0; i < stops.length - 1; i++) {
    if (t >= stops[i][0] && t <= stops[i + 1][0]) {
      lo_stop = stops[i]; hi_stop = stops[i + 1];
      break;
    }
  }
  const span = (hi_stop[0] - lo_stop[0]) || 1;
  const u = (t - lo_stop[0]) / span;
  const rgb = lo_stop[1].map((c, i) => Math.round(c + (hi_stop[1][i] - c) * u));
  return `rgb(${rgb[0]}, ${rgb[1]}, ${rgb[2]})`;
}

let schedOptState = {
  samplesText: JSON.stringify(SAMPLE_DATASET, null, 2),
  topN: 3,
  mode: "spread",
  minGap: 4,
  lastResp: null,
};

async function renderScheduleOptimizer() {
  // Consume a deep-link prefill if any other page (heatmap, capability
  // matrix) stashed engagement samples for us. One-shot: clear the key
  // after consuming so a reload doesn't re-apply stale data.
  try {
    const raw = localStorage.getItem("strivo-sopt-prefill");
    if (raw) {
      const prefill = JSON.parse(raw);
      if (prefill && Array.isArray(prefill.samples) && prefill.samples.length) {
        schedOptState.samplesText = JSON.stringify(prefill.samples, null, 2);
        schedOptState.lastResp = null; // force re-run with new samples
        Toast.success(`Loaded ${prefill.samples.length} sample(s) from ${prefill.source || "deep-link"}`);
      }
      localStorage.removeItem("strivo-sopt-prefill");
    }
  } catch {
    localStorage.removeItem("strivo-sopt-prefill");
  }
  root.innerHTML = chrome(`
    ${pluginHeader("Schedule optimizer",
      "DAW launch-quantize for publish slots — engagement samples → 7×24 grid → top weekly publish times."
    )}
    <div class="sopt-grid">
      <section class="cfg-card sopt-input">
        <h2 class="cfg-title">Engagement samples</h2>
        <p class="pg-cap-hint">JSON list of <code>{day_of_week (0–6), hour_of_day (0–23), score}</code>. Seed below shows the canonical plateau-vs-spike scenario; the buttons fill the box from real recordings.</p>
        <div class="sopt-autofeed">
          <button id="sopt-feed-history" class="sm" type="button" title="Build a sample per finished recording from its started_at + duration. Score weighted by hours streamed in that hour slot. Useful baseline of when you've historically been live.">↘ My streaming history</button>
          <button id="sopt-feed-chatdens" class="sm" type="button" title="Paste a chat log for one of your recordings; chat-density runs server-side, density points map to (DoW, hour, score) via the recording's started_at.">↘ Chat density…</button>
          <span class="pg-cap-hint sopt-autofeed-hint">Auto-feed pulls a real signal into the textarea; edit before running if you want.</span>
        </div>
        <textarea id="sopt-samples" class="sopt-samples" spellcheck="false"></textarea>
        <div class="sopt-controls">
          <label><span>Top N</span>
            <input id="sopt-topn" type="number" min="1" max="14" value="${schedOptState.topN}"/>
          </label>
          <label><span>Mode</span>
            <select id="sopt-mode">
              <option value="spread" ${schedOptState.mode === "spread" ? "selected" : ""}>Spread (min-gap)</option>
              <option value="greedy" ${schedOptState.mode === "greedy" ? "selected" : ""}>Greedy</option>
            </select>
          </label>
          <label><span>Min gap (h)</span>
            <input id="sopt-mingap" type="number" min="0" max="23" value="${schedOptState.minGap}"/>
          </label>
          <button id="sopt-run" class="btn-primary sm" type="button">▶ Run optimizer</button>
        </div>
      </section>
      <section class="cfg-card sopt-output" id="sopt-output">
        <h2 class="cfg-title">Recommendations</h2>
        <div class="pg-cap-hint">Run the optimizer to see top publish slots + the weekly heatmap.</div>
      </section>
    </div>
  `);
  setupChromeHandlers();
  document.getElementById("sopt-samples").value = schedOptState.samplesText;
  document.getElementById("sopt-run").addEventListener("click", () => runScheduleOptimizer());
  document.getElementById("sopt-feed-history")?.addEventListener("click", autoFeedFromHistory);
  document.getElementById("sopt-feed-chatdens")?.addEventListener("click", autoFeedFromChatDensity);
  // Auto-run on mount if we never have — gives users an instant view.
  if (!schedOptState.lastResp) {
    runScheduleOptimizer().catch(() => {});
  } else {
    paintScheduleOptimizer();
  }
}

async function runScheduleOptimizer() {
  const samplesText = document.getElementById("sopt-samples").value.trim();
  let samples;
  try { samples = JSON.parse(samplesText); }
  catch (err) { Toast.error(`Samples JSON invalid: ${err.message}`); return; }
  if (!Array.isArray(samples)) { Toast.error("Samples must be a JSON array"); return; }
  const topN = parseInt(document.getElementById("sopt-topn").value, 10) || 3;
  const mode = document.getElementById("sopt-mode").value;
  const minGap = parseInt(document.getElementById("sopt-mingap").value, 10) || 4;
  schedOptState.samplesText = samplesText;
  schedOptState.topN = topN;
  schedOptState.mode = mode;
  schedOptState.minGap = minGap;
  const out = document.getElementById("sopt-output");
  out.innerHTML = `<h2 class="cfg-title">Recommendations</h2><div class="empty sm">Running…</div>`;
  try {
    const resp = await API.scheduleOptimizerRun("interactive", {
      samples,
      top_n: topN,
      mode,
      min_gap_hours: minGap,
    });
    schedOptState.lastResp = resp;
    paintScheduleOptimizer();
  } catch (err) {
    out.innerHTML = `<h2 class="cfg-title">Recommendations</h2><div class="empty"><div class="glyph">⚠</div>${htmlEscape(err.message)}</div>`;
  }
}

// ── Schedule-optimizer auto-feed helpers ──────────────────────────
// Build EngagementSample[] from sources that already exist in the
// app — historical recordings (presence + duration) and a chat log
// pasted through chat-density.

/** Bucket finished recordings into (day_of_week, hour_of_day) cells
 * keyed by their started_at. Score for each cell = sum of recording
 * durations in hours (a longer presence in that slot is a stronger
 * "people watch me here" signal). Clamped 0.1..5.0 per cell so a
 * single 8h marathon doesn't drown the rest of the week. */
function recordingsToEngagementSamples(recordings) {
  const cells = new Map(); // "dow,hour" → score
  for (const r of recordings) {
    if (r.state !== "Finished" || !r.started_at) continue;
    const ts = Date.parse(r.started_at);
    if (!isFinite(ts)) continue;
    const dur = Number(r.duration_secs) || 0;
    // If duration is zero (older rec), treat as one-hour presence.
    const hours = Math.max(dur / 3600, 1.0);
    const d = new Date(ts);
    const dow = d.getDay();
    const hour = d.getHours();
    const key = `${dow},${hour}`;
    cells.set(key, (cells.get(key) || 0) + hours);
  }
  const out = [];
  for (const [k, score] of cells.entries()) {
    const [dow, hour] = k.split(",").map(Number);
    out.push({ day_of_week: dow, hour_of_day: hour, score: Math.min(5.0, Math.max(0.1, score)) });
  }
  // Sort for stable diffs.
  out.sort((a, b) => (a.day_of_week - b.day_of_week) || (a.hour_of_day - b.hour_of_day));
  return out;
}

async function autoFeedFromHistory() {
  const btn = document.getElementById("sopt-feed-history");
  await withBusy(btn, "Loading…", async () => {
    const r = await API.recordings();
    const recs = r.recordings || r.items || (Array.isArray(r) ? r : []);
    const samples = recordingsToEngagementSamples(recs);
    if (!samples.length) {
      Toast.error("No finished recordings with started_at — nothing to feed");
      return;
    }
    schedOptState.samplesText = JSON.stringify(samples, null, 2);
    const ta = document.getElementById("sopt-samples");
    if (ta) ta.value = schedOptState.samplesText;
    Toast.success(`Loaded ${samples.length} slot(s) from ${recs.length} recording(s) — review then ▶ Run`);
  }).catch((err) => Toast.error(`History feed failed: ${err.message}`));
}

/** Density points → samples. Each point's wall-clock time is
 * `started_at + time_sec`; bucket score by (dow, hour). */
function densityToEngagementSamples(densityPoints, startedAtMs) {
  const cells = new Map();
  for (const p of densityPoints) {
    const t = (Number(p.time_sec) || 0) * 1000 + startedAtMs;
    const d = new Date(t);
    if (isNaN(d.getTime())) continue;
    const key = `${d.getDay()},${d.getHours()}`;
    cells.set(key, (cells.get(key) || 0) + (Number(p.score) || Number(p.count) || 0));
  }
  // Normalise to 0..5 so the optimizer's confidence math stays in
  // the same range as the seeded dataset.
  let max = 0;
  for (const v of cells.values()) if (v > max) max = v;
  const out = [];
  for (const [k, v] of cells.entries()) {
    const [dow, hour] = k.split(",").map(Number);
    const score = max > 0 ? (v / max) * 5.0 : v;
    out.push({ day_of_week: dow, hour_of_day: hour, score: Math.max(0.1, score) });
  }
  out.sort((a, b) => (a.day_of_week - b.day_of_week) || (a.hour_of_day - b.hour_of_day));
  return out;
}

/** Map heatmap fused buckets (per-recording) → engagement samples
 * keyed by (day_of_week, hour_of_day). bucket_start is seconds from
 * the recording's start, so wall-clock time = startedAtMs + bucket
 * × 1000. Score = fused retention proxy × bucket coverage. */
function heatmapBucketsToSamples(buckets, startedAtMs) {
  const cells = new Map();
  const weights = new Map();
  for (const b of buckets || []) {
    const t = (Number(b.bucket_start) || 0) * 1000 + startedAtMs;
    const d = new Date(t);
    if (isNaN(d.getTime())) continue;
    const key = `${d.getDay()},${d.getHours()}`;
    const score = Math.max(0, Number(b.fused) || 0);
    cells.set(key, (cells.get(key) || 0) + score);
    weights.set(key, (weights.get(key) || 0) + 1);
  }
  let max = 0;
  for (const v of cells.values()) if (v > max) max = v;
  const out = [];
  for (const [k, v] of cells.entries()) {
    const [dow, hour] = k.split(",").map(Number);
    // Average × 5 keeps the score in the same 0..5 scale as the
    // seeded dataset.
    const w = weights.get(k) || 1;
    const score = max > 0 ? (v / w) * 5.0 : 0.1;
    out.push({ day_of_week: dow, hour_of_day: hour, score: Math.max(0.1, score) });
  }
  out.sort((a, b) => (a.day_of_week - b.day_of_week) || (a.hour_of_day - b.hour_of_day));
  return out;
}

/** Stash a deep-link prefill so the schedule-optimizer page can
 * consume it on next mount. localStorage so it survives the hash
 * change without state plumbing through the router. */
function stashOptimizerPrefill(samples, source) {
  try {
    localStorage.setItem("strivo-sopt-prefill", JSON.stringify({
      samples,
      source,
      stashed_at: Date.now(),
    }));
  } catch {
    // Quota errors are non-fatal — the user can still paste manually.
  }
}

async function autoFeedFromChatDensity() {
  // One-shot prompt-driven mini-flow: pick a recording, paste its
  // chat log, the rest is automatic.
  const r = await API.recordings();
  const recs = (r.recordings || r.items || []).filter((x) => x.state === "Finished" && x.started_at);
  if (!recs.length) {
    Toast.error("No finished recordings with started_at — nothing to map chat density onto");
    return;
  }
  const pickerLines = recs.slice(0, 12).map((x, i) => `${i + 1}. ${(x.stream_title || x.channel_name || x.id).slice(0, 48)} (${x.started_at})`).join("\n");
  const idx = prompt(`Pick the recording the chat log belongs to (enter row number 1..${Math.min(12, recs.length)}):\n\n${pickerLines}`, "1");
  if (idx == null) return;
  const rec = recs[(parseInt(idx, 10) || 1) - 1];
  if (!rec) { Toast.error("No recording at that row"); return; }
  const csvHint = "Paste an IRC dump OR a CSV with header `time_sec,user,message`.";
  const log = prompt(`${csvHint}\nLeave blank to abort.`, "");
  if (!log || !log.trim()) return;
  const looksLikeCsv = /^[\s]*time_sec\s*,/i.test(log) || /^[\s]*\d+\s*,/.test(log);
  const btn = document.getElementById("sopt-feed-chatdens");
  await withBusy(btn, "Running chat-density…", async () => {
    const body = looksLikeCsv
      ? { csv: log, bucket_secs: 30.0 }
      : { log, stream_start_ts_ms: Date.parse(rec.started_at) || 0, bucket_secs: 30.0 };
    const cd = await API.chatDensityCompute(rec.id, body);
    const points = cd.points || [];
    if (!points.length) {
      Toast.error("chat-density returned no points — log may be empty or malformed");
      return;
    }
    const samples = densityToEngagementSamples(points, Date.parse(rec.started_at) || 0);
    schedOptState.samplesText = JSON.stringify(samples, null, 2);
    const ta = document.getElementById("sopt-samples");
    if (ta) ta.value = schedOptState.samplesText;
    Toast.success(`Loaded ${samples.length} slot(s) from ${points.length} density point(s) (${cd.event_count} chat events)`);
  }).catch((err) => Toast.error(`Chat-density feed failed: ${err.message}`));
}

function paintScheduleOptimizer() {
  const out = document.getElementById("sopt-output");
  if (!out) return;
  const resp = schedOptState.lastResp;
  if (!resp) return;
  const picks = resp.recommendations || [];
  // Pull min/max across non-empty cells for the heatmap colour scale.
  let lo = Infinity, hi = -Infinity;
  const buckets = resp.grid?.buckets || [];
  for (const row of buckets) {
    for (const b of row) {
      if (b.count > 0) { lo = Math.min(lo, b.mean); hi = Math.max(hi, b.mean); }
    }
  }
  if (!isFinite(lo)) { lo = 0; hi = 1; }
  // Header row + day rows.
  const hourCells = [];
  for (let h = 0; h < 24; h++) hourCells.push(`<div class="sopt-hour-label">${h}</div>`);
  const dayRows = DAYS_OF_WEEK.map((day, dIdx) => {
    const cells = [];
    for (let h = 0; h < 24; h++) {
      const b = buckets[dIdx]?.[h] || { mean: 0, count: 0 };
      if (b.count === 0) {
        cells.push(`<div class="sopt-cell sopt-cell-empty" title="${day} ${h}:00 · no data"></div>`);
      } else {
        const color = heatmapColor(b.mean, lo, hi);
        const isPick = picks.some(p => p.day_of_week === dIdx && p.hour_of_day === h);
        cells.push(`<div class="sopt-cell ${isPick ? "sopt-cell-pick" : ""}"
          title="${day} ${h}:00 · mean ${b.mean.toFixed(1)} · n=${b.count}"
          style="background:${color}"></div>`);
      }
    }
    return `<div class="sopt-day-label">${day}</div>${cells.join("")}`;
  }).join("");
  const picksHtml = picks.map((p, i) => `
    <div class="sopt-pick">
      <div class="sopt-pick-rank">#${i + 1}</div>
      <div class="sopt-pick-when"><strong>${DAYS_OF_WEEK[p.day_of_week]}</strong> ${String(p.hour_of_day).padStart(2, "0")}:00</div>
      <div class="sopt-pick-mean">mean <strong>${p.mean_score.toFixed(1)}</strong></div>
      <div class="sopt-pick-bars">
        <div class="sopt-pick-bar" title="confidence ${(p.confidence*100).toFixed(0)}%"><span style="width:${(p.confidence*100).toFixed(1)}%"></span></div>
        <div class="sopt-pick-bar coverage" title="coverage ${(p.window_coverage*100).toFixed(0)}%"><span style="width:${(p.window_coverage*100).toFixed(1)}%"></span></div>
      </div>
      <div class="sopt-pick-meta pg-cap-hint">n=${p.sample_count} · conf ${(p.confidence*100).toFixed(0)}% · coverage ${(p.window_coverage*100).toFixed(0)}%</div>
    </div>`).join("");
  out.innerHTML = `
    <h2 class="cfg-title">Recommendations <span class="pg-cap-hint">${resp.sample_count} sample${resp.sample_count===1?"":"s"} · ${picks.length} pick${picks.length===1?"":"s"}</span></h2>
    <div class="sopt-picks">${picksHtml || '<div class="empty sm">No picks — try a wider range or check your sample data.</div>'}</div>
    <h3 class="sopt-heatmap-h">Weekly heatmap</h3>
    <div class="sopt-heatmap">
      <div class="sopt-corner"></div>
      ${hourCells.join("")}
      ${dayRows}
    </div>
    <div class="sopt-legend">
      <span>${lo.toFixed(1)}</span>
      <div class="sopt-legend-bar"></div>
      <span>${hi.toFixed(1)}</span>
    </div>
  `;
}

// ── Crunchr ──────────────────────────────────────────────────────────
async function renderCrunchr() {
  const resp = await API.crunchrRecordings();
  root.removeAttribute("aria-busy");
  const recs = (resp && resp.recordings) || [];
  const rows = recs
    .map((r) => {
      const an = r.has_analysis
        ? '<span class="cfg-badge ok">analyzed</span>'
        : "";
      return `
        <a class="pg-row" href="#/plugins/crunchr/rec/${encodeURIComponent(r.recording_id)}">
          <span class="pg-row-main">
            <span class="pg-row-title">${htmlEscape(niceTitle(r.title) || "(untitled)")}</span>
            <span class="pg-row-sub">${htmlEscape(r.channel_name)} · ${htmlEscape(r.created_at || "")}</span>
          </span>
          <span class="pg-row-meta">
            <span class="cfg-badge status-${htmlEscape(r.status)}">${htmlEscape(r.status)}</span>
            <span class="pg-row-num">${formatCount(r.segment_count)} segs</span>
            ${an}
          </span>
        </a>`;
    })
    .join("");
  root.innerHTML = chrome(`
    ${pluginHeader("Crunchr", "Transcribed recordings. Click one to read its transcript and analysis.", "#/plugins")}
    <div class="pg-search">
      <input id="crunchr-q" type="search" placeholder="Search transcripts…"
             autocomplete="off" aria-label="Search transcripts" />
    </div>
    <div id="crunchr-search-results"></div>
    <div class="pg-list">${rows || `<div class="empty">Nothing transcribed yet.</div>
      <div class="pg-getstarted"><strong>Get started:</strong> open a finished recording's ⓘ Info on the Recordings page and click <em>Generate subtitles</em>. Transcripts land here after the run completes.</div>`}</div>
  `);
  setupChromeHandlers();

  const q = document.getElementById("crunchr-q");
  const out = document.getElementById("crunchr-search-results");
  let timer = null;
  q.addEventListener("input", () => {
    clearTimeout(timer);
    const term = q.value.trim();
    if (!term) {
      out.innerHTML = "";
      return;
    }
    timer = setTimeout(async () => {
      try {
        const r = await API.crunchrSearch(term);
        const hits = (r && r.results) || [];
        out.innerHTML = hits.length
          ? `<div class="pg-list pg-search-hits">${hits
              .map(
                (h) => `
            <a class="pg-row" href="#/plugins/crunchr/rec/${encodeURIComponent(findRecIdForHit(recs, h))}">
              <span class="pg-row-main">
                <span class="pg-row-title">${htmlEscape(h.snippet)}</span>
                <span class="pg-row-sub">${htmlEscape(h.video_title)} · ${htmlEscape(h.channel_name)} · ${fmtClock(h.start_sec)}</span>
              </span>
            </a>`,
              )
              .join("")}</div>`
          : '<div class="empty sm">No matches.</div>';
      } catch (e) {
        out.innerHTML = `<div class="empty sm">${htmlEscape(e.message)}</div>`;
      }
    }, 220);
  });
}

// FTS rows don't carry a recording_id; match on title+channel against the
// already-loaded list so a hit links to the right transcript.
function findRecIdForHit(recs, hit) {
  const m = recs.find(
    (r) => r.title === hit.video_title && r.channel_name === hit.channel_name,
  );
  return m ? m.recording_id : "";
}

async function renderCrunchrRecording(id) {
  const d = await API.crunchrRecording(id);
  root.removeAttribute("aria-busy");
  const topics = (d.topics || [])
    .map((t) => `<span class="pg-chip">${htmlEscape(t)}</span>`)
    .join("");
  const sentiment = d.sentiment
    ? `<span class="cfg-badge sentiment-${htmlEscape(d.sentiment)}">${htmlEscape(d.sentiment)}</span>`
    : "";
  const analysis = d.summary || topics || sentiment
    ? `<section class="cfg-card">
         <h2 class="cfg-title">Analysis ${sentiment}</h2>
         ${d.summary ? `<p class="pg-summary">${htmlEscape(d.summary)}</p>` : ""}
         ${topics ? `<div class="pg-chips">${topics}</div>` : ""}
       </section>`
    : "";

  const segments = d.segments || [];
  // Build the set of distinct speakers for the filter chip row.
  const speakers = [...new Set(segments.map((s) => s.speaker).filter(Boolean))].sort();
  // Group consecutive same-speaker lines into a single block (Descript-
  // style readability). Each block keeps the seek timestamp of its
  // first line; its `lines` keep their own timestamps for line-level
  // click-to-seek inside the block.
  const blocks = [];
  for (const seg of segments) {
    const top = blocks[blocks.length - 1];
    if (top && top.speaker === seg.speaker) {
      top.lines.push(seg);
    } else {
      blocks.push({ speaker: seg.speaker, lines: [seg] });
    }
  }
  // Speaker chip colours — deterministic per speaker name so the same
  // person stays the same colour across reloads / recordings.
  const speakerColour = (name) => {
    if (!name) return "#888";
    let h = 0;
    for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) | 0;
    return `hsl(${Math.abs(h) % 360}, 55%, 65%)`;
  };

  const chipsRow = speakers.length
    ? `<div class="cr-chips" id="cr-chips">
        <button class="cr-chip is-active" data-spk="" type="button">all</button>
        ${speakers
          .map(
            (s) =>
              `<button class="cr-chip is-active" data-spk="${htmlEscape(s)}" style="--cr-spk:${speakerColour(s)}" type="button"><span class="cr-chip-dot"></span>${htmlEscape(s)}</button>`,
          )
          .join("")}
      </div>`
    : "";

  const blockHtml = blocks
    .map((b) => {
      const colour = speakerColour(b.speaker);
      const firstStart = b.lines[0]?.start_sec ?? 0;
      const linesHtml = b.lines
        .map(
          (line) =>
            `<span class="cr-line" data-seek="${line.start_sec ?? 0}" title="Open player at ${fmtClock(line.start_sec)}">${htmlEscape(line.text)}</span>`,
        )
        .join(" ");
      return `<div class="cr-block" data-spk="${htmlEscape(b.speaker || "")}">
        <div class="cr-block-meta">
          <button class="cr-block-jump" data-seek="${firstStart}" title="Jump to ${fmtClock(firstStart)}">${fmtClock(firstStart)}</button>
          ${b.speaker ? `<span class="cr-block-spk" style="--cr-spk:${colour}"><span class="cr-spk-dot"></span>${htmlEscape(b.speaker)}</span>` : ""}
        </div>
        <div class="cr-block-body">${linesHtml}</div>
      </div>`;
    })
    .join("");

  root.innerHTML = chrome(`
    ${pluginHeader(d.title || "Transcript", `${htmlEscape(d.channel_name)} · ${htmlEscape(d.status)}`, "#/plugins/crunchr")}
    <div class="pg-verbs">
      <button id="retranscribe" data-rec="${htmlEscape(d.recording_id)}">↻ Re-transcribe</button>
      <a class="pg-linkbtn" href="#/plugins/insights/rec/${encodeURIComponent(d.recording_id)}">View insights →</a>
      <button id="cr-export-vtt" class="pg-linkbtn" type="button">Export .vtt</button>
      <button id="cr-export-md" class="pg-linkbtn" type="button">Copy as markdown</button>
      <button id="cr-chapters" class="pg-linkbtn" type="button" title="Generate YouTube/Twitch chapter markers from the transcript">Generate chapters</button>
      <button id="cr-brandsafe" class="pg-linkbtn" type="button" title="Pre-publish brand-safety scan (slurs / profanity / restricted games / music mentions)">⚠ Brand-safety scan</button>
      <div class="cr-caption-export">
        <span class="cr-caption-label">Captions:</span>
        <a class="pg-linkbtn" download="${htmlEscape((d.recording_id || "recording") + ".srt")}" href="${htmlEscape(API.captionsExportUrl(d.recording_id, "srt", "en"))}">.srt</a>
        <a class="pg-linkbtn" download="${htmlEscape((d.recording_id || "recording") + ".vtt")}" href="${htmlEscape(API.captionsExportUrl(d.recording_id, "vtt", "en"))}">.vtt</a>
        <a class="pg-linkbtn" download="${htmlEscape((d.recording_id || "recording") + ".txt")}" href="${htmlEscape(API.captionsExportUrl(d.recording_id, "txt", "en"))}">.txt</a>
        <select id="cr-caption-lang" title="Target language (translation backend ships in a follow-up; today returns identity)">
          <option value="en">en (identity)</option>
          <option value="es">es</option>
          <option value="pt">pt</option>
          <option value="ja">ja</option>
          <option value="de">de</option>
          <option value="fr">fr</option>
        </select>
      </div>
    </div>
    <section class="cfg-card" id="cr-chapters-card" hidden>
      <h2 class="cfg-title">Chapters</h2>
      <p class="pg-cap-hint">Heuristic chapter markers derived from the transcript topic-shift. Paste straight into a YouTube/Twitch description.</p>
      <div class="cr-chapters-list" id="cr-chapters-list"></div>
      <details class="cr-chapters-block"><summary>Description block</summary><pre id="cr-chapters-pre"></pre></details>
      <div class="cr-chapters-actions">
        <button id="cr-chapters-copy" class="pg-linkbtn" type="button">Copy</button>
      </div>
    </section>
    ${analysis}
    <section class="cfg-card" id="cr-heatmap-card" hidden>
      <h2 class="cfg-title">Heatmap <span class="pg-cap-hint">talk · action · highlight · brand-safety (anti-signal)</span></h2>
      <div id="cr-heatmap-strip"></div>
      <div id="cr-heatmap-top"></div>
    </section>
    <section class="cfg-card" id="cr-brandsafe-card" hidden>
      <h2 class="cfg-title">Brand-safety verdicts <span id="cr-brandsafe-count"></span></h2>
      <div id="cr-brandsafe-list"></div>
    </section>
    <section class="cfg-card">
      <h2 class="cfg-title">Transcript <span class="pg-cap-hint">${speakers.length} speaker${speakers.length === 1 ? "" : "s"} · ${blocks.length} block${blocks.length === 1 ? "" : "s"}</span></h2>
      <div class="cr-retention" id="cr-retention" hidden></div>
      ${chipsRow}
      <div class="pg-transcript cr-transcript">${blockHtml || '<div class="empty sm">No segments — transcription may still be running.</div>'}</div>
    </section>
  `);
  // Lazy multi-signal heatmap — surfaces alongside the existing
  // retention curve below. Pulls cuepoints/highlights/brandsafe from
  // their caches; no second ffmpeg pass required.
  if (d.recording_id) {
    API.heatmapCompute(d.recording_id, 30).then((resp) => {
      const card = document.getElementById("cr-heatmap-card");
      const strip = document.getElementById("cr-heatmap-strip");
      const topHost = document.getElementById("cr-heatmap-top");
      if (!card || !strip || !topHost) return;
      const buckets = resp.buckets || [];
      if (!buckets.length) return;
      const dur = resp.duration_sec || 1;
      const bandRow = (key, label, colour) => {
        const bars = buckets
          .map((b) => `<span class="cr-hm-cell" style="--cr-hm-h:${Math.round(b[key] * 100)}%;--cr-hm-c:${colour};" title="${fmtClock(b.bucket_start)} · ${label} ${(b[key] * 100).toFixed(0)}%"></span>`)
          .join("");
        return `<div class="cr-hm-row"><span class="cr-hm-label">${htmlEscape(label)}</span><div class="cr-hm-bars">${bars}</div></div>`;
      };
      const fusedRow = `<div class="cr-hm-row cr-hm-row-fused"><span class="cr-hm-label"><strong>fused</strong></span><div class="cr-hm-bars">${buckets
        .map((b) => `<a class="cr-hm-cell cr-hm-fused-bar" data-seek="${b.bucket_start}" href="#" style="--cr-hm-h:${Math.round(b.fused * 100)}%;--cr-hm-hue:${200 - Math.round((b.highlight - b.brandsafe) * 60)};" title="${fmtClock(b.bucket_start)} · retention ${(b.fused * 100).toFixed(0)}%"></a>`)
        .join("")}</div></div>`;
      strip.innerHTML = `
        ${bandRow("talk", "talk", "hsl(200, 70%, 55%)")}
        ${bandRow("action", "action", "hsl(40, 80%, 60%)")}
        ${bandRow("highlight", "highlight", "hsl(120, 60%, 55%)")}
        ${bandRow("brandsafe", "brandsafe", "hsl(0, 70%, 60%)")}
        ${fusedRow}
        <div class="rec-cp-axis"><span>0:00</span><span>${fmtClock(dur)}</span></div>`;
      const top = (resp.top_k || []).map(
        (b) => `<a class="cr-hm-top" href="#" data-seek="${b.bucket_start}">${fmtClock(b.bucket_start)} <span>${(b.fused * 100).toFixed(0)}%</span></a>`,
      );
      topHost.innerHTML = top.length
        ? `<h5 class="ins-cmp-h">Top moments</h5><div class="cr-hm-top-row">${top.join("")}</div>`
        : "";
      strip.querySelectorAll(".cr-hm-fused-bar, .cr-hm-top").forEach((el) => {
        el.addEventListener("click", (e) => {
          e.preventDefault();
          seek(parseFloat(el.dataset.seek || "0"));
        });
      });
      // Deep-link action: hand the recording's retention buckets to
      // the schedule-optimizer so the user can ask 'when should I
      // publish a stream that retains people like this one?'.
      const actionRow = document.createElement("div");
      actionRow.className = "cr-hm-actions";
      actionRow.innerHTML = `<button class="sm cr-hm-to-sopt" type="button" title="Map this recording's fused retention buckets to (DoW, hour) engagement samples and open the schedule optimizer pre-loaded with them.">↘ Send to schedule optimizer</button>`;
      topHost.appendChild(actionRow);
      actionRow.querySelector(".cr-hm-to-sopt")?.addEventListener("click", async () => {
        try {
          // Look up the recording's started_at from the global list so
          // the heatmap-bucket → wall-clock mapping is accurate.
          const list = await API.recordings();
          const recs = list.recordings || list.items || [];
          const rec = recs.find((x) => x.id === d.recording_id);
          if (!rec || !rec.started_at) {
            Toast.error("No started_at on this recording — can't map buckets to wall-clock cells");
            return;
          }
          const samples = heatmapBucketsToSamples(buckets, Date.parse(rec.started_at) || 0);
          if (!samples.length) {
            Toast.error("Heatmap buckets produced no engagement samples");
            return;
          }
          stashOptimizerPrefill(samples, `heatmap of ${rec.stream_title || rec.id.slice(0, 8)}`);
          location.hash = "#/plugins/schedule-optimizer";
        } catch (err) {
          Toast.error(`Deep-link failed: ${err.message}`);
        }
      });
      card.hidden = false;
    }).catch(() => {});
  }

  // Lazy retention curve — async so the transcript paints first.
  if (d.recording_id) {
    API.insightsRetention(d.recording_id, 30).then((retention) => {
      const host = document.getElementById("cr-retention");
      if (!host || !retention || !retention.points || !retention.points.length) return;
      const dur = retention.duration_sec || 1;
      // Compose a sparkline-ish strip: each bucket is a vertical bar
      // whose height encodes retention and whose hue carries the
      // talk/action mix (cyan-ish for talk-heavy, magenta-ish for
      // action-heavy). Click any bar → seek the (future) player.
      const bars = retention.points
        .map((p) => {
          const pct = Math.max(0, Math.min(1, p.retention || 0));
          const hue = 200 - Math.round((p.action_density - p.talk_density) * 60);
          return `<a class="cr-ret-bar" href="#" data-seek="${p.bucket_start}" title="${fmtClock(p.bucket_start)} · retention ${(pct * 100).toFixed(0)}%" style="--ret-h:${(pct * 100).toFixed(0)}%; --ret-hue:${hue}"></a>`;
        })
        .join("");
      host.hidden = false;
      host.innerHTML = `
        <div class="cr-ret-head">
          <span>Retention proxy</span>
          <span class="pg-cap-hint">${retention.points.length} buckets · ${retention.bucket_secs}s each · talk + cuepoint density</span>
        </div>
        <div class="cr-ret-strip" role="img" aria-label="Retention curve">${bars}</div>
        <div class="rec-cp-axis"><span>0:00</span><span>${fmtClock(dur)}</span></div>`;
      host.querySelectorAll(".cr-ret-bar").forEach((el) => {
        el.addEventListener("click", (e) => {
          e.preventDefault();
          seek(parseFloat(el.dataset.seek || "0"));
        });
      });
    }).catch(() => {});
  }
  setupChromeHandlers();

  const btn = document.getElementById("retranscribe");
  if (btn) {
    btn.addEventListener("click", () =>
      dispatchVerb("crunchr", "Re-transcribe", [btn.dataset.rec], btn),
    );
  }

  // Click any line/jump → open the recording player at that timestamp.
  // openRecordingPlayer reads a `seekTo` argument and the player binds
  // it in the next iteration when we extend the player.
  const seek = (sec) => {
    if (!d.recording_id) return;
    openRecordingPlayer(d.recording_id, { seekTo: sec });
  };
  document.querySelectorAll(".cr-line, .cr-block-jump").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      seek(parseFloat(el.dataset.seek || "0"));
    });
  });

  // Speaker filter — toggling a chip hides blocks not matching the
  // active speaker set. "all" is the reset.
  document.getElementById("cr-chips")?.addEventListener("click", (e) => {
    const btn = e.target.closest(".cr-chip");
    if (!btn) return;
    const chips = [...document.querySelectorAll(".cr-chip")];
    if (btn.dataset.spk === "") {
      chips.forEach((c) => c.classList.add("is-active"));
    } else {
      btn.classList.toggle("is-active");
      const allChip = chips.find((c) => c.dataset.spk === "");
      if (allChip) allChip.classList.toggle("is-active", false);
    }
    const active = new Set(
      chips
        .filter((c) => c.classList.contains("is-active") && c.dataset.spk)
        .map((c) => c.dataset.spk),
    );
    const showAll = chips.find((c) => c.dataset.spk === "" && c.classList.contains("is-active"));
    document.querySelectorAll(".cr-block").forEach((blk) => {
      const visible = showAll || active.size === 0 || active.has(blk.dataset.spk);
      blk.classList.toggle("cr-block-hidden", !visible);
    });
  });

  // .vtt export — bake segments into a WebVTT file and trigger download.
  document.getElementById("cr-export-vtt")?.addEventListener("click", () => {
    const lines = ["WEBVTT", ""];
    const fmtVtt = (sec) => {
      const ms = Math.max(0, Math.round((sec ?? 0) * 1000));
      const h = String(Math.floor(ms / 3_600_000)).padStart(2, "0");
      const m = String(Math.floor((ms / 60_000) % 60)).padStart(2, "0");
      const s = String(Math.floor((ms / 1000) % 60)).padStart(2, "0");
      const f = String(ms % 1000).padStart(3, "0");
      return `${h}:${m}:${s}.${f}`;
    };
    segments.forEach((s, i) => {
      const start = fmtVtt(s.start_sec);
      const end = fmtVtt(s.end_sec ?? (s.start_sec ?? 0) + 5);
      lines.push(String(i + 1), `${start} --> ${end}`);
      lines.push(s.speaker ? `<v ${s.speaker}>${s.text}` : s.text, "");
    });
    const blob = new Blob([lines.join("\n")], { type: "text/vtt" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${(d.title || "transcript").replace(/[\\/:*?"<>|]/g, "_")}.vtt`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    Toast.success("WebVTT exported");
  });

  // Caption language selector — rewrites the .srt/.vtt/.txt URLs so a
  // change reflects in all three download links. Identity-only today
  // (Pro plugin backend ships in a follow-up).
  document.getElementById("cr-caption-lang")?.addEventListener("change", (e) => {
    const lang = e.target.value;
    document
      .querySelectorAll(".cr-caption-export a[download]")
      .forEach((a) => {
        const fmt = a.textContent.replace(".", "");
        a.href = API.captionsExportUrl(d.recording_id, fmt, lang);
      });
  });

  // Brandsafe scan — runs the scanners, renders verdicts.
  document.getElementById("cr-brandsafe")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Scanning…", async () => {
      const resp = await API.brandsafeScan(d.recording_id);
      const verdicts = resp.verdicts || [];
      const card = document.getElementById("cr-brandsafe-card");
      const list = document.getElementById("cr-brandsafe-list");
      const count = document.getElementById("cr-brandsafe-count");
      if (!card || !list || !count) return;
      card.hidden = false;
      count.innerHTML = verdicts.length
        ? `<span class="pg-cap-hint">${verdicts.length} verdict${verdicts.length === 1 ? "" : "s"} · category "${htmlEscape(resp.category)}"</span>`
        : '<span class="cfg-badge ok">all clear</span>';
      if (!verdicts.length) {
        list.innerHTML = '<div class="empty sm">No content-safety risks detected. Scan covers slurs, profanity, restricted game categories, and music mentions.</div>';
        return;
      }
      const sevColour = {
        critical: "hsl(0, 80%, 60%)",
        high: "hsl(20, 80%, 60%)",
        medium: "hsl(40, 80%, 60%)",
        low: "hsl(200, 60%, 60%)",
      };
      list.innerHTML = verdicts
        .map(
          (v) => `
        <div class="cr-bs-row sev-${htmlEscape(v.severity)}" style="--bs-c:${sevColour[v.severity] || sevColour.low}">
          <span class="cr-bs-sev">${htmlEscape(v.severity)}</span>
          <div class="cr-bs-body">
            <div class="cr-bs-head">
              <span class="cr-bs-kind">${htmlEscape(v.kind.replace(/_/g, " "))}</span>
              ${v.platform ? `<span class="mon-plat plat-${htmlEscape(v.platform.toLowerCase())}">${htmlEscape(v.platform)}</span>` : ""}
              ${typeof v.at_sec === "number" ? `<button class="cr-bs-jump" data-seek="${v.at_sec}">${fmtClock(v.at_sec)}</button>` : ""}
            </div>
            <div class="cr-bs-snippet">${htmlEscape(v.snippet)}</div>
            <div class="cr-bs-fix">${htmlEscape(v.fix_hint)}</div>
          </div>
        </div>`,
        )
        .join("");
      list.querySelectorAll(".cr-bs-jump").forEach((el) => {
        el.addEventListener("click", () => seek(parseFloat(el.dataset.seek || "0")));
      });
      Toast.success(`Scan complete: ${verdicts.length} verdict(s)`);
    }).catch((err) => Toast.error(`Brand-safety scan failed: ${err.message}`));
  });

  // Chapters — POST to /api/v1/plugins/chapters/<id>, render the result.
  document.getElementById("cr-chapters")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Generating…", async () => {
      const resp = await API.chaptersGenerate(d.recording_id);
      const card = document.getElementById("cr-chapters-card");
      const list = document.getElementById("cr-chapters-list");
      const pre = document.getElementById("cr-chapters-pre");
      if (!card || !list || !pre) return;
      list.innerHTML = (resp.chapters || [])
        .map(
          (c) =>
            `<a class="cr-chapter" href="#" data-seek="${c.start_sec}"><span class="cr-chapter-time">${fmtClock(c.start_sec)}</span><span class="cr-chapter-title">${htmlEscape(c.title)}</span></a>`,
        )
        .join("") || '<div class="empty sm">No chapter boundaries detected.</div>';
      pre.textContent = resp.description || "";
      card.hidden = false;
      list.querySelectorAll(".cr-chapter").forEach((el) => {
        el.addEventListener("click", (e) => {
          e.preventDefault();
          seek(parseFloat(el.dataset.seek || "0"));
        });
      });
      document.getElementById("cr-chapters-copy")?.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(resp.description || "");
          Toast.success("Description copied");
        } catch (_) {
          Toast.error("Couldn't copy");
        }
      });
      Toast.success(`Generated ${(resp.chapters || []).length} chapter(s)`);
    }).catch((err) => Toast.error(`Chapters failed: ${err.message}`));
  });

  // Markdown export — copy to clipboard, ready to paste into a notes
  // app / show notes draft / Casebook plugin (iter 12).
  document.getElementById("cr-export-md")?.addEventListener("click", async () => {
    const md = blocks
      .map((b) => {
        const head = `**[${fmtClock(b.lines[0].start_sec)}] ${b.speaker || "—"}**`;
        const body = b.lines.map((l) => l.text).join(" ");
        return `${head}\n\n${body}`;
      })
      .join("\n\n");
    try {
      await navigator.clipboard.writeText(md);
      Toast.success("Markdown copied to clipboard");
    } catch (e) {
      Toast.error("Couldn't copy to clipboard");
    }
  });
}

// ── Archiver ─────────────────────────────────────────────────────────
async function renderArchiver() {
  const resp = await API.archiverChannels();
  root.removeAttribute("aria-busy");
  const chans = (resp && resp.channels) || [];
  const rows = chans
    .map((c) => {
      const pct = c.video_count
        ? Math.round((c.downloaded_count / c.video_count) * 100)
        : 0;
      return `
        <a class="pg-row" href="#/plugins/archiver/${encodeURIComponent(c.id)}">
          <span class="pg-row-main">
            <span class="pg-row-title">${htmlEscape(c.name)}</span>
            <span class="pg-row-sub plat-${htmlEscape((c.platform || "").toLowerCase())}">${htmlEscape(c.platform)} · ${htmlEscape(c.last_scan || "never scanned")}</span>
          </span>
          <span class="pg-row-meta">
            <span class="pg-row-num">${formatCount(c.downloaded_count)} / ${formatCount(c.video_count)}</span>
            <span class="pg-mini-gauge"><span style="width:${pct}%"></span></span>
          </span>
        </a>`;
    })
    .join("");
  root.innerHTML = chrome(`
    ${pluginHeader("Archiver", "Tracked channels and their back-catalog download status.", "#/plugins")}
    <div class="pg-list">${rows || `<div class="empty">No channels archived yet.</div>
      <div class="pg-getstarted"><strong>Get started:</strong> add a channel and enable Archiver tandem from its row — Archiver back-fills the channel's existing VODs in priority order.</div>`}</div>
  `);
  setupChromeHandlers();
}

async function renderArchiverVideos(channelId) {
  const resp = await API.archiverVideos(channelId);
  root.removeAttribute("aria-busy");
  const vids = (resp && resp.videos) || [];
  const rows = vids
    .map(
      (v) => `
      <div class="pg-row pg-row-static">
        <span class="pg-row-main">
          <span class="pg-row-title">${htmlEscape(niceTitle(v.title))}</span>
          <span class="pg-row-sub">${htmlEscape(v.upload_date || "")}${v.playlist ? " · " + htmlEscape(v.playlist) : ""}${v.duration ? " · " + fmtClock(v.duration) : ""}</span>
        </span>
        <span class="pg-row-meta">
          ${v.downloaded ? '<span class="cfg-badge ok">downloaded</span>' : '<span class="cfg-badge">pending</span>'}
        </span>
      </div>`,
    )
    .join("");
  root.innerHTML = chrome(`
    ${pluginHeader("Catalog", `${vids.length} videos`, "#/plugins/archiver")}
    <div class="pg-list">${rows || '<div class="empty">No catalog entries.</div>'}</div>
  `);
  setupChromeHandlers();
}

// ── Viewguard ────────────────────────────────────────────────────────
async function renderViewguard() {
  const resp = await API.viewguardVerdicts();
  root.removeAttribute("aria-busy");
  const verdicts = (resp && resp.verdicts) || [];
  const cards = verdicts
    .map((v) => {
      const pct = Math.round((v.final_score || 0) * 100);
      const contributors = Array.isArray(v.contributors)
        ? v.contributors
        : v.contributors && v.contributors.contributors
          ? v.contributors.contributors
          : [];
      const bars = contributors
        .map((c) => {
          const name = c.kind || c.detector || c.name || "signal";
          const score = c.score != null ? c.score : c.weight != null ? c.weight : 0;
          return `<div class="vg-contrib">
              <span class="vg-contrib-name">${htmlEscape(String(name))}</span>
              <span class="vg-bar"><span style="width:${Math.round(score * 100)}%"></span></span>
            </div>`;
        })
        .join("");
      return `
        <section class="cfg-card vg-card">
          <div class="vg-head">
            <span class="vg-channel">${htmlEscape(v.channel_id)}</span>
            <span class="cfg-badge vg-band vg-band-${htmlEscape((v.band || "").toLowerCase())}">${htmlEscape(v.band)}</span>
          </div>
          <div class="vg-score">
            <span class="vg-score-num">${pct}%</span>
            <span class="vg-score-label">suspicion</span>
          </div>
          ${bars ? `<div class="vg-contribs">${bars}</div>` : ""}
          <div class="vg-when">${htmlEscape(v.stream_started_at || "")}</div>
        </section>`;
    })
    .join("");
  root.innerHTML = chrome(`
    ${pluginHeader("Viewguard", "Latest viewbot-fraud verdict per channel. Higher = more suspicious.", "#/plugins")}
    <div id="vg-trend-summary"></div>
    <div class="cfg-grid">${cards || `<div class="empty">No verdicts yet — viewers are sampled while channels are live.</div>
      <div class="pg-getstarted"><strong>Get started:</strong> Viewguard runs automatically during live Twitch captures. Verdicts appear here after a stream ends and samples are scored.</div>`}</div>
  `);
  setupChromeHandlers();
  // Lazy-load the trend dashboard so the per-channel cards paint
  // first. Pure render; the summary inserts above the grid when ready.
  API.viewguardTrend().then(renderViewguardTrend).catch(() => {});
}

// Render the cross-stream trend dashboard above the per-channel grid.
// Shows banded watchlists (Critical / Warning / Watch) — Clear is
// hidden by default since there's nothing actionable there.
function renderViewguardTrend(resp) {
  const host = document.getElementById("vg-trend-summary");
  if (!host || !resp || !resp.watchlist) return;
  const wl = resp.watchlist;
  const bandSpec = [
    ["critical", "Critical", "hsl(0, 80%, 60%)"],
    ["warning", "Warning", "hsl(20, 80%, 60%)"],
    ["watch", "Multi-stream", "hsl(40, 80%, 60%)"],
  ];
  const actionLabel = {
    no_action: "No action",
    keep_monitoring: "Keep monitoring",
    manual_review: "Manual review",
    escalate_and_report: "Escalate + report",
  };
  const directionGlyph = {
    improving: "↓",
    stable: "→",
    worsening: "↑",
  };
  const bands = bandSpec
    .map(([key, label, colour]) => {
      const list = wl[key] || [];
      if (!list.length) return "";
      const rows = list
        .map(
          (t) => `
        <div class="vg-trend-row" style="--vg-c:${colour}">
          <span class="vg-trend-name">${htmlEscape(t.channel_name)}</span>
          <span class="vg-trend-score">${(t.latest_score * 100).toFixed(0)}%</span>
          <span class="vg-trend-dir" title="latest ${t.latest_score.toFixed(2)} vs rolling mean ${t.rolling_mean.toFixed(2)} (Δ ${t.delta >= 0 ? "+" : ""}${(t.delta * 100).toFixed(0)}pp)">
            ${htmlEscape(directionGlyph[t.direction] || "→")} ${htmlEscape(t.direction)}
          </span>
          ${t.anomaly ? '<span class="vg-trend-anomaly" title="latest deviates from rolling mean by >20pp">anomaly</span>' : ""}
          <span class="vg-trend-samples">${t.samples} sample${t.samples === 1 ? "" : "s"}</span>
          <span class="vg-trend-action">${htmlEscape(actionLabel[t.suggested_action] || t.suggested_action)}</span>
        </div>`,
        )
        .join("");
      return `<details class="vg-trend-band" open data-band="${htmlEscape(key)}" style="--vg-c:${colour}">
        <summary><strong>${htmlEscape(label)}</strong> <span class="pg-cap-hint">${list.length} channel${list.length === 1 ? "" : "s"}</span></summary>
        <div class="vg-trend-list">${rows}</div>
      </details>`;
    })
    .filter(Boolean)
    .join("");
  const clearCount = (wl.clear || []).length;
  host.innerHTML = `
    <section class="cfg-card vg-trend-card">
      <h2 class="cfg-title">Cross-stream trend <span class="pg-cap-hint">${resp.samples} verdict sample${resp.samples === 1 ? "" : "s"} · ${clearCount} clear channel${clearCount === 1 ? "" : "s"} hidden</span></h2>
      ${bands || '<div class="empty sm">No actionable trends right now — every channel is in the Clear band.</div>'}
    </section>`;
}

// ── Insights ─────────────────────────────────────────────────────────
let insightsState = { stopwords: false };
async function renderInsights() {
  const parts = routeParts();
  const recId = parts[2] === "rec" ? parts[3] : null;
  const [wordsResp, topicsResp, crunchrResp] = await Promise.all([
    API.insightsWords({ stopwords: insightsState.stopwords, limit: 40 }),
    API.insightsTopics(),
    // Pull the list of transcribed recordings so the comparison
    // picker has options. Failure is fine — the picker just stays
    // empty.
    API.crunchrRecordings().catch(() => ({ recordings: [] })),
  ]);
  root.removeAttribute("aria-busy");
  const words = (wordsResp && wordsResp.words) || [];
  const max = words.reduce((m, w) => Math.max(m, w.count), 0) || 1;
  const wordRows = words
    .map(
      (w) => `
      <div class="wf-row">
        <span class="wf-word">${htmlEscape(w.word)}</span>
        <span class="wf-bar"><span style="width:${Math.round((w.count / max) * 100)}%"></span></span>
        <span class="wf-count">${formatCount(w.count)}</span>
      </div>`,
    )
    .join("");
  const topics = (topicsResp && topicsResp.topics) || [];
  const topicChips = topics
    .slice(0, 60)
    .map(
      (t) =>
        `<span class="pg-chip" title="${htmlEscape(t.first_seen)} → ${htmlEscape(t.last_seen)}">${htmlEscape(t.topic)} <em>${t.count}</em></span>`,
    )
    .join("");

  // Comparison picker: pick any two transcribed recordings.
  const allRecs = (crunchrResp && crunchrResp.recordings) || [];
  const recOptions = allRecs
    .map(
      (r) =>
        `<option value="${htmlEscape(r.recording_id)}">${htmlEscape(niceTitle(r.title) || r.recording_id)} · ${htmlEscape(r.channel_name)}</option>`,
    )
    .join("");

  root.innerHTML = chrome(`
    ${pluginHeader("Insights", "Aggregate signals across every transcribed recording.", "#/plugins")}
    <div class="cfg-grid">
      <section class="cfg-card">
        <h2 class="cfg-title">Top words</h2>
        <div class="pg-toolbar">
          <label class="pg-toggle"><input type="checkbox" id="ins-stopwords" ${insightsState.stopwords ? "checked" : ""}/> include stopwords</label>
          <a class="pg-linkbtn" href="/api/v1/plugins/insights/export?fmt=csv${insightsState.stopwords ? "&stopwords=true" : ""}">Export CSV</a>
          <a class="pg-linkbtn" href="/api/v1/plugins/insights/export?fmt=json${insightsState.stopwords ? "&stopwords=true" : ""}">JSON</a>
        </div>
        <div class="wf-list">${wordRows || '<div class="empty sm">No word data yet.</div>'}</div>
      </section>
      <section class="cfg-card">
        <h2 class="cfg-title">Topics</h2>
        <div class="pg-chips">${topicChips || '<div class="empty sm">No analyzed recordings yet.</div>'}</div>
      </section>
      <section class="cfg-card" id="ins-speakers-card">
        <h2 class="cfg-title">Speaker airtime</h2>
        <div id="ins-speakers"><div class="empty sm">Open a transcript and choose “View insights” to load speaker airtime.</div></div>
      </section>
      <section class="cfg-card" id="ins-compare-card">
        <h2 class="cfg-title">Compare two streams <span class="pg-cap-hint">word overlap · Jaccard · what's new vs gone</span></h2>
        <form id="ins-compare-form" class="mon-add">
          <select id="ins-compare-a">${recOptions}</select>
          <select id="ins-compare-b">${recOptions}</select>
          <button class="btn-primary" type="submit">Compare</button>
        </form>
        <div id="ins-compare-result"></div>
      </section>
    </div>
  `);
  setupChromeHandlers();
  const cb = document.getElementById("ins-stopwords");
  if (cb) {
    cb.addEventListener("change", () => {
      insightsState.stopwords = cb.checked;
      renderInsights();
    });
  }
  if (recId) await loadInsightsSpeakers(recId);

  // Comparison submit — POSTs nothing (idempotent GET).
  document.getElementById("ins-compare-form")?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const a = document.getElementById("ins-compare-a").value;
    const b = document.getElementById("ins-compare-b").value;
    if (!a || !b || a === b) {
      Toast.error("Pick two different recordings");
      return;
    }
    const host = document.getElementById("ins-compare-result");
    host.innerHTML = '<div class="empty sm">Comparing…</div>';
    try {
      const r = await API.insightsCompare(a, b);
      const c = r.comparison;
      const sharedRows = (c.shared || [])
        .slice(0, 30)
        .map(
          (s) =>
            `<tr><td>${htmlEscape(s.word)}</td><td>${s.count_a}</td><td>${s.count_b}</td><td>${isFinite(s.a_over_b) ? s.a_over_b.toFixed(2) : "∞"}</td></tr>`,
        )
        .join("");
      const onlyA = (c.only_a || []).slice(0, 20).map((w) => `<li>${htmlEscape(w.word)} <em>${w.count}</em></li>`).join("");
      const onlyB = (c.only_b || []).slice(0, 20).map((w) => `<li>${htmlEscape(w.word)} <em>${w.count}</em></li>`).join("");
      host.innerHTML = `
        <div class="ins-cmp-summary">
          <span class="cfg-badge">Jaccard ${(c.jaccard * 100).toFixed(0)}%</span>
          <span class="pg-cap-hint">${c.shared.length} shared · ${c.only_a.length} only-A · ${c.only_b.length} only-B</span>
        </div>
        <div class="ins-cmp-grid">
          <div class="ins-cmp-table-wrap">
            <h3 class="ins-cmp-h">Shared words</h3>
            <table class="ins-cmp-table">
              <thead><tr><th>word</th><th>A</th><th>B</th><th>A÷B</th></tr></thead>
              <tbody>${sharedRows || '<tr><td colspan="4" class="empty sm">No shared words</td></tr>'}</tbody>
            </table>
          </div>
          <div>
            <h3 class="ins-cmp-h">Only in A</h3>
            <ul class="ins-cmp-list">${onlyA || '<li class="empty sm">None</li>'}</ul>
          </div>
          <div>
            <h3 class="ins-cmp-h">Only in B</h3>
            <ul class="ins-cmp-list">${onlyB || '<li class="empty sm">None</li>'}</ul>
          </div>
        </div>`;
    } catch (err) {
      host.innerHTML = `<div class="empty sm">Compare failed: ${htmlEscape(err.message)}</div>`;
    }
  });
}

async function loadInsightsSpeakers(recId) {
  const host = document.getElementById("ins-speakers");
  if (!host) return;
  try {
    const r = await API.insightsSpeakers(recId);
    const speakers = (r && r.speakers) || [];
    const max = speakers.reduce((m, s) => Math.max(m, s.seconds), 0) || 1;
    host.innerHTML = speakers.length
      ? `${r.sentiment ? `<p class="page-subtitle">sentiment: <span class="cfg-badge sentiment-${htmlEscape(r.sentiment)}">${htmlEscape(r.sentiment)}</span></p>` : ""}
         ${speakers
           .map(
             (s) => `
        <div class="wf-row">
          <span class="wf-word">${htmlEscape(s.speaker)}</span>
          <span class="wf-bar"><span style="width:${Math.round((s.seconds / max) * 100)}%"></span></span>
          <span class="wf-count">${fmtClock(s.seconds)}</span>
        </div>`,
           )
           .join("")}`
      : '<div class="empty sm">No diarized speakers for this recording.</div>';
  } catch (e) {
    host.innerHTML = `<div class="empty sm">${htmlEscape(e.message)}</div>`;
  }
}

// ── Verb dispatch (actions over IPC) ─────────────────────────────────
async function dispatchVerb(plugin, verb, selection, btn) {
  if (btn) {
    btn.disabled = true;
    btn.dataset.prevLabel = btn.textContent;
    btn.textContent = "…";
  }
  try {
    await API.pluginRpc(plugin, verb, { selection });
    Toast.success(`${verb} queued in the daemon`);
  } catch (e) {
    Toast.error(`${verb} failed: ${e.message}`);
  } finally {
    if (btn) {
      btn.disabled = false;
      if (btn.dataset.prevLabel) btn.textContent = btn.dataset.prevLabel;
    }
  }
}

// mm:ss / h:mm:ss from a float-seconds value.
function fmtClock(sec) {
  const s = Math.max(0, Math.floor(sec || 0));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

// Parse a clock-shaped input ("1:30:00", "1:30", "90", "90.5s") into
// seconds. Falls back to the raw numeric value when the format is
// loose. Used by the EDL editor prompts.
function parseTimeInput(raw, max) {
  const s = String(raw || "").trim().replace(/s$/i, "");
  if (!s) return NaN;
  if (s.includes(":")) {
    const parts = s.split(":").map((x) => parseFloat(x));
    if (parts.some((p) => !isFinite(p))) return NaN;
    let n = 0;
    for (const p of parts) n = n * 60 + p;
    return Math.min(Math.max(n, 0), max ?? n);
  }
  const n = parseFloat(s);
  if (!isFinite(n)) return NaN;
  return Math.min(Math.max(n, 0), max ?? n);
}

// ── Recording info modal + in-app player ─────────────────────────────
//
// Two overlays — `#rec-info-modal` (stats + plugin quick-actions) and
// `#rec-player-modal` (custom mpv-style HTML5 player). Both close on
// Esc / backdrop click; opening one closes any other.

function ensureModalContainer(id) {
  let el = document.getElementById(id);
  if (!el) {
    el = document.createElement("div");
    el.id = id;
    el.className = "modal-overlay";
    el.setAttribute("role", "dialog");
    el.setAttribute("aria-modal", "true");
    document.body.appendChild(el);
  }
  return el;
}

function closeRecordingModals() {
  document.getElementById("rec-info-modal")?.remove();
  const pl = document.getElementById("rec-player-modal");
  if (pl) {
    const v = pl.querySelector("video");
    if (v) { v.pause(); v.removeAttribute("src"); v.load(); }
    pl.remove();
  }
  // Closing a recording modal should also tear down any lingering
  // keymap overlay — they were stacking + the kbd-help's ESC was being
  // eaten by the modal's ESC handler, leaving it stranded.
  document.getElementById("kbd-help")?.classList.remove("open");
  document.body.classList.remove("modal-open");
}

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (document.getElementById("rec-player-modal") || document.getElementById("rec-info-modal")) {
    closeRecordingModals();
    e.preventDefault();
  }
});

// Format bytes/sec into "1.2 Mbps" / "320 kbps" / "12 bps".
function fmtBitrate(bps) {
  if (!bps || bps <= 0) return "";
  if (bps >= 1_000_000) return `${(bps / 1_000_000).toFixed(1)} Mbps`;
  if (bps >= 1_000) return `${Math.round(bps / 1_000)} kbps`;
  return `${bps} bps`;
}
function fmtHz(hz) {
  if (!hz || hz <= 0) return "";
  if (hz >= 1_000) return `${(hz / 1_000).toFixed(hz % 1_000 === 0 ? 0 : 1)} kHz`;
  return `${hz} Hz`;
}

// Stream section for the Info modal: container + per-track summaries from
// ffprobe. Renders empty when the probe failed (ffprobe missing, file
// missing, codec parse failure) — the rest of the modal still shows.
function probeSectionHtml(p) {
  if (!p) {
    return `<section class="rec-info-stream rec-info-stream-missing">
      <h3>Stream</h3>
      <div class="empty sm">ffprobe unavailable or recording file missing.</div>
    </section>`;
  }
  const meta = (k, v) => v ? `<dt>${htmlEscape(k)}</dt><dd>${v}</dd>` : "";
  const headBits = [
    p.container && htmlEscape(p.container),
    fmtBitrate(p.bit_rate || 0),
  ].filter(Boolean).join(" · ");
  const vRows = (p.video || []).map((v) => {
    const bits = [
      v.codec && htmlEscape(v.codec),
      (v.width && v.height) ? `${v.width}×${v.height}` : null,
      v.fps ? `${(+v.fps).toFixed(v.fps % 1 === 0 ? 0 : 2)} fps` : null,
      fmtBitrate(v.bit_rate || 0),
      v.pix_fmt && htmlEscape(v.pix_fmt),
    ].filter(Boolean).join(" · ");
    return bits ? `<div class="rec-info-track">${bits}</div>` : "";
  }).join("");
  const aRows = (p.audio || []).map((a) => {
    const bits = [
      a.codec && htmlEscape(a.codec),
      a.channel_layout ? htmlEscape(a.channel_layout) : (a.channels ? `${a.channels} ch` : null),
      fmtHz(a.sample_rate || 0),
      fmtBitrate(a.bit_rate || 0),
      a.language && htmlEscape(a.language),
    ].filter(Boolean).join(" · ");
    return bits ? `<div class="rec-info-track">${bits}</div>` : "";
  }).join("");
  const sRows = (p.subtitle || []).map((s) => {
    const bits = [s.codec && htmlEscape(s.codec), s.language && htmlEscape(s.language)]
      .filter(Boolean).join(" · ");
    return bits ? `<div class="rec-info-track">${bits}</div>` : "";
  }).join("");
  return `
    <section class="rec-info-stream">
      <h3>Stream</h3>
      <dl class="rec-info-stats rec-info-stream-stats">
        ${meta("Container", headBits)}
        ${vRows ? meta(`Video${p.video.length > 1 ? ` ×${p.video.length}` : ""}`, vRows) : ""}
        ${aRows ? meta(`Audio${p.audio.length > 1 ? ` ×${p.audio.length}` : ""}`, aRows) : ""}
        ${sRows ? meta(`Subtitle${p.subtitle.length > 1 ? ` ×${p.subtitle.length}` : ""}`, sRows) : ""}
      </dl>
    </section>`;
}

// B7/B9: canonical recording-action dispatcher. Every recording
// click-target should funnel through openRecording(id, action) rather
// than calling openRecordingInfo / openRecordingPlayer / showRecordingPath
// directly. Adding a new action (or changing how an existing one
// behaves) means editing one registry entry, not chasing 6+ call sites.
const RECORDING_ACTIONS = {
  info:    (id) => openRecordingInfo(id),
  player:  (id, opts) => openRecordingPlayer(id, opts),
  locate:  (id) => {
    const rec = recCache.find((r) => r.id === id);
    if (rec && rec.output_path) showRecordingPath(rec.output_path);
  },
};
function openRecording(id, action = "info", opts) {
  if (!id) return;
  const fn = RECORDING_ACTIONS[action];
  if (!fn) { console.warn("openRecording: unknown action", action); return; }
  try { fn(id, opts || {}); } catch (e) { console.error(e); }
}

// `opts.seekSec`, when set, is the CE-Fusion F5 archive deep link: the
// caller (an Archive search hit / moment "Open in Editor" action) wants
// this recording's Info modal to open straight into the EDL editor with
// that timecode as the working target. There is no time-scrubbing video
// preview inside the EDL editor to seek programmatically, so "positioned
// at that time" means: the target is shown as a banner, the editor opens
// itself, and the first "Split at time…" prompt is pre-filled with it.
async function openRecordingInfo(jobId, opts = {}) {
  // B13: defensive close — a stray event listener that throws inside
  // closeRecordingModals shouldn't strand the next modal in a half-
  // built state.
  try { closeRecordingModals(); } catch (_) {}
  const overlay = ensureModalContainer("rec-info-modal");
  overlay.innerHTML = `<div class="modal-card rec-info-card"><div class="empty sm">Loading…</div></div>`;
  document.body.classList.add("modal-open");
  overlay.addEventListener("click", (e) => { if (e.target === overlay) closeRecordingModals(); });

  let rec, plugins, probe;
  try {
    // Probe is best-effort (ffprobe may not be installed, file may be
    // missing); a failure must not block the modal from rendering.
    [rec, plugins, probe] = await Promise.all([
      API.recordingOne(jobId),
      API.plugins().catch(() => ({ plugins: [] })),
      API.recordingProbe(jobId).catch(() => null),
    ]);
  } catch (e) {
    overlay.querySelector(".modal-card").innerHTML =
      `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`;
    return;
  }

  const state = stateLabel(rec.state);
  const stateClass = stateClassName(rec.state);
  const isFinished = stateClass === "finished";
  const meta = (k, v) => `<dt>${htmlEscape(k)}</dt><dd>${v}</dd>`;
  // Bullet-proof scope match: accept the canonical lowercase "recording",
  // the Rust-debug form "Item(Recording)", or any string whose lowercase
  // contains "recording". Keeps the SPA right whether the index handler
  // hardcodes the string or eventually serializes the live enum.
  const isRecordingScope = (s) => {
    if (!s) return false;
    const t = String(s).toLowerCase();
    return t === "recording" || t.includes("recording");
  };
  const recordingVerbs = ((plugins && plugins.plugins) || [])
    .flatMap((p) => (p.verbs || [])
      .filter((v) => isRecordingScope(v.scope))
      .map((v) => ({ ...v, plugin: p.name, available: p.available })))
    .filter((v) => v.available);
  // SPA-native action: if Crunchr is available, surface a transcript-view
  // link rather than a no-op IPC dispatch. (`Show transcript` on the
  // plugin returns TUI-only `ActivatePane` actions when handled headless,
  // so we'd otherwise just queue and visibly do nothing.)
  const crunchr = ((plugins && plugins.plugins) || []).find((p) => p.name === "crunchr");
  const showTranscriptHtml = crunchr && crunchr.available
    ? `<a class="sm rec-info-verb-link"
          href="#/plugins/crunchr/rec/${encodeURIComponent(jobId)}"
          data-action="rec-info-route-close">📜 Show transcript</a>`
    : "";
  const verbBtns = recordingVerbs.map((v) => `
      <button class="sm" data-action="rec-info-verb"
              data-plugin="${htmlEscape(v.plugin)}"
              data-verb="${htmlEscape(v.verb)}">
        ${htmlEscape(v.label || v.verb)}
      </button>`).join("");
  const actionsHtml = (verbBtns + showTranscriptHtml) ||
    `<div class="empty sm">No plugin actions available.</div>`;

  // The plugin-actions section is Creator Edition only — every control here
  // dispatches to a plugin route that the pure-PVR daemon does not mount.
  const creatorActionsHtml = CREATOR_ENABLED ? `
    <section class="rec-info-actions">
      <h3>Plugin actions</h3>
      <div class="rec-info-verbs">${actionsHtml}</div>
      ${isFinished ? `<button class="sm rec-info-cuepoints-btn" data-action="rec-info-cuepoints" title="Scene-change cuepoints (ffmpeg full pass)">⌶ Detect scene changes</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-clipper-btn" data-action="rec-info-clipper" title="Mine highlight candidates (uses cuepoints; runs ffmpeg pass if needed)">★ Find highlights</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-thumbs-btn" data-action="rec-info-thumbs" title="Sample candidate thumbnail frames at cuepoints / highlights">▥ Pick thumbnail</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-tracks-btn" data-action="rec-info-tracks" title="List audio tracks (OBS multi-track captures) + extract individual stems">♪ Audio tracks</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-reuse-btn" data-action="rec-info-reuse" title="Build cross-format publish drafts (YT long / Shorts / TikTok / Patreon / podcast / blog)">⇪ Publish drafts</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-casebook-btn" data-action="rec-info-casebook" title="Post-stream Casebook report (markdown briefing)">📓 Casebook</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-editor-btn" data-action="rec-info-editor" title="Open the EDL editor — cut, ripple-delete, render">✄ EDL editor</button>` : ""}
      <div class="rec-cuepoints" id="rec-cuepoints" hidden></div>
      <div class="rec-clipper" id="rec-clipper" hidden></div>
      <div class="rec-thumbs" id="rec-thumbs" hidden></div>
      <div class="rec-tracks" id="rec-tracks" hidden></div>
      <div class="rec-reuse" id="rec-reuse" hidden></div>
      <div class="rec-casebook" id="rec-casebook" hidden></div>
      <div class="rec-editor" id="rec-editor" hidden></div>
    </section>` : "";

  const targetTimeHtml = opts.seekSec != null
    ? `<span class="cfg-badge arc-target-time" title="Archive target — the split prompt below the EDL editor pre-fills with this time">🎯 target ${htmlEscape(fmtClock(opts.seekSec))}</span>`
    : "";
  overlay.querySelector(".modal-card").innerHTML = `
    <header class="rec-info-head">
      <span class="state-pill ${stateClass}">${htmlEscape(state)}</span>
      <h2>${htmlEscape(niceTitle(rec.stream_title) || "(no title)")}</h2>
      ${targetTimeHtml}
      <button class="modal-close" aria-label="Close" data-action="modal-close">✕</button>
    </header>
    <div class="rec-info-body">
      <div class="rec-info-thumb">${recThumb(rec)}</div>
      <dl class="rec-info-stats">
        ${meta("Channel", htmlEscape(rec.channel_name || ""))}
        ${meta("Platform", `<span class="plat-${htmlEscape((rec.platform || "").toLowerCase())}">${htmlEscape(rec.platform || "")}</span>`)}
        ${meta("Started", htmlEscape(rec.started_at ? new Date(rec.started_at).toLocaleString() : "—"))}
        ${meta("Duration", htmlEscape(rec.duration_secs ? fmtClock(rec.duration_secs) : "—"))}
        ${meta("Size", htmlEscape(formatBytes(rec.bytes_written || 0)))}
        ${meta("Transcode", rec.transcode ? "yes" : "no")}
        ${rec.source_url ? meta("Source", `<a href="${htmlEscape(rec.source_url)}" target="_blank" rel="noopener">${htmlEscape(rec.source_url)}</a>`) : ""}
        ${rec.output_path ? meta("File", `<span class="rec-info-pathwrap"><code class="rec-info-path">${htmlEscape(rec.output_path)}</code><button class="rec-copy" data-copy="${htmlEscape(rec.output_path)}" title="Copy path">⧉</button></span>`) : ""}
        ${rec.error ? meta("Error", `<span class="cfg-badge err">${htmlEscape(rec.error)}</span>`) : ""}
      </dl>
    </div>
    ${probeSectionHtml(probe)}
    ${creatorActionsHtml}
    <footer class="rec-info-foot">
      ${isFinished ? `<button class="primary" data-action="rec-info-play">▶ Open in player</button>` : ""}
      ${isFinished ? `<button class="sm" data-action="rec-info-remux" title="Remux to matroska + aac_adtstoasc so the in-browser player can decode it. Keeps the original as .orig.">⟳ Remux for browser</button>` : ""}
      <button class="danger" data-action="rec-info-delete">✕ Delete</button>
    </footer>`;

  overlay.querySelectorAll("[data-action=modal-close]").forEach((b) =>
    b.addEventListener("click", closeRecordingModals));
  overlay.querySelector("[data-action=rec-info-play]")?.addEventListener("click", () => {
    closeRecordingModals();
    if (jobId) window.location.hash = `#/watch?recording=${encodeURIComponent(jobId)}&fresh=1`;
  });
  overlay.querySelector("[data-action=rec-info-cuepoints]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Detecting…", async () => {
      const resp = await API.cuepointsGenerate(jobId);
      const host = document.getElementById("rec-cuepoints");
      if (!host) return;
      // Compute duration as a max-time + 5% pad so the timeline isn't
      // clipped if the last cuepoint is near the end of the file.
      const points = resp.points || [];
      const maxTime = points.length ? Math.max(...points.map((p) => p.time_sec)) : 0;
      const duration = Math.max(maxTime * 1.05, 60);
      if (!points.length) {
        host.innerHTML = `<div class="empty sm">No scene changes detected at threshold ${resp.threshold}.</div>`;
        host.hidden = false;
        return;
      }
      host.innerHTML = `
        <h4 class="rec-cp-title">${points.length} scene change${points.length === 1 ? "" : "s"} <span class="pg-cap-hint">${resp.cached ? "(cached)" : "(fresh extraction)"}</span></h4>
        <div class="rec-cp-strip" style="--rec-cp-dur:${duration}">
          ${points
            .map(
              (p) =>
                `<a class="rec-cp-tick" style="--rec-cp-pct:${((p.time_sec / duration) * 100).toFixed(2)}%" data-seek="${p.time_sec}" title="${fmtClock(p.time_sec)}" href="#"></a>`,
            )
            .join("")}
        </div>
        <div class="rec-cp-axis">
          <span>0:00</span>
          <span>${fmtClock(duration)}</span>
        </div>`;
      host.hidden = false;
      host.querySelectorAll(".rec-cp-tick").forEach((el) => {
        el.addEventListener("click", (e) => {
          e.preventDefault();
          closeRecordingModals();
          openRecordingPlayer(jobId, { seekTo: parseFloat(el.dataset.seek || "0") });
        });
      });
      Toast.success(`Detected ${points.length} scene change(s)`);
    }).catch((err) => Toast.error(`Cuepoints failed: ${err.message}`));
  });
  overlay.querySelector("[data-action=rec-info-clipper]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Mining…", async () => {
      const [analysis, existing] = await Promise.all([
        API.clipperAnalyze(jobId),
        API.clipperListClips(jobId).catch(() => ({ clips: [] })),
      ]);
      const host = document.getElementById("rec-clipper");
      if (!host) return;
      host.hidden = false;
      const highlights = analysis.highlights || [];
      const cutByStart = new Map(
        (existing.clips || []).map((c) => [Math.round(c.start_sec), c]),
      );
      if (!highlights.length) {
        host.innerHTML = `<div class="empty sm">No highlight candidates found (transcript or cuepoints empty?).</div>`;
        return;
      }
      host.innerHTML = `
        <h4 class="rec-cp-title">${highlights.length} highlight candidate${highlights.length === 1 ? "" : "s"} <span class="pg-cap-hint">window ${analysis.window_secs}s</span></h4>
        <div class="rec-hl-list">
          ${highlights
            .map((h, i) => {
              const cut = cutByStart.get(Math.round(h.time_sec));
              return `<div class="rec-hl-row" data-i="${i}">
                <button class="rec-hl-jump" data-seek="${h.time_sec}" title="Jump to ${fmtClock(h.time_sec)}">${fmtClock(h.time_sec)}</button>
                <span class="rec-hl-score" title="Score ${h.score.toFixed(2)} · density ${h.density}">
                  <span class="rec-hl-bar" style="--rec-hl-pct:${(h.score * 100).toFixed(0)}%"></span>
                  <span>${Math.round(h.score * 100)}%</span>
                </span>
                <span class="rec-hl-meta">${h.density} cuepoint${h.density === 1 ? "" : "s"} · ${h.suggested_duration}s</span>
                ${cut
                  ? `<span class="cfg-badge ok" title="${htmlEscape(cut.clip_path)}">✓ cut · ${formatBytes(cut.bytes)}</span>`
                  : `<button class="sm rec-hl-cut" data-start="${h.time_sec}" data-dur="${h.suggested_duration}">Cut clip</button>`}
              </div>`;
            })
            .join("")}
        </div>`;
      host.querySelectorAll(".rec-hl-jump").forEach((el) => {
        el.addEventListener("click", (e) => {
          e.preventDefault();
          closeRecordingModals();
          openRecordingPlayer(jobId, { seekTo: parseFloat(el.dataset.seek || "0") });
        });
      });
      host.querySelectorAll(".rec-hl-cut").forEach((btn) => {
        btn.addEventListener("click", async (e) => {
          const cb = e.currentTarget;
          await withBusy(cb, "Cutting…", async () => {
            const res = await API.clipperExtract(jobId, {
              start_sec: parseFloat(cb.dataset.start),
              duration_sec: parseFloat(cb.dataset.dur),
              stem: `${niceTitle(rec.stream_title).replace(/[^a-zA-Z0-9_-]+/g, "_").slice(0, 60)}_${Math.round(parseFloat(cb.dataset.start))}`,
            });
            Toast.success(`Cut ${formatBytes(res.bytes)} → ${res.clip_path}`);
            cb.outerHTML = `<span class="cfg-badge ok" title="${htmlEscape(res.clip_path)}">✓ cut · ${formatBytes(res.bytes)}</span>`;
          }).catch((err) => Toast.error(`Cut failed: ${err.message}`));
        });
      });
      Toast.success(`Found ${highlights.length} highlight candidate(s)`);
    }).catch((err) => Toast.error(`Highlights failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-thumbs]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Sampling…", async () => {
      const resp = await API.thumbnailsGenerate(jobId, {
        source: "cuepoints",
        facecam: "top_right",
      });
      const host = document.getElementById("rec-thumbs");
      if (!host) return;
      host.hidden = false;
      const candidates = resp.candidates || [];
      if (!candidates.length) {
        host.innerHTML = '<div class="empty sm">No thumbnail candidates generated.</div>';
        return;
      }
      host.innerHTML = `
        <h4 class="rec-cp-title">${candidates.length} thumbnail candidate${candidates.length === 1 ? "" : "s"} <span class="pg-cap-hint">ranked by saliency</span></h4>
        <div class="rec-thumbs-grid">
          ${candidates
            .map(
              (c, i) =>
                `<figure class="rec-thumb-card" data-i="${i}">
                  <a class="rec-thumb-img" href="${htmlEscape(API.thumbnailFileUrl(c.path))}" target="_blank" rel="noopener">
                    <img loading="lazy" alt="" src="${htmlEscape(API.thumbnailFileUrl(c.path))}" />
                    <span class="rec-thumb-time">${fmtClock(c.time_sec)}</span>
                  </a>
                  <figcaption>
                    <span class="rec-thumb-score" title="Score ${c.score.toFixed(2)} · ${formatBytes(c.bytes)}">
                      <span class="rec-hl-bar" style="--rec-hl-pct:${(c.score * 100).toFixed(0)}%"></span>
                      <span>${Math.round(c.score * 100)}%</span>
                    </span>
                    ${c.crop_path ? `<a class="pg-linkbtn" href="${htmlEscape(API.thumbnailFileUrl(c.crop_path))}" target="_blank" rel="noopener" title="9:16 facecam crop">9:16 crop</a>` : ""}
                  </figcaption>
                </figure>`,
            )
            .join("")}
        </div>`;
      Toast.success(`Generated ${candidates.length} thumbnail candidate(s)`);
    }).catch((err) => Toast.error(`Thumbnails failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-editor]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Loading EDL…", async () => {
      const host = document.getElementById("rec-editor");
      if (!host) return;
      let { edl, total_duration } = await API.editorLoad(jobId);
      host.hidden = false;
      // Beat-grid state lives outside paint() so it survives EDL
      // re-renders. When loaded, the strip is painted into the
      // .rec-beatgrid placeholder and Split at time… snaps to the
      // nearest beat within ±60 ms.
      let beatGridState = null; // { tempo_bpm, beats: number[], window_sec }
      // Loudness gauge per-session cache, keyed by job. The full
      // Loudness panel writes here on measure too so the topbar pill
      // and the panel stay in sync without a refetch.
      const loudnessGauge = (window.__strivoLoudnessGauge ||= new Map());
      // CE-Fusion F5: an Archive deep link's target time, consumed once —
      // the first "Split at time…" prompt pre-fills with it, then it's
      // cleared so later splits don't keep jumping back to it.
      let pendingSeekSec = opts.seekSec ?? null;

      const paint = () => {
        const dur = edl.cuts.reduce((a, c) => a + Math.max(0, c.end_sec - c.start_sec), 0);
        const sourceDur = total_duration || dur || 1;
        host.innerHTML = `
          <h4 class="rec-cp-title">EDL editor <span class="pg-cap-hint">${edl.cuts.length} cut${edl.cuts.length === 1 ? "" : "s"} · output ${fmtClock(dur)}</span></h4>
          <div class="rec-ed-actions">
            <button class="sm rec-ed-add-split" type="button">Split at time…</button>
            <button class="sm rec-ed-delete" type="button">Ripple-delete range…</button>
            <button class="sm rec-ed-deadair" type="button" title="Detect dead air (silencedetect) and trim spans longer than 6s">▢ Trim dead air…</button>
            <button class="sm rec-ed-vad" type="button" title="DAW-style voice gate — hysteresis VAD finds speech runs and ripple-deletes the natural breath gaps">▢ Voice gate…</button>
            <button class="sm rec-ed-sidechain" type="button" title="DAW sidechain — VAD voice intervals → ducking automation curve baked at render. Composes VAD + sidechain + automation in one click.">🦆 Sidechain duck…</button>
            <button class="sm rec-ed-insertfx" type="button" title="DAW insert chain — ordered HP/NR/de-esser/comp/limiter etc. Voice + game bus presets, edits persist as a single ffmpeg -af baked at render.">🎛 Insert FX…</button>
            <button class="sm rec-ed-pitch" type="button" title="Pitch / time-stretch — fit the recording to a target slot length without changing voices' pitch, or transpose a stinger without changing tempo. Wraps ffmpeg rubberband.">🎚 Pitch/time…</button>
            <button class="sm rec-ed-branding" type="button" title="Watermark + intro/outro banner overlay applied at render">★ Branding…</button>
            <button class="sm rec-ed-loudness" type="button" title="EBU R128 loudness check + per-platform normalisation target">♪ Loudness…</button>
            <button class="sm rec-ed-beatgrid" type="button" title="Beat grid — onset-detect a tempo, paint vertical guides on the EDL strip. While the grid is loaded, Split at time… snaps to the nearest beat.">🎼 Beat grid…</button>
            <button class="sm rec-ed-history" type="button" title="Revision history — revert across saves (DAW-style undo)">↺ History…</button>
            <button class="sm rec-ed-scenes" type="button" title="Scenes — Ableton-style session save/recall bundling EDL + branding + automation + loudness + captions style">🎬 Scenes…</button>
            <button class="sm rec-ed-autosnap" type="button" title="Auto-snapshot before every save — stashes a scene named 'auto-pre-save · <timestamp>' before each persist, giving you a one-click pre-edit recovery point. Setting persists across reloads.">📸 Auto-snap: <span class="rec-ed-as-state">off</span></button>
            <button class="sm rec-ed-loudgauge" type="button" title="Loudness gauge — measure EBU R128 I / TP / LRA against the YouTube target. Click to measure or refresh; reads from a per-session cache otherwise.">♪ <span class="rec-ed-lg-val">Measure</span></button>
            <button class="btn-primary rec-ed-render" type="button">⚡ Render to MKV</button>
          </div>
          <div class="rec-beatgrid" hidden></div>
          <div class="rec-branding" hidden></div>
          <div class="rec-loudness" hidden></div>
          <div class="rec-history" hidden></div>
          <div class="rec-scenes" hidden></div>
          <div class="rec-ed-list">
            ${edl.cuts
              .map(
                (c, i) => `
              <div class="rec-ed-row" data-i="${i}">
                <span class="rec-ed-idx">${i + 1}</span>
                <span class="rec-ed-kind ${htmlEscape(c.kind.kind || "source")}">${htmlEscape(c.kind.kind || "source")}</span>
                <span class="rec-ed-src">${htmlEscape((c.kind.source_path || c.kind.broll_path || "").split("/").slice(-1)[0])}</span>
                <span class="rec-ed-time">${fmtClock(c.start_sec)} → ${fmtClock(c.end_sec)} · ${fmtClock(c.end_sec - c.start_sec)}</span>
                <button class="sm rec-ed-trim" data-i="${i}" type="button" title="Trim this cut">trim</button>
                <button class="sm danger rec-ed-rm" data-i="${i}" type="button" title="Remove this cut">✕</button>
              </div>`,
              )
              .join("")}
          </div>
          <p class="pg-cap-hint">All edits are non-destructive — original recording stays intact. Render writes &lt;recording_parent&gt;/edl/&lt;id&gt;.mkv.</p>`;

        // 'Capture before' — opt-in scene auto-snapshot on every save.
        // When enabled, persist() stashes the *current* edl as a scene
        // tagged "auto-pre-save · <ISO>" before applying the new edit,
        // giving the user a one-click pre-edit recovery point.
        // Preference lives in localStorage so it survives reloads.
        const autoSnapKey = "strivo-editor-auto-snap";
        const isAutoSnapOn = () => localStorage.getItem(autoSnapKey) === "1";
        const persist = async (label) => {
          try {
            if (isAutoSnapOn()) {
              // Fire-and-forget; scene capture failure shouldn't block
              // the save the user actually asked for.
              const stamp = new Date().toISOString().replace(/[:.]/g, "-");
              API.scenesCapture(jobId, `auto-pre-save · ${stamp}`, null).catch(() => {});
            }
            await API.editorSave(jobId, edl, label);
          } catch (err) {
            Toast.error(`Save failed: ${err.message}`);
          }
        };

        // ── Beat-grid strip ───────────────────────────────────────
        // Snaps an output-timeline time to the nearest beat within
        // ±60ms. Returns the same time when no grid is loaded so
        // every caller can opt in unconditionally.
        const snapToBeat = (t) => {
          if (!beatGridState || !beatGridState.beats || !beatGridState.beats.length) return t;
          const beats = beatGridState.beats;
          // Binary search for nearest.
          let lo = 0, hi = beats.length - 1;
          while (lo < hi) {
            const mid = (lo + hi) >> 1;
            if (beats[mid] < t) lo = mid + 1; else hi = mid;
          }
          const candidates = [beats[lo]];
          if (lo > 0) candidates.push(beats[lo - 1]);
          let best = t, bestDelta = Infinity;
          for (const b of candidates) {
            const d = Math.abs(b - t);
            if (d < bestDelta) { bestDelta = d; best = b; }
          }
          return bestDelta < 0.06 ? best : t;
        };

        const paintBeatStrip = () => {
          const strip = host.querySelector(".rec-beatgrid");
          if (!strip) return;
          if (!beatGridState) { strip.hidden = true; strip.innerHTML = ""; return; }
          const { tempo_bpm, beats } = beatGridState;
          // Limit visible ticks to keep DOM cheap; thin to ~400 evenly.
          const stride = Math.max(1, Math.ceil(beats.length / 400));
          const sampled = beats.filter((_, i) => i % stride === 0);
          strip.hidden = false;
          strip.innerHTML = `
            <div class="rec-bg-head">
              <span>🎼 Tempo grid</span>
              <span class="pg-cap-hint">${tempo_bpm.toFixed(1)} BPM · ${beats.length} beat(s) over ${fmtClock(sourceDur)} · Split snaps within ±60 ms</span>
              <button class="sm rec-bg-clear" type="button" title="Hide the grid and disable snap">✕</button>
            </div>
            <div class="rec-bg-strip">${sampled.map((t) => {
              const pct = sourceDur > 0 ? (t / sourceDur) * 100 : 0;
              return `<span class="rec-bg-tick" style="left:${pct.toFixed(3)}%" title="${fmtClock(t)}"></span>`;
            }).join("")}</div>`;
          strip.querySelector(".rec-bg-clear")?.addEventListener("click", () => {
            beatGridState = null;
            paintBeatStrip();
          });
        };
        paintBeatStrip();

        // ── Loudness gauge ────────────────────────────────────────
        // Inline I / TP / LRA pill next to ⚡ Render. Reads cached
        // measurement when present; click re-measures.
        const paintLoudGauge = () => {
          const val = host.querySelector(".rec-ed-lg-val");
          const btn = host.querySelector(".rec-ed-loudgauge");
          if (!val || !btn) return;
          const c = loudnessGauge.get(jobId);
          if (!c) { val.textContent = "Measure"; btn.classList.remove("ok", "over", "under"); return; }
          val.innerHTML = `I ${c.i.toFixed(1)} <span class="pg-cap-hint">LUFS</span> · TP ${c.tp.toFixed(1)} · LRA ${c.lra.toFixed(1)}`;
          // Colour by integrated delta vs YouTube target (-14 LUFS).
          // ±1 LUFS = ok, > +1 LUFS over = clipping risk, < -1 under = quiet.
          btn.classList.remove("ok", "over", "under");
          const d = c.i_delta;
          if (Math.abs(d) <= 1.0) btn.classList.add("ok");
          else if (d > 0) btn.classList.add("over");
          else btn.classList.add("under");
          btn.title = `EBU R128 vs ${c.platform || "youtube"} target · I Δ ${d >= 0 ? "+" : ""}${d.toFixed(2)} LUFS. Click to re-measure.`;
        };
        paintLoudGauge();

        // Wire the auto-snap toggle (declared above the persist hook).
        const paintAutoSnap = () => {
          const stateEl = host.querySelector(".rec-ed-as-state");
          const btn = host.querySelector(".rec-ed-autosnap");
          if (!stateEl || !btn) return;
          const on = isAutoSnapOn();
          stateEl.textContent = on ? "on" : "off";
          btn.classList.toggle("active", on);
        };
        paintAutoSnap();
        host.querySelector(".rec-ed-autosnap")?.addEventListener("click", () => {
          const next = isAutoSnapOn() ? "0" : "1";
          localStorage.setItem(autoSnapKey, next);
          paintAutoSnap();
          Toast.success(next === "1"
            ? "Auto-snap on · next save will stash a pre-edit scene first"
            : "Auto-snap off");
        });

        host.querySelector(".rec-ed-loudgauge")?.addEventListener("click", async (e2) => {
          const gbtn = e2.currentTarget;
          await withBusy(gbtn, "Measuring…", async () => {
            const platform = "youtube";
            const r = await API.loudnessMeasure(jobId, platform);
            loudnessGauge.set(jobId, {
              i: r.measurement.input_i,
              tp: r.measurement.input_tp,
              lra: r.measurement.input_lra,
              i_delta: r.delta.i_delta,
              tp_delta: r.delta.tp_delta,
              lra_delta: r.delta.lra_delta,
              platform: r.platform,
              measured_at: Date.now(),
            });
            paintLoudGauge();
            Toast.success(`Loudness · I ${r.measurement.input_i.toFixed(2)} LUFS (Δ ${r.delta.i_delta >= 0 ? "+" : ""}${r.delta.i_delta.toFixed(2)})`);
          }).catch((err) => Toast.error(`Loudness failed: ${err.message}`));
        });

        host.querySelector(".rec-ed-beatgrid")?.addEventListener("click", async (e2) => {
          const bbtn = e2.currentTarget;
          await withBusy(bbtn, "Detecting…", async () => {
            const window_sec = Math.max(60, Math.min(3600, Math.round(sourceDur || 600)));
            const r = await API.beatDetectRun(jobId, { window_sec });
            const top = (r.tempo_candidates || [])[0];
            const beats = r.tempo_grid_secs || [];
            if (!top || !beats.length) {
              Toast.error("Beat detect returned no tempo — try a longer window or a different recording");
              return;
            }
            beatGridState = { tempo_bpm: top.bpm, beats, window_sec };
            paintBeatStrip();
            Toast.success(`Tempo locked · ${top.bpm.toFixed(1)} BPM · ${beats.length} beats. Split at time… now snaps.`);
          }).catch((err) => Toast.error(`Beat grid failed: ${err.message}`));
        });

        host.querySelector(".rec-ed-add-split")?.addEventListener("click", async () => {
          const snapHint = beatGridState ? "\n(Beat grid loaded — input will snap to the nearest beat within ±60ms.)" : "";
          const s = prompt(
            `Split at output time (HH:MM:SS or seconds):${snapHint}`,
            pendingSeekSec != null ? fmtClock(pendingSeekSec) : undefined,
          );
          pendingSeekSec = null; // consumed — later splits get a blank prompt
          if (!s) return;
          let t = parseTimeInput(s, sourceDur);
          if (!isFinite(t)) {
            Toast.error("Could not parse time");
            return;
          }
          // Beat-grid snap. No-op when the grid hasn't been detected.
          const snapped = snapToBeat(t);
          if (Math.abs(snapped - t) > 1e-6) {
            Toast.success(`Snapped to beat at ${fmtClock(snapped)}`);
            t = snapped;
          }
          // Local split — same algorithm as server. Walk and split.
          let elapsed = 0;
          for (let i = 0; i < edl.cuts.length; i++) {
            const c = edl.cuts[i];
            const cd = c.end_sec - c.start_sec;
            const out_hi = elapsed + cd;
            if (t > elapsed + 0.001 && t < out_hi - 0.001) {
              const offset = t - elapsed;
              const right = structuredClone(c);
              const newSplit = c.start_sec + offset;
              right.start_sec = newSplit;
              c.end_sec = newSplit;
              edl.cuts.splice(i + 1, 0, right);
              break;
            }
            elapsed = out_hi;
          }
          await persist("split");
          paint();
          Toast.success("Split");
        });
        host.querySelector(".rec-ed-delete")?.addEventListener("click", async () => {
          const range = prompt("Range to delete (e.g. 1:30-2:45 or 90-165):");
          if (!range) return;
          const m = range.match(/(.+?)\s*[-–]\s*(.+)/);
          if (!m) { Toast.error("Use lo-hi format"); return; }
          const lo = parseTimeInput(m[1], sourceDur);
          const hi = parseTimeInput(m[2], sourceDur);
          if (!isFinite(lo) || !isFinite(hi) || hi <= lo) {
            Toast.error("Invalid range");
            return;
          }
          // Mirror server-side delete_range — walk and trim.
          let elapsed = 0;
          const next = [];
          for (const cut of edl.cuts) {
            const cd = cut.end_sec - cut.start_sec;
            const out_lo = elapsed;
            const out_hi = elapsed + cd;
            elapsed = out_hi;
            if (out_hi <= lo || out_lo >= hi) { next.push(cut); continue; }
            if (out_lo >= lo && out_hi <= hi) { continue; }
            if (out_lo < lo && out_hi <= hi) {
              const trim = structuredClone(cut);
              trim.end_sec = cut.start_sec + (lo - out_lo);
              next.push(trim);
              continue;
            }
            if (out_lo >= lo && out_hi > hi) {
              const trim = structuredClone(cut);
              trim.start_sec = cut.start_sec + (hi - out_lo);
              next.push(trim);
              continue;
            }
            const left = structuredClone(cut);
            left.end_sec = cut.start_sec + (lo - out_lo);
            const right = structuredClone(cut);
            right.start_sec = cut.start_sec + (hi - out_lo);
            next.push(left, right);
          }
          edl.cuts = next.filter((c) => c.end_sec - c.start_sec > 0.001);
          await persist("ripple-delete");
          paint();
          Toast.success("Range deleted");
        });
        host.querySelectorAll(".rec-ed-rm").forEach((b) => {
          b.addEventListener("click", async () => {
            const i = +b.dataset.i;
            edl.cuts.splice(i, 1);
            await persist("remove cut");
            paint();
          });
        });
        host.querySelectorAll(".rec-ed-trim").forEach((b) => {
          b.addEventListener("click", async () => {
            const i = +b.dataset.i;
            const c = edl.cuts[i];
            const s = prompt(`Trim cut ${i + 1} — start..end (seconds or HH:MM:SS), e.g. "10-60"`, `${c.start_sec}-${c.end_sec}`);
            if (!s) return;
            const m = s.match(/(.+?)\s*[-–]\s*(.+)/);
            if (!m) { Toast.error("Use start-end format"); return; }
            const lo = parseTimeInput(m[1], sourceDur);
            const hi = parseTimeInput(m[2], sourceDur);
            if (!isFinite(lo) || !isFinite(hi) || hi <= lo) { Toast.error("Invalid"); return; }
            c.start_sec = lo;
            c.end_sec = hi;
            await persist("trim cut");
            paint();
          });
        });
        host.querySelector(".rec-ed-deadair")?.addEventListener("click", async (e2) => {
          const dabtn = e2.currentTarget;
          await withBusy(dabtn, "Scanning silence…", async () => {
            const r = await API.deadairDetect(jobId);
            const cuts = (r.result && r.result.recommended_cuts) || [];
            const totalTrim = (r.result && r.result.total_trim_secs) || 0;
            if (!cuts.length) {
              Toast.success(`No dead-air spans above the trim threshold detected.`);
              return;
            }
            if (!confirm(`Found ${cuts.length} dead-air span(s) totalling ${fmtClock(totalTrim)}.\n\nApply all as ripple-deletes? Edits are non-destructive — only the EDL changes.`)) return;
            // Apply each cut in DESCENDING order so prior deletes
            // don't shift later coordinates. The cut times are in
            // source-file coordinates, but our EDL initially mirrors
            // the source 1:1, so for the first apply they're
            // equivalent. After the first cut everything shifts; we
            // re-fetch the EDL before each subsequent cut to stay
            // honest about output-time coords.
            const sorted = [...cuts].sort((a, b) => b.start_sec - a.start_sec);
            for (const cut of sorted) {
              const lo = cut.start_sec;
              const hi = cut.end_sec;
              let elapsed = 0;
              const next = [];
              for (const c of edl.cuts) {
                const cd = c.end_sec - c.start_sec;
                const out_lo = elapsed;
                const out_hi = elapsed + cd;
                elapsed = out_hi;
                if (out_hi <= lo || out_lo >= hi) { next.push(c); continue; }
                if (out_lo >= lo && out_hi <= hi) { continue; }
                if (out_lo < lo && out_hi <= hi) {
                  const trim = structuredClone(c);
                  trim.end_sec = c.start_sec + (lo - out_lo);
                  next.push(trim);
                  continue;
                }
                if (out_lo >= lo && out_hi > hi) {
                  const trim = structuredClone(c);
                  trim.start_sec = c.start_sec + (hi - out_lo);
                  next.push(trim);
                  continue;
                }
                const left = structuredClone(c);
                left.end_sec = c.start_sec + (lo - out_lo);
                const right = structuredClone(c);
                right.start_sec = c.start_sec + (hi - out_lo);
                next.push(left, right);
              }
              edl.cuts = next.filter((c) => c.end_sec - c.start_sec > 0.001);
            }
            await persist("trim dead air");
            paint();
            Toast.success(`Trimmed ${cuts.length} dead-air span(s) · saved ${fmtClock(totalTrim)}.`);
          }).catch((err) => Toast.error(`Dead-air scan failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-vad")?.addEventListener("click", async (e2) => {
          const vbtn = e2.currentTarget;
          const promptMin = prompt(
            "Minimum pause to KEEP between speech (sec) — gaps below this become ripple-deletes.\n" +
              "Default 1.0; lower = tighter; try 0.3 for podcast pacing.",
            "1.0",
          );
          if (promptMin == null) return;
          const minKeep = parseFloat(promptMin);
          if (!isFinite(minKeep) || minKeep < 0) { Toast.error("Invalid min_keep value"); return; }
          await withBusy(vbtn, "Scanning voice…", async () => {
            // Cap the window at 1h so a 4h archive doesn't melt the
            // host; the editor only cares about the section the user is
            // currently working on.
            const r = await API.vadAnalyze(jobId, { min_keep_sec: minKeep, window_sec: 3600 });
            const gaps = r.recommended_gaps || [];
            const savings = r.total_savings_sec || 0;
            const intervalCount = (r.voice_intervals || []).length;
            if (!gaps.length) {
              Toast.success(`Found ${intervalCount} voice run(s); no gaps above the ${minKeep}s keep threshold to tighten.`);
              return;
            }
            if (!confirm(
              `Voice gate found ${intervalCount} voice run(s) and ${gaps.length} ripple-delete candidate(s) ` +
                `(${fmtClock(savings)} of natural silence to remove).\n\n` +
                `Apply all? Edits are non-destructive — only the EDL changes.`,
            )) return;
            // Same descending-order ripple-delete loop the dead-air
            // path uses — keeps coordinate drift honest.
            const sorted = [...gaps].sort((a, b) => b.start_sec - a.start_sec);
            for (const gap of sorted) {
              const lo = gap.start_sec;
              const hi = gap.end_sec;
              let elapsed = 0;
              const next = [];
              for (const c of edl.cuts) {
                const cd = c.end_sec - c.start_sec;
                const out_lo = elapsed;
                const out_hi = elapsed + cd;
                elapsed = out_hi;
                if (out_hi <= lo || out_lo >= hi) { next.push(c); continue; }
                if (out_lo >= lo && out_hi <= hi) { continue; }
                if (out_lo < lo && out_hi <= hi) {
                  const trim = structuredClone(c);
                  trim.end_sec = c.start_sec + (lo - out_lo);
                  next.push(trim);
                  continue;
                }
                if (out_lo >= lo && out_hi > hi) {
                  const trim = structuredClone(c);
                  trim.start_sec = c.start_sec + (hi - out_lo);
                  next.push(trim);
                  continue;
                }
                const left = structuredClone(c);
                left.end_sec = c.start_sec + (lo - out_lo);
                const right = structuredClone(c);
                right.start_sec = c.start_sec + (hi - out_lo);
                next.push(left, right);
              }
              edl.cuts = next.filter((c) => c.end_sec - c.start_sec > 0.001);
            }
            await persist("voice gate");
            paint();
            Toast.success(`Voice gate · trimmed ${gaps.length} gap(s) · saved ${fmtClock(savings)}.`);
          }).catch((err) => Toast.error(`Voice-gate scan failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-sidechain")?.addEventListener("click", async (e2) => {
          // One-click sidechain compressor: VAD → sidechain → automation.
          // Demonstrates the iter-46 + iter-50 + iter-41 plugin chain
          // composing in a single user gesture. The result is persisted
          // to the volume-automation store so the next ⚡ Render bakes
          // the ducking curve via the existing asendcmd pipeline.
          const sbtn = e2.currentTarget;
          const promptDuck = prompt(
            "Sidechain ducking — how many dB to drop the audio bus while voice is active?\n" +
              "Default -12 dB (podcast-natural). Try -6 dB for a gentler duck or -20 dB for voice-over.",
            "-12",
          );
          if (promptDuck == null) return;
          const duckDb = parseFloat(promptDuck);
          if (!isFinite(duckDb) || duckDb >= 0) { Toast.error("Duck depth must be < 0 dB"); return; }
          await withBusy(sbtn, "Building VAD…", async () => {
            const vad = await API.vadAnalyze(jobId, { window_sec: 3600 });
            const intervals = vad.voice_intervals || [];
            const envelopeDur = (vad.voice_intervals?.length
              ? Math.max(vad.envelope_frames * 0.05, intervals[intervals.length - 1].end_sec + 1)
              : (total_duration || dur || 0));
            if (!intervals.length) {
              Toast.success("VAD found no voice activity — nothing to duck. Try lowering open_db or raising window_sec.");
              return;
            }
            sbtn.textContent = "Sidechain…";
            const sc = await API.sidechainBuild(jobId, {
              voice_intervals: intervals,
              total_duration_sec: envelopeDur,
              knobs: { duck_db: duckDb, attack_sec: 0.05, release_sec: 0.3, hold_sec: 0.1, step_sec: 0.05 },
              persist: true,
            });
            if (!sc.persisted_to_automation_store) {
              Toast.error("Sidechain built but persistence failed — check daemon logs");
              return;
            }
            Toast.success(
              `Sidechain ready · ${intervals.length} voice run(s) → ${sc.point_count} automation point(s) ducking to ${duckDb} dB. Hit ⚡ Render to bake.`,
            );
          }).catch((err) => Toast.error(`Sidechain failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-insertfx")?.addEventListener("click", async (e2) => {
          // DAW insert chain panel. Shows current stages, lets the user
          // install a voice/game preset in one click, drop individual
          // stages, then save. Each stage is one named effect mapping to
          // a single ffmpeg filter; chain composes left-to-right and
          // bakes at render via the existing -af path.
          const ibtn = e2.currentTarget;
          await withBusy(ibtn, "Loading FX…", async () => {
            const r = await API.insertFxLoad(jobId);
            const panel = host.querySelector(".rec-insertfx") || (() => {
              const d = document.createElement("div");
              d.className = "rec-insertfx";
              host.appendChild(d);
              return d;
            })();
            let chain = r.chain || { effects: [] };
            const renderStages = () => (chain.effects || []).map((eff, i) => {
              const params = Object.entries(eff)
                .filter(([k]) => k !== "kind")
                .map(([k, v]) => `${k}=${typeof v === "number" ? v : htmlEscape(String(v))}`)
                .join(" · ");
              return `<div class="rec-ifx-stage" data-i="${i}">
                <span class="rec-ifx-num">${i + 1}</span>
                <span class="rec-ifx-kind">${htmlEscape(eff.kind || "?")}</span>
                <span class="rec-ifx-params pg-cap-hint">${params}</span>
                <button class="sm danger rec-ifx-rm" type="button" title="Remove stage">✕</button>
              </div>`;
            }).join("");
            const paintFx = () => {
              panel.hidden = false;
              panel.innerHTML = `
                <h5>Insert FX chain <span class="pg-cap-hint">${(chain.effects||[]).length} stage(s)</span></h5>
                <div class="rec-ifx-presets">
                  <button class="sm rec-ifx-voice" type="button" title="HP@80 → NR → de-esser → 3:1 comp → limiter — single-mic talk-stream voice bus">🎤 Voice preset</button>
                  <button class="sm rec-ifx-game" type="button" title="HP@40 → 2:1 comp → limiter — game/music bus that won't squash dialogue">🎮 Game preset</button>
                  <button class="sm danger rec-ifx-clear" type="button" title="Empty the chain">Clear</button>
                </div>
                <div class="rec-ifx-stages">${renderStages() || '<div class="pg-cap-hint">No stages — pick a preset above or wire stages via API.</div>'}</div>
                <div class="rec-ifx-actions">
                  <button class="btn-primary rec-ifx-save" type="button">Save chain</button>
                  <span class="pg-cap-hint">Saved chains bake into one ffmpeg <code>-af</code> at render.</span>
                </div>
                <pre class="rec-ifx-filter" title="ffmpeg -af value this chain produces">${htmlEscape(r.audio_filter || chain_to_filter_preview(chain))}</pre>
              `;
              panel.querySelector(".rec-ifx-voice").addEventListener("click", async (ev) => {
                await withBusy(ev.currentTarget, "Installing voice…", async () => {
                  const res = await API.insertFxPreset(jobId, "voice");
                  chain = res.chain;
                  r.audio_filter = res.audio_filter;
                  paintFx();
                  Toast.success(`Voice bus preset installed · ${chain.effects.length} stages`);
                });
              });
              panel.querySelector(".rec-ifx-game").addEventListener("click", async (ev) => {
                await withBusy(ev.currentTarget, "Installing game…", async () => {
                  const res = await API.insertFxPreset(jobId, "game");
                  chain = res.chain;
                  r.audio_filter = res.audio_filter;
                  paintFx();
                  Toast.success(`Game bus preset installed · ${chain.effects.length} stages`);
                });
              });
              panel.querySelector(".rec-ifx-clear").addEventListener("click", () => {
                chain = { effects: [] };
                paintFx();
              });
              panel.querySelectorAll(".rec-ifx-rm").forEach((b) => {
                b.addEventListener("click", () => {
                  const idx = parseInt(b.closest(".rec-ifx-stage").dataset.i, 10);
                  chain.effects.splice(idx, 1);
                  paintFx();
                });
              });
              panel.querySelector(".rec-ifx-save").addEventListener("click", async (ev) => {
                await withBusy(ev.currentTarget, "Saving…", async () => {
                  const saved = await API.insertFxSave(jobId, chain);
                  r.audio_filter = saved.audio_filter;
                  panel.querySelector(".rec-ifx-filter").textContent = saved.audio_filter || "";
                  Toast.success(`Insert FX saved · ${saved.stage_count} stage(s) · baked at next render`);
                });
              });
            };
            // Best-effort client-side preview so the empty/cleared state
            // still shows something sensible without an extra round-trip.
            function chain_to_filter_preview(c) {
              return (c.effects || []).map((e) => `[${e.kind}]`).join(",");
            }
            paintFx();
          }).catch((err) => Toast.error(`Insert FX failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-pitch")?.addEventListener("click", async (e2) => {
          // Pitch / time-stretch panel. Two independent sliders +
          // one-click 'fit to duration' that computes the tempo factor
          // to land the recording on a target publish-slot length. The
          // result composes into a rubberband= filter baked at render.
          const pbtn = e2.currentTarget;
          await withBusy(pbtn, "Loading…", async () => {
            const r = await API.pitchLoad(jobId);
            const panel = host.querySelector(".rec-pitch") || (() => {
              const d = document.createElement("div");
              d.className = "rec-pitch";
              host.appendChild(d);
              return d;
            })();
            const sourceDur = total_duration || dur || 0;
            let pt = r.pitch_time || { tempo: 1, pitch: 1, formant_preserve: true };
            const paint = () => {
              const semis = 12 * Math.log2(Math.max(pt.pitch, 1e-9));
              const projDur = pt.tempo > 0 ? sourceDur / pt.tempo : sourceDur;
              const filter = pt.tempo === 1 && pt.pitch === 1
                ? "(identity — no filter)"
                : `rubberband=tempo=${pt.tempo.toFixed(3)}:pitch=${pt.pitch.toFixed(3)}:formants=${pt.formant_preserve ? "preserved" : "shifted"}`;
              panel.hidden = false;
              panel.innerHTML = `
                <h5>Pitch / time-stretch</h5>
                <div class="rec-pt-row">
                  <label>Tempo <input class="rec-pt-tempo" type="number" step="0.01" min="0.25" max="4" value="${pt.tempo.toFixed(3)}"/>×</label>
                  <span class="pg-cap-hint">Output: ${fmtClock(projDur)} (source ${fmtClock(sourceDur)})</span>
                </div>
                <div class="rec-pt-row">
                  <label>Pitch <input class="rec-pt-semis" type="number" step="0.5" min="-24" max="24" value="${semis.toFixed(2)}"/> semitones</label>
                  <label><input type="checkbox" class="rec-pt-formants" ${pt.formant_preserve ? "checked" : ""}/> Preserve formants (voice)</label>
                </div>
                <div class="rec-pt-row">
                  <button class="sm rec-pt-fit" type="button" title="Compute the tempo factor that lands the source duration on the target. Pitch stays unchanged.">⇥ Fit to duration…</button>
                  <button class="sm rec-pt-reset" type="button" title="Reset to identity">Reset</button>
                  <button class="btn-primary rec-pt-save" type="button">Save</button>
                </div>
                <pre class="rec-pt-filter" title="ffmpeg -af value this setting bakes at render">${filter}</pre>
              `;
              const collect = () => {
                const tempo = parseFloat(panel.querySelector(".rec-pt-tempo").value) || 1;
                const semisVal = parseFloat(panel.querySelector(".rec-pt-semis").value) || 0;
                const formants = panel.querySelector(".rec-pt-formants").checked;
                return { tempo, pitch: Math.pow(2, semisVal / 12), formant_preserve: formants };
              };
              panel.querySelectorAll("input").forEach((inp) => inp.addEventListener("change", () => {
                pt = collect();
                paint();
              }));
              panel.querySelector(".rec-pt-fit").addEventListener("click", async (ev) => {
                const targetStr = prompt(
                  `Fit to duration — target output length, in seconds.\nSource: ${fmtClock(sourceDur)} (${Math.round(sourceDur)}s).\nExample: 3600 for 1h00.`,
                  String(Math.round(sourceDur / 1.1) || 3600),
                );
                if (targetStr == null) return;
                const target = parseFloat(targetStr);
                if (!isFinite(target) || target <= 0) { Toast.error("Target must be > 0"); return; }
                await withBusy(ev.currentTarget, "Computing…", async () => {
                  const res = await API.pitchFit(jobId, sourceDur, target);
                  pt = res.pitch_time;
                  paint();
                  Toast.success(`Fit · tempo ×${pt.tempo.toFixed(3)} → output ${fmtClock(res.projected_output_duration_sec)}`);
                });
              });
              panel.querySelector(".rec-pt-reset").addEventListener("click", () => {
                pt = { tempo: 1, pitch: 1, formant_preserve: true };
                paint();
              });
              panel.querySelector(".rec-pt-save").addEventListener("click", async (ev) => {
                await withBusy(ev.currentTarget, "Saving…", async () => {
                  const res = await API.pitchSave(jobId, { pitch_time: pt, source_duration_sec: sourceDur });
                  Toast.success(res.audio_filter
                    ? `Pitch/time saved · output ≈ ${fmtClock(res.projected_output_duration_sec || projDur)}`
                    : "Pitch/time reset to identity (filter skipped at render)");
                });
              });
            };
            paint();
          }).catch((err) => Toast.error(`Pitch failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-branding")?.addEventListener("click", async (e2) => {
          const bbtn = e2.currentTarget;
          await withBusy(bbtn, "Loading…", async () => {
            const r = await API.brandingLoad(jobId);
            const panel = host.querySelector(".rec-branding");
            if (!panel) return;
            const spec = r.spec || { watermark: null, banners: [] };
            const wm = spec.watermark || { source: { kind: "text", text: "", font_size: 32, color_rgba: "white" }, anchor: "bottom_right", inset_px: 24, opacity: 0.7 };
            const ANCHORS = [
              "top_left","top_center","top_right",
              "middle_left","middle_center","middle_right",
              "bottom_left","bottom_center","bottom_right",
            ];
            const anchorOpts = (sel) => ANCHORS.map((a) => `<option value="${a}"${a === sel ? " selected" : ""}>${a.replace(/_/g, " ")}</option>`).join("");
            const renderBanners = () => (spec.banners || []).map((b, i) => `
              <div class="rec-br-banner" data-i="${i}">
                <select class="rec-br-slot"><option value="intro"${b.slot==="intro"?" selected":""}>intro</option><option value="outro"${b.slot==="outro"?" selected":""}>outro</option></select>
                <input class="rec-br-text" type="text" value="${htmlEscape(b.text||"")}" placeholder="Banner text"/>
                <select class="rec-br-anchor">${anchorOpts(b.anchor)}</select>
                <input class="rec-br-dur" type="number" step="0.5" min="0.5" max="60" value="${b.duration_secs||3}" title="Visible duration (sec)"/>
                <button class="sm danger rec-br-rmb" type="button" title="Remove banner">✕</button>
              </div>`).join("");
            panel.hidden = false;
            panel.innerHTML = `
              <h5>Branding overlay</h5>
              <div class="rec-br-wm">
                <label class="rec-br-on"><input type="checkbox" class="rec-br-enabled" ${spec.watermark ? "checked" : ""}/> Watermark</label>
                <input class="rec-br-wtext" type="text" value="${htmlEscape(wm.source?.text||"@channel")}" placeholder="Watermark text"/>
                <select class="rec-br-wanchor">${anchorOpts(wm.anchor)}</select>
                <input class="rec-br-wop" type="number" step="0.05" min="0" max="1" value="${wm.opacity ?? 0.7}" title="Opacity (0–1)"/>
              </div>
              <div class="rec-br-banners">${renderBanners()}</div>
              <div class="rec-br-actions">
                <button class="sm rec-br-addb" type="button">+ Banner</button>
                <button class="btn-primary rec-br-save" type="button">Save</button>
                <span class="rec-br-preview pg-cap-hint"></span>
              </div>
              <pre class="rec-br-filter" title="filter_complex this spec produces">${htmlEscape(r.filter_complex||"")}</pre>
            `;
            const collect = () => {
              const enabled = panel.querySelector(".rec-br-enabled").checked;
              const newSpec = {
                watermark: enabled ? {
                  source: { kind: "text", text: panel.querySelector(".rec-br-wtext").value || "@channel", font_size: 32, color_rgba: "white" },
                  anchor: panel.querySelector(".rec-br-wanchor").value,
                  inset_px: 24,
                  opacity: parseFloat(panel.querySelector(".rec-br-wop").value) || 0.7,
                } : null,
                banners: Array.from(panel.querySelectorAll(".rec-br-banner")).map((row) => ({
                  slot: row.querySelector(".rec-br-slot").value,
                  text: row.querySelector(".rec-br-text").value || "",
                  font_size: 48,
                  color_rgba: "white",
                  anchor: row.querySelector(".rec-br-anchor").value,
                  inset_px: 40,
                  duration_secs: parseFloat(row.querySelector(".rec-br-dur").value) || 3,
                })),
              };
              return newSpec;
            };
            panel.querySelector(".rec-br-addb").addEventListener("click", () => {
              spec.banners = collect().banners;
              spec.banners.push({ slot: "intro", text: "Welcome", font_size: 48, color_rgba: "white", anchor: "top_center", inset_px: 40, duration_secs: 3.0 });
              panel.querySelector(".rec-br-banners").innerHTML = renderBanners();
            });
            panel.addEventListener("click", (ev) => {
              const t = ev.target;
              if (t && t.classList && t.classList.contains("rec-br-rmb")) {
                const idx = parseInt(t.closest(".rec-br-banner").dataset.i, 10);
                const next = collect();
                next.banners.splice(idx, 1);
                spec.banners = next.banners;
                spec.watermark = next.watermark;
                panel.querySelector(".rec-br-banners").innerHTML = renderBanners();
              }
            });
            panel.querySelector(".rec-br-save").addEventListener("click", async (ev) => {
              const sb = ev.currentTarget;
              const newSpec = collect();
              await withBusy(sb, "Saving…", async () => {
                const saved = await API.brandingSave(jobId, newSpec);
                panel.querySelector(".rec-br-filter").textContent = saved.filter_complex || "";
                Toast.success("Branding saved · applied at next render");
              }).catch((err) => Toast.error(`Save failed: ${err.message}`));
            });
          }).catch((err) => Toast.error(`Branding failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-loudness")?.addEventListener("click", async (_e2) => {
          const panel = host.querySelector(".rec-loudness");
          if (!panel) return;
          panel.hidden = false;
          panel.innerHTML = `
            <h5>EBU R128 loudness</h5>
            <div class="rec-loud-bar">
              <label>
                <span>Target platform</span>
                <select class="rec-loud-platform">
                  <option value="youtube">YouTube · -14 LUFS</option>
                  <option value="spotify">Spotify · -14 LUFS / 7 LU</option>
                  <option value="apple_music">Apple Music · -16 LUFS</option>
                  <option value="ebu_r128">EBU R128 · -23 LUFS</option>
                  <option value="twitch">Twitch · -14 LUFS</option>
                </select>
              </label>
              <button class="btn-primary sm rec-loud-measure">▶ Measure now</button>
            </div>
            <div class="rec-loud-result"></div>
          `;
          panel.querySelector(".rec-loud-measure").addEventListener("click", async (ev) => {
            const mb = ev.currentTarget;
            const platform = panel.querySelector(".rec-loud-platform").value;
            const out = panel.querySelector(".rec-loud-result");
            out.innerHTML = `<div class="empty sm">Running ffmpeg pass-1 — this can take a minute on long captures…</div>`;
            await withBusy(mb, "Measuring…", async () => {
              try {
                const r = await API.loudnessMeasure(jobId, platform);
                const m = r.measurement;
                const d = r.delta;
                // Mirror into the topbar gauge cache.
                loudnessGauge.set(jobId, {
                  i: m.input_i, tp: m.input_tp, lra: m.input_lra,
                  i_delta: d.i_delta, tp_delta: d.tp_delta, lra_delta: d.lra_delta,
                  platform: r.platform, measured_at: Date.now(),
                });
                paintLoudGauge();
                const dRow = (label, value, target, delta, unit) => `
                  <div class="rec-loud-row">
                    <span class="rec-loud-label">${htmlEscape(label)}</span>
                    <span class="rec-loud-meas">${value.toFixed(2)} ${unit}</span>
                    <span class="rec-loud-target">target ${target.toFixed(2)} ${unit}</span>
                    <span class="rec-loud-delta ${delta >= 0 ? "over" : "under"}">${delta >= 0 ? "+" : ""}${delta.toFixed(2)} ${unit}</span>
                  </div>`;
                out.innerHTML = `
                  <p class="pg-cap-hint">Pass-1 measurement complete. Toggle 'Apply normalisation' on the next render to bake the pass-2 filter into the EDL output.</p>
                  ${dRow("Integrated (I)",       m.input_i,    r.target.i,   d.i_delta,   "LUFS")}
                  ${dRow("True peak (TP)",      m.input_tp,   r.target.tp,  d.tp_delta,  "dBTP")}
                  ${dRow("Loudness range (LRA)", m.input_lra,  r.target.lra, d.lra_delta, "LU")}
                  <details class="rec-loud-filter">
                    <summary>Pass-2 ffmpeg filter</summary>
                    <pre>${htmlEscape(r.pass2_filter)}</pre>
                  </details>`;
                Toast.success(`Measured · I=${m.input_i.toFixed(2)} LUFS (Δ ${d.i_delta >= 0 ? "+" : ""}${d.i_delta.toFixed(2)})`);
              } catch (err) {
                out.innerHTML = `<div class="empty sm">⚠ ${htmlEscape(err.message)}</div>`;
              }
            });
          });
        });
        host.querySelector(".rec-ed-history")?.addEventListener("click", async (e2) => {
          const hbtn = e2.currentTarget;
          await withBusy(hbtn, "Loading…", async () => {
            const r = await API.editorRevisions(jobId);
            const panel = host.querySelector(".rec-history");
            if (!panel) return;
            const revs = r.revisions || [];
            panel.hidden = false;
            if (!revs.length) {
              panel.innerHTML = `<p class="pg-cap-hint">No revisions yet. Edits get logged here as you go.</p>`;
              return;
            }
            panel.innerHTML = `
              <h5>Revision history <span class="pg-cap-hint">${revs.length} saved</span></h5>
              <div class="rec-hist-list">
                ${revs.map((v, i) => `
                  <div class="rec-hist-row" data-rev="${v.revision_id}">
                    <span class="rec-hist-idx">v${revs.length - i}</span>
                    <span class="rec-hist-label">${htmlEscape(v.label)}</span>
                    <span class="rec-hist-meta pg-cap-hint">${v.cut_count} cut${v.cut_count===1?"":"s"} · ${fmtClock(v.total_duration_sec)} · ${htmlEscape(v.created_at.replace("T"," ").split(".")[0])}</span>
                    <button class="sm rec-hist-restore" type="button" title="Restore this revision as the current EDL">Restore</button>
                  </div>`).join("")}
              </div>
              <p class="pg-cap-hint">Restoring appends a new revision tagged "revert to vN" so restores are themselves undoable.</p>`;
            panel.querySelectorAll(".rec-hist-restore").forEach((rb) => {
              rb.addEventListener("click", async () => {
                const revId = rb.closest(".rec-hist-row").dataset.rev;
                if (!confirm(`Restore revision v${revId}? Current EDL will become the prior state; a new revision will be appended so this restore is itself undoable.`)) return;
                await withBusy(rb, "Restoring…", async () => {
                  const res = await API.editorRevisionRestore(jobId, revId);
                  edl = res.edl;
                  paint();
                  Toast.success(`Restored · ${res.label}`);
                  // refresh the panel
                  host.querySelector(".rec-ed-history")?.click();
                }).catch((err) => Toast.error(`Restore failed: ${err.message}`));
              });
            });
          }).catch((err) => Toast.error(`History failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-scenes")?.addEventListener("click", async (e2) => {
          const sbtn = e2.currentTarget;
          await withBusy(sbtn, "Loading…", async () => {
            const r = await API.scenesList(jobId);
            const panel = host.querySelector(".rec-scenes");
            if (!panel) return;
            const scenes = r.scenes || [];
            panel.hidden = false;
            const sceneRows = scenes.map((s) => `
              <div class="rec-scene-row" data-scene-id="${htmlEscape(s.id)}">
                <div class="rec-scene-head">
                  <span class="rec-scene-name">${htmlEscape(s.name)}</span>
                  <span class="rec-scene-meta pg-cap-hint">${(s.component_keys || []).length} component${(s.component_keys||[]).length===1?"":"s"} · ${formatBytes(s.size_bytes || 0)} · ${htmlEscape(s.created_at.replace("T"," ").split(".")[0])}</span>
                </div>
                <div class="rec-scene-tags">
                  ${(s.component_keys || []).map(k => `<span class="rec-scene-tag">${htmlEscape(k)}</span>`).join("")}
                </div>
                <div class="rec-scene-actions">
                  <button class="sm rec-scene-restore" type="button" title="Restore this scene as the current state">Restore</button>
                  <button class="sm danger rec-scene-delete" type="button" title="Delete this scene (irreversible)">✕</button>
                </div>
              </div>`).join("");
            panel.innerHTML = `
              <h5>Scene snapshots <span class="pg-cap-hint">${scenes.length} saved</span></h5>
              <form class="rec-scene-capture" onsubmit="return false;">
                <input class="rec-scene-name-input" type="text" placeholder="Scene name (e.g. 'v1 — main mix')" required />
                <button class="btn-primary sm rec-scene-capture-btn" type="submit">+ Capture current state</button>
              </form>
              ${sceneRows ? `<div class="rec-scene-list">${sceneRows}</div>`
                          : `<p class="pg-cap-hint">No scenes yet. Capture the current state to save EDL + branding + automation + loudness + captions style as a named bundle.</p>`}
              <p class="pg-cap-hint">Restoring writes every captured component back to its plugin's store; the EDL restore goes through the editor's revision history so it's itself undoable.</p>`;
            // Wire capture form
            const form = panel.querySelector(".rec-scene-capture");
            form.addEventListener("submit", async (ev) => {
              ev.preventDefault();
              const input = panel.querySelector(".rec-scene-name-input");
              const name = input.value.trim();
              if (!name) { Toast.error("Scene name required"); return; }
              const captureBtn = panel.querySelector(".rec-scene-capture-btn");
              await withBusy(captureBtn, "Capturing…", async () => {
                const res = await API.scenesCapture(jobId, name);
                Toast.success(`Captured · ${res.component_keys.length} component(s) · ${formatBytes(res.size_bytes || 0)}`);
                // Re-open to refresh
                host.querySelector(".rec-ed-scenes")?.click();
              }).catch((err) => Toast.error(`Capture failed: ${err.message}`));
            });
            // Wire restore per row
            panel.querySelectorAll(".rec-scene-restore").forEach((rb) => {
              rb.addEventListener("click", async () => {
                const id = rb.closest(".rec-scene-row").dataset.sceneId;
                if (!confirm("Restore this scene? Every component (EDL, branding, automation, loudness, captions style) will be overwritten with the captured state. The EDL restore is itself undoable via the History panel.")) return;
                await withBusy(rb, "Restoring…", async () => {
                  const res = await API.scenesRestore(jobId, id);
                  // Re-fetch the EDL since the restore touched the
                  // editor store; rebuild the in-memory copy so the
                  // toolbar reflects the new cut list.
                  const reloaded = await API.editorLoad(jobId);
                  edl = reloaded.edl;
                  paint();
                  Toast.success(`Restored · ${res.restored.length} component(s)${res.skipped.length ? ` · ${res.skipped.length} skipped` : ""}.`);
                }).catch((err) => Toast.error(`Restore failed: ${err.message}`));
              });
            });
            // Wire delete per row
            panel.querySelectorAll(".rec-scene-delete").forEach((db) => {
              db.addEventListener("click", async () => {
                const row = db.closest(".rec-scene-row");
                const id = row.dataset.sceneId;
                if (!confirm("Delete this scene? Irreversible.")) return;
                await withBusy(db, "Deleting…", async () => {
                  await API.scenesDelete(jobId, id);
                  row.remove();
                  Toast.success("Scene deleted");
                }).catch((err) => Toast.error(`Delete failed: ${err.message}`));
              });
            });
          }).catch((err) => Toast.error(`Scenes failed: ${err.message}`));
        });
        host.querySelector(".rec-ed-render")?.addEventListener("click", async (e2) => {
          const btnR = e2.currentTarget;
          if (!confirm(`Render EDL to MKV? ${edl.cuts.length} cut(s), total ${fmtClock(dur)}. ffmpeg pass per cut + concat.`)) return;
          await withBusy(btnR, "Rendering…", async () => {
            const res = await API.editorRender(jobId);
            Toast.success(`Rendered ${formatBytes(res.bytes)} → ${res.output_path}`);
          }).catch((err) => Toast.error(`Render failed: ${err.message}`));
        });
      };
      paint();
      Toast.success("EDL loaded");
    }).catch((err) => Toast.error(`Editor failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-casebook]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Composing…", async () => {
      const resp = await API.casebookFetch(jobId);
      const host = document.getElementById("rec-casebook");
      if (!host) return;
      const report = resp.report;
      const md = resp.markdown || "";
      host.hidden = false;
      const sectionHtml = (report.sections || [])
        .map(
          (s) => `<details class="rec-cb-section" open>
            <summary><span class="rec-cb-h">${htmlEscape(s.heading)}</span></summary>
            <div class="rec-cb-body">${md_to_html(s.body)}</div>
          </details>`,
        )
        .join("");
      const titlesHtml = (report.suggested_titles || [])
        .map((t) => `<li>${htmlEscape(t)}</li>`)
        .join("");
      host.innerHTML = `
        <h4 class="rec-cp-title">Casebook · ${htmlEscape(report.title || "")} <span class="pg-cap-hint">${report.sections.length} sections · ${report.suggested_titles.length} title ideas</span></h4>
        <div class="rec-cb-actions">
          <a class="pg-linkbtn" href="${htmlEscape(API.casebookMarkdownUrl(jobId))}" download>Download .md</a>
          <button class="sm rec-cb-copy" type="button">Copy markdown</button>
        </div>
        ${titlesHtml ? `<details class="rec-cb-section"><summary><span class="rec-cb-h">Suggested titles</span></summary><ul class="rec-cb-titles">${titlesHtml}</ul></details>` : ""}
        ${sectionHtml}`;
      host.querySelector(".rec-cb-copy")?.addEventListener("click", async () => {
        try {
          await navigator.clipboard.writeText(md);
          Toast.success("Markdown copied");
        } catch (_) {
          Toast.error("Couldn't copy");
        }
      });
      Toast.success(`Casebook composed (${report.sections.length} sections)`);
    }).catch((err) => Toast.error(`Casebook failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-reuse]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Drafting…", async () => {
      const resp = await API.reuseGenerate(jobId);
      const host = document.getElementById("rec-reuse");
      if (!host) return;
      host.hidden = false;
      const drafts = resp.drafts || [];
      if (!drafts.length) {
        host.innerHTML = '<div class="empty sm">No drafts generated.</div>';
        return;
      }
      const fmtColour = {
        youtube_long: "hsl(0, 70%, 55%)",
        youtube_short: "hsl(15, 80%, 60%)",
        tiktok: "hsl(0, 0%, 90%)",
        patreon: "hsl(20, 70%, 60%)",
        podcast: "hsl(265, 60%, 70%)",
        blog: "hsl(170, 50%, 55%)",
      };
      const fmtLabel = {
        youtube_long: "YouTube (long)",
        youtube_short: "YouTube Shorts",
        tiktok: "TikTok",
        patreon: "Patreon",
        podcast: "Podcast",
        blog: "Blog draft",
      };
      host.innerHTML = `
        <h4 class="rec-cp-title">${drafts.length} publish drafts <span class="pg-cap-hint">queued · re-run regenerates</span></h4>
        <div class="rec-ru-grid">
          ${drafts
            .map(
              (d, i) => `
            <details class="rec-ru-card" style="--ru-c:${fmtColour[d.format] || fmtColour.blog}" data-i="${i}">
              <summary>
                <span class="rec-ru-fmt">${htmlEscape(fmtLabel[d.format] || d.format)}</span>
                <span class="rec-ru-meta">${htmlEscape(d.aspect)} · ${d.duration_sec > 0 ? fmtClock(d.duration_sec) : "—"}${d.clip_starts.length ? ` · ${d.clip_starts.length} clips` : ""}</span>
              </summary>
              <div class="rec-ru-body">
                <h5>Title</h5>
                <div class="rec-ru-title">${htmlEscape(d.title)}</div>
                <h5>Description</h5>
                <pre class="rec-ru-desc">${htmlEscape(d.description)}</pre>
                ${d.hashtags.length ? `<h5>Hashtags</h5><div class="rec-ru-tags">${d.hashtags.map((t) => `<span class="cfg-badge">${htmlEscape(t)}</span>`).join("")}</div>` : ""}
                <div class="rec-ru-actions">
                  <button class="sm rec-ru-copy-title" data-i="${i}">Copy title</button>
                  <button class="sm rec-ru-copy-desc" data-i="${i}">Copy description</button>
                  ${d.hashtags.length ? `<button class="sm rec-ru-copy-tags" data-i="${i}">Copy hashtags</button>` : ""}
                </div>
              </div>
            </details>`,
            )
            .join("")}
        </div>`;
      const cp = async (text) => {
        try {
          await navigator.clipboard.writeText(text || "");
          Toast.success("Copied");
        } catch (_) {
          Toast.error("Couldn't copy");
        }
      };
      host.querySelectorAll(".rec-ru-copy-title").forEach((b) =>
        b.addEventListener("click", () => cp(drafts[+b.dataset.i].title)));
      host.querySelectorAll(".rec-ru-copy-desc").forEach((b) =>
        b.addEventListener("click", () => cp(drafts[+b.dataset.i].description)));
      host.querySelectorAll(".rec-ru-copy-tags").forEach((b) =>
        b.addEventListener("click", () => cp(drafts[+b.dataset.i].hashtags.join(" "))));
      Toast.success(`Generated ${drafts.length} draft(s)`);
    }).catch((err) => Toast.error(`Draft failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-tracks]")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    await withBusy(btn, "Probing…", async () => {
      const resp = await API.multitrackList(jobId);
      const host = document.getElementById("rec-tracks");
      if (!host) return;
      const tracks = resp.tracks || [];
      host.hidden = false;
      if (!tracks.length) {
        host.innerHTML = '<div class="empty sm">No audio tracks detected.</div>';
        return;
      }
      const KIND_COLOUR = {
        mic: "hsl(140, 60%, 60%)",
        game: "hsl(210, 70%, 60%)",
        discord: "hsl(265, 60%, 65%)",
        music: "hsl(35, 80%, 60%)",
        browser: "hsl(195, 60%, 60%)",
        other: "hsl(0, 0%, 65%)",
      };
      host.innerHTML = `
        <h4 class="rec-cp-title">${tracks.length} audio track${tracks.length === 1 ? "" : "s"} <span class="pg-cap-hint">${tracks.length > 1 ? "OBS-style multi-track capture" : "single mixed track"}</span></h4>
        <div class="rec-tk-list">
          ${tracks
            .map(
              (t) => `
            <div class="rec-tk-row" data-idx="${t.index}">
              <span class="rec-tk-kind" style="--rec-tk-c:${KIND_COLOUR[t.inferred_kind] || KIND_COLOUR.other}">${htmlEscape(t.inferred_kind)}</span>
              <span class="rec-tk-label">${htmlEscape(t.title || `track ${t.index}`)}</span>
              <span class="rec-tk-meta">${t.codec} · ${t.channels}ch · ${t.sample_rate ? t.sample_rate + " Hz" : "?"}</span>
              <button class="sm rec-tk-extract" data-idx="${t.index}" data-stem="${htmlEscape((t.title || `track_${t.index}`).replace(/[^A-Za-z0-9_-]+/g, "_"))}">Extract</button>
            </div>`,
            )
            .join("")}
        </div>`;
      host.querySelectorAll(".rec-tk-extract").forEach((btn) => {
        btn.addEventListener("click", async (e) => {
          const b = e.currentTarget;
          await withBusy(b, "Cutting…", async () => {
            const res = await API.multitrackExtract(jobId, {
              track_index: parseInt(b.dataset.idx, 10),
              stem: b.dataset.stem,
            });
            Toast.success(`Cut ${formatBytes(res.bytes)} → ${res.output_path}`);
            b.outerHTML = `<span class="cfg-badge ok" title="${htmlEscape(res.output_path)}">✓ ${formatBytes(res.bytes)}</span>`;
          }).catch((err) => Toast.error(`Extract failed: ${err.message}`));
        });
      });
      Toast.success(`Probed ${tracks.length} track(s)`);
    }).catch((err) => Toast.error(`Probe failed: ${err.message}`));
  });

  overlay.querySelector("[data-action=rec-info-remux]")?.addEventListener("click", async (e) => {
    if (!(await confirmDialog(
      "Remux this recording into a matroska container with the aac_adtstoasc filter? The original is kept as <name>.orig.<ext> until success.",
      { ok: "Remux" },
    )))
      return;
    const btn = e.currentTarget;
    await withBusy(btn, "Remuxing…", async () => {
      await API.remuxRecording(jobId);
      Toast.success("Remuxed — try Play again");
    }).catch((err) => Toast.error(`Remux failed: ${err.message}`));
  });
  overlay.querySelector("[data-action=rec-info-delete]")?.addEventListener("click", async (e) => {
    if (!(await confirmDialog("Delete this recording? File moves to the 7-day trash.", { ok: "Delete", danger: true })))
      return;
    const btn = e.currentTarget;
    await withBusy(btn, "Deleting…", async () => {
      await API.deleteRecordingFile(jobId);
      Toast.success("Deleted");
      recCache = recCache.filter((r) => r.id !== jobId);
      closeRecordingModals();
      if (currentRoute() === "recordings") renderRecordings().catch(() => {});
    }).catch((err) => Toast.error(`Delete failed: ${err.message}`));
  });
  overlay.querySelectorAll("[data-action=rec-info-route-close]").forEach((a) =>
    a.addEventListener("click", () => closeRecordingModals()));
  overlay.querySelectorAll(".rec-copy").forEach((b) =>
    b.addEventListener("click", () => {
      const v = b.dataset.copy || "";
      navigator.clipboard?.writeText(v).then(
        () => Toast.success("Path copied"),
        () => Toast.error("Couldn't copy to clipboard"),
      );
    }));
  overlay.querySelectorAll("[data-action=rec-info-verb]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      await withBusy(btn, "Queued…", async () => {
        await API.pluginRpc(btn.dataset.plugin, btn.dataset.verb, { selection: [jobId] });
        Toast.success(`${btn.dataset.verb} queued`);
      }).catch((err) => Toast.error(`${btn.dataset.verb} failed: ${err.message}`));
    });
  });

  // CE-Fusion F5: an Archive deep link opens straight into the EDL editor
  // rather than making the user find + click the button themselves.
  if (opts.seekSec != null) {
    if (isFinished) {
      overlay.querySelector("[data-action=rec-info-editor]")?.click();
    } else {
      Toast.info(`Target moment ${fmtClock(opts.seekSec)} — the EDL editor unlocks once this recording finishes.`);
    }
  }
}

// ── In-app player ────────────────────────────────────────────────────
// Custom controls — power-user keyboard maps mirror mpv where the HTML5
// video API allows. State is owned by the modal; no globals (except the
// modal-open class) leak out.

async function openRecordingPlayer(jobId, _opts = {}) {
  // The inline modal player has been retired — every recording-open
  // path now navigates to the Player tab so there's a single source of
  // truth for playback. The seek parameter is preserved as a URL param
  // so in-context tools (Crunchr transcript click, cuepoints tick,
  // EDL editor jumps) still land at the right timecode.
  if (!jobId) return;
  // Defensive: dismiss any stray keymap/modal state before the route
  // change so the new Player surface isn't covered by leftover chrome.
  closeRecordingModals();
  document.getElementById("kbd-help")?.classList.remove("open");
  document.body.classList.remove("modal-open");
  const seek = _opts && _opts.seekTo ? `&t=${encodeURIComponent(_opts.seekTo)}` : "";
  window.location.hash = `#/watch?recording=${encodeURIComponent(jobId)}&fresh=1${seek}`;
}

// Legacy modal-player implementation removed. The shim above redirects
// every call site to the Player route. If a future iter needs an
// inline mini-player (e.g. preview-in-context inside an Analytics
// pane), build it as a separate, narrower function rather than
// resurrecting the modal.

// (Legacy modal-player implementation deleted — ~100 lines of
// unreachable code after openRecordingPlayer's return.)

function wirePlayer(overlay, v) {
  const playBtn = overlay.querySelector("#rec-pc-play");
  const seek = overlay.querySelector("#rec-pc-seek");
  const cur = overlay.querySelector("#rec-pc-cur");
  const dur = overlay.querySelector("#rec-pc-dur");
  const speedSel = overlay.querySelector("#rec-pc-speed-sel");
  const muteBtn = overlay.querySelector("#rec-pc-mute");
  const vol = overlay.querySelector("#rec-pc-vol");
  const ccBtn = overlay.querySelector("#rec-pc-cc");
  const pipBtn = overlay.querySelector("#rec-pc-pip");
  const fsBtn = overlay.querySelector("#rec-pc-fs");
  const helpBtn = overlay.querySelector("#rec-pc-help");
  const helpEl = overlay.querySelector("#rec-player-help");
  const abEl = overlay.querySelector("#rec-pc-ab");
  const overlayMsgEl = overlay.querySelector("#rec-player-overlay");
  const overlayMsgText = overlay.querySelector("#rec-player-overlay-msg");
  const state = { a: null, b: null, lastFlash: 0 };

  function flash(msg) {
    overlayMsgText.textContent = msg;
    overlayMsgEl.hidden = false;
    state.lastFlash = Date.now();
    setTimeout(() => {
      if (Date.now() - state.lastFlash >= 700) overlayMsgEl.hidden = true;
    }, 750);
  }
  function paintAb() {
    if (state.a == null && state.b == null) { abEl.textContent = ""; return; }
    const fmt = (s) => s == null ? "—" : fmtClock(s);
    abEl.textContent = `A ${fmt(state.a)} ↔ B ${fmt(state.b)}`;
  }

  v.addEventListener("loadedmetadata", () => {
    dur.textContent = fmtClock(v.duration || 0);
    seek.max = Math.max(1, Math.floor(v.duration * 10));
    // Audio-only files have 0×0 video boxes — collapse the 16:9 stage so
    // the player isn't a giant black rectangle, and hide PiP + fullscreen
    // (PiP throws on a no-video-track stream; fullscreen is pointless).
    const audioOnly = !v.videoWidth && !v.videoHeight;
    overlay.classList.toggle("audio-only", audioOnly);
    // Honour opts.seekTo from openRecordingPlayer callers — e.g. the
    // Crunchr transcript line-click. Once metadata loads we know the
    // duration is valid, so clamping is safe. Auto-play makes the
    // jump feel snappy without the user pressing space.
    if (typeof opts.seekTo === "number" && opts.seekTo >= 0) {
      v.currentTime = Math.min(opts.seekTo, v.duration || opts.seekTo);
      v.play().catch(() => {});
    }
  });
  v.addEventListener("timeupdate", () => {
    cur.textContent = fmtClock(v.currentTime || 0);
    seek.value = Math.floor((v.currentTime || 0) * 10);
    if (state.a != null && state.b != null && v.currentTime >= state.b) {
      v.currentTime = state.a;
    }
  });
  v.addEventListener("play", () => playBtn.textContent = "❚❚");
  v.addEventListener("pause", () => playBtn.textContent = "▶");
  v.addEventListener("error", () => {
    flash("Playback failed — your browser may not support this codec. Try Download from the row menu.");
    overlayMsgEl.hidden = false;
  });

  playBtn.addEventListener("click", () => { v.paused ? v.play() : v.pause(); });
  seek.addEventListener("input", () => { v.currentTime = Number(seek.value) / 10; });
  speedSel.addEventListener("change", () => { v.playbackRate = Number(speedSel.value); flash(`${v.playbackRate}×`); });
  muteBtn.addEventListener("click", () => { v.muted = !v.muted; muteBtn.textContent = v.muted ? "🔇" : "🔊"; });
  vol.addEventListener("input", () => { v.volume = Number(vol.value); v.muted = v.volume === 0; muteBtn.textContent = v.muted ? "🔇" : "🔊"; });
  ccBtn.addEventListener("click", () => {
    const tracks = v.textTracks;
    if (!tracks.length) return;
    const cur = tracks[0];
    cur.mode = cur.mode === "showing" ? "hidden" : "showing";
    ccBtn.classList.toggle("on", cur.mode === "showing");
  });
  pipBtn.addEventListener("click", async () => {
    try {
      if (document.pictureInPictureElement === v) await document.exitPictureInPicture();
      else await v.requestPictureInPicture();
    } catch (e) { flash(e.message); }
  });
  fsBtn.addEventListener("click", () => {
    try {
      if (document.fullscreenElement) document.exitFullscreen();
      else overlay.querySelector(".rec-player-stage").requestFullscreen?.();
    } catch (err) {
      // Safari/iOS reject fullscreen outside user-gesture context.
      Toast.error?.(`Fullscreen denied: ${err && err.message || err}`);
    }
  });
  helpBtn.addEventListener("click", () => { helpEl.hidden = !helpEl.hidden; });

  // Keyboard map.
  function onKey(e) {
    // Don't grab keystrokes inside the speed dropdown / sliders.
    if (e.target.closest("select, input")) return;
    const k = e.key;
    if (k === "?") { helpEl.hidden = !helpEl.hidden; e.preventDefault(); return; }
    if (k === " ") { v.paused ? v.play() : v.pause(); e.preventDefault(); return; }
    if (k === "ArrowLeft") { v.currentTime = Math.max(0, v.currentTime - 5); e.preventDefault(); return; }
    if (k === "ArrowRight") { v.currentTime = Math.min((v.duration || 0), v.currentTime + 5); e.preventDefault(); return; }
    if (k === "j" || k === "J") { v.currentTime = Math.max(0, v.currentTime - 10); e.preventDefault(); return; }
    if (k === "l" || k === "L") { v.currentTime = Math.min((v.duration || 0), v.currentTime + 10); e.preventDefault(); return; }
    if (k === "k" || k === "K") { v.paused ? v.play() : v.pause(); e.preventDefault(); return; }
    if (k === ",") { v.pause(); v.currentTime = Math.max(0, v.currentTime - 1/30); e.preventDefault(); return; }
    if (k === ".") { v.pause(); v.currentTime = Math.min((v.duration || 0), v.currentTime + 1/30); e.preventDefault(); return; }
    if (k === "<") { speedSel.selectedIndex = Math.max(0, speedSel.selectedIndex - 1); speedSel.dispatchEvent(new Event("change")); e.preventDefault(); return; }
    if (k === ">") { speedSel.selectedIndex = Math.min(speedSel.options.length - 1, speedSel.selectedIndex + 1); speedSel.dispatchEvent(new Event("change")); e.preventDefault(); return; }
    if (k === "ArrowUp") { vol.value = Math.min(1, Number(vol.value) + 0.05); vol.dispatchEvent(new Event("input")); e.preventDefault(); return; }
    if (k === "ArrowDown") { vol.value = Math.max(0, Number(vol.value) - 0.05); vol.dispatchEvent(new Event("input")); e.preventDefault(); return; }
    if (k === "m" || k === "M") { muteBtn.click(); e.preventDefault(); return; }
    if (k === "i" || k === "I") { state.a = v.currentTime; paintAb(); flash(`A = ${fmtClock(state.a)}`); e.preventDefault(); return; }
    if (k === "o" || k === "O") { state.b = v.currentTime; paintAb(); flash(`B = ${fmtClock(state.b)}`); e.preventDefault(); return; }
    if (k === "c" || k === "C") { state.a = null; state.b = null; paintAb(); flash("A-B cleared"); e.preventDefault(); return; }
    if (k === "f" || k === "F") { fsBtn.click(); e.preventDefault(); return; }
    if (k === "p" || k === "P") { pipBtn.click(); e.preventDefault(); return; }
    if (k === "t" || k === "T") { ccBtn.click(); e.preventDefault(); return; }
    if (/^[0-9]$/.test(k)) {
      const frac = Number(k) / 10;
      if (v.duration) v.currentTime = v.duration * frac;
      e.preventDefault(); return;
    }
  }
  overlay.addEventListener("keydown", onKey);
  // Tear the global keydown when modal closes — done implicitly because
  // the overlay is removed from the DOM in closeRecordingModals.
}

// ── Stub routes ──────────────────────────────────────────────────────
function renderStub(title, msg) {
  root.removeAttribute("aria-busy");
  root.innerHTML = chrome(`
    <h1 class="page-title">${htmlEscape(title)}</h1>
    <div class="empty">
      <div class="glyph">🚧</div>
      ${htmlEscape(msg)}
    </div>
  `);
  setupChromeHandlers();
}

// ── Settings (Jellyfin-style two-pane shell) ────────────────────────
// Left rail = section nav (sub-route via #/settings/<section>).
// Right pane = section content. All knobs the daemon exposes get a
// visible row — read-only for now (Phase 2a). Phase 2b wires writes;
// Phase 2c adds the platforms wizard + keyring. Tooltip hints (the
// `title` attribute on .stg-hint) explain non-obvious knobs without
// cluttering the layout.
const SETTINGS_SECTIONS = [
  { slug: "general", label: "General", icon: "⚙" },
  { slug: "recording", label: "Recording", icon: "⏺" },
  { slug: "notifications", label: "Notifications", icon: "🔔" },
  { slug: "platforms", label: "Platforms", icon: "🔌" },
  { slug: "plugins", label: "Plugins", icon: "🧩" },
  { slug: "interface", label: "Interface", icon: "🎨" },
  { slug: "multiview", label: "Multi-view", icon: "▦" },
  { slug: "advanced", label: "Advanced", icon: "🛠" },
  { slug: "about", label: "About", icon: "ℹ" },
];

async function renderSettings() {
  const parts = routeParts(); // ["settings", <slug?>]
  const slug = parts[1] || "general";
  let known = SETTINGS_SECTIONS.find((s) => s.slug === slug)
    ? slug
    : "general";
  // The Plugins pane is Creator Edition only.
  if (known === "plugins" && !CREATOR_ENABLED) known = "general";

  let s = {};
  try {
    s = await API.settings();
  } catch (e) {
    if (e.message && e.message.includes("unauthorized")) return;
  }
  root.removeAttribute("aria-busy");

  const rail = SETTINGS_SECTIONS
    .filter((sec) => CREATOR_ENABLED || sec.slug !== "plugins")
    .map((sec) => `
    <a class="stg-rail-item ${sec.slug === known ? "is-active" : ""}"
       href="#/settings/${sec.slug}">
      <span class="stg-rail-icon" aria-hidden="true">${sec.icon}</span>
      <span class="stg-rail-label">${htmlEscape(sec.label)}</span>
    </a>`).join("");

  const pane = renderSettingsPane(known, s);

  root.innerHTML = chrome(`
    <h1 class="page-title">Settings</h1>
    <p class="page-subtitle">Live daemon configuration. Toggles and numeric knobs persist to <code>~/.config/strivo/config.toml</code> on change.</p>
    <div class="stg-shell">
      <nav class="stg-rail" aria-label="Settings sections">
        <div class="stg-search-wrap">
          <input id="stg-search" class="stg-search" type="search"
                 placeholder="Filter settings…" aria-label="Filter settings" />
        </div>
        ${rail}
      </nav>
      <div class="stg-pane" id="stg-pane">${pane}</div>
    </div>
  `);
  setupChromeHandlers();
  wireSettingsControls();
  wireSettingsSearch();
}

// Filter rows in the right pane and rail items by typed query (audit M10).
function wireSettingsSearch() {
  const input = document.getElementById("stg-search");
  if (!input) return;
  input.addEventListener("input", () => {
    const q = input.value.trim().toLowerCase();
    document.querySelectorAll(".stg-row").forEach((r) => {
      const txt = r.textContent.toLowerCase();
      r.classList.toggle("stg-row-hidden", q.length > 0 && !txt.includes(q));
    });
    // Hide group headings whose rows all collapsed.
    document.querySelectorAll(".stg-group").forEach((g) => {
      const anyVisible = g.querySelector(".stg-row:not(.stg-row-hidden)");
      g.style.display = q && !anyVisible ? "none" : "";
    });
  });
}

// Wire every editable control on the right pane. Each control declares
// its dotted config path via `data-stg-path` and its type via the input
// itself (checkbox / number). On change we POST to /settings/update;
// failure rolls the control back to its previous value and toasts.
function wireSettingsControls() {
  const pane = document.getElementById("stg-pane");
  if (!pane) return;
  // Multi-view quality selects are browser-local, so they persist straight
  // to localStorage rather than through the daemon config endpoint. Applying
  // takes effect on the next reconcile — no reload, that being the point.
  pane.querySelectorAll("[data-mv-quality]").forEach((sel) => {
    sel.addEventListener("change", () => {
      setMultiviewQuality(sel.dataset.mvQuality, sel.value);
      Toast.info("Multi-view quality updated — applies to open tiles immediately.");
      for (const [, ctl] of playerState.controllers) {
        try {
          ctl.setQuality(qualityPolicyFor(ctl.kind, ""));
        } catch (_) {
          /* advisory */
        }
      }
    });
  });

  // Configure / Reconfigure buttons on the Platforms section open a
  // wizard modal per platform.
  pane.querySelectorAll(".stg-cfg-btn").forEach((btn) => {
    btn.addEventListener("click", () => openPlatformWizard(btn.dataset.platform));
  });
  // Master toggles dim their dependent conditional subgroup when off — the
  // Notifications master switch dims Events, the Webhook enable toggle dims
  // the Webhook URL field. We do this in JS rather than re-rendering so
  // users see immediate visual feedback during the save round-trip. Each
  // `.stg-subgroup-conditional` names its master via `data-conditional-master`
  // so any number of these pairs can coexist.
  pane.querySelectorAll(".stg-subgroup-conditional[data-conditional-master]").forEach((condEl) => {
    const masterEl = pane.querySelector(`[data-stg-path="${condEl.getAttribute("data-conditional-master")}"]`);
    if (!masterEl) return;
    const syncMaster = () => {
      if (masterEl.checked) {
        condEl.style.opacity = "";
        condEl.style.pointerEvents = "";
      } else {
        condEl.style.opacity = "0.55";
        condEl.style.pointerEvents = "none";
      }
    };
    masterEl.addEventListener("change", syncMaster);
  });
  // Onboarding controls — replay the welcome tour / reset per-page hints.
  pane.querySelector("#stg-replay-tour")?.addEventListener("click", () => {
    localStorage.removeItem("strivo-tour-done");
    startOnboardingTour();
  });
  pane.querySelector("#stg-reset-hints")?.addEventListener("click", () => {
    for (const k of Object.keys(localStorage)) {
      if (k.startsWith("strivo-hint-")) localStorage.removeItem(k);
    }
    Toast.success("Per-page hints reset · will reappear next visit");
    render().catch(() => {});
  });
  // Layout reorder widgets — Kodi/Aeon-style up/down lists.
  // Each .stg-reorder reads its current order from localStorage
  // (falling back to data-default), renders one row per entry with
  // ▲ / ▼ buttons, and persists on any movement.
  pane.querySelectorAll(".stg-reorder").forEach((box) => {
    const key = box.dataset.reorderKey;
    const def = JSON.parse(box.dataset.default || "[]");
    let order;
    try { order = JSON.parse(localStorage.getItem(key) || ""); if (!Array.isArray(order)) order = def; }
    catch { order = def; }
    // Repair: keep only known entries, append any default entries that
    // got added in a later release so the list never goes stale.
    order = order.filter((x) => def.includes(x));
    for (const d of def) if (!order.includes(d)) order.push(d);
    const list = box.querySelector(".stg-reorder-list");
    const render = () => {
      list.innerHTML = order.map((name, i) => `
        <div class="stg-reorder-item">
          <span class="stg-reorder-label">${htmlEscape(name)}</span>
          <button class="sm stg-reorder-up" data-i="${i}" type="button" ${i === 0 ? "disabled" : ""}>▲</button>
          <button class="sm stg-reorder-down" data-i="${i}" type="button" ${i === order.length - 1 ? "disabled" : ""}>▼</button>
        </div>`).join("");
      list.querySelectorAll(".stg-reorder-up").forEach((btn) => btn.addEventListener("click", () => {
        const i = +btn.dataset.i;
        if (i > 0) { [order[i - 1], order[i]] = [order[i], order[i - 1]]; persist(); render(); }
      }));
      list.querySelectorAll(".stg-reorder-down").forEach((btn) => btn.addEventListener("click", () => {
        const i = +btn.dataset.i;
        if (i < order.length - 1) { [order[i + 1], order[i]] = [order[i], order[i + 1]]; persist(); render(); }
      }));
    };
    const persist = () => localStorage.setItem(key, JSON.stringify(order));
    render();
    box.querySelector(".stg-reorder-reset")?.addEventListener("click", () => {
      order = def.slice();
      persist(); render();
      Toast.success("Reset to default order");
    });
  });
  pane.querySelectorAll(".stg-layout-select").forEach((sel) => {
    const key = sel.dataset.layoutKey;
    const stored = localStorage.getItem(key);
    if (stored) sel.value = stored;
    sel.addEventListener("change", () => {
      localStorage.setItem(key, sel.value);
      Toast.success("Layout preference saved");
    });
  });
  // Per-plugin Size / Clear actions — wired here so all plugin rows
  // pick up the handlers via a single querySelectorAll regardless of
  // which section painted them.
  pane.querySelectorAll(".stg-plugin-size").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const name = btn.dataset.plugin;
      try {
        const r = await API.pluginStorageSize(name);
        Toast.success(`${name}: ${formatBytes(r.bytes || 0)} across ${r.file_count || 0} file(s)${r.path ? ` (${r.path})` : ""}`);
      } catch (err) {
        Toast.error(`Size lookup failed: ${err.message}`);
      }
    });
  });
  pane.querySelectorAll(".stg-plugin-clear").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const name = btn.dataset.plugin;
      const ok = confirm(`Permanently delete all stored data for plugin '${name}'?\n\nThis removes per-recording SQLite databases, JSON spec files, and any cached output. Cannot be undone.`);
      if (!ok) return;
      try {
        const r = await API.pluginStorageClear(name);
        Toast.success(`${name}: deleted ${r.files_removed || 0} file(s), reclaimed ${formatBytes(r.bytes_removed || 0)}`);
      } catch (err) {
        Toast.error(`Clear failed: ${err.message}`);
      }
    });
  });
  // Filename template live preview — updates #stg-fn-preview as user types.
  const fnInput = pane.querySelector('[data-stg-path="recording.filename_template"]');
  const fnPreview = pane.querySelector("#stg-fn-preview");
  if (fnInput && fnPreview) {
    const updatePreview = () => {
      const tpl = fnInput.value || "{channel}_{date}_{title}.mkv";
      const d = new Date();
      const preview = tpl
        .replace("{channel}", "mychannel")
        .replace("{platform}", "twitch")
        .replace("{date}", d.toISOString().slice(0, 10))
        .replace("{time}", d.toTimeString().replace(/\D/g, "").slice(0, 6))
        .replace("{title}", "stream_title")
        .replace("{id}", "1234567890");
      fnPreview.textContent = preview;
    };
    fnInput.addEventListener("input", updatePreview);
  }

  // Capture profile add / delete (Task 1).
  pane.querySelector("#stg-profile-add")?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const nameInp = pane.querySelector("#stg-profile-name-inp");
    const tierSel = pane.querySelector("#stg-profile-tier-sel");
    if (!nameInp) return;
    const name = nameInp.value.trim();
    if (!name) return;
    const quality_tier = tierSel ? (tierSel.value || null) : null;
    try {
      await API.captureProfileCreate({ name, quality_tier });
      Toast.success(`Profile '${name}' created`);
      renderSettings();
    } catch (err) {
      Toast.error(`Couldn't create profile: ${err.message}`);
    }
  });
  pane.querySelectorAll(".stg-profile-del").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const name = btn.dataset.profileName;
      if (!confirm(`Delete capture profile '${name}'?`)) return;
      try {
        await API.captureProfileDelete(name);
        Toast.success(`Profile '${name}' deleted`);
        renderSettings();
      } catch (err) {
        Toast.error(`Couldn't delete: ${err.message}`);
      }
    });
  });

  // Channel export / import (Task 4).
  pane.querySelector(".stg-channels-export")?.addEventListener("click", async () => {
    try {
      const data = await API.channelsExport();
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `strivo-channels-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (err) {
      Toast.error(`Export failed: ${err.message}`);
    }
  });
  const importFileInput = pane.querySelector("#stg-channels-import-file");
  pane.querySelector(".stg-channels-import-btn")?.addEventListener("click", () => {
    importFileInput?.click();
  });
  importFileInput?.addEventListener("change", async () => {
    const file = importFileInput.files[0];
    if (!file) return;
    try {
      const text = await file.text();
      const data = JSON.parse(text);
      const result = await API.channelsImport(data);
      Toast.success(`Imported: ${result.added ?? 0} added, ${result.updated ?? 0} updated`);
      renderSettings();
    } catch (err) {
      Toast.error(`Import failed: ${err.message}`);
    } finally {
      importFileInput.value = "";
    }
  });

  pane.querySelectorAll("[data-stg-path]").forEach((el) => {
    el.addEventListener("change", async () => {
      const path = el.getAttribute("data-stg-path");
      let value;
      if (el.type === "checkbox") value = el.checked;
      else if (el.type === "number") value = parseInt(el.value, 10);
      else value = (el.value || "").trim();
      const previous = el.type === "checkbox"
        ? !el.checked
        : el.getAttribute("data-prev") || "";
      // Inline field validation (item 25) — same idiom as the poll-interval
      // input. The webhook URL may be blank (clears the setting) but a
      // non-blank value must be a valid http(s) URL before we round-trip
      // to the server, which enforces the same rule.
      if (path === "notifications.webhook.url" && value) {
        let validUrl = false;
        try {
          const u = new URL(value);
          validUrl = u.protocol === "http:" || u.protocol === "https:";
        } catch { validUrl = false; }
        if (!validUrl) {
          el.setAttribute("aria-invalid", "true");
          Toast.error("Webhook URL must be a valid http:// or https:// URL");
          return;
        }
      }
      el.removeAttribute("aria-invalid");
      try {
        await API.updateSetting(path, value);
        if (el.type !== "checkbox") el.setAttribute("data-prev", String(value));
        if (path === "ui.reduce_motion") {
          REDUCE_MOTION_SETTING = !!value;
          applyReducedMotion();
        }
        Toast.success(`Saved · ${path}`);
      } catch (err) {
        if (el.type === "checkbox") el.checked = previous;
        else el.value = previous;
        Toast.error(`Couldn't save ${path}: ${err.message}`);
      }
    });
  });
}

// Build the right-pane HTML for a section. Each section is a sequence of
// sub-headed groups, then a flat list of rows: label · value · hint.
function renderSettingsPane(slug, s) {
  const rec = s.recording || {};
  const arc = s.archiver || {};
  const ui = s.ui || {};
  const badge = (ok, okText, noText) =>
    `<span class="cfg-badge ${ok ? "ok" : "warn"}">${ok ? okText : noText}</span>`;
  const code = (v) => `<code>${htmlEscape(v || "—")}</code>`;
  // Editable controls: rendered as live inputs bound to a config path.
  // wireSettingsControls() picks them up via [data-stg-path].
  const toggle = (path, checked) => `
    <label class="stg-toggle">
      <input type="checkbox" data-stg-path="${htmlEscape(path)}" ${checked ? "checked" : ""} />
      <span class="stg-toggle-track"><span class="stg-toggle-knob"></span></span>
    </label>`;
  const numInput = (path, value, min, max) => `
    <input class="stg-num" type="number" data-stg-path="${htmlEscape(path)}"
           data-prev="${value ?? ""}" value="${value ?? ""}"
           min="${min}" max="${max}" step="1" />`;
  const textInput = (path, value, placeholder = "") => `
    <input class="stg-text" type="text" data-stg-path="${htmlEscape(path)}"
           data-prev="${htmlEscape(value ?? "")}" value="${htmlEscape(value ?? "")}"
           placeholder="${htmlEscape(placeholder)}" spellcheck="false" />`;
  const selectInput = (path, value, opts) => {
    const options = opts
      .map((o) => `<option value="${htmlEscape(o)}"${o === value ? " selected" : ""}>${htmlEscape(o)}</option>`)
      .join("");
    return `<select class="stg-select" data-stg-path="${htmlEscape(path)}" data-prev="${htmlEscape(value ?? "")}">${options}</select>`;
  };
  // Filename template token browser — collapsible <details> with live preview.
  const TEMPLATE_TOKENS = [
    ["{channel}",  "Channel name"],
    ["{platform}", "twitch / youtube / patreon"],
    ["{date}",     "YYYY-MM-DD (local)"],
    ["{time}",     "HHmmss (local)"],
    ["{title}",    "Stream title (path-safe)"],
    ["{id}",       "Broadcast / video ID"],
  ];
  const tokenBrowserHtml = (currentTpl) => {
    const rows = TEMPLATE_TOKENS.map(([tok, desc]) =>
      `<tr><td><code class="stg-tok">${htmlEscape(tok)}</code></td>` +
      `<td class="muted stg-tok-desc">${htmlEscape(desc)}</td></tr>`
    ).join("");
    const example = (currentTpl || "{channel}_{date}_{title}.mkv")
      .replace("{channel}", "mychannel")
      .replace("{platform}", "twitch")
      .replace("{date}", new Date().toISOString().slice(0, 10))
      .replace("{time}", new Date().toTimeString().replace(/\D/g, "").slice(0, 6))
      .replace("{title}", "stream_title")
      .replace("{id}", "1234567890");
    return `<details class="stg-token-browser">
      <summary class="stg-token-summary">Tokens ▸</summary>
      <table class="stg-token-table">${rows}</table>
      <div class="stg-token-preview">Preview: <code id="stg-fn-preview">${htmlEscape(example)}</code></div>
    </details>`;
  };
  // Row helper. `hint` is rendered as a tooltip on a ⓘ glyph so the
  // layout stays clean; long-form text only appears on hover.
  const row = (label, value, hint) => `
    <div class="stg-row">
      <div class="stg-row-label">
        ${htmlEscape(label)}
        ${hint ? `<span class="stg-hint" title="${htmlEscape(hint)}" aria-label="${htmlEscape(hint)}">ⓘ</span>` : ""}
      </div>
      <div class="stg-row-value">${value}</div>
    </div>`;
  // Sweet-folders glyphs picked by category semantic. Falls back to
  // generic folder.svg for unknown categories.
  const categoryIcon = (title) => {
    const t = (title || "").toLowerCase();
    if (t.includes("editor") || t.includes("publish")) return "folder-templates.svg";
    if (t.includes("audio") || t.includes("music")) return "folder-music.svg";
    if (t.includes("video") || t.includes("recording") || t.includes("watch")) return "folder-videos.svg";
    if (t.includes("archive") || t.includes("download")) return "folder-download.svg";
    if (t.includes("brand") || t.includes("thumbnail") || t.includes("picture")) return "folder-pictures.svg";
    if (t.includes("transcript") || t.includes("caption") || t.includes("report") || t.includes("doc")) return "folder-documents.svg";
    if (t.includes("share") || t.includes("multi") || t.includes("stream")) return "folder-publicshare.svg";
    if (t.includes("home") || t.includes("glance")) return "folder-home.svg";
    if (t.includes("chat") || t.includes("network") || t.includes("remote")) return "folder-remote-symbolic.svg";
    return "folder.svg";
  };
  const group = (title, rows) => `
    <section class="stg-group">
      <h3 class="stg-group-title"><img class="stg-group-icon" src="/assets/icons/sweet-folders/${categoryIcon(title)}" alt="" aria-hidden="true"/> ${htmlEscape(title)}</h3>
      <div class="stg-rows">${rows}</div>
    </section>`;

  switch (slug) {
    case "multiview": {
      const q = multiviewQuality();
      const opts = (current) =>
        MULTIVIEW_QUALITY_CHOICES.map(
          ([v, label]) =>
            `<option value="${v}"${v === current ? " selected" : ""}>${htmlEscape(label)}</option>`,
        ).join("");
      return [
        group("Stream quality", [
          row(
            "Twitch",
            `<select class="stg-select" data-mv-quality="twitch">${opts(q.twitch)}</select>`,
            "Applied per tile without reloading the stream. The options a stream " +
              "actually offers are discovered from the player at runtime, so this " +
              "picks an end of that list rather than a fixed resolution.",
          ),
          row(
            "YouTube",
            `<select class="stg-select" disabled><option>Controlled by YouTube</option></select>`,
            "YouTube decommissioned quality control in its player API — setPlaybackQuality " +
              "is now a no-op, so nothing here could take effect. The only remaining " +
              "influence is how large the tile is drawn.",
          ),
        ].join("")),
        group("About these settings", [
          row(
            "Why no single scale",
            "&mdash;",
            "A resolution label is not a quality measure: the same \"1080p\" is a " +
              "different bitrate between platforms and between streamers on one " +
              "platform. Each provider is therefore offered on its own terms rather " +
              "than flattened into a shared ladder.",
          ),
          row(
            "Where this is stored",
            "&mdash;",
            "Per browser, alongside your layout and volume preferences — a laptop on " +
              "hotel wifi and a desktop on gigabit want different answers.",
          ),
        ].join("")),
      ].join("");
    }
    case "general":
      return [
        group("At a glance", [
          row(
            "Tracked channels",
            `<a href="#/library" class="stg-linkbtn">${channelCache.length} channel${channelCache.length === 1 ? "" : "s"} →</a>`,
            "Click to manage channels in Library.",
          ),
          row(
            "Active recordings",
            `<a href="#/recordings" class="stg-linkbtn">${recCache.filter((r) => isInProgress(r.state)).length} in progress →</a>`,
            "Live captures + VOD pulls in flight.",
          ),
          row(
            "Patreon creators",
            `${(patreonState.creators || []).length}`,
            "Followed Patreon creators (read-only here; manage via the rail).",
          ),
        ].join("")),
        group("Polling", [
          row(
            "Channel poll interval",
            `${s.poll_interval_secs ?? "?"} s`,
            "How often StriVo checks each tracked channel for a live-state change. Twitch EventSub + YouTube WebSub push live signals in real time; this poll is the fallback.",
          ),
          row(
            "Auto-record channels",
            `${(s.auto_record_channels || []).length}`,
            "Channels whose new live broadcasts are recorded automatically. Managed from the Library page.",
          ),
        ].join("")),
        group("Storage", [
          row(
            "Recording directory",
            code(s.recording_dir),
            "Root directory for all recordings. Each platform/channel gets its own subdirectory.",
          ),
        ].join("")),
        group("Channels", [
          row(
            "Export / Import",
            `<button class="sm stg-channels-export" type="button">Export channels JSON</button>
             <button class="sm stg-channels-import-btn" type="button">Import channels JSON</button>
             <input id="stg-channels-import-file" type="file" accept=".json" style="display:none" />`,
            "Export all tracked channels + capture profiles as a JSON file. Import merges new channels and updates existing ones by platform + channel ID.",
          ),
        ].join("")),
      ].join("");

    case "notifications": {
      const n = s.notifications || {};
      const masterOn = n.desktop_enabled !== false;
      const noteAttr = masterOn ? "" : ' style="opacity:0.55;pointer-events:none"';
      const wh = n.webhook || {};
      const whOn = wh.enabled === true;
      const whNoteAttr = whOn ? "" : ' style="opacity:0.55;pointer-events:none"';
      return [
        group("Desktop notifications", [
          row(
            "Master switch",
            toggle("notifications.desktop_enabled", n.desktop_enabled !== false),
            "When off, the daemon skips every notify-rust banner regardless of the toggles below. Useful for headless / kiosk setups.",
          ),
        ].join("")),
        `<div class="stg-subgroup-conditional" data-conditional-master="notifications.desktop_enabled"${noteAttr}>${[
          group("Events", [
            row(
              "Channel goes live",
              toggle("notifications.on_go_live", n.on_go_live !== false),
              "Banner when a tracked channel transitions offline → live.",
            ),
            row(
              "Recording finished",
              toggle("notifications.on_recording_finished", n.on_recording_finished !== false),
              "Banner when a live capture or VOD pull completes successfully.",
            ),
            row(
              "Recording failed",
              toggle("notifications.on_recording_failed", n.on_recording_failed !== false),
              "Recommended — notifies you the moment a capture fails so it doesn't go silently missed.",
            ),
            row(
              "VOD backfill ready",
              toggle("notifications.on_vod_ready", n.on_vod_ready === true),
              "Notify when a VOD becomes available after a live stream finishes. Off by default.",
            ),
          ].join("")),
        ].join("")}</div>`,
        group("Webhook", [
          row(
            "Enable webhook",
            toggle("notifications.webhook.enabled", whOn),
            "Fires on recording events — POSTs a JSON payload (streamerREC-compatible: event, channel, platform, recording ID, status, error) to the URL below for channel-live, recording-started/finished/failed, and generic notifications. Independent of the desktop notifications above.",
          ),
        ].join("")),
        `<div class="stg-subgroup-conditional" data-conditional-master="notifications.webhook.enabled"${whNoteAttr}>${[
          group("Webhook target", [
            row(
              "Webhook URL",
              textInput("notifications.webhook.url", wh.url || "", "https://example.com/hooks/strivo"),
              "Full http(s) URL to POST events to. Required for the webhook to fire; validated before saving.",
            ),
          ].join("")),
        ].join("")}</div>`,
      ].join("");
    }

    case "recording": {
      // Capture-profile rows (Task 1).
      const profiles = s.capture_profiles || [];
      const QUALITY_TIER_LABELS = {
        best: "Best", "1080p": "1080p", "720p": "720p", "480p": "480p", audio_only: "Audio only",
      };
      const profileRows = profiles.length
        ? profiles.map((p) => `
          <div class="stg-row stg-profile-row" data-profile-name="${htmlEscape(p.name)}">
            <div class="stg-row-label">${htmlEscape(p.name)}</div>
            <div class="stg-row-value">
              ${p.quality_tier
                ? `<span class="cfg-badge ok">${htmlEscape(QUALITY_TIER_LABELS[p.quality_tier] || p.quality_tier)}</span>`
                : `<span class="muted">no tier</span>`}
              <button class="sm stg-profile-del" data-profile-name="${htmlEscape(p.name)}"
                      type="button" title="Delete profile '${htmlEscape(p.name)}'">✕</button>
            </div>
          </div>`).join("")
        : `<div class="empty sm">No capture profiles defined yet.</div>`;
      const tierOpts = ["", "best", "1080p", "720p", "480p", "audio_only"]
        .map((v) => `<option value="${htmlEscape(v)}">${htmlEscape(QUALITY_TIER_LABELS[v] || "(none)")}</option>`)
        .join("");
      const addProfileForm = `
        <form id="stg-profile-add" class="stg-profile-add">
          <input id="stg-profile-name-inp" type="text" placeholder="Profile name"
                 maxlength="64" required aria-label="New profile name" />
          <select id="stg-profile-tier-sel" aria-label="Quality tier">${tierOpts}</select>
          <button class="btn-primary" type="submit">Add</button>
        </form>`;
      return [
        group("Output", [
          row("Filename template",
            textInput("recording.filename_template", rec.filename_template, "{channel}_{date}_{title}.mkv") +
            tokenBrowserHtml(rec.filename_template)),
          row("Container",
            selectInput("recording.container",
              (rec.container || "matroska").toLowerCase(),
              ["matroska", "mp4", "webm"]),
            "Output muxer. Matroska is the browser-friendliest default; switch only if you have a downstream pipeline that needs MP4 or WebM."),
          row("Transcode", toggle("recording.transcode", rec.transcode),
            "Re-encode on the fly via h264_nvenc. Off = stream-copy (zero CPU, original bitrate)."),
        ].join("")),
        group("Twitch", [
          row("Record from start", toggle("recording.twitch_live_from_start", rec.twitch_live_from_start),
            "Pull from the first available HLS segment (~5 min back) instead of the live edge. Sub-only channels reject this and StriVo silently falls back to live edge."),
        ].join("")),
        group("YouTube / VOD", [
          row("Auto VOD backfill", toggle("recording.auto_vod_backfill", rec.auto_vod_backfill),
            "When a stream ends, automatically queue the resulting VOD for download via yt-dlp."),
          row("Auto-trim ads", toggle("recording.auto_trim_ads", rec.auto_trim_ads),
            "Run sponsorblock-style ad-segment trimming on completed Twitch VODs."),
        ].join("")),
        group("Capture Profiles", profileRows + addProfileForm),
      ].join("");
    }

    case "platforms": {
      const platformRow = (key, statusOk) => `
        <div class="stg-row">
          <div class="stg-row-label">Status</div>
          <div class="stg-row-value">
            ${badge(statusOk, "configured", "not configured")}
            <button class="stg-linkbtn stg-cfg-btn" data-platform="${htmlEscape(key)}" type="button">
              ${statusOk ? "Reconfigure" : "Configure"} →
            </button>
          </div>
        </div>`;
      return [
        group("Twitch",
          platformRow("twitch", s.twitch_configured) +
          `<div class="stg-row"><div class="stg-row-label">Setup
            <span class="stg-hint" title="Create at dev.twitch.tv/console/apps — type=Other, OAuth Redirect URL http://localhost:8181/oauth/twitch">ⓘ</span>
          </div><div class="stg-row-value muted">Twitch Developer Console → Register Your Application → Client ID + Secret.</div></div>`),
        group("YouTube",
          platformRow("youtube", s.youtube_configured) +
          `<div class="stg-row"><div class="stg-row-label">Setup
            <span class="stg-hint" title="Google Cloud Console → APIs &amp; Services → Credentials → OAuth client ID. Use Desktop type.">ⓘ</span>
          </div><div class="stg-row-value muted">OAuth client (Desktop type). Optional Netscape cookies.txt for member-only / age-gated VODs.</div></div>`),
        group("Patreon",
          platformRow("patreon", s.patreon_configured) +
          `<div class="stg-row"><div class="stg-row-label">Setup
            <span class="stg-hint" title="patreon.com/portal/registration/register-clients">ⓘ</span>
          </div><div class="stg-row-value muted">Optional. Enables Patreon-locked VOD pulls from creators you support.</div></div>`),
      ].join("");
    }

    case "plugins": {
      // Plugin manager. Lists every shipped plugin with a per-plugin
      // enable toggle bound to plugins.<name>.enabled, plus an 'Open'
      // CTA that deep-links into the plugin's own page (when one
      // exists) or the marketplace catalog card otherwise. Pre-existing
      // Archiver per-knob settings stay in their own group below.
      const toggles = s.plugin_toggles || {};
      // PLUGIN_REGISTRY is the same set the Plugins hub + marketplace
      // share — lives at the bottom of spa.js. category drives the
      // sub-group heading.
      const groups = {};
      for (const meta of PLUGIN_REGISTRY) {
        (groups[meta.category] ||= []).push(meta);
      }
      const enabledFor = (name) => {
        const t = toggles[name];
        return t == null ? true : t.enabled !== false;
      };
      const pluginRow = (meta) => {
        const open = meta.route
          ? `<a href="${htmlEscape(meta.route)}" class="stg-linkbtn">Open →</a>`
          : `<a href="#/plugins" class="stg-linkbtn">View in hub →</a>`;
        return `
          <div class="stg-row stg-plugin-row" data-plugin-name="${htmlEscape(meta.name)}">
            <div class="stg-row-label">
              <span class="stg-plugin-name">${htmlEscape(meta.label)}</span>
              <span class="stg-hint" title="${htmlEscape(meta.description)}">ⓘ</span>
              <span class="stg-plugin-tags">
                ${meta.proGated ? '<span class="cfg-badge ok" title="Strivo Pro plugin">Pro</span>' : ""}
                ${meta.installed === false ? '<span class="cfg-badge warn">not installed</span>' : ""}
              </span>
            </div>
            <div class="stg-row-value stg-plugin-actions">
              ${toggle(`plugins.${meta.name}.enabled`, enabledFor(meta.name))}
              <button class="sm stg-plugin-size" type="button" data-plugin="${htmlEscape(meta.name)}" title="View disk usage of this plugin's stored data">📦 Size</button>
              <button class="sm danger stg-plugin-clear" type="button" data-plugin="${htmlEscape(meta.name)}" title="Delete this plugin's stored data on disk. Cannot be undone.">🗑 Clear</button>
              ${open}
            </div>
          </div>`;
      };
      const archiverExtras = `
        <details class="stg-plugin-details">
          <summary>Archiver advanced</summary>
          ${row("Archive directory",
            textInput("archiver.archive_dir", arc.archive_dir, "/path/to/archives"),
            "Where archived VODs land. Defaults under the main recording dir.")}
          ${row("Format",
            textInput("archiver.format", arc.format, "best"),
            "yt-dlp format selector. Default targets bestvideo+bestaudio with a sensible cap.")}
          ${row("Concurrent fragments", numInput("archiver.concurrent_fragments", arc.concurrent_fragments ?? 4, 1, 16),
            "yt-dlp -N flag. 1–16; higher = faster but more rate-limit pressure.")}
        </details>`;
      const sections = Object.keys(groups).sort().map((cat) =>
        group(cat, groups[cat].map(pluginRow).join("") + (cat === "Archive" ? archiverExtras : ""))
      ).join("");
      return sections;
    }

    case "interface":
      return [
        group("Layout", [
          row("Top-nav order",
            `<div class="stg-reorder" data-reorder-key="strivo-layout-topnav" data-default='${JSON.stringify(["library","recordings","schedule","pipelines","plugins","watch","chat","history","logs","system","settings"]).replace(/'/g, "&apos;")}'><div class="stg-reorder-list"></div><button class="sm stg-reorder-reset" type="button">Reset</button></div>`,
            "Drag entries up/down to reorder the top navigation bar. Order persists locally."),
          row("Rail platform order",
            `<div class="stg-reorder" data-reorder-key="strivo-layout-rail-platforms" data-default='${JSON.stringify(["Twitch","YouTube","Patreon"]).replace(/'/g, "&apos;")}'><div class="stg-reorder-list"></div><button class="sm stg-reorder-reset" type="button">Reset</button></div>`,
            "Group the live-channel rail by platform in your preferred order. Order persists locally."),
          row("Recordings group-by default",
            `<select class="stg-layout-select" data-layout-key="strivo-layout-rec-groupby">
              <option value="channel">By channel</option>
              <option value="platform">By platform</option>
              <option value="date">By date</option>
              <option value="state">By state</option>
              <option value="none">Flat list</option>
            </select>`,
            "Default group-by applied when you open Recordings."),
          row("Plugin hub category order",
            `<div class="stg-reorder" data-reorder-key="strivo-layout-plugin-cats" data-default='${JSON.stringify(["Editor","Publish","Viewer","Analytics","Archive","Transcription","Reports"]).replace(/'/g, "&apos;")}'><div class="stg-reorder-list"></div><button class="sm stg-reorder-reset" type="button">Reset</button></div>`,
            "Reorder how categories appear when the plugin hub or Settings → Plugins groups by category."),
        ].join("")),
        group("Onboarding", [
          row("Welcome tour",
            `<button class="sm" id="stg-replay-tour" type="button">Replay tour</button>`,
            "Walk through the topbar one stop at a time. Useful after a major UI change."),
          row("Per-page hints",
            `<button class="sm" id="stg-reset-hints" type="button">Reset dismissed hints</button>`,
            "Make every per-page hint banner show up again on the next visit."),
        ].join("")),
        group("Accessibility", [
          row("Reduce motion", toggle("ui.reduce_motion", ui.reduce_motion),
            "Disables non-essential transitions across the UI. Mirrors the OS-level prefers-reduced-motion."),
          row("Verbose status", toggle("ui.verbose_status", ui.verbose_status),
            "Adds extra status text to long-running operations. Useful on screen readers."),
        ].join("")),
        group("Scheduling", [
          row("Scheduled recordings", `${(s.schedule || []).length}`,
            "Cron-style fixed-time recordings. Edit via TUI."),
        ].join("")),
      ].join("");

    case "advanced":
      return [
        group("Daemon", [
          row("IPC socket", code("~/.local/share/strivo/strivo.sock"),
            "Unix socket the web UI uses to talk to the daemon. Path is fixed."),
          row("Persist DB", code("~/.local/share/strivo/jobs.db"),
            "Recording history + retry queue. SQLite."),
          row("Log file", code("~/.local/share/strivo/strivo.<date>.log"),
            "Rolling daily log. See the Logs page for live tail."),
        ].join("")),
        group("Developer", [
          row("Dev unlock", code(envOrDefault("STRIVO_DEV_UNLOCK_ALL", "off")),
            "Set STRIVO_DEV_UNLOCK_ALL=1 in the daemon's environment to bypass all Strivo Pro gating. Use during plugin development; never in shipped builds."),
        ].join("")),
      ].join("");

    case "about":
    default:
      return [
        group("Build", [
          row("Application", "StriVo",
            "Live-stream PVR for Twitch and YouTube."),
          // Source link points at the home docs site to survive the
          // private-repo flip (audit U19). chorosyne.com → strivo will
          // 404 today but won't link to a 404'd github repo after the
          // visibility flip.
          row("Project", `<a href="https://chorosyne.com" class="stg-linkbtn" target="_blank" rel="noopener">chorosyne.com →</a>`),
          row("Plugins", `<a href="#/plugins" class="stg-linkbtn">Plugin hub →</a>`),
        ].join("")),
        group("Licence", [
          row("Strivo Pro", `<a href="#/plugins" class="stg-linkbtn">Manage entitlement →</a>`,
            "One-time $25 unlock for every shipped plugin. Activate or start a 3-day trial from the Plugins hub."),
        ].join("")),
      ].join("");
  }
}

// Each platform's wizard form spec: the fields it needs + a docs link
// the modal renders below the inputs. Kept tiny so it's obvious what
// each platform asks for; if it grows we lift it to its own module.
const PLATFORM_SPECS = {
  twitch: {
    title: "Configure Twitch",
    docsLabel: "Twitch Developer Console",
    docsUrl: "https://dev.twitch.tv/console/apps",
    fields: [
      { name: "client_id", label: "Client ID", type: "text", required: true },
      { name: "client_secret", label: "Client Secret", type: "password", required: true },
    ],
    notes: "Register Your Application → type 'Other', OAuth Redirect URL <code>http://localhost:8181/oauth/twitch</code>.",
  },
  youtube: {
    title: "Configure YouTube",
    docsLabel: "Google Cloud Console",
    docsUrl: "https://console.cloud.google.com/apis/credentials",
    fields: [
      { name: "client_id", label: "OAuth Client ID", type: "text", required: true },
      { name: "client_secret", label: "OAuth Client Secret", type: "password", required: true },
      { name: "cookies_path", label: "Cookies file (optional)", type: "text", placeholder: "/path/to/cookies.txt" },
      { name: "websub_callback_url", label: "WebSub callback URL (optional)", type: "url", placeholder: "https://your.tld/yt-websub" },
    ],
    notes: "Create OAuth 2.0 client ID, application type <em>Desktop app</em>. Cookies file enables age-restricted + member-only VODs.",
  },
  patreon: {
    title: "Configure Patreon",
    docsLabel: "Patreon Platform",
    docsUrl: "https://www.patreon.com/portal/registration/register-clients",
    fields: [
      { name: "client_id", label: "Client ID", type: "text", required: true },
      { name: "client_secret", label: "Client Secret", type: "password", required: true },
      { name: "cookies_path", label: "Cookies file (optional)", type: "text", placeholder: "/path/to/cookies.txt" },
    ],
    notes: "Cookies file is your logged-in patreon.com session — required to download VOD posts.",
  },
};

function openPlatformWizard(platform) {
  const spec = PLATFORM_SPECS[platform];
  if (!spec) return;
  const fieldHtml = spec.fields
    .map(
      (f) => `
        <label class="modal-field">
          <span class="modal-field-label">${htmlEscape(f.label)}${f.required ? " *" : ""}</span>
          <input class="modal-input" name="${htmlEscape(f.name)}" type="${htmlEscape(f.type)}"
            ${f.required ? "required" : ""}
            ${f.placeholder ? `placeholder="${htmlEscape(f.placeholder)}"` : ""} />
        </label>`,
    )
    .join("");
  const dlg = document.createElement("div");
  dlg.className = "modal-backdrop";
  dlg.innerHTML = `
    <form class="modal" role="dialog" aria-labelledby="pf-title">
      <header class="modal-head">
        <h2 id="pf-title">${htmlEscape(spec.title)}</h2>
        <button type="button" class="modal-close" aria-label="Close">×</button>
      </header>
      <div class="modal-body">
        ${fieldHtml}
        <p class="modal-notes">${spec.notes}
          <a href="${htmlEscape(spec.docsUrl)}" target="_blank" rel="noopener">${htmlEscape(spec.docsLabel)} →</a>
        </p>
      </div>
      <footer class="modal-foot">
        <button type="button" class="btn-ghost modal-cancel">Cancel</button>
        <button type="submit" class="btn-primary">Save</button>
      </footer>
    </form>`;
  document.body.appendChild(dlg);
  const close = () => dlg.remove();
  dlg.querySelector(".modal-close").addEventListener("click", close);
  dlg.querySelector(".modal-cancel").addEventListener("click", close);
  dlg.addEventListener("click", (e) => { if (e.target === dlg) close(); });
  dlg.querySelector(".modal").addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = {};
    spec.fields.forEach((f) => {
      body[f.name] = e.target.elements[f.name].value.trim();
    });
    try {
      await API.setPlatform(platform, body);
      Toast.success(`${spec.title.replace("Configure ", "")} saved`);
      close();
      // Re-render the Settings page so the status badge flips green.
      render();
    } catch (err) {
      Toast.error(`Couldn't save: ${err.message}`);
    }
  });
  // Autofocus the first field for a keyboard-driven flow.
  dlg.querySelector(".modal-input")?.focus();
}

// envOrDefault is a UI-side helper: the daemon doesn't expose env vars
// to the client (it shouldn't — it's behind auth on the local box, but
// minimising attack surface anyway). Until we add a /api/v1/env route
// in Phase 3, surface the placeholder.
function envOrDefault(_name, dflt) {
  return `<span class="muted">${htmlEscape(dflt)}</span>`;
}

// ── System (item 7) — version, daemon connectivity, severity-tiered
// health checks, disk gauge, tasks. (research §E)
async function renderSystem() {
  const [health, storage, checksResp, settings] = await Promise.all([
    API.health().catch(() => null),
    API.storage().catch(() => null),
    API.healthChecks().catch(() => null),
    API.settings().catch(() => null),
  ]);
  root.removeAttribute("aria-busy");

  // Server-side health-check registry is the single source of truth
  // (roadmap item 13): {domain, name, severity, message, fix}.
  const serverChecks = (checksResp && checksResp.checks) || [
    { domain: "Network", name: "Daemon IPC", severity: "error", message: "not reachable", fix: "" },
  ];
  const checks = serverChecks.map((c) => ({ sev: c.severity, label: c.name, msg: c.message }));
  const activeRec = recCache.filter((r) => isInProgress(r.state)).length;

  const sevGlyph = { ok: "✓", warn: "▲", error: "✕" };
  // Group rows by domain so related checks (Storage / Platform Auth /
  // Network) sit together, each with its remediation hint.
  const domains = [...new Set(serverChecks.map((c) => c.domain))];
  const healthRows = domains
    .map((domain) => {
      const rows = serverChecks
        .filter((c) => c.domain === domain)
        .map(
          (c) => {
            // Surface a Re-authenticate link on platform-auth checks
            // (audit M9). When the token's healthy, the link is hidden;
            // either way the Settings wizard is one click away.
            const lc = c.name.toLowerCase();
            const reauth =
              c.domain === "Platform Auth" && ["twitch", "youtube", "patreon"].includes(lc)
                ? ` <a class="sys-reauth" href="#/settings/platforms" title="Open the ${lc} setup wizard">Re-authenticate →</a>`
                : "";
            return `
    <div class="sys-check ${c.severity}">
      <span class="sys-sev">${sevGlyph[c.severity] || "•"}</span>
      <span class="sys-label">${htmlEscape(c.name)}</span>
      <span class="sys-msg">${htmlEscape(c.message)}${c.fix ? ` <span class="sys-fix">— ${htmlEscape(c.fix)}</span>` : ""}${reauth}</span>
    </div>`;
          },
        )
        .join("");
      return `<div class="sys-domain"><h3 class="sys-domain-title">${htmlEscape(domain)}</h3>${rows}</div>`;
    })
    .join("");

  // Segmented disk-usage gauge (Task 2 — storage gauge near disk-space health row).
  // Three segments: recordings (accent), other filesystem usage (muted), free (transparent).
  const gauge = storage && storage.filesystem_total_bytes
    ? (() => {
        const total = storage.filesystem_total_bytes;
        const recBytes = storage.bytes_used_by_recordings || 0;
        const avail = storage.filesystem_avail_bytes || 0;
        const other = Math.max(0, total - avail - recBytes);
        const recPct = Math.min(100, (recBytes / total) * 100);
        const otherPct = Math.min(100 - recPct, (other / total) * 100);
        const freePct = Math.max(0, (avail / total) * 100);
        const warnClass = (avail / total) < 0.05 ? "gauge-crit" : (avail / total) < 0.15 ? "gauge-warn" : "";
        return `
          <div class="sys-gauge-wrap${warnClass ? ` ${warnClass}` : ""}" title="Disk usage — ${freePct.toFixed(0)}% free">
            <div class="sys-gauge-seg" style="width:${recPct.toFixed(1)}%;background:var(--accent)" title="Recordings: ${formatBytes(recBytes)}"></div>
            <div class="sys-gauge-seg" style="width:${otherPct.toFixed(1)}%;background:var(--muted);opacity:0.5" title="Other: ${formatBytes(other)}"></div>
          </div>
          <div class="sys-gauge-legend">
            <span class="sys-gauge-dot" style="background:var(--accent)"></span> Recordings: ${formatBytes(recBytes)}
            &ensp;<span class="sys-gauge-dot" style="background:var(--muted);opacity:0.6"></span> Other: ${formatBytes(other)}
            &ensp;<span class="sys-gauge-dot" style="background:var(--border-light)"></span> Free: ${formatBytes(avail)} of ${formatBytes(total)}
          </div>`;
      })()
    : '<div class="empty sm">Disk stats unavailable</div>';

  const worst = checks.some((c) => c.sev === "error")
    ? "error"
    : checks.some((c) => c.sev === "warn")
    ? "warn"
    : "ok";

  root.innerHTML = chrome(`
    <h1 class="page-title">System</h1>
    <p class="page-subtitle">StriVo v${health ? htmlEscape(health.version || "?") : "?"} ·
      overall <span class="cfg-badge ${worst === "ok" ? "ok" : worst === "warn" ? "warn" : "err"}">${worst}</span></p>
    <div class="cfg-grid">
      <section class="cfg-card">
        <h2 class="cfg-title">Health</h2>
        <div class="sys-checks">${healthRows}</div>
        ${gauge}
      </section>
      <section class="cfg-card" id="backup-card">
        <h2 class="cfg-title">Backup</h2>
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Config + jobs DB</span>
            <span class="task-cadence">on-demand snapshot</span>
          </div>
          <button id="backup-now" class="sm">＋ Backup now</button>
        </div>
        <div id="backup-list"><div class="empty sm">Loading backups…</div></div>
      </section>
      <section class="cfg-card" id="blocklist-card">
        <h2 class="cfg-title">Blocklist</h2>
        <div id="blocklist-list"><div class="empty sm">Loading blocklist…</div></div>
      </section>
      <section class="cfg-card">
        <h2 class="cfg-title">Tasks</h2>
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Channel poll</span>
            <span class="task-cadence">every
              <input id="poll-interval" type="number" min="15" max="86400" step="5"
                     value="${settings ? settings.poll_interval_secs : 60}"
                     aria-label="Poll interval seconds" /> s
              <button id="poll-interval-save" class="sm" title="Apply poll interval">Save</button>
            </span>
          </div>
          <button id="task-poll-now" class="sm" title="Run the channel poll now">↻ Run now</button>
        </div>
        ${(settings && settings.schedule && settings.schedule.length
          ? settings.schedule
          : []
        )
          .map(
            (s) => `
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">⏱ ${htmlEscape(s.channel || "scheduled")}</span>
            <span class="task-cadence">${htmlEscape(s.cron || "")}${s.duration ? ` · ${htmlEscape(s.duration)}` : ""}</span>
          </div>
        </div>`,
          )
          .join("")}
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Active recordings</span>
            <span class="task-cadence">${activeRec} running${activeRec ? " · stop from the dashboard" : ""}</span>
          </div>
          <a class="sm" href="#/library">View</a>
        </div>
      </section>
    </div>
  `);
  setupChromeHandlers();
  // Run-now duality: poll task enqueues the same command as the scheduled poll.
  document.getElementById("task-poll-now")?.addEventListener("click", async (e) => {
    await withBusy(e.currentTarget, "Polling…", async () => {
      await API.pollNow();
      Toast.success("Channel poll triggered");
    }).catch((err) => Toast.error(`Poll failed: ${err.message}`));
  });
  // Live-editable poll interval (item 14b) + inline field validation (item 25).
  document.getElementById("poll-interval-save")?.addEventListener("click", async (e) => {
    const input = document.getElementById("poll-interval");
    const raw = parseInt(input?.value, 10);
    if (!Number.isFinite(raw) || raw < 15) {
      input?.setAttribute("aria-invalid", "true");
      Toast.error("Poll interval must be at least 15 seconds");
      return;
    }
    input?.removeAttribute("aria-invalid");
    await withBusy(e.currentTarget, "Saving…", async () => {
      const r = await API.setPollInterval(raw);
      Toast.success(`Poll interval set to ${r.poll_interval_secs}s`);
    }).catch((err) => Toast.error(`Failed: ${err.message}`));
  });
  // Backup/restore (item 16).
  document.getElementById("backup-now")?.addEventListener("click", async (e) => {
    await withBusy(e.currentTarget, "Backing up…", async () => {
      const r = await API.backupCreate();
      Toast.success(`Backup created — ${r.name}`);
      await paintBackups();
    }).catch((err) => Toast.error(`Backup failed: ${err.message}`));
  });
  paintBackups();
  paintBlocklist();
}

async function paintBlocklist() {
  const el = document.getElementById("blocklist-list");
  if (!el) return;
  try {
    const r = await API.blocklist();
    const rows = r.blocklist || [];
    if (!rows.length) {
      el.innerHTML = '<div class="empty sm">Nothing blocked.</div>';
      return;
    }
    el.innerHTML = rows
      .map((b) => {
        const scope = b.vod_id ? `VOD ${htmlEscape(b.vod_id)}` : "whole channel";
        return `
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(b.platform)} · ${htmlEscape(b.channel_id)}</span>
          <span class="task-cadence">${scope}${b.reason ? ` · ${htmlEscape(b.reason)}` : ""}</span>
        </div>
        <button class="sm unblock" data-platform="${htmlEscape(b.platform)}"
                data-channel="${htmlEscape(b.channel_id)}" data-vod="${htmlEscape(b.vod_id || "")}">Unblock</button>
      </div>`;
      })
      .join("");
    el.querySelectorAll(".unblock").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const d = btn.dataset;
        try {
          await API.blockRemove({
            platform: d.platform,
            channel_id: d.channel,
            vod_id: d.vod || null,
          });
          Toast.success("Unblocked");
          paintBlocklist();
        } catch (e) {
          Toast.error(`Unblock failed: ${e.message}`);
        }
      });
    });
  } catch (e) {
    el.innerHTML = `<div class="empty sm">Could not load blocklist: ${htmlEscape(e.message)}</div>`;
  }
}

async function paintBackups() {
  const el = document.getElementById("backup-list");
  if (!el) return;
  try {
    const r = await API.backups();
    const rows = r.backups || [];
    if (!rows.length) {
      el.innerHTML = '<div class="empty sm">No backups yet.</div>';
      return;
    }
    el.innerHTML = rows
      .map(
        (b) => `
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(b.name)}</span>
          <span class="task-cadence">${formatBytes(b.bytes || 0)} · ${(b.files || []).map(htmlEscape).join(", ")}</span>
        </div>
        <a class="sm" href="/api/v1/backups/${encodeURIComponent(b.name)}/download"
           download="strivo-backup-${htmlEscape(b.name)}.tar.gz"
           title="Download backup as tarball">Download</a>
        <button class="sm restore-backup" data-name="${htmlEscape(b.name)}">Restore</button>
      </div>`,
      )
      .join("");
    el.querySelectorAll(".restore-backup").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const name = btn.dataset.name;
        if (
          !(await confirmDialog(
            `Restore config + jobs DB from ${name}? This overwrites the current files; restart the daemon to apply.`,
            { ok: "Restore", danger: true },
          ))
        )
          return;
        try {
          const res = await API.backupRestore(name);
          Toast.success(`Restored ${(res.restored || []).join(", ")} — restart the daemon to apply`);
        } catch (err) {
          Toast.error(`Restore failed: ${err.message}`);
        }
      });
    });
  } catch (e) {
    el.innerHTML = `<div class="empty sm">Could not load backups: ${htmlEscape(e.message)}</div>`;
  }
}

// ── Logs viewer ──────────────────────────────────────────────────────
// Tails the rolling log file. Level dropdown + per-source filter chips +
// free-text search + multi-line entry collapse (audit U5, M7, R5).
let logsLevel = "info";
let logsQuery = "";
let logsSourceFilter = ""; // crate/module substring; "" = all sources
let logsFollow = localStorage.getItem("strivo-logs-follow") === "1";
let logsRegex = localStorage.getItem("strivo-logs-regex") === "1";
let logsFollowTimer = null;

// A "log entry" is a starting line (parsable timestamp + level) plus any
// following indented/JSON-blob continuation lines. We collapse those
// continuation lines into a single click-to-expand block so YouTube
// quota 403s stop dominating the viewport.
function parseLogEntries(lines) {
  const entries = [];
  const startRe = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/;
  for (const raw of lines) {
    if (startRe.test(raw)) {
      entries.push({ head: raw, tail: [] });
    } else if (entries.length) {
      entries[entries.length - 1].tail.push(raw);
    } else {
      entries.push({ head: raw, tail: [] });
    }
  }
  return entries;
}

// Pull a coarse "source" tag (crate::module) out of a log line head.
function logSource(line) {
  const m = line.match(/\s+(strivo_[a-zA-Z_]+(::[a-zA-Z_]+)*)/);
  return m ? m[1] : "";
}

async function renderLogs() {
  const levels = ["error", "warn", "info", "debug", "trace"];
  const options = levels
    .map((l) => `<option value="${l}"${l === logsLevel ? " selected" : ""}>${l.toUpperCase()}</option>`)
    .join("");
  // Stop any prior tail-follow timer before mounting the page (route
  // navigation, theme change, hot reload, etc.).
  if (logsFollowTimer) { clearInterval(logsFollowTimer); logsFollowTimer = null; }
  root.innerHTML = chrome(`
    <h1 class="page-title">Logs</h1>
    <div class="logs-toolbar">
      <label>Min level <select id="logs-level">${options}</select></label>
      <input id="logs-search" class="logs-search" type="search"
             placeholder="${logsRegex ? "Regex (case-insensitive)…" : "Search log text…"}"
             value="${htmlEscape(logsQuery)}" />
      <label class="logs-daterange" title="Filter log lines by ISO-8601 timestamp prefix. Inclusive of the bounds.">
        from <input id="logs-from" class="logs-date" type="datetime-local" step="1" value="${htmlEscape(logsFrom || "")}"/>
        to <input id="logs-to" class="logs-date" type="datetime-local" step="1" value="${htmlEscape(logsTo || "")}"/>
        <button id="logs-clear-range" class="sm" type="button" title="Clear date range">✕</button>
      </label>
      <label class="logs-toggle" title="Search as case-insensitive regex">
        <input type="checkbox" id="logs-regex" ${logsRegex ? "checked" : ""}/> regex
      </label>
      <label class="logs-toggle" title="Auto-refresh every 4s and pin scroll to bottom">
        <input type="checkbox" id="logs-follow" ${logsFollow ? "checked" : ""}/> follow
      </label>
      <span id="logs-sources" class="logs-sources"></span>
      <button id="logs-refresh" class="sm" title="Reload now">↻ Refresh</button>
      <button id="logs-copy" class="sm" title="Copy filtered log lines to clipboard">⧉ Copy</button>
      <button id="logs-download" class="sm" title="Save filtered log lines as a .log file">⬇ Download</button>
      <span id="logs-file" class="logs-file"></span>
    </div>
    <div id="logs-output" class="logs-output" aria-live="polite">Loading…</div>
  `);
  setupChromeHandlers();

  async function load() {
    const out = document.getElementById("logs-output");
    const fileEl = document.getElementById("logs-file");
    try {
      const r = await API.logs(logsLevel, 500);
      const allLines = r.lines || [];
      const allEntries = parseLogEntries(allLines);
      // Build the source-filter chip set from what's currently in view.
      const sources = [...new Set(allEntries.map((e) => logSource(e.head)).filter(Boolean))].sort();
      const chips = document.getElementById("logs-sources");
      if (chips) {
        chips.innerHTML = ['<button class="logs-chip" data-src="">all</button>']
          .concat(
            sources.map(
              (s) =>
                `<button class="logs-chip${s === logsSourceFilter ? " is-active" : ""}" data-src="${htmlEscape(s)}">${htmlEscape(s.replace(/^strivo_/, ""))}</button>`,
            ),
          )
          .join("");
        chips.querySelectorAll(".logs-chip").forEach((b) => {
          b.addEventListener("click", () => {
            logsSourceFilter = b.dataset.src || "";
            load();
          });
        });
      }
      const q = logsQuery.trim();
      // Regex compile once per load. Invalid pattern → tooltip via input
      // border colour + skip the filter (don't silently exclude
      // everything when the user mistypes).
      let pattern = null;
      let patternBad = false;
      if (q && logsRegex) {
        try { pattern = new RegExp(q, "i"); }
        catch (_) { patternBad = true; }
      }
      const searchInput = document.getElementById("logs-search");
      if (searchInput) searchInput.classList.toggle("logs-search-bad", patternBad);
      const qLower = q.toLowerCase();
      const filtered = allEntries.filter((e) => {
        if (logsSourceFilter && !e.head.includes(logsSourceFilter)) return false;
        if (!logInRange(e.head)) return false;
        if (!q || patternBad) return true;
        const hay = e.head + "\n" + e.tail.join("\n");
        if (pattern) return pattern.test(hay);
        return hay.toLowerCase().includes(qLower);
      });
      // Linkify UUID-shaped trace ids in escaped head HTML so users can
      // click one to filter the view. We do the escape first, then
      // splice in <a> elements; safe because the UUID regex contains
      // no HTML metachars.
      const TRACE_RE = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
      const linkifyTraces = (escapedHead) =>
        escapedHead.replace(TRACE_RE, (id) =>
          `<a class="logs-trace" href="#" data-trace="${id}" title="Filter by trace id ${id}">${id}</a>`,
        );
      out.innerHTML = filtered.length
        ? filtered
            .map((e) => {
              const head = linkifyTraces(htmlEscape(e.head));
              if (!e.tail.length) return `<div class="log-line">${head}</div>`;
              const tail = htmlEscape(e.tail.join("\n"));
              return `<details class="log-line log-multi"><summary>${head} <span class="log-more">+${e.tail.length}</span></summary><pre>${tail}</pre></details>`;
            })
            .join("")
        : "<div class='empty sm'>No log lines match the current filters.</div>";
      out.querySelectorAll(".logs-trace").forEach((a) => {
        a.addEventListener("click", (e) => {
          e.preventDefault();
          logsTraceId = a.dataset.trace || "";
          logsQuery = logsTraceId;
          const si = document.getElementById("logs-search");
          if (si) si.value = logsTraceId;
          load();
          Toast.success(`Filtering by trace id ${logsTraceId.slice(0, 8)}…`);
        });
      });
      if (fileEl) fileEl.textContent = r.file ? `· ${r.file} · ${filtered.length}/${allEntries.length} entries` : "";
      // Pin scroll to bottom in follow mode UNLESS the user has
      // intentionally scrolled up (we treat "within 80px of bottom"
      // as still-following so the auto-pin doesn't fight a fast scroll
      // recovery).
      const userPaused = out.scrollHeight - out.scrollTop - out.clientHeight > 80;
      if (!logsFollow || !userPaused) out.scrollTop = out.scrollHeight;
      // Stash for Copy/Download handlers.
      logsLastFilteredText = filtered
        .map((e) => e.tail.length ? `${e.head}\n${e.tail.join("\n")}` : e.head)
        .join("\n");
      logsLastFile = r.file || "strivo.log";
    } catch (e) {
      out.textContent = `Failed to load logs: ${e.message}`;
    }
  }
  document.getElementById("logs-level")?.addEventListener("change", (e) => {
    logsLevel = e.target.value;
    load();
  });
  document.getElementById("logs-search")?.addEventListener("input", (e) => {
    logsQuery = e.target.value;
    load();
  });
  document.getElementById("logs-from")?.addEventListener("change", (e) => {
    logsFrom = e.target.value; load();
  });
  document.getElementById("logs-to")?.addEventListener("change", (e) => {
    logsTo = e.target.value; load();
  });
  document.getElementById("logs-clear-range")?.addEventListener("click", () => {
    logsFrom = ""; logsTo = "";
    const f = document.getElementById("logs-from");
    const t = document.getElementById("logs-to");
    if (f) f.value = "";
    if (t) t.value = "";
    load();
  });
  document.getElementById("logs-regex")?.addEventListener("change", (e) => {
    logsRegex = e.target.checked;
    localStorage.setItem("strivo-logs-regex", logsRegex ? "1" : "0");
    // Re-render so the placeholder copy updates; load() also re-runs to
    // apply the new pattern interpretation against the cached entries.
    renderLogs().catch(() => {});
  });
  document.getElementById("logs-follow")?.addEventListener("change", (e) => {
    logsFollow = e.target.checked;
    localStorage.setItem("strivo-logs-follow", logsFollow ? "1" : "0");
    if (logsFollow) {
      if (logsFollowTimer) clearInterval(logsFollowTimer);
      logsFollowTimer = setInterval(() => { if (!document.hidden) load(); }, 4000);
    } else if (logsFollowTimer) {
      clearInterval(logsFollowTimer);
      logsFollowTimer = null;
    }
  });
  document.getElementById("logs-refresh")?.addEventListener("click", load);
  document.getElementById("logs-copy")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(logsLastFilteredText || "");
      Toast.success("Logs copied to clipboard");
    } catch (err) {
      Toast.error(`Copy failed: ${err.message}`);
    }
  });
  document.getElementById("logs-download")?.addEventListener("click", () => {
    const blob = new Blob([logsLastFilteredText || ""], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = logsLastFile || "strivo.log";
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  await load();
  // Auto-arm follow if it was previously enabled.
  if (logsFollow) {
    logsFollowTimer = setInterval(() => { if (!document.hidden) load(); }, 4000);
  }
}

// Cache the last-rendered filtered text so Copy/Download don't have to
// re-walk the DOM. Updated inside renderLogs.load().
let logsLastFilteredText = "";
// Date-range filter for /logs. Both are ISO-prefix strings the user
// picked from the datetime-local inputs; empty string = unbounded.
let logsFrom = "";
let logsTo = "";
// Trace-id click-to-filter: when a clickable token is clicked we
// set logsQuery to the trace id and re-render. Stored separately so
// the user can clear it independently.
let logsTraceId = "";

// Parse a log line head into an ISO timestamp prefix (e.g.
// "2026-05-28T22:13:01"). Returns null when nothing recognisable.
function logLineIsoStamp(head) {
  const m = head.match(/(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})/);
  return m ? m[1] : null;
}

// Match a single log line against the active date range (inclusive).
// Empty bounds skip the check on that side.
function logInRange(head) {
  if (!logsFrom && !logsTo) return true;
  const stamp = logLineIsoStamp(head);
  if (!stamp) return false; // structureless lines drop when range is set
  if (logsFrom && stamp < logsFrom) return false;
  if (logsTo && stamp > logsTo) return false;
  return true;
}
let logsLastFile = "strivo.log";

// ── Upcoming agenda (item 18) — first-class calendar of known upcoming
// recordings. Source = scheduled (cron) entries with their server-computed
// next_fire. (Platform-side scheduled broadcasts aren't available via API.) ──
function dayBucket(d) {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const that = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diff = Math.round((that - today) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Tomorrow";
  return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
}

async function renderSchedule() {
  // Page lives at #/schedule for back-compat with bookmarks, but is now
  // the Monitor page — record-when-live and auto-download new uploads
  // replace the cron form that 95% of users found foreign. (Power users
  // can still add cron entries via config.toml's [[schedule]] table;
  // they show up in the "Cron schedule" group below when present.)
  let monitor = { auto_record: [], auto_download: [] };
  let channels = [];
  let cronEntries = [];
  let settings = {};
  let health = {};
  try {
    const [m, c, s, st, h] = await Promise.all([
      API.monitor().catch(() => ({ auto_record: [], auto_download: [] })),
      API.channels().then((r) => r.channels || []).catch(() => []),
      API.schedule().then((r) => r.schedule || []).catch(() => []),
      API.settings().catch(() => ({})),
      API.health().catch(() => ({})),
    ]);
    monitor = {
      auto_record: Array.isArray(m?.auto_record) ? m.auto_record : [],
      auto_download: Array.isArray(m?.auto_download) ? m.auto_download : [],
    };
    channels = c;
    cronEntries = s;
    settings = st;
    health = h;
  } catch (_) {}
  root.removeAttribute("aria-busy");

  // Build a channel lookup so we can show display_name + platform.
  const channelByKey = new Map(
    channels.map((c) => [`${c.platform}:${c.id}`, c]),
  );
  const channelByName = new Map(
    channels.map((c) => [(c.display_name || c.name || "").toLowerCase(), c]),
  );
  const channelsAvailableForDownload = channels.filter(
    (c) => c.platform === "YouTube",
  );

  // Capture profiles drive the per-channel override selects + tier badges
  // below; the option lists are built per-row (with the current value
  // pre-selected) inside the map.
  const captureProfiles = (settings.capture_profiles || []);

  // Quality-tier lookup map from capture profiles (Task 1).
  const profileTierMap = new Map(captureProfiles.map((p) => [p.name, p.quality_tier || ""]));
  const TIER_LABEL = { best: "Best", "1080p": "1080p", "720p": "720p", "480p": "480p", audio_only: "Audio only" };

  // Section 1 — record when live (existing auto-record list).
  const recordRows = monitor.auto_record
    .map(
      (e) => {
        const curContainer = e.format || "";
        const curProfile = e.profile || "";
        const curTier = curProfile ? (profileTierMap.get(curProfile) || "") : "";
        // Rebuild option lists with selected=true on the current values.
        const containerOpts = ["", "mkv", "mp4", "ts"]
          .map((c) => `<option value="${htmlEscape(c)}"${c === curContainer ? " selected" : ""}>${htmlEscape(c || "(default)")}</option>`)
          .join("");
        const profileOpts = ["", ...captureProfiles.map((p) => p.name)]
          .map((p) => `<option value="${htmlEscape(p)}"${p === curProfile ? " selected" : ""}>${htmlEscape(p || "(default)")}</option>`)
          .join("");
        const tierBadge = curTier
          ? `<span class="mon-tier-badge" data-key="${htmlEscape(e.key)}">${htmlEscape(TIER_LABEL[curTier] || curTier)}</span>`
          : `<span class="mon-tier-badge" data-key="${htmlEscape(e.key)}"></span>`;
        return `
    <div class="task-row">
      <div class="task-info">
        <span class="task-name">${htmlEscape(e.channel_name || e.channel_id)} <span class="mon-plat plat-${htmlEscape(e.platform.toLowerCase())}">${htmlEscape(e.platform)}</span></span>
        <span class="task-cadence">${htmlEscape(e.key)}</span>
        <span class="mon-fmt-row">
          <label class="mon-fmt-label" title="Container override for this channel">Container</label>
          <select class="mon-fmt-sel" data-key="${htmlEscape(e.key)}" data-field="format"
                  title="Per-channel container override (empty = global default)">${containerOpts}</select>
          <label class="mon-fmt-label" title="Named capture profile for this channel">Profile</label>
          <select class="mon-fmt-sel" data-key="${htmlEscape(e.key)}" data-field="profile"
                  title="Per-channel capture profile (empty = global default)">${profileOpts}</select>
          ${tierBadge}
        </span>
      </div>
      <button class="sm mon-rec-rm" data-key="${htmlEscape(e.key)}" title="Stop auto-recording this channel">✕</button>
    </div>`;
      },
    )
    .join("");

  // Section 2 — auto-download new uploads (YouTube only).
  const downloadRows = monitor.auto_download
    .map((e) => {
      const ch = channelByKey.get(e.key);
      const name = ch ? (ch.display_name || ch.name) : e.channel_id;
      const playlistsValue = (e.playlists || []).join(", ");
      return `
      <div class="task-row mon-dl-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(name)} <span class="mon-plat plat-${htmlEscape(e.platform.toLowerCase())}">${htmlEscape(e.platform)}</span></span>
          <span class="task-cadence">
            <label class="mon-scope">
              <span>Limit to playlists (optional, comma-separated)</span>
              <input class="mon-playlists" type="text" data-key="${htmlEscape(e.key)}"
                     placeholder="PLxxx, PLyyy — leave empty for whole channel"
                     value="${htmlEscape(playlistsValue)}" />
            </label>
          </span>
        </div>
        <button class="sm mon-dl-rm" data-key="${htmlEscape(e.key)}" title="Stop auto-downloading uploads from this channel">✕</button>
      </div>`;
    })
    .join("");

  // Auto-download (Archiver tandem) is Creator Edition only — the PVR daemon
  // mounts no archiver routes, so the whole section is hidden there.
  const downloadSectionHtml = CREATOR_ENABLED ? `
    <section class="cfg-card">
      <h2 class="cfg-title">Auto-download new uploads</h2>
      <p class="mon-help">Pulls new uploads from a YouTube channel as the monitor sees them. Leave the playlist field empty for the whole channel, or paste one or more playlist IDs to limit scope.</p>
      ${downloadRows || '<div class="empty sm">No channels are set to auto-download yet.</div>'}
      <form id="mon-dl-add" class="mon-add">
        <select id="mon-dl-channel">
          <option value="">Pick a YouTube channel…</option>
          ${channelsAvailableForDownload
            .map((c) => `<option value="${htmlEscape(`${c.platform}:${c.id}`)}">${htmlEscape(c.display_name || c.name)}</option>`)
            .join("")}
        </select>
        <button class="btn-primary" type="submit">Enable</button>
      </form>
    </section>` : "";

  // Cron schedule section — kept for power users who already use it,
  // collapsed by default. Empty unless config.toml has entries.
  const cronGroup = cronEntries.length
    ? `<details class="mon-cron"><summary>Advanced cron schedule (${cronEntries.length})</summary>
        ${cronEntries
          .map(
            (e, i) => `
          <div class="task-row">
            <div class="task-info">
              <span class="task-name">${htmlEscape(e.channel || "scheduled")}</span>
              <span class="task-cadence"><code>${htmlEscape(e.cron || "")}</code>${e.duration ? ` · ${htmlEscape(e.duration)}` : ""}${e.next_fire ? ` · next: ${htmlEscape(new Date(e.next_fire).toLocaleString())}` : ""}</span>
            </div>
            <button class="sm sch-del" data-i="${i}" title="Delete this cron entry">✕</button>
          </div>`,
          )
          .join("")}
        <p class="mon-cron-hint">Cron entries are added via <code>~/.config/strivo/config.toml</code> under <code>[[schedule]]</code>. They fire at the cron expression's next match regardless of live state — useful for predictable shows on platforms without a live API. Most users want the simpler primitives above.</p>
      </details>`
    : "";

  // Get-channel-name helper for the Add forms — match by case-insensitive
  // display name, fall back to "Platform:id" parsing.
  const resolveChannelKey = (raw) => {
    const t = raw.trim();
    if (!t) return null;
    if (t.includes(":")) return t;
    const c = channelByName.get(t.toLowerCase());
    return c ? `${c.platform}:${c.id}` : null;
  };

  // Live capture status: active recordings + disk free + current limits.
  // Recordings cache is shared across the SPA so we can read in-progress
  // count without a separate fetch.
  const activeCount = recCache.filter((r) => isInProgress(r.state)).length;
  const limits = settings.monitor_limits || {};
  const maxConcurrent = limits.max_concurrent_recordings || 0;
  const diskBudgetGb = limits.disk_budget_reserved_gb || 0;
  const diskAvailBytes = (health.disk && health.disk.filesystem_avail_bytes) || 0;
  const diskTotalBytes = (health.disk && health.disk.filesystem_total_bytes) || 0;
  const availPct = diskTotalBytes > 0 ? (diskAvailBytes / diskTotalBytes) * 100 : 0;
  // Reserve budget vs free: warn when free < reserved + 5 GB headroom.
  const reservedBytes = diskBudgetGb * 1024 * 1024 * 1024;
  const diskOverBudget = reservedBytes > 0 && diskAvailBytes < reservedBytes;
  const concurrentSaturated = maxConcurrent > 0 && activeCount >= maxConcurrent;
  const statusBanner = (concurrentSaturated || diskOverBudget)
    ? `<div class="mon-status-banner warn">
         ${concurrentSaturated ? `<span>⚠ Concurrent cap hit: ${activeCount}/${maxConcurrent} recordings in flight — new live captures will queue.</span>` : ""}
         ${diskOverBudget ? `<span>⚠ Disk free (${formatBytes(diskAvailBytes)}) is below the reserved ${diskBudgetGb} GB — new captures will defer.</span>` : ""}
       </div>`
    : `<div class="mon-status-banner ok">
         <span>✓ ${activeCount} recording${activeCount === 1 ? "" : "s"} in flight${maxConcurrent ? ` / ${maxConcurrent}` : ""} · ${formatBytes(diskAvailBytes)} free</span>
       </div>`;

  root.innerHTML = chrome(`
    <h1 class="page-title">Monitor</h1>
    <p class="page-subtitle">Channels StriVo is watching. Record live broadcasts as they happen, or auto-download new YouTube uploads.</p>

    ${buildCalStrip(cronEntries)}

    ${statusBanner}

    <section class="cfg-card">
      <h2 class="cfg-title">Capture limits <a href="#/settings/notifications" class="stg-linkbtn" style="margin-left:auto;font-size:0.78em">Configure go-live banners →</a></h2>
      <p class="mon-help">Safety knobs that defer new captures when StriVo is already busy or disk is tight. Zero in either field disables that cap.</p>
      <div class="mon-limits-grid">
        <label class="mon-limit">
          <span class="mon-limit-label">Max concurrent recordings</span>
          <input class="mon-limit-input" type="number" min="0" max="64" step="1"
                 id="mon-limit-concurrent" value="${maxConcurrent}" />
          <span class="mon-limit-hint">${maxConcurrent === 0 ? "unlimited" : `${activeCount} of ${maxConcurrent} in use`}</span>
        </label>
        <label class="mon-limit">
          <span class="mon-limit-label">Reserved disk budget (GB)</span>
          <input class="mon-limit-input" type="number" min="0" max="100000" step="1"
                 id="mon-limit-disk" value="${diskBudgetGb}" />
          <span class="mon-limit-hint">${diskBudgetGb === 0 ? "no circuit breaker" : diskOverBudget ? "ENGAGED" : "armed"}</span>
        </label>
        <div class="mon-disk-gauge" title="Recording filesystem usage">
          <span class="mon-disk-label">Free disk</span>
          <div class="mon-disk-bar"><div class="mon-disk-fill" style="width:${(100 - availPct).toFixed(1)}%"></div></div>
          <span class="mon-disk-meta">${formatBytes(diskAvailBytes)} free of ${formatBytes(diskTotalBytes)}</span>
        </div>
      </div>
    </section>

    <section class="cfg-card">
      <h2 class="cfg-title">Record when live</h2>
      <p class="mon-help">Twitch and YouTube live broadcasts capture automatically. Add channels from the topbar's <em>+ Add channel</em>, then enable Auto-record on the channel card.</p>
      ${recordRows || '<div class="empty sm">No channels are set to record-when-live yet.</div>'}
    </section>

    ${downloadSectionHtml}

    ${cronGroup}
  `);
  setupChromeHandlers();

  // Capture-limit inputs — debounced save to /settings/update so each
  // keystroke doesn't fire a round-trip. Repaint on save so the gauge
  // and banner reflect the new state.
  const wireLimit = (id, path, max) => {
    const el = document.getElementById(id);
    if (!el) return;
    let timer;
    el.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(async () => {
        const v = Math.max(0, Math.min(max, parseInt(el.value, 10) || 0));
        if (v !== parseInt(el.value, 10)) el.value = v;
        try {
          await API.updateSetting(path, v);
          Toast.success(`Saved · ${path}`);
          renderSchedule().catch(() => {});
        } catch (err) {
          Toast.error(`Save failed: ${err.message}`);
        }
      }, 600);
    });
  };
  wireLimit("mon-limit-concurrent", "monitor_limits.max_concurrent_recordings", 64);
  wireLimit("mon-limit-disk", "monitor_limits.disk_budget_reserved_gb", 100000);

  // Record-when-live row delete.
  document.querySelectorAll(".mon-rec-rm").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!confirm("Stop auto-recording this channel?")) return;
      try {
        await API.toggleAutoRecord(btn.dataset.key, false);
        Toast.success("Stopped");
        renderSchedule();
      } catch (e) {
        Toast.error(`Couldn't stop: ${e.message}`);
      }
    });
  });

  // Per-channel format/profile override — save on change (Task 4).
  // Quality-tier badge data is embedded as JSON for JS lookup after render.
  const monProfileTierData = {};
  document.querySelectorAll(".mon-tier-badge").forEach((badge) => {
    const profSel = badge.closest(".mon-fmt-row")?.querySelector('.mon-fmt-sel[data-field="profile"]');
    if (profSel) monProfileTierData[badge.dataset.key] = { badge, profSel };
  });
  document.querySelectorAll(".mon-fmt-sel").forEach((sel) => {
    sel.addEventListener("change", async () => {
      const key = sel.dataset.key;
      const taskRow = sel.closest(".task-row");
      if (!taskRow) return;
      const fmtSel = taskRow.querySelector('.mon-fmt-sel[data-field="format"]');
      const profSel = taskRow.querySelector('.mon-fmt-sel[data-field="profile"]');
      // Update quality-tier badge if profile changed.
      if (sel.dataset.field === "profile") {
        const tierBadge = taskRow.querySelector(".mon-tier-badge");
        if (tierBadge) {
          const tierRaw = profSel ? (window._monProfileTiers || {})[profSel.value] || "" : "";
          const TIER_LABEL_JS = { best: "Best", "1080p": "1080p", "720p": "720p", "480p": "480p", audio_only: "Audio only" };
          tierBadge.textContent = tierRaw ? (TIER_LABEL_JS[tierRaw] || tierRaw) : "";
        }
      }
      try {
        await API.setAutoRecordFormat(key, fmtSel ? fmtSel.value : "", profSel ? profSel.value : "");
        Toast.success("Format saved");
      } catch (err) {
        Toast.error(`Format save failed: ${err.message}`);
      }
    });
  });
  // Expose profile→tier map for the change handler above.
  window._monProfileTiers = Object.fromEntries(
    (settings?.capture_profiles || []).map((p) => [p.name, p.quality_tier || ""])
  );

  // Auto-download row delete + playlist edits.
  document.querySelectorAll(".mon-dl-rm").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!confirm("Stop auto-downloading new uploads from this channel?")) return;
      try {
        await API.setArchiverTandem(btn.dataset.key, false);
        Toast.success("Stopped");
        renderSchedule();
      } catch (e) {
        Toast.error(`Couldn't stop: ${e.message}`);
      }
    });
  });
  // Debounced save on playlist field changes — split on comma/space.
  document.querySelectorAll(".mon-playlists").forEach((inp) => {
    let timer;
    inp.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(async () => {
        const key = inp.dataset.key;
        const playlists = inp.value
          .split(/[\s,]+/)
          .map((s) => s.trim())
          .filter(Boolean);
        try {
          await API.setArchiverPlaylists(key, playlists);
          Toast.success("Playlists saved");
        } catch (e) {
          Toast.error(`Couldn't save: ${e.message}`);
        }
      }, 600);
    });
  });

  // Add new auto-download channel.
  document.getElementById("mon-dl-add")?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const key = document.getElementById("mon-dl-channel").value;
    if (!key) return;
    try {
      await API.setArchiverTandem(key, true);
      Toast.success("Enabled");
      renderSchedule();
    } catch (err) {
      Toast.error(`Couldn't enable: ${err.message}`);
    }
  });
  // Cron entry delete (still works for power users).
  document.querySelectorAll(".sch-del").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const i = parseInt(btn.dataset.i, 10);
      if (!confirm("Delete this cron entry?")) return;
      try {
        await API.scheduleDelete(i);
        Toast.success("Removed");
        renderSchedule();
      } catch (err) {
        Toast.error(`Couldn't delete: ${err.message}`);
      }
    });
  });
  // Silence the unused channel-lookup helper warning when no channels
  // happen to be queried — kept for future quick-add by name.
  void resolveChannelKey;
}

// Legacy cron-form renderer retained for ref but unused — kept as a
// no-op to avoid breaking any externally-cached bookmarks of the old
// shape. Power users still add cron entries via config.toml.
// eslint-disable-next-line no-unused-vars
async function _renderSchedule_legacy_cron_unused() {
  let entries = [];
  try {
    const r = await API.schedule();
    entries = r.schedule || [];
  } catch (_) {}
  root.removeAttribute("aria-busy");

  const dated = entries
    .filter((e) => e.next_fire)
    .map((e) => ({ ...e, when: new Date(e.next_fire) }))
    .sort((a, b) => a.when - b.when);
  const undated = entries.filter((e) => !e.next_fire);

  // Group by day bucket, preserving sorted order.
  const groups = [];
  for (const e of dated) {
    const label = dayBucket(e.when);
    let g = groups.find((x) => x.label === label);
    if (!g) {
      g = { label, items: [] };
      groups.push(g);
    }
    g.items.push(e);
  }

  const row = (e) => `
    <div class="task-row">
      <div class="task-info">
        <span class="task-name">${htmlEscape(e.channel || "scheduled")}</span>
        <span class="task-cadence">${htmlEscape(e.cron || "")}${e.duration ? ` · ${htmlEscape(e.duration)}` : ""}</span>
      </div>
      <span class="agenda-time">${e.when ? e.when.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : ""}</span>
    </div>`;

  const groupsHtml = groups
    .map(
      (g) => `
    <section class="cfg-card">
      <h2 class="cfg-title">${htmlEscape(g.label)}</h2>
      ${g.items.map(row).join("")}
    </section>`,
    )
    .join("");

  const undatedHtml = undated.length
    ? `<section class="cfg-card">
         <h2 class="cfg-title">Unscheduled</h2>
         ${undated.map((e) => `<div class="task-row"><div class="task-info"><span class="task-name">${htmlEscape(e.channel || "")}</span><span class="task-cadence">${htmlEscape(e.cron || "")} · unparsed cron</span></div></div>`).join("")}
       </section>`
    : "";

  const empty = !entries.length
    ? '<div class="empty">No scheduled recordings yet. Add one below.</div>'
    : "";

  const listHtml = entries
    .map(
      (e, i) => `
    <div class="task-row">
      <div class="task-info">
        <span class="task-name">${htmlEscape(e.channel || "scheduled")}</span>
        <span class="task-cadence"><code>${htmlEscape(e.cron || "")}</code>${e.duration ? ` · ${htmlEscape(e.duration)}` : ""}${e.next_fire ? ` · next: ${htmlEscape(new Date(e.next_fire).toLocaleString())}` : ""}</span>
      </div>
      <button class="sm sch-del" data-i="${i}" title="Delete this schedule entry">✕</button>
    </div>`,
    )
    .join("");

  root.innerHTML = chrome(`
    <h1 class="page-title">Schedule</h1>
    <p class="page-subtitle">Upcoming scheduled recordings · ${dated.length} upcoming</p>
    ${empty}
    <section class="cfg-card">
      <h2 class="cfg-title">Add scheduled recording</h2>
      <form id="sch-add" class="sch-form">
        <label class="sch-field">
          <span>Channel</span>
          <input name="channel" type="text" placeholder="Platform:channel_id (e.g. Twitch:12345)" required />
        </label>
        <label class="sch-field">
          <span>Cron <span class="stg-hint" title="5-field cron: minute hour day-of-month month day-of-week. Example: 0 9 * * 1-5 = 9am weekdays.">ⓘ</span></span>
          <input name="cron" type="text" placeholder="0 9 * * 1-5" required />
        </label>
        <label class="sch-field">
          <span>Duration</span>
          <input name="duration" type="text" placeholder="4h" />
        </label>
        <button class="btn-primary" type="submit">Add</button>
      </form>
    </section>
    <div class="cfg-grid">${groupsHtml}${undatedHtml}</div>
    ${entries.length ? `<section class="cfg-card"><h2 class="cfg-title">All schedule entries</h2>${listHtml}</section>` : ""}
  `);
  setupChromeHandlers();
  document.getElementById("sch-add")?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const fd = new FormData(e.target);
    try {
      await API.scheduleAdd({
        channel: fd.get("channel"),
        cron: fd.get("cron"),
        duration: fd.get("duration"),
      });
      Toast.success("Schedule entry added");
      renderSchedule();
    } catch (err) {
      Toast.error(`Add failed: ${err.message}`);
    }
  });
  document.querySelectorAll(".sch-del").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const i = parseInt(btn.dataset.i, 10);
      if (!confirm("Delete this schedule entry?")) return;
      try {
        await API.scheduleDelete(i);
        Toast.success("Schedule entry removed");
        renderSchedule();
      } catch (err) {
        Toast.error(`Delete failed: ${err.message}`);
      }
    });
  });
}

// ── Durable History (item 17) — completed/failed audit from the jobs DB,
// survives restarts (unlike the in-memory /recordings snapshot). ──
// History page filter / group state — persisted like the Recordings ones.
let histFilter = "";
let histGroupBy = localStorage.getItem("strivo-hist-groupby") || "none"; // "none" | "channel" | "date"
// Date heatmap click-day filter — "YYYY-MM-DD" or "" for unset.
let histDay = "";
let histStateFilter = new Set(
  (localStorage.getItem("strivo-hist-state-filter") || "")
    .split(",").filter(Boolean),
);
let histCache = [];
let histNextCursor = null;
let histTotal = 0;
let histRenderLimit = 200;

async function renderHistory() {
  // Fetch history alongside the live /recordings snapshot so we can
  // overlay file_exists state (audit B4). Without this, History happily
  // reports 'Finished, 9 GB' for files the Recordings page knows are
  // long gone.
  let [hist, recs] = [[], []];
  try {
    const [h, r] = await Promise.all([
      API.history().catch(() => ({ history: [] })),
      API.recordings().catch(() => ({ recordings: [] })),
    ]);
    hist = h.history || [];
    histNextCursor = h.next_cursor ?? null;
    histTotal = h.total ?? hist.length;
    recs = r.recordings || [];
  } catch (_) {}
  const liveById = new Map(recs.map((r) => [r.id, r]));
  histCache = hist.map((row) => {
    const live = liveById.get(row.id);
    if (live && live.file_exists === false) {
      return { ...row, file_exists: false, state: "Failed" };
    }
    return row;
  });
  root.removeAttribute("aria-busy");

  if (histCache.length === 0) {
    root.innerHTML = chrome(`
      <h1 class="page-title">History</h1>
      <div class="empty">
        <div class="glyph">🗂</div>
        No recording history yet. Captures land here automatically.
      </div>
    `);
    setupChromeHandlers();
    return;
  }

  root.innerHTML = chrome(`
    <h1 class="page-title">History</h1>
    <p class="page-subtitle" id="hist-count"></p>
    <div id="hist-heatmap"></div>
    <div class="rec-toolbar">
      <input id="hist-filter" class="grid-filter" type="search"
             placeholder="Filter by channel or title…"
             aria-label="Filter history" value="${htmlEscape(histFilter)}">
      <button id="hist-groupby" class="sm" title="Group rows">
        ${histGroupBy === "channel" ? "▼ Grouped by channel"
          : histGroupBy === "date" ? "▼ Grouped by month"
          : "≣ Group by…"}
      </button>
      ${histDay ? `<button id="hist-clear-day" class="sm" type="button" title="Clear day filter">✕ ${htmlEscape(histDay)}</button>` : ""}
    </div>
    <div id="hist-state-chips" class="rec-state-chips" role="group" aria-label="Filter by state"></div>
    <div id="hist-list" class="media-list"></div>
    ${histNextCursor != null ? `<button id="hist-load-more" class="button secondary" type="button">Load more history</button>` : ""}
  `);
  setupChromeHandlers();
  paintHistHeatmap();
  paintHistChips();
  paintHistory();
  document.getElementById("hist-clear-day")?.addEventListener("click", () => {
    histDay = "";
    renderHistory().catch((e) => Toast.error(e.message));
  });

  document.getElementById("hist-filter")?.addEventListener("input", (e) => {
    histFilter = e.target.value;
    paintHistory();
  });
  document.getElementById("hist-groupby")?.addEventListener("click", () => {
    histGroupBy = histGroupBy === "none"
      ? "channel"
      : histGroupBy === "channel" ? "date" : "none";
    localStorage.setItem("strivo-hist-groupby", histGroupBy);
    renderHistory().catch((e) => Toast.error(e.message));
  });
  document.getElementById("hist-load-more")?.addEventListener("click", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = "Loading…";
    try {
      const page = await API.history({ cursor: histNextCursor, limit: 200 });
      histCache.push(...(page.history || []));
      histNextCursor = page.next_cursor ?? null;
      histTotal = page.total ?? histCache.length;
      if (histNextCursor == null) button.remove();
      else {
        button.disabled = false;
        button.textContent = "Load more history";
      }
      paintHistHeatmap();
      paintHistChips();
      paintHistory();
    } catch (error) {
      button.disabled = false;
      button.textContent = "Load more history";
      Toast.error(`History load failed: ${error.message}`);
    }
  });
}

function paintHistChips() {
  const host = document.getElementById("hist-state-chips");
  if (!host) return;
  const counts = new Map();
  for (const r of histCache) {
    const key = stateClassName(r.state);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  if (counts.size <= 1) { host.innerHTML = ""; return; }
  const sorted = Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  const allActive = histStateFilter.size === 0;
  host.innerHTML = `
    <button class="rec-state-chip rec-state-chip-all ${allActive ? "active" : ""}" type="button">
      <span class="rec-state-chip-dot"></span>All <span class="rec-state-chip-count">${histCache.length}</span>
    </button>
    ${sorted.map(([state, n]) => {
      const active = histStateFilter.size === 0 || histStateFilter.has(state);
      return `<button class="rec-state-chip state-${htmlEscape(state)} ${active ? "active" : ""}"
                data-state="${htmlEscape(state)}" type="button">
        <span class="rec-state-chip-dot"></span>
        ${htmlEscape(stateChipLabel(state))}
        <span class="rec-state-chip-count">${n}</span>
      </button>`;
    }).join("")}`;
  host.querySelector(".rec-state-chip-all")?.addEventListener("click", () => {
    histStateFilter.clear();
    localStorage.setItem("strivo-hist-state-filter", "");
    paintHistChips();
    paintHistory();
  });
  host.querySelectorAll("[data-state]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const s = btn.dataset.state;
      if (histStateFilter.size === 0) histStateFilter = new Set([s]);
      else if (histStateFilter.has(s)) histStateFilter.delete(s);
      else histStateFilter.add(s);
      localStorage.setItem("strivo-hist-state-filter",
        Array.from(histStateFilter).join(","));
      paintHistChips();
      paintHistory();
    });
  });
}

// GitHub-style calendar heatmap above the history list: last 12 weeks
// of recording activity. Each day cell is colour-scaled by the
// recording count that day. Click a cell to set histDay and filter.
function paintHistHeatmap() {
  const host = document.getElementById("hist-heatmap");
  if (!host) return;
  const counts = new Map();
  for (const r of histCache) {
    const d = (r.started_at || "").slice(0, 10);
    if (!d) continue;
    counts.set(d, (counts.get(d) || 0) + 1);
  }
  // Build a 12-week × 7-day grid ending today. Empty days render as
  // the lowest-tier colour so the grid stays visually anchored.
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const start = new Date(today);
  start.setDate(start.getDate() - 7 * 12 + 1);
  const max = Math.max(1, ...counts.values());
  const cells = [];
  for (let i = 0; i < 7 * 12; i++) {
    const d = new Date(start);
    d.setDate(d.getDate() + i);
    const iso = d.toISOString().slice(0, 10);
    const c = counts.get(iso) || 0;
    const tier = c === 0 ? 0 : Math.min(4, Math.ceil((c / max) * 4));
    cells.push({ iso, count: c, tier });
  }
  // Arrange column-major so each column is a week.
  const weeks = [];
  for (let w = 0; w < 12; w++) {
    weeks.push(cells.slice(w * 7, w * 7 + 7));
  }
  host.innerHTML = `
    <div class="hist-hm-wrap" title="Last 12 weeks of recording activity. Click a day to filter.">
      <div class="hist-hm-grid">
        ${weeks.map((col) => `<div class="hist-hm-col">${col.map((cell) => `
          <button class="hist-hm-cell hist-hm-t${cell.tier} ${histDay === cell.iso ? "active" : ""}"
                  data-day="${cell.iso}" type="button"
                  title="${cell.iso} · ${cell.count} recording${cell.count === 1 ? "" : "s"}"></button>`).join("")}</div>`).join("")}
      </div>
      <div class="hist-hm-legend">
        <span>less</span>
        ${[0,1,2,3,4].map((t) => `<span class="hist-hm-cell hist-hm-t${t}"></span>`).join("")}
        <span>more</span>
      </div>
    </div>`;
  host.querySelectorAll(".hist-hm-cell[data-day]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const day = btn.dataset.day;
      histDay = histDay === day ? "" : day;
      renderHistory().catch((e) => Toast.error(e.message));
    });
  });
}

function paintHistory() {
  const host = document.getElementById("hist-list");
  if (!host) return;
  const q = histFilter.trim().toLowerCase();
  const rows = histCache.filter((r) => {
    if (histStateFilter.size > 0 && !histStateFilter.has(stateClassName(r.state))) return false;
    if (histDay) {
      const d = (r.started_at || "").slice(0, 10);
      if (d !== histDay) return false;
    }
    if (!q) return true;
    return (r.channel_name || "").toLowerCase().includes(q)
        || niceTitle(r.stream_title).toLowerCase().includes(q);
  });
  // Newest-first inside each cluster + as the default flat order.
  rows.sort((a, b) => new Date(b.started_at) - new Date(a.started_at));
  const visibleRows = rows.slice(0, histRenderLimit);
  const countEl = document.getElementById("hist-count");
  if (countEl) {
    countEl.textContent = (q || histStateFilter.size > 0 || rows.length !== histCache.length)
      ? `${rows.length} matching · ${histCache.length} of ${histTotal} loaded`
      : `${histCache.length} of ${histTotal} entries loaded · durable across restarts`;
  }

  if (rows.length === 0) {
    host.innerHTML = `<div class="empty"><div class="glyph">🗂</div>No history rows match the current filter.</div>`;
    return;
  }
  let html;
  if (histGroupBy === "channel") {
    const order = [];
    const groups = new Map();
    for (const r of visibleRows) {
      const k = r.channel_name || "(unknown)";
      if (!groups.has(k)) { groups.set(k, []); order.push(k); }
      groups.get(k).push(r);
    }
    html = order.map((ch) => {
      const list = groups.get(ch);
      const totalBytes = list.reduce((a, b) => a + (b.bytes_written || 0), 0);
      return `<div class="hist-group">
        <div class="hist-group-head">
          <span class="rec-group-name">${htmlEscape(ch)}</span>
          <span class="rec-group-meta">${list.length} entr${list.length === 1 ? "y" : "ies"} · ${formatBytes(totalBytes)}</span>
        </div>
        ${list.map(historyPillHtml).join("")}
      </div>`;
    }).join("");
  } else if (histGroupBy === "date") {
    const order = [];
    const groups = new Map();
    for (const r of visibleRows) {
      const d = new Date(r.started_at);
      const k = isNaN(d.getTime()) ? "(unknown)"
        : `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,"0")}`;
      if (!groups.has(k)) { groups.set(k, []); order.push(k); }
      groups.get(k).push(r);
    }
    html = order.map((mo) => {
      const list = groups.get(mo);
      const totalBytes = list.reduce((a, b) => a + (b.bytes_written || 0), 0);
      const niceMonth = mo === "(unknown)" ? mo
        : new Date(mo + "-01").toLocaleString(undefined, { year: "numeric", month: "long" });
      return `<div class="hist-group">
        <div class="hist-group-head">
          <span class="rec-group-name">${htmlEscape(niceMonth)}</span>
          <span class="rec-group-meta">${list.length} entr${list.length === 1 ? "y" : "ies"} · ${formatBytes(totalBytes)}</span>
        </div>
        ${list.map(historyPillHtml).join("")}
      </div>`;
    }).join("");
  } else {
    html = visibleRows.map(historyPillHtml).join("");
  }
  if (visibleRows.length < rows.length) {
    html += `<button id="hist-show-more" class="button secondary" type="button">
      Show ${Math.min(200, rows.length - visibleRows.length)} more
    </button>`;
  }
  host.innerHTML = html;
  host.querySelector("#hist-show-more")?.addEventListener("click", () => {
    histRenderLimit += 200;
    paintHistory();
  });

  // Wire per-row buttons. Reuses the same handlers Recordings table
  // mounts so behaviours stay consistent.
  host.querySelectorAll("[data-action=rec-play]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const id = btn.dataset.jobId;
      if (id) window.location.hash = `#/watch?recording=${encodeURIComponent(id)}&fresh=1`;
    });
  });
  host.querySelectorAll("[data-action=rec-info]").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openRecordingInfo(btn.dataset.jobId);
    });
  });
  host.querySelectorAll("[data-action=rec-rescan]").forEach((btn) => {
    btn.addEventListener("click", (e) => { e.stopPropagation(); reScanRecording(btn); });
  });
  host.querySelectorAll("[data-action=rec-locate]").forEach((btn) => {
    btn.addEventListener("click", (e) => { e.stopPropagation(); showRecordingPath(btn.dataset.path); });
  });
  host.querySelectorAll("[data-action=rec-delete]").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!(await confirmDialog("Delete this recording? The file moves to the 7-day trash.", { ok: "Delete", danger: true })))
        return;
      await withBusy(btn, "Deleting…", async () => {
        await API.deleteRecordingFile(btn.dataset.jobId);
        Toast.success("Deleted");
        histCache = histCache.filter((r) => r.id !== btn.dataset.jobId);
        renderHistory().catch(() => {});
      }).catch((err) => Toast.error(`Delete failed: ${err.message}`));
    });
  });
  // Clicking anywhere on the pill (outside buttons) opens the Info
  // modal — same convention as the Recordings table.
  host.querySelectorAll(".media-pill").forEach((pill) => {
    pill.addEventListener("click", (e) => {
      if (e.target.closest("button, input, a")) return;
      const id = pill.dataset.jobId;
      if (id) openRecordingInfo(id);
    });
  });
}

// Action-rich pill used on the History page. Mirrors recordingPillHtml's
// layout but adds the Recordings-page Play/Info/Delete affordance set.
function historyPillHtml(j) {
  const when = j.started_at ? new Date(j.started_at).toLocaleString() : "—";
  const missingOverlay = j.file_exists === false
    ? '<span class="mp-missing">FILE MISSING</span>' : "";
  const sourceBadge = j.source_url
    ? '<span class="mp-source" title="From Twitch/YouTube VOD backfill">VOD</span>' : "";
  const isFinished = stateClassName(j.state) === "finished" && j.file_exists !== false;
  const isFileError = j.file_exists === false;
  const playBtn = isFinished
    ? `<button class="primary sm" data-action="rec-play" data-job-id="${htmlEscape(j.id)}" title="Open player">▶ Play</button>`
    : `<button class="primary sm rec-play-disabled" disabled aria-disabled="true" title="${isFileError ? "File missing" : "Not finished"}">▶ Play</button>`;
  const fileErrorBtns = isFileError
    ? `<button class="sm" data-action="rec-rescan" data-job-id="${htmlEscape(j.id)}" title="Re-check whether the file exists">↻ Re-scan</button>
       <button class="sm" data-action="rec-locate" data-job-id="${htmlEscape(j.id)}" data-path="${htmlEscape(j.output_path || "")}" title="Show the expected file path">📂 Show path</button>`
    : "";
  return `
    <div class="media-pill hist-pill${j.file_exists === false ? " mp-broken" : ""}"
         data-job-id="${htmlEscape(j.id)}">
      <div class="mp-thumb">${missingOverlay}<img class="mp-thumb-img" loading="lazy" alt=""
        src="/api/v1/recordings/${encodeURIComponent(j.id)}/thumb" onerror="this.remove()"></div>
      <div class="mp-info">
        <div class="mp-title">${htmlEscape(niceTitle(j.stream_title) || j.channel_name || "(recording)")} ${sourceBadge}</div>
        <div class="mp-sub">${htmlEscape(j.channel_name || "")} · ${htmlEscape(when)}</div>
      </div>
      <div class="mp-meta">
        ${(() => { const d = recordingDisplayState(j); return `<span class="state-pill ${d.className}">${htmlEscape(d.label)}</span>`; })()}
        <span class="mp-size">${formatBytes(j.bytes_written || 0)}</span>
      </div>
      <div class="hist-actions">
        ${playBtn}
        ${fileErrorBtns}
        <button class="sm" data-action="rec-info" data-job-id="${htmlEscape(j.id)}" title="Recording details">ⓘ Info</button>
        <button class="danger sm" data-action="rec-delete" data-job-id="${htmlEscape(j.id)}" title="Delete (moves file to 7-day trash)">✕</button>
      </div>
    </div>`;
}

// ── 7-day calendar strip (Task 1) ────────────────────────────────────
// Derives a stable hue [0,360) from a channel name by DJB2-style hash.
function channelHue(name) {
  let h = 5381;
  for (let i = 0; i < (name || "").length; i++)
    h = ((h << 5) + h + (name || "").charCodeAt(i)) | 0;
  return Math.abs(h) % 360;
}

// Build a compact 7-day strip of upcoming cron schedule firings.
// `entries` is the /schedule array with next_fire timestamps.
// Returns an HTML string (empty string when there are no upcoming firings).
function buildCalStrip(entries) {
  const upcoming = (entries || []).filter((e) => e.next_fire);
  if (!upcoming.length) return "";
  const now = new Date();
  const days = Array.from({ length: 7 }, (_, d) => {
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate() + d);
    const end = new Date(start);
    end.setDate(start.getDate() + 1);
    const label =
      d === 0 ? "Today" :
      d === 1 ? "Tomorrow" :
      start.toLocaleDateString([], { weekday: "short", month: "numeric", day: "numeric" });
    return { start, end, label, firings: [] };
  });
  for (const e of upcoming) {
    const t = new Date(e.next_fire);
    const day = days.find((d) => t >= d.start && t < d.end);
    if (day) day.firings.push(e);
  }
  const dayHtml = days.map(({ label, firings, start }) => {
    const isToday = start.toDateString() === now.toDateString();
    const blocks = firings.map((e) => {
      const hue = channelHue(e.channel || "");
      const time = new Date(e.next_fire).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      const tip = `${e.channel || ""}  ·  ${time}${e.duration ? "  ·  " + e.duration : ""}`;
      return `<a class="cal-block" href="#/schedule"
                 style="background:hsl(${hue},45%,32%);border-color:hsl(${hue},50%,48%)"
                 title="${htmlEscape(tip)}">${htmlEscape((e.channel || "").slice(0, 12))}</a>`;
    }).join("");
    return `<div class="cal-day${isToday ? " cal-today" : ""}">
      <div class="cal-day-label">${htmlEscape(label)}</div>
      <div class="cal-day-blocks">${blocks || '<span class="cal-no-fire">—</span>'}</div>
    </div>`;
  }).join("");
  return `<section class="cfg-card cal-section">
    <h2 class="cfg-title">Upcoming cron schedule <span class="cal-subtitle">(next 7 days)</span></h2>
    <div class="cal-strip">${dayHtml}</div>
  </section>`;
}

// ── Live-count ticker + concurrent-slot indicator ────────────────────
// Both pills sit in the topbar. The slot pill shows "N / M rec" where N
// is in-progress count and M is monitor_limits.max_concurrent_recordings.
// maxConcurrentSlots is populated by setupChromeHandlers from /settings.
/// Is this job a VOD/post download rather than a live capture? Downloads
/// carry the source they were pulled from; live captures have none.
function isDownloadJob(r) {
  return !!(r && r.source_url);
}

/// Refresh the top-bar activity pills from the recording cache.
///
/// Takes no argument on purpose: five call sites each recomputed the same
/// `recCache.filter(isInProgress).length`, and passing that single number
/// is what made a Patreon post download announce itself as "● LIVE". A
/// download occupies a capture slot — so it belongs in the slot pill — but
/// nothing about it is live.
function updateLiveCount() {
  const inProgress = (recCache || []).filter((r) => isInProgress(r.state));
  const downloads = inProgress.filter(isDownloadJob).length;
  const captures = inProgress.length - downloads;
  const n = inProgress.length; // slots consumed: the daemon caps on all of them

  const pill = document.getElementById("live-pill");
  if (pill) {
    if (captures > 0) {
      pill.textContent = `● LIVE: ${captures}`;
      pill.className = "live-pill";
      pill.title = `${captures} live capture${captures === 1 ? "" : "s"} in progress`;
      pill.style.display = "";
    } else if (downloads > 0) {
      // Still show activity, but do not call a download live.
      pill.textContent = `↓ ${downloads}`;
      pill.className = "live-pill live-pill-download";
      pill.title = `${downloads} download${downloads === 1 ? "" : "s"} in progress`;
      pill.style.display = "";
    } else {
      pill.style.display = "none";
    }
  }
  const slotPill = document.getElementById("rec-slot-pill");
  if (slotPill) {
    if (maxConcurrentSlots > 0) {
      slotPill.textContent = `${n} / ${maxConcurrentSlots} rec`;
      slotPill.style.display = "";
      const saturated = n >= maxConcurrentSlots;
      slotPill.className = `storage-pill${saturated ? " storage-pill-warn" : ""}`;
      slotPill.title = saturated
        ? `⚠ Concurrent cap hit: ${n}/${maxConcurrentSlots} — click to adjust`
        : `${n} of ${maxConcurrentSlots} capture slots in use (downloads count toward the cap) — click to manage`;
    } else if (n > 0) {
      slotPill.textContent = `${n} rec`;
      slotPill.style.display = "";
      slotPill.className = "storage-pill";
      slotPill.title = `${n} recording${n === 1 ? "" : "s"} in progress`;
    } else {
      slotPill.style.display = "none";
    }
  }
}

// ── Utilities ────────────────────────────────────────────────────────
function htmlEscape(s) {
  if (s == null) return "";
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
// md_to_html — tiny subset of markdown for Casebook section bodies.
// Handles **bold**, `code`, leading-dash unordered lists, and newlines.
// Not a full markdown parser — Casebook only emits a tight subset.
function md_to_html(text) {
  if (!text) return "";
  const escaped = htmlEscape(text);
  // Lists first: turn lines starting with "- " into <ul><li>.
  const lines = escaped.split("\n");
  const out = [];
  let inUl = false;
  for (const raw of lines) {
    if (raw.startsWith("- ")) {
      if (!inUl) {
        out.push("<ul>");
        inUl = true;
      }
      out.push(`<li>${raw.slice(2)}</li>`);
    } else {
      if (inUl) {
        out.push("</ul>");
        inUl = false;
      }
      out.push(raw + "<br/>");
    }
  }
  if (inUl) out.push("</ul>");
  return out
    .join("")
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/`([^`]+)`/g, "<code>$1</code>");
}

// niceTitle — strip filename-derived noise from a recording title so the
// UI shows the semantic title only. The on-disk filename is untouched.
//
// Strips:
//   - leading HHMMSS_ timestamp prefix from ffmpeg filename templates
//   - trailing API/source decorations like "_Video_", "[Video]", "_AUDIO_"
//   - underscores standing in for spaces (filesystem-safe substitution)
//   - editorial appendations Patreon/YouTube auto-tag (BONUS Video, etc.)
//   - bracketed/parens descriptors that are non-semantic
// Then collapses double-spaces and trims.
const TITLE_TRAILING_TAGS = [
  // Order matters: most specific (multi-word) first.
  "BONUS Video", "BONUS Audio", "BONUS [Video]", "BONUS [Audio]",
  "Full Episode", "Patreon Exclusive", "Patreon Only", "Members Only",
  "BONUS", "FREE", "EXCLUSIVE", "VOD",
  "_Video_", "[Video]", "_VIDEO_", "[VIDEO]",
  "_Audio_", "[Audio]", "_AUDIO_", "[AUDIO]",
];
function niceTitle(t) {
  if (t == null) return "";
  let s = String(t);
  // 4-6 digit timestamp prefix produced by {date}/{time} in the template.
  s = s.replace(/^\d{4,6}_+/, "");
  // Underscore → space (filename-safe substitution).
  s = s.replace(/_+/g, " ");
  // Strip each known trailing tag, repeatedly, with surrounding punctuation.
  for (let i = 0; i < 4; i++) {
    let before = s;
    for (const tag of TITLE_TRAILING_TAGS) {
      const re = new RegExp(
        "[\\s\\-\\u2013\\u2014:,\\(\\[]*" +
          tag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") +
          "[\\s\\)\\]]*$",
        "i",
      );
      s = s.replace(re, "");
    }
    if (s === before) break;
  }
  // Collapse double-spaces; tidy stray punctuation tails like " - " " — ".
  s = s.replace(/\s+/g, " ")
       .replace(/[\s\-–—:,]+$/g, "")
       .trim();
  return s;
}
function formatCount(n) {
  if (n >= 1000000) return (n / 1000000).toFixed(1) + "M";
  if (n >= 1000) return (n / 1000).toFixed(1) + "k";
  return String(n);
}
function formatBytes(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " GB";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " MB";
  if (n >= 1e3) return (n / 1e3).toFixed(0) + " KB";
  return n + " B";
}
// Compact relative age for the rail "last live" slot. Progression:
//   <1h  → "Xm ago"   (sub-hour, kept for usability on a freshly-offline row)
//   <1d  → "Xh ago"
//   <1mo → "Xd ago"
//   <1y  → "Xm ago"   or "Xm Yd ago" when there's a calendar-day remainder
//   ≥1y  → "Xy ago"   or "Xy Ym ago" when there's a calendar-month remainder
// Months and years are calendar-aware so leap years and short months don't lie.
function relTime(iso) {
  const past = new Date(iso);
  const t = past.getTime();
  if (!t) return "";
  const now = new Date();
  const secs = Math.max(0, Math.floor((now.getTime() - t) / 1000));
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;

  // Calendar diff (UTC) — match by year / month / day fields so DST and
  // leap years don't shift the answer by a day.
  let years = now.getUTCFullYear() - past.getUTCFullYear();
  let months = now.getUTCMonth() - past.getUTCMonth();
  let days = now.getUTCDate() - past.getUTCDate();
  if (days < 0) {
    months -= 1;
    // Borrow days from the previous calendar month.
    const prev = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 0));
    days += prev.getUTCDate();
  }
  if (months < 0) {
    years -= 1;
    months += 12;
  }

  if (years >= 1) return months > 0 ? `${years}y ${months}m ago` : `${years}y ago`;
  if (months >= 1) return days > 0 ? `${months}m ${days}d ago` : `${months}m ago`;
  return `${days}d ago`;
}

// Tooltip companion — absolute local timestamp, since the rail label already
// carries the relative form.
function lastLiveLong(iso) {
  const d = new Date(iso);
  if (!d.getTime()) return "unknown";
  return d.toLocaleString(undefined, {
    weekday: "short",
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

// ── W6 keyboard shortcuts ────────────────────────────────────────────
// Linear-/GitHub-style: prefix `g` then route letter to jump (gl/gr/gs
// etc.), `?` to open the help overlay, `Esc` to close, `a` to toggle
// the activity rail, `p` to trigger Poll.
let prefixActive = false;
let prefixTimer = null;

// Refetch caches when the tab regains focus and the rail looks emptied
// out (e.g. after the daemon socket bounced while the tab was idle). This
// is belt-and-braces alongside the Promise.allSettled fan-out above: the
// SSE reconnect handles the live-update channel, but a one-shot fetch is
// the cheapest way to reconcile a partial-fetch render that's already on
// screen. Cheap — the route render itself is idempotent.
document.addEventListener("visibilitychange", () => {
  if (document.hidden) return;
  if (channelCache.length === 0 || recCache.length === 0) {
    render();
  }
});

document.addEventListener("keydown", (e) => {
  // ⌘K / Ctrl+K — command palette. Handled before the input guard so it
  // works from anywhere, including while a field is focused. (W4-alt.)
  if ((e.metaKey || e.ctrlKey) && (e.key === "k" || e.key === "K")) {
    e.preventDefault();
    toggleCommandPalette();
    return;
  }
  // If the palette is open, it owns the keyboard.
  if (document.getElementById("cmdk")?.classList.contains("open")) {
    handleCmdkKey(e);
    return;
  }

  // Don't intercept while the user is typing. Catches:
  //   - native text fields (input / textarea)
  //   - contenteditable widgets (custom rich-text editors, chat compose
  //     boxes, anything with the global isContentEditable flag)
  //   - select dropdowns mid-keyboard-navigation
  const tag = (e.target.tagName || "").toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select") return;
  if (e.target.isContentEditable) return;
  if (e.ctrlKey || e.metaKey || e.altKey) return;

  // `/` focuses the recordings filter when on that route.
  if (e.key === "/" && currentRoute() === "recordings") {
    const f = document.getElementById("rec-filter");
    if (f) {
      e.preventDefault();
      f.focus();
      return;
    }
  }

  // Keyboard help: Shift+I (capital I). The earlier `?` binding was
  // collateral-fired by video-element native shortcuts inside the player
  // modal, leaving the help stuck visible behind it.
  if (e.shiftKey && (e.key === "I" || e.key === "i")) {
    e.preventDefault();
    document.getElementById("kbd-help")?.classList.add("open");
    return;
  }
  if (e.key === "Escape") {
    document.getElementById("kbd-help")?.classList.remove("open");
    if (selectedChannelKey) {
      selectedChannelKey = null;
      render();
    }
    return;
  }
  if (e.key === "p") {
    e.preventDefault();
    API.pollNow().catch(() => {});
    return;
  }
  if (e.key === "g" && !prefixActive) {
    prefixActive = true;
    prefixTimer = setTimeout(() => (prefixActive = false), 1000);
    return;
  }
  if (prefixActive) {
    clearTimeout(prefixTimer);
    prefixActive = false;
    const link = document.querySelector(`.topnav a[data-key="${e.key}"]`);
    if (link) {
      e.preventDefault();
      route(link.dataset.route);
    }
  }
});

// ── ⌘K command palette (W4-alt) ───────────────────────────────────────
let cmdkItems = [];
let cmdkSelected = 0;

function commandList() {
  const nav = [
    ["library", "Go to Home"],
    ["recordings", "Go to Recordings"],
    ["schedule", "Go to Schedule"],
    ["pipelines", "Go to Pipelines"],
    ["plugins", "Go to Plugins"],
    ["settings", "Go to Settings"],
    ["system", "Go to System"],
  ].map(([r, label]) => ({ label, run: () => route(r) }));
  const actions = [
    { label: "Poll channels now", run: () => API.pollNow().catch(() => {}) },
    {
      label: "Stop all recordings",
      run: () => API._fetch("/recordings/stop_all", { method: "POST" }).catch(() => {}),
    },
    { label: "Logout", run: () => API.logout().then(() => route("login")) },
  ];
  return [...nav, ...actions];
}

function toggleCommandPalette() {
  let el = document.getElementById("cmdk");
  if (!el) {
    el = document.createElement("div");
    el.id = "cmdk";
    el.className = "kbd-help";
    el.innerHTML = `
      <div class="card">
        <input id="cmdk-input" class="grid-filter" type="text"
               placeholder="Type a command…" autocomplete="off" aria-label="Command palette">
        <div id="cmdk-list" class="pl-list"></div>
      </div>`;
    document.body.appendChild(el);
    el.addEventListener("click", (ev) => {
      if (ev.target === el) el.classList.remove("open");
    });
    el.querySelector("#cmdk-input").addEventListener("input", paintCmdk);
  }
  const open = el.classList.toggle("open");
  if (open) {
    cmdkSelected = 0;
    const input = el.querySelector("#cmdk-input");
    input.value = "";
    paintCmdk();
    input.focus();
  }
}

function paintCmdk() {
  const q = (document.getElementById("cmdk-input")?.value || "")
    .trim()
    .toLowerCase();
  const all = commandList();
  cmdkItems = q
    ? all.filter((c) => c.label.toLowerCase().includes(q))
    : all;
  if (cmdkSelected >= cmdkItems.length) cmdkSelected = 0;
  const list = document.getElementById("cmdk-list");
  if (!list) return;
  list.innerHTML = cmdkItems
    .map(
      (c, i) =>
        `<div class="pl-row ${i === cmdkSelected ? "sel" : ""}" data-i="${i}">${htmlEscape(
          c.label,
        )}</div>`,
    )
    .join("");
  list.querySelectorAll(".pl-row").forEach((row) => {
    row.addEventListener("click", () => runCmdk(parseInt(row.dataset.i, 10)));
  });
}

function handleCmdkKey(e) {
  const el = document.getElementById("cmdk");
  if (e.key === "Escape") {
    e.preventDefault();
    el.classList.remove("open");
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    cmdkSelected = Math.min(cmdkSelected + 1, cmdkItems.length - 1);
    paintCmdk();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    cmdkSelected = Math.max(cmdkSelected - 1, 0);
    paintCmdk();
  } else if (e.key === "Enter") {
    e.preventDefault();
    runCmdk(cmdkSelected);
  }
}

function runCmdk(i) {
  const item = cmdkItems[i];
  document.getElementById("cmdk")?.classList.remove("open");
  if (item) item.run();
}

// ── Onboarding tour + per-page hint banners ──────────────────────────
// LocalStorage keys:
//   strivo-tour-done                → seen the welcome walkthrough
//   strivo-hint-<route>             → dismissed the per-page hint
// Hint copy is intentionally one line each — the goal is "I know what
// this surface is for", not full docs.
const PAGE_HINTS = {
  library:    "Live channels in the rail + the current capture dashboard. Click any rail row to see channel detail.",
  recordings: "Every recording past + present. Tick rows to enable bulk actions, click headers to sort, chips filter by state.",
  schedule:   "Per-channel record-when-live + auto-download switches. Capture limits + disk gauge live up top.",
  pipelines:  "Durable Creator workflows with live state, cancellation, retries, restart recovery, and capability blueprints.",
  watch:      "Tile any subset of currently live channels. Unmute one tile at a time; Shift+I shows shortcuts.",
  chat:       "Twitch IRC over WSS. Tab strip picks a room; filter chips narrow live. BTTV globals + Twitch emotes render as images.",
  plugins:    "Plugin hub. Each card opens the plugin; ⚙ deep-links to the per-plugin Settings panel.",
  settings:   "Live daemon config. Toggles persist to ~/.config/strivo/config.toml on change.",
  system:     "Health checks + storage gauge + platform-auth status + Backup/Restore.",
  logs:       "Rolling daemon log. Toggle Follow for tail mode; Copy / Download exports the filtered view.",
  history:    "Durable per-recording journal that survives daemon restarts.",
};

// Top-bar slots the tour walks. Order matches the natural left-to-right
// flow; each step pins to the corresponding .topnav-link by data-route.
const TOUR_STEPS = [
  { route: "library",    title: "Library",    body: "Your home. Live channel rail on the left; current captures + recent recordings in the centre." },
  { route: "recordings", title: "Recordings", body: "Every past + active recording in a sortable / filterable / groupable table. Bulk actions on selection." },
  { route: "schedule",   title: "Monitor",    body: "Tell StriVo which channels to auto-record + auto-download. Capture limits + disk-budget circuit breaker live here." },
  { route: "watch",      title: "Player", body: "Single + multi-stream player. Pick a preset (split-screen, split/quadrant, quadrant) or build a custom split layout. Drag channels from the rail into empty tiles; drag tiles to swap." },
  { route: "chat",       title: "Chat",       body: "Twitch IRC client with filter chips, mention highlighting, BTTV global emotes." },
  { route: "pipelines",  title: "Pipelines",  body: "Cross-plugin DAGs. Click a node to open it; 'Run on…' picks a recording + opens the right plugin." },
  { route: "plugins",    title: "Plugins",    body: "The shipped plugin set + marketplace catalog. Click any card to open; gear icon → per-plugin Settings." },
  { route: "settings",   title: "Settings",   body: "All daemon config: Notifications, Platforms, plugin enable/disable, theme, advanced paths." },
];

function tourDone() { return localStorage.getItem("strivo-tour-done") === "1"; }
function markTourDone() { localStorage.setItem("strivo-tour-done", "1"); }
function hintDismissed(route) { return localStorage.getItem(`strivo-hint-${route}`) === "1"; }
function dismissHint(route) { localStorage.setItem(`strivo-hint-${route}`, "1"); }

function startOnboardingTour() {
  if (tourDone()) return;
  let idx = 0;
  const overlay = document.createElement("div");
  overlay.id = "tour-overlay";
  overlay.className = "tour-overlay";
  document.body.appendChild(overlay);

  const paint = () => {
    const step = TOUR_STEPS[idx];
    const target = document.querySelector(`.topnav-link[data-route="${step.route}"]`);
    const rect = target?.getBoundingClientRect();
    const cardLeft = rect ? Math.max(12, Math.min(window.innerWidth - 380, rect.left + rect.width / 2 - 180)) : 24;
    const cardTop = rect ? rect.bottom + 12 : 80;
    overlay.innerHTML = `
      <div class="tour-spotlight" style="${rect ? `left:${rect.left - 6}px;top:${rect.top - 6}px;width:${rect.width + 12}px;height:${rect.height + 12}px;` : "display:none"}"></div>
      <div class="tour-card" style="left:${cardLeft}px;top:${cardTop}px;">
        <div class="tour-step-meta">Step ${idx + 1} of ${TOUR_STEPS.length}</div>
        <h3 class="tour-title">${htmlEscape(step.title)}</h3>
        <p class="tour-body">${htmlEscape(step.body)}</p>
        <div class="tour-actions">
          <button class="sm tour-skip" type="button">Skip tour</button>
          <span class="spacer"></span>
          ${idx > 0 ? `<button class="sm tour-prev" type="button">← Back</button>` : ""}
          <button class="btn-primary sm tour-next" type="button">
            ${idx === TOUR_STEPS.length - 1 ? "Finish" : "Next →"}
          </button>
        </div>
      </div>`;
    overlay.querySelector(".tour-skip").addEventListener("click", finish);
    overlay.querySelector(".tour-prev")?.addEventListener("click", () => { idx = Math.max(0, idx - 1); paint(); });
    overlay.querySelector(".tour-next").addEventListener("click", () => {
      if (idx >= TOUR_STEPS.length - 1) { finish(); return; }
      idx += 1;
      paint();
    });
  };
  const finish = () => {
    markTourDone();
    overlay.remove();
  };
  paint();
}

// Mount a per-page hint banner above the current route's main content
// IFF the user hasn't dismissed this route's hint yet. Idempotent —
// called after each render() and short-circuits when already mounted.
function maybeMountPageHint(route) {
  if (!route || hintDismissed(route) || !PAGE_HINTS[route]) return;
  if (document.getElementById("page-hint")) return;
  const banner = document.createElement("div");
  banner.id = "page-hint";
  banner.className = "page-hint";
  banner.innerHTML = `
    <span class="page-hint-icon" aria-hidden="true">💡</span>
    <span class="page-hint-text">${htmlEscape(PAGE_HINTS[route])}</span>
    <button class="page-hint-dismiss sm" type="button" aria-label="Dismiss this hint">✕</button>`;
  // Insert as the first child of the main chrome region so it sits
  // above any page-specific page-title / subtitle.
  const chrome = document.querySelector(".chrome");
  if (!chrome) return;
  chrome.insertBefore(banner, chrome.children[1] || null);
  banner.querySelector(".page-hint-dismiss").addEventListener("click", () => {
    dismissHint(route);
    banner.remove();
  });
}

function injectKeyboardHelp() {
  if (document.getElementById("kbd-help")) return;
  const div = document.createElement("div");
  div.id = "kbd-help";
  div.className = "kbd-help";
  div.setAttribute("role", "dialog");
  div.setAttribute("aria-label", "Keyboard shortcuts");
  // Multiple dismiss paths: click backdrop, click X, ESC anywhere.
  // The legacy version listened only for ESC; the user reported the
  // overlay was undismissable when stacked behind a modal (the modal
  // ate the ESC). Now ANY click on the backdrop closes it, the X
  // button is always visible, and a delegated capture-phase ESC
  // handler dismisses it before any modal can swallow the event.
  const close = () => div.classList.remove("open");
  div.addEventListener("click", (e) => { if (e.target === div) close(); });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && div.classList.contains("open")) { close(); }
  }, true); // capture so we run before any modal handler
  div.innerHTML = `
    <div class="card">
      <button class="kbd-help-close sm" type="button" aria-label="Close help">✕</button>
      <h2>Keyboard shortcuts</h2>
      <dl>
        <dt>Shift+I</dt><dd>This help</dd>
        <dt>⌘K</dt><dd>Command palette</dd>
        <dt>/</dt><dd>Filter recordings</dd>
        <dt>g l</dt><dd>Library</dd>
        <dt>g r</dt><dd>Recordings</dd>
        <dt>g s</dt><dd>Schedule</dd>
        <dt>g d</dt><dd>Pipelines (DAG)</dd>
        <dt>g g</dt><dd>Plugins</dd>
        <dt>g i</dt><dd>Activity feed (page)</dd>
        <dt>g c</dt><dd>Settings</dd>
        <dt>g y</dt><dd>System</dd>
        <dt>g v</dt><dd>Archive (Creator Edition)</dd>
        <dt>a</dt><dd>Toggle activity rail</dd>
        <dt>p</dt><dd>Poke channel monitor</dd>
        <dt>Esc</dt><dd>Close overlay</dd>
      </dl>
    </div>
  `;
  div.querySelector(".kbd-help-close")?.addEventListener("click", close);
  document.body.appendChild(div);
}

// ── Boot ─────────────────────────────────────────────────────────────

// rAF-coalesced paint scheduler — collapses N paint requests within one
// animation frame into a single execution. Used by the SSE
// RecordingProgress handler so a busy session with multiple downloads
// in flight doesn't full-repaint the recordings grid 4×/tick.
let _pendingPaint = null;
function schedulePaint(fn) {
  if (_pendingPaint) {
    _pendingPaint = fn; // overwrite — latest paint wins, prior coalesced.
    return;
  }
  _pendingPaint = fn;
  requestAnimationFrame(() => {
    const f = _pendingPaint;
    _pendingPaint = null;
    if (f) try { f(); } catch (_) {}
  });
}

// A19: debounce paintChannelList — multiple SSE events in the same
// tick should coalesce to one repaint. Without this, ChannelsUpdated
// followed instantly by ChannelWentLive caused two rail repaints +
// brief flicker. rAF-coalesced like schedulePaint.
let _railPaintQueued = false;
function paintChannelListSoon() {
  if (_railPaintQueued) return;
  _railPaintQueued = true;
  requestAnimationFrame(() => {
    _railPaintQueued = false;
    try { paintChannelList(); } catch (_) {}
  });
}

events.on((event) => {
  // A18: every cache read in this handler is now guarded so an event
  // arriving before renderHome hydrates the caches can't crash the
  // page. Each branch checks the relevant array exists + is populated
  // before mutating; ChannelsUpdated is special-cased because it
  // SOURCES the cache.
  const onHome = currentRoute() === "library";

  if (event.ChannelsUpdated) {
    channelCache = event.ChannelsUpdated || [];
    paintChannelListSoon();
  }
  if (event.ChannelWentLive || event.ChannelWentOffline) {
    // Refetch so the new live state (and ordering) is reflected.
    API.channels()
      .then((d) => {
        channelCache = d.channels || [];
        paintChannelListSoon();
      })
      .catch(() => {});
  }

  // High-frequency progress: update the in-memory job + the dashboard
  // subtree in place. No rail/detail rebuild.
  if (event.RecordingProgress && recCache.length) {
    const p = event.RecordingProgress;
    const j = recCache.find((r) => r.id === p.job_id);
    if (j) {
      j.bytes_written = p.bytes_written;
      j.duration_secs = p.duration_secs;
      // VOD downloads carry yt-dlp progress; stash on the cached job so
      // a re-render of the channel detail picks the latest values.
      if (p.download_pct != null) j.download_pct = p.download_pct;
      if (p.download_eta_secs != null) j.download_eta_secs = p.download_eta_secs;
      if (p.download_rate_bps != null) j.download_rate_bps = p.download_rate_bps;
      // Surgical DOM update for the matching VOD pill (avoids repainting
      // the whole channel detail every 2s).
      updateVodProgressDom(j);
    }
    updateLiveCount();
    // Coalesce broader-subtree repaints to one per animation frame.
    // The SSE stream fires RecordingProgress every ~2s per active job;
    // without coalescing, an N-recording session would full-repaint N×
    // per tick. updateVodProgressDom already did the surgical pill
    // update, so this is purely for the wider grid/dashboard refresh.
    schedulePaint(() => {
      if (currentRoute() === "recordings") paintRecordings();
      else paintDashboard();
    });
  }

  // Lifecycle state changes (rare): refetch recordings, refresh the
  // dashboard + rail rec-dots, without rebuilding the detail.
  if (event.RecordingStarted || event.RecordingFinished || event.AllRecordingsStopped) {
    API.recordings()
      .then((d) => {
        recCache = d.recordings || [];
        dashRecordings = recCache;
        seedVodDownloadStateFromRecCache();
        updateLiveCount();
        if (currentRoute() === "recordings") renderRecordings().catch(() => {});
        else {
          paintDashboard();
          paintChannelList();
        }
        // If a channel detail is open, refresh its VOD pills so any
        // newly-Finished source_url flips the button to Downloaded
        // (and any newly-Started one to Downloading).
        if (selectedChannelKey) {
          const [platform, id] = selectedChannelKey.split(":");
          if (id) paintChannelVods(id, platform);
        }
      })
      .catch(() => {});
  }

  // Explicit prune event from delete-recording / clear-errored — the daemon
  // tells us exactly which job_ids it dropped from jobs.db. Surgically
  // remove them from recCache + repaint, without an extra refetch.
  if (event.RecordingsPruned) {
    const ids = new Set(event.RecordingsPruned.job_ids || []);
    if (ids.size) {
      recCache = recCache.filter((r) => !ids.has(r.id));
      dashRecordings = recCache;
      updateLiveCount();
      if (currentRoute() === "recordings") renderRecordings().catch(() => {});
      else { paintDashboard(); paintChannelList(); }
    }
  }

  // #74 — bulk-download progress: update state + the rail bulk badge only.
  if (event.BulkProgress) {
    const p = event.BulkProgress;
    if (p.active) {
      bulkStatus[p.channel_id] = { done: p.done, total: p.total, active: true };
    } else {
      delete bulkStatus[p.channel_id];
    }
    paintChannelList();
  }

  // #75 — Patreon snapshot feeds the channel list + Patreon detail.
  if (event.PatreonState) {
    const ps = event.PatreonState;
    patreonState.creators = ps.creators || [];
    patreonState.posts = {};
    for (const post of ps.posts || []) {
      (patreonState.posts[post.campaign_id] ||= []).push(post);
    }
    for (const list of Object.values(patreonState.posts)) {
      list.sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""));
    }
    paintChannelList();
    // Refresh an open Patreon detail.
    if (onHome && selectedChannelKey && selectedChannelKey.startsWith("Patreon:")) {
      const id = selectedChannelKey.slice("Patreon:".length);
      const c = patreonState.creators.find((x) => x.id === id);
      if (c) renderPatreonPosts(c);
    }
  }

  // Channel VODs answer the detail-pane request.
  if (event.ChannelVods) {
    const cv = event.ChannelVods;
    channelVods[cv.channel_id] = cv.vods || [];
    if (onHome && selectedChannelKey && selectedChannelKey.endsWith(`:${cv.channel_id}`)) {
      const platform = selectedChannelKey.split(":")[0];
      paintChannelVods(cv.channel_id, platform);
    }
  }

  // #19 — Add-Channel wizard resolve reply.
  if (event.ChannelResolved) {
    paintAddWizardConfirm(event.ChannelResolved);
  }

  // #74 / #73 — playlist list answers the picker request.
  if (event.PlaylistList) {
    const pl = event.PlaylistList;
    if (pendingPlaylistChannel && pl.channel_id === pendingPlaylistChannel.id) {
      showPlaylistModal({
        loading: false,
        name: pendingPlaylistChannel.name,
        playlists: pl.playlists || [],
      });
    }
  }
  if (event.PipelineUpdated && currentRoute() === "pipelines") {
    renderPipelines().catch(() => {});
  }
});
events.start();
injectKeyboardHelp();
// Resolve the edition (creator vs PVR) and seed Patreon from the daemon
// snapshot before first paint, so the nav hides creator routes and the
// Patreon section is populated on load (not after the next poll).
fetchEdition()
  .finally(seedPatreon)
  .finally(render)
  .finally(() => {
    // Fire the welcome tour once per machine — runs after the first
    // paint settles so the topnav slots have their bounding rects.
    if (currentRoute() !== "login") {
      setTimeout(startOnboardingTour, 600);
    }
  });

// ── Test hooks ───────────────────────────────────────────────────────
//
// spa.js is loaded as a module, so its top-level declarations are
// module-scoped and unreachable from Playwright's page.evaluate. Rather
// than leak the whole module onto window, a narrow surface is exported for
// e2e — and only when the page explicitly opts in, so an ordinary session
// gains no extra globals and no way to swap the player factory.
if (typeof window !== "undefined") {
  try {
    const optedIn =
      localStorage.getItem("strivo:e2e") === "1" ||
      new URLSearchParams(location.search).get("e2e") === "1";
    if (optedIn) {
      window.__strivoTestHooks = {
        embedParentHost,
        buildEmbedUrl,
        computeMuted,
        playerState,
        setPlayerControllerFactory,
        defaultPlayerControllerFactory,
      };
    }
  } catch (_) {
    /* private mode / blocked storage — hooks simply stay off */
  }
}
