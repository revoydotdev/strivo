// ── Add-Channel two-phase wizard (item 19) ───────────────────────────
// Phase 1: pick platform + type a name → resolve (live, via SSE).
// Phase 2: show the resolved channel → confirm → enable auto-record.
// Config is deferred until the entity is confirmed.
let addWizard = null; // { platform, query } while a resolve is in flight

const MODAL_FOCUSABLE =
  "a[href], button, input, select, textarea, [tabindex]:not([tabindex='-1'])";

function modalFocusables(modal) {
  return [...modal.querySelectorAll(MODAL_FOCUSABLE)].filter(
    (el) => !el.disabled && el.getClientRects().length > 0 && !el.closest("[hidden]"),
  );
}

// The dialog contract the confirm modal in 008-pvr.js already has (remember
// the opener, move focus in, trap Tab, hand focus back on close). Without it
// this wizard left focus on the trigger behind the overlay: Tab walked the
// page underneath, and closing dropped focus on <body>.
let _appModalPrevFocus = null;
function openAppModal(modal) {
  if (!_appModalPrevFocus) _appModalPrevFocus = document.activeElement;
  modal.classList.add("open");
  document.body.classList.add("modal-open");
  const first = modalFocusables(modal)[0];
  if (first) first.focus();
}

function openAddChannelWizard() {
  let modal = document.getElementById("add-channel-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "add-channel-modal";
    modal.className = "app-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "Add channel");
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => {
      if (e.target === modal) closeAppModal(modal);
    });
    modal.addEventListener("keydown", (e) => {
      if (e.key !== "Tab") return;
      const stops = modalFocusables(modal);
      if (stops.length < 2) return;
      const first = stops[0];
      const last = stops[stops.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    });
  }
  paintAddWizardSearch(modal);
  openAppModal(modal);
}

// One owner for the click-outside / ESC / route-change dismissal of all
// .app-modal dialogs. Built so the keyboard-help (kbd-help) overlay
// stays separate — it has its own toggle and shouldn't be auto-closed
// on navigation.
function closeAppModal(modal) {
  if (!modal) return;
  modal.classList.remove("open");
  const back = _appModalPrevFocus;
  _appModalPrevFocus = null;
  if (back && back.isConnected && back.focus) back.focus();
  // B3: decrement the modal-open counter, which auto-clears the body
  // class only when no other open modal remains. The DOM-query check
  // is kept as a defensive belt for callers that bypass bumpModalOpen.
  bumpModalOpen(-1);
  if (_modalOpenCount === 0 && !document.querySelector(".app-modal.open")) {
    document.body.classList.remove("modal-open");
  }
}
function closeAllAppModals() {
  document.querySelectorAll(".app-modal.open").forEach(closeAppModal);
}
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeAllAppModals();
});
window.addEventListener("hashchange", closeAllAppModals);

function paintAddWizardSearch(modal, opts = {}) {
  modal = modal || document.getElementById("add-channel-modal");
  if (!modal) return;
  const plat = opts.platform || "Twitch";
  const sel = (p) => (p === plat ? " selected" : "");
  modal.innerHTML = `
    <div class="card">
      <h2>Add channel</h2>
      <p class="wizard-step">Step 1 of 2 — find the channel</p>
      <div class="wizard-row">
        <select id="aw-platform">
          <option value="Twitch"${sel("Twitch")}>Twitch</option>
          <option value="YouTube"${sel("YouTube")}>YouTube</option>
          <option value="Patreon"${sel("Patreon")}>Patreon</option>
        </select>
        <input id="aw-query" type="text" placeholder="Twitch login, or YouTube/Patreon id"
               value="${htmlEscape(opts.query || "")}" autofocus />
        <button id="aw-search" class="primary">Search</button>
      </div>
      <div id="aw-result" class="wizard-result">${htmlEscape(opts.message || "")}</div>
    </div>`;
  const doSearch = async () => {
    const platform = modal.querySelector("#aw-platform").value;
    const query = modal.querySelector("#aw-query").value.trim();
    if (!query) return;
    addWizard = { platform, query };
    modal.querySelector("#aw-result").innerHTML = '<div class="empty sm">Searching…</div>';
    try {
      await API.resolveChannel(platform, query);
    } catch (e) {
      modal.querySelector("#aw-result").innerHTML = `<div class="empty sm">Search failed: ${htmlEscape(e.message)}</div>`;
    }
  };
  modal.querySelector("#aw-search")?.addEventListener("click", doSearch);
  modal.querySelector("#aw-query")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") doSearch();
  });
}

// Phase 2: render the resolved entity for confirmation (called from the
// ChannelResolved SSE handler).
function paintAddWizardConfirm(ev) {
  const modal = document.getElementById("add-channel-modal");
  if (!modal || !modal.classList.contains("open") || !addWizard) return;
  if (ev.platform !== addWizard.platform || ev.query !== addWizard.query) return;
  const result = modal.querySelector("#aw-result");
  if (!result) return;
  if (ev.error || !ev.channel_id) {
    result.innerHTML = `<div class="empty sm">Not found: ${htmlEscape(ev.error || "no match")}</div>`;
    return;
  }
  const name = ev.display_name || ev.channel_id;
  // Three genuinely different promises, so three distinct actions rather
  // than one button that silently follows the channel: a one-off back-catalogue
  // pull, a permanent auto-record subscription, and a plain dismissal.
  result.innerHTML = `
    <div class="wizard-confirm">
      <p class="wizard-step">Step 2 of 2 — confirm</p>
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(name)}</span>
          <span class="task-cadence">${htmlEscape(ev.platform)} · ${htmlEscape(ev.channel_id)}</span>
        </div>
      </div>
      <div class="wizard-actions">
        <button type="button" id="aw-pull">Pull past media</button>
        <button type="button" id="aw-confirm" class="primary" data-key="${htmlEscape(ev.platform)}:${htmlEscape(ev.channel_id)}">
          Add &amp; enable auto-record
        </button>
        <button type="button" id="aw-cancel">Close</button>
      </div>
      <p class="pg-cap-hint">Pull past media downloads what they already streamed and leaves them out of your config. Add &amp; enable auto-record follows them and records every future stream.</p>
    </div>`;
  result.querySelector("#aw-pull")?.addEventListener("click", () => {
    openPullMedia({ platform: ev.platform, channelId: ev.channel_id, name });
  });
  result.querySelector("#aw-cancel")?.addEventListener("click", () => {
    addWizard = null;
    closeAppModal(modal);
  });
  result.querySelector("#aw-confirm")?.addEventListener("click", async (e) => {
    const key = e.currentTarget.dataset.key;
    try {
      await API.toggleAutoRecord(key, true);
      Toast.success(`Added ${name} — auto-record on`);
      modal.classList.remove("open");
      addWizard = null;
      if (currentRoute() === "library") render();
    } catch (err) {
      Toast.error(`Add failed: ${err.message}`);
    }
  });
}

// ── "Pull past media" — a one-off back-catalogue pull that never follows ──
//
// The wizard's other exit writes auto_record_channels and hands the channel
// to the live monitor for good. This one doesn't: it asks the daemon for the
// channel's past broadcasts over the SAME membership-agnostic routes the
// channel-detail pane already uses (POST /channels/{id}/vods → the
// `channel-vods` SSE event → the shared `channelVods` global) and queues the
// ticked rows through POST /vods/download. Nothing about the channel is
// persisted, so looking someone up and grabbing their back catalogue no longer
// costs a permanent subscription to their future streams.
//
// Step 2 stays a one-decision screen. The VOD list is unbounded, so it lives
// in its own `.app-modal` built on the playlist-viewer pattern
// (showPlaylistModal / openPlaylistViewer) rather than inside the wizard's
// 480px card, and every row reuses the exact pill structure `vodSectionHtml`
// emits: a label wrapping thumb + info, with the download button as a SIBLING
// so no interactive element is nested inside the selectable row.
let pullMedia = null; // { platform, channelId, name, selected: Set<url> }
// The wizard's own prev-focus. openAppModal binds _appModalPrevFocus to the
// FIRST modal's opener, so closing the stacked pull modal would strand the
// wizard with a null binding; re-arming it here keeps the wizard's close
// landing on the "+ Add channel" button that opened it.
let _pullPrevFocus = null;

function pullMediaModal() {
  let modal = document.getElementById("pull-media-modal");
  if (modal) return modal;
  modal = document.createElement("div");
  modal.id = "pull-media-modal";
  modal.className = "app-modal";
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  document.body.appendChild(modal);
  modal.addEventListener("click", (e) => {
    if (e.target === modal) closePullMedia();
  });
  modal.addEventListener("keydown", (e) => {
    if (e.key !== "Tab") return;
    const stops = modalFocusables(modal);
    if (stops.length < 2) return;
    const first = stops[0];
    const last = stops[stops.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  });
  return modal;
}

// Newest first — the platform's own ordering isn't guaranteed, and this is a
// "recent past broadcasts" picker.
function pullMediaVods() {
  return (channelVods[pullMedia.channelId] || [])
    .filter((v) => v.url)
    .slice()
    .sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""));
}

// Read the app-wide derived state first — exactly what the channel-detail
// pills read, and what repaintOpenChannelDownloads() re-derives on every
// lifecycle event — then fall back to recCache for the window between our
// POST landing and that asynchronous reseed. A job that failed or vanished
// reads as "idle" again, so the row is re-tickable exactly like the
// channel-detail pill it mirrors.
function pullMediaStatus(url) {
  if (vodDownloadState[url] === "downloaded") return "downloaded";
  if (vodDownloadState[url] === "downloading") return "downloading";
  const job = activeVodJob(url);
  if (!job) return "idle";
  if (job.state === "Finished" && job.file_exists !== false) return "downloaded";
  return isInProgress(job.state) ? "downloading" : "idle";
}

function pullMediaRowHtml(v) {
  const url = v.url;
  const state = pullMediaStatus(url);
  const title = niceTitle(v.title) || url;
  const thumb = vodThumb(v.thumbnail_url);
  const meta = [(v.published_at || "").slice(0, 10), fmtDur(v.duration)]
    .filter(Boolean).map(htmlEscape).join(" · ");
  const badge = v.kind === "LiveBroadcast"
    ? '<span class="mp-badge">Live VOD</span>'
    : '<span class="mp-badge">Upload</span>';
  // The .vod-dl button + progress widget is the channel-detail pill's,
  // verbatim: same classes, same data attributes. `wireVodDownloadButtons`
  // wires the click, `updateVodProgressDom` patches the live percentage, and
  // `seedVodDownloadStateFromRecCache` finalises it — so there is no second
  // progress pipeline here to invent or keep in sync. The button also needs an
  // explicit name: its label is the bare word "Download", which would be the
  // accessible name of every row in the list.
  const { html: inner, title: dlTitle } = vodDownloadButtonInner(url, state);
  const btn = `<button type="button" class="vod-dl vod-dl-${state}" data-action="vod-download"
        data-url="${htmlEscape(url)}"
        data-channel="${htmlEscape(pullMedia.name)}"
        data-channel-id="${htmlEscape(pullMedia.channelId)}"
        data-platform="${htmlEscape(pullMedia.platform)}"
        data-title="${htmlEscape(v.title || "")}"
        aria-label="${htmlEscape(`Download ${title}`)}"
        ${dlTitle ? `title="${htmlEscape(dlTitle)}"` : ""}
        ${state !== "idle" ? "disabled" : ""}>${inner}</button>`;
  return `
    <div class="media-pill pm-row">
      <label class="mp-link pm-label">
        <input type="checkbox" class="pm-check" data-url="${htmlEscape(url)}"
               ${pullMedia.selected.has(url) ? "checked" : ""}
               ${state !== "idle" ? "disabled" : ""}
               aria-label="${htmlEscape(`Queue ${title}`)}">
        <span class="mp-thumb">${thumb ? `<img class="mp-thumb-img" loading="lazy" decoding="async" alt="" src="${htmlEscape(thumb)}" onerror="this.remove()">` : ""}</span>
        <span class="mp-info">
          <span class="mp-title">${htmlEscape(title)}</span>
          <span class="mp-sub">${meta}</span>
        </span>
        <span class="mp-meta">${badge}</span>
      </label>
      ${btn}
    </div>`;
}

// Repaint target. Two SSE arrivals drive it, never a timer: the
// `channel-vods` answer via the listener at the bottom of this file, and the
// recording lifecycle via repaintOpenChannelDownloads() (012c-pvr.js), which
// is the codebase's existing "vodDownloadState changed, redraw the VOD lists"
// hook. Both guard on `.open`, so a dismissed modal costs nothing.
function paintPullMediaList() {
  const modal = document.getElementById("pull-media-modal");
  if (!modal || !modal.classList.contains("open") || !pullMedia) return;
  const listEl = modal.querySelector("#pm-list");
  if (!listEl) return;
  const vods = pullMediaVods();
  if (channelVods[pullMedia.channelId] === undefined) {
    listEl.innerHTML = '<div class="empty sm">Loading past broadcasts…</div>';
  } else if (!vods.length) {
    listEl.innerHTML =
      '<div class="empty sm">No past broadcasts or uploads found for this channel.</div>';
  } else {
    listEl.innerHTML = `<div class="media-list">${vods.map(pullMediaRowHtml).join("")}</div>`;
  }
  // "Select all" covers the rows that are still tickable, and reports a
  // partial selection rather than silently dropping the ticked ones.
  const tickable = vods.filter((v) => pullMediaStatus(v.url) === "idle");
  const picked = tickable.filter((v) => pullMedia.selected.has(v.url)).length;
  const all = modal.querySelector("#pm-all");
  if (all) {
    all.checked = tickable.length > 0 && picked === tickable.length;
    all.indeterminate = picked > 0 && picked < tickable.length;
    all.disabled = tickable.length === 0;
  }
  // The count is part of the button's name (announced on focus) rather than
  // the live region, which stays reserved for the outcome — ticking boxes
  // must not talk over the user.
  const dl = modal.querySelector("#pm-download");
  if (dl) {
    dl.textContent = picked ? `Download ${picked} selected` : "Download selected";
    dl.disabled = picked === 0;
  }
  // Wire the rows this paint produced. They are destroyed by the next paint,
  // so the binding has to live here rather than at open time — and the
  // `wired` guard keeps a repaint from stacking a second listener on a node
  // that survived. Same idiom as wireVodDownloadButtons (012c-pvr.js).
  listEl.querySelectorAll(".pm-check").forEach((check) => {
    if (check.dataset.wired === "1") return;
    check.dataset.wired = "1";
    check.addEventListener("change", () => {
      if (check.checked) pullMedia.selected.add(check.dataset.url);
      else pullMedia.selected.delete(check.dataset.url);
      repaintPullMediaKeepingFocus();
    });
  });
  wireVodDownloadButtons();
}

// Repainting rebuilds #pm-list, so the checkbox that fired the change is
// destroyed by its own repaint — without this, tabbing the list and pressing
// Space drops focus to <body> on every tick.
function repaintPullMediaKeepingFocus() {
  const active = document.activeElement;
  const url = active && active.classList && active.classList.contains("pm-check")
    ? active.dataset.url
    : null;
  paintPullMediaList();
  if (!url) return;
  const again = [...document.querySelectorAll("#pull-media-modal .pm-check")]
    .find((c) => c.dataset.url === url);
  (again || document.getElementById("pm-all"))?.focus();
}

function openPullMedia(ctx) {
  const modal = pullMediaModal();
  pullMedia = { platform: ctx.platform, channelId: ctx.channelId, name: ctx.name, selected: new Set() };
  _pullPrevFocus = _appModalPrevFocus;
  // Ticking many items queues many downloads that share the live-capture
  // concurrency cap, so say that up front rather than implying unbounded
  // parallelism. maxConcurrentSlots comes from /settings via
  // setupChromeHandlers; 0 means "not read yet", hence the fallback wording.
  const capHint = maxConcurrentSlots > 0
    ? `Each selected item queues one download. StriVo captures at most ${maxConcurrentSlots} at a time — the same cap as live recording.`
    : "Each selected item queues one download, and they share the concurrent-recording cap.";
  // #pm-status is a polite live region, created empty here and filled only on
  // commit, so a screen reader registers it before the outcome lands (the same
  // pre-create rule Toast's regions follow). It replaces the toast rather than
  // doubling it — one announcement, persistent, in context.
  modal.innerHTML = `
    <div class="card playlist-viewer-card">
      <div class="playlist-viewer-head">
        <div>
          <h2>Pull past media — ${htmlEscape(pullMedia.name)}</h2>
          <p class="pg-cap-hint">One-off download. This channel is not added to your config and will not be monitored.</p>
        </div>
        <button type="button" class="modal-close" id="pm-close" aria-label="Close">✕</button>
      </div>
      <label class="pm-all-row"><input type="checkbox" id="pm-all" aria-label="Select every past broadcast and upload"> Select all</label>
      <div id="pm-list"></div>
      <p class="pg-cap-hint">${htmlEscape(capHint)}</p>
      <p class="pg-cap-hint" id="pm-status" role="status" aria-live="polite"></p>
      <div class="playlist-viewer-actions">
        <button type="button" id="pm-cancel">Close</button>
        <button type="button" class="primary" id="pm-download" disabled>Download selected</button>
      </div>
    </div>`;
  modal.querySelectorAll("#pm-close, #pm-cancel").forEach((btn) => {
    btn.addEventListener("click", closePullMedia);
  });
  modal.querySelector("#pm-all").addEventListener("change", (e) => {
    for (const v of pullMediaVods()) {
      if (pullMediaStatus(v.url) !== "idle") continue;
      if (e.currentTarget.checked) pullMedia.selected.add(v.url);
      else pullMedia.selected.delete(v.url);
    }
    paintPullMediaList();
  });
  modal.querySelector("#pm-download").addEventListener("click", startPullMedia);
  paintPullMediaList();
  openAppModal(modal);
  modal.querySelector(".pm-check:not(:disabled)")?.focus();
  // The endpoint is membership-agnostic — the channel does not have to be in
  // config.toml for it to answer. That is what makes this feature possible.
  if (channelVods[pullMedia.channelId] === undefined) {
    API.requestChannelVods(pullMedia.channelId, pullMedia.platform).catch((e) => {
      Toast.error(`Couldn't list past media: ${e.message}`);
    });
  }
}

function closePullMedia() {
  const modal = document.getElementById("pull-media-modal");
  if (!modal) return;
  closeAppModal(modal);
  // Hand focus back inside the wizard and re-arm its prev-focus binding, so
  // Tab keeps cycling the wizard and closing it returns to the topbar button.
  _appModalPrevFocus = _pullPrevFocus;
  _pullPrevFocus = null;
  document.getElementById("aw-pull")?.focus();
}

// Queue one POST /vods/download per ticked row. Deliberately NOT the
// per-row button's click path: that fires a toast per item, and a 20-item
// pull should announce itself once with a count.
async function startPullMedia() {
  const modal = document.getElementById("pull-media-modal");
  const ctx = pullMedia;
  const btn = modal && modal.querySelector("#pm-download");
  if (!ctx || !btn || btn.disabled || btn.dataset.busy === "1") return;
  const picked = pullMediaVods().filter(
    (v) => pullMediaStatus(v.url) === "idle" && ctx.selected.has(v.url),
  );
  if (!picked.length) return;
  btn.dataset.busy = "1";
  btn.setAttribute("aria-busy", "true");
  let queued = 0;
  let failed = 0;
  let firstError = "";
  for (const v of picked) {
    // Same optimistic bookkeeping wireVodDownloadButtons does for a single
    // pill: snapshot the job ids already known for this url so the shared
    // seed function can recognise the job THIS pull starts, and show the row
    // as downloading immediately instead of leaving it re-clickable.
    vodDownloadPending[v.url] = {
      at: Date.now(),
      knownIds: new Set(recCache.filter((r) => r.source_url === v.url).map((r) => r.id)),
    };
    vodDownloadState[v.url] = "downloading";
    try {
      await API.vodDownload({
        url: v.url,
        channel_name: ctx.name,
        channel_id: ctx.channelId,
        platform: ctx.platform,
        post_title: v.title || null,
      });
      queued++;
    } catch (err) {
      failed++;
      firstError = firstError || err.message;
      delete vodDownloadPending[v.url];
      delete vodDownloadState[v.url];
    }
  }
  delete btn.dataset.busy;
  btn.removeAttribute("aria-busy");
  paintPullMediaList();
  const status = modal.querySelector("#pm-status");
  const msg = failed
    ? `Queued ${queued} of ${picked.length} past broadcasts from ${ctx.name}. ${failed} could not be queued — ${firstError}`
    : `Queued ${queued} past broadcast${queued === 1 ? "" : "s"} from ${ctx.name}. Progress appears on each row.`;
  if (status) status.textContent = msg;
}

// The VOD list arrives over SSE into the shared `channelVods` cache, but
// 036-pvr.js's handler only repaints the channel-detail pane, so repaint the
// pull modal here. Deferred by one task because this listener is registered
// before 036's and that one assigns the cache synchronously — a direct read
// would still see the pre-answer value. One hop per event, not a poll.
events.on((event) => {
  if (event.ChannelVods && pullMedia && event.ChannelVods.channel_id === pullMedia.channelId) {
    setTimeout(paintPullMediaList, 0);
  }
});

function showPlaylistModal(opts) {
  let modal = document.getElementById("playlist-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "playlist-modal";
    modal.className = "app-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => {
      if (e.target === modal) modal.classList.remove("open");
    });
  }
  const rows = opts.loading
    ? "<div>Loading playlists…</div>"
    : [
        `<div class="pl-row" data-pl=""><b>▣ Whole channel</b> (all uploads)</div>`,
        ...opts.playlists.map(
          (p) =>
            `<div class="pl-row" data-pl="${htmlEscape(p.id)}">≡ ${htmlEscape(p.title)}${
              p.item_count != null ? ` (${p.item_count})` : ""
            }</div>`,
        ),
      ].join("");
  modal.innerHTML = `
    <div class="card">
      <h2>Bulk download — ${htmlEscape(opts.name)}</h2>
      <p class="pg-cap-hint">Choose a scope, then explicitly start the download.</p>
      <div class="pl-list">${rows}</div>
    </div>`;
  modal.classList.add("open");
  modal.querySelectorAll(".pl-row").forEach((row) => {
    row.addEventListener("click", async () => {
      const ch = pendingPlaylistChannel;
      if (!ch) return;
      const playlist_id = row.dataset.pl || null;
      if (!confirm(`Download ${row.textContent.trim()} now?`)) return;
      try {
        await API.bulkDownload(ch.id, {
          channel_name: ch.name,
          platform: "YouTube",
          action: "start",
          playlist_id,
        });
        bulkStatus[ch.id] = { done: 0, total: 0, active: true };
        modal.classList.remove("open");
        Toast.success(`Bulk download started — ${ch.name}`);
        if (currentRoute() === "library") render();
      } catch (e) {
        Toast.error(`Bulk download failed: ${e.message}`);
      }
    });
  });
}

// #74 — bulk-download toggle button reflecting live SSE progress.
function bulkButton(c) {
  const st = bulkStatus[c.id];
  if (st && st.active) {
    const pct = st.percent != null ? ` ${Math.round(st.percent)}%` : "";
    const label = st.total > 0 ? `⇩ ${st.done}/${st.total}${pct} — Stop` : `⇩ …${pct} — Stop`;
    return `<button data-action="bulk" data-bulk-active="true"
              data-channel-id="${c.id}"
              data-channel-name="${htmlEscape(c.display_name || c.name)}"
              data-platform="${c.platform}">${label}</button>`;
  }
  return `<button data-action="bulk" data-bulk-active="false"
            data-channel-id="${c.id}"
            data-channel-name="${htmlEscape(c.display_name || c.name)}"
            data-platform="${c.platform}">⇩ Bulk DL</button>`;
}

async function startRecordingFromCard(d) {
  try {
    await API.startRecording({
      channel_id: d.channelId,
      channel_name: d.channelName,
      display_name: d.displayName,
      platform: d.platform,
      from_start: d.fromStart === "true",
      stream_title: d.streamTitle || null,
      thumbnail_url: d.thumbnail || null,
      transcode: false,
    });
    Toast.success(
      `Recording ${d.fromStart === "true" ? "from start " : ""}— ${d.displayName || d.channelName}`,
    );
  } catch (e) {
    Toast.error(`Start failed: ${e.message}`);
  }
}

async function toggleAutoRecord(d) {
  const enabling = d.enabled === "true";
  try {
    await API.toggleAutoRecord(d.channelKey, enabling);
    Toast.success(enabling ? "Auto-record enabled" : "Auto-record disabled");
    await render();
  } catch (e) {
    Toast.error(`Auto-record toggle failed: ${e.message}`);
  }
}
