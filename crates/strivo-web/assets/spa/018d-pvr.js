// ── Player layout tree (multi-view collapsed into the player) ────────
//
// The viewing stage is a recursive layout tree. Two node kinds:
//   slot:  { kind: "slot", streamId: string|null }
//   split: { kind: "split", dir: "h"|"v", ratio: 0..1, a: node, b: node }
//
// 'h' splits stack left|right, 'v' splits stack top|bottom. The split
// ratio governs how much room the 'a' child gets. Presets always
// create EMPTY slots (per user request) — picking a preset never
// auto-populates streams.
//
// Cap at 9 leaves keeps the iframe count reasonable; beyond that the
// browser starts paging and Twitch/YT rate-limit your IP.

const PLAYER_LEAF_CAP = 9;
const PLAYER_LAYOUT_KEY = "strivo-player-layout";
const PLAYER_PRESET_KEY = "strivo-player-preset";
const PLAYER_SOLO_KEY = "strivo-player-solo";

// A slot can hold ONE of (or neither):
//   streamId      — live channel (rendered as a platform embed iframe)
//   recordingId   — finished recording (rendered as a <video> sourced
//                   from /api/v1/recordings/<id>/download — the /file
//                   route is DELETE-only)
function _slot(streamId = null, recordingId = null) { return { kind: "slot", streamId, recordingId }; }
function _split(dir, ratio, a, b) { return { kind: "split", dir, ratio, a, b }; }
// Row builders for the grid presets — kept as helpers so the trees read as
// rows rather than as nested split soup.
function _row2() { return _split("h", 0.5, _slot(), _slot()); }
function _row3() { return _split("h", 1 / 3, _slot(), _split("h", 0.5, _slot(), _slot())); }

/// Nested split tree for an EXACT cols x rows grid of empty slots, in
/// reading order — generalises _row2/_row3 above to any grid shape.
/// (Reproduces PLAYER_PRESETS' own split-screen/quadrant/grid-6/grid-9
/// trees exactly for (2,1)/(2,2)/(2,3)/(3,3).) Used by the aspect-aware
/// repacking below to rebuild a grid-regular preset's tree for whichever
/// (cols, rows) actually fits the stage box.
function buildGridTree(cols, rows) {
  const buildRow = (n) => (n <= 1 ? _slot() : _split("h", 1 / n, _slot(), buildRow(n - 1)));
  const buildCol = (n) => (n <= 1 ? buildRow(cols) : _split("v", 1 / n, buildRow(cols), buildCol(n - 1)));
  return buildCol(rows);
}

/// Structural signature (dir sequence only — no content, no ratio) of a
/// layout tree. Two trees with the same signature have the same shape;
/// used to check whether a layout already matches a candidate grid shape
/// without a full deep-equality helper.
function gridSignature(node) {
  return node.kind === "slot" ? "s" : `(${node.dir}:${gridSignature(node.a)},${gridSignature(node.b)})`;
}

/// Rebuild `layout` as an exact cols x rows grid, carrying every
/// populated slot's content across in reading order — same rule the
/// preset-switch handler uses — and re-pointing soloPath at wherever its
/// source landed, if it's still on the wall.
function repackLayoutToGrid(layout, cols, rows) {
  const preserved = [];
  const prevSoloPath = playerState.soloPath || "";
  walkLayout(layout, (n, path) => {
    if (n.kind === "slot" && (n.streamId || n.recordingId)) {
      preserved.push({ streamId: n.streamId || null, recordingId: n.recordingId || null, wasSoloed: path === prevSoloPath });
    }
  });
  let next = buildGridTree(cols, rows);
  const slotPaths = [];
  walkLayout(next, (n, path) => { if (n.kind === "slot") slotPaths.push(path); });
  let newSoloPath = "";
  for (let i = 0; i < Math.min(preserved.length, slotPaths.length); i++) {
    next = setNodeAt(next, slotPaths[i], _slot(preserved[i].streamId, preserved[i].recordingId));
    if (preserved[i].wasSoloed) newSoloPath = slotPaths[i];
  }
  if (newSoloPath) playerState.soloPath = newSoloPath;
  else if (prevSoloPath) playerState.soloPath = "";
  return next;
}

// One drag-payload codec, shared by the stage, the rail and the composer.
// encodeDragPayload builds the wire string; decodeDragPayload validates
// shape + ID grammar so a stray browser URL drag (or a corrupted/old
// payload) can't slip into the layout as a real stream. Grammar is
// unchanged from the previous ad-hoc parser: `strivo-tile:<path>`,
// `strivo-stream:<id>`, `strivo-recording:<id>`. decode returns:
//   { type: "tile",      path: string }
//   { type: "stream",    id:   string }
//   { type: "recording", id:   string }
// or null when the payload doesn't match any known schema.
function encodeDragPayload(payload) {
  if (!payload || typeof payload !== "object") return "";
  if (payload.type === "tile") return `strivo-tile:${payload.path ?? ""}`;
  if (payload.type === "stream") return `strivo-stream:${payload.id ?? ""}`;
  if (payload.type === "recording") return `strivo-recording:${payload.id ?? ""}`;
  return "";
}
function decodeDragPayload(text) {
  if (typeof text !== "string" || !text) return null;
  // Path can be empty (root); slug chars only beyond that.
  if (text.startsWith("strivo-tile:")) {
    const path = text.slice("strivo-tile:".length);
    if (path !== "" && !/^[ab](\.[ab])*$/.test(path)) return null;
    return { type: "tile", path };
  }
  if (text.startsWith("strivo-stream:")) {
    const id = text.slice("strivo-stream:".length);
    // Backend stream-id shape: 'PlatformDebug:Id' — alphanumeric +
    // colons/dashes/underscores. Reject anything wilder.
    if (!/^[A-Za-z0-9:_-]+$/.test(id)) return null;
    return { type: "stream", id };
  }
  if (text.startsWith("strivo-recording:")) {
    const id = text.slice("strivo-recording:".length);
    if (!/^[A-Za-z0-9_-]+$/.test(id)) return null;
    return { type: "recording", id };
  }
  return null;
}

const PLAYER_PRESETS = {
  single: () => _slot(),
  "split-screen": () => _split("h", 0.5, _slot(), _slot()),
  // Split / quadrant = 3 streams: left half single, right split top/bottom.
  "split-quadrant": () => _split("h", 0.5, _slot(), _split("v", 0.5, _slot(), _slot())),
  // 2×2 grid.
  quadrant: () => _split("v", 0.5,
    _split("h", 0.5, _slot(), _slot()),
    _split("h", 0.5, _slot(), _slot()),
  ),
  // Grids are built rows-first so a depth-first walk visits tiles in
  // reading order — that walk is what "fill the next empty tile" uses, and
  // a column-major tree makes clicked sources land in a scattered order.
  // 2 columns x 3 rows.
  "grid-6": () => _split("v", 1 / 3,
    _row2(),
    _split("v", 0.5, _row2(), _row2()),
  ),
  // 3x3 wall — 9 leaves, exactly the cap countLeaves enforces.
  "grid-9": () => _split("v", 1 / 3,
    _row3(),
    _split("v", 0.5, _row3(), _row3()),
  ),
  // One large focus tile with a stacked sidebar of three.
  "focus-3": () => _split("h", 0.68,
    _slot(),
    _split("v", 1 / 3, _slot(), _split("v", 0.5, _slot(), _slot())),
  ),
  custom: () => _slot(),
};

const PLAYER_PRESET_LABELS = {
  single: "Single",
  "split-screen": "Split-screen (2)",
  "split-quadrant": "Split / Quadrant (3)",
  quadrant: "Quadrant (4)",
  "focus-3": "Focus + 3",
  "grid-6": "Grid (6)",
  "grid-9": "Wall (9)",
  custom: "Custom",
};

const PLAYER_CHAT_RAIL_KEY = "strivo-player-chat-rail-open";
function loadPlayerChatRailOpen() {
  try { return localStorage.getItem(PLAYER_CHAT_RAIL_KEY) === "1"; } catch (_) { return false; }
}
function savePlayerChatRailOpen() {
  try { localStorage.setItem(PLAYER_CHAT_RAIL_KEY, playerState.chatRailOpen ? "1" : "0"); } catch (_) {}
}

const playerState = {
  layout: null,        // root layout node
  preset: "single",    // last-applied preset name (for the toolbar label)
  soloPath: "",        // path to soloed (audible) slot — "" = mute-all
  refreshTimer: null,
  resizeFx: null,      // gutter drag state
  chatRailOpen: loadPlayerChatRailOpen(),  // right-rail chat persistence
  chatRailRoom: null,  // currently-rendered room (Twitch login) on the rail
  chatRailMount: null, // teardown handle from mountChatRail
  chatRailLastStreams: [], // last streams[] cache for toggle re-reconciliation
  chatRailCompose: null,   // mountChatCompose controller for the rail
  lastPaintedLayout: null, // snapshot used by tryPatchPlayerStage to diff
  autoplay: loadPlayerAutoplay(), // wall-wide default; false = open paused
  playing: [],         // paths the viewer explicitly started while paused
  // contentKey -> PlayerController. Outlives every repaint; see the
  // "Player controllers" block for why ownership is not in the DOM.
  controllers: new Map(),
  volumes: loadTileVolumes(), // contentKey -> 0..1; muted is volume === 0
};

// Path strings are dot-joined sequences of "a"/"b" descending the tree.
// Root = "". Example: "a.b" → root.a.b.
function pathParts(path) { return path ? path.split(".") : []; }
function pathStr(parts) { return parts.join("."); }

// Walk a layout. Calls cb(node, path) for every node (depth-first).
function walkLayout(layout, cb, path = "") {
  cb(layout, path);
  if (layout.kind === "split") {
    walkLayout(layout.a, cb, path ? `${path}.a` : "a");
    walkLayout(layout.b, cb, path ? `${path}.b` : "b");
  }
}

function countLeaves(layout) {
  let n = 0;
  walkLayout(layout, (node) => { if (node.kind === "slot") n++; });
  return n;
}

// Collect every live-Twitch tile in layout order. The chat rail uses
// this to render a PFP/letter-avatar strip the user can tap to swap
// chat rooms. YouTube/Patreon are skipped — only Twitch chat is wired
// end-to-end on the SPA today.
function collectChatableStreams(streams) {
  if (!streams || streams.length === 0 || !playerState.layout) return [];
  const seen = new Set();
  const out = [];
  walkLayout(playerState.layout, (n, path) => {
    if (n.kind !== "slot" || !n.streamId) return;
    if (seen.has(n.streamId)) return;
    const s = streams.find((x) => x.stream_id === n.streamId);
    // Only Twitch chat is wired end-to-end on the SPA. The backend
    // returns the platform tag in lowercase; the rest of the SPA uses
    // the canonical capitalised form. Accept both.
    if (!s) return;
    const plat = (s.platform || "").toLowerCase();
    if (plat !== "twitch") return;
    seen.add(n.streamId);
    out.push({ stream: s, path });
  });
  return out;
}

// Default stream for the rail when the user hasn't explicitly picked
// one (or the previously-picked stream dropped off): solo wins, else
// first chatable tile in layout order.
function derivePlayerChatDefault(streams) {
  const chatable = collectChatableStreams(streams);
  if (chatable.length === 0) return null;
  const soloPath = playerState.soloPath || "";
  if (soloPath) {
    const soloed = chatable.find((c) => c.path === soloPath);
    if (soloed) return soloed.stream;
  }
  return chatable[0].stream;
}

// Reconcile the chat rail against the current layout + streams. Idempotent;
// safe to call after every player paint. Honours the user's explicit room
// pick when it's still in the layout, else falls back to the focus default.
function reconcilePlayerChatRail(streams) {
  const rail = document.getElementById("player-chat-rail");
  if (!rail) return;
  const body = rail.querySelector(".player-chat-rail-body");
  const tabs = rail.querySelector(".player-chat-rail-tabs");
  const roomLabel = rail.querySelector(".player-chat-rail-room");
  if (!body || !tabs) return;
  const watch = document.getElementById("watch");

  const chatable = collectChatableStreams(streams);
  if (chatable.length === 0) {
    // No live, chat-capable tile anywhere in the layout (a recordings-only
    // wall, or an empty one) — hide the whole rail rather than rendering
    // an empty affordance next to a stage that could use the width. The
    // persisted open/closed preference (playerState.chatRailOpen) is left
    // untouched, so a live Twitch tile added later restores exactly what
    // the viewer had before.
    rail.hidden = true;
    watch?.classList.remove("has-chat-rail");
    watch?.classList.add("no-chat-target");
    if (playerState.chatRailMount) {
      try { playerState.chatRailMount.teardown(); } catch (_) {}
      playerState.chatRailMount = null;
    }
    if (playerState.chatRailCompose) {
      try { playerState.chatRailCompose.teardown(); } catch (_) {}
      playerState.chatRailCompose = null;
    }
    playerState.chatRailRoom = null;
    tabs.innerHTML = "";
    body.innerHTML = "";
    if (roomLabel) roomLabel.textContent = "";
    return;
  }
  rail.hidden = false;
  watch?.classList.remove("no-chat-target");
  watch?.classList.toggle("has-chat-rail", playerState.chatRailOpen);

  if (!playerState.chatRailOpen) {
    if (playerState.chatRailMount) {
      try { playerState.chatRailMount.teardown(); } catch (_) {}
      playerState.chatRailMount = null;
    }
    if (playerState.chatRailCompose) {
      try { playerState.chatRailCompose.teardown(); } catch (_) {}
      playerState.chatRailCompose = null;
    }
    playerState.chatRailRoom = null;
    tabs.innerHTML = "";
    body.innerHTML = "";
    if (roomLabel) roomLabel.textContent = "";
    return;
  }

  // PFP/letter-avatar strip. Click to override follow-focus and switch
  // the rail to that channel's chat.
  tabs.innerHTML = chatable.map(({ stream: s }) => {
    const room = (s.channel_name || "").toLowerCase();
    const display = s.channel_name || room;
    const hue = chatAvatarHue(display);
    const active = room === playerState.chatRailRoom ? " active" : "";
    return `
      <button class="player-chat-rail-tab${active}" type="button"
              data-room="${htmlEscape(room)}"
              data-stream-id="${htmlEscape(s.stream_id)}"
              title="Chat: ${htmlEscape(display)}">
        <span class="player-chat-rail-avatar"
              style="background:hsl(${hue} 55% 32%);">${htmlEscape(display.slice(0, 1).toUpperCase())}</span>
      </button>`;
  }).join("");
  tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      const room = btn.dataset.room || "";
      if (!room || room === playerState.chatRailRoom) return;
      playerState.chatRailRoom = room;
      reconcilePlayerChatRail(playerState.chatRailLastStreams);
    });
  });

  // Resolve target room: respect explicit pick if still valid; else
  // fall back to the default (solo / first leaf).
  const validRooms = new Set(chatable.map((c) => (c.stream.channel_name || "").toLowerCase()));
  let targetRoom = playerState.chatRailRoom;
  if (!targetRoom || !validRooms.has(targetRoom)) {
    const def = derivePlayerChatDefault(streams);
    targetRoom = def ? (def.channel_name || "").toLowerCase() : null;
  }

  // Resolve the platform of the target room — drives the compose-bar
  // per-platform accent. chat-rooms metadata may report `youtube`
  // (rejected for chat today, connectable:false), so we coerce here.
  const targetMeta = (chatState.rooms || []).find((r) => r.room === targetRoom);
  const targetPlatform = (targetMeta?.platform || "twitch").toLowerCase();

  if (targetRoom === (playerState.chatRailMount ? playerState.chatRailRoom : null) && targetRoom) {
    // Already mounted to the right room — just sync UI bits.
    playerState.chatRailRoom = targetRoom;
    if (roomLabel) roomLabel.textContent = `#${targetRoom}`;
    tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
      btn.classList.toggle("active", btn.dataset.room === targetRoom);
    });
    if (playerState.chatRailCompose) {
      playerState.chatRailCompose.setRoom(targetRoom, targetPlatform);
    }
    return;
  }

  // Different room (or first mount, or no room): drop old, mount new.
  if (playerState.chatRailMount) {
    try { playerState.chatRailMount.teardown(); } catch (_) {}
    playerState.chatRailMount = null;
  }
  playerState.chatRailRoom = targetRoom;
  if (!targetRoom) {
    body.innerHTML = `<div class="player-chat-rail-empty pg-cap-hint">No Twitch stream in the layout.</div>`;
    if (roomLabel) roomLabel.textContent = "—";
    // Tear down compose too — nothing to send to.
    if (playerState.chatRailCompose) {
      try { playerState.chatRailCompose.teardown(); } catch (_) {}
      playerState.chatRailCompose = null;
    }
    return;
  }
  if (roomLabel) roomLabel.textContent = `#${targetRoom}`;
  tabs.querySelectorAll(".player-chat-rail-tab").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.room === targetRoom);
  });

  // Ensure the compose bar exists — mount once per rail-open session,
  // then call setRoom on each subsequent target change.
  const composeHost = rail.querySelector("#player-chat-rail-compose");
  if (composeHost && !playerState.chatRailCompose) {
    composeHost.innerHTML = "";
    playerState.chatRailCompose = mountChatCompose(composeHost, {
      room: targetRoom,
      platform: targetPlatform,
      compact: true,
    });
  }

  // Mount the message body for the target room.
  const proceed = () => {
    if (playerState.chatRailRoom !== targetRoom) return; // race: another switch landed
    playerState.chatRailMount = mountChatRail(body, targetRoom);
    if (playerState.chatRailCompose) {
      playerState.chatRailCompose.setRoom(targetRoom, targetPlatform);
    }
  };
  // Pre-populate the rooms cache if the rail came up before /chat
  // ever ran (otherwise per-channel emote/badge prefetch can't find
  // user_id).
  if (!chatState.rooms || chatState.rooms.length === 0) {
    API.chatRooms()
      .then((r) => { chatState.rooms = r.rooms || []; proceed(); })
      .catch(() => proceed());
  } else {
    proceed();
  }
}

// Stage size handed to /multistream/tiles. Measures the real `.ms-stage`
// element so tile geometry matches what's actually on screen; falls back
// to the old fixed guess when the stage isn't mounted yet (first paint,
// before the DOM exists) or reports a zero-size box (display:none,
// mid-teardown).
function stageGeometry() {
  const stage = document.querySelector(".ms-stage");
  if (stage) {
    const r = stage.getBoundingClientRect();
    if (r.width > 0 && r.height > 0) {
      return { w: Math.round(r.width), h: Math.round(r.height) };
    }
  }
  return { w: 800, h: 450 };
}

// Fixed cols x rows shape for the grid-regular presets — matches the
// PLAYER_PRESETS trees above exactly, so the aspect ratio painted onto
// `.ms-stage` always agrees with the tiles actually on screen. Presets
// not listed here (single, focus-3, custom) don't have a uniform grid
// shape and stay unconstrained.
const PRESET_GRID_SHAPE = {
  "split-screen": { cols: 2, rows: 1 },
  quadrant: { cols: 2, rows: 2 },
  "grid-6": { cols: 2, rows: 3 },
  "grid-9": { cols: 3, rows: 3 },
};

/// Pure helper: for `n` same-size 16:9 tiles packed into a WxH box, pick
/// the cols x rows combination that maximises each tile's area. Exposed
/// as a hook for anything that wants an aspect-aware grid without a fixed
/// shape above (and for e2e to exercise the geometry math directly).
function bestGridFor(n, W, H) {
  if (!(n > 0)) return { cols: 1, rows: 1 };
  let best = { cols: n, rows: 1, area: -1 };
  for (let rows = 1; rows <= n; rows++) {
    const cols = Math.ceil(n / rows);
    const tileW = W / cols;
    const tileH = H / rows;
    const fitW = Math.min(tileW, tileH * (16 / 9));
    const fitH = fitW * (9 / 16);
    const area = fitW * fitH;
    if (area > best.area) best = { cols, rows, area };
  }
  return { cols: best.cols, rows: best.rows };
}

/// `--stage-aspect` value for a preset, or null to leave the stage
/// unconstrained (single / focus-3 / custom — none of these are a
/// uniform grid, so letterboxing math doesn't apply to the stage as a
/// whole).
function stageAspectFor(preset, w, h) {
  void w; void h; // reserved for a future non-fixed-shape preset
  const shape = PRESET_GRID_SHAPE[preset];
  if (!shape) return null;
  return (shape.cols * 16) / (shape.rows * 9);
}

