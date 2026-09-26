// ── Recordings table ─────────────────────────────────────────────────
// Table | Timeline segmented control shared by both renderRecordings()
// and renderRecordingsTimeline(). Timeline is the old #/history page,
// folded in here (see recView/renderRecordingsTimeline below) — the
// route now redirects (render(), 012-pvr.js) instead of rendering
// standalone.
function recordingsViewToggleHtml(active) {
  return `<div class="rec-view-toggle" role="group" aria-label="View">
    <button type="button" class="rec-view-btn ${active === "table" ? "is-active" : ""}" data-view="table" aria-pressed="${active === "table"}">Table</button>
    <button type="button" class="rec-view-btn ${active === "timeline" ? "is-active" : ""}" data-view="timeline" aria-pressed="${active === "timeline"}">Timeline</button>
  </div>`;
}
function wireRecordingsViewToggle() {
  document.querySelectorAll(".rec-view-toggle [data-view]").forEach((btn) => {
    btn.addEventListener("click", () => {
      location.hash = btn.dataset.view === "timeline" ? "#/recordings?view=timeline" : "#/recordings";
    });
  });
}

// The API's first page is a current snapshot, not ownership of the entire
// client library.  Keep rows reached through Load more and overlay the
// refreshed records by ID, so lifecycle events cannot collapse a 502-row
// browse back to its first 500 rows.
function reconcileRecordingSnapshot(records, nextCursor) {
  const incoming = new Map((records || []).map((r) => [String(r.id), r]));
  const seen = new Set();
  const merged = recCache.map((old) => {
    const id = String(old.id);
    seen.add(id);
    return incoming.get(id) || old;
  });
  // New active jobs usually arrive on the first page. Put them first; normal
  // table sorting decides their final visual position.
  const additions = (records || []).filter((record) => !seen.has(String(record.id)));
  recCache = [...additions, ...merged];
  dashRecordings = recCache;
  if (nextCursor !== undefined && !recHasLoadedPages) {
    recNextCursor = nextCursor ?? null;
  }
  seedVodDownloadStateFromRecCache();
}

async function renderRecordings(context = captureRouteContext()) {
  if (!isRouteCurrent(context)) return;
  // Allow sidebar links / external bookmarks to seed the search box
  // via #/recordings?channel=NAME (audit M2), and pick Table vs Timeline
  // via #/recordings?view=timeline (also what #/history now redirects to).
  const hash = window.location.hash || "";
  const qIdx = hash.indexOf("?");
  let view = "table";
  if (qIdx !== -1) {
    try {
      const params = new URLSearchParams(hash.slice(qIdx + 1));
      const ch = params.get("channel");
      if (ch != null) recFilter = ch;
      if (params.get("view") === "timeline") view = "timeline";
    } catch (_) {}
  }
  if (view === "timeline") {
    await renderRecordingsTimeline(context);
    return;
  }
  let recordings = [];
  try {
    const data = await API.recordings();
    if (!isRouteCurrent(context)) return;
    recordings = data.recordings || [];
    recNextCursor = data.next_cursor ?? null;
  } catch (e) {
    if (e.message.includes("unauthorized")) return;
    mountPage(`<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`, context);
    return;
  }
  if (!isRouteCurrent(context)) return;
  root.removeAttribute("aria-busy");
  if (recordings.length === 0) {
    if (!mountPage(`
      <h1 class="page-title">Recordings</h1>
      ${recordingsViewToggleHtml("table")}
      <div class="empty">
        <div class="glyph">📁</div>
        No recordings yet. Start one from the Library tab.
      </div>
    `, context)) return;
    wireRecordingsViewToggle();
    return;
  }
  reconcileRecordingSnapshot(recordings, recNextCursor);
  // W4-alt: sortable + filterable data grid. Column headers toggle sort;
  // the filter box narrows by channel/title live without refetching.
  if (!mountPage(`
    <h1 class="page-title">Recordings</h1>
    ${recordingsViewToggleHtml("table")}
    <div class="rec-toolbar">
      <input id="rec-filter" class="grid-filter" type="search"
             placeholder="Filter by channel or title… (/)"
             aria-label="Filter recordings" value="${htmlEscape(recFilter)}">
      <label class="rec-daterange" title="Filter recordings by started_at; inclusive.">
        from <input id="rec-from" class="rec-date" type="datetime-local" step="60" value="${htmlEscape(recDateFrom || "")}"/>
        to <input id="rec-to" class="rec-date" type="datetime-local" step="60" value="${htmlEscape(recDateTo || "")}"/>
        <button id="rec-clear-range" class="sm" type="button" title="Clear date range">✕</button>
      </label>
      <button id="rec-groupby" class="sm" title="Group rows by channel">
        ${recGroupBy === "channel" ? "▼ Grouped by channel" : "≣ Group by channel"}
      </button>
      <button id="rec-density" class="sm" title="Toggle row density">
        ${recDensity === "compact" ? "≡ Comfortable rows" : "═ Compact rows"}
      </button>
      ${(() => {
        const errored = recCache.filter((r) => stateClassName(r.state) === "failed" || stateLabel(r.state).toLowerCase().includes("interrupt")).length;
        return errored > 0
          ? `<button id="rec-clear-errored" class="danger sm" title="Trash all failed/interrupted recordings">✕ Clear errored (${errored})</button>`
          : "";
      })()}
    </div>
    <div id="rec-state-chips" class="rec-state-chips" role="group" aria-label="Filter by state"></div>
    <p class="page-subtitle" id="rec-count"></p>
    <div id="rec-massbar" class="massbar"></div>
    <table class="recordings-table ${recDensity === "compact" ? "compact" : ""}">
      <thead>
        <tr>
          <th class="rec-check"><input type="checkbox" id="rec-select-all" aria-label="Select all"></th>
          ${recHeader("state", "State")}
          ${recHeader("channel", "Channel")}
          ${recHeader("title", "Title")}
          ${recHeader("started", "Started")}
          ${recHeader("size", "Size")}
          <th></th>
        </tr>
      </thead>
      <tbody id="rec-body"></tbody>
    </table>
    ${recNextCursor != null ? `<button id="rec-load-more" class="button secondary" type="button">Load more recordings</button>` : ""}
  `, context)) return;
  wireRecordingsViewToggle();
  paintRecordings();

  document.getElementById("rec-filter")?.addEventListener("input", (e) => {
    recFilter = e.target.value;
    recWindowOffset = 0;
    paintRecordings();
  });
  document.getElementById("rec-from")?.addEventListener("change", (e) => {
    recDateFrom = e.target.value;
    recWindowOffset = 0;
    paintRecordings();
  });
  document.getElementById("rec-to")?.addEventListener("change", (e) => {
    recDateTo = e.target.value;
    recWindowOffset = 0;
    paintRecordings();
  });
  document.getElementById("rec-clear-range")?.addEventListener("click", () => {
    recDateFrom = ""; recDateTo = "";
    recWindowOffset = 0;
    const f = document.getElementById("rec-from"); const t = document.getElementById("rec-to");
    if (f) f.value = ""; if (t) t.value = "";
    paintRecordings();
  });
  document.getElementById("rec-density")?.addEventListener("click", () => {
    recDensity = recDensity === "compact" ? "comfortable" : "compact";
    localStorage.setItem("strivo-rec-density", recDensity);
    renderRecordings().catch((e) => Toast.error(e.message));
  });
  document.getElementById("rec-groupby")?.addEventListener("click", () => {
    recGroupBy = recGroupBy === "channel" ? "none" : "channel";
    localStorage.setItem("strivo-rec-groupby", recGroupBy);
    renderRecordings().catch((e) => Toast.error(e.message));
  });
  // Build state chips from the unique states currently in the cache, so
  // we don't paint chips for states that have zero rows. Each chip is a
  // toggle that AND-narrows the visible rows (empty filter = show all).
  paintRecStateChips();
  document.getElementById("rec-clear-errored")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const errored = recCache.filter((r) => {
      const c = stateClassName(r.state);
      const l = stateLabel(r.state).toLowerCase();
      return c === "failed" || l.includes("interrupt");
    });
    if (errored.length === 0) return;
    if (!(await confirmDialog(`Trash ${errored.length} errored recording(s)? Files move to the 7-day trash.`, { ok: "Clear", danger: true })))
      return;
    await withBusy(btn, "Clearing…", async () => {
      await API.clearErroredRecordings();
      Toast.success(`Cleared ${errored.length}`);
      // Optimistic prune; SSE refetch confirms.
      const erroredIds = new Set(errored.map((r) => r.id));
      recCache = recCache.filter((r) => !erroredIds.has(r.id));
      renderRecordings().catch(() => {});
    }).catch((err) => Toast.error(`Clear failed: ${err.message}`));
  });
  document.getElementById("rec-select-all")?.addEventListener("change", (e) => {
    const visible = visibleRecordingIds();
    if (e.target.checked) visible.forEach((id) => recSelected.add(id));
    else visible.forEach((id) => recSelected.delete(id));
    paintRecordings();
  });
  document.querySelectorAll("th[data-sort]").forEach((th) => {
    const sortBy = () => {
      const col = th.dataset.sort;
      if (recSort.col === col) {
        recSort.dir = recSort.dir === "asc" ? "desc" : "asc";
      } else {
        recSort = { col, dir: "asc" };
      }
      renderRecordings()
        .then(() => {
          // renderRecordings() replaces the whole <thead>, so the header the
          // user just activated no longer exists and focus fell to <body> —
          // a keyboard user could sort exactly once. Re-find it by the same
          // key and put focus back.
          document.querySelector(`th[data-sort="${col}"]`)?.focus();
        })
        .catch((e) => Toast.error(e.message));
    };
    th.addEventListener("click", sortBy);
    // R04 — Enter/Space activate sort for keyboard users (headers carry
    // tabindex="0" from recHeader(); no role override, so `aria-sort`
    // stays valid on the native columnheader).
    th.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        sortBy();
      }
    });
  });
  document.getElementById("rec-load-more")?.addEventListener("click", async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = "Loading…";
    try {
      const page = await API.recordings({ cursor: recNextCursor, limit: 500 });
      reconcileRecordingSnapshot(page.recordings || []);
      recNextCursor = page.next_cursor ?? null;
      recHasLoadedPages = true;
      seedVodDownloadStateFromRecCache();
      if (recNextCursor == null) button.remove();
      else {
        button.disabled = false;
        button.textContent = "Load more recordings";
      }
      paintRecStateChips();
      paintRecordings();
    } catch (error) {
      button.disabled = false;
      button.textContent = "Load more recordings";
      Toast.error(`Recordings load failed: ${error.message}`);
    }
  });
}

