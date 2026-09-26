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
  // R02 — B-roll finder. Backend expects the streamer-curated library
  // inline on every call (it stays stateless); the SPA persists the raw
  // JSON in localStorage so it survives reloads (see rec-info-broll).
  brollSuggest: (recordingId, body) =>
    API._fetch(`/plugins/broll/${encodeURIComponent(recordingId)}`, { method: "POST", body }),
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
  // Crunchr's transcript, from the one route that serves it. The old
  // `/plugins/crunchr/transcript/{id}` never existed, and this call sat
  // behind `.catch(() => null)`, so "no such route" and "no transcript"
  // were the same value to every caller. No mask now: a refusal (402) and a
  // real failure reach the caller as the error `_fetch` threw.
  crunchrTranscript: (recordingId) =>
    API._fetch(`/plugins/crunchr/recordings/${encodeURIComponent(recordingId)}`),
