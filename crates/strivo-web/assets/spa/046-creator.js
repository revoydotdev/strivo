// ── Mounting the graph (todo 52) ─────────────────────────────────────
// Keyboard operation and the mount. One delegated handler serves both the
// X6 HTML cells and the no-vendor fallback list, because the buttons are
// built by the same function either way.

function graphNodesIn(section) {
  return Array.from(section.querySelectorAll(".pg-node"));
}

// One tab stop per graph, arrows within it. Tabbing through ten stages is
// tedious rather than operable, and a roving tabindex is the pattern the
// a11y lane's target-size check can measure.
function graphBindKeyboard(host) {
  host.addEventListener("focusin", (event) => {
    if (!event.target.classList.contains("pg-node")) return;
    const section = event.target.closest(".pg-section");
    if (section) graphNodesIn(section).forEach((el) => {
      el.tabIndex = el === event.target ? 0 : -1;
    });
  });
  host.addEventListener("click", (event) => {
    if (event.target.classList.contains("pg-node")) graphSelect(event.target);
  });
  host.addEventListener("keydown", (event) => {
    if (!event.target.classList.contains("pg-node")) return;
    const section = event.target.closest(".pg-section");
    const buttons = section ? graphNodesIn(section) : [];
    const at = buttons.indexOf(event.target);
    if (at < 0 || !buttons.length) return;
    let next = at;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") {
      next = (at + 1) % buttons.length;
    } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
      next = (at - 1 + buttons.length) % buttons.length;
    } else if (event.key === "Home") {
      next = 0;
    } else if (event.key === "End") {
      next = buttons.length - 1;
    } else if (event.key === "Escape") {
      // Todo 16 found a dialog that did not close on Escape. The selection is
      // the thing that has to give, and it gives with no residue left.
      event.preventDefault();
      buttons.forEach((el) => {
        el.tabIndex = -1;
        el.setAttribute("aria-pressed", "false");
      });
      const detail = section.querySelector("[data-pg-detail]");
      if (detail) detail.textContent = "";
      return;
    } else {
      return;
    }
    event.preventDefault();
    buttons[next].focus();
    graphSelect(buttons[next]);
  });
  // Recheck resolves the executor verdict against the registry as it stands
  // now, which is the only way a stale "runnable" can stop being claimed.
  host.addEventListener("click", (event) => {
    if (event.target.closest("[data-pg-recheck]")) renderPipelines();
  });
}

async function renderPipelineGraphs(payload) {
  graphStyles();
  graphDispose();
  const host = document.getElementById("pg-graphs");
  if (!host) return;
  graphRecords = new Map();

  const graphs = payload && payload.graph ? payload.graph.graphs : null;
  if (!graphs) {
    host.innerHTML = graphStatePanel(
      GRAPH_STATES.FAILED,
      "The server returned no pipeline graph.",
      "This build of the API does not send one, so nothing is drawn rather than something invented here.",
    );
    return;
  }
  if (!graphs.length) {
    host.innerHTML = graphStatePanel(
      GRAPH_STATES.EMPTY,
      "No pipelines to show.",
      "Nothing has been submitted, and this build ships no pipeline template.",
    );
    return;
  }
  host.innerHTML = graphs.map(graphSectionHtml).join("");
  graphs.forEach((graph) => {
    const records = {};
    (graph.nodes || []).forEach((node) => {
      records[String(node.stage_id)] = node;
    });
    graphRecords.set(String(graph.key), records);
  });
  graphBindKeyboard(host);

  // Loaded after the first paint, so a slow or failing vendor fetch never
  // delays the page and never leaves a blank panel.
  const vendor = await loadGraphVendor();
  const canvases = Array.from(host.querySelectorAll("[data-pg-canvas]"));
  if (vendor.state) {
    canvases.forEach((canvas) => {
      const graph = graphs.find((g) => String(g.key) === canvas.dataset.pgCanvas);
      canvas.innerHTML = graphStatePanel(
        vendor.state,
        "The node graph library could not be loaded, so nothing is drawn.",
        `${vendor.error} — every stage is still listed below, carrying the same recorded state and the same executor verdict.`,
      );
      if (graph) canvas.insertAdjacentHTML("beforeend", graphNodeList(graph));
      canvas.removeAttribute("aria-busy");
    });
    return;
  }
  canvases.forEach((canvas) => {
    const graph = graphs.find((g) => String(g.key) === canvas.dataset.pgCanvas);
    if (graph) graphDraw(canvas, graph, vendor.X6);
  });
}
