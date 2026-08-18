// notebook — Memos, relationships, coder agreement, and export.
//
// Creator Edition only. Imported from spa.js inside a creator block; this whole
// directory is removed from the PVR build by crates/strivo-web/build.rs.
//
// Contract: mount(root, ctx) renders the surface into `root`.
//   ctx = { api, projectId, toast, fmt }
// Return a cleanup function if the surface registers listeners on document.

function downloadFile(filename, content, mime) {
  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export function mount(root, ctx) {
  const { api, projectId, toast, fmt } = ctx;
  const esc = fmt.escapeHtml;

  const state = {
    memos: [],
    relationships: [],
    sources: [],
    codings: [],
    codes: [],
    authors: [],
    memoFormOpen: false,
    relFormOpen: false,
    agreement: null,
    agreementError: null,
    agreementLoading: false,
    loading: true,
    error: null,
  };

  function memoRowHtml(m) {
    const src = state.sources.find((s) => s.id === m.source_id);
    const coding = state.codings.find((c) => c.id === m.coding_id);
    const attach = src
      ? `<span class="pg-cap-hint">on source · ${esc(src.title)}</span>`
      : coding
        ? `<span class="pg-cap-hint">on coding · ${esc(coding.excerpt.slice(0, 40))}${coding.excerpt.length > 40 ? "…" : ""}</span>`
        : "";
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          <span class="arc-row-title">${esc(m.title)}</span>
          <span class="pg-cap-hint">${esc(m.author)}</span>
          ${attach}
        </div>
        <p class="arc-row-snippet">${esc(m.body)}</p>
      </div>`;
  }

  function memosHtml() {
    if (!state.memos.length) return `<div class="empty sm">No memos yet.</div>`;
    return state.memos.map(memoRowHtml).join("");
  }

  function relRowHtml(r) {
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          <span class="cfg-badge">${esc(r.from_kind)}</span>
          <span class="arc-row-title">${esc(r.from_id.slice(0, 8))}…</span>
          <span class="pg-cap-hint">${esc(r.relation)} →</span>
          <span class="cfg-badge">${esc(r.to_kind)}</span>
          <span class="arc-row-title">${esc(r.to_id.slice(0, 8))}…</span>
        </div>
        ${r.note ? `<p class="arc-row-snippet">${esc(r.note)}</p>` : ""}
        <span class="pg-cap-hint">${esc(r.author)}</span>
      </div>`;
  }

  function relationshipsHtml() {
    if (!state.relationships.length) return `<div class="empty sm">No relationships recorded yet.</div>`;
    return state.relationships.map(relRowHtml).join("");
  }

  function memoFormHtml() {
    if (!state.memoFormOpen) return "";
    return `
      <form class="cfg-card cb-inline-form" id="nb-memo-form">
        <h3 class="cfg-title">New memo</h3>
        <label class="arc-field">Title
          <input id="nb-memo-title" class="arc-input" type="text" required maxlength="200"/>
        </label>
        <label class="arc-field">Body
          <textarea id="nb-memo-body" class="arc-input" rows="4" required maxlength="8000"></textarea>
        </label>
        <label class="arc-field">Author
          <input id="nb-memo-author" class="arc-input" type="text" required maxlength="120"/>
        </label>
        <label class="arc-field">Attach to source <span class="pg-cap-hint">(optional)</span>
          <select id="nb-memo-source" class="arc-select">
            <option value="">— none —</option>
            ${state.sources.map((s) => `<option value="${esc(s.id)}">${esc(s.title)}</option>`).join("")}
          </select>
        </label>
        <label class="arc-field">Attach to coding <span class="pg-cap-hint">(optional)</span>
          <select id="nb-memo-coding" class="arc-select">
            <option value="">— none —</option>
            ${state.codings.map((c) => `<option value="${esc(c.id)}">${esc(c.excerpt.slice(0, 60))}</option>`).join("")}
          </select>
        </label>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="nb-memo-cancel">Cancel</button>
          <button type="submit" class="btn-primary">Add memo</button>
        </div>
      </form>`;
  }

  function relFormHtml() {
    if (!state.relFormOpen) return "";
    const kindOpts = ["source", "code", "coding", "case", "memo"]
      .map((k) => `<option value="${k}">${k}</option>`).join("");
    return `
      <form class="cfg-card cb-inline-form" id="nb-rel-form">
        <h3 class="cfg-title">New relationship</h3>
        <label class="arc-field">From kind
          <select id="nb-rel-from-kind" class="arc-select">${kindOpts}</select>
        </label>
        <label class="arc-field">From id <span class="pg-cap-hint">(UUID)</span>
          <input id="nb-rel-from-id" class="arc-input" type="text" required placeholder="00000000-0000-0000-0000-000000000000"/>
        </label>
        <label class="arc-field">Relation
          <input id="nb-rel-relation" class="arc-input" type="text" required maxlength="80" placeholder="e.g. supports, contradicts, follows"/>
        </label>
        <label class="arc-field">To kind
          <select id="nb-rel-to-kind" class="arc-select">${kindOpts}</select>
        </label>
        <label class="arc-field">To id <span class="pg-cap-hint">(UUID)</span>
          <input id="nb-rel-to-id" class="arc-input" type="text" required placeholder="00000000-0000-0000-0000-000000000000"/>
        </label>
        <label class="arc-field">Note <span class="pg-cap-hint">(optional)</span>
          <textarea id="nb-rel-note" class="arc-input" rows="2" maxlength="2000"></textarea>
        </label>
        <label class="arc-field">Author
          <input id="nb-rel-author" class="arc-input" type="text" required maxlength="120"/>
        </label>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="nb-rel-cancel">Cancel</button>
          <button type="submit" class="btn-primary">Add relationship</button>
        </div>
      </form>`;
  }

  function agreementResultHtml() {
    if (state.agreementLoading) return `<div class="empty sm" aria-busy="true">Computing agreement…</div>`;
    if (state.agreementError) return `<div class="empty sm"><div class="glyph">⚠</div>${esc(state.agreementError)}</div>`;
    if (!state.agreement) return "";
    const a = state.agreement;
    const kappa = a.kappa != null ? a.kappa.toFixed(3) : "—";
    const observed = a.observed != null ? (a.observed * 100).toFixed(1) + "%" : "—";
    const expected = a.expected != null ? (a.expected * 100).toFixed(1) + "%" : "—";
    return `
      <div class="nb-agreement-stats">
        <div class="nb-stat"><span class="nb-stat-value">${esc(kappa)}</span><span class="pg-cap-hint">Cohen's κ</span></div>
        <div class="nb-stat"><span class="nb-stat-value">${esc(observed)}</span><span class="pg-cap-hint">Observed agreement</span></div>
        <div class="nb-stat"><span class="nb-stat-value">${esc(expected)}</span><span class="pg-cap-hint">Expected agreement</span></div>
        <div class="nb-stat"><span class="nb-stat-value">${esc(String(a.n ?? "—"))}</span><span class="pg-cap-hint">n</span></div>
      </div>`;
  }

  function render() {
    if (state.loading) {
      root.innerHTML = `<div class="empty sm" aria-busy="true">Loading notebook…</div>`;
      return;
    }
    if (state.error) {
      root.innerHTML = `
        <div class="empty sm"><div class="glyph">⚠</div>${esc(state.error)}
          <button class="sm" id="nb-retry" type="button">Retry</button>
        </div>`;
      root.querySelector("#nb-retry")?.addEventListener("click", load);
      return;
    }
    const authorOpts = (id) => state.authors.map((a) => `<option value="${esc(a)}" ${a === id ? "selected" : ""}>${esc(a)}</option>`).join("");
    root.innerHTML = `
      <div class="arc-grid cb-grid">
        <section class="cfg-card">
          <div class="cb-card-head">
            <h2 class="cfg-title">Memos</h2>
            <button class="sm" id="nb-new-memo-btn" type="button">＋ New memo</button>
          </div>
          ${memoFormHtml()}
          <div class="cb-codings-host">${memosHtml()}</div>
        </section>
        <section class="cfg-card">
          <div class="cb-card-head">
            <h2 class="cfg-title">Relationships</h2>
            <button class="sm" id="nb-new-rel-btn" type="button">＋ New relationship</button>
          </div>
          ${relFormHtml()}
          <div class="cb-codings-host">${relationshipsHtml()}</div>
        </section>
      </div>
      <section class="cfg-card" style="margin-top:0.9rem">
        <h2 class="cfg-title">Coder agreement</h2>
        <p class="pg-cap-hint">Cohen's kappa between two authors' codings, optionally scoped to one code.</p>
        <div class="arc-moments-filter">
          <label for="nb-agr-a">Author A</label>
          <input id="nb-agr-a" class="arc-input" type="text" list="nb-authors" placeholder="author a"/>
          <label for="nb-agr-b">Author B</label>
          <input id="nb-agr-b" class="arc-input" type="text" list="nb-authors" placeholder="author b"/>
          <datalist id="nb-authors">${authorOpts()}</datalist>
          <label for="nb-agr-code">Code</label>
          <select id="nb-agr-code" class="arc-select">
            <option value="">any</option>
            ${state.codes.map((c) => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join("")}
          </select>
          <button class="sm" id="nb-agr-run" type="button">Compute</button>
        </div>
        <div id="nb-agreement-host">${agreementResultHtml()}</div>
      </section>
      <section class="cfg-card" style="margin-top:0.9rem">
        <h2 class="cfg-title">Export</h2>
        <p class="pg-cap-hint">Download this workspace's codebook, sources, and codings.</p>
        <div class="arc-moment-actions" style="justify-content:flex-start">
          <button class="sm" id="nb-export-json" type="button">⬇ Export JSON</button>
          <button class="sm" id="nb-export-refi" type="button">⬇ Export REFI-QDA</button>
        </div>
      </section>`;
    wire();
  }

  function wire() {
    root.querySelector("#nb-new-memo-btn")?.addEventListener("click", () => {
      state.memoFormOpen = !state.memoFormOpen;
      render();
    });
    root.querySelector("#nb-memo-cancel")?.addEventListener("click", () => {
      state.memoFormOpen = false;
      render();
    });
    root.querySelector("#nb-memo-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const title = document.getElementById("nb-memo-title").value.trim();
      const body = document.getElementById("nb-memo-body").value.trim();
      const author = document.getElementById("nb-memo-author").value.trim();
      const sourceId = document.getElementById("nb-memo-source").value || undefined;
      const codingId = document.getElementById("nb-memo-coding").value || undefined;
      if (!title || !body || !author) { toast.error("Title, body, and author are required"); return; }
      const btn = e.submitter;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Adding…";
      try {
        await api.researchCreateMemo(projectId, {
          id: crypto.randomUUID(),
          project_id: projectId,
          source_id: sourceId,
          coding_id: codingId,
          title,
          body,
          author,
        });
        toast.success("Memo added");
        state.memoFormOpen = false;
        await loadMemos();
      } catch (err) {
        toast.error(`Couldn't add memo: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });

    root.querySelector("#nb-new-rel-btn")?.addEventListener("click", () => {
      state.relFormOpen = !state.relFormOpen;
      render();
    });
    root.querySelector("#nb-rel-cancel")?.addEventListener("click", () => {
      state.relFormOpen = false;
      render();
    });
    root.querySelector("#nb-rel-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const fromKind = document.getElementById("nb-rel-from-kind").value;
      const fromId = document.getElementById("nb-rel-from-id").value.trim();
      const relation = document.getElementById("nb-rel-relation").value.trim();
      const toKind = document.getElementById("nb-rel-to-kind").value;
      const toId = document.getElementById("nb-rel-to-id").value.trim();
      const note = document.getElementById("nb-rel-note").value.trim();
      const author = document.getElementById("nb-rel-author").value.trim();
      if (!fromId || !toId || !relation || !author) { toast.error("From id, to id, relation, and author are required"); return; }
      const btn = e.submitter;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Adding…";
      try {
        await api.researchCreateRelationship(projectId, {
          id: crypto.randomUUID(),
          project_id: projectId,
          from_kind: fromKind,
          from_id: fromId,
          to_kind: toKind,
          to_id: toId,
          relation,
          note,
          author,
        });
        toast.success("Relationship added");
        state.relFormOpen = false;
        await loadRelationships();
      } catch (err) {
        toast.error(`Couldn't add relationship: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });

    root.querySelector("#nb-agr-run")?.addEventListener("click", async () => {
      const authorA = document.getElementById("nb-agr-a").value.trim();
      const authorB = document.getElementById("nb-agr-b").value.trim();
      const codeId = document.getElementById("nb-agr-code").value || undefined;
      if (!authorA || !authorB) { toast.error("Both authors are required"); return; }
      state.agreementLoading = true;
      state.agreementError = null;
      state.agreement = null;
      const host = document.getElementById("nb-agreement-host");
      if (host) host.innerHTML = agreementResultHtml();
      try {
        const resp = await api.researchAgreement(projectId, { authorA, authorB, codeId });
        state.agreement = resp;
      } catch (err) {
        state.agreementError = err.message || "Couldn't compute agreement.";
      } finally {
        state.agreementLoading = false;
        const h = document.getElementById("nb-agreement-host");
        if (h) h.innerHTML = agreementResultHtml();
      }
    });

    root.querySelector("#nb-export-json")?.addEventListener("click", async (e) => {
      const btn = e.currentTarget;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Exporting…";
      try {
        const resp = await api.researchExport(projectId, { format: "json" });
        downloadFile(`archive-${projectId}.json`, JSON.stringify(resp, null, 2), "application/json");
        toast.success("Export downloaded");
      } catch (err) {
        toast.error(`Couldn't export: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });
    root.querySelector("#nb-export-refi")?.addEventListener("click", async (e) => {
      const btn = e.currentTarget;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Exporting…";
      try {
        const resp = await api.researchExport(projectId, { format: "refi" });
        const content = typeof resp === "string" ? resp : JSON.stringify(resp, null, 2);
        downloadFile(`archive-${projectId}.qdpx.xml`, content, "application/xml");
        toast.success("Export downloaded");
      } catch (err) {
        toast.error(`Couldn't export: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });
  }

  async function loadMemos() {
    try {
      const resp = await api.researchMemos(projectId);
      state.memos = resp.memos || [];
    } catch (err) {
      state.memos = [];
      toast.error(`Couldn't load memos: ${err.message}`);
    }
    render();
  }

  async function loadRelationships() {
    try {
      const resp = await api.researchRelationships(projectId);
      state.relationships = resp.relationships || [];
    } catch (err) {
      state.relationships = [];
      toast.error(`Couldn't load relationships: ${err.message}`);
    }
    render();
  }

  async function load() {
    state.loading = true;
    state.error = null;
    render();
    try {
      const [sourcesResp, codesResp, codingsResp] = await Promise.all([
        api.researchSources(projectId),
        api.researchCodes(projectId),
        api.researchCodings(projectId).catch(() => ({ codings: [] })),
      ]);
      state.sources = sourcesResp.sources || [];
      state.codes = codesResp.codes || [];
      state.codings = codingsResp.codings || [];
      state.authors = [...new Set(state.codings.map((c) => c.author).filter(Boolean))];
    } catch (err) {
      state.error = err.message || "Couldn't load the notebook.";
      state.loading = false;
      render();
      return;
    }
    state.loading = false;
    await Promise.all([loadMemos(), loadRelationships()]);
  }

  load();

  return () => {
    // No document-level listeners registered; root gets torn down by the
    // router on navigation.
  };
}
