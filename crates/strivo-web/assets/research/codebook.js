// codebook — Code tree, codings list, and apply-coding.
//
// Creator Edition only. Imported from spa.js inside a creator block; this whole
// directory is removed from the PVR build by crates/strivo-web/build.rs.
//
// Contract: mount(root, ctx) renders the surface into `root`.
//   ctx = { api, projectId, toast, fmt }
// Return a cleanup function if the surface registers listeners on document.

function fmtClockLocal(fmt, ms) {
  return fmt.clock((ms || 0) / 1000);
}

export function mount(root, ctx) {
  const { api, projectId, toast, fmt } = ctx;
  const esc = fmt.escapeHtml;

  const state = {
    codes: [],
    codesById: new Map(),
    sourcesById: new Map(),
    codings: [],
    selectedCodeId: null,
    formOpen: false,
    codingFormOpen: false,
    loading: true,
    error: null,
  };

  function codeChildren(parentId) {
    return state.codes
      .filter((c) => (c.parent_id || null) === (parentId || null))
      .sort((a, b) => a.name.localeCompare(b.name));
  }

  function codeSwatch(color) {
    const c = /^#[0-9a-fA-F]{6}$/.test(color || "") ? color : "#888888";
    return `<span class="cb-swatch" style="background:${c}"></span>`;
  }

  function codeNodeHtml(code, depth) {
    const kids = codeChildren(code.id);
    const active = code.id === state.selectedCodeId ? "is-active" : "";
    return `
      <li class="cb-node" style="--depth:${depth}">
        <button type="button" class="cb-node-btn ${active}" data-code-id="${esc(code.id)}">
          ${codeSwatch(code.color)}
          <span class="cb-node-name">${esc(code.name)}</span>
        </button>
        ${kids.length ? `<ul class="cb-tree-list">${kids.map((k) => codeNodeHtml(k, depth + 1)).join("")}</ul>` : ""}
      </li>`;
  }

  function treeHtml() {
    const roots = codeChildren(null);
    if (!roots.length) return `<div class="empty sm">No codes yet. Create one to start building your codebook.</div>`;
    return `<ul class="cb-tree-list cb-tree-root">${roots.map((c) => codeNodeHtml(c, 0)).join("")}</ul>`;
  }

  function codeOptionsHtml(selectedId) {
    return state.codes
      .slice()
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((c) => `<option value="${esc(c.id)}" ${c.id === selectedId ? "selected" : ""}>${esc(c.name)}</option>`)
      .join("");
  }

  function newCodeFormHtml() {
    if (!state.formOpen) return "";
    return `
      <form class="cfg-card cb-inline-form" id="cb-code-form">
        <h3 class="cfg-title">New code</h3>
        <label class="arc-field">Name
          <input id="cb-code-name" class="arc-input" type="text" required maxlength="120" placeholder="e.g. Onboarding friction"/>
        </label>
        <label class="arc-field">Description <span class="pg-cap-hint">(optional)</span>
          <textarea id="cb-code-desc" class="arc-input" rows="2" maxlength="2000"></textarea>
        </label>
        <label class="arc-field">Colour
          <input id="cb-code-color" type="color" value="#00E5FF"/>
        </label>
        <label class="arc-field">Parent code <span class="pg-cap-hint">(optional)</span>
          <select id="cb-code-parent" class="arc-select">
            <option value="">— none (top level) —</option>
            ${codeOptionsHtml(null)}
          </select>
        </label>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="cb-code-cancel">Cancel</button>
          <button type="submit" class="btn-primary">Create code</button>
        </div>
      </form>`;
  }

  function applyCodingFormHtml() {
    if (!state.codingFormOpen) return "";
    const sources = [...state.sourcesById.values()];
    return `
      <form class="cfg-card cb-inline-form" id="cb-coding-form">
        <h3 class="cfg-title">Apply coding</h3>
        ${sources.length === 0 ? `<p class="empty sm">No sources in this workspace yet — index your archive from the Search tab first.</p>` : `
        <label class="arc-field">Code
          <select id="cb-coding-code" class="arc-select" required>${codeOptionsHtml(state.selectedCodeId)}</select>
        </label>
        <label class="arc-field">Source
          <select id="cb-coding-source" class="arc-select" required>
            ${sources.map((s) => `<option value="${esc(s.id)}">${esc(s.title)}</option>`).join("")}
          </select>
        </label>
        <label class="arc-field">Start (mm:ss or seconds)
          <input id="cb-coding-start" class="arc-input" type="text" required placeholder="0:00"/>
        </label>
        <label class="arc-field">End (mm:ss or seconds)
          <input id="cb-coding-end" class="arc-input" type="text" required placeholder="0:10"/>
        </label>
        <label class="arc-field">Excerpt
          <textarea id="cb-coding-excerpt" class="arc-input" rows="2" required maxlength="4000" placeholder="The transcript text this coding covers"></textarea>
        </label>
        <label class="arc-field">Note <span class="pg-cap-hint">(optional)</span>
          <textarea id="cb-coding-note" class="arc-input" rows="2" maxlength="4000"></textarea>
        </label>
        <label class="arc-field">Author
          <input id="cb-coding-author" class="arc-input" type="text" required maxlength="120" placeholder="Your name"/>
        </label>`}
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="cb-coding-cancel">Cancel</button>
          ${sources.length ? `<button type="submit" class="btn-primary">Apply coding</button>` : ""}
        </div>
      </form>`;
  }

  function codingRowHtml(coding) {
    const code = state.codesById.get(coding.code_id);
    const src = state.sourcesById.get(coding.source_id);
    const time = `${fmtClockLocal(fmt, coding.start_ms)} → ${fmtClockLocal(fmt, coding.end_ms)}`;
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          ${code ? `<span class="cfg-badge">${codeSwatch(code.color)}${esc(code.name)}</span>` : ""}
          <span class="arc-row-title">${src ? esc(src.title) : "(unresolved source)"}</span>
          <span class="arc-row-time">${time}</span>
          <span class="pg-cap-hint">${esc(coding.origin)} · ${esc(coding.author)}</span>
        </div>
        <p class="arc-row-snippet">${esc(coding.excerpt)}</p>
        ${coding.note ? `<p class="pg-cap-hint">${esc(coding.note)}</p>` : ""}
      </div>`;
  }

  function codingsHtml() {
    if (!state.codings.length) {
      return `<div class="empty sm">No codings ${state.selectedCodeId ? "for this code" : "yet"}.</div>`;
    }
    return state.codings.map(codingRowHtml).join("");
  }

  function render() {
    if (state.loading) {
      root.innerHTML = `<div class="empty sm" aria-busy="true">Loading codebook…</div>`;
      return;
    }
    if (state.error) {
      root.innerHTML = `
        <div class="empty sm"><div class="glyph">⚠</div>${esc(state.error)}
          <button class="sm" id="cb-retry" type="button">Retry</button>
        </div>`;
      root.querySelector("#cb-retry")?.addEventListener("click", load);
      return;
    }
    const activeCode = state.selectedCodeId ? state.codesById.get(state.selectedCodeId) : null;
    root.innerHTML = `
      <div class="arc-grid cb-grid">
        <section class="cfg-card">
          <div class="cb-card-head">
            <h2 class="cfg-title">Code tree</h2>
            <button class="sm" id="cb-new-code-btn" type="button">＋ New code</button>
          </div>
          ${newCodeFormHtml()}
          <div class="cb-tree-host">${treeHtml()}</div>
        </section>
        <section class="cfg-card">
          <div class="cb-card-head">
            <h2 class="cfg-title">Codings ${activeCode ? `· ${esc(activeCode.name)}` : ""}</h2>
            <button class="sm" id="cb-apply-coding-btn" type="button">＋ Apply coding</button>
          </div>
          ${activeCode ? `<button class="sm" id="cb-clear-filter" type="button">Show all codes</button>` : ""}
          ${applyCodingFormHtml()}
          <div class="cb-codings-host" aria-live="polite">${codingsHtml()}</div>
        </section>
      </div>`;
    wire();
  }

  function wire() {
    root.querySelectorAll(".cb-node-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const id = btn.dataset.codeId;
        state.selectedCodeId = state.selectedCodeId === id ? null : id;
        loadCodings();
      });
    });
    root.querySelector("#cb-clear-filter")?.addEventListener("click", () => {
      state.selectedCodeId = null;
      loadCodings();
    });
    root.querySelector("#cb-new-code-btn")?.addEventListener("click", () => {
      state.formOpen = !state.formOpen;
      render();
    });
    root.querySelector("#cb-code-cancel")?.addEventListener("click", () => {
      state.formOpen = false;
      render();
    });
    root.querySelector("#cb-code-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const name = document.getElementById("cb-code-name").value.trim();
      const description = document.getElementById("cb-code-desc").value.trim();
      const color = document.getElementById("cb-code-color").value;
      const parentId = document.getElementById("cb-code-parent").value || undefined;
      if (!name) { toast.error("Name is required"); return; }
      const btn = e.submitter;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Creating…";
      try {
        await api.researchCreateCode(projectId, {
          id: crypto.randomUUID(),
          project_id: projectId,
          parent_id: parentId,
          name,
          description,
          color,
        });
        toast.success("Code created");
        state.formOpen = false;
        await load();
      } catch (err) {
        toast.error(`Couldn't create code: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });

    root.querySelector("#cb-apply-coding-btn")?.addEventListener("click", () => {
      state.codingFormOpen = !state.codingFormOpen;
      render();
    });
    root.querySelector("#cb-coding-cancel")?.addEventListener("click", () => {
      state.codingFormOpen = false;
      render();
    });
    root.querySelector("#cb-coding-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const codeId = document.getElementById("cb-coding-code").value;
      const sourceId = document.getElementById("cb-coding-source").value;
      const startSec = fmt.parseTime(document.getElementById("cb-coding-start").value);
      const endSec = fmt.parseTime(document.getElementById("cb-coding-end").value);
      const excerpt = document.getElementById("cb-coding-excerpt").value.trim();
      const note = document.getElementById("cb-coding-note").value.trim();
      const author = document.getElementById("cb-coding-author").value.trim();
      if (!isFinite(startSec) || !isFinite(endSec)) { toast.error("Couldn't parse start/end time"); return; }
      if (endSec < startSec) { toast.error("End must be at or after start"); return; }
      if (!excerpt) { toast.error("Excerpt is required"); return; }
      if (!author) { toast.error("Author is required"); return; }
      const btn = e.submitter;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Applying…";
      try {
        await api.researchCreateCoding(projectId, {
          id: crypto.randomUUID(),
          project_id: projectId,
          source_id: sourceId,
          code_id: codeId,
          start_ms: Math.round(startSec * 1000),
          end_ms: Math.round(endSec * 1000),
          excerpt,
          note,
          author,
          origin: "human",
        });
        toast.success("Coding applied");
        state.codingFormOpen = false;
        await loadCodings();
      } catch (err) {
        toast.error(`Couldn't apply coding: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });
  }

  async function loadCodings() {
    try {
      const resp = await api.researchCodings(projectId, { codeId: state.selectedCodeId || undefined });
      let codings = resp.codings || [];
      // Defensive client-side filter: don't trust the backend to honor
      // code_id until it's confirmed live (research kernel's GET codings
      // endpoint is landing in parallel with this UI).
      if (state.selectedCodeId) codings = codings.filter((c) => c.code_id === state.selectedCodeId);
      state.codings = codings;
    } catch (err) {
      state.codings = [];
      toast.error(`Couldn't load codings: ${err.message}`);
    }
    render();
  }

  async function load() {
    state.loading = true;
    state.error = null;
    render();
    try {
      const [codesResp, sourcesResp] = await Promise.all([
        api.researchCodes(projectId),
        api.researchSources(projectId).catch(() => ({ sources: [] })),
      ]);
      state.codes = codesResp.codes || [];
      state.codesById = new Map(state.codes.map((c) => [c.id, c]));
      state.sourcesById = new Map((sourcesResp.sources || []).map((s) => [s.id, s]));
    } catch (err) {
      state.error = err.message || "Couldn't load the codebook.";
      state.loading = false;
      render();
      return;
    }
    state.loading = false;
    await loadCodings();
  }

  load();

  return () => {
    // Nothing registers document-level listeners here — DOM inside `root`
    // is torn down by the router replacing root.innerHTML on navigation.
  };
}
