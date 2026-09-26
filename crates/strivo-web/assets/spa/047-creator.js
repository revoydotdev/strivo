// ── Authoring a pipeline graph (todo 53) ────────────────────────────
// The editor over the same `Pipeline` the executor runs and the read-only
// graph draws. It saves through `PUT /api/v1/pipelines/templates/<id>`, and
// the server refuses the same seven rules the browser refuses here — the
// mirror is held together by `crates/strivo-web/tests/pipeline_authoring.rs`,
// which reads THIS FILE and asserts the rule ids are the executor's.
//
// Three things this file refuses to do:
//
//   * It never resolves runnability. Rule 7 matches a dispatch against
//     `payload.registry.plugins`, which is `registered_plugin_names` read off
//     the executor's own registry and shipped to the browser. The browser
//     quotes the server's answer; it is not a second one.
//   * It never subscribes to the run SSE stream. A `PipelineUpdated` frame
//     carries the job table, not the registry verdict, and an editor that
//     merged the two would answer "can this run" from a frame that cannot.
//     The read-only graph marks its verdict stale instead; this surface is
//     not on that stream at all, which is why it needs no stale marking.
//   * It never lets a broken graph be saved. The save button is disabled
//     while any rule fires, so the invalid graph cannot reach the executor
//     even if the server's own validation were absent — and it is not.

const PA_RULE_LABELS = {
  acyclic: "The stages form a cycle",
  "empty-stage-list": "A pipeline needs at least one stage",
  "duplicate-stage-id": "Two stages share one id",
  "dangling-input": "A stage reads a stage that is not here",
  "zero-capacity": "An attempt count or resource cap is zero",
  "empty-dispatch": "A dispatch has an empty plugin or verb",
  "unknown-dispatch-target":
    "This build registers no plugin by that name, so it would fail at dispatch",
  "template-id": "The template id must be letters, digits, dash or underscore",
};

const PA_STAGE_KINDS = [
  "Extract",
  "Transcribe",
  "Diarize",
  "Subtitle",
  "Analyze",
  "ExportClip",
  "Concat",
  "Archive",
  "Custom",
];
// The three variants that carry a provider, so the editor shows the field
// only where `StageKind` actually has one.
const PA_PROVIDER_KINDS = ["Transcribe", "Diarize", "Analyze"];
const PA_LOCK_KINDS = ["Gpu", "Cpu", "Disk", "Api", "File"];

// One template under edit, plus what the payload said about it. Held at
// module scope because the editor re-renders whole on every keystroke; a
// store per field would be seven sources of truth for one graph.
let authorState = null;

function authorRuleLabel(rule) {
  return PA_RULE_LABELS[rule] || rule;
}

// `kind` as the tag name, from serde's externally tagged form: a unit
// variant serialises as the bare string, a variant with fields as a
// single-key object.
function authorKindTag(kind) {
  if (typeof kind === "string") return kind;
  const keys = Object.keys(kind || {});
  return keys[0] || "Custom";
}

function authorKindBody(kind) {
  const tag = authorKindTag(kind);
  if (typeof kind === "string") return "";
  const inner = kind[tag];
  return inner && typeof inner === "object" ? inner.provider || "" : "";
}

// Rebuild the exact JSON `StageKind` deserialises from, so a save sends the
// value the server will read back rather than a shape it has to guess at.
function authorKindJson(tag, body) {
  if (tag === "Custom") return { Custom: body || "" };
  if (PA_PROVIDER_KINDS.includes(tag)) return { [tag]: { provider: body || "" } };
  return tag;
}

function authorDispatchJson(stage) {
  if (!stage.dispatch) return null;
  return {
    plugin: String(stage.dispatch.plugin || ""),
    verb: String(stage.dispatch.verb || ""),
    selection: stage.dispatch.selection || [],
    payload:
      stage.dispatch.payload === undefined ? null : stage.dispatch.payload,
  };
}

function authorLocksJson(stage) {
  return (stage.requires || []).map((lock) => {
    const kind = authorKindTag(lock);
    if (kind === "Api") return { Api: { name: lock.Api.name, cap: lock.Api.cap } };
    if (kind === "Cpu" || kind === "Disk") return { [kind]: { cap: lock[kind].cap } };
    if (kind === "File") return { File: { path: lock.File.path } };
    return "Gpu";
  });
}

function authorStageJson(stage) {
  return {
    id: stage.id,
    name: stage.name,
    kind: stage.kind,
    inputs: (stage.inputs || []).slice(),
    state: stage.state === undefined ? "Pending" : stage.state,
    attempts: stage.attempts || 0,
    max_attempts: stage.max_attempts,
    fallback_provider: stage.fallback_provider === undefined
      ? null
      : stage.fallback_provider,
    requires: stage.requires || [],
    cost_cents_estimate: stage.cost_cents_estimate || 0,
    dispatch: authorDispatchJson(stage),
    artifacts: stage.artifacts || [],
  };
}

function authorPipelineJson() {
  const source = authorState.pipeline;
  return {
    id: source.id,
    name: source.name,
    stages: (source.stages || []).map(authorStageJson),
    state: "Pending",
    started_at_secs: null,
    completed_at_secs: null,
    subject_id: source.subject_id === undefined ? null : source.subject_id,
    trigger: source.trigger || "manual",
    error: null,
  };
}

// ── The six executor rules, mirrored ─────────────────────────────────
// Each returns `{rule, detail, node}` with `node` set when the violation is
// one stage's, so the editor can put the error on that row instead of in a
// summary the user has to read twice. The ids are the executor's
// (`PipelineRegistry::RULE_IDS`) and the seventh is
// `pipeline_graph::RULE_UNKNOWN_DISPATCH`; a test compares the two sets.
function authorValidate(stages, plugins) {
  const out = [];
  const byId = new Map();
  stages.forEach((stage) => {
    if (!byId.has(stage.id)) byId.set(stage.id, stage);
  });

  if (!stages.length) {
    out.push({
      rule: "empty-stage-list",
      detail: "Delete every stage and there is nothing to run.",
      node: null,
    });
  }

  const seen = new Set();
  const dupes = new Set();
  stages.forEach((stage) => {
    if (seen.has(stage.id)) dupes.add(stage.id);
    seen.add(stage.id);
  });
  dupes.forEach((id) => {
    const stage = stages.find((s) => s.id === id);
    out.push({
      rule: "duplicate-stage-id",
      detail: `Two stages share the id ${id}.`,
      node: stage ? stage.id : null,
    });
  });

  stages.forEach((stage) => {
    (stage.inputs || []).forEach((input) => {
      if (!byId.has(input)) {
        out.push({
          rule: "dangling-input",
          detail: `${stage.name || stage.id} reads ${input}, which is not a stage here.`,
          node: stage.id,
        });
      }
    });
  });

  stages.forEach((stage) => {
    if (!stage.max_attempts) {
      out.push({
        rule: "zero-capacity",
        detail: `${stage.name || stage.id} allows 0 attempts, so a failure is never retried.`,
        node: stage.id,
      });
    }
    (stage.requires || []).forEach((lock) => {
      const tag = authorKindTag(lock);
      const cap = tag === "Gpu" ? null : (lock[tag] || {}).cap;
      if (cap === 0) {
        out.push({
          rule: "zero-capacity",
          detail: `${stage.name || stage.id} locks ${tag} at 0, so it can never acquire it.`,
          node: stage.id,
        });
      }
    });
  });

  stages.forEach((stage) => {
    if (!stage.dispatch) return;
    if (!String(stage.dispatch.plugin || "").trim() || !String(stage.dispatch.verb || "").trim()) {
      out.push({
        rule: "empty-dispatch",
        detail: `${stage.name || stage.id} has a dispatch with an empty plugin or verb.`,
        node: stage.id,
      });
    }
  });

  // Rule 7. `plugins` is the executor registry's own name list, shipped in
  // the payload — see the file header. This matches against it and decides
  // nothing about any other property.
  stages.forEach((stage) => {
    if (!stage.dispatch) return;
    const plugin = String(stage.dispatch.plugin || "").trim();
    if (!plugin || (plugins || []).includes(plugin)) return;
    out.push({
      rule: "unknown-dispatch-target",
      detail: `${stage.name || stage.id} dispatches to ${plugin}, which this build does not register.`,
      node: stage.id,
    });
  });

  const cycle = authorCycle(stages, byId);
  if (cycle) {
    out.push({
      rule: "acyclic",
      detail: `${cycle.join(" → ")}. Remove one of those links.`,
      node: null,
    });
  }
  return out;
}

// The cycle in walk order, so the detail names the hop that closed the loop
// as its last element. The same reconstruction as `Pipeline::cycle` on the
// server; a boolean would name nothing the user can delete.
function authorCycle(stages, byId) {
  const visited = new Set();
  const walk = (id, stack) => {
    const at = stack.indexOf(id);
    if (at !== -1) return stack.slice(at).concat([id]);
    if (visited.has(id)) return null;
    stack.push(id);
    const stage = byId.get(id);
    if (stage) {
      for (const input of stage.inputs || []) {
        const found = walk(input, stack);
        if (found) return found;
      }
    }
    stack.pop();
    visited.add(id);
    return null;
  };
  for (const stage of stages) {
    const found = walk(stage.id, []);
    if (found) {
      return found.map((id) => {
        const stage = byId.get(id);
        return stage ? stage.name || id : id;
      });
    }
  }
  return null;
}

function authorNewId() {
  // `crypto.randomUUID` where available, and a shape the server's uuid
  // parser accepts either way. Ids are minted here, never accepted from the
  // user, which is why `duplicate-stage-id` is unreachable through the
  // editor's own controls.
  if (globalThis.crypto && typeof globalThis.crypto.randomUUID === "function") {
    return globalThis.crypto.randomUUID();
  }
  return "00000000-0000-4000-8000-" + String(Date.now()).padStart(12, "0");
}
