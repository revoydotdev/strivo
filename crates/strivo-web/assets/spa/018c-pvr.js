/// YouTube tile driven through `YT.Player`.
///
/// Parity with Twitch on the basics — mute and volume without reloading —
/// which is the point: a mute button that works on one platform and not the
/// other is more confusing than either behaviour applied consistently.
///
/// Quality is deliberately absent. Google decommissioned it: setPlaybackQuality
/// is a documented no-op, so a control here could not take effect.
function makeYouTubeController(spec) {
  const host = document.createElement("div");
  host.className = "watch-tile-iframe ms-iframe ms-youtube";

  let player = null;
  let ready = false;
  let muted = !!spec.muted;
  let volume = typeof spec.volume === "number" ? spec.volume : 1;
  let videoId = spec.videoId || "";
  let wantPlaying = spec.playing !== false;

  const subs = new Set();
  const notify = () => {
    const s = controller.state();
    subs.forEach((fn) => { try { fn(s); } catch (_) { /* advisory */ } });
  };
  // Like Twitch, no time/progress event — sample state while playing only.
  const statePoll = makeStatePoll(() => player && ready, notify);
  const startPoll = () => statePoll.start();
  const stopPoll = () => statePoll.stop();

  // No video id means no player API: YT.Player addresses a video, and the
  // live embed strivo builds addresses a channel. Fall straight back rather
  // than construct something that cannot work.
  if (!videoId) return makeIframeController(spec);

  /// Swap this controller's guts for the plain iframe, in place.
  ///
  /// `spec.embedUrl` is the channel-live embed, which is a different (and
  /// historically more forgiving) surface than addressing the video by id,
  /// so this is a genuine second chance rather than a repeat of the same
  /// request. Idempotent: a failing player can report more than once.
  let fellBack = false;
  const fallBackToIframe = () => {
    if (fellBack) return;
    fellBack = true;
    stopPoll();
    let fb;
    try {
      if (player && typeof player.destroy === "function") player.destroy();
    } catch (_) {
      /* the node is replaced below regardless */
    }
    player = null;
    ready = false;
    fb = makeIframeController(spec);
    const parent = controller.root.parentElement;
    controller.root.replaceWith(fb.root);
    controller.root = fb.root;
    controller.capabilities = fb.capabilities;
    controller.setMuted = fb.setMuted;
    controller.setVolume = fb.setVolume;
    controller.setQuality = fb.setQuality;
    controller.repoint = fb.repoint;
    controller.destroy = fb.destroy;
    controller.mount = fb.mount;
    controller.isReady = fb.isReady;
    controller.play = fb.play;
    controller.pause = fb.pause;
    controller.togglePlay = fb.togglePlay;
    controller.seek = fb.seek;
    controller.currentTime = fb.currentTime;
    controller.duration = fb.duration;
    controller.setRate = fb.setRate;
    controller.state = fb.state;
    if (parent) fb.mount(parent);
    // Re-apply the level the tile is supposed to be at.
    try {
      fb.setMuted(muted);
    } catch (_) {
      /* best effort */
    }
    notify();
  };

  loadYouTubeApiOnce()
    .then((YT) => {
      if (!host.parentElement) return; // tile removed while loading
      player = new YT.Player(host, {
        videoId,
        playerVars: {
          autoplay: wantPlaying ? 1 : 0,
          mute: muted ? 1 : 0,
          playsinline: 1,
          enablejsapi: 1,
          origin: window.location.origin,
        },
        events: {
          onReady: () => {
            ready = true;
            try {
              if (muted) player.mute();
              else player.unMute();
              player.setVolume(Math.round(volume * 100));
            } catch (_) {
              /* pre-ready races */
            }
            notify();
          },
          onStateChange: (e) => {
            const st = e && e.data;
            if (st === 1) startPoll(); else stopPoll();
            notify();
          },
          // Any player-side failure falls back to the plain channel-live
          // iframe — the path used before this controller existed. The API
          // is only worth having when it works; when it does not, playing
          // the stream matters more than being able to set its volume.
          onError: (e) => {
            const codes = {
              2: "invalid parameter",
              5: "HTML5 player error",
              100: "video not found",
              101: "embedding disallowed by the owner",
              150: "embedding disallowed by the owner",
              153: "missing referer",
            };
            console.warn(
              `[strivo] YouTube player error ${e && e.data} (${codes[e && e.data] || "unknown"}) — falling back to the iframe embed`,
            );
            fallBackToIframe();
          },
        },
      });
    })
    .catch(() => fallBackToIframe());

  const controller = {
    kind: "youtube",
    root: host,
    // Quality is intentionally absent — see makeYouTubeController's own
    // comment: setPlaybackQuality is a documented no-op. Seek/duration are
    // also absent: the embed strivo builds addresses a live CHANNEL, not a
    // VOD, so a seek control would imply a capability the live embed does
    // not actually have.
    capabilities: {
      play: true, pause: true, seek: false, duration: false, volume: true,
      mute: true, quality: false, rate: false, pip: false, fullscreen: true,
      live: true, audioOnly: false,
    },
    mount(container) {
      if (controller.root.parentElement !== container) container.appendChild(controller.root);
    },
    destroy() {
      stopPoll();
      try {
        if (player && typeof player.destroy === "function") player.destroy();
      } catch (_) {
        /* fall through to dropping the node */
      }
      player = null;
      controller.root.remove();
    },
    setMuted(next) {
      muted = next;
      if (player && ready) {
        try {
          next ? player.mute() : player.unMute();
        } catch (_) {
          /* advisory */
        }
      }
    },
    setVolume(v) {
      volume = Math.max(0, Math.min(1, v));
      if (player && ready) {
        try {
          player.setVolume(Math.round(volume * 100));
        } catch (_) {
          /* advisory */
        }
      }
    },
    setQuality() {
      // Intentionally empty: Google decommissioned setPlaybackQuality.
    },
    repoint(next) {
      const id = next && next.videoId;
      if (!id || id === videoId) return;
      videoId = id;
      if (player && ready) {
        try {
          player.loadVideoById(id);
        } catch (_) {
          /* leave the tile where it is rather than blanking it */
        }
      }
    },
    isReady() {
      return ready;
    },
    play() {
      wantPlaying = true;
      if (player && ready) { try { player.playVideo(); } catch (_) { /* advisory */ } }
    },
    pause() {
      wantPlaying = false;
      if (player && ready) { try { player.pauseVideo(); } catch (_) { /* advisory */ } }
    },
    togglePlay() {
      if (player && ready) {
        try {
          const st = player.getPlayerState();
          (st === 1 ? controller.pause : controller.play)();
          return;
        } catch (_) { /* fall through */ }
      }
      controller.play();
    },
    seek() { /* the live-channel embed has no timeline to seek */ },
    currentTime() { return NaN; },
    duration() { return NaN; },
    setRate() { /* not offered for the same reason as seek */ },
    state() {
      let playing = wantPlaying;
      let ended = false;
      let buffering = false;
      let vol = volume;
      let mutedNow = muted;
      if (player && ready) {
        try {
          const st = player.getPlayerState();
          playing = st === 1;
          ended = st === 0;
          buffering = st === 3;
        } catch (_) { /* advisory */ }
        try { vol = (player.getVolume() || 0) / 100; } catch (_) { /* advisory */ }
        try { mutedNow = player.isMuted(); } catch (_) { /* advisory */ }
      }
      return {
        ready, playing, buffering, ended, muted: mutedNow, volume: vol,
        currentTime: NaN, duration: NaN, rate: 1,
        quality: null, qualities: [], live: true, error: null,
      };
    },
    onState(fn) {
      subs.add(fn);
      try { fn(controller.state()); } catch (_) { /* advisory */ }
      return () => subs.delete(fn);
    },
  };
  return controller;
}

function defaultPlayerControllerFactory(kind, spec) {
  if (kind === "recording") return makeRecordingController(spec);
  if (kind === "twitch") return makeTwitchController(spec);
  if (kind === "youtube") return makeYouTubeController(spec);
  return makeIframeController(spec);
}

let _playerControllerFactory = defaultPlayerControllerFactory;
function setPlayerControllerFactory(fn) {
  _playerControllerFactory = fn || defaultPlayerControllerFactory;
}

/// Diff the controller registry against the mount points in freshly-painted
/// stage HTML. Called by BOTH the full repaint and the surgical patch path,
/// so neither needs its own notion of player lifecycle.
function reconcileControllers(stage) {
  if (!stage) return;
  const wanted = new Map();
  const mounts = [...stage.querySelectorAll(".ms-mount[data-content-key]")];
  const counts = new Map();
  mounts.forEach((mount) => counts.set(
    mount.dataset.contentKey,
    (counts.get(mount.dataset.contentKey) || 0) + 1,
  ));
  mounts.forEach((mount) => {
    // A source may intentionally be shown in more than one tile (for
    // example, two quality/volume views of the same channel).  The source
    // key remains useful for persisted volume state, but it is not a player
    // identity: using it directly made the second mount overwrite the first
    // in this registry.  Give duplicate mounts deterministic per-paint
    // identities while retaining the source key as the prefix.
    const sourceKey = mount.dataset.contentKey;
    const key = counts.get(sourceKey) > 1
      ? `${sourceKey}#${mount.dataset.path || "root"}`
      : sourceKey;
    wanted.set(key, mount);
  });

  // Anything no longer on the wall is destroyed. Skipping this leaks a live
  // vendor connection — bandwidth and CPU for a tile that is gone.
  for (const [key, ctl] of playerState.controllers) {
    if (!wanted.has(key)) {
      try {
        ctl.destroy();
      } catch (_) {
        /* a controller that fails to tear down must not block the rest */
      }
      playerState.controllers.delete(key);
    }
  }

  for (const [key, mount] of wanted) {
    const sourceKey = key.split("#", 1)[0];
    const path = mount.dataset.path || "";
    const muted = computeMuted(path);
    let ctl = playerState.controllers.get(key);
    if (!ctl) {
      try {
        ctl = _playerControllerFactory(mount.dataset.kind || "iframe-fallback", {
          embedUrl: mount.dataset.embedBase || "",
          src: mount.dataset.src || "",
          // YouTube's player API addresses a video; the daemon resolves the
          // airing broadcast's id during live detection.
          videoId: mount.dataset.videoId || "",
          muted,
          volume: tileVolumeForKey(sourceKey),
          playing: mount.dataset.playing !== "0",
        });
      } catch (e) {
        // A factory that throws must not take the wall down with it.
        tracingWarn("player controller failed to construct", e);
        continue;
      }
      playerState.controllers.set(key, ctl);
    } else if (mount.dataset.embedBase || mount.dataset.src) {
      // Same content, different URL (host changed, for instance) — retarget
      // rather than rebuild.
      ctl.repoint({
        embedUrl: mount.dataset.embedBase || "",
        src: mount.dataset.src || "",
        videoId: mount.dataset.videoId || "",
      });
    }
    ctl.mount(mount);
    const vol = tileVolumeForKey(sourceKey);
    ctl.setMuted(vol === 0);
    ctl.setVolume(vol);
    ctl.setQuality(qualityPolicyFor(mount.dataset.kind || "", path));
    const leaf = mount.closest(".ms-leaf");
    if (leaf) mountPlayerBar(leaf, ctl, { path });
  }
}

/// Drop every controller — used when leaving the watch route entirely.
function destroyAllControllers() {
  for (const [, ctl] of playerState.controllers) {
    try {
      ctl.destroy();
    } catch (_) {
      /* best effort */
    }
  }
  playerState.controllers.clear();
}

function tracingWarn(msg, e) {
  console.warn(`[strivo] ${msg}:`, e);
}

// Tiles start paused. A wall of live streams that all begin playing the
// moment it opens burns bandwidth and CPU on streams the viewer has not
// chosen to watch yet, so the default is a still, cheap grid you press
// play on. Persisted so the preference survives a reload.
const PLAYER_AUTOPLAY_KEY = "strivo-player-autoplay";
function loadPlayerAutoplay() {
  try { return localStorage.getItem(PLAYER_AUTOPLAY_KEY) === "1"; } catch (_) { return false; }
}
function savePlayerAutoplay() {
  try {
    localStorage.setItem(PLAYER_AUTOPLAY_KEY, playerState.autoplay ? "1" : "0");
  } catch (_) { /* private mode */ }
}
/// Is this slot playing? A stream the viewer pressed play on stays playing
/// while the wall default is paused.
///
/// Keyed on CONTENT, not layout path, for the same reason the controller
/// registry is: a tile that moves is still the same stream. Keying on path
/// meant switching preset — which relocates a stream from "" to "a.a" —
/// silently paused it, because the new path had never been started.
function tilePlaying(node) {
  if (playerState.autoplay) return true;
  const key = contentKeyOf(node);
  return !!key && (playerState.playing || []).includes(key);
}
function setTilePlaying(node, on) {
  const key = contentKeyOf(node);
  if (!key) return;
  const set = new Set(playerState.playing || []);
  if (on) set.add(key); else set.delete(key);
  playerState.playing = [...set];
}
