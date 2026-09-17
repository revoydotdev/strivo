// ── Recordings → Timeline view (former #/history route) ────────────────
// Durable per-recording journal from the jobs DB, survives daemon
// restarts unlike the in-memory /recordings snapshot. Used to be its own
// #/history page with its own nav slot + a second copy of the Recordings
// table's actions; #/history now redirects to #/recordings?view=timeline
// (render(), above) and this is that view. The paint* functions
// (paintHistHeatmap/paintHistChips/paintHistory/historyPillHtml) stayed
// in 036-pvr.js — they're not route-specific, they just paint into
// whichever DOM currently has the matching #hist-* ids.
let histFilter = "";
let histGroupBy = localStorage.getItem("strivo-hist-groupby") || "none"; // "none" | "channel" | "date"
// Date heatmap click-day filter — "YYYY-MM-DD" or "" for unset.
let histDay = "";
let histStateFilter = new Set(
  (localStorage.getItem("strivo-hist-state-filter") || "")
    .split(",").filter(Boolean),
);
let histCache = [];
let histNextCursor = null;
let histTotal = 0;
let histRenderLimit = 200;

async function renderRecordingsTimeline(context = captureRouteContext()) {
  if (!isRouteCurrent(context)) return;
  // Fetch history alongside the live /recordings snapshot so we can
  // overlay file_exists state (audit B4). Without this, Timeline happily
  // reports 'Finished, 9 GB' for files the Recordings table knows are
  // long gone.
  let [hist, recs] = [[], []];
  try {
    const [h, r] = await Promise.all([
      API.history().catch(() => ({ history: [] })),
      API.recordings().catch(() => ({ recordings: [] })),
    ]);
    if (!isRouteCurrent(context)) return;
    hist = h.history || [];
    histNextCursor = h.next_cursor ?? null;
    histTotal = h.total ?? hist.length;
    recs = r.recordings || [];
  } catch (_) {}
  if (!isRouteCurrent(context)) return;
  const liveById = new Map(recs.map((r) => [r.id, r]));
  histCache = hist.map((row) => {
    const live = liveById.get(row.id);
    if (live && live.file_exists === false) {
      return { ...row, file_exists: false, state: "Failed" };
    }
    // Carry over SSE-only progress fields the /history snapshot never
    // has (it's a point-in-time REST fetch of the durable journal, while
    // RecordingProgress patches only the in-memory recCache array).
    // Without this a Timeline row for an in-progress job is stuck
    // showing whatever /history happened to return, however stale.
    if (live) {
      const merged = { ...row };
      for (const k of ["download_pct", "download_eta_secs", "download_rate_bps", "bytes_written", "duration_secs"]) {
        if (live[k] != null) merged[k] = live[k];
      }
      return merged;
    }
    return row;
  });
  root.removeAttribute("aria-busy");

  if (histCache.length === 0) {
    if (!mountPage(`
      <h1 class="page-title">Recordings</h1>
      ${recordingsViewToggleHtml("timeline")}
      <div class="empty">
        <div class="glyph">🗂</div>
        No recording history yet. Captures land here automatically.
      </div>
    `, context)) return;
    wireRecordingsViewToggle();
    return;
  }

  if (!mountPage(`
    <h1 class="page-title">Recordings</h1>
    ${recordingsViewToggleHtml("timeline")}
    <p class="page-subtitle" id="hist-count"></p>
    <div id="hist-heatmap"></div>
    <div class="rec-toolbar">
      <input id="hist-filter" class="grid-filter" type="search"
             placeholder="Filter by channel or title…"
             aria-label="Filter history" value="${htmlEscape(histFilter)}">
      <button id="hist-groupby" class="sm" title="Group rows">
        ${histGroupBy === "channel" ? "▼ Grouped by channel"
          : histGroupBy === "date" ? "▼ Grouped by month"
          : "≣ Group by…"}
      </button>
      ${histDay ? `<button id="hist-clear-day" class="sm" type="button" title="Clear day filter">✕ ${htmlEscape(histDay)}</button>` : ""}
    </div>
    <div id="hist-state-chips" class="rec-state-chips" role="group" aria-label="Filter by state"></div>
    <div id="hist-list" class="media-list"></div>
    ${histNextCursor != null ? `<button id="hist-load-more" class="button secondary" type="button">Load more history</button>` : ""}
  `, context)) return;
  wireRecordingsViewToggle();
  paintHistHeatmap();
  paintHistChips();
  paintHistory();
  document.getElementById("hist-clear-day")?.addEventListener("click", () => {
    histDay = "";
    renderRecordingsTimeline().catch((e) => Toast.error(e.message));
  });

  document.getElementById("hist-filter")?.addEventListener("input", (e) => {
    histFilter = e.target.value;
    paintHistory();
  });
  document.getElementById("hist-groupby")?.addEventListener("click", () => {
    histGroupBy = histGroupBy === "none"
      ? "channel"
      : histGroupBy === "channel" ? "date" : "none";
    localStorage.setItem("strivo-hist-groupby", histGroupBy);
    renderRecordingsTimeline().catch((e) => Toast.error(e.message));
  });
  document.getElementById("hist-load-more")?.addEventListener("click", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = "Loading…";
    try {
      const page = await API.history({ cursor: histNextCursor, limit: 200 });
      histCache.push(...(page.history || []));
      histNextCursor = page.next_cursor ?? null;
      histTotal = page.total ?? histCache.length;
      if (histNextCursor == null) button.remove();
      else {
        button.disabled = false;
        button.textContent = "Load more history";
      }
      paintHistHeatmap();
      paintHistChips();
      paintHistory();
    } catch (error) {
      button.disabled = false;
      button.textContent = "Load more history";
      Toast.error(`History load failed: ${error.message}`);
    }
  });
}

// Surgical DOM patch for a single Timeline (`.hist-pill`) row's progress
// state pill, mirroring patchRecordingRow's role for the Recordings
// table. Called from the RecordingProgress SSE handler (036-pvr.js) so a
// live download's percent updates without a full
// renderRecordingsTimeline re-fetch/re-paint.
function patchHistPillProgress(job) {
  if (!job || !job.id) return;
  const pill = document.querySelector(`.hist-pill[data-job-id="${CSS.escape(job.id)}"]`);
  if (!pill) return;
  const meta = pill.querySelector(".mp-meta");
  if (!meta) return;
  const existing = meta.querySelector(".state-pill");
  if (existing) existing.outerHTML = renderStatePill(recordingDisplayState(job));
}

function paintRecStateChips() {
  const host = document.getElementById("rec-state-chips");
  if (!host) return;
  const counts = new Map();
  for (const r of recCache) {
    const key = stateClassName(r.state);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  if (counts.size <= 1) {
    // Single state in cache → chips add no value; skip the row entirely.
    host.innerHTML = "";
    return;
  }
  const sorted = Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  const chips = sorted
    .map(([state, n]) => {
      const active = recStateFilter.size === 0 || recStateFilter.has(state);
      return `<button class="rec-state-chip state-${htmlEscape(state)} ${active ? "active" : ""}"
                data-state="${htmlEscape(state)}" type="button">
        <span class="rec-state-chip-dot"></span>
        ${htmlEscape(stateChipLabel(state))}
        <span class="rec-state-chip-count">${n}</span>
      </button>`;
    })
    .join("");
  const allActive = recStateFilter.size === 0;
  host.innerHTML = `
    <button class="rec-state-chip rec-state-chip-all ${allActive ? "active" : ""}"
            type="button" title="Show every state">
      <span class="rec-state-chip-dot"></span>All <span class="rec-state-chip-count">${recCache.length}</span>
    </button>
    ${chips}`;
  host.querySelector(".rec-state-chip-all")?.addEventListener("click", () => {
    recStateFilter.clear();
    localStorage.setItem("strivo-rec-state-filter", "");
    paintRecStateChips();
    paintRecordings();
  });
  host.querySelectorAll("[data-state]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const s = btn.dataset.state;
      // Click pattern: starting from "all visible", a click selects ONLY
      // that state. Subsequent clicks toggle additional states (AND-narrow
      // turns into OR-additive — matches gmail's chip behaviour).
      if (recStateFilter.size === 0) {
        recStateFilter = new Set([s]);
      } else if (recStateFilter.has(s)) {
        recStateFilter.delete(s);
      } else {
        recStateFilter.add(s);
      }
      localStorage.setItem(
        "strivo-rec-state-filter",
        Array.from(recStateFilter).join(","),
      );
      paintRecStateChips();
      paintRecordings();
    });
  });
}

// Human-friendly label for a state classname. Falls back to title-case.
function stateChipLabel(cls) {
  switch (cls) {
    case "finished": return "Finished";
    case "recording": return "Recording";
    case "downloading": return "Downloading";
    case "failed": return "Failed";
    case "file-error": return "File missing";
    case "scheduled": return "Scheduled";
    default: return cls.replace(/[-_]/g, " ").replace(/\b\w/g, c => c.toUpperCase());
  }
}

function recHeader(key, label) {
  // Active column shows the direction arrow; inactive sortable columns
  // get a faint ↕ so the affordance is discoverable (R6 audit fix).
  const arrow =
    recSort.col === key
      ? (recSort.dir === "asc" ? " ▲" : " ▼")
      : ' <span class="rec-th-sort-hint" aria-hidden="true">↕</span>';
  // R04 — headers are keyboard-operable: tabbable + role="button" so
  // Enter/Space (wired where the click handler is bound) can sort, not
  // just a mouse click.
  const ariaSort = recSort.col === key
    ? (recSort.dir === "asc" ? "ascending" : "descending")
    : "none";
  return `<th data-sort="${key}" class="rec-th-sortable micro" tabindex="0" role="button" aria-sort="${ariaSort}">${label}${arrow}</th>`;
}

// Close every open "⋯" row menu (recordingRow's .rec-row-menu-list). One
// document-level listener (Escape + click-outside) keeps this to O(1)
// bindings regardless of row count.
function closeAllRecRowMenus() {
  document.querySelectorAll(".rec-row-menu-list").forEach((list) => {
    list.hidden = true;
    list.previousElementSibling?.setAttribute("aria-expanded", "false");
  });
}
if (!document.body.dataset.recMenuBound) {
  document.body.dataset.recMenuBound = "1";
  document.addEventListener("click", closeAllRecRowMenus);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeAllRecRowMenus();
  });
}

// Normalize display/search/sort values once per record revision.  Progress
// updates only revise the numeric fields they change, so typing and sorting a
// long library do not repeatedly run title cleanup/regex work in comparators.
const recIndexCache = new WeakMap();
function recordingIndex(r) {
  const revision = [r.state, r.channel_name, r.stream_title, r.started_at, r.bytes_written].join("\u0001");
  const cached = recIndexCache.get(r);
  if (cached && cached.revision === revision) return cached;
  const index = {
    revision,
    state: stateLabel(r.state).toLowerCase(),
    channel: (r.channel_name || "").toLowerCase(),
    title: niceTitle(r.stream_title).toLowerCase(),
    started: recordingTime(r),
    size: r.bytes_written || 0,
  };
  recIndexCache.set(r, index);
  return index;
}

function patchRecordingRow(row, recording) {
  const before = row.dataset.recState;
  const display = recordingDisplayState(recording);
  // State transitions change available actions and may move a row between
  // groups, so use the full keyed reconciliation path for those rare events.
  if (before !== display.className) return false;
  if (row.dataset.recSignature !== recordingRowSignature(recording)) return false;
  row.classList.toggle("rec-sel", recSelected.has(recording.id));
  const stateCell = row.querySelector("td:nth-child(2)");
  const stateHtml = renderStatePill(display);
  if (stateCell.innerHTML !== stateHtml) stateCell.innerHTML = stateHtml;
  const size = row.querySelector("td:nth-child(6)");
  if (size) size.textContent = formatBytes(recording.bytes_written || 0);
  const checkbox = row.querySelector(".rec-row-check");
  if (checkbox) checkbox.checked = recSelected.has(recording.id);
  return true;
}

function updateRecPager(body, totalRows, rendered) {
  let pager = body.querySelector("[data-rec-pager]")?.closest("tr");
  if (totalRows <= recRenderLimit) {
    pager?.remove();
    return;
  }
  if (!pager) {
    body.insertAdjacentHTML("beforeend", '<tr data-rec-pager><td colspan="7" class="empty sm"></td></tr>');
    pager = body.lastElementChild;
  }
  const signature = `${recWindowOffset}:${recRenderLimit}:${totalRows}:${rendered}`;
  if (pager.dataset.pageSignature === signature) return;
  pager.dataset.pageSignature = signature;
  pager.firstElementChild.innerHTML = `
    <button id="rec-page-prev" type="button" ${recWindowOffset === 0 ? "disabled" : ""}>Previous ${recRenderLimit}</button>
    <button id="rec-page-next" type="button" ${recWindowOffset + recRenderLimit >= totalRows ? "disabled" : ""}>Next ${recRenderLimit}</button>
    <span class="pg-cap-hint"> ${recWindowOffset + 1}–${recWindowOffset + rendered} of ${totalRows}</span>`;
  pager.querySelector("#rec-page-prev")?.addEventListener("click", () => {
    recWindowOffset = Math.max(0, recWindowOffset - recRenderLimit);
    paintRecordings();
  });
  pager.querySelector("#rec-page-next")?.addEventListener("click", () => {
    recWindowOffset = Math.min(totalRows - 1, recWindowOffset + recRenderLimit);
    paintRecordings();
  });
}

// Apply the live filter + sort to recCache. Stable rows are patched in place
// so SSE progress preserves focused controls, open row menus, selection, and
// the scroll anchor.
function paintRecordings(dirtyIds = null) {
  const body = document.getElementById("rec-body");
  if (!body) return;
  const focusedElement = document.activeElement;
  const q = recFilter.trim().toLowerCase();
  let rows = recCache.filter((r) => {
    if (recStateFilter.size > 0 && !recStateFilter.has(stateClassName(r.state))) return false;
    // Started-at date-range filter. Empty bound = unbounded.
    if (recDateFrom || recDateTo) {
      const sa = (r.started_at || "").slice(0, 19); // YYYY-MM-DDTHH:MM:SS
      if (!sa) return false;
      if (recDateFrom && sa < recDateFrom) return false;
      if (recDateTo && sa > recDateTo) return false;
    }
    if (!q) return true;
    return (
      recordingIndex(r).channel.includes(q) || recordingIndex(r).title.includes(q)
    );
  });
  const dir = recSort.dir === "asc" ? 1 : -1;
  const key = (r) => recordingIndex(r)[recSort.col] ?? recordingIndex(r).started;
  rows.sort((a, b) => {
    const ka = key(a), kb = key(b);
    return ka < kb ? -dir : ka > kb ? dir : 0;
  });
  const totalRows = rows.length;
  if (recWindowOffset >= rows.length) recWindowOffset = Math.max(0, rows.length - recRenderLimit);
  const renderRows = rows.slice(recWindowOffset, recWindowOffset + recRenderLimit);
  recVisible = renderRows;
  const existing = Array.from(body.querySelectorAll("tr[data-rec-row]"));
  const stableRows = existing.length === renderRows.length &&
    existing.every((row, i) => row.dataset.recRow === String(renderRows[i].id));
  if (stableRows && existing.every((row, i) => !dirtyIds || dirtyIds.has(String(renderRows[i].id))
    ? patchRecordingRow(row, renderRows[i]) : true)) {
    const count = document.getElementById("rec-count");
    if (count) count.textContent = `${recCache.length} recordings · ${totalRows} match`;
    updateRecPager(body, totalRows, renderRows.length);
    const all = document.getElementById("rec-select-all");
    if (all) all.checked = renderRows.length > 0 && renderRows.every((r) => recSelected.has(r.id));
    updateMassbar();
    return;
  }
  if (recGroupBy === "channel") {
    // Cluster rows by channel_name while preserving the active sort order
    // within each cluster. Each cluster gets a heading row spanning every
    // column — sticky-styled via CSS — so the table reads like a grouped
    // ledger without needing a separate render pass per group.
    const order = [];
    const byChannel = new Map();
    for (const r of renderRows) {
      const k = r.channel_name || "(unknown)";
      if (!byChannel.has(k)) { byChannel.set(k, []); order.push(k); }
      byChannel.get(k).push(r);
    }
    const existingById = new Map(existing.map((row) => [row.dataset.recRow, row]));
    const wanted = new Set(renderRows.map((recording) => String(recording.id)));
    // Group headers are presentation-only. Retire those, then reconcile the
    // keyed data rows in the still-connected tbody.
    body.querySelectorAll("tr.rec-group-head, tr[data-rec-pager]").forEach((row) => row.remove());
    let cursor = body.firstElementChild;
    for (const ch of order) {
      const list = byChannel.get(ch);
      const totalBytes = list.reduce((a, b) => a + (b.bytes_written || 0), 0);
      const template = document.createElement("template");
      template.innerHTML = `<tr class="rec-group-head"><td colspan="7">
        <span class="rec-group-name">${htmlEscape(ch)}</span>
        <span class="rec-group-meta">${list.length} recording${list.length === 1 ? "" : "s"} · ${formatBytes(totalBytes)}</span>
      </td></tr>`;
      const header = template.content.firstElementChild;
      body.insertBefore(header, cursor);
      for (const recording of list) {
        let row = existingById.get(String(recording.id));
        if (!row || !patchRecordingRow(row, recording)) {
          const previous = row;
          const rowTemplate = document.createElement("template");
          rowTemplate.innerHTML = recordingRow(recording).trim();
          row = rowTemplate.content.firstElementChild;
          if (previous) {
            if (cursor === previous) cursor = row;
            previous.replaceWith(row);
          }
        }
        if (row !== cursor) body.insertBefore(row, cursor);
        cursor = row.nextElementSibling;
      }
    }
    for (const row of Array.from(body.querySelectorAll("tr[data-rec-row]"))) {
      if (!wanted.has(row.dataset.recRow)) row.remove();
    }
  } else {
    // Reconcile in the connected tbody. `replaceChildren(fragment)` would
    // briefly disconnect every focused row; insertBefore moves only rows
    // whose position changed and leaves an unrelated open menu in place.
    const existingById = new Map(existing.map((row) => [row.dataset.recRow, row]));
    const wanted = new Set(renderRows.map((recording) => String(recording.id)));
    let cursor = body.firstElementChild;
    for (const recording of renderRows) {
      let row = existingById.get(String(recording.id));
      if (!row || !patchRecordingRow(row, recording)) {
        const previous = row;
        const template = document.createElement("template");
        template.innerHTML = recordingRow(recording).trim();
        row = template.content.firstElementChild;
        if (previous) {
          if (cursor === previous) cursor = row;
          previous.replaceWith(row);
        }
      }
      if (row !== cursor) body.insertBefore(row, cursor);
      cursor = row.nextElementSibling;
    }
    for (const row of Array.from(body.querySelectorAll("tr[data-rec-row]"))) {
      if (!wanted.has(row.dataset.recRow)) row.remove();
    }
    // Footer/group rows are derived presentation and are rebuilt below.
    body.querySelectorAll("tr[data-rec-pager]").forEach((row) => row.remove());
  }
  updateRecPager(body, totalRows, renderRows.length);
  const count = document.getElementById("rec-count");
  if (count) {
    count.textContent = `${recCache.length} recordings · ${totalRows} match`;
  }
  body.querySelectorAll("[data-action=stop]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", async () => {
      if (!(await confirmDialog("Stop this recording?", { ok: "Stop", danger: true })))
        return;
      await withBusy(btn, "Stopping…", async () => {
        await API.stopRecording(btn.dataset.jobId);
        Toast.success("Recording stopped");
        setTimeout(() => render().catch(() => {}), 500);
      }).catch((e) => Toast.error(`Stop failed: ${e.message}`));
    });
  });
  body.querySelectorAll("[data-action=rec-play]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const id = btn.dataset.jobId;
      if (id) openRecordingPlayer(id);
    });
  });
  body.querySelectorAll("[data-action=rec-info]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openRecordingInfo(btn.dataset.jobId);
    });
  });
  body.querySelectorAll("[data-action=rec-rescan]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", (e) => { e.stopPropagation(); reScanRecording(btn); });
  });
  body.querySelectorAll("[data-action=rec-locate]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", (e) => { e.stopPropagation(); showRecordingPath(btn.dataset.path); });
  });
  body.querySelectorAll("[data-action=rec-delete]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      if (!(await confirmDialog("Delete this recording? The file moves to the 7-day trash.", { ok: "Delete", danger: true })))
        return;
      await withBusy(btn, "Deleting…", async () => {
        await API.deleteRecordingFile(btn.dataset.jobId);
        Toast.success("Deleted");
        // Optimistic: drop from local cache + repaint; the SSE refetch
        // confirms shortly.
        recCache = recCache.filter((r) => r.id !== btn.dataset.jobId);
        renderRecordings().catch(() => {});
      }).catch((err) => Toast.error(`Delete failed: ${err.message}`));
    });
  });
  body.querySelectorAll("[data-action=rec-rerecord]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      closeAllRecRowMenus();
      const r = recCache.find((row) => row.id === btn.dataset.jobId);
      if (!r) return;
      if (!(await confirmDialog(`Re-record '${r.channel_name}' now? This starts a fresh capture and may collide with any active recording on that channel.`, { ok: "Re-record", danger: true })))
        return;
      await withBusy(btn, "Queuing…", async () => {
        await API.startRecording({
          channel_id: r.channel_id,
          channel_name: r.channel_name,
          platform: r.platform,
          from_start: true,
        });
        Toast.success("Re-record queued");
        render().catch(() => {});
      }).catch((err) => Toast.error(`Re-record failed: ${err.message}`));
    });
  });
  body.querySelectorAll("[data-action=rec-remux]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      closeAllRecRowMenus();
      if (!(await confirmDialog("Remux this recording for browser playback? The original is kept as <name>.orig until success.", { ok: "Remux" })))
        return;
      await withBusy(btn, "Remuxing…", async () => {
        await API.remuxRecording(btn.dataset.jobId);
        Toast.success("Remuxed");
        render().catch(() => {});
      }).catch((err) => Toast.error(`Remux failed: ${err.message}`));
    });
  });
  // "⋯" row menu — one open at a time; a click anywhere else (or Escape)
  // closes it. Delegated on body rather than per-row so N rows cost one
  // listener, matching the rest of this table's wiring.
  body.querySelectorAll("[data-action=rec-menu-toggle]").forEach((btn) => {
    if (btn.dataset.recBound) return;
    btn.dataset.recBound = "1";
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const list = btn.nextElementSibling;
      const willOpen = list.hidden;
      closeAllRecRowMenus();
      if (willOpen) {
        list.hidden = false;
        btn.setAttribute("aria-expanded", "true");
      }
    });
  });
  // Row click:
  //   plain                           → open Info modal
  //   Shift+click                     → select range (anchor → here)
  //   Ctrl/Cmd+click                  → toggle just this row
  // Buttons/inputs/anchors still get their own handlers (early-return).
  // W4 keyboard nav: rows are tabbable; Enter plays, I opens info, Del
  // confirms delete. Delegated on body so we attach one handler total
  // regardless of N rows (audit P1 perf #4).
  if (!body.dataset.kbBound) {
    body.dataset.kbBound = "1";
    body.tabIndex = -1;
    body.addEventListener("keydown", (e) => {
      const tr = e.target.closest("tr[data-rec-row]");
      if (!tr) return;
      const id = tr.dataset.recRow;
      if (e.key === "Enter") {
        e.preventDefault();
        const playable = tr.querySelector('button[data-action="play-rec"]') ||
                         tr.querySelector('.rec-action-play');
        if (playable) playable.click();
        else if (id) openRecordingPlayer(id);
      } else if (e.key === "i" || e.key === "I") {
        e.preventDefault();
        const info = tr.querySelector('[data-action="info"], .rec-action-info');
        info?.click();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        const del = tr.querySelector('[data-action="delete"], .rec-action-del');
        del?.click();
      }
    });
  }
  body.querySelectorAll("tr[data-rec-row]").forEach((tr) => {
    if (tr.dataset.recBound) return;
    tr.dataset.recBound = "1";
    if (!tr.hasAttribute("tabindex")) tr.tabIndex = 0;
    tr.addEventListener("click", (e) => {
      if (e.target.closest("button, input, a")) return;
      const id = tr.dataset.recRow;
      if (e.shiftKey && recAnchorId) {
        e.preventDefault();
        const ids = visibleRecordingIds();
        const i = ids.indexOf(recAnchorId);
        const j = ids.indexOf(id);
        if (i >= 0 && j >= 0) {
          const [lo, hi] = i < j ? [i, j] : [j, i];
          for (let k = lo; k <= hi; k++) recSelected.add(ids[k]);
          paintRecordings();
        }
        return;
      }
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        if (recSelected.has(id)) recSelected.delete(id);
        else recSelected.add(id);
        recAnchorId = id;
        paintRecordings();
        return;
      }
      openRecordingInfo(id);
    });
  });
  // Selection model:
  //   Click checkbox            → toggle this row
  //   Shift+click checkbox/row  → select range from anchor to here
  //   Ctrl/Cmd+click row body   → toggle this row (without opening Info)
  //   Plain click row body      → open Info modal (handled below)
  body.querySelectorAll(".rec-row-check").forEach((cb) => {
    if (cb.dataset.recBound) return;
    cb.dataset.recBound = "1";
    // Let the native input commit first, then synchronize our selection on
    // change. This keeps programmatic `.check()` and keyboard Space truthful
    // while retaining the range-selection extension.
    cb.addEventListener("change", (e) => {
      const id = cb.dataset.jobId;
      if (e.shiftKey && recAnchorId) {
        const ids = visibleRecordingIds();
        const i = ids.indexOf(recAnchorId);
        const j = ids.indexOf(id);
        if (i >= 0 && j >= 0) {
          const [lo, hi] = i < j ? [i, j] : [j, i];
          for (let k = lo; k <= hi; k++) recSelected.add(ids[k]);
        }
      } else {
        if (cb.checked) recSelected.add(id);
        else recSelected.delete(id);
        recAnchorId = id;
      }
      paintRecordings();
    });
  });
  const all = document.getElementById("rec-select-all");
  if (all) {
    const vis = visibleRecordingIds();
    all.checked = vis.length > 0 && vis.every((id) => recSelected.has(id));
  }
  updateMassbar();
  if (focusedElement?.isConnected && document.activeElement !== focusedElement) {
    focusedElement.focus({ preventScroll: true });
  }
}

// IDs currently visible after filter/sort (for select-all + mass actions).
let recVisible = [];
let recRenderLimit = 200;
let recWindowOffset = 0;
// `null` cursor means either an initial page has no continuation or every
// page has been loaded. Keep that distinction so an SSE first-page refresh
// cannot reintroduce a Load more control after exhaustive browsing.
let recHasLoadedPages = false;
function visibleRecordingIds() {
  return recVisible.map((r) => r.id);
}

// Show/hide the multi-select mass-action bar (item 22). Acts on the selection
// intersected with currently-visible rows.
function updateMassbar() {
  const bar = document.getElementById("rec-massbar");
  if (!bar) return;
  const visible = new Set(visibleRecordingIds());
  const sel = recVisible.filter((r) => recSelected.has(r.id) && visible.has(r.id));
  if (sel.length === 0) {
    // Reversal of an earlier "audit fix": that version kept this bar
    // permanently visible (disabled buttons) so the bulk affordances were
    // discoverable before any selection. The persistent chrome itself is
    // now the problem the user picked to fix — the bar was one of four
    // always-on rows in the topbar/toolbar area competing for attention.
    // Hide it entirely until a row is actually ticked.
    bar.hidden = true;
    bar.classList.remove("massbar-empty");
    bar.innerHTML = "";
    delete bar.dataset.selectionSignature;
    return;
  }
  bar.classList.remove("massbar-empty");
  const active = sel.filter((r) => stateClassName(r.state) === "recording");
  bar.hidden = false;
  // Pre-compute which selected rows are finished + look browser-broken,
  // so the Remux button is only offered when it could actually help.
  const remuxable = sel.filter((r) => stateClassName(r.state) === "finished" && r.file_exists !== false);
  const deletable = sel.filter((r) => r.file_exists !== false || stateClassName(r.state) !== "recording");
  const selectionSignature = JSON.stringify(sel.map((r) => [r.id, recordingRowSignature(r)]));
  if (bar.dataset.selectionSignature === selectionSignature) return;
  bar.dataset.selectionSignature = selectionSignature;
  bar.innerHTML = `
    <span class="massbar-count">${sel.length} selected</span>
    ${active.length ? `<button id="mass-stop" class="danger sm">Stop ${active.length} active</button>` : ""}
    <button id="mass-rerecord" class="sm">Re-record ${sel.length}</button>
    ${remuxable.length ? `<button id="mass-remux" class="sm" title="Remux for browser playback (matroska + aac_adtstoasc)">Remux ${remuxable.length}</button>` : ""}
    ${deletable.length ? `<button id="mass-delete" class="danger sm">Delete ${deletable.length}</button>` : ""}
    <button id="mass-clear" class="sm">Clear</button>`;
  document.getElementById("mass-clear")?.addEventListener("click", () => {
    recSelected.clear();
    paintRecordings();
  });
  document.getElementById("mass-stop")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Stop ${active.length} active recording(s)?`, { ok: "Stop", danger: true })))
      return;
    let ok = 0;
    for (const r of active) {
      try {
        await API.stopRecording(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Stopped ${ok}/${active.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-rerecord")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Re-record ${sel.length} channel(s) now? This starts fresh captures and may collide with any active recording on those channels.`, { ok: "Re-record", danger: true })))
      return;
    let ok = 0;
    for (const r of sel) {
      try {
        await API.startRecording({
          channel_id: r.channel_id,
          channel_name: r.channel_name,
          platform: r.platform,
          from_start: true,
        });
        ok++;
      } catch (_) {}
    }
    Toast.success(`Re-record queued ${ok}/${sel.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-remux")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Remux ${remuxable.length} recording(s) for browser playback? Originals are kept as <name>.orig until success.`, { ok: "Remux" })))
      return;
    let ok = 0;
    for (const r of remuxable) {
      try {
        await API.remuxRecording(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Remuxed ${ok}/${remuxable.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
  document.getElementById("mass-delete")?.addEventListener("click", async () => {
    if (!(await confirmDialog(`Delete ${deletable.length} recording(s)? Files move to the 7-day trash.`, { ok: "Delete", danger: true })))
      return;
    let ok = 0;
    for (const r of deletable) {
      try {
        await API.deleteRecordingFile(r.id);
        ok++;
      } catch (_) {}
    }
    Toast.success(`Deleted ${ok}/${deletable.length}`);
    recSelected.clear();
    setTimeout(() => render().catch(() => {}), 500);
  });
}

// Cover thumbnail for a recording. The wrapper renders a channel-initials
// tile coloured by a hash of the channel name; the inner <img> sits on top
// and covers it when /thumb returns a real jpg. On 404 the img self-removes
// and the initials show through, so old recordings (made before the source-
// thumbnail snapshot landed, and missed by ffmpeg fallback on the server)
// still look intentional rather than broken.
function recThumb(r) {
  const initials = (r.channel_name || r.stream_title || "?")
    .trim()
    .replace(/[^\p{L}\p{N} ]/gu, "")
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join("") || "?";
  const hue = thumbHue(r.channel_name || r.id || "");
  // r.file_exists is set by the backend's augment_recording; when false the
  // recording's output_path is gone from disk (moved / deleted / external
  // drive offline) so we surface it as a red-caps overlay over the thumb.
  const missing = r.file_exists === false ? " rec-thumb-missing" : "";
  return `<span class="rec-thumb-wrap${missing}" data-init="${htmlEscape(initials)}"
    style="--ch-hue:${hue}deg">
    <img class="rec-thumb" loading="lazy" decoding="async" alt=""
      src="/api/v1/recordings/${encodeURIComponent(r.id)}/thumb"
      onerror="this.remove()" />
  </span>`;
}

function recordingRowSignature(r) {
  const display = recordingDisplayState(r);
  return [
    display.className, r.channel_name, r.stream_title, r.started_at,
    r.file_exists, r.output_path, r.source_url, r.channel_id, r.platform,
  ].map((value) => String(value ?? "")).join("\u0001");
}

// Stable hash → hue so the same channel always gets the same colour, but
// different channels get different ones across the rail.
function thumbHue(s) {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
  return Math.abs(h) % 360;
}

function recordingRow(r) {
  const disp = recordingDisplayState(r);
  const stateClass = disp.className;
  // Active includes both live captures (Recording) and VOD pulls (Downloading);
  // both are in-flight and offer Stop.
  const isActive = stateClass === "recording" || stateClass === "downloading";
  const isFinished = stateClass === "finished";
  // Action set per state. Play sits in slot 1 across every row; when the
  // recording isn't playable yet we render a disabled placeholder so the
  // button columns stay vertically aligned (in-flight downloads + failed
  // captures previously dropped slot 1 and the remaining buttons hopped
  // left).
  const playBtn = isFinished
    ? `<button class="primary sm" data-action="rec-play" data-job-id="${r.id}" title="Open player (Enter)">▶ Play</button>`
    : `<button class="primary sm rec-play-disabled" disabled aria-disabled="true" title="${isActive ? "Playable when capture finishes" : "Recording unavailable"}">▶ Play</button>`;
  // Delete/Re-record/Remux live in a "⋯" row menu instead of sitting as
  // inline buttons beside Play — only Stop (while active) and Info stay
  // directly visible. Each item is offered only when it would actually do
  // something for this row's state.
  const canRerecord = !isActive;
  const canRemux = stateClass === "finished" && r.file_exists !== false;
  const canDelete = r.file_exists !== false || stateClass !== "recording";
  const menuItems = [
    canRerecord ? `<button class="rec-row-menu-item" type="button" data-action="rec-rerecord" data-job-id="${r.id}">↺ Re-record</button>` : "",
    canRemux ? `<button class="rec-row-menu-item" type="button" data-action="rec-remux" data-job-id="${r.id}" title="Remux for browser playback">⇄ Remux</button>` : "",
    canDelete ? `<button class="rec-row-menu-item danger" type="button" data-action="rec-delete" data-job-id="${r.id}" title="Delete (Del)">✕ Delete</button>` : "",
  ].join("");
  const rowMenu = menuItems
    ? `<div class="rec-row-menu">
         <button class="sm rec-row-menu-toggle" type="button" data-action="rec-menu-toggle" aria-haspopup="true" aria-expanded="false" title="More actions">⋯</button>
         <div class="rec-row-menu-list" hidden role="menu">${menuItems}</div>
       </div>`
    : "";
  const infoBtn = `<button class="sm" data-action="rec-info" data-job-id="${r.id}" title="Recording details (I)">ⓘ Info</button>`;
  const tailBtns = isActive
    ? `<button class="danger sm" data-action="stop" data-job-id="${r.id}">Stop</button>${infoBtn}${rowMenu}`
    : `${infoBtn}${rowMenu}`;
  // File-error remediation: re-scan (re-check file_exists, in case the
  // user remounted a drive or restored from backup) + locate (show the
  // absolute path with a copy gesture). Distinct from Failed which is
  // a process error — file-error means the journal-vs-disk drifted.
  const fileErrorBtns = stateClass === "file-error"
    ? `<button class="sm" data-action="rec-rescan" data-job-id="${r.id}" title="Re-check whether the file exists">↻ Re-scan</button>
       <button class="sm" data-action="rec-locate" data-job-id="${r.id}" data-path="${htmlEscape(r.output_path || "")}" title="Show the expected file path">📂 Show path</button>`
    : "";
  const actions = `${playBtn}${fileErrorBtns}${tailBtns}`;
  return `
    <tr class="${recSelected.has(r.id) ? "rec-sel" : ""}" data-rec-row="${htmlEscape(r.id)}" data-rec-state="${htmlEscape(stateClass)}" data-rec-signature="${htmlEscape(recordingRowSignature(r))}">
      <td class="rec-check"><input type="checkbox" class="rec-row-check" data-job-id="${htmlEscape(r.id)}" ${recSelected.has(r.id) ? "checked" : ""} aria-label="Select recording"></td>
      <td>${renderStatePill(disp)}</td>
      <td>${htmlEscape(r.channel_name)}</td>
      <td><div class="rec-title-cell">${recThumb(r)}<span>${htmlEscape(niceTitle(r.stream_title) || "(no title)")}</span></div></td>
      <td>${new Date(r.started_at).toLocaleString()}</td>
      <td>${formatBytes(r.bytes_written || 0)}</td>
      <td class="rec-actions"><div class="rec-actions-inner">${actions}</div></td>
    </tr>
  `;
}

// VOD pulls and live captures both ride `RecordingState::Recording`, but
// "Recording" reads wrong for a yt-dlp-backed VOD pull. Distinguish by
// `source_url`: when set, label + colour as a download instead. Other
// states (Finished/Failed/etc) read the same regardless.
// File-error remediation: refetch /recordings so the backend re-runs
// augment_recording's file_exists probe on the current row. When the
// flag flips back to true (file restored / drive remounted) the next
// render shows it as plain Finished again.
async function reScanRecording(btn) {
  const id = btn.dataset.jobId;
  await withBusy(btn, "Scanning…", async () => {
    try {
      const r = await API.recordingOne(id);
      if (r && r.file_exists !== false) {
        Toast.success("File found — refreshing");
      } else {
        Toast.error("Still missing — file not present at the recorded path");
      }
      // Whichever way it went, repaint the current route so the badge updates.
      render().catch(() => {});
    } catch (err) {
      Toast.error(`Re-scan failed: ${err.message}`);
    }
  });
}

// Pop a tiny copy-friendly modal showing the recording's intended file
// path. Doesn't try to open a native file manager (the SPA can't reach
// the desktop) — instead lets the user copy the path with one click so
// they can paste it into their own shell / finder.
function showRecordingPath(path) {
  if (!path) {
    Toast.error("No path recorded for this row");
    return;
  }
  const overlay = ensureModalContainer("rec-locate-modal");
  overlay.innerHTML = `
    <div class="modal-card rec-locate-card">
      <header class="rec-locate-head">
        <h2>Recording file path</h2>
        <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
      </header>
      <p class="pg-cap-hint">The recording was written here. The SPA can't open your file manager directly — copy the path and open it yourself.</p>
      <div class="rec-locate-row">
        <code class="rec-locate-path">${htmlEscape(path)}</code>
        <button class="primary sm rec-locate-copy">Copy path</button>
      </div>
    </div>`;
  document.body.classList.add("modal-open");
  overlay.addEventListener("click", (e) => { if (e.target === overlay) closeRecLocate(); });
  overlay.querySelector("[data-action=modal-close]").addEventListener("click", closeRecLocate);
  overlay.querySelector(".rec-locate-copy").addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(path);
      Toast.success("Path copied to clipboard");
      closeRecLocate();
    } catch (err) {
      Toast.error(`Copy failed: ${err.message}`);
    }
  });
}
function closeRecLocate() {
  document.getElementById("rec-locate-modal")?.remove();
  document.body.classList.remove("modal-open");
}

function recordingDisplayState(j) {
  const cls = stateClassName(j.state);
  const lbl = stateLabel(j.state);
  // A row whose file is gone overrides every other state — the journal
  // says "Finished" but the recording has no file behind it, so reading
  // that as a green Finished pill misleads.
  if (j && j.file_exists === false) {
    return { label: "File Error", className: "file-error", pct: null };
  }
  if (j && j.source_url && cls === "recording") {
    return { label: "Downloading", className: "downloading", pct: pctForDownload(j) };
  }
  return { label: lbl, className: cls, pct: null };
}

// Project an in-flight VOD pull's percent complete. yt-dlp publishes an
// authoritative `download_pct` whenever the server-side total size is
// known; for HLS / segment-list captures it isn't, so we fall back to a
// projection from the cached VOD's known duration × detected bitrate.
//
// The blend (avg-so-far weighted 0.7, instantaneous 0.3) keeps a brief
// network stall from collapsing the bar mid-download.
function pctForDownload(j) {
  if (!j) return null;
  if (j.download_pct != null && Number.isFinite(j.download_pct)) {
    const raw = Math.max(0, Math.min(100, Number(j.download_pct)));
    // Cap short of 100 unless the job's own state says it's actually
    // done — mirrors the estimate-fallback branch below, which already
    // reserves 100% for the real Finish transition. Without this, a
    // backend-reported 100% mid-stream could paint a "Finished"-looking
    // bar for a job that is still in the Recording/Downloading state.
    const isActuallyFinished = typeof stateClassName === "function" && stateClassName(j.state) === "finished";
    return isActuallyFinished ? raw : Math.min(99, raw);
  }
  const bw = Number(j.bytes_written) || 0;
  const elapsed = Number(j.duration_secs) || 0;
  if (bw < 1 || elapsed < 1) return null;
  const expected = expectedDurationFromVodCache(j);
  if (!expected || expected <= 0) return null;
  const avgBps = bw / elapsed;
  const instBps = Number(j.download_rate_bps) || 0;
  const blendedBps = instBps > 0 ? avgBps * 0.7 + instBps * 0.3 : avgBps;
  const projectedTotal = blendedBps * expected;
  if (projectedTotal <= 0) return null;
  // Cap at 99 — only the Finish event grants 100.
  return Math.max(0, Math.min(99, (bw / projectedTotal) * 100));
}

// Cross-reference the channel-VOD cache to find the expected total
// duration for a recording's source URL. Returns seconds, or null when
// the SPA hasn't visited the source channel's detail page this session
// (no cache entry to consult).
function expectedDurationFromVodCache(j) {
  if (!j || !j.source_url) return null;
  if (typeof channelVods === "undefined" || !channelVods) return null;
  for (const list of Object.values(channelVods)) {
    if (!Array.isArray(list)) continue;
    for (const v of list) {
      if (v && v.url === j.source_url && v.duration_seconds) {
        return Number(v.duration_seconds);
      }
    }
  }
  return null;
}

// Render the state pill, with a progress fill + percentage label when
// `disp.pct` is known. The fill is driven by the `--state-fill` custom
// property so the CSS doesn't need a class per percentage bucket.
function renderStatePill(disp) {
  if (disp.pct == null) {
    return `<span class="state-pill micro ${disp.className}">${htmlEscape(disp.label)}</span>`;
  }
  const pct = disp.pct;
  const rounded = Math.round(pct);
  return `<span class="state-pill micro ${disp.className} has-fill" style="--state-fill:${pct.toFixed(1)}%">
    <span class="state-pill-fill" aria-hidden="true"></span>
    <span class="state-pill-label">${rounded}%</span>
  </span>`;
}

function stateLabel(s) {
  if (typeof s === "string") return s;
  if (s && typeof s === "object") return Object.keys(s)[0];
  return "?";
}
function stateClassName(s) {
  const label = stateLabel(s).toLowerCase();
  if (label.includes("record")) return "recording";
  if (label.includes("finish")) return "finished";
  if (label.includes("fail")) return "failed";
  return "";
}

