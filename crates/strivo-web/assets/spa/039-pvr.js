// Right-click context menu for a channel — rail row (`.ch-row`) and
// multiview live tile (`.ms-leaf`). One delegated `contextmenu` listener,
// idempotently bound, mirrors the "⋯" row-menu dismiss convention in
// 012-pvr.js (`closeAllRecRowMenus` / `recMenuBound`).
//
// Sections: per-channel Alerts, automatic live capture, and — Creator
// edition only — automatic upload downloads. The two download controls are
// binary toggles; there is no ambiguous "latest upload" one-shot action.
// The Creator section is gated behind `typeof buildCreatorPluginActionsPanel
// === "function"` (the same edition-detection idiom 028-pvr.js already
// uses), so it never throws and never renders in a PVR-only bundle.

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

async function openChannelCtxMenu(ctx, clientX, clientY) {
  closeChannelCtxMenu();

  // chCtxSetAutoDownload is defined only in 040-creator.js (a `-creator.js`
  // module build.rs drops entirely from a PVR build) — this typeof guard is
  // the codebase's edition-detection idiom, mirroring
  // `typeof buildCreatorPluginActionsPanel === "function"` in 028-pvr.js.
  const isCreator = typeof chCtxSetAutoDownload === "function";

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

  // Auto-record state: reuse the channel list the rail already renders from
  // (API.channels() is cached briefly, so this is not a fresh network hit
  // on every menu open).
  let autoRecord = false;
  try {
    const channels = await API.channels();
    const match = (channels || []).find((c) => c.id === ctx.channelId && c.platform === ctx.platform);
    autoRecord = !!(match && match.auto_record);
  } catch (_) {
    // Default to false — the toggle just reflects "unknown" as off.
  }

  // Auto-download (archiver tandem) state — Creator edition only, reuses
  // the existing Monitor payload rather than a new endpoint.
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
       ${ctxMenuItemHtml("auto-record", autoRecord, "Download livestreams")}
    </div>`,
    isCreator
      ? `<div class="ch-ctx-menu-section">
           ${ctxMenuItemHtml("auto-download", tandemOn, "Download uploads")}
         </div>`
      : `<div class="ch-ctx-menu-section">
           <button class="ch-ctx-menu-item" type="button" disabled aria-disabled="true" title="Upload automation requires StriVo Creator">
             <span class="ch-ctx-check" aria-hidden="true">—</span>Download uploads (Creator)
           </button>
         </div>`,
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
      const next = !(onLive !== false);
      onLive = next;
      await API.setChannelAlerts(ctx.channelKey, { on_live: next, on_upload: onUpload });
      btn.setAttribute("aria-checked", next ? "true" : "false");
      btn.querySelector(".ch-ctx-check").textContent = next ? "✓" : "";
    } else if (action === "clear-notifications") {
      if (!(await confirmDialog(`Clear notifications for ${ctx.title || ctx.channelKey}?`, { ok: "Clear", danger: true }))) return;
      await API.setChannelAlerts(ctx.channelKey, { on_live: null, on_upload: null });
      closeChannelCtxMenu();
    } else if (action === "alert-upload") {
      const next = !(onUpload !== false);
      onUpload = next;
      await API.setChannelAlerts(ctx.channelKey, { on_live: onLive, on_upload: next });
      btn.setAttribute("aria-checked", next ? "true" : "false");
      btn.querySelector(".ch-ctx-check").textContent = next ? "✓" : "";
    } else if (action === "auto-record") {
      const next = !autoRecord;
      autoRecord = next;
      await API.toggleAutoRecord(ctx.channelKey, next);
      btn.setAttribute("aria-checked", next ? "true" : "false");
      btn.querySelector(".ch-ctx-check").textContent = next ? "✓" : "";
    } else if (action === "auto-download" && typeof chCtxSetAutoDownload === "function") {
      const next = !tandemOn;
      tandemOn = next;
      await chCtxSetAutoDownload(ctx.channelKey, next);
      btn.setAttribute("aria-checked", next ? "true" : "false");
      btn.querySelector(".ch-ctx-check").textContent = next ? "✓" : "";
    }
  });

  positionCtxMenu(menu, clientX, clientY);
}

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
