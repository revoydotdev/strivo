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
  return `
    <div class="pg-upsell-card">
      <div class="pg-upsell-icon">★</div>
      <div class="pg-upsell-body">
        <h2 class="pg-upsell-title">${htmlEscape(toTitleCase(plugin))} is unavailable</h2>
        <p class="pg-upsell-pitch">Creator Edition work is not released or supported.</p>
        <p class="pg-upsell-trial-note pg-cap-hint">No trial, activation, or purchase path is available.</p>
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
    <a class="pro-tab ${t.slug === activeTab.slug ? "is-active" : ""}" href="#/${paneKey}/${t.slug}"${t.slug === activeTab.slug ? ' aria-current="page"' : ""}>${htmlEscape(t.label)}</a>`).join("");

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
    <nav class="pro-tabs" aria-label="${htmlEscape(pane.title)} sections">${tabStrip}</nav>
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
    API.pluginList(),
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
  if (!licence || licence.entitled) return "";
  return `
    <section class="upgrade-card" data-tier="${htmlEscape(licence.tier || "free")}">
      <img class="upgrade-logo" src="/assets/img/strivo-mark.svg" alt="StriVo" />
      <div class="upgrade-body">
        <h2 class="upgrade-title">Creator Edition unavailable</h2>
        <p class="upgrade-tagline">Creator/research tooling is experimental development work and is not released or supported.</p>
        <p class="upgrade-hint">No activation, trial, or purchase path is available.</p>
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
