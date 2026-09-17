
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

// Tracks which recording's Info modal is currently open, so the SSE
// RecordingProgress handler (036-pvr.js) can live-patch its percent
// without a full modal re-render. null when no modal is open.
let _openRecInfoJobId = null;

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
  _openRecInfoJobId = null;
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

// Live-patch the open Recording Info modal's header state pill from an
// SSE RecordingProgress tick (036-pvr.js calls this unconditionally on
// every tick; it no-ops when no modal is open or the tick is for a
// different job). Mirrors updateVodProgressDom's surgical-patch style
// (012-pvr.js) — this modal has no independent progress bar, just the
// header renderStatePill, so patching that one element is enough.
function updateRecInfoModalProgress(p) {
  if (!_openRecInfoJobId || String(p.job_id) !== String(_openRecInfoJobId)) return;
  const overlay = document.getElementById("rec-info-modal");
  if (!overlay) { _openRecInfoJobId = null; return; }
  const live = recCache.find((r) => r.id === _openRecInfoJobId);
  if (!live) return;
  if (p.bytes_written != null) live.bytes_written = p.bytes_written;
  if (p.duration_secs != null) live.duration_secs = p.duration_secs;
  if (p.download_pct != null) live.download_pct = p.download_pct;
  if (p.download_eta_secs != null) live.download_eta_secs = p.download_eta_secs;
  if (p.download_rate_bps != null) live.download_rate_bps = p.download_rate_bps;
  const pillHost = overlay.querySelector(".rec-info-head");
  const existing = pillHost && pillHost.querySelector(".state-pill");
  if (existing) existing.outerHTML = renderStatePill(recordingDisplayState(live));
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

  // recordingOne() is a point-in-time REST fetch; recCache carries
  // whatever SSE RecordingProgress ticks have landed since. Merge so the
  // modal opens with the freshest known percent instead of a
  // potentially-stale/zeroed field, matching the Timeline merge in
  // renderRecordingsTimeline (012-pvr.js).
  const liveJob = recCache.find((r) => r.id === jobId);
  if (liveJob) {
    for (const k of ["download_pct", "download_eta_secs", "download_rate_bps", "bytes_written", "duration_secs"]) {
      if (liveJob[k] != null) rec[k] = liveJob[k];
    }
  }
  _openRecInfoJobId = jobId;

  // Routed through the canonical recordingDisplayState() (012-pvr.js)
  // instead of raw stateLabel/stateClassName, so this modal's pill agrees
  // with the Recordings table on Downloading/File Error nuances instead of
  // just reading the bare backend state.
  const disp = recordingDisplayState(rec);
  const isFinished = disp.className === "finished";
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
  // buildCreatorPluginActionsPanel is defined only in 037-creator.js (a
  // hoisted top-level `function`, so it's callable here regardless of file
  // order); the typeof guard is the same idiom check-pvr-bundle.mjs already
  // expects for a Creator-only render function that a PVR bundle omits.
  const creatorActionsHtml = typeof buildCreatorPluginActionsPanel === "function"
    ? buildCreatorPluginActionsPanel(actionsHtml, isFinished)
    : "";

  const targetTimeHtml = opts.seekSec != null
    ? `<span class="cfg-badge arc-target-time" title="Archive target — the split prompt below the EDL editor pre-fills with this time">🎯 target ${htmlEscape(fmtClock(opts.seekSec))}</span>`
    : "";
  overlay.querySelector(".modal-card").innerHTML = `
    <header class="rec-info-head">
      ${renderStatePill(disp)}
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
    if (jobId) openRecordingPlayer(jobId);
  });
