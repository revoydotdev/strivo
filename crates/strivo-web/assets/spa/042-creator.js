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
  return `<p class="tl-notice" data-tl-notice="${htmlEscape(timelineState.notice.kind)}"
    role="status">${htmlEscape(timelineState.notice.text)}</p>`;
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
  const noticeSlot = document.getElementById("tl-notice-slot");
  if (noticeSlot) noticeSlot.innerHTML = timelineNotice();
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
async function timelineCommit(edit) {
  if (timelineState.busy) return;
  timelineState.busy = true;
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
    timelineState.notice = result.clamps && result.clamps.length
      ? {
          kind: "clamped",
          text: `Clamped to the source: ${result.clamps
            .map((c) => `clip ${c.clip} ${c.edge} → ${timelineClock(c.applied_micros)}`)
            .join(", ")}`,
        }
      : { kind: "applied", text: `${edit.op} applied.` };
  } catch (e) {
    timelineState.notice = { kind: "failed", text: e instanceof Error ? e.message : String(e) };
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
    timelineState.notice = { kind: "snapped", text: `Playhead snapped to ${timelineClock(best)}.` };
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

function timelineExportNotice() {
  const plan = timelineState.plan;
  timelineState.notice = {
    kind: "export",
    text: `Export contract: ${plan.clips.length} clip(s), ${plan.output_duration} us, revision ${plan.revision}.`,
  };
}

// Keyboard-first: every edit has a key, and the key path is the tested one.
// A mouse drag commits the same intents, so the two cannot diverge.
function timelineOnKeydown(event) {
  const clipEl = event.target.closest && event.target.closest("[data-tl-clip]");
  const plan = timelineState.plan;
  if (!plan || !plan.clips.length) return;
  const frame = timelineFrameUs();
  if (event.key === "Escape") {
    timelineState.selected = -1;
    timelineState.notice = null;
    timelinePaint();
    event.preventDefault();
    return;
  }
  if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
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
  // The playhead is not a property of a clip, so these two work whether the
  // lane or a clip holds focus.
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
  // Everything below acts on one clip, so it needs one under focus. The
  // playhead keys above deliberately do not: a playhead is not a clip's.
  if (!clipEl) return;
  const index = Number(clipEl.dataset.tlClip);
  timelineState.selected = index;
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
  }
}
