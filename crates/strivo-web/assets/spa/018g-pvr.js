// ── Multi-view composer ──────────────────────────────────────────────
//
// A modal for building the wall: pick a tiling, then fill it by dragging
// sources onto cells or by clicking a cell and then a source. Everything
// applies to the live stage immediately — there is no OK/Cancel, because
// the stage behind the modal *is* the preview.
//
// The mini-map renders the same layout tree the stage uses, so cell paths
// and drop semantics are shared with the real tiles rather than reimplemented.
let composerSelectedPath = null;

function composerSourceChips(streams) {
  const live = (streams || []).map((s) => `
    <button type="button" class="msc-src" draggable="true"
            data-src-kind="stream" data-src-id="${htmlEscape(s.stream_id)}"
            title="${htmlEscape(s.channel_name)} · ${htmlEscape(s.platform)}">
      <span class="msc-src-dot ${htmlEscape(String(s.platform || "").toLowerCase())}"></span>
      <span class="msc-src-name">${htmlEscape(s.channel_name)}</span>
      <span class="msc-src-meta">${s.viewer_count != null ? formatCount(s.viewer_count) : "live"}</span>
    </button>`).join("");
  const recs = (recCache || [])
    .filter((r) => r.state === "Finished" && r.file_exists !== false)
    .sort((a, b) => recordingTime(b) - recordingTime(a))
    .slice(0, 12)
    .map((r) => `
    <button type="button" class="msc-src msc-src-rec" draggable="true"
            data-src-kind="rec" data-src-id="${htmlEscape(r.id)}"
            title="${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id)}">
      <span class="msc-src-dot rec"></span>
      <span class="msc-src-name">${htmlEscape(niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8))}</span>
      <span class="msc-src-meta">${htmlEscape(shortWhen(r.started_at))}</span>
    </button>`).join("");
  return `
    <div class="msc-src-group">
      <div class="msc-src-head">Live now <span class="ch-count">${(streams || []).length}</span></div>
      <div class="msc-src-list">${live || '<div class="empty sm">No live channels</div>'}</div>
    </div>
    <div class="msc-src-group">
      <div class="msc-src-head">Recent recordings</div>
      <div class="msc-src-list">${recs || '<div class="empty sm">No recordings</div>'}</div>
    </div>`;
}

/// Recursively render the layout tree as a proportional mini-map. Mirrors
/// the stage's own geometry so what you arrange is what you get.
function composerMapHtml(node, path, streams) {
  if (!node) return "";
  if (node.kind === "split") {
    const dir = node.dir === "h" ? "row" : "column";
    const ratio = Math.max(0.1, Math.min(0.9, node.ratio ?? 0.5));
    return `
      <div class="msc-split" style="flex-direction:${dir}">
        <div class="msc-branch" style="flex:${ratio}">${composerMapHtml(node.a, path ? path + ".a" : "a", streams)}</div>
        <div class="msc-branch" style="flex:${1 - ratio}">${composerMapHtml(node.b, path ? path + ".b" : "b", streams)}</div>
      </div>`;
  }
  const sel = composerSelectedPath === path ? " is-selected" : "";
  if (node.streamId) {
    const s = (streams || []).find((x) => x.stream_id === node.streamId);
    const name = s ? s.channel_name : node.streamId;
    const stale = s ? "" : " is-stale";
    return `
      <div class="msc-cell is-filled${sel}${stale}" data-path="${htmlEscape(path)}" draggable="true" tabindex="0">
        <span class="msc-cell-name">${htmlEscape(name)}</span>
        ${s ? "" : '<span class="msc-cell-warn">offline</span>'}
        <button type="button" class="msc-cell-clear" data-clear="${htmlEscape(path)}" title="Clear this tile" aria-label="Clear tile">✕</button>
      </div>`;
  }
  if (node.recordingId) {
    const r = (recCache || []).find((x) => x.id === node.recordingId);
    const name = r ? (niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8)) : node.recordingId.slice(0, 8);
    return `
      <div class="msc-cell is-filled is-rec${sel}" data-path="${htmlEscape(path)}" draggable="true" tabindex="0">
        <span class="msc-cell-name">${htmlEscape(name)}</span>
        <button type="button" class="msc-cell-clear" data-clear="${htmlEscape(path)}" title="Clear this tile" aria-label="Clear tile">✕</button>
      </div>`;
  }
  return `
    <div class="msc-cell${sel}" data-path="${htmlEscape(path)}" tabindex="0">
      <span class="msc-cell-empty">+</span>
    </div>`;
}

/// Path of the first leaf in reading order, empty or not. Used as the
/// fallback destination when a wall is full but the viewer asked for
/// something specific.
function firstLeafPath(layout) {
  let found = null;
  walkLayout(layout, (node, path) => {
    if (found === null && node.kind === "slot") found = path;
  });
  return found;
}

function firstEmptyPath(layout) {
  let found = null;
  walkLayout(layout, (node, path) => {
    if (found === null && node.kind === "slot" && !node.streamId && !node.recordingId) found = path;
  });
  return found;
}

function openComposer(watch, streams) {
  const existing = document.getElementById("ms-composer");
  if (existing) existing.remove();
  composerSelectedPath = null;

  // Restore focus to whatever opened the composer (the Compose button) on
  // every close path — a modal that eats focus and never gives it back
  // strands keyboard/screen-reader users on a detached button.
  const opener = document.activeElement;
  let onEsc = null;
  let onTrapKey = null;

  const overlay = document.createElement("div");
  overlay.className = "modal-overlay";
  overlay.id = "ms-composer";
  document.body.appendChild(overlay);

  // Registered once, not inside wire() — wire() reruns on every draw()
  // (each state change redraws overlay.innerHTML), and `overlay` itself
  // persists across those redraws, so a listener attached there on every
  // wire() call would accumulate one per redraw.
  const closeComposer = () => {
    if (onEsc) {
      document.removeEventListener("keydown", onEsc);
      onEsc = null;
    }
    if (onTrapKey) {
      overlay.removeEventListener("keydown", onTrapKey);
      onTrapKey = null;
    }
    overlay.remove();
    if (opener && typeof opener.focus === "function" && document.contains(opener)) opener.focus();
  };
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) closeComposer();
  });

  const repaintStage = () => {
    savePlayerLayout();
    playerState.lastPaintedLayout = null;
    paintPlayerStage(watch, streams);
  };

  const draw = () => {
    const presets = Object.entries(PLAYER_PRESET_LABELS)
      .filter(([k]) => k !== "custom")
      .map(([k, v]) => `<button type="button" class="sm msc-preset${k === playerState.preset ? " active" : ""}" data-preset="${k}">${htmlEscape(v)}</button>`)
      .join("");
    overlay.innerHTML = `
      <div class="modal-card msc-card" role="dialog" aria-modal="true" aria-label="Multi-view composer">
        <div class="modal-head">
          <h2 class="msc-title">Multi-view composer</h2>
          <button class="modal-close" data-action="modal-close" aria-label="Close">✕</button>
        </div>
        <div class="msc-body">
          <aside class="msc-sources">${composerSourceChips(streams)}</aside>
          <section class="msc-plane">
            <div class="msc-presets">${presets}</div>
            <div class="msc-map" id="msc-map">${composerMapHtml(playerState.layout, "", streams)}</div>
            <div class="msc-hint pg-cap-hint">
              Drag a source onto a tile, or click a tile then a source. Drag tile-to-tile to swap.
            </div>
          </section>
        </div>
        <div class="msc-foot">
          <label class="msc-toggle">
            <input type="checkbox" id="msc-paused" ${playerState.autoplay ? "" : "checked"}>
            <span>Open paused</span>
          </label>
          <span class="pg-cap-hint">${countLeaves(playerState.layout)}/${PLAYER_LEAF_CAP} tiles</span>
          <span class="msc-foot-spacer"></span>
          <button type="button" class="sm" id="msc-clear-all">Clear all</button>
          <button type="button" class="sm primary" id="msc-playall">${playerState.autoplay ? "⏸ Pause all" : "▶ Play all"}</button>
        </div>
      </div>`;
    wire();

    // Tab focus trap inside the card, and initial focus on open. Rebuilt
    // every draw() since innerHTML (and therefore every focusable element
    // in it) is rebuilt wholesale each time.
    const card = overlay.querySelector(".msc-card");
    const focusables = () =>
      card
        ? [...card.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')]
            .filter((el) => !el.disabled && el.offsetParent !== null)
        : [];
    if (onTrapKey) overlay.removeEventListener("keydown", onTrapKey);
    onTrapKey = (e) => {
      if (e.key !== "Tab") return;
      const f = focusables();
      if (!f.length) return;
      const first = f[0];
      const last = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    overlay.addEventListener("keydown", onTrapKey);
    if (!card?.contains(document.activeElement)) {
      (card?.querySelector(".modal-close") || focusables()[0])?.focus();
    }
  };

  const assign = (path, kind, id) => {
    // The root tile's path is "", which is falsy — never test these paths
    // for truthiness or a single-tile layout looks like "no tile at all".
    if (path === null || path === undefined) return;
    playerState.layout = setNodeAt(
      playerState.layout,
      path,
      kind === "rec" ? _slot(null, id) : _slot(id, null),
    );
    repaintStage();
    draw();
  };

  const wire = () => {
    overlay.querySelector('[data-action="modal-close"]')?.addEventListener("click", () => closeComposer());

    overlay.querySelectorAll(".msc-preset").forEach((b) => {
      b.addEventListener("click", () => {
        const k = b.dataset.preset;
        if (!PLAYER_PRESETS[k]) return;
        playerState.preset = k;
        playerState.layout = PLAYER_PRESETS[k]();
        playerState.playing = [];
        composerSelectedPath = null;
        repaintStage();
        draw();
      });
    });

    // Sources: draggable, and click-to-assign into the selected (or first
    // empty) cell so the whole flow works without a drag if you prefer.
    overlay.querySelectorAll(".msc-src").forEach((chip) => {
      chip.addEventListener("dragstart", (e) => {
        const type = chip.dataset.srcKind === "rec" ? "recording" : "stream";
        e.dataTransfer.setData("text/plain", encodeDragPayload({ type, id: chip.dataset.srcId }));
        e.dataTransfer.effectAllowed = "copy";
      });
      chip.addEventListener("click", () => {
        const target =
          composerSelectedPath !== null ? composerSelectedPath : firstEmptyPath(playerState.layout);
        if (target === null || target === undefined) {
          Toast.error("No empty tile — pick a bigger layout or clear one.");
          return;
        }
        assign(target, chip.dataset.srcKind, chip.dataset.srcId);
      });
    });

    overlay.querySelectorAll(".msc-cell").forEach((cell) => {
      const path = cell.dataset.path || "";
      cell.addEventListener("click", (e) => {
        if (e.target.closest(".msc-cell-clear")) return;
        composerSelectedPath = composerSelectedPath === path ? null : path;
        draw();
      });
      cell.addEventListener("dragover", (e) => {
        e.preventDefault();
        cell.classList.add("is-drop-target");
      });
      cell.addEventListener("dragleave", () => cell.classList.remove("is-drop-target"));
      cell.addEventListener("drop", (e) => {
        e.preventDefault();
        cell.classList.remove("is-drop-target");
        const parsed = decodeDragPayload(e.dataTransfer.getData("text/plain") || "");
        if (!parsed) return;
        if (parsed.type === "stream") return assign(path, "stream", parsed.id);
        if (parsed.type === "recording") return assign(path, "rec", parsed.id);
        if (parsed.type === "tile" && parsed.path !== path) {
          const from = getNodeAt(playerState.layout, parsed.path);
          const to = getNodeAt(playerState.layout, path);
          let next = setNodeAt(playerState.layout, parsed.path, _slot(to.streamId || null, to.recordingId || null));
          next = setNodeAt(next, path, _slot(from.streamId || null, from.recordingId || null));
          playerState.layout = next;
          repaintStage();
          draw();
        }
      });
      if (cell.classList.contains("is-filled")) {
        cell.addEventListener("dragstart", (e) => {
          e.dataTransfer.setData("text/plain", encodeDragPayload({ type: "tile", path }));
          e.dataTransfer.effectAllowed = "move";
        });
      }
    });

    overlay.querySelectorAll(".msc-cell-clear").forEach((btn) => {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        playerState.layout = setNodeAt(playerState.layout, btn.dataset.clear || "", _slot());
        repaintStage();
        draw();
      });
    });

    overlay.querySelector("#msc-clear-all")?.addEventListener("click", () => {
      walkLayout(playerState.layout, (node, path) => {
        if (node.kind === "slot") {
          playerState.layout = setNodeAt(playerState.layout, path, _slot());
        }
      });
      repaintStage();
      draw();
    });

    overlay.querySelector("#msc-paused")?.addEventListener("change", (e) => {
      playerState.autoplay = !e.currentTarget.checked;
      if (!playerState.autoplay) playerState.playing = [];
      savePlayerAutoplay();
      repaintStage();
      draw();
    });

    overlay.querySelector("#msc-playall")?.addEventListener("click", () => {
      playerState.autoplay = !playerState.autoplay;
      if (!playerState.autoplay) playerState.playing = [];
      savePlayerAutoplay();
      repaintStage();
      draw();
    });
  };

  draw();
  onEsc = (e) => {
    if (e.key === "Escape") closeComposer();
  };
  document.addEventListener("keydown", onEsc);
}

/// Poster frame for a paused live tile.
///
/// A paused tile deliberately renders NO iframe: a Twitch/YouTube embed
/// downloads and boots an entire player application even when it is not
/// going to play, which on a 9-tile wall is nine player apps for a grid
/// nobody has pressed play on yet. A still frame costs one image.
function tilePosterUrl(s) {
  if (!s) return null;
  const c = (channelCache || []).find((x) => `${x.platform}:${x.id}` === s.stream_id);
  return c ? liveThumbUrl(c) : null;
}

function renderSlot(slot, path, streams) {
  // Populated tiles (recording or live stream) render in 019a-pvr.js so the
  // player chrome can evolve without touching the layout code here.
  if (slot.recordingId || slot.streamId) return renderPopulatedSlotHtml(slot, path, streams);
  return renderEmptySlotHtml(path, streams);
}

/// Empty-slot picker card — replaces the old native `<select>`. Live
/// channels first, then up to 24 recent finished recordings; a filter
/// input narrows both lists by name. Sized to the tile itself so it reads
/// as part of the wall rather than a floating menu.
function renderEmptySlotHtml(path, streams) {
  const liveRows = (streams || []).map((s) => `
    <button type="button" class="ms-pick" data-pick="live:${htmlEscape(s.stream_id)}"
            data-pick-label="${htmlEscape(s.channel_name)}" role="option">
      <span class="ms-pick-dot live" aria-hidden="true">●</span>
      <span class="ms-pick-name">${htmlEscape(s.channel_name)}</span>
      <span class="ms-pick-meta">${htmlEscape(s.platform)}${s.viewer_count != null ? ` · ${formatCount(s.viewer_count)}` : ""}</span>
    </button>`).join("");
  const finishedRecs = (recCache || [])
    .filter((r) => r.state === "Finished" && r.file_exists !== false)
    .sort((a, b) => recordingTime(b) - recordingTime(a))
    .slice(0, 24);
  const recRows = finishedRecs.map((r) => {
    const label = niceTitle(r.stream_title) || r.channel_name || r.id.slice(0, 8);
    // The filter matches channel AND title, even though the display
    // label only shows whichever one it picked (title first) — typing
    // the channel name must still find a recording titled something
    // else entirely.
    const filterLabel = [niceTitle(r.stream_title), r.channel_name].filter(Boolean).join(" ") || label;
    return `
      <button type="button" class="ms-pick ms-pick-rec" data-pick="rec:${htmlEscape(r.id)}"
              data-pick-label="${htmlEscape(filterLabel)}" role="option">
        <span class="ms-pick-dot rec" aria-hidden="true">▣</span>
        <span class="ms-pick-name">${htmlEscape(label)}</span>
        <span class="ms-pick-meta">${htmlEscape(shortWhen(r.started_at))}</span>
      </button>`;
  }).join("");
  const empty = !liveRows && !recRows
    ? '<div class="ms-picker-empty pg-cap-hint">No live channels or recordings yet.</div>'
    : "";
  return `
    <div class="ms-leaf ms-empty" data-path="${htmlEscape(path)}" tabindex="0">
      <div class="ms-picker" data-path="${htmlEscape(path)}">
        <input type="text" class="ms-picker-filter" placeholder="Filter live channels / recordings…"
               aria-label="Filter streams and recordings" autocomplete="off">
        <div class="ms-picker-list" role="listbox" aria-label="Pick a stream or recording for this tile">
          ${liveRows ? `<div class="ms-picker-group-label">Live now</div>${liveRows}` : ""}
          ${recRows ? `<div class="ms-picker-group-label">Recent recordings</div>${recRows}` : ""}
          ${empty}
        </div>
        <div class="ms-empty-hint pg-cap-hint">…or drag a channel from the rail.</div>
      </div>
    </div>`;
}

// ── Rail click on #/watch ────────────────────────────────────────────
//
// Clicking a live rail row while on #/watch used to navigate away to
// #/library — the click landed on the default selectChannel() behaviour
// because nothing claimed it first. This claims it: a plain click assigns
// the channel into a tile (like dropping it there), Ctrl/Cmd-click keeps
// the old "go to channel detail" behaviour for anyone who wants it.
RAIL_CLICK_HANDLERS.watch = (channelKey, ev) => {
  if (ev && (ev.metaKey || ev.ctrlKey)) return false;
  const sep = (channelKey || "").indexOf(":");
  if (sep < 0) return false;
  const platform = channelKey.slice(0, sep);
  const id = channelKey.slice(sep + 1);
  const channel = (channelCache || []).find((c) => c.platform === platform && String(c.id) === id);
  if (!channel || !channel.is_live) return false;

  const watch = document.getElementById("watch");
  if (!watch || !playerState.layout) return false;
  const streamId = `${channel.platform}:${channel.id}`;
  const stage = watch.querySelector(".ms-stage");
  const focused = stage?.querySelector(".ms-leaf.is-focused");
  const dest = (focused && focused.dataset.path) ?? firstEmptyPath(playerState.layout) ?? firstLeafPath(playerState.layout);
  if (dest === null || dest === undefined) return false;

  const node = _slot(streamId, null);
  playerState.layout = setNodeAt(playerState.layout, dest, node);
  setTilePlaying(node, true, dest);
  savePlayerLayout();

  const streams = playerState.chatRailLastStreams || [];
  if (streams.find((s) => s.stream_id === streamId)) {
    paintPlayerStage(watch, streams);
  } else {
    // Rail said live, but the multistream snapshot hasn't caught up yet —
    // refresh before repainting so the tile can resolve an embed URL
    // instead of showing "Stream offline" for a channel that's fine.
    API.multistreamTiles(stageGeometry().w, stageGeometry().h, { mode: "auto" }, window.location.host)
      .then((resp) => {
        const fresh = resp.streams || [];
        playerState.chatRailLastStreams = fresh;
        paintPlayerStage(watch, fresh);
      })
      .catch((err) => {
        Toast.error(`Couldn't load ${channel.display_name || channel.name}: ${err && err.message || err}`);
      });
  }
  return true;
};

// ── e2e hooks ────────────────────────────────────────────────────────
Object.assign(TEST_HOOK_EXTENSIONS, {
  encodeDragPayload,
  decodeDragPayload,
  bestGridFor,
  stageAspectFor,
  paintChannelList,
  bestPackedGridShape,
  candidateGridShapes,
  gridFitArea,
  paintPlayerStage,
  makeRecordingController,
  debugActiveTimers,
});
