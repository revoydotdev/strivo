// ── Mounting the timeline ─────────────────────────────────────────────
// The route body, split from 042-creator.js for the same size reason. It is
// the only place that touches the DOM shell or the vendored libraries.

async function renderCreatorTimeline() {
  timelineStyles();
  timelineState.capability = timelinePreviewCapability(detectTimelineEnvironment());
  root.innerHTML = chrome(`
    <h1 class="page-title">Timeline</h1>
    <p class="page-subtitle">A view over the compiled plan. Every edit is sent as an intent and answered with the plan the renderer will use.</p>
    <div class="tl-wrap">
      <div class="tl-toolbar" role="toolbar" aria-label="Timeline">
        <button type="button" class="sm" id="tl-split">Split <span class="tl-kbd">S</span></button>
        <button type="button" class="sm" id="tl-ripple">Ripple delete <span class="tl-kbd">X</span></button>
        <button type="button" class="sm" id="tl-link">Link lanes <span class="tl-kbd">L</span></button>
        <button type="button" class="sm" id="tl-undo">Undo <span class="tl-kbd">U</span></button>
        <button type="button" class="sm" id="tl-snap">Snap playhead <span class="tl-kbd">G</span></button>
        <button type="button" class="sm" id="tl-export">Export contract <span class="tl-kbd">E</span></button>
        <span class="tl-notice" id="tl-mode" data-tl-mode="${timelineState.capability.mode}"></span>
      </div>
      <div id="tl-preview"></div>
      <div class="tl-track"><div class="tl-lane" id="tl-clips" role="listbox"
        aria-label="Timeline clips"></div>
        <div class="tl-playhead" id="tl-playhead"></div>
      </div>
      <div id="tl-notice-slot">${timelineNotice()}</div>
    </div>
  `);
  document.getElementById("tl-mode").textContent = `Preview: ${timelineState.capability.mode}`;

  try {
    await timelineReload();
  } catch (e) {
    document.getElementById("tl-clips").innerHTML = timelineDegradedPanel(
      "failed",
      "The timeline could not be read from the server.",
      e instanceof Error ? e.message : String(e)
    );
    return;
  }

  const lane = document.getElementById("tl-clips");
  timelineState.laneWidth = lane.clientWidth || 960;
  const preview = document.getElementById("tl-preview");
  const capability = timelineState.capability;
  if (!capability.picture || !capability.audibleAudio) {
    preview.innerHTML = timelineDegradedPanel("degraded", capability.reason, capability.remedy);
  }

  timelinePaint();
  lane.addEventListener("keydown", timelineOnKeydown);
  document.getElementById("tl-split").addEventListener("click", () => {
    const clip = timelineSelected();
    if (clip) timelineCommit({ op: "split", clip: timelineState.selected, at: timelineState.playheadUs });
  });
  document.getElementById("tl-ripple").addEventListener("click", () => {
    const plan = timelineState.plan;
    const frame = timelineFrameUs();
    const from = Math.min(timelineState.playheadUs, plan.output_duration);
    timelineCommit({ op: "ripple", range: { start: from, end: Math.min(from + frame * 10, plan.output_duration) } });
  });
  document.getElementById("tl-link").addEventListener("click", () => {
    const plan = timelineState.plan;
    const partner = (timelineState.selected + 1) % plan.clips.length;
    timelineCommit({ op: "link", clip: timelineState.selected, with: partner, linked: true });
  });
  document.getElementById("tl-snap").addEventListener("click", () => {
    timelineSnapPlayhead();
    timelinePaint();
  });
  document.getElementById("tl-undo").addEventListener("click", timelineUndo);
  document.getElementById("tl-export").addEventListener("click", () => {
    timelineExportNotice();
    timelinePaint();
  });

  // The vendored libraries load after the first paint, so a slow or failing
  // fetch never delays the timeline itself and never leaves a blank panel.
  const vendor = await loadTimelineVendor();
  if (vendor.error && preview) {
    preview.insertAdjacentHTML(
      "afterend",
      timelineDegradedPanel(
        "degraded",
        "The browser media libraries could not be loaded, so nothing is decoded here.",
        `${vendor.error} — the timeline, the waveform-from-peaks and the plan are unaffected; render to see the picture.`
      )
    );
  }
}
