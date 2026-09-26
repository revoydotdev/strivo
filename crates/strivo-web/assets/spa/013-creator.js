// ── Pipelines (W5 — read PluginRpc dispatch state from daemon) ────────
// The run list, the run controls and the graph mount. The graph itself lives
// in 044–046-creator.js: its nodes are the executor's own `Stage`s and its
// edges are those stages' `inputs`, delivered on the `graph` key of this same
// response. The capability-tag blueprints `/pipelines/dag` still serves are
// documentation and are no longer rendered here.

// Bounded client-side run store, keyed by run id and newest-first. The
// `PipelineUpdated` SSE frame carries the whole `Pipeline` — the identical
// type `GET /pipelines/runs` returns — so a live stage transition can patch
// the cached run instead of re-downloading the run history. Capped so a
// long-lived tab cannot grow without bound; whatever the cap evicts comes
// back on the next coarse refetch, which the unknown-run path below always
// takes.
const RUN_STORE_MAX = 50;
const RUNS_VISIBLE = 20;
let runStore = [];
let showAllRuns = false;

// Returns true when the run was already held and is now patched in place,
// false when this client has never seen it — the caller then falls back to
// a coarse refetch, which is also how a newly-started run enters the store.
function storeRun(run) {
  const at = runStore.findIndex((r) => r.id === run.id);
  if (at === -1) return false;
  runStore[at] = run;
  return true;
}

async function renderPipelines(livePipeline) {
  // A live transition for a run we already hold: patch the store and repaint
  // the run list from it. No network, and — unlike a refetch behind
  // `API._fetch`'s 2 s GET cache — never a stale one. An unknown id (another
  // client's run, or one the cap evicted) falls through to the coarse load
  // below rather than being silently dropped.
  if (livePipeline && livePipeline.id && storeRun(livePipeline)) {
    paintRunList();
    // The frame carries the whole `Pipeline`, so the graph's node states
    // move with it. The executor verdict is not in the frame and is not
    // invented here — `graphPatchRunState` marks the section stale instead.
    graphPatchRunState(livePipeline);
    return;
  }
  let recs = { recordings: [] };
  let runPayload = { runs: [] };
  try {
    [recs, runPayload] = await Promise.all([
      API.recordings().catch(() => ({ recordings: [] })),
      API.pipelineRuns().catch(() => ({ runs: [] })),
    ]);
  } catch (_) {}
  root.removeAttribute("aria-busy");
  // Newest-first, capped: the oldest runs fall off the store, and the cap is
  // announced below rather than truncating silently.
  runStore = (runPayload.runs || []).slice().reverse().slice(0, RUN_STORE_MAX);
  showAllRuns = false;
  // Cache finished recordings so the Run-on-… picker can list them.
  const finishedRecs = (recs.recordings || [])
    .filter((r) => stateClassName(r.state) === "finished" && r.file_exists !== false)
    .sort((a, b) => new Date(b.started_at) - new Date(a.started_at));

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
      <h2 class="cfg-title">Runs <span class="pg-cap-hint" id="pl-runs-count">${runListCount()}</span></h2>
      <p class="pg-cap-hint" id="pl-run-status" role="status" aria-live="polite">${runStatusText()}</p>
      <div class="pg-list" id="pl-run-list">${runListHtml()}</div>
      ${runStore.length > RUNS_VISIBLE ? `<button class="sm" id="pl-runs-show-all">Show all ${runStore.length} runs</button>` : ""}
    </section>
    <h2 class="cfg-title">Pipeline graph <span class="pg-cap-hint">the executor's own stages and their inputs</span></h2>
    <div id="pg-graphs"></div>
    <div id="pa-editor-host"></div>
  `);
  setupChromeHandlers();
  // The graph mounts after the page is on screen, and the vendored library
  // it draws with loads after that. Neither blocks first paint.
  // Both mounts run after first paint and NEITHER is awaited before the
  // other: `renderPipelineGraphs` waits on the vendored X6 import, which has
  // an 8 s deadline, and awaiting it here held the editor — and its save
  // button — hostage to a library the editor does not use.
  renderPipelineGraphs(runPayload);
  renderPipelineAuthoring();

  document.getElementById("pl-run-ultimate")?.addEventListener("click", () => {
    openRecordingPickerForPipeline({ name: "Ultimate creator publish", id: "creator_publish" }, finishedRecs);
  });
  document.getElementById("pl-runs-show-all")?.addEventListener("click", () => {
    showAllRuns = true;
    paintRunList();
  });
  bindRunHandlers();
}

function runListCount() {
  return `${runStore.length} durable run${runStore.length === 1 ? "" : "s"}`;
}

function runListHtml() {
  const visible = showAllRuns ? runStore : runStore.slice(0, RUNS_VISIBLE);
  return (
    visible.map(runRowHtml).join("") ||
    '<div class="empty sm">No runs yet. Pick a finished recording to start the first workflow.</div>'
  );
}

// One sentence on the newest run, for the live region below. A pipeline's
// stage transitions patch the run list in place, so without this a
// screen-reader user is told nothing for the whole length of a long run —
// the announcement the research roadmap asks for.
function runStatusText() {
  const run = runStore[0];
  if (!run) return "";
  const stages = run.stages || [];
  const done = stages.filter((s) => s.state === "Done" || s.state === "Skipped").length;
  const state = typeof run.state === "string" ? run.state : Object.keys(run.state || {})[0] || "Pending";
  return `${run.name} — ${state}, ${done} of ${stages.length} stages done`;
}

// Repaint just the run list from the store. The graph and the
// recordings the "New run" picker needs are untouched, so a live transition
// costs no network and no full-page churn.
function paintRunList() {
  const list = document.getElementById("pl-run-list");
  if (!list) return;
  list.innerHTML = runListHtml();
  const count = document.getElementById("pl-runs-count");
  if (count) count.textContent = runListCount();
  const status = document.getElementById("pl-run-status");
  if (status) status.textContent = runStatusText();
  const more = document.getElementById("pl-runs-show-all");
  if (more) more.hidden = showAllRuns;
  bindRunHandlers();
}

function runRowHtml(run) {
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
}

// Rebound on every repaint: paintRunList() replaces the run rows wholesale,
// so handlers attached to the previous nodes would be gone.
function bindRunHandlers() {
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
