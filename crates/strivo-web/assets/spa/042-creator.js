// ── The timeline's editing surface ────────────────────────────────────
// Painting, keyboard operation and edit commit, split from 041-creator.js
// only to keep each file inside the size a reviewer can hold. Same module
// scope: the bundle concatenates them into one ES module, so these are
// ordinary top-level declarations of the same file.

function timelineClipLabel(clip) {
  // The source identity is untrusted text reaching a canvas-adjacent label and
  // a title attribute. It is escaped here and never interpreted, and it is
  // truncated: a 10 MiB "name" must not become a 10 MiB tooltip.
  const name = String(clip.span.source);
  return htmlEscape(name.length > 48 ? `${name.slice(0, 48)}…` : name);
}

function timelineRenderClips() {
  const plan = timelineState.plan;
  const total = plan.output_duration;
  if (!total) return '<div class="tl-state" data-tl-state="empty">This composition has no output.</div>';
  const width = timelineState.laneWidth || 960;
  const pc = (us) => (us / total) * width;
  const drawn = [];
  for (const [index, clip] of plan.clips.entries()) {
    const start = timelineMicros(clip.output.start, "clip output start");
    const end = timelineMicros(clip.output.end, "clip output end");
    if (start === null || end === null) {
      return timelineDegradedPanel(
        "malformed",
        "The server sent a clip boundary that is not a whole microsecond, so it cannot be placed.",
        "Nothing is drawn from it. The plan is the render's own; report the revision."
      );
    }
    const left = pc(start);
    const w = Math.max(2, pc(end) - pc(start));
    const row = `<button type="button" class="tl-clip" role="option"
        data-tl-clip="${index}" data-cut="${clip.cut}"
        data-start="${start}" data-end="${end}"
        data-selected="${index === timelineState.selected}"
        aria-selected="${index === timelineState.selected}"
        aria-label="Clip ${clip.cut}, ${timelineClock(start)} to ${timelineClock(end)}"
        title="${timelineClipLabel(clip)}"
        style="left:${left}px;width:${w}px">${timelineClipLabel(clip)}</button>`;
    drawn.push(row);
  }
  return drawn.join("");
}

function timelineClock(us) {
  const total = us / 1_000_000;
  return `${Math.floor(total / 60)}:${(total % 60).toFixed(3).padStart(6, "0")}`;
}

function timelineNotice() {
  if (!timelineState.notice) return "";
  return htmlEscape(timelineState.notice.text);
}

// The notice is a live region, so it is created ONCE by the route and patched
// in place. Re-rendering its container's innerHTML destroyed and recreated the
// region on every paint, which announces nothing: assistive tech sees a fresh
// empty node rather than a change.
function timelinePaintNotice() {
  const el = document.getElementById("tl-live");
  if (!el) return;
  const notice = timelineState.notice;
  const text = notice ? notice.text : "";
  if (el.textContent !== text) el.textContent = text;
  if (notice) el.dataset.tlNotice = notice.kind;
  else delete el.dataset.tlNotice;
  // `aria-busy` marks the lane, not the region: a busy ANCESTOR of a live
  // region tells assistive tech to hold the announcement back.
  const lane = document.getElementById("tl-clips");
  if (lane) lane.setAttribute("aria-busy", String(Boolean(timelineState.busy)));
}

// Repaint without destroying focus. Replacing the lane's innerHTML on every
// keystroke took the focused clip out of the document, so the *second* key
// press of any two-key sequence went to the body instead of the timeline —
// which is how a "keyboard-operable" surface ends up operable for exactly one
// key. Elements are reused when the clip set is unchanged and only their
// geometry and state are patched.
function timelinePaint() {
  const host = document.getElementById("tl-clips");
  if (!host) return;
  const plan = timelineState.plan;
  const reusable =
    plan &&
    host.children.length === plan.clips.length &&
    plan.clips.every((_, i) => host.children[i].dataset.tlClip === String(i));
  if (reusable) {
    timelinePatchClips(host, plan);
  } else {
    host.innerHTML = timelineRenderClips();
  }
  timelinePaintNotice();
  if (timelineState.plan) timelineSyncControls();
  const playhead = document.getElementById("tl-playhead");
  if (playhead && plan && plan.output_duration) {
    playhead.style.left = `${(timelineState.playheadUs / plan.output_duration) *
      (timelineState.laneWidth || 960)}px`;
  }
}

function timelinePatchClips(host, plan) {
  const total = plan.output_duration || 1;
  const width = timelineState.laneWidth || host.clientWidth || 960;
  plan.clips.forEach((clip, index) => {
    const el = host.children[index];
    if (!el) return;
    const start = clip.output.start;
    const end = clip.output.end;
    const selected = index === timelineState.selected;
    el.style.left = `${(start / total) * width}px`;
    el.style.width = `${Math.max(2, ((end - start) / total) * width)}px`;
    el.dataset.start = String(start);
    el.dataset.end = String(end);
    el.dataset.selected = String(selected);
    el.setAttribute("aria-selected", String(selected));
  });
}

// One edit, sent as an intent. The plan that comes back is the only thing
// painted, so the surface cannot drift from the render.
//
// `label` names the operation for a request that carries no `edit` — a caption
// change is a document edit, so it rides the same endpoint with `edit: null`
// and the notice would otherwise read "null applied".
async function timelineCommit(edit, label) {
  if (timelineState.busy) return;
  timelineState.busy = true;
  const what = label || (edit && edit.op) || "edit";
  // Announced before the await, not after: the round trip is the long part,
  // and an announcement that only appears once the work is over is not an
  // announcement of anything in progress.
  timelineSay({ kind: "compiling", text: `Compiling the plan — ${what}…` });
  const announced = timelineState.noticeSeq;
  timelinePaint();
  const before = timelineState.request;
  try {
    const result = await timelineApi(edit);
    timelineState.undo.push(before);
    // The server answers with the request it compiled, not with a description
    // of it. That is what lets a second edit build on the first without the
    // surface re-deriving the composition the server already decided.
    timelineState.request = result.request;
    timelineState.plan = result.plan;
    timelineState.snapPoints = result.snap_points_us || [];
    if (timelineState.noticeSeq === announced) timelineState.notice = result.clamps && result.clamps.length
      ? {
          kind: "clamped",
          text: `Clamped to the source: ${result.clamps
            .map((c) => `clip ${c.clip} ${c.edge} → ${timelineClock(c.applied_micros)}`)
            .join(", ")}`,
        }
      : { kind: "applied", text: `${what} applied.` };
  } catch (e) {
    if (timelineState.noticeSeq === announced) {
      timelineState.notice = { kind: "failed", text: e instanceof Error ? e.message : String(e) };
    }
  } finally {
    timelineState.busy = false;
    timelinePaint();
  }
}

// The response carries the plan; the request the surface holds is the
// pre-edit one, so a reload of the page re-asks rather than guessing. The
// server is the only place the edited request exists.
async function timelineReload() {
  const result = await timelineApi(null);
  timelineState.request = result.request;
  timelineState.plan = result.plan;
  timelineState.snapPoints = result.snap_points_us || [];
  timelineState.undo = [];
}

// The playhead pulled onto the nearest place the server says a drag snaps to,
// within half a frame. The snap points come from the compiler's own interval
// map, so this surface cannot invent an edge the plan does not have.
function timelineSnapPlayhead() {
  const points = timelineState.snapPoints || [];
  const tolerance = Math.floor(timelineFrameUs() / 2);
  let best = null;
  for (const point of points) {
    if (!Number.isInteger(point)) continue;
    const distance = Math.abs(point - timelineState.playheadUs);
    if (distance > tolerance) continue;
    // Ties go to the earlier point, matching the server's own rule.
    if (best === null || distance < Math.abs(best - timelineState.playheadUs) ||
        (distance === Math.abs(best - timelineState.playheadUs) && point < best)) {
      best = point;
    }
  }
  if (best !== null) {
    timelineState.playheadUs = best;
    timelineSay({ kind: "snapped", text: `Playhead snapped to ${timelineClock(best)}.` });
  }
  return best;
}

function timelineFocusSelected() {
  const sel = document.querySelector('[data-tl-clip][data-selected="true"]');
  if (sel) sel.focus();
}

function timelineUndo() {
  if (!timelineState.undo.length) {
    timelineState.notice = { kind: "empty", text: "Nothing to undo." };
    timelinePaint();
    return;
  }
  timelineState.request = timelineState.undo.pop();
  timelineReload().then(timelinePaint);
}

// Any notice written through here bumps a sequence. A compile takes time, and
// a creator can press something during it — the answer then arrives and
// silently replaces whatever they just asked for, so the export contract
// vanishes under a "split applied." and the surface looks broken. A commit
// only publishes its result if nothing has been announced since it started.
function timelineSay(notice) {
  timelineState.noticeSeq = (timelineState.noticeSeq || 0) + 1;
  timelineState.notice = notice;
}

function timelineExportNotice() {
  const plan = timelineState.plan;
  const cues = (timelineState.request.captions || {}).cues || [];
  timelineSay({
    kind: "export",
    text: `Export contract: ${plan.clips.length} clip(s), ${plan.output_duration} us, ` +
      `${cues.length} caption cue(s), revision ${plan.revision}.`,
  });
}

// Captioning is a document edit, not a plan edit: the cues live on the request
// the surface already holds, and the compiler resolves them through the
// interval map. A cue goes at the selected clip's own source start, which is
// inside a span the composition kept — the compiler refuses a cue inside a cut
// the composition deleted, and this placement cannot be one.
function timelineCaptionAt(index) {
  const clip = timelineState.plan.clips[index];
  if (!clip) return null;
  const at = clip.span.range.start;
  const cues = (timelineState.request.captions || {}).cues || [];
  return cues.find((cue) => cue.at === at) || null;
}

function timelineToggleCaption() {
  const index = timelineState.selected;
  if (index < 0 || !timelineState.plan.clips[index]) {
    timelineSay({ kind: "empty", text: "Select a clip to caption." });
    timelinePaint();
    return;
  }
  // Refuse BEFORE touching the request. This operation edits the document in
  // place rather than posting an intent, so a compile already in flight would
  // answer with its own echo of the old request and silently discard the cue —
  // a lost edit with no error anywhere.
  if (timelineState.busy) {
    timelineSay({ kind: "empty", text: "Still compiling — caption again in a moment." });
    timelinePaint();
    return;
  }
  const clip = timelineState.plan.clips[index];
  const at = clip.span.range.start;
  const cues = ((timelineState.request.captions || {}).cues || []).filter((cue) => cue.at !== at);
  const adding = !timelineCaptionAt(index);
  if (adding) cues.push({ at, duration: clip.span.range.end - at });
  timelineState.request.captions = {
    ...timelineState.request.captions,
    mode: cues.length ? "sidecar" : "none",
    coordinate: "source",
    cues,
  };
  timelineCommit(null, adding ? "caption" : "uncaption");
}

// The caption control is a toggle, so its state is `aria-pressed` — and the
// name follows the state, because "Caption" on an already-captioned clip
// promises an action that removes the caption.
function timelineSyncControls() {
  const button = document.getElementById("tl-caption");
  if (!button) return;
  const on = timelineState.selected >= 0 && Boolean(timelineCaptionAt(timelineState.selected));
  button.setAttribute("aria-pressed", String(on));
  button.setAttribute(
    "aria-label",
    on ? "Remove the caption from the selected clip" : "Caption the selected clip",
  );
}

// ── Pointer ──────────────────────────────────────────────────────────────
// A drag is the same intent a key posts, so the two cannot diverge. The three
// gestures are: scrub the playhead on the empty lane, slip a clip by its body,
// trim a clip by either edge. Every one of them has a key.
const TIMELINE_DRAG_SLOP = 4;
const TIMELINE_EDGE_PX = 8;

function timelineUsAtX(lane, clientX) {
  const rect = lane.getBoundingClientRect();
  const total = timelineState.plan ? timelineState.plan.output_duration : 0;
  if (!rect.width || !total) return 0;
  const raw = ((clientX - rect.left) / rect.width) * total;
  return Math.max(0, Math.min(total, Math.round(raw)));
}

function timelineOnPointerDown(event) {
  const plan = timelineState.plan;
  const lane = document.getElementById("tl-clips");
  if (event.button !== 0 || !plan || !lane) return;
  const clipEl = event.target.closest && event.target.closest("[data-tl-clip]");
  if (!clipEl) {
    timelineState.drag = { mode: "scrub", startX: event.clientX };
  } else {
    const rect = clipEl.getBoundingClientRect();
    timelineState.selected = Number(clipEl.dataset.tlClip);
    const edge =
      event.clientX - rect.left <= TIMELINE_EDGE_PX
        ? "in"
        : rect.right - event.clientX <= TIMELINE_EDGE_PX
          ? "out"
          : null;
    timelineState.drag = edge
      ? { mode: "trim", index: timelineState.selected, edge, startX: event.clientX }
      : {
          mode: "slip",
          index: timelineState.selected,
          startX: event.clientX,
          from: timelineUsAtX(lane, event.clientX),
        };
  }
  event.currentTarget.setPointerCapture(event.pointerId);
  timelinePaint();
}

function timelineOnPointerMove(event) {
  const drag = timelineState.drag;
  const lane = document.getElementById("tl-clips");
  if (!drag || !lane || !timelineState.plan) return;
  if (Math.abs(event.clientX - drag.startX) < TIMELINE_DRAG_SLOP) return;
  const at = timelineUsAtX(lane, event.clientX);
  if (drag.mode === "scrub") {
    timelineState.playheadUs = at;
    timelinePaint();
    return;
  }
  const clip = timelineState.plan.clips[drag.index];
  const el = lane.children[drag.index];
  if (!clip || !el) return;
  const total = timelineState.plan.output_duration || 1;
  const width = timelineState.laneWidth || lane.clientWidth;
  // Preview only: nothing is committed until the pointer is released, and the
  // plan the surface holds is untouched either way.
  if (drag.mode === "slip") {
    el.style.left = `${(clip.output.start / total) * width + ((at - drag.from) / total) * width}px`;
  } else {
    const start = drag.edge === "in" ? Math.min(at, clip.output.end - 1) : clip.output.start;
    const end = drag.edge === "out" ? Math.max(at, start + 1) : clip.output.end;
    el.style.left = `${(start / total) * width}px`;
    el.style.width = `${Math.max(2, ((end - start) / total) * width)}px`;
  }
}

function timelineOnPointerUp(event) {
  const drag = timelineState.drag;
  if (!drag) return;
  timelineState.drag = null;
  const lane = document.getElementById("tl-clips");
  if (event.currentTarget.hasPointerCapture(event.pointerId)) {
    event.currentTarget.releasePointerCapture(event.pointerId);
  }
  if (drag.mode === "scrub" || !lane) {
    timelinePaint();
    return;
  }
  const at = timelineUsAtX(lane, event.clientX);
  if (drag.mode === "slip") timelineCommit({ op: "slip", clip: drag.index, by: at - drag.from });
  else timelineCommit({ op: "trim", clip: drag.index, edge: drag.edge, to: at });
  // The clip keeps focus so the gesture can be finished or undone by key.
  timelineFocusSelected();
}

function timelineOnPointerCancel() {
  timelineState.drag = null;
  timelinePaint();
}

// Keyboard-first: every operation has a key, and the key path is the tested
// one. A pointer drag posts the same intents, so the two cannot diverge.
function timelineOnKeydown(event) {
  const clipEl = event.target.closest && event.target.closest("[data-tl-clip]");
  const plan = timelineState.plan;
  if (!plan || !plan.clips.length) return;
  const frame = timelineFrameUs();
  // Focusing a clip does NOT select it — the selection is what the arrow keys
  // move, and a surface that selected on focus made the first arrow press walk
  // away from the clip you had just tabbed to. The key acts on the focused
  // clip when there is one and on the selection otherwise, which is also what
  // lets it work from a toolbar button.
  const focused = clipEl ? Number(clipEl.dataset.tlClip) : -1;
  if (event.key === "Escape") {
    timelineState.selected = -1;
    timelineState.notice = null;
    timelineState.noticeSeq = (timelineState.noticeSeq || 0) + 1;
    timelinePaint();
    event.preventDefault();
    return;
  }
  if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
    // Shift is the slip modifier, handled below, so a held shift must not also
    // walk the selection.
    if (event.shiftKey) return;
    const step = event.key === "ArrowRight" ? 1 : -1;
    timelineState.selected = Math.max(
      0,
      Math.min(plan.clips.length - 1, timelineState.selected + step)
    );
    timelinePaint();
    timelineFocusSelected();
    event.preventDefault();
    return;
  }
  // The playhead is not a property of a clip, so these work wherever focus
  // sits inside the timeline.
  if (event.key === "," || event.key === ".") {
    const delta = event.key === "." ? frame : -frame;
    timelineState.playheadUs = Math.max(
      0,
      Math.min(plan.output_duration, timelineState.playheadUs + delta)
    );
    timelinePaint();
    event.preventDefault();
    return;
  }
  if (event.key === "g") {
    timelineSnapPlayhead();
    timelinePaint();
    event.preventDefault();
    return;
  }
  if (event.key === "e") {
    timelineExportNotice();
    timelinePaint();
    event.preventDefault();
    return;
  }
  if (event.key === "u") {
    timelineUndo();
    event.preventDefault();
    return;
  }
  if (event.key === "c") {
    timelineToggleCaption();
    event.preventDefault();
    return;
  }
  // Everything below acts on a clip, and says so rather than refusing
  // silently: a key that does nothing reads as a broken surface.
  const index = focused >= 0 ? focused : timelineState.selected;
  if (index < 0) {
    if ("sxyltc".includes(event.key) || (event.shiftKey && event.key.startsWith("Arrow"))) {
      timelineSay({ kind: "empty", text: "Select a clip first — Tab to the lane, then an arrow key." });
      timelinePaint();
    }
    return;
  }
  const clip = plan.clips[index];
  if (event.key === "[") {
    timelineState.playheadUs = timelineMicros(clip.output.start, "clip output start");
    timelinePaint();
    event.preventDefault();
  } else if (event.key === "]") {
    timelineState.playheadUs = timelineMicros(clip.output.end, "clip output end");
    timelinePaint();
    event.preventDefault();
  } else if (event.key === "s") {
    timelineCommit({ op: "split", clip: index, at: timelineState.playheadUs });
    event.preventDefault();
  } else if (event.key === "x") {
    const from = Math.min(timelineState.playheadUs, plan.output_duration);
    timelineCommit({ op: "ripple", range: { start: from, end: Math.min(from + frame * 10, plan.output_duration) } });
    event.preventDefault();
  } else if (event.key === "l") {
    const partner = (index + 1) % plan.clips.length;
    timelineCommit({ op: "link", clip: index, with: partner, linked: true });
    event.preventDefault();
  } else if (event.key === "t" || event.key === "y") {
    // The keyboard twin of dragging an edge: both edges, because a drag can
    // reach either and an operation reachable only by pointer is a gap.
    timelineCommit({
      op: "trim",
      clip: index,
      edge: event.key === "t" ? "out" : "in",
      to: timelineState.playheadUs,
    });
    event.preventDefault();
  } else if (event.shiftKey && (event.key === "ArrowRight" || event.key === "ArrowLeft")) {
    // The keyboard twin of dragging a clip's body. A slip slides the source
    // inside fixed out-points, so the composition does not move.
    timelineCommit({
      op: "slip",
      clip: index,
      by: event.key === "ArrowRight" ? frame : -frame,
    });
    event.preventDefault();
  }
}
