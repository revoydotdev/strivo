
// Single source of truth for the plugin set across the Settings →
// Plugins manager AND the hub. Every shipped plugin appears once with
// the metadata the SPA needs to render its enable toggle + open link.
//
// `proGated` and `installed` are NOT stated here: they are answers the
// server owns, and this module asks. `applyCapabilityDescriptor()`
// rewrites them in place from GET /api/v1/plugins/descriptor, so the
// badge the user sees and the 402 the server answers with come from the
// same rows. Until (or unless) the descriptor lands, the SPA's own prior
// claim stands, so a slow or failed fetch cannot strip a badge.
const PLUGIN_REGISTRY = [
  // First-party Pro plugins with dedicated SPA sub-routes.
  { name: "crunchr",   label: "Crunchr",   category: "Transcription", route: "#/plugins/crunchr",   description: "Transcribe every recording — speaker timeline, quote search, exportable subtitles." },
  { name: "archiver",  label: "Archiver",  category: "Archive",       route: "#/plugins/archiver",  description: "Auto-catalog the full back-catalog of any followed channel." },
  { name: "insights",  label: "Insights",  category: "Analytics",     route: "#/plugins/insights",  description: "Cross-stream analytics, word frequency, topic shifts, retention proxy." },
  { name: "viewguard", label: "Viewguard", category: "Analytics",     route: "#/plugins/viewguard", description: "Live fraud-signal scoring during captures + cross-stream trend dashboard." },
  // Editor stack.
  { name: "editor",     label: "EDL editor",   category: "Editor", description: "Non-destructive EDL with split / ripple-delete / dead-air trim / branding overlay + revision history." },
  { name: "deadair",    label: "Dead-air trim", category: "Editor", description: "Silence detection + one-click EDL trim from inside the editor." },
  { name: "branding",   label: "Branding",      category: "Editor", description: "Watermark + intro/outro banner overlay spec, applied via ffmpeg filter_complex on render." },
  { name: "broll",      label: "B-roll finder", category: "Editor", description: "Suggest B-roll cuts from a tagged local library based on transcript topics." },
  { name: "loudness",   label: "Loudness",      category: "Editor", description: "EBU R128 master-bus loudness check with per-platform presets (YouTube/Spotify/Apple/EBU/Twitch)." },
  { name: "structure",  label: "Structure",     category: "Editor", description: "DAW-style section labeller — intro / gameplay / break / outro tiling from chapters + chat density + scene cuepoints." },
  { name: "automation", label: "Automation",    category: "Editor", description: "Volume automation curves — time-keyed gain with linear/cosine/step interpolation, baked via ffmpeg asendcmd." },
  { name: "scenes",     label: "Scene snapshots", category: "Editor", description: "DAW-style session save/recall — bundle every per-recording plugin state as a named scene." },
  { name: "schedule-optimizer", label: "Schedule optimizer", category: "Publish", description: "Publish-slot recommender — engagement samples → top weekly publish times with confidence + plateau coverage." },
  { name: "beat-detect",        label: "Beat detection",     category: "Editor", description: "DAW-style tempo grid — onset detector + BPM autocorrelation for music-sync montage cuts." },
  { name: "vad",                label: "Voice gate",         category: "Editor", description: "DAW-style noise gate — hysteresis VAD that surfaces auto-tighten ripple-deletes for podcast/commentary recordings." },
  { name: "sidechain",          label: "Sidechain compressor", category: "Editor", description: "DAW sidechain — VAD voice intervals → ducking automation curve baked via the existing volume-automation render path." },
  { name: "insert-fx",          label: "Insert FX chain",      category: "Editor", description: "DAW-style ordered insert chain per recording: HP, NR, de-esser, comp, limiter, reverb. Voice + game bus presets. Composes into one ffmpeg -af baked at render." },
  { name: "pitch",              label: "Pitch / time-stretch", category: "Editor", description: "Independent tempo + pitch. Fit a 1h45 stream to a 1h slot without changing voices' pitch, or transpose a stinger without changing tempo. Wraps ffmpeg rubberband, formant-preserving by default." },
  // Asset / analytics / publishing.
  { name: "chapters",         label: "Chapters",         category: "Analytics", description: "Heuristic chapter markers extracted from pacing." },
  { name: "cuepoints",        label: "Cuepoints",        category: "Analytics", description: "Scene-change detection from ffmpeg's select filter." },
  { name: "thumbnails",       label: "Thumbnails",       category: "Analytics", description: "Frame ranking + facecam crop candidates." },
  { name: "clipper",          label: "Clipper",          category: "Editor",    description: "Highlight detection + one-click clip extraction." },
  { name: "captions",         label: "Captions",         category: "Transcription", description: "SRT / VTT / TXT export with translator-trait pluggable backend." },
  { name: "multitrack",       label: "Multitrack",       category: "Editor",    description: "Audio track enumeration + extraction." },
  { name: "brandsafe",        label: "Brand safety",     category: "Publish",   description: "Pre-publish content classifier." },
  { name: "reuse",            label: "Reuse",            category: "Publish",   description: "Cross-format publish-queue drafter." },
  { name: "casebook",         label: "Casebook",         category: "Reports",   description: "Post-stream markdown briefing." },
  { name: "heatmap",          label: "Heatmap",          category: "Analytics", description: "Multi-signal retention overlay." },
  { name: "insights-compare", label: "Compare", category: "Analytics", description: "Stream-vs-stream side-by-side." },
  { name: "viewguard-trend",  label: "Viewguard trend",  category: "Analytics", description: "Cross-stream fraud trend dashboard." },
  { name: "chat-density",     label: "Chat density",     category: "Analytics", description: "Audience-retention proxy from chat rate." },
  // Viewer layer.
  { name: "multistream", label: "Multistream viewer", category: "Viewer", route: "#/watch", description: "Auto-tile any subset of currently-live followed channels." },
  { name: "chat",        label: "Chat client",       category: "Viewer", route: "#/chat",  description: "Chatterino-class IRC + tokenizer + filter pipeline + ring buffer." },
  // Cross-cutting.
  { name: "pipelines-dag", label: "Pipelines DAG", category: "Reports", route: "#/pipelines", description: "Cross-plugin pipeline graph." },
  { name: "marketplace",   label: "Marketplace",   category: "Reports", route: "#/plugins",   description: "Third-party plugin catalog stub." },
];

// `proGated` means "this build does not ship the capability at all" — a
// reachability claim. The upgrade card's "paid" claim is a different
// question the descriptor answers separately (`entitlement`), and the two
// sets are not the same: four capabilities are paid, all of these are
// Creator-only. Conflating them is exactly the drift this file had.
for (const meta of PLUGIN_REGISTRY) {
  if (meta.proGated === undefined) meta.proGated = true;
}

// Fold the server's descriptor into the registry. `proGated` is the
// row's own `edition` compared against the PVR edition — a capability the
// PVR edition does not ship is badged, whatever edition asked for it. (Do
// not compare against the descriptor's top-level `edition`: a Creator
// build's descriptor says "creator", so that comparison is false for every
// row and silently strips all 35 badges.) `installed` comes from
// `entry_point`, so a roadmap slot the SPA offers cannot render as a
// working plugin. An unknown key is left as-is: the registry entry stays
// visible, and the server's own 402 is the backstop.
function applyCapabilityDescriptor(descriptor) {
  if (!descriptor || !Array.isArray(descriptor.capabilities)) return;
  const byKey = new Map(descriptor.capabilities.map((c) => [c.key, c]));
  for (const meta of PLUGIN_REGISTRY) {
    const row = byKey.get(meta.name);
    if (!row) continue;
    if (row.edition) meta.proGated = row.edition !== "pvr";
    if (row.installed === false) meta.installed = false;
  }
}

API._fetch("/plugins/descriptor")
  .then(applyCapabilityDescriptor)
  .catch(() => {});

