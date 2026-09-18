/// Local recording playback. A `<video>` already has a real API, so this
/// never needed the reload workaround — it is a controller purely so the
/// reconciler can treat every tile the same way, and so a repaint stops
/// dropping playback position on the floor.
// Defect 2: codec → MIME map used to pre-check browser support and to name
// the format in the stall/error overlay. Values are representative profile
// strings, not the file's exact profile — good enough for isTypeSupported
// to answer yes/no on the codec family.
const RECORDING_CODEC_MIME = {
  av1: { mime: 'video/webm; codecs="av01.0.08M.08"', label: "AV1", kind: "video" },
  vp9: { mime: 'video/webm; codecs="vp09.00.40.08"', label: "VP9", kind: "video" },
  h264: { mime: 'video/mp4; codecs="avc1.640028"', label: "H.264", kind: "video" },
  hevc: { mime: 'video/mp4; codecs="hvc1.1.6.L120.90"', label: "HEVC", kind: "video" },
  opus: { mime: 'audio/webm; codecs="opus"', label: "Opus", kind: "audio" },
  aac: { mime: 'audio/mp4; codecs="mp4a.40.2"', label: "AAC", kind: "audio" },
};
function recordingCodecSupport(codec) {
  const entry = RECORDING_CODEC_MIME[String(codec || "").toLowerCase()];
  if (!entry) return null;
  let supported = null;
  try {
    if (typeof MediaSource !== "undefined" && MediaSource.isTypeSupported) {
      supported = MediaSource.isTypeSupported(entry.mime);
    }
  } catch (_) {
    /* advisory */
  }
  if (supported === null) {
    try {
      const probe = document.createElement(entry.kind === "audio" ? "audio" : "video");
      const ans = probe.canPlayType(entry.mime);
      supported = ans !== "";
    } catch (_) {
      supported = null;
    }
  }
  return { ...entry, supported };
}
function recordingCodecNote(videoCodec, audioCodec) {
  const parts = [];
  const flags = [];
  [["video", videoCodec], ["audio", audioCodec]].forEach(([role, codec]) => {
    if (!codec) return;
    const info = recordingCodecSupport(codec);
    const label = (info && info.label) || codec;
    parts.push(`${label} ${role}`);
    if (info && info.supported === false) flags.push(`${label} is not supported by this browser`);
  });
  if (!parts.length) return "";
  return parts.join(" / ") + (flags.length ? ` — ${flags.join("; ")}.` : ".");
}

function makeRecordingController(spec) {
  const el = document.createElement("video");
  el.className = "watch-tile-iframe ms-video";
  el.controls = false;
  el.playsInline = true;
  el.muted = !!spec.muted;
  const playing = spec.playing !== false;
  el.preload = playing ? "metadata" : "none";
  if (playing) el.autoplay = true;
  if (spec.src) el.src = spec.src;

  // A recording can be on disk, non-zero, and still unplayable — an
  // interrupted VOD pull leaves a truncated MP4 with no moov atom, and the
  // browser then hunts forever for an index that was never written. Without
  // this the tile just sits there looking like it is still loading, which is
  // indistinguishable from a slow network.
  let watchdog = null;
  let stallWatchdog = null;
  let stallBaseline = null;
  let stallProgressSeen = false;
  let errorNote = null;
  let loadingRequested = false;
  const outputPath = spec.outputPath || "";
  const recordingId = spec.recordingId || "";
  const downloadUrl = recordingId ? `/api/v1/recordings/${encodeURIComponent(recordingId)}/download` : "";
  const clearFailure = () => {
    delete el.dataset.failed;
    errorNote?.remove();
    errorNote = null;
  };
  const cancelWatchdog = () => {
    if (watchdog) clearTimeout(watchdog);
    watchdog = null;
  };
  const cancelStallWatchdog = () => {
    if (stallWatchdog) clearTimeout(stallWatchdog);
    stallWatchdog = null;
    stallBaseline = null;
  };
  const startWatchdog = () => {
    cancelWatchdog();
    if (!loadingRequested) return;
    watchdog = setTimeout(() => {
      if (loadingRequested && el.isConnected && el.readyState === 0) {
        fail("loading", "it never returned media metadata");
      }
    }, 15000);
  };
  // Metadata loading is not the only way playback can go nowhere: a file can
  // report loadedmetadata and then simply never decode a frame (black
  // screen, dead audio, no error event at all). Once metadata is in and
  // playback is wanted, sample currentTime every 8s; if it hasn't moved and
  // the element isn't paused/ended/seeking, that's a stall — unless it is
  // ordinary buffering: data still arriving (a `progress` event since the
  // last sample) with nothing decodable ahead yet (readyState below
  // HAVE_FUTURE_DATA) just re-arms instead of failing a slow-disk load.
  const armStallWatchdog = () => {
    cancelStallWatchdog();
    if (!loadingRequested || el.paused || el.ended || el.dataset.failed) return;
    stallBaseline = el.currentTime;
    stallProgressSeen = false;
    stallWatchdog = setTimeout(() => {
      if (!loadingRequested || el.paused || el.ended || el.seeking || el.dataset.failed) return;
      const buffering = stallProgressSeen && el.readyState < 3;
      if (Math.abs(el.currentTime - stallBaseline) < 0.25 && !buffering) {
        fail("stall", "playback loaded but never started decoding frames");
      } else {
        armStallWatchdog();
      }
    }, 8000);
  };
  const fail = (kind, why) => {
    if (el.dataset.failed === kind) return;
    cancelWatchdog();
    cancelStallWatchdog();
    el.dataset.failed = kind;
    errorNote?.remove();
    const codecNote = recordingCodecNote(spec.videoCodec, spec.audioCodec);
    const note = document.createElement("div");
    note.className = "ms-media-error";
    note.innerHTML =
      `<strong>Can't play this recording</strong><span>${htmlEscape(why)}</span>` +
      (kind === "decode" ? `<span class="pg-cap-hint">The file may need repair or remuxing.</span>` : "") +
      (codecNote ? `<span class="pg-cap-hint">${htmlEscape(codecNote)}</span>` : "") +
      `<span class="ms-media-error-actions">` +
      `<button type="button" class="btn btn-ghost btn-sm ms-media-error-retry">Retry</button>` +
      (downloadUrl ? `<a class="btn btn-ghost btn-sm ms-media-error-download" href="${htmlEscape(downloadUrl)}" download>Download file</a>` : "") +
      (outputPath ? `<button type="button" class="btn btn-ghost btn-sm ms-media-error-copy-path" data-path="${htmlEscape(outputPath)}">Copy file path</button>` : "") +
      `</span>`;
    note.querySelector(".ms-media-error-retry")?.addEventListener("click", () => {
      clearFailure();
      loadingRequested = true;
      el.load();
      startWatchdog();
      el.play().catch(() => {});
    });
    note.querySelector(".ms-media-error-copy-path")?.addEventListener("click", (e) => {
      const p = e.currentTarget.dataset.path || "";
      try { navigator.clipboard && navigator.clipboard.writeText(p); } catch (_) { /* advisory */ }
    });
    el.parentElement?.appendChild(note);
    errorNote = note;
  };
  el.addEventListener("error", () => {
    const codes = {
      1: ["aborted", "playback was aborted"],
      2: ["network", "a network error interrupted playback"],
      3: ["decode", "the file could not be decoded"],
      4: ["unsupported", "this format is not supported by this browser"],
    };
    const [kind, message] = codes[el.error?.code] || ["media", "the media failed to load"];
    fail(kind, message);
  });
  // `error` never fires for a file that merely never finishes loading its
  // metadata, which is exactly the truncated-MP4 case, so time it out too.
  const seen = () => { cancelWatchdog(); clearFailure(); };
  el.addEventListener("loadedmetadata", seen);
  el.addEventListener("loadedmetadata", () => { if (playing) armStallWatchdog(); });
  el.addEventListener("playing", seen);
  el.addEventListener("playing", () => armStallWatchdog());
  el.addEventListener("progress", () => { stallProgressSeen = true; });
  // Disarm on anything that legitimately stops advancing time, so a healthy
  // paused/seeking/ended video never trips the watchdog.
  el.addEventListener("pause", cancelStallWatchdog);
  el.addEventListener("seeking", cancelStallWatchdog);
  el.addEventListener("seeked", () => { if (!el.paused) armStallWatchdog(); });
  el.addEventListener("ended", cancelStallWatchdog);
  if (playing) { loadingRequested = true; startWatchdog(); }

  const subs = new Set();
  let lastEmit = 0;
  const notify = (opts = {}) => {
    const now = Date.now();
    if (opts.throttle && now - lastEmit < 250) return; // ≤4Hz for timeupdate
    lastEmit = now;
    const s = controller.state();
    subs.forEach((fn) => { try { fn(s); } catch (_) { /* advisory */ } });
  };
  el.addEventListener("loadedmetadata", () => {
    const audioOnly = !el.videoWidth && !el.videoHeight;
    controller.capabilities = { ...controller.capabilities, audioOnly };
    notify();
  });
  el.addEventListener("play", () => notify());
  el.addEventListener("pause", () => notify());
  el.addEventListener("ended", () => notify());
  el.addEventListener("playing", () => notify());
  el.addEventListener("waiting", () => notify());
  el.addEventListener("volumechange", () => notify());
  el.addEventListener("ratechange", () => notify());
  el.addEventListener("timeupdate", () => notify({ throttle: true }));
  el.addEventListener("error", () => notify());

  const controller = {
    kind: "recording",
    root: el,
    capabilities: {
      play: true, pause: true, seek: true, duration: true, volume: true,
      mute: true, quality: false, rate: true, pip: true, fullscreen: true,
      live: false, audioOnly: false,
    },
    mount(container) {
      if (el.parentElement !== container) container.appendChild(el);
    },
    destroy() {
      cancelWatchdog();
      cancelStallWatchdog();
      loadingRequested = false;
      clearFailure();
      try {
        el.pause();
      } catch (_) {
        /* already detached */
      }
      el.removeAttribute("src");
      el.remove();
    },
    setMuted(next) {
      if (el.muted !== next) el.muted = next;
    },
    setVolume(v) {
      el.volume = Math.max(0, Math.min(1, v));
    },
    setQuality() {
      /* the file is the file */
    },
    repoint(next) {
      if (next && next.src && next.src !== el.getAttribute("src")) {
        cancelWatchdog(); cancelStallWatchdog(); clearFailure();
        loadingRequested = !!next.playing;
        el.src = next.src;
        if (loadingRequested) startWatchdog();
      }
    },
    isReady() {
      return true;
    },
    play() { loadingRequested = true; startWatchdog(); el.play().catch(() => {}); },
    pause() { loadingRequested = false; cancelWatchdog(); cancelStallWatchdog(); el.pause(); },
    togglePlay() { el.paused ? controller.play() : controller.pause(); },
    seek(sec) { try { el.currentTime = Math.max(0, sec); } catch (_) { /* advisory */ } },
    currentTime() { return el.currentTime; },
    duration() { return Number.isFinite(el.duration) ? el.duration : NaN; },
    setRate(r) { try { el.playbackRate = r; } catch (_) { /* advisory */ } },
    state() {
      return {
        ready: el.readyState >= 1,
        playing: !el.paused && !el.ended,
        buffering: el.readyState < 3 && !el.paused && !el.ended,
        ended: el.ended,
        muted: el.muted,
        volume: el.volume,
        currentTime: el.currentTime,
        duration: Number.isFinite(el.duration) ? el.duration : NaN,
        rate: el.playbackRate,
        quality: null,
        qualities: [],
        live: false,
        error: el.dataset.failed || null,
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

/// Load Twitch's embed SDK once, shared by every tile.
///
/// Memoised on the PROMISE, not a boolean: two tiles starting at the same
/// moment must share one in-flight load rather than injecting the script
/// twice. Rejection is sticky and harmless — the factory simply keeps
/// handing out iframe controllers.
let _twitchSdk = null;
function loadTwitchSdkOnce() {
  if (_twitchSdk) return _twitchSdk;
  _twitchSdk = new Promise((resolve, reject) => {
    if (window.Twitch && window.Twitch.Player) return resolve(window.Twitch);
    const el = document.createElement("script");
    el.src = "https://player.twitch.tv/js/embed/v1.js";
    el.async = true;
    el.onload = () =>
      window.Twitch && window.Twitch.Player
        ? resolve(window.Twitch)
        : reject(new Error("Twitch SDK loaded without a Player"));
    el.onerror = () => reject(new Error("Twitch SDK failed to load"));
    document.head.appendChild(el);
  });
  return _twitchSdk;
}

/// Pull the channel and parent back out of an embed URL we built ourselves.
function parseTwitchEmbed(url) {
  try {
    const u = new URL(url);
    return {
      channel: u.searchParams.get("channel") || "",
      parent: u.searchParams.get("parent") || embedParentHost(),
    };
  } catch (_) {
    return null;
  }
}

