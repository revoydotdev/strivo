// Right-click context menu for a channel — rail row (`.ch-row`) and
// multiview live tile (`.ms-leaf`). One delegated `contextmenu` listener,
// idempotently bound, mirrors the "⋯" row-menu dismiss convention in
// 012-pvr.js (`closeAllRecRowMenus` / `recMenuBound`).
//
// Sections: per-channel Alerts; "Automatic downloads" (Livestreams, then
// Uploads — both binary toggles, both functional in every edition, via
// PUT .../auto_record and PUT .../auto_download_uploads); a one-shot
// "Download latest VOD" action; and, Creator edition only, a separate
// "Archive back catalog after capture" toggle (the archiver tandem —
// NOT the same thing as "download new uploads": it back-fills a
// channel's whole catalog once a tandem-flagged capture finishes, see
// `crates/strivo-plugins/src/archiver/mod.rs`). The Creator section is
// gated behind `typeof chCtxSetAutoDownload === "function"` (the same
// edition-detection idiom 028-pvr.js uses for
// `buildCreatorPluginActionsPanel`), so it never throws and never renders
// in a PVR-only bundle.

function closeChannelCtxMenu() {
  const existing = document.querySelector(".ch-ctx-menu");
  if (existing) existing.remove();
}

// Resolve the Platform:id channel key + display context from whichever of
// the two right-clickable elements was hit. Returns null when the element
// carries no channel identity (e.g. a recording-backed `.ms-leaf`, which has
// no live channel to scope Alerts/Auto-record/Auto-download to).
function channelCtxFromTarget(el) {
  if (el.classList.contains("ch-row")) {
    const platform = el.dataset.platform || "";
    const channelId = el.dataset.channelId || "";
    if (!platform || !channelId) return null;
    return {
      channelKey: `${platform}:${channelId}`,
      platform,
      channelId,
      title: el.querySelector(".ch-name")?.textContent || "",
    };
  }
  if (el.classList.contains("ms-leaf")) {
    const streamId = el.dataset.streamId || "";
    if (!streamId) return null; // recording-backed tile: no live channel
    const sep = streamId.indexOf(":");
    if (sep < 0) return null;
    return {
      channelKey: streamId,
      platform: streamId.slice(0, sep),
      channelId: streamId.slice(sep + 1),
      title: el.dataset.title || "",
    };
  }
  return null;
}

function ctxMenuItemHtml(action, checked, label) {
  return `<button class="ch-ctx-menu-item" type="button" data-ctx-action="${action}" role="menuitemcheckbox" aria-checked="${checked ? "true" : "false"}">
    <span class="ch-ctx-check" aria-hidden="true">${checked ? "✓" : ""}</span>${htmlEscape(label)}
  </button>`;
}

// C4: a monitored channel (auto_record on, Twitch/YouTube) now auto-downloads
// its uploads too — the explicit "Uploads" toggle is redundant, not just
// inert, for as long as Livestreams stays on. Render it checked-and-disabled
// with a one-line reason rather than a togglable control that would silently
// no-op (or worse, read as "off" once unchecked while uploads keep landing
// anyway).
function ctxMenuImpliedItemHtml(label, hint) {
  return `<button class="ch-ctx-menu-item is-implied" type="button" role="menuitemcheckbox"
             aria-checked="true" aria-disabled="true" disabled title="${htmlEscape(hint)}">
    <span class="ch-ctx-check" aria-hidden="true">✓</span>${htmlEscape(label)}
    <span class="ch-ctx-hint micro">${htmlEscape(hint)}</span>
  </button>`;
}

function positionCtxMenu(menu, clientX, clientY) {
  // Append first so offsetWidth/Height are real, then clamp into the
  // viewport — a naive left/top=clientX/clientY menu clips at screen edges.
  document.body.appendChild(menu);
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const left = Math.min(clientX, Math.max(0, vw - w - 8));
  const top = Math.min(clientY, Math.max(0, vh - h - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
}

// Flip a menuitemcheckbox optimistically, call `apiCall(next)`, and roll the
// checkbox (+ the caller's own state var, via `setState`) back to `prev` with
// a `Toast.error` if the request fails. Shared by every toggle in this menu
// so a failed PUT never leaves the UI showing a state the server didn't
// actually save.
async function ctxToggle(btn, prev, setState, apiCall, what) {
  const next = !prev;
  setState(next);
  btn.setAttribute("aria-checked", next ? "true" : "false");
  btn.querySelector(".ch-ctx-check").textContent = next ? "✓" : "";
  try {
    await apiCall(next);
  } catch (err) {
    setState(prev);
    btn.setAttribute("aria-checked", prev ? "true" : "false");
    btn.querySelector(".ch-ctx-check").textContent = prev ? "✓" : "";
    Toast.error(`Couldn't update ${what}: ${err.message || err}`);
  }
}

async function openChannelCtxMenu(ctx, clientX, clientY) {
  closeChannelCtxMenu();

  // chCtxSetAutoDownload is defined only in 040-creator.js (a `-creator.js`
  // module build.rs drops entirely from a PVR build) — this typeof guard is
  // the codebase's edition-detection idiom, mirroring
  // `typeof buildCreatorPluginActionsPanel === "function"` in 028-pvr.js.
  const isCreator = typeof chCtxSetAutoDownload === "function";
  const isPatreon = ctx.platform === "Patreon";

  // Alerts state: the dedicated single-channel route (also lets the e2e
  // suite and any future settings page hit one stable endpoint).
  let onLive = null;
  let onUpload = null;
  try {
    const alerts = await API._fetch(`/channels/${encodeURIComponent(ctx.channelKey)}/alerts`);
    onLive = alerts && alerts.on_live != null ? !!alerts.on_live : null;
    onUpload = alerts && alerts.on_upload != null ? !!alerts.on_upload : null;
  } catch (_) {
    // Leave as "follow global default" if the lookup fails.
  }

  // Auto-record (livestream capture) + auto-download-uploads state both
  // live on the same ChannelEntry, so one list fetch covers both toggles.
  // Patreon rows don't appear in API.channels() (they're not a monitored
  // "platform" the way Twitch/YouTube are) — their auto_download_uploads
  // flag is stamped onto API.patreon()'s creators list instead.
  let autoRecord = false;
  let autoDownloadUploads = false;
  try {
    if (isPatreon) {
      const p = await API.patreon();
      const match = (p.creators || []).find((c) => c.id === ctx.channelId);
      autoDownloadUploads = !!(match && match.auto_download_uploads);
    } else {
      // API.channels() resolves { channels: [...] } (see loadChannelList,
      // 012-pvr.js) — this used to .find() directly on that wrapper
      // object, which has no .find method, so the lookup always threw and
      // silently fell through to the catch below, leaving autoRecord (and
      // therefore the Uploads-implied state, C4) permanently false.
      const resp = await API.channels();
      const match = (resp.channels || []).find((c) => c.id === ctx.channelId && c.platform === ctx.platform);
      autoRecord = !!(match && match.auto_record);
      autoDownloadUploads = !!(match && match.auto_download_uploads);
    }
  } catch (_) {
    // Default to false — the toggles just reflect "unknown" as off.
  }

  // Archiver-tandem state — Creator edition only, reuses the existing
  // Monitor payload rather than a new endpoint. NOT the same switch as
  // "Download uploads" above — see the module doc comment.
  let tandemOn = false;
  if (isCreator) {
    try {
      const mon = await API.monitor();
      tandemOn = (mon.auto_download || []).some((d) => d.key === ctx.channelKey);
    } catch (_) {
      // Default to false.
    }
  }

  const sections = [
    `<div class="ch-ctx-menu-section">
       <div class="ch-ctx-menu-title">${htmlEscape(ctx.title || ctx.channelKey)}</div>
     </div>`,
    `<div class="ch-ctx-menu-section">
       <div class="ch-ctx-menu-label micro">Alerts</div>
       ${ctxMenuItemHtml("alert-live", onLive !== false, "Alert on live")}
       ${ctxMenuItemHtml("alert-upload", onUpload !== false, "Alert on new upload")}
       <button class="ch-ctx-menu-item" type="button" data-ctx-action="clear-notifications">
         <span class="ch-ctx-check" aria-hidden="true">×</span>Clear notifications
       </button>
     </div>`,
    `<div class="ch-ctx-menu-section">
       <div class="ch-ctx-menu-label micro">Automatic downloads</div>
       ${isPatreon ? "" : ctxMenuItemHtml("auto-record", autoRecord, "Livestreams")}
       ${!isPatreon && autoRecord
         ? ctxMenuImpliedItemHtml("Uploads", "Included with Livestreams")
         : ctxMenuItemHtml("auto-download-uploads", autoDownloadUploads, "Uploads")}
     </div>`,
    `<div class="ch-ctx-menu-section">
       <button class="ch-ctx-menu-item" type="button" data-ctx-action="download-latest-vod">
         <span class="ch-ctx-check" aria-hidden="true">⇩</span>Download latest VOD
       </button>
     </div>`,
    isCreator
      ? `<div class="ch-ctx-menu-section">
           <div class="ch-ctx-menu-label micro">Creator</div>
           ${ctxMenuItemHtml("archiver-tandem", tandemOn, "Archive back catalog after capture")}
         </div>`
      : "",
  ].join("");

  const menu = document.createElement("div");
  menu.className = "ch-ctx-menu";
  menu.setAttribute("role", "menu");
  menu.dataset.channelKey = ctx.channelKey;
  menu.innerHTML = sections;

  menu.addEventListener("click", async (e) => {
    const btn = e.target.closest("[data-ctx-action]");
    if (!btn) return;
    e.stopPropagation();
    const action = btn.dataset.ctxAction;
    if (action === "alert-live") {
      const prev = onLive !== false;
      await ctxToggle(
        btn,
        prev,
        (v) => { onLive = v; },
        (next) => API.setChannelAlerts(ctx.channelKey, { on_live: next, on_upload: onUpload }),
        "live alerts",
      );
    } else if (action === "clear-notifications") {
      if (!(await confirmDialog(`Clear notifications for ${ctx.title || ctx.channelKey}?`, { ok: "Clear", danger: true }))) return;
      try {
        await API.setChannelAlerts(ctx.channelKey, { on_live: null, on_upload: null });
        closeChannelCtxMenu();
      } catch (err) {
        Toast.error(`Couldn't clear notifications: ${err.message || err}`);
      }
    } else if (action === "alert-upload") {
      const prev = onUpload !== false;
      await ctxToggle(
        btn,
        prev,
        (v) => { onUpload = v; },
        (next) => API.setChannelAlerts(ctx.channelKey, { on_live: onLive, on_upload: next }),
        "upload alerts",
      );
    } else if (action === "auto-record") {
      await ctxToggle(
        btn,
        autoRecord,
        (v) => { autoRecord = v; },
        (next) => API.toggleAutoRecord(ctx.channelKey, next),
        "livestream auto-download",
      );
      // Uploads flips between its own toggle and the disabled "implied by
      // Livestreams" state (C4) — reopen in place so it reflects the new
      // autoRecord value immediately instead of only on the next open.
      openChannelCtxMenu(ctx, clientX, clientY);
    } else if (action === "auto-download-uploads") {
      await ctxToggle(
        btn,
        autoDownloadUploads,
        (v) => { autoDownloadUploads = v; },
        (next) => API.setAutoDownloadUploads(ctx.channelKey, next),
        "upload auto-download",
      );
    } else if (action === "archiver-tandem" && typeof chCtxSetAutoDownload === "function") {
      await ctxToggle(btn, tandemOn, (v) => { tandemOn = v; }, (next) => chCtxSetAutoDownload(ctx.channelKey, next), "back-catalog archiving");
    } else if (action === "download-latest-vod") {
      await downloadLatestVod(ctx);
    }
  });

  positionCtxMenu(menu, clientX, clientY);
}

// One-shot "grab whatever this channel's most recent VOD is right now"
// action — distinct from the Automatic-downloads toggles above, which are
// ongoing opt-ins. Uses the cached per-channel VOD list when available
// (`channelVods`, populated by the channel-detail pane) and otherwise
// requests a fresh fetch and waits for the `ChannelVods` event.
let pendingLatestVodCtxMenu = null;
async function downloadLatestVod(ctx) {
  const cached = ((typeof channelVods !== "undefined" && channelVods[ctx.channelId]) || [])
    .filter((v) => v.url)
    .sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""))[0];
  if (cached) {
    if (!(await confirmDialog(`Download latest VOD from ${ctx.title || ctx.channelId}?`, { ok: "Download" }))) return;
    try {
      await API.vodDownload({ url: cached.url, channel_name: ctx.title, channel_id: ctx.channelId || null, platform: ctx.platform, post_title: cached.title || null });
      Toast.success("Latest VOD download started");
    } catch (err) {
      Toast.error(`Download failed: ${err.message || err}`);
    }
    closeChannelCtxMenu();
    return;
  }
  pendingLatestVodCtxMenu = { channelId: ctx.channelId, name: ctx.title, platform: ctx.platform };
  // channelId travels through the pending record so the deferred download
  // below (once the ChannelVods SSE reply lands) still carries it (C1).
  try {
    await API.requestChannelVods(ctx.channelId, ctx.platform);
    Toast.info("Loading the latest VOD…");
  } catch (err) {
    pendingLatestVodCtxMenu = null;
    Toast.error(`Couldn't load VODs: ${err.message || err}`);
  }
}

events.on((event) => {
  if (event.ChannelVods && pendingLatestVodCtxMenu && event.ChannelVods.channel_id === pendingLatestVodCtxMenu.channelId) {
    const latest = (event.ChannelVods.vods || []).filter((v) => v.url).sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""))[0];
    const request = pendingLatestVodCtxMenu;
    pendingLatestVodCtxMenu = null;
    if (latest) {
      API.vodDownload({ url: latest.url, channel_name: request.name, channel_id: request.channelId || null, platform: request.platform, post_title: latest.title || null })
        .then(() => Toast.success(`Downloading latest VOD — ${request.name}`))
        .catch((err) => Toast.error(`Download failed: ${err.message || err}`));
    } else {
      Toast.error("No downloadable VOD found");
    }
  }
});

if (!document.body.dataset.chCtxMenuBound) {
  document.body.dataset.chCtxMenuBound = "1";
  document.addEventListener("contextmenu", (e) => {
    const el = e.target.closest(".ch-row, .ms-leaf");
    if (!el) return;
    const ctx = channelCtxFromTarget(el);
    if (!ctx) return;
    e.preventDefault();
    openChannelCtxMenu(ctx, e.clientX, e.clientY);
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".ch-ctx-menu")) closeChannelCtxMenu();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeChannelCtxMenu();
  });
}
