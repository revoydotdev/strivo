// ── Pipelines (W5 — read PluginRpc dispatch state from daemon) ────────
// Plugins that have a dedicated SPA sub-route. Clicking a node routes
// there; everything else goes to the plugin hub so users land on the
// catalog entry.
const PIPELINE_NODE_ROUTES = new Set([
  "crunchr", "archiver", "viewguard", "insights",
  "schedule-optimizer",
]);

async function renderPipelines() {
  let payload = { pipelines: [] };
  let recs = { recordings: [] };
  let runPayload = { runs: [] };
  try {
    [payload, recs, runPayload] = await Promise.all([
      API.pipelinesDag(),
      API.recordings().catch(() => ({ recordings: [] })),
      API.pipelineRuns().catch(() => ({ runs: [] })),
    ]);
  } catch (_) {}
  root.removeAttribute("aria-busy");
  const pipelines = payload.pipelines || [];
  const runs = (runPayload.runs || []).slice().reverse();
  // Cache finished recordings so the Run-on-… picker can list them.
  const finishedRecs = (recs.recordings || [])
    .filter((r) => stateClassName(r.state) === "finished" && r.file_exists !== false)
    .sort((a, b) => new Date(b.started_at) - new Date(a.started_at));

  const flow = (pipe) => {
    // Layout the nodes left→right by the topological order the server
    // shipped. Edges are encoded as " → " arrows between consecutive
    // nodes that actually connect, with a tag chip.
    const order = pipe.order && pipe.order.length ? pipe.order : pipe.nodes.map((n) => n.id);
    const nodeById = new Map(pipe.nodes.map((n) => [n.id, n]));
    const edges = pipe.edges || [];
    const edgeBetween = (a, b) => edges.find((e) => e.from === a && e.to === b);
    const cells = [];
    for (let i = 0; i < order.length; i++) {
      const node = nodeById.get(order[i]);
      if (!node) continue;
      const runnable = node.runnable === true;
      const statusClass = runnable ? "is-avail" : "is-roadmap";
      // A blueprint node is labelled by what it actually is, and the
      // tooltip names the capabilities with no executor behind them.
      const missing = node.missing || [];
      const verdict = runnable
        ? "runnable"
        : `blueprint — no executor for ${missing.join(", ")}`;
      const produces = (node.produces || [])
        .map((c) => `<span class="pl-cap pl-cap-produces">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      const consumes = (node.consumes || [])
        .map((c) => `<span class="pl-cap pl-cap-consumes">${htmlEscape(c.replace(/_/g, " "))}</span>`)
        .join("");
      // Every node is a clickable anchor — routes to the plugin's own
      // sub-page when one exists, else to the plugin-hub catalog. The
      // hub upsell card (iter 26) handles the Pro-gate UX without us
      // having to know entitlement here.
      const href = PIPELINE_NODE_ROUTES.has(node.id)
        ? `#/plugins/${node.id}`
        : `#/plugins`;
      cells.push(`<a class="pl-node ${statusClass}" href="${htmlEscape(href)}"
          title="${htmlEscape(node.blurb)} · ${htmlEscape(verdict)}"
          data-plugin="${htmlEscape(node.id)}">
          <div class="pl-node-head">
            <span class="pl-node-label">${htmlEscape(node.label)}</span>
            <span class="pl-node-status">${htmlEscape(runnable ? "runnable" : "blueprint")}</span>
          </div>
          <div class="pl-node-caps">${consumes}${produces}</div>
        </a>`);
      const next = order[i + 1];
      if (next) {
        const eRec = edgeBetween(node.id, next);
        const viaLabel = eRec ? eRec.via.replace(/_/g, " ") : "";
        cells.push(`<div class="pl-arrow${eRec ? "" : " pl-arrow-loose"}" title="${htmlEscape(viaLabel)}">
          <span class="pl-arrow-line"></span>
          ${eRec ? `<span class="pl-arrow-via">${htmlEscape(viaLabel)}</span>` : ""}
          <span class="pl-arrow-tip">▸</span>
        </div>`);
      }
    }
    return cells.join("");
  };

  const cards = pipelines
    .map((p, idx) => {
      const totalNodes = (p.nodes || []).length;
      const runnableNodes = (p.nodes || []).filter((n) => n.runnable === true).length;
      const pct = totalNodes === 0 ? 0 : Math.round((runnableNodes / totalNodes) * 100);
      return `
    <section class="cfg-card pl-pipe-card">
      <header class="pl-pipe-head">
        <h2 class="cfg-title">${htmlEscape(p.name)} <span class="pg-cap-hint">${htmlEscape(p.description)}</span></h2>
        <div class="pl-pipe-actions">
          <span class="pl-pipe-readiness ${p.runnable ? "complete" : "partial"}"
                title="${runnableNodes} of ${totalNodes} nodes have an executor for every capability they produce">
            ${runnableNodes}/${totalNodes} runnable
          </span>
          <span class="pg-cap-hint">Blueprint</span>
        </div>
      </header>
      <div class="pl-pipe-bar"><span style="width:${pct}%"></span></div>
      <div class="pl-flow">${flow(p)}</div>
    </section>`;
    })
    .join("");

  root.innerHTML = chrome(`
    <h1 class="page-title">Pipelines</h1>
    <p class="page-subtitle">
      Durable, daemon-owned Creator workflows. Runs survive restarts, respect
      resource locks, retry with backoff, and stream every transition live.
    </p>
    <section class="cfg-card pl-pipe-card">
      <header class="pl-pipe-head">
        <div>
          <h2 class="cfg-title">Ultimate creator publish</h2>
          <p class="pg-cap-hint">Transcript intelligence + visual scene mining → captions, chapters, safety, highlights, clips, thumbnails → publish drafts + Casebook. Ten durable stages, real downloadable artifacts.</p>
        </div>
        <button class="btn-primary sm" id="pl-run-ultimate"
                ${finishedRecs.length === 0 ? "disabled title=\"No finished recordings available yet\"" : ""}>▶ New run</button>
      </header>
    </section>
    <section class="cfg-card">
      <h2 class="cfg-title">Runs <span class="pg-cap-hint">${runs.length} durable run${runs.length === 1 ? "" : "s"}</span></h2>
      <div class="pg-list">
        ${runs.slice(0, 20).map((run) => {
          const stages = run.stages || [];
          const done = stages.filter((stage) => stage.state === "Done" || stage.state === "Skipped").length;
          const state = typeof run.state === "string" ? run.state : Object.keys(run.state || {})[0] || "Pending";
          const failed = stages.find((stage) => stage.state && (stage.state.Exhausted || stage.state.Failed));
          const stageRows = stages.map((stage) => {
            const stageState = typeof stage.state === "string"
              ? stage.state
              : Object.keys(stage.state || {})[0] || "Pending";
            const artifacts = (stage.artifacts || []).map((artifact, artifactIndex) => {
              const path = String(artifact.path || "");
              const leaf = path.split(/[\\/]/).pop() || artifact.kind || "artifact";
              const href = `/api/v1/pipelines/runs/${encodeURIComponent(run.id)}/stages/${encodeURIComponent(stage.id)}/artifacts/${artifactIndex}`;
              return `<a class="pl-cap pl-cap-produces" href="${href}" download title="Download ${htmlEscape(path)}">${htmlEscape(artifact.kind || leaf)} · ${htmlEscape(leaf)}</a>`;
            }).join("");
            return `<div class="task-row">
              <div class="task-info">
                <span class="task-name">${htmlEscape(stage.name)}</span>
                <span class="task-cadence">${htmlEscape(stage.kind && stage.kind.Custom ? stage.kind.Custom : stageState)}</span>
                ${artifacts ? `<span class="pl-node-caps">${artifacts}</span>` : ""}
              </div>
              <span class="cfg-badge ${stageState === "Done" ? "ok" : stageState === "Exhausted" ? "bad" : ""}">${htmlEscape(stageState)}</span>
            </div>`;
          }).join("");
          return `<div class="pg-row pl-run-row" data-run-id="${htmlEscape(run.id)}">
            <div class="pg-row-main">
              <strong>${htmlEscape(run.name)}</strong>
              <span class="pg-cap-hint">${done}/${stages.length} stages · ${htmlEscape(run.trigger || "manual")}</span>
              ${failed ? `<span class="error">${htmlEscape((failed.state.Exhausted || failed.state.Failed).error || "stage failed")}</span>` : ""}
              <details class="pl-run-stages">
                <summary>${stages.length} stage${stages.length === 1 ? "" : "s"} · ${(stages.flatMap((stage) => stage.artifacts || [])).length} artifact${stages.flatMap((stage) => stage.artifacts || []).length === 1 ? "" : "s"}</summary>
                ${stageRows}
              </details>
            </div>
            <span class="cfg-badge ${state === "Done" ? "ok" : state === "Failed" ? "bad" : ""}">${htmlEscape(state)}</span>
            ${state === "Running" || state === "Pending" ? `<button class="sm pl-cancel" data-run="${htmlEscape(run.id)}">Cancel</button>` : ""}
            ${failed ? `<button class="sm pl-retry" data-stage="${htmlEscape(failed.id)}">Retry</button>` : ""}
          </div>`;
        }).join("") || '<div class="empty sm">No runs yet. Pick a finished recording to start the first workflow.</div>'}
      </div>
    </section>
    <h2 class="cfg-title">Workflow blueprints <span class="pg-cap-hint">trajectory and capability topology</span></h2>
    ${cards || '<div class="empty">No pipelines defined.</div>'}
  `);
  setupChromeHandlers();

  document.getElementById("pl-run-ultimate")?.addEventListener("click", () => {
    openRecordingPickerForPipeline({ name: "Ultimate creator publish", id: "creator_publish" }, finishedRecs);
  });
  document.querySelectorAll(".pl-cancel").forEach((button) => {
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await API.pipelineCancel(button.dataset.run);
        Toast.success("Pipeline cancellation requested");
        renderPipelines();
      } catch (error) {
        button.disabled = false;
        Toast.error(`Could not cancel pipeline: ${error.message}`);
      }
    });
  });
  document.querySelectorAll(".pl-retry").forEach((button) => {
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await API.pipelineRetryStage(button.dataset.stage);
        Toast.success("Stage re-queued");
        renderPipelines();
      } catch (error) {
        button.disabled = false;
        Toast.error(`Could not retry stage: ${error.message}`);
      }
    });
  });
}

function openRecordingPickerForPipeline(pipe, recs) {
  if (!recs.length) return;
  const overlay = ensureModalContainer("pl-run-picker");
  overlay.innerHTML = `
    <div class="modal-card pl-picker-card">
      <header class="pl-picker-head">
        <h2>Run "${htmlEscape(pipe.name)}" on a recording</h2>
        <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
      </header>
      <p class="pg-cap-hint">Pick a finished recording. The daemon queues the run immediately and keeps it durable across restarts.</p>
      <div class="pl-picker-list">
        ${recs.slice(0, 12).map((r) => `
          <button class="pl-picker-row" data-job-id="${htmlEscape(r.id)}" type="button">
            <span class="pl-picker-channel">${htmlEscape(r.channel_name || "(channel)")}</span>
            <span class="pl-picker-title">${htmlEscape(niceTitle(r.stream_title) || "(no title)")}</span>
            <span class="pl-picker-meta">${htmlEscape(new Date(r.started_at).toLocaleDateString())} · ${formatBytes(r.bytes_written || 0)}</span>
          </button>`).join("")}
      </div>
    </div>`;
  bumpModalOpen(+1);
  let closed = false;
  const escHandler = (e) => { if (e.key === "Escape") { e.preventDefault(); close(); } };
  const close = () => {
    if (closed) return;
    closed = true;
    overlay.remove();
    bumpModalOpen(-1);
    document.removeEventListener("keydown", escHandler, true);
  };
  // B2: capture-phase ESC so a nested modal opening inside this picker
  // can't eat the keystroke before we get to dismiss.
  document.addEventListener("keydown", escHandler, true);
  overlay.addEventListener("click", (e) => { if (e.target === overlay) close(); });
  overlay.querySelector("[data-action=modal-close]").addEventListener("click", close);
  overlay.querySelectorAll(".pl-picker-row").forEach((row) => {
    row.addEventListener("click", async () => {
      const id = row.dataset.jobId;
      close();
      try {
        await API.pipelineRun(id, pipe.id || "creator_publish");
        Toast.success(`Queued ${pipe.name}`);
        renderPipelines();
      } catch (error) {
        Toast.error(`Pipeline failed to queue: ${error.message}`);
      }
    });
  });
}
