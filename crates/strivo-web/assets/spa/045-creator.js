// ── Drawing the graph (todo 52) ───────────────────────────────────────
// The DOM shell, the X6 instance lifecycle and the live patch. Split from
// 044 (the vendor load and the node control) and 046 (the keyboard and the
// mount) for size; same module scope, since the bundle concatenates all three
// into one ES module.

let graphInstances = [];
// Node records keyed by graph key, so the delegated detail renderer needs
// neither a closure over the payload nor a copy of it in the DOM.
let graphRecords = new Map();

// Unmount every graph. Called before a repaint and on route change: an X6
// Graph holds a ResizeObserver, a document-level listener set and a reference
// to its container, so leaving one attached to a node the SPA has already
// replaced is how handlers end up bound to a detached DOM node.
function graphDispose() {
  graphInstances.forEach((graph) => {
    try {
      graph.dispose();
    } catch (_) {}
  });
  graphInstances = [];
}

function graphSectionHtml(graph, index) {
  const totals = graph.totals || {};
  const mismatches = (graph.mapping && graph.mapping.mismatches) || [];
  const readiness = `${totals.runnable || 0}/${totals.nodes || 0} stages have an executor`;
  return `<section class="cfg-card pg-section" data-pg-graph="${htmlEscape(graph.key)}"
      data-pg-graph-index="${index}" data-pg-verdict="as-loaded">
    <header class="pl-pipe-head">
      <h2 class="cfg-title">${htmlEscape(graph.name)}
        <span class="pg-cap-hint">${htmlEscape(graph.kind)} ·
        ${htmlEscape(graph.trigger || "manual")}</span>
      </h2>
      <span class="pl-pipe-readiness ${totals.not_runnable ? "partial" : "complete"}"
            data-pg-readiness="${htmlEscape(readiness)}"
            title="${htmlEscape(readiness)}">${htmlEscape(readiness)}</span>
      <button type="button" class="sm" data-pg-recheck>Recheck</button>
    </header>
    <p class="pg-detail" data-pg-verdict-note role="status"></p>
    ${mismatches.length
      ? graphStatePanel(
          GRAPH_STATES.MALFORMED,
          "This pipeline's graph does not match its stages, so it is not drawn.",
          "The stages are listed instead, because a drawn graph would assert a mapping the server has already reported as broken.",
          mismatches.join(" · "),
        )
      : ""}
    <div class="pg-surface">
      <!-- role=group, not a bare div: axe reports aria-prohibited-attr on a
           role-less element carrying aria-label, and an img role would be a
           lie because the canvas holds focusable controls. -->
      <div class="pg-canvas" data-pg-canvas="${htmlEscape(graph.key)}"
           role="group" aria-busy="true"
           aria-label="${htmlEscape(graph.name)} graph"></div>
    </div>
    <div class="pg-detail" data-pg-detail="${htmlEscape(graph.key)}" role="region"
         aria-live="polite"
         aria-label="${htmlEscape(graph.name)} stage detail"></div>
  </section>`;
}

// Draw one graph. `vendor` is the namespace read off globalThis; the caller
// has already established that it carries a `Graph` class.
function graphDraw(canvas, graph, vendor) {
  const nodes = graph.nodes || [];
  const edges = graph.edges || [];
  if (!nodes.length) {
    canvas.innerHTML = graphStatePanel(
      GRAPH_STATES.EMPTY,
      "This pipeline has no stages.",
      "An empty stage list is not the same as an unresolved one: nothing here is waiting on an executor.",
    );
    canvas.removeAttribute("aria-busy");
    return;
  }
  const depth = graphColumns(graph);
  if (!depth) {
    canvas.innerHTML = graphStatePanel(
      GRAPH_STATES.MALFORMED,
      "This pipeline's stages form a cycle.",
      "A cycle has no topological order to lay out. The executor rejects one at submission, so a payload that got past it is reported rather than drawn.",
    );
    canvas.removeAttribute("aria-busy");
    return;
  }

  const instance = new vendor.Graph({
    container: canvas,
    grid: 8,
    panning: false,
    mousewheel: false,
    interacting: false,
  });
  // No listeners on the cells: selection and the roving tabindex are driven
  // by the section's delegated handler, so disposing a graph cannot leave a
  // handler bound to a node it replaced.
  const byColumn = new Map();
  nodes.forEach((node, index) => {
    const column = depth.get(node.stage_id) || 0;
    const row = byColumn.get(column) || 0;
    byColumn.set(column, row + 1);
    instance.addNode({
      id: String(node.stage_id),
      x: column * (GRAPH_NODE_W + GRAPH_COL_GAP),
      y: row * (GRAPH_NODE_H + GRAPH_ROW_GAP),
      width: GRAPH_NODE_W,
      height: GRAPH_NODE_H,
      shape: "html",
      data: { markup: graphNodeHtml(node, index === 0) },
      // Read-only. Todo 53 owns authoring, and a graph you can drag but not
      // save is a worse lie than one that says it is a view.
      interactive: false,
    });
  });
  edges.forEach((edge) => {
    instance.addEdge({
      source: { cell: String(edge.from) },
      target: { cell: String(edge.to) },
    });
  });
  // Painted, not merely requested: two frames is when X6's render queue has
  // run, so a test synchronises on this attribute instead of sleeping.
  requestAnimationFrame(() => requestAnimationFrame(() => {
    canvas.dataset.pgPainted = "true";
  }));
  canvas.removeAttribute("aria-busy");
  graphInstances.push(instance);
}

function graphDetailHtml(node) {
  const dispatch = node.dispatch
    ? `${node.dispatch.plugin} · ${node.dispatch.verb}`
    : "none";
  const verdict = node.runnable === true
    ? "this build has an executor for it"
    : `not runnable — ${(node.missing || []).join(", ") || "no reason given"}`;
  return `<p><b>${htmlEscape(graphTruncate(node.name, 80))}</b> ·
      ${htmlEscape(String(node.kind))}</p>
    <p>Job table: <b>${htmlEscape(String(node.state))}</b> ·
      ${node.artifacts || 0} artifact${node.artifacts === 1 ? "" : "s"} ·
      attempt ${node.attempts || 0}/${node.max_attempts || 0}</p>
    <p>Executor registry: <b>${htmlEscape(verdict)}</b> ·
      dispatch ${htmlEscape(dispatch)}</p>
    <p>Locks: ${htmlEscape((node.requires || []).join(", ") || "none")} ·
      cost ${node.cost_cents_estimate || 0}¢</p>
    <p>Capabilities declared by this executor:
      ${htmlEscape((node.capabilities || []).join(", ") || "none")}</p>`;
}

function graphSelect(button) {
  const section = button.closest(".pg-section");
  section.querySelectorAll(".pg-node").forEach((el) => {
    el.tabIndex = el === button ? 0 : -1;
    el.setAttribute("aria-pressed", String(el === button));
  });
  const records = graphRecords.get(section.dataset.pgGraph) || {};
  section.querySelector("[data-pg-detail]").innerHTML = graphDetailHtml(
    records[button.dataset.pgNode] || { name: button.dataset.pgNode },
  );
}

// Patch a run's node states from a `PipelineUpdated` frame. Only the job
// table's half of each node moves: the frame carries `StageState`, and the
// executor verdict came from the registry at load time. Rather than leave a
// node claiming "runnable" on a verdict nothing has re-checked, the section
// is marked stale and says so.
function graphPatchRunState(run) {
  const section = document.querySelector(`[data-pg-graph="run:${run.id}"]`);
  if (!section) return;
  const states = new Map(
    (run.stages || []).map((stage) => [
      String(stage.id),
      String(
        typeof stage.state === "string"
          ? stage.state
          : Object.keys(stage.state || {})[0] || "Pending",
      ),
    ]),
  );
  section.querySelectorAll(".pg-node").forEach((el) => {
    const next = states.get(el.dataset.pgNode);
    if (next) el.dataset.pgStageState = next;
  });
  section.dataset.pgVerdict = "stale";
  const note = section.querySelector("[data-pg-verdict-note]");
  if (note) {
    note.textContent =
      "A stage transition arrived, so the recorded states above are live. The executor verdict is still the one loaded with the page; recheck to resolve it against the registry as it stands now.";
  }
}
