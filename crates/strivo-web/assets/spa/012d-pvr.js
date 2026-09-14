// ── Add-Channel two-phase wizard (item 19) ───────────────────────────
// Phase 1: pick platform + type a name → resolve (live, via SSE).
// Phase 2: show the resolved channel → confirm → enable auto-record.
// Config is deferred until the entity is confirmed.
let addWizard = null; // { platform, query } while a resolve is in flight

function openAddChannelWizard() {
  let modal = document.getElementById("add-channel-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "add-channel-modal";
    modal.className = "app-modal";
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => {
      if (e.target === modal) closeAppModal(modal);
    });
  }
  paintAddWizardSearch(modal);
  modal.classList.add("open");
  document.body.classList.add("modal-open");
}

// One owner for the click-outside / ESC / route-change dismissal of all
// .app-modal dialogs. Built so the keyboard-help (kbd-help) overlay
// stays separate — it has its own toggle and shouldn't be auto-closed
// on navigation.
function closeAppModal(modal) {
  if (!modal) return;
  modal.classList.remove("open");
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
  result.innerHTML = `
    <div class="wizard-confirm">
      <p class="wizard-step">Step 2 of 2 — confirm</p>
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(name)}</span>
          <span class="task-cadence">${htmlEscape(ev.platform)} · ${htmlEscape(ev.channel_id)}</span>
        </div>
      </div>
      <button id="aw-confirm" class="primary" data-key="${htmlEscape(ev.platform)}:${htmlEscape(ev.channel_id)}">
        Add &amp; enable auto-record
      </button>
    </div>`;
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

function showPlaylistModal(opts) {
  let modal = document.getElementById("playlist-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "playlist-modal";
    modal.className = "app-modal";
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
