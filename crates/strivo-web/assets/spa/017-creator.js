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
  return `<nav class="pro-tabs" aria-label="Archive sections">${tabs.map((t) => `
    <a class="pro-tab ${t.slug === activeSlug ? "is-active" : ""}" href="#/archive/${t.slug}"${t.slug === activeSlug ? ' aria-current="page"' : ""}>${htmlEscape(t.label)}</a>`).join("")}</nav>`;
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
