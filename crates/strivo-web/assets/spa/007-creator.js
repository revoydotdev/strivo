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
  // ── Strivo Pro licensing (Phase 1: status only; activate/trial 501) ──
  licenceStatus: () => API._fetch("/licence/status"),
  licenceTrial: () => API._fetch("/licence/trial", { method: "POST" }),
  licenceActivate: (key) =>
    API._fetch("/licence/activate", { method: "POST", body: { key } }),
  // ── The multi-track timeline ───────────────────────────────────────
  // One route for the read and the write: it compiles a request and answers
  // with the plan, so a caller can never hold a composition the renderer
  // would not produce.
  timelineApply: (body) => API._fetch("/creator/timeline/apply", { method: "POST", body }),
