// corpus — Sources browser, cases, and the signal browser.
//
// Creator Edition only. Imported from spa.js inside a creator block; this whole
// directory is removed from the PVR build by crates/strivo-web/build.rs.
//
// Contract: mount(root, ctx) renders the surface into `root`.
//   ctx = { api, projectId, toast, fmt }
// Return a cleanup function if the surface registers listeners on document.

export function mount(root, ctx) {
  const { api, projectId, toast, fmt } = ctx;
  const esc = fmt.escapeHtml;

  const state = {
    sources: [],
    cases: [],
    signals: [],
    signalsOffset: 0,
    signalsLimit: 20,
    signalsHasMore: false,
    signalKind: "",
    signalSourceId: "",
    caseFormOpen: false,
    assignCaseId: "",
    loading: true,
    error: null,
  };

  function durationHtml(ms) {
    if (ms == null) return "—";
    return fmt.clock(ms / 1000);
  }

  function sourceRowHtml(s) {
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          <span class="arc-row-title">${esc(s.title)}</span>
          <span class="cfg-badge">${esc(s.kind)}</span>
          <span class="arc-row-time">${durationHtml(s.duration_ms)}</span>
          ${s.recording_id ? `<span class="pg-cap-hint">linked recording</span>` : `<span class="pg-cap-hint">no local recording</span>`}
        </div>
      </div>`;
  }

  function sourcesHtml() {
    if (!state.sources.length) return `<div class="empty sm">No sources indexed in this workspace yet.</div>`;
    return state.sources.map(sourceRowHtml).join("");
  }

  function caseOptionsHtml() {
    return state.cases
      .map((c) => `<option value="${esc(c.id)}">${esc(c.name)}</option>`)
      .join("");
  }

  function sourceOptionsHtml() {
    return state.sources
      .map((s) => `<option value="${esc(s.id)}">${esc(s.title)}</option>`)
      .join("");
  }

  function newCaseFormHtml() {
    if (!state.caseFormOpen) return "";
    return `
      <form class="cfg-card cb-inline-form" id="cp-case-form">
        <h3 class="cfg-title">New case</h3>
        <label class="arc-field">Name
          <input id="cp-case-name" class="arc-input" type="text" required maxlength="120"/>
        </label>
        <label class="arc-field">Description <span class="pg-cap-hint">(optional)</span>
          <textarea id="cp-case-desc" class="arc-input" rows="2" maxlength="2000"></textarea>
        </label>
        <div class="arc-moment-actions">
          <button type="button" class="sm" id="cp-case-cancel">Cancel</button>
          <button type="submit" class="btn-primary">Create case</button>
        </div>
      </form>`;
  }

  function caseRowHtml(c) {
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          <span class="arc-row-title">${esc(c.name)}</span>
        </div>
        ${c.description ? `<p class="arc-row-snippet">${esc(c.description)}</p>` : ""}
        <div class="arc-row-actions">
          <select class="arc-select cp-assign-source" data-case-id="${esc(c.id)}" ${state.sources.length ? "" : "disabled"}>
            <option value="">Assign a source…</option>
            ${sourceOptionsHtml()}
          </select>
        </div>
      </div>`;
  }

  function casesHtml() {
    if (!state.cases.length) return `<div class="empty sm">No cases yet.</div>`;
    return state.cases.map(caseRowHtml).join("");
  }

  function signalRowHtml(sig) {
    const src = state.sources.find((s) => s.id === sig.source_id);
    const conf = sig.confidence != null ? `<span class="cfg-badge">${Math.round(sig.confidence * 100)}%</span>` : "";
    return `
      <div class="arc-row">
        <div class="arc-row-main">
          <span class="cfg-badge">${esc(sig.kind)}</span>
          <span class="arc-row-title">${src ? esc(src.title) : "(unresolved source)"}</span>
          <span class="arc-row-time">${fmt.clock(sig.start_ms / 1000)} → ${fmt.clock(sig.end_ms / 1000)}</span>
          ${conf}
        </div>
        <p class="arc-row-snippet">${esc(sig.label)}</p>
      </div>`;
  }

  function signalsHtml() {
    if (!state.signals.length) return `<div class="empty sm">No signals match this filter.</div>`;
    return state.signals.map(signalRowHtml).join("");
  }

  function render() {
    if (state.loading) {
      root.innerHTML = `<div class="empty sm" aria-busy="true">Loading corpus…</div>`;
      return;
    }
    if (state.error) {
      root.innerHTML = `
        <div class="empty sm"><div class="glyph">⚠</div>${esc(state.error)}
          <button class="sm" id="cp-retry" type="button">Retry</button>
        </div>`;
      root.querySelector("#cp-retry")?.addEventListener("click", load);
      return;
    }
    root.innerHTML = `
      <div class="arc-grid cb-grid">
        <section class="cfg-card">
          <h2 class="cfg-title">Sources</h2>
          <div class="cb-codings-host">${sourcesHtml()}</div>
        </section>
        <section class="cfg-card">
          <div class="cb-card-head">
            <h2 class="cfg-title">Cases</h2>
            <button class="sm" id="cp-new-case-btn" type="button">＋ New case</button>
          </div>
          ${newCaseFormHtml()}
          <div class="cb-codings-host">${casesHtml()}</div>
        </section>
      </div>
      <section class="cfg-card" style="margin-top:0.9rem">
        <h2 class="cfg-title">Signal browser</h2>
        <div class="arc-moments-filter">
          <label for="cp-sig-kind">Kind</label>
          <input id="cp-sig-kind" class="arc-input" type="text" placeholder="any" value="${esc(state.signalKind)}"/>
          <label for="cp-sig-source">Source</label>
          <select id="cp-sig-source" class="arc-select">
            <option value="">any</option>
            ${state.sources.map((s) => `<option value="${esc(s.id)}" ${s.id === state.signalSourceId ? "selected" : ""}>${esc(s.title)}</option>`).join("")}
          </select>
          <button class="sm" id="cp-sig-apply" type="button">Apply</button>
        </div>
        <div id="cp-signals-host" aria-live="polite">${signalsHtml()}</div>
        <div class="arc-pager" id="cp-signals-pager" ${state.signalsOffset === 0 && !state.signalsHasMore ? "hidden" : ""}>
          <button class="sm" id="cp-signals-prev" type="button" ${state.signalsOffset === 0 ? "disabled" : ""}>← Prev</button>
          <span class="pg-cap-hint">${state.signals.length ? `${state.signalsOffset + 1}–${state.signalsOffset + state.signals.length}` : ""}</span>
          <button class="sm" id="cp-signals-next" type="button" ${state.signalsHasMore ? "" : "disabled"}>Next →</button>
        </div>
      </section>`;
    wire();
  }

  function wire() {
    root.querySelector("#cp-new-case-btn")?.addEventListener("click", () => {
      state.caseFormOpen = !state.caseFormOpen;
      render();
    });
    root.querySelector("#cp-case-cancel")?.addEventListener("click", () => {
      state.caseFormOpen = false;
      render();
    });
    root.querySelector("#cp-case-form")?.addEventListener("submit", async (e) => {
      e.preventDefault();
      const name = document.getElementById("cp-case-name").value.trim();
      const description = document.getElementById("cp-case-desc").value.trim();
      if (!name) { toast.error("Name is required"); return; }
      const btn = e.submitter;
      const prev = btn.textContent;
      btn.disabled = true; btn.textContent = "Creating…";
      try {
        await api.researchCreateCase(projectId, {
          id: crypto.randomUUID(),
          project_id: projectId,
          name,
          description,
        });
        toast.success("Case created");
        state.caseFormOpen = false;
        await loadCases();
      } catch (err) {
        toast.error(`Couldn't create case: ${err.message}`);
      } finally {
        if (btn.isConnected) { btn.disabled = false; btn.textContent = prev; }
      }
    });

    root.querySelectorAll(".cp-assign-source").forEach((sel) => {
      sel.addEventListener("change", async () => {
        const sourceId = sel.value;
        const caseId = sel.dataset.caseId;
        if (!sourceId) return;
        sel.disabled = true;
        try {
          await api.researchAddCaseSource(projectId, caseId, sourceId);
          toast.success("Source assigned to case");
        } catch (err) {
          toast.error(`Couldn't assign source: ${err.message}`);
        } finally {
          if (sel.isConnected) { sel.disabled = false; sel.value = ""; }
        }
      });
    });

    root.querySelector("#cp-sig-apply")?.addEventListener("click", () => {
      state.signalKind = document.getElementById("cp-sig-kind").value.trim();
      state.signalSourceId = document.getElementById("cp-sig-source").value;
      state.signalsOffset = 0;
      loadSignals();
    });
    root.querySelector("#cp-signals-prev")?.addEventListener("click", () => {
      state.signalsOffset = Math.max(0, state.signalsOffset - state.signalsLimit);
      loadSignals();
    });
    root.querySelector("#cp-signals-next")?.addEventListener("click", () => {
      state.signalsOffset += state.signalsLimit;
      loadSignals();
    });
  }

  async function loadCases() {
    try {
      const resp = await api.researchCases(projectId);
      state.cases = resp.cases || [];
    } catch (err) {
      state.cases = [];
      toast.error(`Couldn't load cases: ${err.message}`);
    }
    render();
  }

  async function loadSignals() {
    const host = document.getElementById("cp-signals-host");
    if (host) host.innerHTML = `<div class="empty sm">Loading…</div>`;
    try {
      const resp = await api.researchSignals(projectId, {
        kind: state.signalKind || undefined,
        sourceId: state.signalSourceId || undefined,
        limit: state.signalsLimit,
        offset: state.signalsOffset,
      });
      const signals = resp.signals || [];
      state.signals = signals;
      state.signalsHasMore = signals.length === state.signalsLimit;
    } catch (err) {
      state.signals = [];
      state.signalsHasMore = false;
      toast.error(`Couldn't load signals: ${err.message}`);
    }
    render();
  }

  async function load() {
    state.loading = true;
    state.error = null;
    render();
    try {
      const sourcesResp = await api.researchSources(projectId);
      state.sources = sourcesResp.sources || [];
    } catch (err) {
      state.error = err.message || "Couldn't load the corpus.";
      state.loading = false;
      render();
      return;
    }
    state.loading = false;
    await Promise.all([loadCases(), loadSignals()]);
  }

  load();

  return () => {
    // No document-level listeners registered; root gets torn down by the
    // router on navigation.
  };
}
