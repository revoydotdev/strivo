// Data viz / analytics route — research-grade aggregation +
// experiment runner over a corpus of transcribed recordings.
// User picks recordings to assemble a corpus, picks an experiment,
// SPA POSTs to /dataviz/run and renders the returned Series as a
// chart (bar / line / treemap). Pure SVG renderer — no library
// dependency.
const DATAVIZ_EXPERIMENTS = [
  { kind: "word_frequency", label: "Top words", body: { kind: "word_frequency", top_n: 30 } },
  { kind: "speaker_time", label: "Speaker minutes", body: { kind: "speaker_time" } },
  { kind: "speaker_episode_count", label: "Speaker appearances", body: { kind: "speaker_episode_count" } },
  { kind: "episodes_per_month", label: "Episodes per month", body: { kind: "episodes_per_month" } },
  { kind: "episode_durations", label: "Episode durations", body: { kind: "episode_durations" } },
  { kind: "speaker_cooccurrence", label: "Speaker co-occurrence", body: { kind: "speaker_cooccurrence" } },
];

let datavizState = {
  selectedIds: new Set(),
  series: null,
  experimentKind: "word_frequency",
};

async function renderDataviz() {
  const recs = (await API.recordings().catch(() => ({ recordings: [] }))).recordings || [];
  const finished = recs.filter((r) => r.state === "Finished");
  const expBtns = DATAVIZ_EXPERIMENTS.map((e) =>
    `<button class="sm dz-exp ${datavizState.experimentKind === e.kind ? "active" : ""}" data-kind="${e.kind}" type="button">${htmlEscape(e.label)}</button>`
  ).join("");
  const list = finished.map((r) => `
    <label class="dz-rec-row">
      <input type="checkbox" class="dz-rec-pick" data-id="${htmlEscape(r.id)}" ${datavizState.selectedIds.has(r.id) ? "checked" : ""}/>
      <span class="dz-rec-title">${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</span>
      <span class="dz-rec-meta pg-cap-hint">${htmlEscape((r.started_at || "").slice(0, 10))} · ${htmlEscape(r.platform || "")}</span>
    </label>`).join("");
  root.innerHTML = chrome(`
    <h1 class="page-title">📊 Data viz</h1>
    <p class="page-subtitle">Aggregate transcribed/diarised recordings, run experiments, render charts. Pick recordings → run experiment → swap chart type. All runs are local; no telemetry.</p>
    <div class="dz-grid">
      <section class="cfg-card dz-corpus">
        <h2 class="cfg-title">Corpus <span class="pg-cap-hint">${finished.length} finished recording${finished.length === 1 ? "" : "s"} eligible</span></h2>
        <div class="dz-corpus-actions">
          <button class="sm" id="dz-select-all" type="button">Select all</button>
          <button class="sm" id="dz-select-none" type="button">Clear</button>
          <span class="pg-cap-hint" id="dz-count">0 selected</span>
        </div>
        <div class="dz-rec-list">${list || '<div class="empty sm">No finished recordings to analyse yet.</div>'}</div>
      </section>
      <section class="cfg-card dz-exp-card">
        <h2 class="cfg-title">Experiments</h2>
        <div class="dz-exp-buttons">${expBtns}</div>
        <button class="btn-primary" id="dz-run" type="button">▶ Run experiment</button>
        <p class="pg-cap-hint">Crunchr transcripts feed the corpus — make sure each recording has been transcribed first (open it from /recordings → ⓘ Info → Generate subtitles).</p>
      </section>
      <section class="cfg-card dz-chart-card">
        <h2 class="cfg-title" id="dz-chart-title">Result</h2>
        <div id="dz-chart" class="dz-chart"></div>
      </section>
    </div>
  `);
  setupChromeHandlers();
  const updateCount = () => {
    document.getElementById("dz-count").textContent =
      `${datavizState.selectedIds.size} selected`;
  };
  updateCount();
  root.querySelectorAll(".dz-rec-pick").forEach((cb) => {
    cb.addEventListener("change", () => {
      if (cb.checked) datavizState.selectedIds.add(cb.dataset.id);
      else datavizState.selectedIds.delete(cb.dataset.id);
      updateCount();
    });
  });
  document.getElementById("dz-select-all").addEventListener("click", () => {
    root.querySelectorAll(".dz-rec-pick").forEach((cb) => { cb.checked = true; datavizState.selectedIds.add(cb.dataset.id); });
    updateCount();
  });
  document.getElementById("dz-select-none").addEventListener("click", () => {
    root.querySelectorAll(".dz-rec-pick").forEach((cb) => { cb.checked = false; });
    datavizState.selectedIds.clear();
    updateCount();
  });
  root.querySelectorAll(".dz-exp").forEach((btn) => {
    btn.addEventListener("click", () => {
      datavizState.experimentKind = btn.dataset.kind;
      root.querySelectorAll(".dz-exp").forEach((b) => b.classList.toggle("active", b === btn));
    });
  });
  document.getElementById("dz-run").addEventListener("click", async (ev) => {
    if (datavizState.selectedIds.size === 0) { Toast.error("Pick at least one recording first"); return; }
    const exp = DATAVIZ_EXPERIMENTS.find((e) => e.kind === datavizState.experimentKind);
    await withBusy(ev.currentTarget, "Fetching transcripts…", async () => {
      // Build the Corpus client-side from each recording's Crunchr
      // transcript. Three outcomes, and only one of them is "skip me":
      //   404 — this recording genuinely has no transcript;
      //   402 — the transcript store is a Pro capability and this machine is
      //         not entitled, which is a refusal, not a missing row;
      //   anything else — a real failure, surfaced as-is.
      // Collapsing all three (the old `.catch(() => null)`) made a
      // non-entitled user read "you have no transcripts" instead of
      // "activate Crunchr".
      const episodes = [];
      for (const id of datavizState.selectedIds) {
        const r = finished.find((x) => x.id === id);
        const name = niceTitle(r?.stream_title) || r?.channel_name || id.slice(0, 8);
        let tr;
        try {
          tr = await API.crunchrTranscript(id);
        } catch (e) {
          if (e && e.code === 402) throw e;
          if (e && /^HTTP 404:/.test(e.message || "")) continue;
          throw e;
        }
        const segments = (tr && tr.segments) || [];
        if (segments.length === 0) continue;
        episodes.push({
          id,
          title: name,
          date: r?.started_at || "",
          utterances: segments.map((u) => ({
            speaker: u.speaker || "Speaker",
            text: u.text || "",
            start_sec: u.start_sec || 0,
            end_sec: u.end_sec || (u.start_sec || 0) + 1,
          })),
        });
      }
      if (episodes.length === 0) {
        Toast.error("None of the selected recordings have Crunchr transcripts yet");
        return;
      }
      const corpus = { label: "selection", episodes };
      const resp = await API.datavizRun(corpus, exp.body);
      datavizState.series = resp.series;
      document.getElementById("dz-chart-title").textContent = resp.series.label;
      renderDatavizChart(resp.series, document.getElementById("dz-chart"));
      Toast.success(`Ran ${exp.label} over ${episodes.length} episode(s)`);
    }).catch((err) => Toast.error(err.message || "Run failed"));
  });
  // Re-render chart on resize so the SVG scales. Listener stored on
  // datavizState so a route change can tear it down — without this
  // each /dataviz visit added a permanent handler (P0 perf #1).
  if (datavizState.resizeHandler) {
    window.removeEventListener("resize", datavizState.resizeHandler);
  }
  datavizState.resizeHandler = () => {
    if (datavizState.series) renderDatavizChart(datavizState.series, document.getElementById("dz-chart"));
  };
  window.addEventListener("resize", datavizState.resizeHandler, { passive: true });
}

// Called from the router when leaving /plugins/dataviz so the resize
// handler doesn't accumulate across navigations.
function teardownDataviz() {
  if (datavizState && datavizState.resizeHandler) {
    window.removeEventListener("resize", datavizState.resizeHandler);
    datavizState.resizeHandler = null;
  }
}

// Pure SVG bar / line / treemap renderer. Takes a Series, emits SVG
// straight into the host element. No external dependency.
function renderDatavizChart(series, host) {
  if (!host || !series) return;
  const points = series.points || [];
  if (!points.length) { host.innerHTML = `<div class="empty sm">No data points</div>`; return; }
  const w = Math.max(400, host.clientWidth || 800);
  const h = 360;
  const max = Math.max(...points.map((p) => p.value));
  const accent = "var(--accent, #b07cff)";
  if (series.chart_hint === "treemap") {
    // Greedy slice-and-dice — single-row layout proportional to value.
    const total = points.reduce((a, p) => a + p.value, 0) || 1;
    let x = 0;
    const cells = points.map((p) => {
      const cw = (p.value / total) * w;
      const cell = `<g transform="translate(${x},0)">
        <rect width="${cw}" height="${h}" fill="${accent}" fill-opacity="${0.3 + 0.6 * (p.value / max)}"/>
        <text x="6" y="20" fill="#fff" font-size="12">${htmlEscape(p.label)}</text>
        <text x="6" y="36" fill="#fff" font-size="11" opacity="0.7">${p.value.toFixed(0)}</text>
      </g>`;
      x += cw;
      return cell;
    }).join("");
    host.innerHTML = `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${cells}</svg>`;
    return;
  }
  if (series.chart_hint === "line") {
    const stepX = w / Math.max(1, points.length - 1);
    const path = points.map((p, i) => `${i === 0 ? "M" : "L"}${(i * stepX).toFixed(1)},${(h - (p.value / max) * (h - 40) - 20).toFixed(1)}`).join(" ");
    const dots = points.map((p, i) =>
      `<circle cx="${(i * stepX).toFixed(1)}" cy="${(h - (p.value / max) * (h - 40) - 20).toFixed(1)}" r="3" fill="${accent}"><title>${htmlEscape(p.label)} · ${p.value.toFixed(1)}</title></circle>`
    ).join("");
    const axis = points.map((p, i) =>
      `<text x="${(i * stepX).toFixed(1)}" y="${h - 4}" fill="#fff" opacity="0.5" font-size="9" text-anchor="middle">${htmlEscape(p.label)}</text>`
    ).join("");
    host.innerHTML = `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
      <path d="${path}" stroke="${accent}" stroke-width="2" fill="none"/>
      ${dots}${axis}
    </svg>`;
    return;
  }
  // Default: horizontal bar chart.
  const rowH = Math.max(18, Math.min(36, Math.floor((h - 20) / points.length)));
  const ww = Math.max(400, w);
  const labelW = 180;
  const barMaxW = ww - labelW - 80;
  const svgH = points.length * rowH + 20;
  const rows = points.map((p, i) => {
    const bw = max > 0 ? (p.value / max) * barMaxW : 0;
    return `<g transform="translate(0,${i * rowH + 10})">
      <text x="0" y="${rowH * 0.65}" fill="#fff" font-size="12" opacity="0.85">${htmlEscape(p.label)}</text>
      <rect x="${labelW}" y="${rowH * 0.2}" width="${bw}" height="${rowH * 0.6}" fill="${accent}" rx="2"/>
      <text x="${labelW + bw + 6}" y="${rowH * 0.65}" fill="#fff" font-size="11" opacity="0.7">${p.value.toFixed(p.value < 10 ? 1 : 0)}</text>
    </g>`;
  }).join("");
  host.innerHTML = `<svg width="${ww}" height="${svgH}" viewBox="0 0 ${ww} ${svgH}">${rows}</svg>`;
}
