
const API = {
  _inflight: new Map(),
  _cache: new Map(),
  // A cache invalidation also retires reads which began before it.  Keeping
  // this separate from expiry matters for SSE: an old in-flight response must
  // never put its snapshot back after an authoritative lifecycle event.
  _allGeneration: 0,
  _resourceGenerations: new Map(),
  _generationFor(path) {
    let generation = API._allGeneration;
    for (const [prefix, value] of API._resourceGenerations) {
      if (path.startsWith(prefix)) generation = Math.max(generation, value);
    }
    return generation;
  },
  _cacheTtlMs: 2000,
  async _fetch(path, opts = {}) {
    const method = String(opts.method || "GET").toUpperCase();
    // Collapse duplicate GETs issued by chrome hydration + the route painter.
    // A short stale window absorbs navigation churn without making live state
    // feel cached; SSE still patches the active screen immediately.
    if (method === "GET" && !opts.__direct) {
      const generation = API._generationFor(path);
      const cached = API._cache.get(path);
      if (cached && cached.generation === generation && performance.now() - cached.at < API._cacheTtlMs) {
        return cached.value;
      }
      const inflightKey = `${generation}:${path}`;
      if (API._inflight.has(inflightKey)) return API._inflight.get(inflightKey);
      const pending = API._fetch(path, { ...opts, __direct: true })
        .then((value) => {
          // An earlier read may still resolve after a mutation/SSE refresh.
          // Return it to its original caller, but do not make it current.
          if (API._generationFor(path) === generation) {
            API._cache.set(path, { at: performance.now(), value, generation });
            return value;
          }
          // A caller may still be on the same route when an SSE event
          // invalidates its read. Do not hand that caller an obsolete
          // snapshot which it could commit into its own local cache.
          const retry = { ...opts };
          delete retry.__direct;
          return API._fetch(path, retry);
        })
        .finally(() => API._inflight.delete(inflightKey));
      API._inflight.set(inflightKey, pending);
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
      let detail = "Creator functionality is unavailable in this release.";
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
    // Scope the epoch to the resource family. Recording lifecycle events
    // should not evict an unrelated settings/health read, but they must make
    // every older /recordings response unable to refill its cache.
    const next = Math.max(API._allGeneration, ...API._resourceGenerations.values(), 0) + 1;
    if (prefix) API._resourceGenerations.set(prefix, next);
    else API._allGeneration = next;
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
  // Auto-download new uploads (Twitch/YouTube: config.auto_download_uploads;
  // Patreon: config.auto_pull_creators — the route picks the right store from
  // the channel key's platform). Present and functional in both editions.
  setAutoDownloadUploads: (channelKey, enabled) =>
    API._fetch(`/channels/${encodeURIComponent(channelKey)}/auto_download_uploads`, {
      method: "PUT",
      body: { enabled },
    }),
  // Per-channel override of the global live/upload alert switches. Pass
  // `{on_live, on_upload}`; both undefined/null clears the override and
  // reverts the channel to the global default.
  setChannelAlerts: (channelKey, body) =>
    API._fetch(`/channels/${encodeURIComponent(channelKey)}/alerts`, {
      method: "PUT",
      body,
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
