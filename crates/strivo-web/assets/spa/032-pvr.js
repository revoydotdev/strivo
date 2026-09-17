
  // CE-Fusion F5: an Archive deep link opens straight into the EDL editor
  // rather than making the user find + click the button themselves.
  if (opts.seekSec != null) {
    if (isFinished) {
      overlay.querySelector("[data-action=rec-info-editor]")?.click();
    } else {
      Toast.info(`Target moment ${fmtClock(opts.seekSec)} — the EDL editor unlocks once this recording finishes.`);
    }
  }
}

// ── In-app player ────────────────────────────────────────────────────
// Custom controls — power-user keyboard maps mirror mpv where the HTML5
// video API allows. State is owned by the modal; no globals (except the
// modal-open class) leak out.

async function openRecordingPlayer(jobId, _opts = {}) {
  // The inline modal player has been retired — every recording-open
  // path now navigates to the dedicated recording player route
  // (#/play, 019b-pvr.js) rather than the live multiview wall (#/watch),
  // so a chat rail/composer/preset toolbar never show up for something
  // that isn't live. The seek parameter is preserved as a URL param so
  // in-context tools (Crunchr transcript click, cuepoints tick, EDL
  // editor jumps) still land at the right timecode.
  if (!jobId) return;
  // Defensive: dismiss any stray keymap/modal state before the route
  // change so the new Player surface isn't covered by leftover chrome.
  closeRecordingModals();
  document.getElementById("kbd-help")?.classList.remove("open");
  document.body.classList.remove("modal-open");
  const seek = _opts && _opts.seekTo ? `&t=${encodeURIComponent(_opts.seekTo)}` : "";
  window.location.hash = `#/play?recording=${encodeURIComponent(jobId)}${seek}`;
}

// Legacy modal-player implementation removed. The shim above redirects
// every call site to the Player route. If a future iter needs an
// inline mini-player (e.g. preview-in-context inside an Analytics
// pane), build it as a separate, narrower function rather than
// resurrecting the modal.

// (Legacy modal-player implementation deleted — ~100 lines of
// unreachable code after openRecordingPlayer's return.)

// The custom in-modal player (`wirePlayer`, mpv-style keymap + PiP +
// A-B loop) was retired along with the modal it lived in — every
// recording-open path now navigates to #/watch, whose player bar
// (019a-pvr.js) carries the same keymap via `wireTileKeys`, driven
// through the controller interface instead of a raw <video> element.

// ── Stub routes ──────────────────────────────────────────────────────
function renderStub(title, msg, context = captureRouteContext()) {
  if (!mountPage(`
    <h1 class="page-title">${htmlEscape(title)}</h1>
    <div class="empty">
      <div class="glyph">🚧</div>
      ${htmlEscape(msg)}
    </div>
  `, context)) return;
  setupChromeHandlers();
}

// ── Settings (Jellyfin-style two-pane shell) ────────────────────────
// Left rail = section nav (sub-route via #/settings/<section>).
// Right pane = section content. All knobs the daemon exposes get a
// visible row — read-only for now (Phase 2a). Phase 2b wires writes;
// Phase 2c adds the platforms wizard + keyring. Tooltip hints (the
// `title` attribute on .stg-hint) explain non-obvious knobs without
// cluttering the layout.
const SETTINGS_SECTIONS = [
  { slug: "general", label: "General", icon: "⚙" },
  { slug: "recording", label: "Recording", icon: "⏺" },
  { slug: "notifications", label: "Notifications", icon: "🔔" },
  { slug: "platforms", label: "Platforms", icon: "🔌" },
  { slug: "plugins", label: "Plugins", icon: "🧩" },
  { slug: "interface", label: "Interface", icon: "🎨" },
  { slug: "multiview", label: "Multi-view", icon: "▦" },
  { slug: "advanced", label: "Advanced", icon: "🛠" },
  { slug: "about", label: "About", icon: "ℹ" },
];

async function renderSettings(context = captureRouteContext()) {
  if (!mountRouteShell(context)) return;
  const parts = routeParts(); // ["settings", <slug?>]
  const slug = parts[1] || "general";
  let known = SETTINGS_SECTIONS.find((s) => s.slug === slug)
    ? slug
    : "general";
  // The Plugins pane is Creator Edition only.
  if (known === "plugins" && !CREATOR_ENABLED) known = "general";

  let s = {};
  try {
    s = await API.settings();
  } catch (e) {
    if (e.message && e.message.includes("unauthorized")) return;
  }
  if (!isRouteCurrent(context)) return;

  const rail = SETTINGS_SECTIONS
    .filter((sec) => CREATOR_ENABLED || sec.slug !== "plugins")
    .map((sec) => `
    <a class="stg-rail-item ${sec.slug === known ? "is-active" : ""}"
       href="#/settings/${sec.slug}">
      <span class="stg-rail-icon" aria-hidden="true">${sec.icon}</span>
      <span class="stg-rail-label">${htmlEscape(sec.label)}</span>
    </a>`).join("");

  const pane = renderSettingsPane(known, s);

  if (!mountPage(`
    <h1 class="page-title">Settings</h1>
    <p class="page-subtitle">Live daemon configuration. Toggles and numeric knobs persist to <code>~/.config/strivo/config.toml</code> on change.</p>
    <div class="stg-shell">
      <nav class="stg-rail" aria-label="Settings sections">
        <div class="stg-search-wrap">
          <input id="stg-search" class="stg-search" type="search"
                 placeholder="Filter settings…" aria-label="Filter settings" />
        </div>
        ${rail}
      </nav>
      <div class="stg-pane" id="stg-pane">${pane}</div>
    </div>
  `, context)) return;
  setupChromeHandlers();
  wireSettingsControls();
  wireSettingsSearch();
}

// Filter rows in the right pane and rail items by typed query (audit M10).
function wireSettingsSearch() {
  const input = document.getElementById("stg-search");
  if (!input) return;
  input.addEventListener("input", () => {
    const q = input.value.trim().toLowerCase();
    document.querySelectorAll(".stg-row").forEach((r) => {
      const txt = r.textContent.toLowerCase();
      r.classList.toggle("stg-row-hidden", q.length > 0 && !txt.includes(q));
    });
    // Hide group headings whose rows all collapsed.
    document.querySelectorAll(".stg-group").forEach((g) => {
      const anyVisible = g.querySelector(".stg-row:not(.stg-row-hidden)");
      g.style.display = q && !anyVisible ? "none" : "";
    });
  });
}

// Wire every editable control on the right pane. Each control declares
// its dotted config path via `data-stg-path` and its type via the input
// itself (checkbox / number). On change we POST to /settings/update;
// failure rolls the control back to its previous value and toasts.
function wireSettingsControls() {
  const pane = document.getElementById("stg-pane");
  if (!pane) return;
  // Multi-view quality selects are browser-local, so they persist straight
  // to localStorage rather than through the daemon config endpoint. Applying
  // takes effect on the next reconcile — no reload, that being the point.
  pane.querySelectorAll("[data-mv-quality]").forEach((sel) => {
    sel.addEventListener("change", () => {
      setMultiviewQuality(sel.dataset.mvQuality, sel.value);
      Toast.info("Multi-view quality updated — applies to open tiles immediately.");
      for (const [, ctl] of playerState.controllers) {
        try {
          ctl.setQuality(qualityPolicyFor(ctl.kind, ""));
        } catch (_) {
          /* advisory */
        }
      }
    });
  });

  // Configure / Reconfigure buttons on the Platforms section open a
  // wizard modal per platform.
  pane.querySelectorAll(".stg-cfg-btn").forEach((btn) => {
    btn.addEventListener("click", () => openPlatformWizard(btn.dataset.platform));
  });
  // Master toggles dim their dependent conditional subgroup when off — the
  // Notifications master switch dims Events, the Webhook enable toggle dims
  // the Webhook URL field. We do this in JS rather than re-rendering so
  // users see immediate visual feedback during the save round-trip. Each
  // `.stg-subgroup-conditional` names its master via `data-conditional-master`
  // so any number of these pairs can coexist.
  pane.querySelectorAll(".stg-subgroup-conditional[data-conditional-master]").forEach((condEl) => {
    const masterEl = pane.querySelector(`[data-stg-path="${condEl.getAttribute("data-conditional-master")}"]`);
    if (!masterEl) return;
    const syncMaster = () => {
      if (masterEl.checked) {
        condEl.style.opacity = "";
        condEl.style.pointerEvents = "";
      } else {
        condEl.style.opacity = "0.55";
        condEl.style.pointerEvents = "none";
      }
    };
    masterEl.addEventListener("change", syncMaster);
  });
  // Onboarding controls — replay the welcome tour.
  pane.querySelector("#stg-replay-tour")?.addEventListener("click", () => {
    localStorage.removeItem("strivo-tour-done");
    startOnboardingTour();
  });
  // Layout reorder widgets — Kodi/Aeon-style up/down lists.
  // Each .stg-reorder reads its current order from localStorage
  // (falling back to data-default), renders one row per entry with
  // ▲ / ▼ buttons, and persists on any movement.
  pane.querySelectorAll(".stg-reorder").forEach((box) => {
    const key = box.dataset.reorderKey;
    const def = JSON.parse(box.dataset.default || "[]");
    let order;
    try { order = JSON.parse(localStorage.getItem(key) || ""); if (!Array.isArray(order)) order = def; }
    catch { order = def; }
    // Repair: keep only known entries, append any default entries that
    // got added in a later release so the list never goes stale.
    order = order.filter((x) => def.includes(x));
    for (const d of def) if (!order.includes(d)) order.push(d);
    const list = box.querySelector(".stg-reorder-list");
    const render = () => {
      list.innerHTML = order.map((name, i) => `
        <div class="stg-reorder-item">
          <span class="stg-reorder-label">${htmlEscape(name)}</span>
          <button class="sm stg-reorder-up" data-i="${i}" type="button" ${i === 0 ? "disabled" : ""}>▲</button>
          <button class="sm stg-reorder-down" data-i="${i}" type="button" ${i === order.length - 1 ? "disabled" : ""}>▼</button>
        </div>`).join("");
      list.querySelectorAll(".stg-reorder-up").forEach((btn) => btn.addEventListener("click", () => {
        const i = +btn.dataset.i;
        if (i > 0) { [order[i - 1], order[i]] = [order[i], order[i - 1]]; persist(); render(); }
      }));
      list.querySelectorAll(".stg-reorder-down").forEach((btn) => btn.addEventListener("click", () => {
        const i = +btn.dataset.i;
        if (i < order.length - 1) { [order[i + 1], order[i]] = [order[i], order[i + 1]]; persist(); render(); }
      }));
    };
    const persist = () => localStorage.setItem(key, JSON.stringify(order));
    render();
    box.querySelector(".stg-reorder-reset")?.addEventListener("click", () => {
      order = def.slice();
      persist(); render();
      Toast.success("Reset to default order");
    });
  });
  pane.querySelectorAll(".stg-layout-select").forEach((sel) => {
    const key = sel.dataset.layoutKey;
    const stored = localStorage.getItem(key);
    if (stored) sel.value = stored;
    sel.addEventListener("change", () => {
      localStorage.setItem(key, sel.value);
      Toast.success("Layout preference saved");
    });
  });
