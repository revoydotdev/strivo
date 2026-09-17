// ── Dedicated recording player route (#/play?recording=<id>[&t=sec]) ───
//
// #/watch is the live multiview wall — chat rail, tabs, composer, preset
// toolbar. All of that is leftover chrome once nothing on the wall is
// live, which is exactly the situation every recording-open path used to
// land in (openRecordingPlayer, 032-pvr.js, used to send everyone to
// `#/watch?recording=<id>&fresh=1`). This route is the focused
// alternative: one tile, a slim meta strip, nothing else. A recording can
// still be placed as a tile in the wall through the composer or
// drag-and-drop ("Keep both") — this route doesn't touch that path.
//
// Reuses makeRecordingController / mountPlayerBar / wireTileKeys
// unchanged, so a recording's volume/mute stays keyed by CONTENT
// (`r:<id>`, see contentKeyOf in 018-pvr.js) exactly like the wall — the
// same recording carries the same volume whether it's played here or
// tiled on #/watch. Those helpers resolve a tile's content key by
// walking `playerState.layout` from its `data-path`, so this route
// installs its OWN scratch single-slot layout for the duration — never
// persisted (savePlayerLayout() is never called here) — and restores
// whatever the wall had in memory when teardownAcrossRoutes() (008-pvr.js)
// runs ahead of the next render.
// `var`, not `let`: teardownAcrossRoutes() can run during bundle evaluation,
// before this module is reached, and a `let` would throw from its TDZ.
var _prePlayLayout;
function restoreWallLayoutAfterPlay() {
  if (_prePlayLayout === undefined) return;
  playerState.layout = _prePlayLayout;
  _prePlayLayout = undefined;
}

async function renderRecordingPlayer(ctx) {
  // Collapses the left channel rail the same way #/watch does — a
  // single-tile player floating next to a full-width channel list is the
  // same wasted-space problem section 1's theater mode already solved.
  enterWatchRoute();

  const params = new URLSearchParams(window.location.hash.split("?")[1] || "");
  const recordingId = params.get("recording") || "";
  const seekTo = parseFloat(params.get("t") || "0") || 0;

  if (!recordingId) {
    if (!mountPage(`
      <h1 class="page-title">Player</h1>
      <div class="empty"><div class="glyph">🚧</div>No recording specified.</div>
    `, ctx)) return;
    return;
  }

  // Same hydration pattern renderWatch uses (018e): trust the cache when
  // it's already warm, else resolve this ONE recording directly rather
  // than waiting on the full /recordings list.
  let rec = recCache.find((r) => r.id === recordingId) || null;
  if (!rec) {
    try {
      rec = await API.recordingOne(recordingId);
    } catch (_) {
      rec = null;
    }
    if (!isRouteCurrent(ctx)) return;
  }

  const title = rec
    ? (niceTitle(rec.stream_title) || rec.channel_name || recordingId.slice(0, 8))
    : recordingId.slice(0, 8);
  const channel = rec ? (rec.channel_name || "") : "";
  const when = rec && rec.started_at ? new Date(rec.started_at).toLocaleString() : "";
  const duration = rec && rec.duration_secs ? fmtClock(rec.duration_secs) : "";
  const missing = !rec || rec.file_exists === false;
  // B-04: a job still being written has no stable Content-Length/Range —
  // same contract renderPopulatedSlotHtml's recording branch enforces on
  // the wall (019a-pvr.js): never source /download for a job that isn't
  // Finished yet.
  const stillRecording = rec && isInProgress(rec.state);

  const stageHtml = stillRecording
    ? `<div class="ms-leaf ms-empty play-leaf" data-recording-id="${htmlEscape(recordingId)}" tabindex="0">
         <div class="ms-empty-pill">Still recording — check back when it finishes</div>
       </div>`
    : missing
    ? `<div class="empty"><div class="glyph">🚧</div>This recording is missing, or its file is gone.</div>`
    : `<div class="ms-leaf ms-leaf-rec play-leaf" data-recording-id="${htmlEscape(recordingId)}" data-path=""
            tabindex="0" data-title="${htmlEscape(title)}" data-platform="recording">
         <div class="ms-media">
           <div class="ms-mount" data-content-key="r:${htmlEscape(recordingId)}"
                data-kind="recording" data-path="" data-playing="1"
                data-src="/api/v1/recordings/${encodeURIComponent(recordingId)}/download"></div>
         </div>
         <div class="player-bar"></div>
       </div>`;

  if (!mountPage(`
    <div id="play" class="play-root" role="main">
      <div class="play-stage">${stageHtml}</div>
      <div class="play-meta">
        <span class="play-meta-title">${htmlEscape(title)}</span>
        ${channel ? `<span class="play-meta-sep pg-cap-hint">·</span><span class="play-meta-channel">${htmlEscape(channel)}</span>` : ""}
        ${when ? `<span class="play-meta-sep pg-cap-hint">·</span><span class="play-meta-when pg-cap-hint">${htmlEscape(when)}</span>` : ""}
        ${duration ? `<span class="play-meta-sep pg-cap-hint">·</span><span class="play-meta-duration pg-cap-hint">${htmlEscape(duration)}</span>` : ""}
        <span class="play-meta-spacer"></span>
        ${rec ? `<button class="sm" id="play-info" type="button">ⓘ Info</button>` : ""}
        <a class="sm" id="play-back" href="#/library">← Back</a>
      </div>
    </div>
  `, ctx)) return;

  document.getElementById("play-info")?.addEventListener("click", () => openRecordingInfo(recordingId));

  if (missing || stillRecording) return;

  const leaf = document.querySelector(".play-leaf");
  const mount = leaf && leaf.querySelector(".ms-mount");
  if (!leaf || !mount) return;

  // Scratch single-slot layout so contentKeyOf(getNodeAt(layout, ""))
  // resolves to this exact recording — see this file's header comment.
  // teardownAcrossRoutes() restored the wall before this render, so the
  // snapshot is always the wall, never a previous scratch tree.
  if (_prePlayLayout === undefined) _prePlayLayout = playerState.layout;
  playerState.layout = _slot(null, recordingId);

  const contentKey = `r:${recordingId}`;
  let ctl = playerState.controllers.get(contentKey);
  if (!ctl) {
    try {
      ctl = _playerControllerFactory("recording", {
        src: mount.dataset.src,
        muted: computeMuted(""),
        volume: tileVolumeAt(""),
        playing: true,
      });
    } catch (e) {
      tracingWarn("recording player controller failed to construct", e);
      return;
    }
    playerState.controllers.set(contentKey, ctl);
  }
  ctl.mount(mount);
  ctl.setMuted(computeMuted(""));
  ctl.setVolume(tileVolumeAt(""));

  mountPlayerBar(leaf, ctl, { path: "" });
  wireTileKeys(leaf, () => playerState.controllers.get(contentKey));

  if (seekTo > 0 && typeof ctl.onState === "function") {
    const unsub = ctl.onState((s) => {
      if (s.ready) {
        try { ctl.seek(seekTo); ctl.play && ctl.play(); } catch (_) { /* advisory */ }
        unsub();
      }
    });
  }
}
