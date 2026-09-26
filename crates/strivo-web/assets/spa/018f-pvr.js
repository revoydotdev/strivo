// ── Delegated stage drag-and-drop ────────────────────────────────────
//
// Drag used to be wired per-tile with `draggable` on the whole `.ms-leaf`.
// Once a tile is playing it's covered by a cross-origin iframe, and a
// mousedown inside that iframe never produces a `dragstart` on the
// parent document — so playing tiles silently stopped being draggable
// ("sometimes works" from the user's report). Delegating to the stage and
// dragging only from a small handle inside the tile chrome sidesteps the
// iframe entirely: the handle is never covered.
//
// Contract with Lane A (player frame): `.pb-grab[data-drag-handle]` in
// the player bar is the handle.
function markStageDnd(on) {
  document.querySelectorAll(".ms-stage").forEach((s) => {
    s.classList.toggle("is-dnd", on);
    if (!on) {
      s.querySelectorAll(".is-dragging, .is-drop-target").forEach((el) =>
        el.classList.remove("is-dragging", "is-drop-target"),
      );
    }
  });
}
// A native drag can end anywhere — over a valid target, off-window, on
// Escape — and `dragend` is the one event guaranteed to fire regardless.
// One document-level listener here is simpler and more reliable than
// trying to clean up from every possible drop/cancel path individually.
document.addEventListener("dragend", () => markStageDnd(false));

/// Resolve which `.ms-leaf` a drop over the stage should land on. Direct
/// hits (including gutters/panes, which used to be dead drop zones) fall
/// through to the nearest leaf by centre-to-point distance so nothing
/// between tiles swallows a drop silently.
function resolveDropLeaf(stage, e) {
  const direct = e.target.closest && e.target.closest(".ms-leaf");
  if (direct) return direct;
  const leaves = [...stage.querySelectorAll(".ms-leaf")];
  if (!leaves.length) return null;
  let best = null;
  let bestDist = Infinity;
  for (const leaf of leaves) {
    const r = leaf.getBoundingClientRect();
    const dx = Math.max(r.left - e.clientX, 0, e.clientX - r.right);
    const dy = Math.max(r.top - e.clientY, 0, e.clientY - r.bottom);
    const dist = dx * dx + dy * dy;
    if (dist < bestDist) {
      bestDist = dist;
      best = leaf;
    }
  }
  return best;
}

/// Apply a decoded drag payload onto a resolved target leaf. Shared by
/// the stage's delegated drop handler for every payload kind (tile swap,
/// stream assign, recording assign).
function applyDropOntoLeaf(leaf, parsed, watch, streams) {
  const toPath = leaf.dataset.path || "";
  if (parsed.type === "tile") {
    const fromPath = parsed.path;
    if (fromPath === toPath) return;
    const fromNode = getNodeAt(playerState.layout, fromPath);
    const toNode = getNodeAt(playerState.layout, toPath);
    if (!fromNode || !toNode) return;
    let next = setNodeAt(
      playerState.layout,
      fromPath,
      _slot(toNode.streamId || null, toNode.recordingId || null),
    );
    next = setNodeAt(
      next,
      toPath,
      _slot(fromNode.streamId || null, fromNode.recordingId || null),
    );
    playerState.layout = next;
    savePlayerLayout();
    paintPlayerStage(watch, streams);
    return;
  }
  if (parsed.type === "stream") {
    playerState.layout = setNodeAt(playerState.layout, toPath, _slot(parsed.id, null));
    savePlayerLayout();
    // If the dropped stream isn't in our current cache (rail showed it as
    // live but the multistream snapshot is stale — race between the
    // channels poll and the tiles poll), refresh before repainting so
    // renderSlot can resolve the embed URL. Without this the dropped
    // tile renders the "Stream offline" pill even though the channel is
    // fine — the bug the user kept hitting.
    if (!streams.find((x) => x.stream_id === parsed.id)) {
      API.multistreamTiles(stageGeometry().w, stageGeometry().h, { mode: "auto" }, window.location.host)
        .then((resp) => {
          const fresh = resp.streams || [];
          playerState.chatRailLastStreams = fresh;
          paintPlayerStage(watch, fresh);
        })
        .catch((err) => {
          Toast.error(`Couldn't load dropped stream: ${err && err.message || err}`);
          paintPlayerStage(watch, streams);
        });
      return;
    }
    paintPlayerStage(watch, streams);
    return;
  }
  if (parsed.type === "recording") {
    playerState.layout = setNodeAt(playerState.layout, toPath, _slot(null, parsed.id));
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  }
}

/// Wire the whole stage's drag-and-drop ONCE per paint. Delegation means
/// a surgical tile swap (tryPatchPlayerStage) or a rail repaint never
/// loses drag wiring the way per-tile listeners did.
function wireStageDnD(stage, watch, streams) {
  let currentTarget = null;
  const clearTarget = () => {
    if (currentTarget) currentTarget.classList.remove("is-drop-target");
    currentTarget = null;
  };

  stage.addEventListener("dragstart", (e) => {
    const handle = e.target.closest && e.target.closest("[data-drag-handle]");
    if (!handle) return;
    const leaf = handle.closest(".ms-leaf");
    if (!leaf || (!leaf.dataset.streamId && !leaf.dataset.recordingId)) return;
    e.dataTransfer.setData("text/plain", encodeDragPayload({ type: "tile", path: leaf.dataset.path || "" }));
    e.dataTransfer.effectAllowed = "move";
    markStageDnd(true);
    leaf.classList.add("is-dragging");
  });

  // A drop target must accept BOTH dragenter and dragover (some DnD
  // implementations — including CDP-driven automation — only look at
  // dragenter to decide whether the zone is valid at all) — accepting
  // only dragover produced a drag that visually tracked the cursor but
  // silently cancelled on mouseup instead of firing `drop`.
  //
  // `dropEffect` MUST agree with the source's `effectAllowed`
  // (tile-drags set "move", rail/composer sources set "copy") — forcing
  // "move" unconditionally made Chromium quietly refuse the drop for a
  // "copy" source instead of firing it, with no error and no `drop`
  // event at all.
  const accept = (e) => {
    e.preventDefault();
    const allowed = e.dataTransfer.effectAllowed;
    e.dataTransfer.dropEffect = allowed === "copy" || allowed === "copyLink" ? "copy" : "move";
    const leaf = resolveDropLeaf(stage, e);
    if (leaf !== currentTarget) {
      clearTarget();
      currentTarget = leaf;
      if (currentTarget) currentTarget.classList.add("is-drop-target");
    }
  };
  stage.addEventListener("dragenter", accept);
  stage.addEventListener("dragover", accept);

  stage.addEventListener("drop", (e) => {
    e.preventDefault();
    const leaf = resolveDropLeaf(stage, e);
    clearTarget();
    markStageDnd(false);
    if (!leaf) return;
    const parsed = decodeDragPayload(e.dataTransfer.getData("text/plain"));
    if (!parsed) return;
    applyDropOntoLeaf(leaf, parsed, watch, streams);
  });
}

/// Nearest leaf to `leaf` in a compass direction, by centre-to-centre
/// position — used by both Shift+Arrow (swap) and Alt+Arrow (move focus).
function neighborLeafInDirection(stage, leaf, dir) {
  const rect = leaf.getBoundingClientRect();
  const cx = (rect.left + rect.right) / 2;
  const cy = (rect.top + rect.bottom) / 2;
  let best = null;
  let bestDist = Infinity;
  stage.querySelectorAll(".ms-leaf").forEach((other) => {
    if (other === leaf) return;
    const r = other.getBoundingClientRect();
    const ox = (r.left + r.right) / 2;
    const oy = (r.top + r.bottom) / 2;
    const dx = ox - cx;
    const dy = oy - cy;
    let ok = false;
    if (dir === "left") ok = dx < -1 && Math.abs(dy) < rect.height;
    else if (dir === "right") ok = dx > 1 && Math.abs(dy) < rect.height;
    else if (dir === "up") ok = dy < -1 && Math.abs(dx) < rect.width;
    else if (dir === "down") ok = dy > 1 && Math.abs(dx) < rect.width;
    if (!ok) return;
    const dist = dx * dx + dy * dy;
    if (dist < bestDist) {
      bestDist = dist;
      best = other;
    }
  });
  return best;
}

/// Keyboard on the stage: Enter (play poster / focus a tile / open its
/// picker), x/Delete (remove → empty slot), Shift+Arrows (swap focused
/// tile with its neighbour), Alt+Arrows (move focus without touching
/// content), Escape (blur out of an open picker). Partitioned from Lane
/// A's per-tile keys (Space/k/j/l/arrows/m/s/f/p/i/o/c/,/./<>/0-9/t on
/// `.ms-leaf`) by listening on `.ms-stage` instead and stopping
/// propagation on every key handled here, so the global 036 shortcut
/// handler never double-fires.
function wireStageKeyboard(stage, watch, streams) {
  stage.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      const active = document.activeElement;
      if (active && active.closest(".ms-picker")) {
        active.blur();
        e.stopPropagation();
      }
      return;
    }

    const leaf = e.target.closest && e.target.closest(".ms-leaf");
    if (!leaf) return;

    if (e.key === "Enter") {
      const playBtn = leaf.querySelector(".ms-play");
      const filter = leaf.querySelector(".ms-picker-filter");
      if (playBtn) {
        playBtn.click();
      } else if (filter) {
        filter.focus();
      } else {
        stage.querySelectorAll(".ms-leaf.is-focused").forEach((x) => x.classList.remove("is-focused"));
        leaf.classList.add("is-focused");
      }
      e.stopPropagation();
      return;
    }

    if (e.key === "x" || e.key === "X" || e.key === "Delete") {
      if (!leaf.dataset.streamId && !leaf.dataset.recordingId) return;
      playerState.layout = setNodeAt(playerState.layout, leaf.dataset.path || "", _slot());
      savePlayerLayout();
      paintPlayerStage(watch, streams);
      e.stopPropagation();
      return;
    }

    const dirKey = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" }[e.key];
    if (!dirKey) return;

    // Ctrl+Shift+Arrow nudges the focused leaf's PARENT split ratio —
    // checked before the plain Shift+Arrow swap below, since Ctrl+Shift
    // also carries shiftKey. Only responds when the arrow's axis matches
    // the parent split's own direction (h: left/right, v: up/down); the
    // perpendicular pair is a no-op rather than resizing an unrelated
    // ancestor split.
    if (e.ctrlKey && e.shiftKey) {
      const path = leaf.dataset.path || "";
      if (!path) return; // the root has no parent split to resize
      const parts = pathParts(path);
      const parentPath = pathStr(parts.slice(0, -1));
      const parent = getNodeAt(playerState.layout, parentPath);
      if (!parent || parent.kind !== "split") return;
      let delta = 0;
      if (parent.dir === "h") {
        if (dirKey === "left") delta = -0.05;
        else if (dirKey === "right") delta = 0.05;
      } else if (parent.dir === "v") {
        if (dirKey === "up") delta = -0.05;
        else if (dirKey === "down") delta = 0.05;
      }
      if (!delta) return;
      e.preventDefault();
      parent.ratio = Math.max(0.1, Math.min(0.9, parent.ratio + delta));
      savePlayerLayout();
      // Ratio isn't part of the patch path's content diff (only slot
      // contents are compared), so a same-shape repaint would leave the
      // pane flex styles stale — force the full render.
      playerState.lastPaintedLayout = null;
      paintPlayerStage(watch, streams);
      e.stopPropagation();
      return;
    }

    if (e.shiftKey) {
      e.preventDefault();
      const other = neighborLeafInDirection(stage, leaf, dirKey);
      if (!other) return;
      const pathA = leaf.dataset.path || "";
      const pathB = other.dataset.path || "";
      const nodeA = getNodeAt(playerState.layout, pathA);
      const nodeB = getNodeAt(playerState.layout, pathB);
      let next = setNodeAt(playerState.layout, pathA, _slot(nodeB.streamId || null, nodeB.recordingId || null));
      next = setNodeAt(next, pathB, _slot(nodeA.streamId || null, nodeA.recordingId || null));
      playerState.layout = next;
      savePlayerLayout();
      paintPlayerStage(watch, streams);
      e.stopPropagation();
      return;
    }

    if (e.altKey) {
      e.preventDefault();
      const other = neighborLeafInDirection(stage, leaf, dirKey);
      if (!other) return;
      stage.querySelectorAll(".ms-leaf.is-focused").forEach((x) => x.classList.remove("is-focused"));
      other.classList.add("is-focused");
      other.focus?.();
      e.stopPropagation();
    }
  });
}

// Rail rows are draggable as a stream source. One delegated listener on
// `document`, registered once at module load, survives every rail
// repaint (`paintChannelList` rebuilds `#channel-list` innerHTML wholesale
// on every SSE update) — the previous per-row wiring was wiped by the
// very next repaint, which is why rail drag "stopped working" after the
// first live/offline transition. `<a>` tags are draggable by default
// (the browser drags the href); setting our own payload + effectAllowed
// overrides that default.
document.addEventListener("dragstart", (e) => {
  const row = e.target.closest && e.target.closest("#channel-list .ch-row[data-live-stream-id]");
  if (!row || !row.dataset.liveStreamId) return;
  e.dataTransfer.setData("text/plain", encodeDragPayload({ type: "stream", id: row.dataset.liveStreamId }));
  e.dataTransfer.effectAllowed = "copy";
  markStageDnd(true);
});

/// Wire the filter + keyboard nav on one empty tile's picker card.
/// Replaces the old native `<select class="ms-slot-pick">` — sized to
/// the tile, arrow-key navigable, Enter picks, Escape blurs.
function wirePickerCard(tile, ctx) {
  const { watch, streams } = ctx;
  const picker = tile.querySelector(".ms-picker");
  if (!picker) return;
  const path = picker.dataset.path || "";
  const filterInput = picker.querySelector(".ms-picker-filter");
  const list = picker.querySelector(".ms-picker-list");
  if (!filterInput || !list) return;

  const rows = () => [...list.querySelectorAll(".ms-pick")];

  const applyFilter = () => {
    const q = (filterInput.value || "").trim().toLowerCase();
    rows().forEach((r) => {
      const label = (r.dataset.pickLabel || "").toLowerCase();
      r.hidden = !(!q || label.includes(q));
    });
    list.querySelectorAll(".ms-picker-group-label").forEach((h) => {
      let sib = h.nextElementSibling;
      let any = false;
      while (sib && !sib.classList.contains("ms-picker-group-label")) {
        if (!sib.hidden) any = true;
        sib = sib.nextElementSibling;
      }
      h.hidden = !any;
    });
  };
  filterInput.addEventListener("input", applyFilter);

  const pick = (btn) => {
    const val = btn.dataset.pick || "";
    let next;
    if (val.startsWith("rec:")) next = _slot(null, val.slice(4));
    else if (val.startsWith("live:")) next = _slot(val.slice(5), null);
    else return;
    // Assigning a slot does NOT start it playing — the wall opens paused
    // by design (see PLAYER_AUTOPLAY_KEY's doc comment), and the old
    // `<select>` picker never auto-played either. The viewer presses
    // Play (or Play-all) explicitly.
    playerState.layout = setNodeAt(playerState.layout, path, next);
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  };

  rows().forEach((btn) => {
    btn.addEventListener("click", () => pick(btn));
    btn.addEventListener("keydown", (e) => {
      const visible = rows().filter((r) => !r.hidden);
      const i = visible.indexOf(btn);
      if (e.key === "ArrowDown") {
        e.preventDefault();
        (visible[i + 1] || visible[0])?.focus();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        if (i <= 0) filterInput.focus();
        else visible[i - 1].focus();
      } else if (e.key === "Enter") {
        e.preventDefault();
        pick(btn);
      } else if (e.key === "Escape") {
        btn.blur();
      }
    });
  });

  filterInput.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const visible = rows().filter((r) => !r.hidden);
      if (!visible.length) return;
      visible[e.key === "ArrowDown" ? 0 : visible.length - 1].focus();
    } else if (e.key === "Escape") {
      filterInput.blur();
    }
  });
}

// Re-bind interactive handlers on a single freshly-replaced .ms-leaf
// tile. Mirrors the per-tile loop at the bottom of paintPlayerStage so
// surgical patches don't need a full repaint just to wire up focus /
// slot-picker / chrome on one tile. Drag-and-drop is delegated to the
// stage (wireStageDnD) — it is wired once per paint, not per tile.
function wireTileHandlers(tile, stage, watch, streams) {
  // Focus on background click (not on buttons / input / picker / iframe).
  tile.addEventListener("mousedown", (e) => {
    if (e.target.closest("button, select, input, .ms-picker, iframe")) return;
    stage.querySelectorAll(".ms-leaf.is-focused").forEach((x) => x.classList.remove("is-focused"));
    tile.classList.add("is-focused");
  });

  wirePickerCard(tile, { stage, watch, streams });

  // A slot whose stream left the live set renders the "Stream offline"
  // pill, and a slot holding a recording that hasn't finished writing
  // yet renders "Still recording" (both in 019a's
  // renderPopulatedSlotHtml) — dead ends with no picker. `.ms-empty-pill`
  // only ever appears on those two branches: a real empty slot renders
  // `.ms-picker` instead (renderEmptySlotHtml, above), so this check
  // can't misfire on a genuinely empty tile. Clicking it converts the
  // slot back to a real empty one and repaints, landing on the same
  // picker card an empty slot shows.
  const offlinePill = tile.querySelector(".ms-empty-pill");
  if (offlinePill) {
    offlinePill.addEventListener("click", () => {
      const path = tile.dataset.path || "";
      playerState.layout = setNodeAt(playerState.layout, path, _slot());
      savePlayerLayout();
      paintPlayerStage(watch, streams);
    });
  }

  wireTileChrome(tile, { stage, watch, streams });
}

// Try to apply a slot-content diff between `prev` and `curr` without
// rebuilding the whole stage. Returns true if the patch succeeded
// (caller can skip the full repaint).
//
// Preserves existing iframes for any slot whose streamId/recordingId is
// unchanged — Twitch embeds pause/reload on DOM detach, so the only
// safe way to "add a stream" without nuking the others is to leave
// their tile DOM entirely alone. Falls back to a full repaint when the
// tree shape changes (split / collapse / preset switch).
function tryPatchPlayerStage(watch, prev, curr, streams) {
  const stage = watch.querySelector(".ms-stage");
  if (!stage || !prev || !sameLayoutShape(prev, curr)) return false;

  // Walk both trees in lockstep, patching only slots whose content
  // changed. The mute state for unchanged tiles is also reapplied
  // (mute is a function of soloPath, not slot content).
  const work = [];
  (function walkParallel(p, c, path) {
    if (p.kind === "split") {
      walkParallel(p.a, c.a, path ? `${path}.a` : "a");
      walkParallel(p.b, c.b, path ? `${path}.b` : "b");
      return;
    }
    work.push({ path, prevNode: p, currNode: c });
  })(prev, curr, "");

  for (const { path, prevNode, currNode } of work) {
    const tile = stage.querySelector(`.ms-leaf[data-path="${cssEscape(path)}"]`);
    if (!tile) return false; // shape thought it was the same but DOM disagrees → bail
    // A slot's own streamId never changes when a channel goes live/offline
    // — only whether `streams` still resolves it does — so the identity
    // check above is blind to exactly the transition that matters here.
    // Compare against what's actually painted (`data-stream-id` is absent
    // on the "Stream offline" pill markup) so a resolvability flip forces
    // the re-render that swaps between the two.
    const domHasStream = !!tile.dataset.streamId;
    const nowResolves = currNode.streamId ? streams.some((s) => s.stream_id === currNode.streamId) : null;
    const wentLiveOrOffline = currNode.streamId != null && nowResolves !== domHasStream;
    const sameContent =
      (prevNode.streamId || null) === (currNode.streamId || null) &&
      (prevNode.recordingId || null) === (currNode.recordingId || null) &&
      !wentLiveOrOffline;
    if (sameContent) {
      // Same content. Mute may have flipped; mountPlayerBar (via
      // reconcileControllers below) repaints the whole bar from current
      // state on every call, so there is nothing left for this branch to
      // patch by hand.
      continue;
    }
    // Content changed. Render the new tile, swap it in, re-wire it.
    // Existing iframes in OTHER tiles never get detached — only this
    // tile's old element does.
    const tmp = document.createElement("div");
    tmp.innerHTML = renderSlot(currNode, path, streams);
    const newTile = tmp.firstElementChild;
    if (!newTile) return false;
    tile.replaceWith(newTile);
    wireTileHandlers(newTile, stage, watch, streams);
  }

  // Apply mute / mount any tile this patch swapped in. The patch path
  // edits tiles in place, so without this a solo change would update the
  // button label and nothing else.
  reconcileControllers(watch.querySelector(".ms-stage"));

  // Toolbar reflects layout-aware bits (preset label, leaf count,
  // mute-all pressed state). Tree shape didn't change, so the only
  // things that can shift are leaf-count (same — slots unchanged) and
  // the mute-all pressed state.
  const muteAll = watch.querySelector("#watch-mute-all");
  if (muteAll) muteAll.classList.toggle("active", !playerState.soloPath);
  // A solo/unsolo click takes this fast path (content unchanged, only the
  // mute state moved) — persist here too, or soloPath silently reverts to
  // whatever the last full repaint saved on the next reload.
  savePlayerLayout();
  return true;
}

// Robust CSS attribute-selector escaping for data-path values (root is
// "" and paths are alphabetic). Falls back when the browser lacks
// CSS.escape (some older runtimes).
function cssEscape(s) {
  if (typeof CSS !== "undefined" && CSS && typeof CSS.escape === "function") {
    return CSS.escape(s);
  }
  return String(s).replace(/[^A-Za-z0-9_-]/g, (m) => `\\${m}`);
}

// A native <details> has no Escape behaviour, so a keyboard user who opened
// the Layout menu with Enter had no way back out except finding the toggle
// again by hand. One document listener, guarded so a re-render does not
// stack them.
function bindLayoutMenuEscape() {
  if (document.body.dataset.layoutMenuEscapeBound) return;
  document.body.dataset.layoutMenuEscapeBound = "1";
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    document.querySelectorAll("details.ms-layout-menu[open]").forEach((details) => {
      details.open = false;
      details.querySelector("summary")?.focus();
    });
  });
}

function paintPlayerStage(watch, streams) {
  const layout = playerState.layout;
  // Fast path: if the tree shape didn't change, patch only the slots
  // whose contents differ. Unchanged tiles' DOM is never touched, so
  // their iframes never detach — adding / removing / dropping a
  // single stream no longer reloads the others. Falls through to the
  // full repaint when the tree shape changes (split / collapse / preset
  // switch / first paint).
  if (playerState.lastPaintedLayout
      && tryPatchPlayerStage(watch, playerState.lastPaintedLayout, layout, streams)) {
    playerState.lastPaintedLayout = structuredClone(layout);
    playerState.chatRailLastStreams = streams;
    reconcilePlayerChatRail(streams);
    return;
  }
  const leaves = countLeaves(layout);

  // Build the preset menu (rendered as a details element). The current
  // preset's label is the summary; click expands to the option list.
  const presetLabel = PLAYER_PRESET_LABELS[playerState.preset] || "Custom";
  const presetOpts = Object.entries(PLAYER_PRESET_LABELS).map(([k, v]) => `
    <button class="sm ms-preset-opt${k === playerState.preset ? " active" : ""}" type="button" data-preset="${k}">${htmlEscape(v)}</button>`).join("");

  // Toolbar — preset dropdown · split buttons (custom-mode) · mute-all.
  const muteAllPressed = playerState.soloPath ? "" : "active";
  const customTools = playerState.preset === "custom" ? `
    <span class="watch-tb-sep" aria-hidden="true">·</span>
    <span class="pg-cap-hint">Focus a tile, then split:</span>
    <button class="sm ms-split-h" type="button" title="Split focused tile horizontally (side-by-side)">▥ Split H</button>
    <button class="sm ms-split-v" type="button" title="Split focused tile vertically (top + bottom)">▤ Split V</button>
    <button class="sm ms-collapse" type="button" title="Collapse the focused tile back into its sibling">↶ Undo split</button>` : "";
  // Single-tile (theater) view: the wall-management controls (leaf
  // count, Compose, Play-all, Mute-all) are about managing MULTIPLE
  // tiles, so they collapse behind a disclosure once there's only one to
  // manage. The preset picker stays outside it — switching to a grid
  // preset is how you'd ever leave theater mode in the first place, so
  // burying it behind its own toggle would be self-defeating. ids are
  // preserved so Lane B's handlers still bind regardless of which
  // markup shape wraps them.
  const isTheater = leaves === 1;
  const presetMenu = `
      <details class="ms-preset" id="ms-preset-menu">
        <summary class="sm ms-preset-summary" title="Multi-stream layout presets">▦ Multi-stream: ${htmlEscape(presetLabel)} ▾</summary>
        <div class="ms-preset-menu">${presetOpts}</div>
      </details>
      ${customTools}`;
  const wallControls = `
      <span class="watch-count pg-cap-hint">${streams.length} live · ${leaves}/${PLAYER_LEAF_CAP} tile${leaves === 1 ? "" : "s"}</span>
      <button class="sm ms-compose-open" id="ms-compose-open" type="button"
              title="Open the multi-view composer — drag streams onto the plane">⊞ Compose</button>
      <button class="sm watch-playall ${playerState.autoplay ? "active" : ""}" id="watch-playall"
              type="button" title="${playerState.autoplay ? "Pause every tile" : "Start every tile"}">${playerState.autoplay ? "⏸ Pause all" : "▶ Play all"}</button>
      <button class="sm watch-mute-all ${muteAllPressed}" id="watch-mute-all" title="Mute every tile">🔇 Mute all</button>`;
  const railToggle = `
      <button class="icon-btn watch-rail-toggle" type="button" id="watch-rail-toggle"
              title="${isWatchRailOpen() ? "Collapse channel rail" : "Expand channel rail"}"
              aria-pressed="${isWatchRailOpen() ? "true" : "false"}">☰</button>`;
  const toolbar = `
    <div class="watch-toolbar">
      ${railToggle}
      ${presetMenu}
      <span class="watch-tb-sep" aria-hidden="true">·</span>
      ${isTheater
        ? `<details class="ms-layout-menu"><summary>Layout ▾</summary>${wallControls}</details>`
        : wallControls}
    </div>`;
  bindLayoutMenuEscape();

  // Capture existing iframes/videos before we blow the stage away so
  // we can re-attach them to the new layout intact. Adding a stream,
  // switching presets, splitting tiles, etc. should not reload the
  // streams that were already playing — only newly-added slots
  // actually need fresh media elements. Keyed by stream/recording id
  // because layout paths shift across operations.
  // No detach/reattach dance any more: controllers are not owned by this
  // DOM, so wiping it cannot destroy a player. Rebuild freely, then let
  // reconcileControllers re-parent the survivors.
  const stage = document.createElement("div");
  stage.className = "ms-stage";
  stage.innerHTML = renderLayoutNode(layout, "", streams);

  watch.innerHTML = "";
  watch.insertAdjacentHTML("beforeend", toolbar);
  watch.appendChild(stage);
  document.getElementById("watch")?.classList.toggle("is-theater", isTheater);

  reconcileControllers(stage);

  // Repack a grid-regular preset's tree BEFORE locking any aspect ratio
  // to it, when the fixed shape would waste too much of the box (see
  // bestPackedGridShape's doc comment). This changes the tree itself —
  // not just a CSS ratio — so it's a full re-render: mutate the layout
  // and recurse into a fresh paintPlayerStage call rather than trying to
  // patch pane ratios in place here. Terminates in one extra pass: the
  // rebuilt tree already has the packed shape's signature, so the next
  // call's own repack check finds nothing left to do.
  const baseShape = PRESET_GRID_SHAPE[playerState.preset];
  if (baseShape) {
    const geoForPacking = stageGeometry();
    const packed = bestPackedGridShape(leaves, geoForPacking.w, geoForPacking.h, baseShape);
    if (gridSignature(playerState.layout) !== gridSignature(buildGridTree(packed.cols, packed.rows))) {
      playerState.layout = repackLayoutToGrid(playerState.layout, packed.cols, packed.rows);
      savePlayerLayout();
      paintPlayerStage(watch, streams);
      return;
    }
  }

  // Aspect-aware presets. A grid-regular preset (split-screen, quadrant,
  // grid-6, grid-9) gets a --stage-aspect derived from the shape it's
  // ACTUALLY laid out as (baseShape, or bestPackedGridShape's pick when
  // that shape covers more of the box — see above), so CSS can letterbox
  // the whole stage to a true 16:9-per-tile box instead of stretching
  // every tile to whatever the viewport happens to be. single/focus-3/
  // custom have no uniform grid shape, so they stay unconstrained.
  const geo = stageGeometry();
  const effectiveShape = baseShape ? bestPackedGridShape(leaves, geo.w, geo.h, baseShape) : null;
  const aspect = effectiveShape ? (effectiveShape.cols * 16) / (effectiveShape.rows * 9) : null;
  // `.has-aspect` (CSS) is what actually opts the stage out of the
  // default fill-the-row sizing — see 004b's doc comment. Gating on a
  // class rather than always setting --stage-aspect and letting
  // `aspect-ratio: auto` be a no-op means single/focus-3/custom get ZERO
  // touch from this feature, same specificity fight 020 already won for
  // theater mode.
  if (aspect) {
    stage.style.setProperty("--stage-aspect", String(aspect));
    stage.classList.add("has-aspect");
  } else {
    stage.style.removeProperty("--stage-aspect");
    stage.classList.remove("has-aspect");
  }

  // ── Preset menu ──
  watch.querySelectorAll(".ms-preset-opt").forEach((btn) => {
    btn.addEventListener("click", () => {
      const p = btn.dataset.preset;
      if (!PLAYER_PRESETS[p]) return;
      // Preserve any populated streams across the preset switch. The
      // user expects 'go from single to quadrant' to keep the open
      // stream in the first slot, not to wipe it. Collect every
      // populated slot from the current layout (depth-first, a→b),
      // build the fresh preset, then refill the first N empty slots
      // with the preserved streams in order.
      const preserved = [];
      // Also capture which source path was previously soloed so the
      // user's audio choice survives the layout switch.
      const prevSoloPath = playerState.soloPath || "";
      walkLayout(playerState.layout, (n, path) => {
        if (n.kind === "slot" && (n.streamId || n.recordingId)) {
          preserved.push({ streamId: n.streamId || null, recordingId: n.recordingId || null, wasSoloed: path === prevSoloPath });
        }
      });
      let next = PLAYER_PRESETS[p]();
      let newSoloPath = "";
      if (preserved.length) {
        const slotPaths = [];
        walkLayout(next, (n, path) => {
          if (n.kind === "slot") slotPaths.push(path);
        });
        for (let i = 0; i < Math.min(preserved.length, slotPaths.length); i++) {
          next = setNodeAt(next, slotPaths[i], _slot(preserved[i].streamId, preserved[i].recordingId));
          if (preserved[i].wasSoloed) newSoloPath = slotPaths[i];
        }
      }
      playerState.preset = p;
      playerState.layout = next;
      // Restore soloPath if the soloed source landed in the new layout.
      if (newSoloPath) playerState.soloPath = newSoloPath;
      else if (prevSoloPath) playerState.soloPath = ""; // its source dropped off — fall back to mute-all
      savePlayerLayout();
      paintPlayerStage(watch, streams);
    });
  });

  // ── Custom-mode split / collapse ──
  const splitFocused = (dir) => {
    const focused = stage.querySelector(".ms-leaf.is-focused") || stage.querySelector(".ms-leaf");
    if (!focused) return;
    const path = focused.dataset.path || "";
    if (countLeaves(playerState.layout) >= PLAYER_LEAF_CAP) {
      Toast.error(`Tile cap reached (${PLAYER_LEAF_CAP}) — collapse a tile first.`);
      return;
    }
    const node = getNodeAt(playerState.layout, path);
    if (node.kind !== "slot") return;
    const next = _split(dir, 0.5, _slot(node.streamId), _slot());
    playerState.layout = setNodeAt(playerState.layout, path, next);
    playerState.preset = "custom";
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  };
  // `button.`-qualified: `.ms-split-h`/`.ms-split-v` are ALSO the class
  // names `renderLayoutNode` puts on an h/v split's own container div
  // (`class="ms-split ms-split-h"`). An unqualified `.ms-split-h` lookup
  // silently bound this click handler to that layout div instead of the
  // (at the time, absent — customTools is empty outside "custom" preset)
  // toolbar button whenever the tree's root happened to be an h-split —
  // e.g. every "split-screen" wall. Any click bubbling up through the
  // stage then force-split whatever leaf it landed in. The old
  // `<select>` picker's "change" event never bubbled into this, which is
  // why nothing surfaced it until the picker card's "click" did.
  watch.querySelector("button.ms-split-h")?.addEventListener("click", () => splitFocused("h"));
  watch.querySelector("button.ms-split-v")?.addEventListener("click", () => splitFocused("v"));
  watch.querySelector(".ms-collapse")?.addEventListener("click", () => {
    const focused = stage.querySelector(".ms-leaf.is-focused") || stage.querySelector(".ms-leaf");
    if (!focused) return;
    const path = focused.dataset.path || "";
    if (!path) return; // can't collapse the root
    const parts = pathParts(path);
    const parentPath = pathStr(parts.slice(0, -1));
    const parent = getNodeAt(playerState.layout, parentPath);
    if (!parent || parent.kind !== "split") return;
    const siblingKey = parts[parts.length - 1] === "a" ? "b" : "a";
    playerState.layout = setNodeAt(playerState.layout, parentPath, parent[siblingKey]);
    playerState.preset = "custom";
    savePlayerLayout();
    paintPlayerStage(watch, streams);
  });

  // Per-tile interactions (dragstart on populated, drop targets on all,
  // slot picker on empty, solo/fs/remove on populated). Single source
  // of truth — `wireTileHandlers` is also called by the in-place patch
  // path so a surgical tile replacement gets identical wiring.
  stage.querySelectorAll(".ms-leaf").forEach((tile) => {
    wireTileHandlers(tile, stage, watch, streams);
  });
  // Delegated once per paint — see wireStageDnD's own doc comment for why
  // this replaced per-tile `draggable`/drag listeners.
  wireStageDnD(stage, watch, streams);
  wireStageKeyboard(stage, watch, streams);

  // Mute-all toolbar button: top-level, not per-tile.
  watch.querySelector("#watch-mute-all")?.addEventListener("click", () => {
    playerState.soloPath = "";
    paintPlayerStage(watch, streams);
  });

  watch.querySelector("#watch-playall")?.addEventListener("click", () => {
    playerState.autoplay = !playerState.autoplay;
    // Pausing the wall also clears per-tile plays, so "Pause all" means it.
    if (!playerState.autoplay) playerState.playing = [];
    savePlayerAutoplay();
    // Full repaint: every tile's src changes, so the patch fast-path has
    // nothing to conserve here.
    playerState.lastPaintedLayout = null;
    paintPlayerStage(watch, streams);
  });

  watch.querySelector("#ms-compose-open")?.addEventListener("click", () => {
    openComposer(watch, streams);
  });

  // Rail rows are draggable via the delegated document-level `dragstart`
  // listener declared next to wireStageDnD — it survives every rail
  // repaint, unlike the old per-row wiring this replaced.

  // ── Resize gutters ──
  stage.querySelectorAll(".ms-gutter").forEach((gutter) => {
    gutter.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const path = gutter.dataset.path || "";
      const split = getNodeAt(playerState.layout, path);
      if (!split || split.kind !== "split") return;
      const parent = gutter.parentElement;
      const rect = parent.getBoundingClientRect();
      const dir = split.dir;
      playerState.resizeFx = { path, parentRect: rect, dir };
      document.body.classList.add("ms-resizing");
      const onMove = (ev) => {
        const fx = playerState.resizeFx;
        if (!fx) return;
        const pos = fx.dir === "h"
          ? (ev.clientX - fx.parentRect.left) / fx.parentRect.width
          : (ev.clientY - fx.parentRect.top) / fx.parentRect.height;
        const ratio = Math.min(0.9, Math.max(0.1, pos));
        const node = getNodeAt(playerState.layout, fx.path);
        node.ratio = ratio;
        // Live update without full repaint — tweak flex on siblings.
        const a = parent.children[0];
        const b = parent.children[2];
        if (a && b) {
          a.style.flex = `${ratio} ${ratio} 0`;
          b.style.flex = `${1 - ratio} ${1 - ratio} 0`;
        }
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.classList.remove("ms-resizing");
        playerState.resizeFx = null;
        savePlayerLayout();
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    });
  });

  // Snapshot the layout we just painted so the next call can try the
  // fast in-place patch path instead of rebuilding the stage. Stashed
  // *after* the full paint so a mid-paint exception doesn't leave the
  // diff comparing against a partial state.
  playerState.lastPaintedLayout = structuredClone(layout);
  playerState.chatRailLastStreams = streams;
  reconcilePlayerChatRail(streams);
}

// Recursive renderer. Returns an HTML string.
function renderLayoutNode(node, path, streams) {
  if (node.kind === "slot") return renderSlot(node, path, streams);
  // Split: flex container with two children + a gutter between.
  const flexDir = node.dir === "h" ? "row" : "column";
  const r = node.ratio;
  return `
    <div class="ms-split ms-split-${node.dir}" style="flex-direction:${flexDir}" data-path="${htmlEscape(path)}">
      <div class="ms-pane" style="flex:${r} ${r} 0">${renderLayoutNode(node.a, path ? `${path}.a` : "a", streams)}</div>
      <div class="ms-gutter ms-gutter-${node.dir}" data-path="${htmlEscape(path)}" title="Drag to resize"></div>
      <div class="ms-pane" style="flex:${1 - r} ${1 - r} 0">${renderLayoutNode(node.b, path ? `${path}.b` : "b", streams)}</div>
    </div>`;
}

