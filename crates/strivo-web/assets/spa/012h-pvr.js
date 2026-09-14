// Bulk/download interaction contract (Remediation B).
//
// This module is intentionally the last owner of the channel-detail bulk
// controls.  The earlier picker grew organically around a click handler,
// which made opening a playlist start a download.  Every action below first
// describes a scope, then requires an explicit submit confirmation.

const DOWNLOAD_SCOPE_LABELS = Object.freeze({
  channel: "whole channel",
  playlist: "whole playlist",
  selected: "selected uploads",
});

const playlistItemsCache = Object.create(null);
let openPlaylistContext = null;
let pendingLatestVod = null;

function bulkProgressText(st) {
  if (!st || !st.active) return "";
  const pct = st.percent == null ? "" : ` · ${Math.round(Math.max(0, Math.min(100, st.percent)))}%`;
  return st.total ? `${st.done || 0}/${st.total}${pct}` : (pct ? pct.slice(3) : "Starting…");
}

// Shared channel button semantics.  This replaces the old terse "Bulk DL"
// label in every channel detail surface while retaining its data-action API.
bulkButton = function remediationBulkButton(c) {
  const st = bulkStatus[c.id];
  const name = c.display_name || c.name || c.id;
  if (st && st.active) {
    return `<button class="danger" data-action="bulk" data-bulk-active="true"
      data-channel-id="${htmlEscape(c.id)}" data-channel-name="${htmlEscape(name)}"
      data-platform="${htmlEscape(c.platform)}" aria-label="Cancel channel download">
      Cancel channel download${bulkProgressText(st) ? ` · ${htmlEscape(bulkProgressText(st))}` : ""}
    </button>`;
  }
  return `<button data-action="bulk" data-bulk-active="false"
    data-channel-id="${htmlEscape(c.id)}" data-channel-name="${htmlEscape(name)}"
    data-platform="${htmlEscape(c.platform)}" aria-label="Download whole channel">Download whole channel</button>`;
};

async function remediationBulkStart(channelId, channelName, platform, scope, playlistId, items) {
  const label = DOWNLOAD_SCOPE_LABELS[scope] || scope;
  const count = Array.isArray(items) ? items.length : 0;
  const detail = count ? ` (${count} item${count === 1 ? "" : "s"})` : "";
  if (!(await confirmDialog(`Start download for ${label}${detail} from ${channelName}?`, { ok: "Start download" }))) return false;
  if (scope === "selected") {
    const vodIds = items.map((item) => item.id || item.video_id).filter(Boolean);
    if (!vodIds.length) {
      Toast.error("The selected playlist items have no platform IDs");
      return false;
    }
    try {
      const reply = await API.bulkDownload(channelId, {
        channel_name: channelName,
        platform,
        action: "start",
        playlist_id: playlistId || null,
        vod_ids: vodIds,
      });
      bulkStatus[channelId] = { done: 0, total: vodIds.length, percent: 0, active: true, scope, operation_id: reply && reply.operation_id };
      Toast.success(`Started ${vodIds.length} selected download${vodIds.length === 1 ? "" : "s"} — ${channelName}`);
      paintChannelList();
      if (currentRoute() === "library" && typeof render === "function") render().catch(() => {});
      return true;
    } catch (e) {
      Toast.error(`Selected download failed: ${e.message}`);
      return false;
    }
  }
  const body = {
    channel_name: channelName,
    platform,
    action: "start",
    playlist_id: playlistId || null,
  };
  try {
    const reply = await API.bulkDownload(channelId, body);
    const operationId = reply && (reply.operation_id || reply.id);
    bulkStatus[channelId] = {
      done: 0,
      total: count,
      percent: 0,
      active: true,
      operation_id: operationId || null,
      scope,
    };
    Toast.success(`Started ${label} download — ${channelName}`);
    paintChannelList();
    if (currentRoute() === "library" && typeof render === "function") render().catch(() => {});
    return true;
  } catch (e) {
    Toast.error(`Download failed: ${e.message}`);
    return false;
  }
}

toggleBulk = async function remediationToggleBulk(ds) {
  const active = ds.bulkActive === "true";
  if (active) {
    if (!(await confirmDialog(`Cancel the channel download for ${ds.channelName}?`, { ok: "Cancel download", danger: true }))) return;
    try {
      await API.bulkDownload(ds.channelId, {
        channel_name: ds.channelName,
        platform: ds.platform,
        action: "stop",
        operation_id: bulkStatus[ds.channelId]?.operation_id || null,
      });
      delete bulkStatus[ds.channelId];
      Toast.success(`Cancelled channel download — ${ds.channelName}`);
      paintChannelList();
      if (currentRoute() === "library" && typeof render === "function") render().catch(() => {});
    } catch (e) {
      Toast.error(`Cancel failed: ${e.message}`);
    }
    return;
  }
  await remediationBulkStart(ds.channelId, ds.channelName, ds.platform, "channel", null, null);
};

function playlistItemThumb(item) {
  return item.thumbnail_url || item.thumbnail || item.thumb || "";
}

function playlistItemUrl(item) {
  return item.url || item.web_url || (item.video_id ? `https://www.youtube.com/watch?v=${item.video_id}` : "");
}

function playlistViewerHtml(ctx, playlist, items, loading) {
  const title = playlist ? playlist.title : "Playlist";
  const rows = loading
    ? '<div class="empty sm">Loading playlist items…</div>'
    : items.length
      ? `<div class="playlist-item-list">${items.map((item, i) => {
        const url = playlistItemUrl(item);
        const thumb = playlistItemThumb(item);
        const itemTitle = item.title || item.name || `Item ${i + 1}`;
        return `<label class="playlist-item-row">
          <input type="checkbox" class="playlist-item-check" data-index="${i}" checked>
          ${thumb ? `<img src="${htmlEscape(thumb)}" alt="" loading="lazy">` : '<span class="playlist-item-thumb"></span>'}
          <span class="playlist-item-copy"><b>${htmlEscape(itemTitle)}</b><small>${htmlEscape((item.published_at || item.upload_date || "").slice(0, 10))}</small></span>
          ${url ? `<a href="${htmlEscape(url)}" target="_blank" rel="noopener" aria-label="Open ${htmlEscape(itemTitle)}">↗</a>` : ""}
        </label>`;
      }).join("")}</div>`
      : '<div class="empty sm">No uploads in this playlist.</div>';
  return `<div class="card playlist-viewer-card">
    <div class="playlist-viewer-head"><div><h2>${htmlEscape(title)}</h2><p class="pg-cap-hint">Read-only playlist viewer. Selecting items never starts a download.</p></div>
      <button type="button" class="modal-close" data-playlist-close aria-label="Close">✕</button></div>
    ${rows}
    ${items.length ? `<div class="playlist-viewer-actions">
      <button type="button" data-playlist-download="selected">Download selected</button>
      <button type="button" class="primary" data-playlist-download="playlist">Download whole playlist</button>
    </div>` : ""}
  </div>`;
}

function openPlaylistViewer(ctx, playlist, items, loading = false) {
  const modal = document.getElementById("playlist-modal");
  if (!modal) return;
  openPlaylistContext = { ...ctx, playlist };
  modal.innerHTML = playlistViewerHtml(ctx, playlist, items || [], loading);
  modal.classList.add("open");
  modal.querySelector("[data-playlist-close]")?.addEventListener("click", () => modal.classList.remove("open"));
  modal.querySelectorAll("[data-playlist-download]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const scope = btn.dataset.playlistDownload;
      const selected = [...modal.querySelectorAll(".playlist-item-check:checked")]
        .map((check) => (items || [])[Number(check.dataset.index)])
        .filter(Boolean);
      if (scope === "selected" && !selected.length) {
        Toast.error("Select at least one playlist item");
        return;
      }
      const playlistId = playlist && playlist.id;
      const ok = await remediationBulkStart(ctx.id, ctx.name, ctx.platform,
        scope === "selected" ? "selected" : "playlist", playlistId,
        scope === "selected" ? selected : null);
      if (ok) modal.classList.remove("open");
    });
  });
}

function playlistListModal(ctx, playlists, loading = false) {
  let modal = document.getElementById("playlist-modal");
  if (!modal) {
    modal = document.createElement("div");
    modal.id = "playlist-modal";
    modal.className = "app-modal";
    document.body.appendChild(modal);
    modal.addEventListener("click", (e) => { if (e.target === modal) modal.classList.remove("open"); });
  }
  const rows = loading ? '<div class="empty sm">Loading playlists…</div>'
    : `<button type="button" class="bulk-scope-row" data-whole-channel>Download whole channel<span>All recent uploads</span></button>
       <div class="bulk-scope-label">Playlists</div>
       ${(playlists || []).map((p) => `<button type="button" class="bulk-scope-row" data-playlist-id="${htmlEscape(p.id)}"><b>${htmlEscape(p.title)}</b><span>${p.item_count == null ? "Open viewer" : `${p.item_count} items · Open viewer`}</span></button>`).join("") || '<div class="empty sm">No playlists found.</div>'}`;
  modal.innerHTML = `<div class="card playlist-picker-card"><div class="playlist-viewer-head"><div><h2>Downloads — ${htmlEscape(ctx.name)}</h2><p class="pg-cap-hint">Choose a scope. Opening a playlist only opens its viewer.</p></div><button type="button" class="modal-close" data-playlist-close aria-label="Close">✕</button></div>${rows}</div>`;
  modal.classList.add("open");
  modal.querySelector("[data-playlist-close]")?.addEventListener("click", () => modal.classList.remove("open"));
  modal.querySelector("[data-whole-channel]")?.addEventListener("click", async () => {
    if (await remediationBulkStart(ctx.id, ctx.name, ctx.platform, "channel", null, null)) modal.classList.remove("open");
  });
  modal.querySelectorAll("[data-playlist-id]").forEach((row) => {
    row.addEventListener("click", async () => {
      const playlist = (playlists || []).find((p) => p.id === row.dataset.playlistId);
      if (!playlist) return;
      const key = `${ctx.id}:${playlist.id}`;
      const cached = playlistItemsCache[key];
      openPlaylistViewer(ctx, playlist, cached || [], !cached);
      if (!cached) {
        try {
          if (API.requestPlaylistItems) await API.requestPlaylistItems(ctx.id, playlist.id);
          else await API._fetch(`/channels/${encodeURIComponent(ctx.id)}/playlists/${encodeURIComponent(playlist.id)}`);
        } catch (e) {
          Toast.error(`Couldn't load playlist: ${e.message}`);
        }
      }
    });
  });
}

// PlaylistList is an SSE response; override the old row-click picker while
// retaining its request path for callers that already invoke it.
showPlaylistModal = function remediationShowPlaylistModal(opts) {
  const ctx = pendingPlaylistChannel || { id: "", name: opts.name || "", platform: "YouTube" };
  playlistListModal({ id: ctx.id, name: opts.name || ctx.name, platform: "YouTube" }, opts.playlists || [], !!opts.loading);
};

// A playlist row is now a viewer navigation target, never a download target.
openPlaylistPicker = async function remediationOpenPlaylistPicker(ds) {
  pendingPlaylistChannel = { id: ds.channelId, name: ds.channelName, platform: ds.platform || "YouTube" };
  playlistListModal(pendingPlaylistChannel, [], true);
  try {
    await API.requestPlaylists(ds.channelId);
  } catch (e) {
    Toast.error(`Couldn't load playlists: ${e.message}`);
  }
};

if (typeof API.requestPlaylistItems !== "function") {
  API.requestPlaylistItems = (channelId, playlistId) =>
    API._fetch(`/channels/${encodeURIComponent(channelId)}/playlists/${encodeURIComponent(playlistId)}`);
}

// Keep operation state visible if the daemon includes operation_id.  The
// original handler still updates bulkStatus for compatibility with older SSE.
events.on((event) => {
  if (event.BulkProgress) {
    const p = event.BulkProgress;
    if (p.active) {
      bulkStatus[p.channel_id] = { ...bulkStatus[p.channel_id], done: p.done, total: p.total, percent: p.percent, active: true, operation_id: p.operation_id || bulkStatus[p.channel_id]?.operation_id || null };
    }
    if (p.active && currentRoute() === "library") paintChannelList();
  }
  if (event.PlaylistItems) {
    const p = event.PlaylistItems;
    playlistItemsCache[`${p.channel_id}:${p.playlist_id}`] = p.items || [];
    if (openPlaylistContext && openPlaylistContext.id === p.channel_id && openPlaylistContext.playlist?.id === p.playlist_id) {
      openPlaylistViewer(openPlaylistContext, openPlaylistContext.playlist, p.items || [], false);
    }
  }
  if (event.ChannelVods && pendingLatestVod && event.ChannelVods.channel_id === pendingLatestVod.channelId) {
    const latest = (event.ChannelVods.vods || []).filter((v) => v.url).sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""))[0];
    const request = pendingLatestVod;
    pendingLatestVod = null;
    if (latest) API.vodDownload({ url: latest.url, channel_name: request.name, platform: request.platform, post_title: latest.title || null }).then(() => Toast.success(`Downloading latest VOD — ${request.name}`)).catch((e) => Toast.error(`Download failed: ${e.message}`));
    else Toast.error("No downloadable VOD found");
  }
});

// Add a one-shot latest-VOD action to the shared context menu in PVR and
// Creator builds.  It deliberately does not depend on Creator hooks.
// This source sorts before 039-pvr.js (where the context menu is declared),
// so defer the wrapper until the concatenated module has finished evaluating.
queueMicrotask(() => {
  if (typeof openChannelCtxMenu !== "function") return;
  const priorOpenChannelCtxMenu = openChannelCtxMenu;
  openChannelCtxMenu = async function remediationContextMenu(ctx, x, y) {
    await priorOpenChannelCtxMenu(ctx, x, y);
    const menu = document.querySelector(".ch-ctx-menu");
    if (!menu) return;
    const section = document.createElement("div");
    section.className = "ch-ctx-menu-section";
    section.innerHTML = '<button class="ch-ctx-menu-item" type="button" data-ctx-action="download-latest-vod"><span class="ch-ctx-check" aria-hidden="true">⇩</span>Download latest VOD</button>';
    menu.appendChild(section);
    menu.addEventListener("click", async (e) => {
      if (!e.target.closest('[data-ctx-action="download-latest-vod"]')) return;
      const cached = (channelVods[ctx.channelId] || []).filter((v) => v.url).sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""))[0];
      if (cached) {
        if (!(await confirmDialog(`Download latest VOD from ${ctx.title || ctx.channelId}?`, { ok: "Download" }))) return;
        API.vodDownload({ url: cached.url, channel_name: ctx.title, platform: ctx.platform, post_title: cached.title || null }).then(() => Toast.success("Latest VOD download started")).catch((err) => Toast.error(`Download failed: ${err.message}`));
        closeChannelCtxMenu();
      } else {
        pendingLatestVod = { channelId: ctx.channelId, name: ctx.title, platform: ctx.platform };
        try { await API.requestChannelVods(ctx.channelId, ctx.platform); Toast.info("Loading the latest VOD…"); } catch (err) { pendingLatestVod = null; Toast.error(`Couldn't load VODs: ${err.message}`); }
      }
    });
  };
});
