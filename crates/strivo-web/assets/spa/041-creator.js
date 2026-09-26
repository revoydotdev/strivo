// ── The multi-track timeline ──────────────────────────────────────────
// Report item E03. A view over the compiled plan and nothing else: this
// module never decides what a composition is, it asks `POST
// /api/v1/creator/timeline/apply` and draws the `CompiledPlan` that comes
// back. An edit the surface could express but the compiler would refuse is
// therefore impossible by construction — there is no local edit path.
//
// Every coordinate here is an integer number of microseconds, the unit
// todo 22 defines. There is no seconds float and no millisecond column in
// this file, and `Number.isInteger` is asserted on every value read off the
// wire rather than trusted.
//
// Styles are injected at runtime from the DESIGN.md token names
// (`var(--…)`) rather than added to `assets/spa-css/`, which todo 17 owns.
// The tokens are the same ones the assembled sheet defines, so the timeline
// is themed by the same source of truth as everything else.

// The three named preview outcomes. A caller switches on `mode`; the prose
// beside it is for a person and is never parsed.
const TIMELINE_PREVIEW = {
  PICTURE_AND_AUDIO: "picture-and-audio",
  PICTURE_RENDER_ONLY_AUDIO: "picture-render-only-audio",
  NO_PICTURE: "no-picture",
};

// Which preview this browser can honestly offer.
//
// `env` is a plain record of *capabilities*, not of the browser, so the
// decision is a pure function that a test can drive with a browser that has
// no `AudioDecoder`, no `VideoDecoder`, or a codec the demuxer refused — none
// of which this repository's own Chromium can be asked to do honestly.
// `videoCodecDecodable` is tri-state on purpose: `null` means "not probed
// yet", which is a different answer from `false` and must not collapse into
// it. A `||` here would turn "unknown" into "false" and show a browser a
// failure it may not have.
function timelinePreviewCapability(env) {
  const noDecoder = (what, remedy) => ({
    mode: TIMELINE_PREVIEW.NO_PICTURE,
    picture: false,
    audibleAudio: false,
    waveform: false,
    reason: what,
    remedy,
  });
  if (env.videoDecoder !== true) {
    return noDecoder(
      "This browser has no WebCodecs VideoDecoder.",
      "Open the timeline in Chrome or Edge 94+, or Firefox 130+. StriVo renders the composition on the server either way."
    );
  }
  if (env.videoCodecDecodable === false) {
    return noDecoder(
      `The browser cannot decode this recording's video codec (${env.videoCodec || "unknown"}).`,
      "Render the composition on the server to see it, or import a copy the browser can decode."
    );
  }
  if (env.audioDecoder !== true) {
    // The Safari 26+ case the plan names: video codecs landed in 16.4, audio
    // codecs did not. Picture and waveform-from-peaks still work; audio is
    // render-only and the surface must say so rather than play nothing.
    return {
      mode: TIMELINE_PREVIEW.PICTURE_RENDER_ONLY_AUDIO,
      picture: true,
      audibleAudio: false,
      waveform: true,
      reason: "This browser cannot decode audio in a stream (no WebCodecs AudioDecoder).",
      remedy: "Audio is render-only here: mix and render to hear it.",
    };
  }
  return {
    mode: TIMELINE_PREVIEW.PICTURE_AND_AUDIO,
    picture: true,
    audibleAudio: true,
    waveform: true,
    reason: null,
    remedy: null,
  };
}

// The decision is exposed by name so a browser test can drive it with a
// capability record no real browser on this machine can be made to have (no
// `AudioDecoder`, a refused codec). The surface itself calls it directly, so
// this is not a test-only code path — it is the same function either way.
// In a PVR build this module is not assembled, so the global does not exist.
globalThis.timelinePreviewCapability = timelinePreviewCapability;

// What this browser actually has, read once. Feature detection rather than
// user-agent sniffing: the plan's constraint is a missing *API*, and a UA
// string is a claim about a browser rather than a measurement of one.
function detectTimelineEnvironment() {
  return {
    videoDecoder: typeof globalThis.VideoDecoder === "function",
    audioDecoder: typeof globalThis.AudioDecoder === "function",
    videoCodecDecodable: null,
    videoCodec: null,
  };
}


// The composition the surface starts from. A revision is a *value* the
// caller already holds, so the read needs nothing from a store this todo
// does not own: what the plan is made of arrives in the request.
const TIMELINE_FIXTURE_SOURCE = "/recordings/stream-01.mkv";
const TIMELINE_FIXTURE_RATE = { num: 30, den: 1 };

function timelineFixtureRequest() {
  const span = (startUs, endUs) => ({
    source: `locator:${TIMELINE_FIXTURE_SOURCE}`,
    timebase: TIMELINE_FIXTURE_RATE,
    range: { start: startUs, end: endUs },
    rate: { num: 1, den: 1 },
  });
  return {
    subject: "stream-01",
    revision: "rev-7",
    clips: [
      { cut: 0, span: span(0, 1_000_000), fade_in: 0, fade_out: 0 },
      { cut: 1, span: span(2_000_000, 3_500_000), fade_in: 0, fade_out: 0 },
    ],
    layers: [
      {
        id: "programme",
        kind: "video",
        transform: { scale: null, offset_px: [0, 0], z: 0 },
        colour: { transfer: null, primaries: null, matrix: null, full_range: null },
        clips: [0, 1],
        visible: true,
      },
    ],
    transitions: [
      { id: "cut-1", on_layer: "programme", kind: "dissolve", at: 900_000, duration: 200_000 },
    ],
    tracks: [
      { label: "programme", bus: "master", gain_db: 0, pan: 0, muted: false, clips: [0, 1] },
    ],
    effects: [],
    loudness: null,
    captions: { mode: "none", style_json: "{}", coordinate: "composition", safe_area: true, cues: [] },
    video: { filter_complex: "", colour: null },
    container: { extension: "mkv", video_codec: "libx264", audio_codec: "aac", stream_copy: false },
    profile: { name: "long-form", renderer: "ffmpeg", renderer_version: "n9.0.2" },
  };
}

function timelineSourceLengthUs() {
  return 10_000_000;
}


const timelineState = {
  // Starts holding a real request rather than null: the first read posts it,
  // and a null here would be a null dereference on the very first navigation
  // rather than a state anybody could see.
  request: timelineFixtureRequest(),
  plan: null,
  snapPoints: [],
  selected: -1,
  playheadUs: 0,
  undo: [],
  capability: null,
  busy: false,
  notice: null,
};

// One integer microsecond value read off the wire, or `null` for a refusal.
// A float here would be the second time representation the whole model exists
// to remove. It is a *state* rather than a throw because a non-integer is a
// server bug the creator must be able to read about, and a throw inside a
// paint leaves the page half-drawn with no explanation.
function timelineMicros(value, what) {
  if (!Number.isInteger(value)) {
    timelineState.notice = {
      kind: "malformed",
      text: `The server sent ${what} as ${value}, which is not a whole number of microseconds.`,
    };
    return null;
  }
  return value;
}

function timelineFrameUs() {
  const { num, den } = TIMELINE_FIXTURE_RATE;
  return Math.ceil((1_000_000 * den) / num);
}

async function timelineApi(edit) {
  return API.timelineApply({
    request: timelineState.request,
    base_revision: timelineState.request.revision,
    sources: [
      {
        source: `locator:${TIMELINE_FIXTURE_SOURCE}`,
        length_us: timelineSourceLengthUs(),
      },
    ],
    edit: edit || null,
  });
}

// Vendored libraries, loaded on first navigation and never statically: a
// static top-level import of ~685 KB of Mediabunny would sit in the initial
// parse path of every Creator page load, which is the whole thing the lazy
// load exists to avoid. A failure to load is a *state*, not an exception that
// leaves a blank panel.
let timelineVendor = null;
async function loadTimelineVendor() {
  if (timelineVendor) return timelineVendor;
  timelineVendor = (async () => {
    const out = { mediabunny: null, wavesurfer: null, error: null };
    try {
      const [mb, ws] = await Promise.all([
        import("/assets/vendor/mediabunny.min.mjs"),
        import("/assets/vendor/wavesurfer.esm.js"),
      ]);
      out.mediabunny = mb;
      // The ESM build's only export is a default; the class name lives in the
      // importing module, which is why the Rust route test checks the artifact
      // for `as default` and not for `WaveSurfer`.
      out.wavesurfer = ws.default || null;
    } catch (e) {
      out.error = e instanceof Error ? e.message : String(e);
    }
    return out;
  })();
  return timelineVendor;
}

function timelineStyles() {
  if (document.getElementById("tl-styles")) return;
  const style = document.createElement("style");
  style.id = "tl-styles";
  // Only DESIGN.md token names and structural values: no colour literal
  // appears here, so `scripts/check-design-tokens.mjs` stays at zero and the
  // timeline follows the theme the way the rest of the SPA does.
  style.textContent = `
.tl-wrap{display:grid;grid-template-columns:minmax(0,1fr);gap:var(--space-md,16px)}
.tl-toolbar{display:flex;flex-wrap:wrap;gap:var(--space-xs,8px);align-items:center}
.tl-track{position:relative;height:56px;border-radius:var(--radius-md,8px);
  background:var(--surface);border:1px solid var(--border);overflow:hidden}
.tl-lane{position:relative;height:100%}
/* A clip is a dark surface with an accent edge, not a block of accent with
   light text on it: #eef3f8 on --primary measures 3.79:1, which fails AA for
   13px text, and the only token-only fix is to stop putting body text on the
   accent. The accent still reads as the clip's identity, on the edge. */
.tl-clip{position:absolute;top:4px;bottom:4px;border-radius:var(--radius-sm,4px);
  background:var(--surface-overlay);color:var(--fg);
  border:1px solid var(--border-lighter);border-left:3px solid var(--primary);
  display:flex;align-items:center;padding:0 var(--space-xs,8px);cursor:grab;
  font-size:var(--fs-sm,13px);overflow:hidden;white-space:nowrap}
.tl-clip[data-selected="true"]{outline:2px solid var(--accent2);outline-offset:-2px}
.tl-clip:focus-visible{outline:2px solid var(--accent2);outline-offset:-2px}
.tl-clip[data-linked="true"]{border-style:dashed}
.tl-playhead{position:absolute;top:0;bottom:0;width:2px;background:var(--recording);pointer-events:none}
.tl-ruler{position:relative;height:20px;color:var(--fg-dim);font-size:var(--fs-xs,11px)}
.tl-state{padding:var(--space-md,16px);border-radius:var(--radius-md,8px);
  background:var(--surface);border:1px solid var(--border)}
.tl-state[data-kind="degraded"]{border-color:var(--secondary)}
.tl-state[data-kind="failed"]{border-color:var(--recording)}
.tl-notice{font-size:var(--fs-sm,13px);color:var(--fg-muted)}
.tl-kbd{display:inline-block;padding:0 4px;border-radius:var(--radius-sm,4px);
  border:1px solid var(--border);font-family:var(--font-mono,monospace)}
`;
  document.head.appendChild(style);
}

function timelineDegradedPanel(kind, reason, remedy) {
  return `<div class="tl-state" data-kind="${kind}" data-tl-state="${kind}" role="status">
    <strong>${htmlEscape(reason)}</strong>
    <p class="tl-notice">${htmlEscape(remedy)}</p>
  </div>`;
}
