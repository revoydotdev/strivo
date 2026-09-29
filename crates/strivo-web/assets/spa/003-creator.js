  // Read-only plugin list — used by the plugin hub/pane and by the
  // recording-info modal (028-pvr.js), which checks for this method before
  // calling it: the PVR daemon mounts no /plugins route.
  pluginList: () => API._fetch("/plugins"),
  pluginRpc: (plugin, verb, body) =>
    API._fetch(`/plugins/${encodeURIComponent(plugin)}/${encodeURIComponent(verb)}`, {
      method: "POST",
      body,
    }),
  // ── Plugin data (read-only, served from each plugin's SQLite DB) ──
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
