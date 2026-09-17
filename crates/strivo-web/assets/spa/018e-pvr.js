// ── Aspect-aware repacking ────────────────────────────────────────────
//
// A preset's FIXED cols x rows shape (PRESET_GRID_SHAPE) is the right
// call most of the time, but it can waste more than half the stage: a
// quadrant's 2x2 in a short, wide box (chat rail open) locks every tile
// to a 16:9 slice of a square-ish aspect that doesn't fit the box at
// all. When that happens, every OTHER exact factorisation of the same
// leaf count is tried and whichever covers the most actual video area
// wins — for 2 tiles on a wide-short stage that's 2x1 (side by side),
// for 4 it can mean 4x1 instead of 2x2, depending on the box.

/// Total on-screen video area a cols x rows grid of 16:9 tiles covers
/// inside a WxH box — same fit math as bestGridFor, just for one
/// candidate shape rather than searching for the best (cols, rows).
function gridFitArea(shape, W, H) {
  const tileW = W / shape.cols;
  const tileH = H / shape.rows;
  const fitW = Math.min(tileW, tileH * (16 / 9));
  const fitH = fitW * (9 / 16);
  return fitW * fitH * shape.cols * shape.rows;
}

/// Every (cols, rows) pair that exactly tiles n leaves — the only
/// shapes worth considering, since a preset's leaf count is fixed.
function candidateGridShapes(n) {
  const out = [];
  for (let rows = 1; rows <= n; rows++) {
    if (n % rows === 0) out.push({ cols: n / rows, rows });
  }
  return out;
}

/// The shape to actually render a grid-regular preset with: `base`
/// unless it covers under 70% of the WxH box AND a same-leaf-count
/// alternative covers more.
function bestPackedGridShape(n, W, H, base) {
  if (!base || !(W > 0) || !(H > 0)) return base;
  const baseArea = gridFitArea(base, W, H);
  if (baseArea / (W * H) >= 0.7) return base;
  let best = base;
  let bestArea = baseArea;
  for (const cand of candidateGridShapes(n)) {
    const area = gridFitArea(cand, W, H);
    if (area > bestArea) {
      best = cand;
      bestArea = area;
    }
  }
  return best;
}

function getNodeAt(layout, path) {
  let n = layout;
  for (const step of pathParts(path)) n = n[step];
  return n;
}

/// Safe existence check — never throws on a stale/garbage path string.
function nodeExistsAt(layout, path) {
  try {
    let n = layout;
    for (const step of pathParts(path)) {
      if (!n || typeof n !== "object") return false;
      n = n[step];
    }
    return !!n && n.kind === "slot";
  } catch (_) {
    return false;
  }
}

// Replace the node at path with newNode (immutably-ish — we structuredClone
// the root and patch). Returns the new root.
function setNodeAt(layout, path, newNode) {
  const root = structuredClone(layout);
  if (!path) return newNode;
  const parts = pathParts(path);
  let parent = root;
  for (let i = 0; i < parts.length - 1; i++) parent = parent[parts[i]];
  parent[parts[parts.length - 1]] = newNode;
  return root;
}

function savePlayerLayout() {
  try {
    localStorage.setItem(PLAYER_LAYOUT_KEY, JSON.stringify(playerState.layout));
    localStorage.setItem(PLAYER_PRESET_KEY, playerState.preset);
    // Focus (audible tile) is a viewer choice worth surviving a reload —
    // losing it meant every refresh silently fell back to mute-all.
    localStorage.setItem(PLAYER_SOLO_KEY, playerState.soloPath || "");
  } catch (_) {}
}

// Validate a parsed layout tree. Rejects null/undefined, unknown kinds,
// out-of-range ratios, malformed children. Recursive — every node has
// to pass on its own merits.
function validatePlayerLayout(node) {
  if (!node || typeof node !== "object") return false;
  if (node.kind === "slot") {
    // streamId / recordingId either null/undefined or non-empty string.
    if (node.streamId !== null && node.streamId !== undefined && typeof node.streamId !== "string") return false;
    if (node.recordingId !== null && node.recordingId !== undefined && typeof node.recordingId !== "string") return false;
    return true;
  }
  if (node.kind === "split") {
    if (node.dir !== "h" && node.dir !== "v") return false;
    if (typeof node.ratio !== "number" || node.ratio <= 0 || node.ratio >= 1) return false;
    return validatePlayerLayout(node.a) && validatePlayerLayout(node.b);
  }
  return false;
}

function loadPlayerLayout() {
  try {
    const raw = localStorage.getItem(PLAYER_LAYOUT_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      if (validatePlayerLayout(parsed)) {
        playerState.layout = parsed;
        playerState.preset = localStorage.getItem(PLAYER_PRESET_KEY) || "custom";
        // Only restore focus onto a path that still resolves to a real
        // leaf in THIS layout — a stale solo path from a previous, larger
        // layout must not silently point at nothing.
        const savedSolo = localStorage.getItem(PLAYER_SOLO_KEY) || "";
        playerState.soloPath = savedSolo && nodeExistsAt(playerState.layout, savedSolo) ? savedSolo : "";
        return;
      }
    }
  } catch (_) {}
  // Anything invalid drops back to a single empty slot. The user is
  // never silently stuck with a corrupted layout.
  playerState.layout = PLAYER_PRESETS.single();
  playerState.preset = "single";
  playerState.soloPath = "";
}

async function renderWatch(ctx) {
  // Honour URL params from rail / dashboard clicks:
  //   ?focus=<streamId>      → load that LIVE stream into the (empty) single slot
  //   ?recording=<recId>     → load that RECORDING into the (empty) single slot
  // 'fresh=1' forces a single-slot reset before loading so the user
  // doesn't end up dropping a click target into a stale multi-tile layout.
  const params = new URLSearchParams(window.location.hash.split("?")[1] || "");
  const focusId = params.get("focus") || "";
  const recordingId = params.get("recording") || "";
  const fresh = params.get("fresh") === "1";
  const seekTo = parseFloat(params.get("t") || "0") || 0;

  // Back-compat: every recording-open path used to land here
  // (`#/watch?recording=<id>&fresh=1`) before the dedicated player route
  // (019b, `#/play`) existed — old bookmarks and any stray external link
  // still use that shape. Redirect rather than build a wall around one
  // recording, mirroring the `#/viewer` redirect above (018:7-24).
  // Recording slots added through the composer or drag-and-drop never
  // set `fresh=1` and are unaffected — they still land in the wall.
  if (recordingId && fresh) {
    const t = params.get("t");
    location.replace(`#/play?recording=${encodeURIComponent(recordingId)}${t ? `&t=${encodeURIComponent(t)}` : ""}`);
    return;
  }

  // teardownAcrossRoutes() in render() already cleared any prior refresh
  // poll (playerState.refreshTimer + the legacy _watchRefreshTimer
  // alias). A12 consolidation — don't duplicate the clear here.
  if (!playerState.layout) loadPlayerLayout();
  // Collapses the left channel rail to an icon strip on this route (019a)
  // — a 292px rail plus a 340px open chat rail otherwise leaves a
  // 16:9-locked wall floating in a fraction of the viewport.
  enterWatchRoute();
  // B12: explicit null/undefined check — a serialised slot with
  // streamId: "" should still be treated as empty here.
  const slotIsEmpty = playerState.layout.kind === "slot"
    && (playerState.layout.streamId == null || playerState.layout.streamId === "")
    && (playerState.layout.recordingId == null || playerState.layout.recordingId === "");
  if (focusId || recordingId) {
    const target = recordingId ? _slot(null, recordingId) : _slot(focusId);
    if (fresh || slotIsEmpty || playerState.layout.kind === "slot") {
      // Single-tile layout (or an explicit reset): the request owns the wall.
      // This used to require the slot be EMPTY, so clicking a live channel
      // while the last thing you watched was still loaded silently did
      // nothing and left you staring at that old recording.
      playerState.preset = "single";
      playerState.layout = target;
    } else {
      // Multi-tile: honour the click without destroying the wall — fill the
      // first empty tile, else replace the first one.
      const dest = firstEmptyPath(playerState.layout) ?? firstLeafPath(playerState.layout);
      if (dest !== null && dest !== undefined) {
        playerState.layout = setNodeAt(playerState.layout, dest, target);
      }
    }
    // An explicit "watch this" click is a request to watch it, so it starts
    // playing even though the wall opens paused by default — unless it's a
    // recording that's still being written (B-04): renderPopulatedSlotHtml
    // already refuses to source /download for that state, so forcing
    // "playing" here would just be requesting playback of a tile that
    // renders as the "still recording" affordance instead.
    const recStillRecording = recordingId
      && recCache.some((r) => r.id === recordingId && isInProgress(r.state));
    if (!recStillRecording) setTilePlaying(target, true);
    savePlayerLayout();
  }

  // Channel + recording caches are guaranteed hydrated by render()'s
  // ensureRouteHydration() before this runs.
  // The watch root holds a `.watch-content` (toolbar + stage) and an
  // `<aside class="player-chat-rail">` that follows the focused tile.
  // The rail can be collapsed via the toggle; its open/closed state
  // persists in localStorage.
  const railOpen = playerState.chatRailOpen ? "true" : "false";
  const railToggleGlyph = playerState.chatRailOpen ? "▶" : "◀";
  const railTitle = playerState.chatRailOpen ? "Collapse chat rail" : "Open chat rail";
  const theaterClass = countLeaves(playerState.layout) === 1 ? "is-theater" : "";
  // The rail starts hidden and without `has-chat-rail` regardless of the
  // persisted open/closed preference — reconcilePlayerChatRail (018d) is
  // what decides whether a live, chat-capable tile exists at all and
  // reveals the rail accordingly, right after this markup mounts. Baking
  // the preference in here would flash the rail open for one frame on a
  // recordings-only wall before reconcile hides it again.
  if (!mountPage(`
    <div id="watch" class="watch-root ${theaterClass}" role="main">
      <div class="watch-content"><div class="empty">Loading…</div></div>
      <aside class="player-chat-rail" id="player-chat-rail" data-open="${railOpen}" hidden>
        <div class="player-chat-rail-head">
          <button class="player-chat-rail-toggle sm" id="player-chat-rail-toggle"
                  type="button" title="${railTitle}" aria-pressed="${railOpen}">${railToggleGlyph}</button>
          <span class="player-chat-rail-title">Chat</span>
          <span class="player-chat-rail-room"></span>
        </div>
        <div class="player-chat-rail-tabs" role="tablist"
             title="Tap a stream to switch chats"></div>
        <div class="player-chat-rail-body" role="log" aria-live="polite"></div>
        <div class="player-chat-rail-compose" id="player-chat-rail-compose"></div>
      </aside>
    </div>
  `, ctx)) return;
  const watch = document.getElementById("watch");
  const watchContent = watch.querySelector(".watch-content");
  // Router hydration is intentionally non-blocking. Start the picker data at
  // the same time as the tile request, then reconcile once both are ready so
  // a direct watch entry never mounts an empty, permanent picker.
  const watchData = Promise.all([
    hydrationLoaded.channels ? Promise.resolve(channelCache) : API.channels().then((response) => {
      const channels = response.channels || [];
      if (isRouteCurrent(ctx)) { channelCache = channels; hydrationLoaded.channels = true; }
      return channels;
    }),
    hydrationLoaded.recordings ? Promise.resolve(recCache) : API.recordings().then((response) => {
      const recordings = response.recordings || [];
      if (isRouteCurrent(ctx)) { recCache = recordings; hydrationLoaded.recordings = true; }
      return recordings;
    }),
  ]);

  // Rail toggle — flip persisted state, sync class + glyph, reconcile.
  document.getElementById("player-chat-rail-toggle")?.addEventListener("click", () => {
    playerState.chatRailOpen = !playerState.chatRailOpen;
    savePlayerChatRailOpen();
    watch.classList.toggle("has-chat-rail", playerState.chatRailOpen);
    const rail = document.getElementById("player-chat-rail");
    if (rail) rail.dataset.open = playerState.chatRailOpen ? "true" : "false";
    const btn = document.getElementById("player-chat-rail-toggle");
    if (btn) {
      btn.textContent = playerState.chatRailOpen ? "▶" : "◀";
      btn.title = playerState.chatRailOpen ? "Collapse chat rail" : "Open chat rail";
      btn.setAttribute("aria-pressed", playerState.chatRailOpen ? "true" : "false");
    }
    reconcilePlayerChatRail(playerState.chatRailLastStreams);
  });

  let resp;
  try {
    // Backend still drives 'which live streams are present + embed URLs';
    // we ignore its tile geometry and lay things out via the layout tree.
    resp = await API.multistreamTiles(stageGeometry().w, stageGeometry().h, { mode: "auto" }, window.location.host);
  } catch (e) {
    if (typeof isRouteCurrent === "function" && !isRouteCurrent(ctx)) return;
    watchContent.innerHTML = `<div class="empty"><div class="glyph">⚠</div>${htmlEscape(e.message)}</div>`;
    return;
  }
  if (typeof isRouteCurrent === "function" && !isRouteCurrent(ctx)) return;
  try {
    const [channels, recordings] = await watchData;
    if (typeof isRouteCurrent === "function" && !isRouteCurrent(ctx)) return;
    channelCache = channels;
    recCache = recordings;
  } catch (_) {
    // Existing cache contents are still useful when this refresh fails.
  }
  if (typeof isRouteCurrent === "function" && !isRouteCurrent(ctx)) return;
  // `let`, not `const`: the background refresh below updates this baseline
  // in place when the live-set changes, so a one-off go-live/offline doesn't
  // make every subsequent tick compare against a stale set.
  let streams = resp.streams || [];
  playerState.chatRailLastStreams = streams;
  paintPlayerStage(watchContent, streams);
  reconcilePlayerChatRail(streams);
  // Apply ?t=<sec> from in-context tools (Crunchr transcript jump,
  // cuepoints tick, EDL jumps) via the controller interface — works for
  // any future seekable controller, not just the recording <video>.
  if (seekTo > 0) {
    const key = contentKeyOf(getNodeAt(playerState.layout, ""));
    const ctl = key && playerState.controllers.get(key);
    if (ctl && typeof ctl.onState === "function") {
      const unsub = ctl.onState((s) => {
        if (s.ready) {
          try { ctl.seek(seekTo); ctl.play && ctl.play(); } catch (_) { /* advisory */ }
          unsub();
        }
      });
    }
  }

  // Background refresh: poll the tiles endpoint every 30s and patch the
  // per-tile viewer counts in place. Avoids tearing the iframes (the
  // streams keep playing) but keeps the meta-line fresh.
  playerState.refreshTimer = setInterval(async () => {
    if (document.hidden) return;
    try {
      const r = await API.multistreamTiles(stageGeometry().w, stageGeometry().h, { mode: "auto" }, window.location.host);
      if (typeof isRouteCurrent === "function" && !isRouteCurrent(ctx)) return;
      const byId = new Map((r.streams || []).map((s) => [s.stream_id, s]));
      const have = new Set(streams.map((s) => s.stream_id));
      const got = new Set([...byId.keys()]);
      const sameSet = have.size === got.size && [...have].every((x) => got.has(x));
      if (!sameSet) {
        // Live-set changed (a followed channel went live/offline — routine
        // with a dozen follows). Repaint via paintPlayerStage, NOT
        // renderWatch: paintPlayerStage's fast path patches only the slots
        // whose contents actually changed and re-attaches the still-playing
        // iframes, so the rail's stream-picker refreshes without reloading
        // the open stream. It also rebuilds the rail itself.
        //
        // Calling renderWatch here was the reload bug: it did a full
        // root.innerHTML reset (remounting every iframe) AND armed a fresh
        // 30s interval without clearing this one. Because each interval's
        // `streams` baseline was frozen, a single set change made it mismatch
        // forever — so it pumped a renderWatch every tick, the intervals
        // multiplied, and the player reloaded on a ~30-90s beat.
        //
        // Update the baseline so the next tick compares against reality.
        streams = r.streams || [];
        playerState.chatRailLastStreams = streams;
        paintPlayerStage(watchContent, streams);
        return;
      }
      // Same set — patch viewer counts in place.
      watchContent.querySelectorAll(".ms-leaf").forEach((tile) => {
        const s = byId.get(tile.dataset.streamId);
        if (!s) return;
        const meta = tile.querySelector('[data-watch-meta="viewers"]');
        if (meta && s.viewer_count != null) meta.textContent = formatCount(s.viewer_count);
      });
    } catch (_) {}
  }, 30000);
  // A12: keep the legacy _watchRefreshTimer in sync so any external
  // teardown (test harness, browser extension) that knew about the
  // old name still works during the deprecation window.
  _watchRefreshTimer = playerState.refreshTimer;
}

// ── Player stage rendering + interactions ────────────────────────────
//
// paintPlayerStage walks the layout tree, emits HTML, then wires every
// interaction (preset menu, split buttons, gutter drag, slot stream
// picker, click-to-swap drag-drop, fullscreen, solo).

// True iff two layouts have an identical tree shape (kind + split dir at
// every node). Slot contents (streamId / recordingId) may differ; that
// is the patchable subset. Anything else (split→slot, dir flip, etc.)
// triggers a full repaint.
function sameLayoutShape(a, b) {
  if (!a || !b) return false;
  if (a.kind !== b.kind) return false;
  if (a.kind === "split") {
    return a.dir === b.dir && sameLayoutShape(a.a, b.a) && sameLayoutShape(a.b, b.b);
  }
  return true; // both slots — shape matches regardless of content
}

