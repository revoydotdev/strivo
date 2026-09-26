// ── The pipeline graph (todo 52) ─────────────────────────────────────
//
// The nodes are the executor's own `Stage`s and the edges are those stages'
// `inputs` links. The payload arrives on `GET /api/v1/pipelines/runs` as the
// `graph` key, built server-side by `routes::pipeline_graph::graph_payload`
// from the same `Pipeline` the daemon submits and runs — there is no
// blueprint table behind this view and no second "what can run" answer.
//
// Three things this file is careful about:
//
//   * X6 ships UMD. A side-effect `import()` is legal in ESM; a static
//     `import { Graph }` is a link-time SyntaxError that would take down the
//     whole concatenated spa.js. The namespace has to be read off
//     globalThis, and the import is bounded so a stalled fetch reaches a
//     named state instead of leaving the await pending (todo 43 left that
//     open; this is the fix, not a repeat).
//   * Runnability is *never* computed here. `data-pg-runnable` is copied off
//     the payload, so the browser cannot become a second answer to "can this
//     run". The two independent facts the node shows are the job table's
//     `state` and the executor registry's verdict, and they are rendered as
//     two separate attributes so a disagreement is visible rather than
//     reconciled away.
//   * Node identity is untrusted text reaching a DOM button and a tooltip, so
//     it is escaped and truncated, exactly as the timeline does it.

const GRAPH_STATES = {
  EMPTY: "empty",
  DEGRADED: "degraded",
  FAILED: "failed",
  MALFORMED: "malformed",
  VENDOR: "vendor-load-failed",
};

// A stalled vendor response must not hold the surface in a pending await.
// The import itself cannot be aborted, so the timer rejects the *wait* and
// the panel names the timeout; nothing downstream stays suspended.
const GRAPH_VENDOR_TIMEOUT_MS = 8000;
const GRAPH_NODE_W = 216;
const GRAPH_NODE_H = 96;
const GRAPH_COL_GAP = 64;
const GRAPH_ROW_GAP = 24;

let graphVendor = null;

async function loadGraphVendor() {
  if (graphVendor) return graphVendor;
  graphVendor = (async () => {
    const out = { X6: null, error: null, state: null };
    try {
      await graphDeadline(
        import("/assets/vendor/x6.umd.js"),
        GRAPH_VENDOR_TIMEOUT_MS,
      );
      const ns = globalThis.X6;
      // `Object.keys(namespace)` is empty on this artifact, so a named
      // binding is not merely unavailable — it is a link error. Read the
      // global, and check it actually attached rather than trusting the
      // import resolving.
      if (!ns || typeof ns.Graph !== "function") {
        out.state = GRAPH_STATES.VENDOR;
        out.error = "the X6 bundle imported but attached no Graph to globalThis";
        return out;
      }
      // 3.1.8's built-in `html` shape has NO default content source: its view
      // reads `HTML.register`'s table, which nothing populates, so
      // `addNode({ shape: "html", html })` renders a permanently empty
      // foreignObject. Registering the source is what makes the HTML shape
      // work at all — and it is the whole reason the library was chosen,
      // since the node content is real DOM.
      if (ns.Shape && ns.Shape.HTML && typeof ns.Shape.HTML.register === "function") {
        ns.Shape.HTML.register({
          shape: "html",
          html: (cell) => String((cell.getData() || {}).markup || ""),
        });
      }
      out.X6 = ns;
    } catch (e) {
      out.state = GRAPH_STATES.VENDOR;
      out.error = e instanceof Error ? e.message : String(e);
    }
    return out;
  })();
  return graphVendor;
}

function graphDeadline(promise, ms) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`no response within ${ms} ms`)),
      ms,
    );
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function graphStyles() {
  if (document.getElementById("pg-styles")) return;
  const style = document.createElement("style");
  style.id = "pg-styles";
  // DESIGN.md token names only, no colour literal: `check-design-tokens.mjs`
  // audits the assembled sheet and never sees this, so its zero says nothing
  // about these rules. The reason they hold is that every value is a token.
  style.textContent = `
.pg-surface{border:1px solid var(--border);border-radius:var(--radius-md,8px);
  background:var(--surface);padding:var(--space-2,8px);overflow:auto}
.pg-canvas{min-height:120px;position:relative}
.pg-node{display:block;width:100%;text-align:left;border-radius:var(--radius-sm,4px);
  background:var(--surface-overlay);color:var(--fg);border:1px solid var(--border-lighter);
  border-left:3px solid var(--accent2);padding:var(--space-1,4px) var(--space-2,8px);
  font-size:var(--text-xs,12px);cursor:pointer;overflow:hidden}
.pg-node[data-pg-runnable="false"]{border-left-color:var(--secondary);border-style:dashed}
.pg-node[data-pg-stage-state="Failed"],.pg-node[data-pg-stage-state="Exhausted"]{border-left-color:var(--recording)}
.pg-node[aria-pressed="true"]{outline:2px solid var(--primary);outline-offset:-2px}
.pg-node:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}
.pg-node-name{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.pg-node-meta{display:block;color:var(--fg-muted);overflow:hidden;
  text-overflow:ellipsis;white-space:nowrap}
.pg-state{padding:var(--space-2,8px);border-radius:var(--radius-sm,4px);
  background:var(--surface);border:1px solid var(--border);font-size:var(--text-xs,12px)}
.pg-state[data-kind="degraded"]{border-color:var(--secondary)}
.pg-state[data-kind="failed"]{border-color:var(--recording)}
.pg-state[data-kind="malformed"]{border-color:var(--recording)}
.pg-list{display:grid;gap:var(--space-2,8px);grid-template-columns:repeat(auto-fill,minmax(216px,1fr))}
.pg-detail{font-size:var(--text-xs,12px);color:var(--fg-muted)}
.pg-detail b{color:var(--fg);font-weight:500}
`;
  document.head.appendChild(style);
}

// The same panel vocabulary todo 43 established for the timeline, so a
// degraded graph and a degraded timeline read identically. `data-kind` and
// `data-pg-state` carry the same value, as they do there.
function graphStatePanel(state, reason, remedy, detail) {
  return `<div class="pg-state" data-kind="${htmlEscape(state)}"
      data-pg-state="${htmlEscape(state)}" role="status">
    <strong>${htmlEscape(reason)}</strong>
    <p>${htmlEscape(remedy)}</p>
    ${detail ? `<p class="pg-detail">${htmlEscape(detail)}</p>` : ""}
  </div>`;
}

// Longest-path depth per stage, derived from the edges. Returns `null` for a
// cyclic graph: relaxation never stabilises there, so the pass cap is what
// turns a cycle into a named state instead of a hung tab.
function graphColumns(graph) {
  const nodes = graph.nodes || [];
  const edges = graph.edges || [];
  const depth = new Map();
  nodes.forEach((node) => {
    depth.set(node.stage_id, 0);
  });
  for (let pass = 0; pass <= nodes.length; pass++) {
    let moved = false;
    for (const edge of edges) {
      const from = depth.get(edge.from);
      const to = depth.get(edge.to);
      if (from === undefined || to === undefined) continue;
      if (to < from + 1) {
        depth.set(edge.to, from + 1);
        moved = true;
      }
    }
    if (!moved) return depth;
  }
  return null;
}

function graphTruncate(text, limit) {
  const value = String(text == null ? "" : text);
  return value.length > limit ? `${value.slice(0, limit)}…` : value;
}

// The node's control, as markup. A real <button>, so it is in the browser's
// tab order and the a11y tree without X6's help — which is the whole reason
// the HTML shape was chosen over a Canvas renderer. One function builds it
// for both the graph and the no-vendor fallback, so the two cannot diverge.
//
// Returned as a string rather than an element because X6's HTML shape writes
// the string into a `foreignObject`; handing it a live element produced an
// empty cell. Delegation is on the section, so nothing is lost by
// stringifying: there is no per-node listener to copy. It travels as the
// cell's `data.markup`, which is the only key the shape's registered content
// source reads.
function graphNodeHtml(node, tabStop) {
  const missing = (node.missing || []).join(", ");
  const caps = (node.capabilities || []).filter(Boolean);
  const runnable = node.runnable === true;
  // Both facts, stated, because the interesting case is when they disagree.
  const verdict = runnable
    ? `${caps.length} capabilities`
    : `no executor: ${missing}`;
  const spoken = runnable
    ? "This build has an executor for it."
    : `No executor in this build: ${graphTruncate(missing, 80) || "unknown"}`;
  const title = `${node.name} · ${node.kind} · ${node.state} · ${
    runnable ? "executor present" : `no executor: ${missing}`
  }`;
  return `<button type="button" class="pg-node"
    data-pg-node="${htmlEscape(String(node.stage_id))}"
    data-pg-stage-state="${htmlEscape(String(node.state))}"
    data-pg-runnable="${String(runnable)}"
    aria-pressed="false"
    tabindex="${tabStop === true ? "0" : "-1"}"
    aria-label="${htmlEscape(
      `${graphTruncate(node.name, 60)}. ${node.kind}. Recorded ${node.state}. ${spoken}`,
    )}"
    title="${htmlEscape(graphTruncate(title, 120))}">
    <span class="pg-node-name">${htmlEscape(graphTruncate(node.name, 40))}</span>
    <span class="pg-node-meta">${htmlEscape(graphTruncate(node.kind, 40))}</span>
    <span class="pg-node-meta">${htmlEscape(graphTruncate(String(node.state), 20))} ·
      ${htmlEscape(verdict)}</span>
  </button>`;
}

// The same nodes, as a plain list, for when the vendor did not load. The
// first node carries the tab stop here too, so the keyboard contract does not
// depend on which path drew the surface.
//
// `tabStop` is the roving tabindex and `aria-pressed` is the selection, and
// they are different things: a graph opens with one tab stop and nothing
// selected, and claiming otherwise would announce a selection the user never
// made.
function graphNodeList(graph) {
  const nodes = graph.nodes || [];
  return `<div class="pg-list" role="group"
      aria-label="${htmlEscape(`${graph.name}: ${nodes.length} stages`)}">${
    nodes.map((node, index) => graphNodeHtml(node, index === 0)).join("")
  }</div>`;
}
