    case "chat":
      await renderChat(context);
      break;
    case "settings":
      await renderSettings(context);
      break;
    case "system":
      await renderSystem(context);
      break;
    case "logs":
      await renderLogs(context);
      break;
    case "history":
      // History folded into Recordings as a Timeline view (see the
      // Table|Timeline segmented control in renderRecordings, 012-pvr.js).
      // "history" stays a real ROUTES entry (008-pvr.js) purely so old
      // #/history links/bookmarks still land somewhere instead of 404ing.
      location.hash = "#/recordings?view=timeline";
      return;
  }
}

// ── Edition gating ────────────────────────────────────────────────────
// The SPA bundle is shared by both editions. `creator_enabled` from
// /api/v1/settings says whether this server is the Creator Edition; the
// pure-PVR daemon does not mount the plugin/tooling routes, so those nav
// entries + actions would 404. Default false (a missing flag = PVR/older
// daemon) so we fail closed and hide creator surfaces.
let CREATOR_ENABLED = false;
// Routes whose backend lives behind `--features creator`. (Chat is NOT here:
// it speaks Twitch IRC directly from the browser, so it works in the PVR build.)
const CREATOR_ROUTES = new Set([
  "studio", "analytics", "publish", "pipelines", "plugins", "dataviz", "archive",
]);

// Reduced motion (F-18): the `ui.reduce_motion` setting is a manual override
// on top of the OS-level `prefers-reduced-motion` media query — either one
// being "on" should suppress decorative animation. We track the setting
// value ourselves (settings responses aren't otherwise cached globally) and
// recompute the effective state whenever the setting changes or the OS
// preference flips, so no reload is required either way.
let REDUCE_MOTION_SETTING = false;
const REDUCE_MOTION_QUERY = window.matchMedia
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : null;
function applyReducedMotion() {
  const osReduced = !!(REDUCE_MOTION_QUERY && REDUCE_MOTION_QUERY.matches);
  document.documentElement.classList.toggle("reduce-motion", REDUCE_MOTION_SETTING || osReduced);
}
if (REDUCE_MOTION_QUERY) {
  if (REDUCE_MOTION_QUERY.addEventListener) {
    REDUCE_MOTION_QUERY.addEventListener("change", applyReducedMotion);
  } else if (REDUCE_MOTION_QUERY.addListener) {
    // Safari < 14 fallback.
    REDUCE_MOTION_QUERY.addListener(applyReducedMotion);
  }
}
// Apply the OS preference immediately, before settings have loaded, so a
// reduced-motion browser never sees an unsuppressed boot-glyph pulse.
applyReducedMotion();

// Set on the first successful authenticated API call (a login, or — on
// reload with a still-valid session cookie — fetchEdition() itself
// succeeding). Boot-time SSE/Patreon-seed calls wait on this so a
// logged-out visitor doesn't trigger a pre-login fetch storm (008-pvr.js's
// SSE reconnect loop gates its retry on the same flag).
let authed = false;

async function fetchEdition() {
  try {
    const st = await API.settings();
    CREATOR_ENABLED = !!st.creator_enabled;
    REDUCE_MOTION_SETTING = !!(st.ui && st.ui.reduce_motion);
    applyReducedMotion();
    authed = true;
  } catch (_) {
    CREATOR_ENABLED = false;
  }
}

// Top-bar route nav (functional pages). The left rail is the channel
// list now; these icon links reach the management pages.
// Tuple: [route, fallbackGlyph, label, key, iconHref?]
// Eight slots ship Eliver Lara's candy-icons (GPL-3.0, vendored under
// /assets/icons/candy/ with the upstream LICENSE + ATTRIBUTION).
// History has no nav slot of its own any more — it's a Timeline view
// inside Recordings (segmented control in renderRecordings). "history"
// stays a ROUTES entry purely so old #/history links redirect there
// instead of 404ing (render(), 012-pvr.js).
const TOPNAV = [
  // Free panes — capture-loop core.
  ["library", "▣", "Home", "l", "/assets/icons/sweet-folders/folder-home.svg"],
  ["recordings", "📁", "Recordings", "r", "/assets/icons/sweet-folders/folder-videos.svg"],
  ["schedule", "📅", "Monitor", "s", "/assets/icons/candy/schedule.svg"],
  ["watch", "▶", "Player", "w", "/assets/icons/candy/watch.svg"],
  // Pro panes — unified app, each pane bundles every contributing
  // plugin's UI under its own tabs. Discrete plugin entries are kept
  // accessible via /plugins → deep-link rows but no longer hold the
  // primary topnav slot.
  ["studio", "🎬", "Studio", "u", "/assets/icons/candy/plugins.svg"],
  ["analytics", "📈", "Analytics", "a", "/assets/icons/sweet-folders/folder-documents.svg"],
  ["publish", "🚀", "Publish", "p", "/assets/icons/candy/pipelines.svg"],
  // No vendored candy icon for Archive yet — falls back to the glyph.
  ["archive", "🗄", "Archive", "v"],
  ["chat", "💬", "Chat", "t", "/assets/icons/candy/chat.svg"],
  ["settings", "⚙", "Settings", "c", "/assets/icons/candy/settings.svg"],
  ["system", "🛠", "System", "y", "/assets/icons/candy/system.svg"],
  ["logs", "📜", "Logs", "o", "/assets/icons/candy/logs.svg"],
];

function chrome(content) {
  const r = currentRoute();
  // Apply the user's Aeon-style top-nav reorder if any. Unknown
  // entries fall through in their default position so new releases
  // can extend TOPNAV without breaking saved order.
  let layoutOrder;
  try { layoutOrder = JSON.parse(localStorage.getItem("strivo-layout-topnav") || ""); }
  catch { layoutOrder = null; }
  const navItems = Array.isArray(layoutOrder)
    ? [
        ...layoutOrder
          .map((name) => TOPNAV.find((e) => e[0] === name))
          .filter(Boolean),
        ...TOPNAV.filter((e) => !layoutOrder.includes(e[0])),
      ]
    : TOPNAV;
  // Hide Creator Edition routes in the pure-PVR build.
  const visibleNav = CREATOR_ENABLED
    ? navItems
    : navItems.filter((e) => !CREATOR_ROUTES.has(e[0]));
  const nav = visibleNav.map(([route, glyph, label, key, iconHref]) => {
    const inner = iconHref
      ? `<img class="topnav-icon" src="${iconHref}" alt="" />`
      : `<span aria-hidden="true">${glyph}</span>`;
    return `<a class="topnav-link ${route === r ? "active" : ""}"
              href="#/${route}" data-route="${route}" data-key="${key}"
              title="${label}" aria-label="${label}">
            ${inner}
          </a>`;
  }).join("");
  return `
    <div class="chrome">
      <a class="skip-link" href="#content">Skip to content</a>
      <header class="topbar" role="banner">
        <a class="brand" href="#/library" id="brand-home" title="Home">StriVo</a>
        <span id="conn-status" class="conn-status" role="status" hidden
              title="Live updates connection">● reconnecting…</span>
        <a id="health-pill" class="health-pill" href="#/system" hidden
           role="status" title="System health — click for details"></a>
        <span id="rec-slot-pill" class="storage-pill" style="display:none"
              title="Active recordings / concurrent cap — click to manage"
              role="status"></span>
        <span id="route-status" class="conn-status" role="status" aria-live="polite" hidden></span>
        <span class="spacer"></span>
        <nav class="topnav" aria-label="Main navigation">${nav}</nav>
        <button id="add-channel" title="Add a channel to monitor"
                aria-label="Add channel">＋ Add</button>
        <button id="poll-now" title="Poke channel monitor (p)"
                aria-label="Trigger immediate channel poll">↻ Poll</button>
        <button id="logout" title="Logout" aria-label="Sign out">
          <img class="topnav-icon" src="/assets/icons/candy/logout.svg" alt="" />
        </button>
      </header>
      <nav class="leftrail" id="channel-list" aria-label="Channels"></nav>
      <main class="content" id="content" tabindex="-1">${content}</main>
    </div>
  `;
}

// Keep the navigation chrome alive between route paints.  Apart from making
// navigation feel immediate, this preserves the rail's scroll position and
// keyboard focus instead of recreating all of its controls after each fetch.
function mountRouteShell(context) {
  if (!isRouteCurrent(context)) return false;
  if (!root.querySelector(":scope > .chrome")) {
    root.innerHTML = chrome('<div class="route-placeholder" role="status">Loading…</div>');
    setupChromeHandlers();
  }
  root.setAttribute("aria-busy", "true");
  const status = document.getElementById("route-status");
  if (status) {
    status.textContent = `Loading ${context.route === "library" ? "Home" : context.route}…`;
    status.hidden = false;
  }
  return true;
}

function mountPage(content, context = captureRouteContext()) {
  if (!isRouteCurrent(context)) return false;
  if (!root.querySelector(":scope > .chrome")) {
    root.innerHTML = chrome(content);
    setupChromeHandlers();
  } else {
    const main = document.getElementById("content");
    if (!main) return false;
    main.innerHTML = content;
    document.querySelectorAll(".topnav-link").forEach((link) => {
      link.classList.toggle("active", link.dataset.route === context.route);
    });
    paintChannelList();
  }
  root.removeAttribute("aria-busy");
  const status = document.getElementById("route-status");
  if (status) {
    status.textContent = "";
    status.hidden = true;
  }
  return true;
}

function setupChromeHandlers() {
  const shell = root.querySelector(":scope > .chrome");
  if (!shell) return;
  if (shell.dataset.chromeWired === "1") {
    paintChannelList();
    return;
  }
  shell.dataset.chromeWired = "1";
  // Brand → home: clear any selected channel and go to the dashboard.
  document.getElementById("brand-home")?.addEventListener("click", (e) => {
    e.preventDefault();
    selectedChannelKey = null;
    if (currentRoute() === "library") render();
    else route("library");
  });
  document.getElementById("poll-now")?.addEventListener("click", async () => {
    try {
      await API.pollNow();
    } catch (e) {
      console.error(e);
    }
  });
  document.getElementById("add-channel")?.addEventListener("click", () => openAddChannelWizard());
  // Slot pill navigates to Monitor page so users can adjust the cap.
  document.getElementById("rec-slot-pill")?.addEventListener("click", () => route("schedule"));
  document.getElementById("logout")?.addEventListener("click", async () => {
    // Quick confirm — one misclick on the topbar shouldn't sign you out.
    if (!(await confirmDialog("Sign out? You'll need to re-enter the API key to come back.", { ok: "Sign out" }))) return;
    API.logout().catch(() => {}).then(() => route("login"));
  });
  // Health pill — amber/red when any check is degraded (roadmap item 13).
  refreshHealthPill();
  // Populate the concurrent-slot pill with the configured cap. Fire-and-forget;
  // failures silently leave maxConcurrentSlots at its current cached value so
  // a transient /settings error doesn't blank the topbar.
  API.settings().then((s) => {
    maxConcurrentSlots = (s && s.monitor_limits && s.monitor_limits.max_concurrent_recordings) || 0;
    updateLiveCount();
  }).catch(() => {});
  // Channel list lives in the left rail on every page.
  paintChannelList();
}

// Topbar health pill: only shown when the worst check is warn/error, so a
// healthy system stays uncluttered. Links to the System page. (Item 13.)
async function refreshHealthPill() {
  const pill = document.getElementById("health-pill");
  if (!pill) return;
  try {
    const h = await API.healthChecks();
    const worst = h.status || "ok";
    if (worst === "ok") {
      pill.hidden = true;
      return;
    }
    const bad = (h.checks || []).filter((c) => c.severity !== "ok");
    pill.className = `health-pill ${worst}`;
    pill.textContent = `${worst === "error" ? "✕" : "▲"} ${bad.length} issue${bad.length === 1 ? "" : "s"}`;
    pill.title = bad.map((c) => `${c.domain}/${c.name}: ${c.message}`).join("\n");
    pill.hidden = false;
  } catch (_) {
    pill.hidden = true;
  }
}

// ── Channel list (left rail) ─────────────────────────────────────────
// Merges /channels (Twitch/YT) with Patreon creators (patreonState),
// live first + bold, then offline. Clicking selects a channel and shows
// its detail in the center (home route only).
// Rail ordering + collapse state. Platform rank keeps Twitch → YouTube →
// Patreon stable regardless of how the daemon happens to return channels.
const RAIL_PLATFORM_ORDER = { Twitch: 0, YouTube: 1, Patreon: 2 };
function byPlatformThenName(a, b) {
  const rank =
    (RAIL_PLATFORM_ORDER[a.platform] ?? 99) - (RAIL_PLATFORM_ORDER[b.platform] ?? 99);
  if (rank !== 0) return rank;
  return (a.display_name || a.name || "").localeCompare(
    b.display_name || b.name || "",
    undefined,
    { sensitivity: "base" },
  );
}

// Rail sort. Alphabetical-within-platform is the default because it makes a
// channel findable by name; the last-live orders answer the other question
// people actually ask of this rail ("who's been on recently / who's gone
// quiet"). Channels StriVo has never seen live sort last in both directions
// rather than pretending to be infinitely old or infinitely recent.
const RAIL_SORT_KEY = "strivo:rail-sort";
const RAIL_SORTS = {
  name: { label: "Name (A–Z)", cmp: byPlatformThenName },
  "live-desc": { label: "Last live (newest)", cmp: (a, b) => byLastLive(a, b, -1) },
  "live-asc": { label: "Last live (oldest)", cmp: (a, b) => byLastLive(a, b, 1) },
  viewers: { label: "Viewers (highest)", cmp: (a, b) => (b.viewer_count || 0) - (a.viewer_count || 0) || byPlatformThenName(a, b) },
};
function byLastLive(a, b, dir) {
  const ta = a.is_live ? Infinity : Date.parse(a.last_live_at || "") || null;
  const tb = b.is_live ? Infinity : Date.parse(b.last_live_at || "") || null;
  if (ta === null && tb === null) return byPlatformThenName(a, b);
  if (ta === null) return 1; // never-seen-live sinks, whichever direction
  if (tb === null) return -1;
  if (ta === tb) return byPlatformThenName(a, b);
  return ta < tb ? dir : -dir;
}
function railSort() {
  try {
    const v = localStorage.getItem(RAIL_SORT_KEY);
    return RAIL_SORTS[v] ? v : "name";
  } catch (_) {
    return "name";
  }
}
function setRailSort(v) {
  try {
    localStorage.setItem(RAIL_SORT_KEY, RAIL_SORTS[v] ? v : "name");
  } catch (_) {
    /* private mode — sort just won't persist */
  }
}

const RAIL_COLLAPSE_KEY = "strivo:rail-collapsed";
function railCollapsedSet() {
  try {
    const raw = JSON.parse(localStorage.getItem(RAIL_COLLAPSE_KEY) || "[]");
    return new Set(Array.isArray(raw) ? raw : []);
  } catch (_) {
    return new Set();
  }
}
function railSectionOpen(id) {
  return !railCollapsedSet().has(id);
}
function setRailSectionOpen(id, open) {
  const set = railCollapsedSet();
  if (open) set.delete(id);
  else set.add(id);
  try {
    localStorage.setItem(RAIL_COLLAPSE_KEY, JSON.stringify([...set]));
  } catch (_) {
    /* private mode — collapse just won't persist */
  }
}

function railSortControlHtml() {
  const current = railSort();
  const opts = Object.entries(RAIL_SORTS)
    .map(
      ([v, { label }]) =>
        `<option value="${v}"${v === current ? " selected" : ""}>${label}</option>`,
    )
    .join("");
  return `<div class="ch-sort">
      <label class="ch-sort-label micro" for="rail-sort">Sort</label>
      <select id="rail-sort" class="ch-sort-select" data-rail-sort>${opts}</select>
      <button type="button" class="ch-clear-notifications" data-clear-all-notifications title="Clear all channel notification overrides">Clear notifications</button>
    </div>`;
}

function paintChannelList() {
  const rail = document.getElementById("channel-list");
  if (!rail) return;

  const merged = [...channelCache, ...patreonState.creators];
  // De-dupe by platform:id in case a Patreon creator is also in /channels.
  const seen = new Set();
  const channels = merged.filter((c) => {
    const k = `${c.platform}:${c.id}`;
    if (seen.has(k)) return false;
    seen.add(k);
    return true;
  });

  // One ordering for both rails: platform groups in a fixed order, then
  // A-Z inside each. Offline used to be split into a header per platform,
  // which cost three headers of vertical space and made a single alphabetical
  // scan impossible. The per-row platform glyph already says which is which.
  const cmp = (RAIL_SORTS[railSort()] || RAIL_SORTS.name).cmp;
  const live = channels.filter((c) => c.is_live).sort(cmp);
  const offline = channels.filter((c) => !c.is_live).sort(cmp);
  const monitored = channels.filter((c) => c.auto_record).sort(cmp);
  updateLiveCount();

  const recordingChannelIds = new Set(
    recCache.filter((r) => isInProgress(r.state)).map((r) => r.channel_id),
  );
  const unwatched = (c) => {
    const n = recCache.filter((r) => r.channel_id === c.id && r.watched === false && !isInProgress(r.state)).length;
    return n > 9 ? "9+" : n ? String(n) : "";
  };
  // Route commits call this to make the rail available, but recreating an
  // already-correct rail discards focus and an open section.  Repaint only
  // when its visible model actually changed.
  const railSignature = JSON.stringify({
    selectedChannelKey,
    sort: railSort(),
    channels: channels.map((c) => [c.platform, c.id, c.display_name || c.name, c.is_live, c.viewer_count, c.last_live_at, c.stream_title]),
    recordingChannelIds: [...recordingChannelIds].sort(),
  });
  if (rail.dataset.modelSignature === railSignature) return;

  const row = (c) => {
    const key = `${c.platform}:${c.id}`;
    const sel = key === selectedChannelKey ? "sel" : "";
    const isPatreon = c.platform === "Patreon";
    const rec = recordingChannelIds.has(c.id)
      ? '<span class="ch-rec" title="recording">●</span>'
      : "";
    const unseen = unwatched(c);
    // Live → viewer count; offline Twitch/YT → "last live: N ago" in the same
    // slot (when StriVo has observed it live at least once).
    let viewers = "";
    if (c.is_live && c.viewer_count) {
      viewers = `<span class="ch-viewers micro">${formatCount(c.viewer_count)}</span>`;
    } else if (!c.is_live && !isPatreon && c.last_live_at) {
      viewers = `<span class="ch-lastlive" title="last live: ${htmlEscape(lastLiveLong(c.last_live_at))}">${htmlEscape(relTime(c.last_live_at))}</span>`;
    }
    // Patreon rows are visually distinct (item 6): a pledged-tier chip
    // (stored in stream_title) and a patreon-accented platform glyph.
    const tier = isPatreon && c.stream_title
      ? `<span class="ch-tier micro" title="pledged tier">${htmlEscape(c.stream_title)}</span>`
      : "";
    // Filter Recordings + History by this channel when clicked. Live
    // channels link to the recording dashboard so you can spot the
    // active capture quickly; offline rows go straight to the filtered
    // Recordings page (audit B7/M2).
    const href = c.is_live
      ? "#/library"
      : `#/recordings?channel=${encodeURIComponent(c.display_name || c.name || "")}`;
    // Live rows expose a drag handle to the player stage. The id shape
    // here must match the backend's stream_id format (`PlatformKind:id`)
    // so dropping onto a tile resolves to a known stream.
    const liveStreamId = c.is_live ? `${c.platform}:${c.id}` : "";
    return `
      <a class="ch-row ${c.is_live ? "live" : ""} ${isPatreon ? "patreon" : ""} ${sel}"
         data-channel-key="${key}" data-channel-id="${c.id}"
         data-platform="${c.platform}" data-live-stream-id="${htmlEscape(liveStreamId)}" href="${href}">
        <span class="ch-plat micro ${c.platform.toLowerCase()}" aria-hidden="true">${platformGlyph(c.platform)}</span>
        <span class="ch-name">${htmlEscape(c.display_name || c.name)}</span>
        ${tier}${viewers}${rec}${unseen ? `<span class="ch-unwatched" title="Unwatched uploads">${unseen}</span>` : ""}
      </a>`;
  };

  // Section headers are buttons so they can be collapsed; the open/closed
  // state persists per section so a rail collapsed to just LIVE stays that
  // way across repaints and reloads.
  const section = (id, title, list) =>
    list.length
      ? `<button type="button" class="ch-section-title micro" data-rail-section="${id}"
                 aria-expanded="${railSectionOpen(id)}" aria-controls="rail-sec-${id}">
           <span class="ch-caret" aria-hidden="true">▾</span>
           <span class="ch-section-label">${title}</span>
           <span class="ch-count">${list.length}</span>
         </button>
         <div class="ch-section-body" id="rail-sec-${id}" data-rail-body="${id}"
              ${railSectionOpen(id) ? "" : "hidden"}>${list.map(row).join("")}</div>`
      : "";

  // Preserve scroll position across repaints (the rail is rebuilt
  // wholesale, which would otherwise jump it to the top on every event).
  const prevScroll = rail.scrollTop;
  rail.innerHTML =
    channels.length === 0
      ? `<div class="ch-empty">No channels yet.<br><br>
           Connect Twitch / YouTube / Patreon and follow channels — they
           appear here automatically.<br>
           <a href="#/settings">Check Settings →</a></div>`
      : railSortControlHtml() +
        section("monitored", "MONITORED", monitored) +
        section("live", `● LIVE`, live) +
        section("offline", "OFFLINE", offline);
  rail.dataset.modelSignature = railSignature;

  rail.querySelectorAll(".ch-row").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.preventDefault();
      selectChannel(el.dataset.channelKey, e);
    });
  });
  const sortSel = rail.querySelector("[data-rail-sort]");
  if (sortSel) {
    sortSel.addEventListener("change", () => {
      setRailSort(sortSel.value);
      paintChannelList();
    });
  }
  rail.querySelector("[data-clear-all-notifications]")?.addEventListener("click", async () => {
    if (!(await confirmDialog("Clear notification settings for every channel?", { ok: "Clear", danger: true }))) return;
    await Promise.all(channels.map((c) => API.setChannelAlerts(`${c.platform}:${c.id}`, { on_live: null, on_upload: null })));
    paintChannelList();
  });
  rail.querySelectorAll("[data-rail-section]").forEach((el) => {
    el.addEventListener("click", () => {
      const id = el.dataset.railSection;
      const open = !railSectionOpen(id);
      setRailSectionOpen(id, open);
      el.setAttribute("aria-expanded", String(open));
      const body = rail.querySelector(`[data-rail-body="${id}"]`);
      if (body) body.hidden = !open;
    });
  });
  rail.scrollTop = prevScroll;
}

function platformGlyph(p) {
  return p === "Twitch" ? "🟣" : p === "YouTube" ? "🔴" : "◈";
}

// Seed patreonState from the daemon snapshot (/patreon) so Patreon shows
// immediately on load, instead of only after the next ~5-min poll's
// patreon-state SSE event. Idempotent; refreshed live by SSE thereafter.
async function seedPatreon(context = captureRouteContext()) {
  try {
    const p = await API.patreon();
    if (!isRouteCurrent(context)) return false;
    patreonState.creators = p.creators || [];
    patreonState.posts = {};
    for (const post of p.posts || []) {
      (patreonState.posts[post.campaign_id] ||= []).push(post);
    }
    for (const list of Object.values(patreonState.posts)) {
      list.sort((a, b) => (b.published_at || "").localeCompare(a.published_at || ""));
    }
    return true;
  } catch (_) {
    /* non-fatal — SSE still refreshes it */
    return false;
  }
}

// Per-route rail-click overrides. A route registers `RAIL_CLICK_HANDLERS.<route>
// = (channelKey, event) => handled` to claim rail clicks while it is active
// (e.g. the player loads the channel into a tile instead of leaving the page).
// Returning false falls through to the default: open channel detail.
const RAIL_CLICK_HANDLERS = {};
// Extra e2e hooks contributed by later modules; spread into
// window.__strivoTestHooks when the page opts in (see 036-pvr.js).
const TEST_HOOK_EXTENSIONS = {};

function selectChannel(key, ev) {
  const override = RAIL_CLICK_HANDLERS[currentRoute()];
  if (override && override(key, ev)) return;
  selectedChannelKey = key;
  if (currentRoute() !== "library") {
    route("library"); // hashchange triggers render()
  } else {
    render();
  }
}
