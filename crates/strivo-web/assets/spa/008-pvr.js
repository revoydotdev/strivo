  // Multi-stream tile layout for the watch player. Core (single + multi
  // view), available in the PVR build — kept outside the @creator block so
  // the player isn't broken by a missing API method.
  multistreamTiles: (containerW, containerH, mode, host) => {
    const p = new URLSearchParams({ container_w: containerW, container_h: containerH, host });
    if (mode) p.set("mode", JSON.stringify(mode));
    return API._fetch(`/multistream/tiles?${p.toString()}`);
  },
  patreonPull: (body) =>
    API._fetch("/patreon/pull", { method: "POST", body }),
  vodDownload: (body) =>
    API._fetch("/vods/download", { method: "POST", body }),
  remuxRecording: (id) =>
    API._fetch(`/recordings/${encodeURIComponent(id)}/remux`, { method: "POST" }),
  login: (apiKey) =>
    API._fetch("/auth/login", { method: "POST", body: { api_key: apiKey } }),
  logout: () => API._fetch("/auth/logout", { method: "POST" }),
  updateSetting: (path, value) =>
    API._fetch("/settings/update", { method: "POST", body: { path, value } }),
  setPlatform: (name, body) =>
    API._fetch(`/settings/platform/${encodeURIComponent(name)}`, {
      method: "POST",
      body,
    }),
  // ── Capture-profile CRUD ─────────────────────────────────────────────
  captureProfileCreate: (profile) =>
    API._fetch("/capture_profiles", { method: "POST", body: profile }),
  captureProfileUpdate: (name, profile) =>
    API._fetch(`/capture_profiles/${encodeURIComponent(name)}`, { method: "PUT", body: profile }),
  captureProfileDelete: (name) =>
    API._fetch(`/capture_profiles/${encodeURIComponent(name)}`, { method: "DELETE" }),
  // ── Channel JSON export / import ─────────────────────────────────────
  channelsExport: () => API._fetch("/channels/export"),
  channelsImport: (data) =>
    API._fetch("/channels/import", { method: "POST", body: data }),
};

// ── SSE event stream ─────────────────────────────────────────────────
const events = {
  source: null,
  listeners: new Set(),
  degradedPoll: null,
  start() {
    if (this.source) return;
    this.source = new EventSource("/events", { withCredentials: true });
    this.source.onopen = () => this.setConnected(true);
    this.source.onmessage = (e) => {
      this.setConnected(true);
      try {
        const data = JSON.parse(e.data);
        this.listeners.forEach((fn) => fn(data));
      } catch (_) {}
    };
    this.source.onerror = () => {
      // Make the stale-data state VISIBLE (research §A/§5: silent
      // real-time breakage is the #1 cited gotcha). The browser
      // auto-reconnects on transient errors; meanwhile we show a pill
      // and degrade to a slow poll so list views don't go stale.
      this.setConnected(false);
      // On a hard close (e.g. a session cookie that expired mid-stream),
      // EventSource will NOT auto-reconnect. Recreate it on a timer so the
      // stream comes back once the session is valid again — but only while
      // `authed` (012-pvr.js) is still true. Retrying unconditionally here
      // used to spam /events with 401s every 3s for as long as a visitor
      // sat on the (unauthenticated) login screen.
      if (this.source && this.source.readyState === EventSource.CLOSED) {
        this.source.close();
        this.source = null;
        if (authed) setTimeout(() => this.start(), 3000);
      }
    };
  },
  // Show/hide the topbar "reconnecting…" pill and arm/disarm a 10s
  // degraded re-poll of the current data route.
  setConnected(ok) {
    const pill = document.getElementById("conn-status");
    if (pill) pill.hidden = ok;
    if (ok) {
      if (this.degradedPoll) {
        clearInterval(this.degradedPoll);
        this.degradedPoll = null;
      }
    } else if (!this.degradedPoll) {
      this.degradedPoll = setInterval(() => {
        if (document.hidden) return;
        const r = currentRoute();
        if (r === "library") renderHome().catch(() => {});
        else if (r === "recordings") renderRecordings().catch(() => {});
      }, 10000);
    }
  },
  on(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  },
};

// #74 — per-channel bulk-download status, keyed by channel_id:
// { done, total, active }. Fed by the `bulk-progress` SSE event.
const bulkStatus = {};
// #75 — Patreon snapshot, fed by the `patreon-state` SSE event:
// { creators: [ChannelEntry], posts: { campaign_id: [PatreonPost] } }.
const patreonState = { creators: [], posts: {} };
// W4-alt — recordings grid sort/filter state + last-fetched cache.
let recSort = { col: "started", dir: "desc" };
let recFilter = "";
let recCache = [];
// R01 — recordings pagination cursor, mirrors histNextCursor/history's
// "Load more" pattern. null = no further pages.
let recNextCursor = null;
// Item 22 — recordings index density (compact|comfortable) + multi-select.
let recDensity = localStorage.getItem("strivo-rec-density") || "comfortable";
let recSelected = new Set();
// State chip filter — Set of state classnames the user has whitelisted
// ("finished", "recording", "downloading", "failed", "file-error"…).
// Empty = no filter (show everything). Persisted across page reloads.
let recStateFilter = new Set(
  (localStorage.getItem("strivo-rec-state-filter") || "")
    .split(",").filter(Boolean),
);
// Group-by toggle — "none" or "channel". Persisted; respects the
// Settings → Layout default when one has been set.
let recGroupBy = localStorage.getItem("strivo-rec-groupby")
  || localStorage.getItem("strivo-layout-rec-groupby")
  || "none";
// Date-range filter on started_at — ISO-prefix bounds, inclusive.
// Empty string = unbounded on that side.
let recDateFrom = "";
let recDateTo = "";
// Anchor for shift+click range selection. Tracks the last row whose
// selection state was toggled by direct interaction (click on checkbox or
// modifier+click on row). Reset when the recordings page re-renders.
let recAnchorId = null;
// TUI-redesign — left-rail channel cache, current selection, per-channel
// VOD cache (channel_id -> [VodEntry]), and the recordings dashboard cache.
let channelCache = [];
let selectedChannelKey = null;
const channelVods = {};
// Per-VOD download state for the Past Broadcasts / Recent uploads pills.
// Keys: VOD URL. Values: "downloading" | "downloaded". Absence = idle.
// Seeded from recCache on every recordings refresh via
// `seedVodDownloadStateFromRecCache()` — correlation is by exact source_url
// match (RecordingJob.source_url, stamped on DownloadVod), so a page reload
// or a previously-finished download both surface correctly without a FIFO
// guess.
const vodDownloadState = {};
let dashRecordings = [];
let dashSchedule = [];
// Cached max-concurrent-recordings limit from /settings; updated by
// setupChromeHandlers on every page render so the topbar slot pill
// stays in sync when the user changes the limit on the Monitor page.
let maxConcurrentSlots = 0;

// True for recording states still in flight.
function isInProgress(state) {
  const s = stateLabel(state).toLowerCase();
  return s.includes("record") || s.includes("resolv") || s.includes("stopp");
}

// ── Toasts (research §D) ──────────────────────────────────────────────
// One singleton with two pre-created ARIA live regions: polite for
// success/info, assertive for errors. Errors are sticky (action-needed);
// success/info auto-dismiss with hover-pause. Toasts are non-interactive
// (message + close only).
const Toast = (() => {
  let polite, assertive;
  function ensure() {
    if (polite && document.body.contains(polite)) return;
    const wrap = document.createElement("div");
    wrap.className = "toast-wrap";
    const mk = (role, live) => {
      const r = document.createElement("div");
      r.className = "toast-region";
      r.setAttribute("role", role);
      r.setAttribute("aria-live", live);
      return r;
    };
    assertive = mk("alert", "assertive");
    polite = mk("status", "polite");
    wrap.append(assertive, polite);
    document.body.appendChild(wrap);
  }
  // Pre-create the live regions at load so screen readers register them
  // BEFORE any message is injected — injecting a region and its content in
  // the same frame is unreliably announced (item 24).
  if (typeof document !== "undefined" && document.body) ensure();
  function show(kind, msg, sticky) {
    ensure();
    const region = kind === "error" ? assertive : polite;
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    el.innerHTML = `<span class="toast-msg"></span><button class="toast-close" aria-label="Dismiss">×</button>`;
    el.querySelector(".toast-msg").textContent = msg;
    const close = () => {
      el.classList.add("out");
      setTimeout(() => el.remove(), 200);
    };
    el.querySelector(".toast-close").addEventListener("click", close);
    region.appendChild(el);
    while (region.children.length > 4) region.firstChild.remove();
    if (!sticky) {
      const ttl = 5000;
      let timer = setTimeout(close, ttl);
      el.addEventListener("mouseenter", () => clearTimeout(timer));
      el.addEventListener("mouseleave", () => (timer = setTimeout(close, ttl)));
    }
    return close;
  }
  return {
    success: (m) => show("success", m, false),
    info: (m) => show("info", m, false),
    error: (m) => show("error", m, true), // sticky — user must see/dismiss
  };
})();

// Focus-trapped confirmation dialog for destructive actions. Resolves
// true/false. (research §D: modals only for irreversible actions.)
function confirmDialog(message, opts = {}) {
  return new Promise((resolve) => {
    const prev = document.activeElement;
    const modal = document.createElement("div");
    modal.className = "kbd-help open confirm-modal";
    modal.innerHTML = `<div class="card" role="alertdialog" aria-modal="true">
      <p class="confirm-msg"></p>
      <div class="confirm-actions">
        <button class="confirm-cancel">${htmlEscape(opts.cancel || "Cancel")}</button>
        <button class="confirm-ok ${opts.danger ? "danger" : "primary"}">${htmlEscape(opts.ok || "Confirm")}</button>
      </div></div>`;
    modal.querySelector(".confirm-msg").textContent = message;
    document.body.appendChild(modal);
    // B1: track modal-open via reference count so confirms stacked
    // alongside other modals don't yank the body lock prematurely.
    bumpModalOpen(+1);
    const ok = modal.querySelector(".confirm-ok");
    const cancel = modal.querySelector(".confirm-cancel");
    const done = (v) => {
      modal.remove();
      bumpModalOpen(-1);
      // C5: only restore focus if prev still exists in the document.
      if (prev && prev.isConnected && prev.focus) prev.focus();
      else document.getElementById("brand-home")?.focus?.();
      resolve(v);
    };
    ok.addEventListener("click", () => done(true));
    cancel.addEventListener("click", () => done(false));
    modal.addEventListener("click", (e) => {
      if (e.target === modal) done(false);
    });
    modal.addEventListener("keydown", (e) => {
      if (e.key === "Escape") { e.preventDefault(); done(false); }
      if (e.key === "Tab") {
        e.preventDefault();
        (document.activeElement === ok ? cancel : ok).focus();
      }
    });
    ok.focus();
  });
}

// B3/B17/B20: reference-counted body.modal-open so stacked modals
// (confirm-over-info, info-over-palette, etc.) don't fight over the
// single class. Every modal-open writer bumps +1 on open and -1 on
// close. The class flips off only when the count hits 0.
let _modalOpenCount = 0;
function bumpModalOpen(delta) {
  _modalOpenCount = Math.max(0, _modalOpenCount + delta);
  document.body.classList.toggle("modal-open", _modalOpenCount > 0);
}

// Run an async action with a busy/debounced button: aria-busy + label
// swap + double-fire guard. Safe even if the handler re-renders the page.
async function withBusy(btn, busyLabel, fn, timeoutMs = 30000) {
  if (btn) {
    if (btn.dataset.busy === "1") return; // debounce double-submit
    btn.dataset.busy = "1";
    btn.setAttribute("aria-busy", "true");
    btn.classList.add("busy");
    if (busyLabel) {
      btn.dataset.prevLabel = btn.textContent;
      btn.textContent = busyLabel;
    }
  }
  // Never strand a spinner: race the work against a timeout so a hung
  // request still tears the busy state down and surfaces an error (item 25).
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("timed out")), timeoutMs);
  });
  try {
    return await Promise.race([fn(), timeout]);
  } finally {
    clearTimeout(timer);
    if (btn && btn.isConnected) {
      btn.dataset.busy = "0";
      btn.removeAttribute("aria-busy");
      btn.classList.remove("busy");
      if (btn.dataset.prevLabel) btn.textContent = btn.dataset.prevLabel;
    }
  }
}

// ── Hash router ──────────────────────────────────────────────────────
const ROUTES = [
  "library",
  "recordings",
  "schedule",
  "watch",
  "play",
  "studio",
  "analytics",
  "publish",
  "pipelines",
  "viewer",
  "dataviz",
  "archive",
  "plugins",
  "chat",
  "settings",
  "system",
  "logs",
  "history",
  "login",
];

function currentRoute() {
  // Strip any query string ("#/recordings?channel=foo") so the route
  // matcher only sees the path segment.
  const raw = window.location.hash.replace(/^#\/?/, "").split("?")[0];
  const hash = raw || "library";
  // Sub-routes (e.g. #/plugins/crunchr) highlight their base tab.
  const base = hash.split("/")[0];
  return ROUTES.includes(base) ? base : "library";
}

// Path segments after the leading "#/", e.g. #/plugins/crunchr/rec/<id>
// → ["plugins", "crunchr", "rec", "<id>"].
function routeParts() {
  return window.location.hash
    .replace(/^#\/?/, "")
    .split("/")
    .filter(Boolean)
    .map((s) => decodeURIComponent(s));
}

function route(name) {
  window.location.hash = `#/${name}`;
}

// R03: the skip-link targets the in-page anchor `#content`, not a route —
// every real route hash has the "#/..." shape (see `route()` above and
// every `location.hash = "#/..."` assignment in this file). Re-running the
// router for `#content` used to fall through `currentRoute()`'s unknown-
// route fallback to "library" and repaint the whole chrome, replacing the
// just-focused `<main id="content">` with a fresh node — a race against
// the browser's own fragment-focus step that flaked the skip-link e2e
// test. Real route changes still repaint; the skip-link's own navigation
// no longer fights the browser for focus of the element it just landed on.
window.addEventListener("hashchange", () => {
  if (!window.location.hash.startsWith("#/")) return;
  render();
});

// C1/C15: defensive preventDefault for anchors styled as buttons —
// any <a href="#" data-action / data-seek / data-trace> would otherwise
// navigate to '#' if clicked BEFORE its specific handler wires. The
// per-element click listener still does the actual work; this just
// stops the default navigation flicker from racing it. Document-level
// capture so it runs before per-element handlers.
document.addEventListener("click", (e) => {
  const a = e.target.closest('a[href="#"], a[href="#/"]');
  if (!a) return;
  // Only suppress for the styled-as-button cases — any anchor with
  // one of these data attrs is acting as a click target.
  if (a.matches("[data-action], [data-seek], [data-trace]")) {
    e.preventDefault();
  }
}, true);

// ── Render ───────────────────────────────────────────────────────────
const root = document.getElementById("app");

// A route owns every asynchronous commit it starts.  Hash changes can arrive
// while a previous route is waiting for data, so matching just the route name
// is insufficient (a channel selection can re-render the same route).
let routeGeneration = 0;
function captureRouteContext() {
  return {
    generation: routeGeneration,
    route: currentRoute(),
    hash: window.location.hash,
  };
}
function isRouteCurrent(context) {
  return !!context
    && context.generation === routeGeneration
    && context.route === currentRoute()
    && context.hash === window.location.hash;
}

function announceNavigation(context) {
  if (!isRouteCurrent(context)) return;
  // `mountPage` owns the long-lived chrome.  It also gives a direct deep
  // link a usable shell before any API request completes.
  if (typeof mountRouteShell === "function") mountRouteShell(context);
}

// Per-route cache requirements. Every entry lists which module-scoped
// caches the route's render needs hydrated before chrome() paints.
// Routes that depend on the chrome's rail (channelCache + recCache)
// MUST list both — direct deep-link entry never runs renderHome() so
// the rail would otherwise be empty.
const ROUTE_HYDRATION = {
  library:     ["channels", "recordings", "schedule", "patreon"],
  recordings:  ["recordings"],
  watch:       ["channels", "recordings"],
  schedule:    ["recordings"], // active-count chip needs it
  pipelines:   ["channels"],
  studio:      ["channels"],
  analytics:   ["channels"],
  publish:     ["channels"],
  viewer:      ["channels", "recordings"],
  dataviz:     ["recordings"],
  archive:     ["channels"],
  plugins:     ["channels"],
  chat:        ["channels"],
  history:     ["recordings"],
};

// The rail is application chrome, not route data.  Keep an explicit loaded
// bit: an empty account is successfully hydrated and must not refetch on
// every navigation just because its array has length zero.
const hydrationLoaded = {
  channels: false,
  recordings: false,
  schedule: false,
  patreon: false,
};

async function ensureRouteHydration(route, context = captureRouteContext()) {
  const wants = new Set(["channels", "recordings", ...(ROUTE_HYDRATION[route] || [])]);
  const jobs = [];
  // A2/A3: track failed hydrations so the UI can surface a toast
  // instead of silently rendering an empty rail / 0-active-count.
  const failures = [];
  if (wants.has("channels") && !hydrationLoaded.channels) {
    jobs.push(API.channels()
      .then((r) => {
        if (!isRouteCurrent(context)) return;
        channelCache = r.channels || [];
        hydrationLoaded.channels = true;
        if (isRouteCurrent(context)) paintChannelList();
      })
      .catch((e) => { failures.push(`channels: ${e.message || e}`); }));
  }
  if (wants.has("recordings") && !hydrationLoaded.recordings) {
    jobs.push(API.recordings()
      .then((r) => {
        if (!isRouteCurrent(context)) return;
        recCache = r.recordings || [];
        if (typeof seedVodDownloadStateFromRecCache === "function") seedVodDownloadStateFromRecCache();
        dashRecordings = recCache;
        hydrationLoaded.recordings = true;
        if (isRouteCurrent(context)) paintChannelList();
      })
      .catch((e) => { failures.push(`recordings: ${e.message || e}`); }));
  }
  if (wants.has("schedule") && !hydrationLoaded.schedule) {
    jobs.push(API.schedule()
      .then((r) => {
        if (!isRouteCurrent(context)) return;
        dashSchedule = r.schedule || []; hydrationLoaded.schedule = true;
      })
      .catch((e) => { failures.push(`schedule: ${e.message || e}`); }));
  }
  if (wants.has("patreon") && !hydrationLoaded.patreon) {
    jobs.push(typeof seedPatreon === "function"
      ? seedPatreon(context).then((loaded) => { if (loaded) hydrationLoaded.patreon = true; }).catch(() => {})
      : Promise.resolve());
  }
  if (jobs.length) await Promise.allSettled(jobs);
  // Suppress 401 noise (handled by the auth probe a few lines later).
  const real = failures.filter((s) => !/unauthorized/i.test(s));
  if (real.length && isRouteCurrent(context) && typeof Toast !== "undefined") {
    Toast.error?.(`Couldn't load: ${real.join(" · ")}`);
  }
}

// A15: unified lookup across both channel sources. Patreon channels live
// in patreonState.creators, regular channels in channelCache; anywhere
// the code needs to resolve a channel by id OR (platform, id) tuple
// should call this so nothing is missing for Patreon.
function findChannelById(id, platform) {
  if (!id) return null;
  const inMain = channelCache.find((c) => c.id === id && (!platform || c.platform === platform));
  if (inMain) return inMain;
  const inPatreon = (patreonState.creators || []).find((c) => c.id === id && (!platform || c.platform === platform));
  return inPatreon || null;
}

// Centralised per-route teardown — all per-route timers + transient
// UI flags get cleared on every render() call so no route bleed.
function teardownAcrossRoutes() {
  // Player controllers hold live vendor connections; dropping the DOM does
  // not close them.
  if (typeof destroyAllControllers === "function") destroyAllControllers();
  if (typeof destroyChannelDetailPreview === "function") destroyChannelDetailPreview();
  // #/play swaps in a scratch single-slot layout; hand the wall its own back
  // before any route (including #/watch) renders.
  if (typeof restoreWallLayoutAfterPlay === "function") restoreWallLayoutAfterPlay();
  // Modals: kbd-help + body class + every app-modal still in the DOM.
  document.getElementById("kbd-help")?.classList.remove("open");
  // B3: route change always zeroes the modal-open ref count + the
  // class. Any modal that didn't pair its open/close cleanly is
  // forcibly cleaned up at the boundary.
  _modalOpenCount = 0;
  document.body.classList.remove("modal-open");
  try { closeAllAppModals?.(); } catch (_) {}
  try { closeRecordingModals?.(); } catch (_) {}
  // Per-route timers that previously leaked between routes.
  if (typeof cdPosterTimer !== "undefined" && cdPosterTimer) { clearInterval(cdPosterTimer); cdPosterTimer = null; }
  if (playerState && playerState.refreshTimer) { clearInterval(playerState.refreshTimer); playerState.refreshTimer = null; }
  if (typeof _watchRefreshTimer !== "undefined" && _watchRefreshTimer) { clearInterval(_watchRefreshTimer); _watchRefreshTimer = null; }
  if (typeof logsFollowTimer !== "undefined" && logsFollowTimer) { clearInterval(logsFollowTimer); logsFollowTimer = null; }
  // Transient keyboard-prefix + command-palette state.
  if (typeof prefixActive !== "undefined") prefixActive = false;
  if (typeof prefixTimer !== "undefined" && prefixTimer) { clearTimeout(prefixTimer); prefixTimer = null; }
}

// Timer-leak guard (mock-lane e2e only, via window.__strivoTestHooks):
// the current value of every per-route timer teardownAcrossRoutes() is
// responsible for clearing. All of these must read null once a route has
// actually torn down — a stray interval id here is the A-01 class of bug.
function debugActiveTimers() {
  return {
    cdPosterTimer: typeof cdPosterTimer !== "undefined" ? cdPosterTimer : null,
    playerRefreshTimer: (typeof playerState !== "undefined" && playerState) ? playerState.refreshTimer : null,
    watchRefreshTimer: typeof _watchRefreshTimer !== "undefined" ? _watchRefreshTimer : null,
    logsFollowTimer: typeof logsFollowTimer !== "undefined" ? logsFollowTimer : null,
  };
}

async function render() {
  routeGeneration += 1;
  const context = captureRouteContext();
  const r = currentRoute();
  // Bounce Creator Edition deep-links to Home in the pure-PVR build (their
  // backend routes don't exist here). The hash change re-enters render().
  if (CREATOR_ROUTES.has(r) && !CREATOR_ENABLED) {
    location.hash = "#/library";
    return;
  }
  announceNavigation(context);
  teardownAcrossRoutes();
  // P0 perf: tear down per-route long-lived resources before painting
  // the next route. Chat WebSockets, chat buffers, and dataviz resize
  // listeners were accumulating across navigations.
  if (r !== "chat" && typeof chatState !== "undefined") {
    // Only kill sockets that no painter still references — the
    // player-view chat rail keeps its room alive across navigations
    // when open.
    for (const room of Object.keys(chatState.sockets || {})) {
      if (!mountedChatRooms.has(room)) {
        try { disconnectChatRoom(room); } catch (_) {}
        delete chatState.buffers[room];
      }
    }
  }
  if (r !== "dataviz" && typeof teardownDataviz === "function") {
    teardownDataviz();
  }
  if (r !== "archive" && typeof teardownArchive === "function") {
    teardownArchive();
  }
  // Shell and route data load concurrently.  The route painter commits its
  // own placeholders immediately; neither a health probe nor a Patreon
  // failure should hold the destination hostage.
  if (r !== "login") {
    ensureRouteHydration(r, context).catch(() => {});
  }
  if (r !== "login") {
    API.health().catch((e) => console.warn(e));
  }
  if (!isRouteCurrent(context)) return;
  switch (r) {
    case "login":
      renderLogin(undefined, context);
      break;
    case "library":
      await renderHome(context);
      break;
    case "recordings":
      await renderRecordings(context);
      break;
    case "schedule":
      await renderSchedule(context);
      break;
