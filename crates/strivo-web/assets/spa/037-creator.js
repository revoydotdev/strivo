// Creator Edition additions to the onboarding tour + page hints declared in
// 036-pvr.js: the Pipelines and Plugins nav slots. Mutating the existing
// const bindings (rather than redeclaring them) keeps this additive, and
// keeps the copy itself — including the marketplace-catalog pitch — out of
// a PVR build's bytes entirely.
PAGE_HINTS.pipelines = "Durable Creator workflows with live state, cancellation, retries, restart recovery, and capability blueprints.";
PAGE_HINTS.plugins = "Plugin hub. Each card opens the plugin; ⚙ deep-links to the per-plugin Settings panel.";

// Insert right after "chat" and before "settings", matching the nav order.
const settingsStepIdx = TOUR_STEPS.findIndex((s) => s.route === "settings");
TOUR_STEPS.splice(settingsStepIdx, 0,
  { route: "pipelines", title: "Pipelines", body: "Cross-plugin DAGs. Click a node to open it; 'Run on…' picks a recording + opens the right plugin." },
  { route: "plugins",   title: "Plugins",   body: "The shipped plugin set + marketplace catalog. Click any card to open; gear icon → per-plugin Settings." },
);

// Same idea for the keyboard-shortcuts help overlay: insert the Creator-only
// nav hotkeys right after "g s" Schedule, and the Archive toggle right
// before "Esc" (the list's last row) — re-anchored off these two stable
// rows now that the never-wired "g i"/"a" rows they used to key off of
// have been removed from KBD_HELP_ROWS (036-pvr.js).
const scheduleRowIdx = KBD_HELP_ROWS.findIndex(([k]) => k === "g s");
KBD_HELP_ROWS.splice(scheduleRowIdx + 1, 0,
  ["g d", "Pipelines (DAG)"],
  ["g g", "Plugins"],
);
const escRowIdx = KBD_HELP_ROWS.findIndex(([k]) => k === "Esc");
KBD_HELP_ROWS.splice(escRowIdx, 0, ["g v", "Archive (Creator Edition)"]);

// Real implementation of the recording-info modal's "Plugin actions" panel
// (028-pvr.js falls back to "" via a `typeof` guard when this doesn't
// exist). Every control here dispatches to a plugin route the pure-PVR
// daemon does not mount, so this whole renderer — including the button
// copy — is absent from a PVR build entirely.
//
// Defined as a plain top-level `function` (not a `let`-then-reassign) on
// purpose: `openRecordingInfo` in 028-pvr.js is itself defined before this
// file loads, and a function declaration hoists to the top of its enclosing
// (module) scope, so it's safely callable from the very first invocation —
// no dependency on load-order timing the way a reassigned closure would
// need.
function buildCreatorPluginActionsPanel(actionsHtml, isFinished) {
  return CREATOR_ENABLED ? `
    <section class="rec-info-actions">
      <h3>Plugin actions</h3>
      <div class="rec-info-verbs">${actionsHtml}</div>
      ${isFinished ? `<button class="sm rec-info-cuepoints-btn" data-action="rec-info-cuepoints" title="Scene-change cuepoints (ffmpeg full pass)">⌶ Detect scene changes</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-clipper-btn" data-action="rec-info-clipper" title="Mine highlight candidates (uses cuepoints; runs ffmpeg pass if needed)">★ Find highlights</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-thumbs-btn" data-action="rec-info-thumbs" title="Sample candidate thumbnail frames at cuepoints / highlights">▥ Pick thumbnail</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-broll-btn" data-action="rec-info-broll" title="Suggest B-roll cuts from a tagged local library based on transcript topics">🎞 B-roll suggestions</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-tracks-btn" data-action="rec-info-tracks" title="List audio tracks (OBS multi-track captures) + extract individual stems">♪ Audio tracks</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-reuse-btn" data-action="rec-info-reuse" title="Build cross-format publish drafts (YT long / Shorts / TikTok / Patreon / podcast / blog)">⇪ Publish drafts</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-casebook-btn" data-action="rec-info-casebook" title="Post-stream Casebook report (markdown briefing)">📓 Casebook</button>` : ""}
      ${isFinished ? `<button class="sm rec-info-editor-btn" data-action="rec-info-editor" title="Open the EDL editor — cut, ripple-delete, render">✄ EDL editor</button>` : ""}
      <div class="rec-cuepoints" id="rec-cuepoints" hidden></div>
      <div class="rec-clipper" id="rec-clipper" hidden></div>
      <div class="rec-thumbs" id="rec-thumbs" hidden></div>
      <div class="rec-broll" id="rec-broll" hidden></div>
      <div class="rec-tracks" id="rec-tracks" hidden></div>
      <div class="rec-reuse" id="rec-reuse" hidden></div>
      <div class="rec-casebook" id="rec-casebook" hidden></div>
      <div class="rec-editor" id="rec-editor" hidden></div>
    </section>` : "";
}

// Real implementation of the Monitor page's "Auto-download new uploads"
// (Archiver tandem) section (034d-pvr.js falls back to "" via the same
// `typeof` guard idiom). The PVR daemon mounts no archiver routes, so this
// section — including its copy — is absent from a PVR build entirely.
function buildCreatorDownloadSection(downloadRows, channelsAvailableForDownload) {
  return CREATOR_ENABLED ? `
    <section class="cfg-card">
      <h2 class="cfg-title">Auto-download new uploads</h2>
      <p class="mon-help">Pulls new uploads from a YouTube channel as the monitor sees them. Leave the playlist field empty for the whole channel, or paste one or more playlist IDs to limit scope.</p>
      ${downloadRows || '<div class="empty sm">No channels are set to auto-download yet.</div>'}
      <form id="mon-dl-add" class="mon-add">
        <select id="mon-dl-channel" aria-label="YouTube channel to auto-download">
          <option value="">Pick a YouTube channel…</option>
          ${channelsAvailableForDownload
            .map((c) => `<option value="${htmlEscape(`${c.platform}:${c.id}`)}">${htmlEscape(c.display_name || c.name)}</option>`)
            .join("")}
        </select>
        <button class="btn-primary" type="submit">Enable</button>
      </form>
    </section>` : "";
}
