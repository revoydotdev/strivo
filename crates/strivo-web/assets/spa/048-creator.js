// ── The editor surface (todo 53) ────────────────────────────────────
// Rendering and wiring. The rules and the graph algebra are in 047; this
// file only shows what they said and sends what they allowed.
//
// The whole editor re-renders from `authorState` on every change, and every
// control is reached by delegation from the editor root. One delegated
// handler means a re-render cannot leave a listener bound to a row it
// replaced — the same reasoning as `graphDispose` on the read-only side.

function authorStyles() {
  if (document.getElementById("pa-styles")) return;
  const style = document.createElement("style");
  style.id = "pa-styles";
  // Token names only, no colour literal, for the same reason 044's sheet
  // uses them: `check-design-tokens.mjs` never sees this text, so a literal
  // here would make its zero meaningless.
  style.textContent = `
.pa-editor{border:1px solid var(--border);border-radius:var(--radius-md,8px);
  background:var(--surface);padding:var(--space-2,8px)}
.pa-row{display:grid;gap:var(--space-1,4px);border:1px solid var(--border-lighter);
  border-radius:var(--radius-sm,4px);padding:var(--space-2,8px);margin-bottom:var(--space-2,8px);
  background:var(--surface-overlay)}
.pa-row[data-pa-broken="true"]{border-color:var(--recording)}
.pa-fields{display:flex;flex-wrap:wrap;gap:var(--space-1,4px);align-items:center}
.pa-fields label{display:flex;gap:var(--space-1,4px);align-items:center;
  font-size:var(--text-xs,12px);color:var(--fg-muted)}
.pa-chip{display:inline-flex;gap:var(--space-1,4px);align-items:center;
  border:1px solid var(--border);border-radius:var(--radius-sm,4px);
  padding:0 var(--space-1,4px);font-size:var(--text-xs,12px);color:var(--fg)}
.pa-error{margin:0;font-size:var(--text-xs,12px);color:var(--recording)}
.pa-error::before{content:"\\2715 "}
.pa-summary{font-size:var(--text-xs,12px)}
.pa-summary[data-pa-ok="false"]{color:var(--recording)}
.pa-status{font-size:var(--text-xs,12px);color:var(--fg-muted)}
.pa-status[data-pa-outcome="conflict"]{color:var(--recording)}
.pa-choose{display:flex;flex-wrap:wrap;gap:var(--space-1,4px);margin-bottom:var(--space-2,8px)}
`;
  document.head.appendChild(style);
}

function authorLockJson(tag, cap, name, path) {
  if (tag === "Gpu") return "Gpu";
  if (tag === "File") return { File: { path: path || "" } };
  if (tag === "Api") return { Api: { name: name || "", cap: Number(cap) || 0 } };
  return { [tag]: { cap: Number(cap) || 0 } };
}

function authorNodeRow(state, stage, errors) {
  const mine = errors.filter((e) => e.node === stage.id);
  const tag = authorKindTag(stage.kind);
  const dispatch = stage.dispatch || null;
  const options = (values, current) =>
    values
      .map(
        (v) =>
          `<option value="${htmlEscape(v)}"${
            v === current ? " selected" : ""
          }>${htmlEscape(v)}</option>`,
      )
      .join("");
  const others = state.pipeline.stages.filter((s) => s.id !== stage.id);
  return `<div class="pa-row" data-pa-node="${htmlEscape(String(stage.id))}"
      data-pa-broken="${mine.length ? "true" : "false"}">
    <div class="pa-fields">
      <label>Name <input data-pa-field="name" value="${htmlEscape(
        graphTruncate(String(stage.name || ""), 60),
      )}" size="18"></label>
      <label>Kind <select data-pa-field="kind">${options(PA_STAGE_KINDS, tag)}</select></label>
      ${
        PA_PROVIDER_KINDS.includes(tag) || tag === "Custom"
          ? `<label>${tag === "Custom" ? "Identifier" : "Provider"} <input
              data-pa-field="provider" value="${htmlEscape(
                graphTruncate(authorKindBody(stage.kind), 40),
              )}" size="14"></label>`
          : ""
      }
      <label>Attempts <input data-pa-field="max_attempts" type="number" min="0" max="9"
        value="${htmlEscape(String(stage.max_attempts))}" size="2"></label>
      <label>Fallback <input data-pa-field="fallback_provider" value="${htmlEscape(
        graphTruncate(String(stage.fallback_provider || ""), 40),
      )}" size="12"></label>
      <button type="button" class="sm" data-pa-action="delete">Delete stage</button>
    </div>
    <div class="pa-fields">
      <label><input type="checkbox" data-pa-field="has_dispatch"${
        dispatch ? " checked" : ""
      }> dispatches</label>
      <label>Plugin <input data-pa-field="plugin" value="${htmlEscape(
        graphTruncate(String((dispatch && dispatch.plugin) || ""), 40),
      )}" size="14"></label>
      <label>Verb <input data-pa-field="verb" value="${htmlEscape(
        graphTruncate(String((dispatch && dispatch.verb) || ""), 40),
      )}" size="14"></label>
    </div>
    <div class="pa-fields">
      <span class="pg-cap-hint">Locks</span>
      ${
        (stage.requires || [])
          .map(
            (lock, i) =>
              `<span class="pa-chip" data-pa-lock="${i}">${htmlEscape(
                authorLockLabel(lock),
              )}<button type="button" class="sm" data-pa-action="unlock"
                aria-label="Remove lock">×</button></span>`,
          )
          .join("") || '<span class="pg-cap-hint">none</span>'
      }
      <select data-pa-field="lock_kind">${options(
        PA_LOCK_KINDS,
        state.pending.lockKind,
      )}</select>
      <input data-pa-field="lock_cap" type="number" min="0"
             value="${htmlEscape(state.pending.lockCap)}" size="2"
             aria-label="Lock capacity">
      <input data-pa-field="lock_name" value="${htmlEscape(state.pending.lockName)}"
             size="8" aria-label="Lock name" placeholder="name/path">
      <button type="button" class="sm" data-pa-action="lock">Add lock</button>
    </div>
    <div class="pa-fields">
      <span class="pg-cap-hint">Runs after</span>
      ${
        (stage.inputs || [])
          .map(
            (input) =>
              `<span class="pa-chip" data-pa-input="${htmlEscape(String(input))}">${htmlEscape(
                graphTruncate(authorStageName(input), 24),
              )}<button type="button" class="sm" data-pa-action="unlink"
                aria-label="Remove link">×</button></span>`,
          )
          .join("") || '<span class="pg-cap-hint">nothing — it is a root</span>'
      }
      <select data-pa-field="input">${options(
        others.map((s) => s.id),
        state.pending.input,
      )}</select>
      <button type="button" class="sm" data-pa-action="connect">Connect</button>
    </div>
    ${mine
      .map(
        (e) =>
          `<p class="pa-error" data-pa-error="${htmlEscape(e.rule)}"
             data-pa-node="${htmlEscape(String(stage.id))}">${htmlEscape(
             `${authorRuleLabel(e.rule)}. ${e.detail}`,
           )}</p>`,
      )
      .join("")}
  </div>`;
}

// `ResourceLock::label()`'s spelling, not serde's variant name: the graph and
// the detail pane both show `cpu:2`, and a lock chip reading `Cpu:2` beside a
// detail line reading `cpu:2` reads as two different locks.
function authorLockLabel(lock) {
  const tag = authorKindTag(lock);
  if (tag === "Gpu") return "gpu";
  if (tag === "File") return `file:${(lock.File || {}).path || ""}`;
  if (tag === "Api") return `api:${(lock.Api || {}).name || ""}:${(lock.Api || {}).cap}`;
  return `${tag.toLowerCase()}:${(lock[tag] || {}).cap}`;
}

function authorStageName(id) {
  const stage = (authorState.pipeline.stages || []).find((s) => s.id === id);
  return stage ? stage.name || id : `${id} (missing)`;
}

function authorEditorHtml(errors) {
  const global = errors.filter((e) => !e.node);
  const state = authorState;
  if (!state.pending) {
    state.pending = { lockKind: "Gpu", lockCap: "1", lockName: "", input: "" };
  }
  return `<section class="cfg-card pa-editor" id="pa-editor"
      data-pa-template="${htmlEscape(state.id)}"
      data-pa-source="${htmlEscape(state.source)}"
      data-pa-valid="${errors.length ? "false" : "true"}"
      data-pa-error-count="${errors.length}">
    <header class="pl-pipe-head">
      <h2 class="cfg-title">Author a pipeline
        <span class="pg-cap-hint">the executor's own stages, validated before they can run</span>
      </h2>
      <button type="button" class="btn-primary sm" id="pa-save"
        ${errors.length ? 'disabled aria-disabled="true"' : ""}
        title="${
          errors.length
            ? htmlEscape(`${errors.length} problem${errors.length === 1 ? "" : "s"} to fix first`)
            : "Save this template"
        }">Save template</button>
    </header>
    <div class="pa-choose" role="group" aria-label="Template to edit">
      ${state.templates
        .map(
          (t) =>
            `<button type="button" class="sm" data-pa-pick="${htmlEscape(t.id)}"${
              t.id === state.id ? ' aria-pressed="true"' : ""
            }>${htmlEscape(t.id)}</button>`,
        )
        .join("")}
      <input data-pa-field="template_id" value="${htmlEscape(state.id)}" size="12"
             aria-label="Save as template id">
      <button type="button" class="sm" data-pa-action="new">New template</button>
    </div>
    <p class="pa-summary" data-pa-summary data-pa-ok="${errors.length ? "false" : "true"}"
       role="status">${
         errors.length
           ? htmlEscape(
               errors
                 .map((e) => `${authorRuleLabel(e.rule)} — ${e.detail}`)
                 .join(" · "),
             )
           : htmlEscape(
               `Every rule holds: ${state.pipeline.stages.length} stage${
                 state.pipeline.stages.length === 1 ? "" : "s"
               }, ready to save.`,
             )
       }</p>
    <div data-pa-globals>
      ${global
        .map(
          (e) =>
            `<p class="pa-error" data-pa-error="${htmlEscape(e.rule)}">${htmlEscape(
              `${authorRuleLabel(e.rule)}. ${e.detail}`,
            )}</p>`,
        )
        .join("")}
    </div>
    <div data-pa-nodes>
      ${(state.pipeline.stages || []).map((s) => authorNodeRow(state, s, errors)).join("")}
    </div>
    <div class="pa-fields">
      <button type="button" class="sm" data-pa-action="add">Add stage</button>
      <span class="pa-status" data-pa-status role="status" aria-live="polite"></span>
    </div>
  </section>`;
}

// `focus` is `{node, field, start, end}` for the control the user is in.
// The editor re-renders whole on every keystroke, so without this the field
// would be replaced under the caret after one character — the errors have to
// be live, and losing the caret on every key is not a way to get them.
function authorRepaint(status, focus) {
  const host = document.getElementById("pa-editor-host");
  if (!host || !authorState) return;
  const errors = authorValidate(
    authorState.pipeline.stages || [],
    authorState.plugins,
  );
  host.innerHTML = authorEditorHtml(errors);
  if (focus) {
    const row = host.querySelector(`[data-pa-node="${cssEscape(focus.node)}"]`);
    const el =
      (row && row.querySelector(`[data-pa-field="${focus.field}"]`)) ||
      host.querySelector(`[data-pa-field="${focus.field}"]`);
    if (el && el.focus) {
      el.focus();
      if (typeof el.setSelectionRange === "function" && focus.start != null) {
        try {
          el.setSelectionRange(focus.start, focus.end);
        } catch (_) {}
      }
    }
  }
  if (status) {
    const el = host.querySelector("[data-pa-status]");
    if (el) {
      el.dataset.paOutcome = status.outcome || "ok";
      el.textContent = status.text || "";
    }
  }
  authorState.errors = errors;
}

// The four controls that describe a NOT-YET-MADE edge or lock. They live in
// `authorState` rather than only in the DOM because the editor re-renders
// whole on every keystroke: a control that is not bound is reset by the
// repaint, so "choose Cpu, set the cap to 0, press Add" silently adds a Gpu
// lock. This is the whole reason they are read here rather than at the
// button.
const PA_PENDING_FIELDS = {
  lock_kind: "lockKind",
  lock_cap: "lockCap",
  lock_name: "lockName",
  input: "input",
};

function authorReadPending(target) {
  const field = target && target.dataset ? target.dataset.paField : null;
  const key = field ? PA_PENDING_FIELDS[field] : null;
  if (key) authorState.pending[key] = target.value;
}

// Re-read the controls of one row into the graph, then repaint. Reading the
// whole row on every change (rather than patching the one field) is what
// keeps a re-render from being able to disagree with the state it renders.
function authorReadRow(row) {
  const stage = authorState.pipeline.stages.find(
    (s) => s.id === row.dataset.paNode,
  );
  if (!stage) return;
  const field = (name) => row.querySelector(`[data-pa-field="${name}"]`);
  const tag = authorKindTag(stage.kind);
  const nextKind = field("kind") ? field("kind").value : tag;
  if (nextKind !== tag) {
    stage.kind = authorKindJson(nextKind, "");
  } else if (field("provider")) {
    stage.kind = authorKindJson(tag, field("provider").value);
  }
  stage.name = field("name").value;
  stage.max_attempts = Number(field("max_attempts").value) || 0;
  const fallback = field("fallback_provider").value.trim();
  stage.fallback_provider = fallback || null;
  if (field("has_dispatch").checked) {
    stage.dispatch = {
      plugin: field("plugin").value,
      verb: field("verb").value,
      selection: (stage.dispatch && stage.dispatch.selection) || [],
      payload: stage.dispatch ? stage.dispatch.payload : null,
    };
  } else {
    stage.dispatch = null;
  }
  if (field("template_id")) {
    const wanted = field("template_id").value.trim();
    if (wanted) authorState.id = wanted;
  }
}

async function authorSave() {
  const errors = authorState.errors || [];
  if (errors.length) return;
  const status = { outcome: "ok", text: "" };
  try {
    const response = await fetch(
      `/api/v1/pipelines/templates/${encodeURIComponent(authorState.id)}`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          base_revision: authorState.source === "authored" ? authorState.revision : null,
          pipeline: authorPipelineJson(),
        }),
      },
    );
    const body = await response.json().catch(() => ({}));
    if (response.ok) {
      authorState.revision = body.revision;
      authorState.source = "authored";
      status.text = `Saved as ${body.id} at revision ${body.revision}.`;
    } else if (response.status === 409) {
      // Todo 49's refusal, reported and not resolved: two clients hold
      // different edits and only this one can say which survives.
      status.outcome = "conflict";
      status.text = `Not saved: this edit was made against revision ${body.base}, but ${body.current} is now current. Reload to see the other edit, then reapply yours.`;
    } else {
      status.outcome = "invalid";
      status.text = `Not saved: ${body.detail || "the server refused this graph"}.`;
    }
  } catch (e) {
    status.outcome = "failed";
    status.text = `Not saved: ${e && e.message ? e.message : String(e)}`;
  }
  authorRepaint(status);
}

function authorBind(host) {
  // `change` is the fallback for a control that fired no `input`. Chromium
  // fires `input` on a `<select>`, so repainting again from `change` would
  // replace the DOM twice per interaction.
  //
  // Repaint ONLY when the graph itself moved. The template id and the four
  // pending controls change what the NEXT save will send, not what is
  // valid, and a repaint for them replaces the button the user is reaching
  // for — a click that then lands on a detached node and silently does
  // nothing.
  host.addEventListener("change", (event) => {
    authorReadPending(event.target);
    const before = JSON.stringify(authorState.pipeline);
    const row = event.target.closest("[data-pa-node]");
    if (row) authorReadRow(row);
    const picker = event.target.closest("[data-pa-field='template_id']");
    if (picker) authorState.id = picker.value.trim() || authorState.id;
    if (JSON.stringify(authorState.pipeline) !== before) authorRepaint();
  });
  host.addEventListener("input", (event) => {
    authorReadPending(event.target);
    const row = event.target.closest("[data-pa-node]");
    if (!row || !event.target.dataset.paField) return;
    authorReadRow(row);
    authorRepaint(null, {
      node: row.dataset.paNode,
      field: event.target.dataset.paField,
      start: event.target.selectionStart,
      end: event.target.selectionEnd,
    });
  });
  host.addEventListener("click", async (event) => {
    const pick = event.target.closest("[data-pa-pick]");
    if (pick) {
      authorSelectTemplate(pick.dataset.paPick);
      return;
    }
    if (event.target.closest("#pa-save")) {
      await authorSave();
      return;
    }
    const action = event.target.closest("[data-pa-action]");
    if (!action) return;
    const row = action.closest("[data-pa-node]");
    const stages = authorState.pipeline.stages;
    switch (action.dataset.paAction) {
      case "add": {
        stages.push({
          id: authorNewId(),
          name: `Stage ${stages.length + 1}`,
          kind: "Extract",
          inputs: [],
          state: "Pending",
          attempts: 0,
          max_attempts: 3,
          fallback_provider: null,
          requires: [],
          cost_cents_estimate: 0,
          dispatch: { plugin: "", verb: "", selection: [], payload: null },
          artifacts: [],
        });
        break;
      }
      case "new": {
        authorState.id = "";
        authorState.source = "authored";
        authorState.revision = null;
        authorState.pipeline = {
          id: authorNewId(),
          name: "Untitled pipeline",
          stages: [],
          trigger: "manual",
          subject_id: null,
        };
        break;
      }
      case "delete": {
        // A dependent's `inputs` entry is deliberately NOT rewritten here.
        // Silently rewiring would hide a link the user made, and the
        // dangling-input rule exists to name it.
        stages.splice(
          stages.findIndex((s) => s.id === row.dataset.paNode),
          1,
        );
        break;
      }
      case "connect": {
        const stage = stages.find((s) => s.id === row.dataset.paNode);
        const chosen = authorState.pending.input;
        if (stage && chosen && !stage.inputs.includes(chosen)) {
          stage.inputs.push(chosen);
          authorState.pending.input = "";
        }
        break;
      }
      case "unlink": {
        const stage = stages.find((s) => s.id === row.dataset.paNode);
        const chip = action.closest("[data-pa-input]");
        if (stage && chip) {
          stage.inputs = stage.inputs.filter((i) => i !== chip.dataset.paInput);
        }
        break;
      }
      case "lock": {
        const stage = stages.find((s) => s.id === row.dataset.paNode);
        if (stage) {
          const pending = authorState.pending;
          stage.requires.push(
            authorLockJson(
              pending.lockKind,
              pending.lockCap,
              pending.lockName,
              pending.lockName,
            ),
          );
        }
        break;
      }
      case "unlock": {
        const stage = stages.find((s) => s.id === row.dataset.paNode);
        const chip = action.closest("[data-pa-lock]");
        if (stage && chip) {
          stage.requires.splice(Number(chip.dataset.paLock), 1);
        }
        break;
      }
      default:
        return;
    }
    authorRepaint();
  });
}

function authorSelectTemplate(id) {
  const found = authorState.templates.find((t) => t.id === id);
  if (!found) return;
  authorState.id = found.id;
  authorState.source = found.source;
  authorState.revision = found.revision;
  authorState.pipeline = JSON.parse(JSON.stringify(found.pipeline));
  authorRepaint();
}

async function renderPipelineAuthoring() {
  authorStyles();
  const host = document.getElementById("pa-editor-host");
  if (!host) return;
  const payload = await API
    .pipelineTemplates()
    .catch(() => null);
  if (!payload || !Array.isArray(payload.templates)) {
    host.innerHTML = graphStatePanel(
      GRAPH_STATES.FAILED,
      "This build of the API does not serve pipeline templates.",
      "Nothing is editable here rather than something invented in the browser.",
    );
    return;
  }
  authorState = {
    templates: payload.templates,
    plugins: (payload.registry && payload.registry.plugins) || [],
    id: payload.templates.length ? payload.templates[0].id : "",
    source: payload.templates.length ? payload.templates[0].source : "authored",
    revision: payload.templates.length ? payload.templates[0].revision : null,
    pipeline: payload.templates.length
      ? JSON.parse(JSON.stringify(payload.templates[0].pipeline))
      : { id: authorNewId(), name: "Untitled pipeline", stages: [], trigger: "manual" },
    errors: [],
    pending: { lockKind: "Gpu", lockCap: "1", lockName: "", input: "" },
  };
  authorBind(host);
  authorRepaint();
  if ((payload.unreadable || []).length) {
    const note = host.querySelector("[data-pa-summary]");
    if (note) {
      note.textContent += ` Unreadable stored templates: ${payload.unreadable.join(" · ")}`;
    }
  }
}
