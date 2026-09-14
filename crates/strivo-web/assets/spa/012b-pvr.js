// ── Home: channel detail (if selected) + recordings dashboard ─────────
// First-run gate (item 20): a fresh install with no platform connected gets
// a guided setup checklist instead of an empty/half-configured dashboard.
// Platform setup and recording storage are available from the Settings editor,
// so this screen can hand users directly to the relevant controls.
let firstRunDismissed = false;

function renderFirstRun(setup, context = captureRouteContext()) {
  if (!isRouteCurrent(context)) return;
  root.removeAttribute("aria-busy");
  const step = (done, label, detail) => `
    <div class="fr-step ${done ? "done" : "todo"}">
      <span class="fr-mark">${done ? "✓" : "○"}</span>
      <div class="fr-body">
        <div class="fr-label">${htmlEscape(label)}</div>
        <div class="fr-detail">${detail}</div>
      </div>
    </div>`;
  const plat = (name, ok) =>
    `<span class="fr-pill ${ok ? "ok" : ""}">${ok ? "✓" : "○"} ${htmlEscape(name)}</span>`;
  const anyPlatform =
    setup.twitch_configured || setup.youtube_configured || setup.patreon_configured;
  const recDir = setup.recording_dir || "(unset)";
  const chanCount = (setup.auto_record_channels || []).length;

  if (!mountPage(`
    <h1 class="page-title">Welcome to StriVo</h1>
    <p class="page-subtitle">Finish setup before the dashboard fills in.</p>
    <div class="cfg-card fr-card">
      ${step(
        anyPlatform,
        "1 · Connect a platform",
        `Open <a class="stg-linkbtn" href="#/settings/platforms">Settings → Platforms</a>
         to connect Twitch, YouTube, or Patreon. Then re-check below.
         <div class="fr-pills">${plat("Twitch", setup.twitch_configured)}
           ${plat("YouTube", setup.youtube_configured)}
           ${plat("Patreon", setup.patreon_configured)}</div>`,
      )}
      ${step(
        !!setup.recording_dir,
        "2 · Recording directory",
        `Where captures are written: <code>${htmlEscape(recDir)}</code>.
         <a class="stg-linkbtn" href="#/settings/recording">Edit it in Settings → Recording</a> if needed.`,
      )}
      ${step(
        chanCount > 0,
        "3 · Pick channels to record",
        `Use the <b>＋ Add</b> button (top bar) to find a channel and enable
         auto-record. ${chanCount} channel(s) configured so far.`,
      )}
      <div class="fr-actions">
        <button id="fr-recheck">↻ Re-check</button>
        <button id="fr-continue" class="primary">${anyPlatform ? "Continue to dashboard" : "Continue anyway"}</button>
      </div>
    </div>
  `, context)) return;
  setupChromeHandlers();
  document.getElementById("fr-recheck")?.addEventListener("click", () => renderHome(captureRouteContext()));
  document.getElementById("fr-continue")?.addEventListener("click", () => {
    firstRunDismissed = true;
    renderHome(captureRouteContext());
  });
}

async function renderHome(context = captureRouteContext()) {
  if (!mountRouteShell(context)) return;
  let setup = null;
  // These dashboard sources do not depend on one another.  Starting them
  // together removes the settings → channels/recordings → Patreon → schedule
  // waterfall that made a warm Home transition wait several RTTs.
  const patreonTask = typeof seedPatreon === "function" ? seedPatreon(context) : Promise.resolve();
  const scheduleTask = API.schedule();
  // Ancillary sections update after the dashboard is available.  A slow
  // Patreon integration or schedule endpoint must never delay Home.
  patreonTask.then((loaded) => {
    if (!loaded) return;
    hydrationLoaded.patreon = true;
    if (isRouteCurrent(context)) paintChannelList();
  }).catch(() => {});
  scheduleTask.then((result) => {
    if (!isRouteCurrent(context)) return;
    dashSchedule = result.schedule || [];
    hydrationLoaded.schedule = true;
    if (currentRoute() === "library") paintDashboard();
  }).catch(() => {});
  const [setupRes, chRes, recRes] = await Promise.allSettled([
    API.settings(), API.channels(), API.recordings(),
  ]);
  if (!isRouteCurrent(context)) return;
  if (setupRes.status === "fulfilled") setup = setupRes.value;
  else if (setupRes.reason?.message?.includes("unauthorized")) return;
  const anyPlatform =
    setup &&
    (setup.twitch_configured || setup.youtube_configured || setup.patreon_configured);
  if (setup && !anyPlatform && !firstRunDismissed) {
    renderFirstRun(setup, context);
    return;
  }
  // Refresh the channel + recordings caches that feed the left rail and
  // the dashboard. Both are cheap snapshots.
  //
  // Use Promise.allSettled so a transient failure on one side (e.g. the
  // daemon socket bouncing) doesn't drop the OTHER side's data into the
  // empty-rail state. Previously Promise.all rejected atomically and we
  // caught at the outer try/catch, leaving both caches stale — visually
  // that surfaced as "rail vanished" because the unauth check at the top
  // already returned for genuine 401s.
  if (chRes.status === "fulfilled") {
    channelCache = chRes.value.channels || [];
    hydrationLoaded.channels = true;
  } else if (chRes.reason && chRes.reason.message && chRes.reason.message.includes("unauthorized")) {
    return;
  }
  if (recRes.status === "fulfilled") {
    recCache = recRes.value.recordings || [];
    if (typeof seedVodDownloadStateFromRecCache === "function") seedVodDownloadStateFromRecCache();
    dashRecordings = recCache;
    hydrationLoaded.recordings = true;
    seedVodDownloadStateFromRecCache();
  } else if (recRes.reason && recRes.reason.message && recRes.reason.message.includes("unauthorized")) {
    return;
  }
  // A14: prune bulkStatus entries whose channel is no longer in the
  // current channelCache. Without this the bulkStatus map grew across
  // weeks of uptime as channels were added + removed.
  if (Object.keys(bulkStatus).length) {
    const liveIds = new Set(channelCache.map((c) => c.id));
    for (const id of Object.keys(bulkStatus)) {
      if (!liveIds.has(id)) delete bulkStatus[id];
    }
  }
  root.removeAttribute("aria-busy");

  const selected = selectedChannelKey
    ? [...channelCache, ...patreonState.creators].find(
        (c) => `${c.platform}:${c.id}` === selectedChannelKey,
      )
    : null;

  // The recordings dashboard (In progress / Recent / Upcoming) lives only on
  // the home view; opening a channel shows just its detail.
  const center = selected
    ? channelDetailHtml(selected)
    : `<div id="dash" data-dashboard-signature="${htmlEscape(dashboardSignature(false))}">${recordingsDashboardHtml(false)}</div>`;

  if (!mountPage(center, context)) return;
  setupChromeHandlers();

  if (selected) {
    wireChannelDetail(selected);
    loadChannelDetailData(selected);
  }
  wireDashboard();
}

// Repaint ONLY the recordings dashboard subtree (#dash) — never the chrome,
// left rail, or channel-detail iframe. Driven by high-frequency recording
// events so they don't reload the live preview or reset rail scroll.
function paintDashboard(dirtyIds = null) {
  const el = document.getElementById("dash");
  if (!el) return;
  // Progress events only change a card's live fields.  Do not replace this
  // subtree: it contains focusable play/stop controls and horizontal scroll
  // strips that users may be browsing while a capture is active.
  const signature = dashboardSignature(!!selectedChannelKey);
  if (el.dataset.dashboardSignature !== signature) {
    el.innerHTML = recordingsDashboardHtml(!!selectedChannelKey);
    el.dataset.dashboardSignature = signature;
    wireDashboard();
    return;
  }
  const cards = el.querySelectorAll("[data-dashboard-recording]");
  const wanted = dashboardCardIds(!!selectedChannelKey);
  const structureChanged = cards.length !== wanted.length ||
    Array.from(cards).some((card, index) => card.dataset.dashboardRecording !== wanted[index]);
  if (structureChanged) {
    el.innerHTML = recordingsDashboardHtml(!!selectedChannelKey);
    wireDashboard();
    return;
  }
  let patched = 0;
  for (const card of cards) {
    const recording = dashRecordings.find((r) => r.id === card.dataset.dashboardRecording);
    if (!recording) continue;
    if (!dirtyIds || dirtyIds.has(String(recording.id))) patchDashboardRecording(card, recording);
    patched++;
  }
  // A record absent from the normalized cache means lifecycle structure
  // changed while this paint was queued; the next lifecycle refresh repairs
  // it without disturbing cards that still have stable identities.
}

function dashboardCardIds(compact) {
  return [
    ...dashRecordings.filter((r) => isInProgress(r.state)),
    ...dashRecordings
      .filter((r) => !isInProgress(r.state))
      .sort((a, b) => recordingTime(b) - recordingTime(a))
      .slice(0, compact ? 12 : 24),
  ].map((r) => String(r.id));
}

function dashboardSignature(compact) {
  return JSON.stringify({
    cards: dashboardCardIds(compact),
    live: (channelCache || []).filter((c) => c.is_live).map((c) => `${c.platform}:${c.id}:${c.viewer_count || 0}`),
    upcoming: (dashSchedule || []).filter((s) => s.next_fire).map((s) => `${s.channel}:${s.next_fire}`),
  });
}

function patchDashboardRecording(card, recording) {
  const size = card.querySelector("[data-rec-size]");
  if (size) size.textContent = formatBytes(recording.bytes_written || 0);
  const state = card.querySelector("[data-rec-state]");
  if (state) state.innerHTML = isNoteworthyState(recordingDisplayState(recording))
    ? renderStatePill(recordingDisplayState(recording)) : "";
}

// ── Home dashboard (Jellyfin-style horizontal carousels) ─────────────
//
// Rows: Live Now → In Progress → Recently Finished → Upcoming.
// Each row is a horizontal-scroll strip. Recently Finished pills are
// click-to-play (per user request); Live Now cards deep-link to the
// /watch route focused on that stream.
function recordingsDashboardHtml(compact) {
  const inProgress = dashRecordings.filter((r) => isInProgress(r.state));
  // Sort before slicing. This previously took whatever order the daemon
  // returned, so "Recent" was only accidentally chronological and the slice
  // could drop newer rows in favour of older ones.
  const recent = dashRecordings
    .filter((r) => !isInProgress(r.state))
    .sort((a, b) => recordingTime(b) - recordingTime(a))
    .slice(0, compact ? 12 : 24);
  const upcoming = [...dashSchedule]
    .filter((s) => s.next_fire)
    .sort((a, b) => new Date(a.next_fire) - new Date(b.next_fire));
  const liveChannels = (channelCache || []).filter((c) => c.is_live);

  const schedPillEl = (s) => `
    <div class="media-pill">
      <div class="mp-thumb"></div>
      <div class="mp-info">
        <div class="mp-title">${htmlEscape(s.channel)}</div>
        <div class="mp-sub">${htmlEscape(new Date(s.next_fire).toLocaleString())}${s.duration ? ` · ${htmlEscape(s.duration)}` : ""}</div>
      </div>
      <div class="mp-meta"><span class="mp-badge micro">scheduled</span></div>
    </div>`;

  // Live-now card: thumbnail + channel name + viewer count + LIVE
  // chip. Whole card is a hash link to /watch?focus=<id>.
  const liveCardEl = (c) => {
    const thumb = liveThumbUrl(c);
    const focus = `${c.platform}:${c.id}`;
    const href = `#/watch?mode=focus&focus=${encodeURIComponent(focus)}`;
    const viewers = c.viewer_count != null ? formatCount(c.viewer_count) : "";
    return `
      <a class="live-card" href="${href}" data-live-focus="${htmlEscape(focus)}"
         title="Open ${htmlEscape(c.display_name || c.name)} in the multi-stream viewer">
        <div class="live-card-thumb">${thumb ? `<img loading="lazy" decoding="async" src="${htmlEscape(thumb)}" alt=""/>` : ""}<span class="live-card-badge micro">LIVE</span></div>
        <div class="live-card-meta">
          <span class="live-card-name">${htmlEscape(c.display_name || c.name)}</span>
          <span class="live-card-sub pg-cap-hint">${htmlEscape(c.platform)}${viewers ? ` · ${viewers}` : ""}</span>
        </div>
      </a>`;
  };

  const rowEl = (title, count, html, empty, klass = "") => `
    <section class="dash-row${klass ? " " + klass : ""}">
      <h2 class="dash-row-title">${title}${count != null ? ` <span class="dash-count micro">${count}</span>` : ""}</h2>
      <div class="dash-scroll">${html || `<div class="empty sm">${empty}</div>`}</div>
    </section>`;

  const heading = compact ? "" : `<h1 class="page-title">Home</h1>`;
  // Live Now hidden when zero live (avoids "No channels live" noise on
  // dashboards where the rail's offline-only state already conveys
  // that). Same for Upcoming when no schedule.
  const liveRow = liveChannels.length
    ? rowEl("Live Now", liveChannels.length, liveChannels.map(liveCardEl).join(""), "", "live-now-row")
    : "";
  const upcomingRow = upcoming.length
    ? rowEl("Upcoming", upcoming.length, upcoming.map(schedPillEl).join(""), "", "")
    : "";
  return `${heading}
    ${liveRow}
    ${rowEl("In progress", inProgress.length, inProgress.map((r) => recordingPillHtml(r, true)).join(""), "Nothing recording")}
    ${rowEl("Recent", null, recent.map((r) => recordingPillHtml(r, true)).join(""), "No recordings yet — start one from the rail.")}
    ${upcomingRow}`;
}

// Shared recording media-pill (used by the home dashboard + History): cover
// thumbnail + title + channel·date + state/size, with a Stop on active rows.
function recordingPillHtml(j, dashboard = false) {
  const when = j.started_at ? new Date(j.started_at).toLocaleString() : "—";
  const stop = isInProgress(j.state)
    ? `<button class="danger sm" data-action="stop" data-job-id="${htmlEscape(j.id)}">Stop</button>`
    : "";
  // FILE MISSING overlay on the thumbnail mirrors the Recordings page
  // treatment so the Library dashboard doesn't quietly hide broken
  // rows (audit U2).
  const missingOverlay = j.file_exists === false
    ? '<span class="mp-missing">FILE MISSING</span>'
    : "";
  // Twitch live-pull + auto-VOD-backfill produces two rows per
  // broadcast — surface a small chip when the source is the
  // backfill path so the user can tell them apart at a glance
  // (audit B5). source_url is set when the recording was created
  // via DownloadVod (the backfill path).
  const sourceBadge = j.source_url
    ? '<span class="mp-source" title="From Twitch/YouTube VOD backfill">VOD</span>'
    : "";
  // Finished recordings with a file → click-to-play; in-progress &
  // file-missing rows stay inert (they don't have a playable artefact).
  const playable = !isInProgress(j.state) && j.file_exists !== false;
  const playAttrs = playable
    ? ` data-action="play" data-job-id="${htmlEscape(j.id)}" role="button" tabindex="0"`
    : "";
  // "Finished" is the state of nearly every row here, so rendering a pill
  // for it spends a column of every card restating the default. Only states
  // that actually need attention get a pill; the rest read as unremarkable,
  // which is the point.
  const state = recordingDisplayState(j);
  const statePill = isNoteworthyState(state) ? renderStatePill(state) : "";
  const title = htmlEscape(niceTitle(j.stream_title) || j.channel_name || "(recording)");
  return `
    <div class="media-pill mp-card${j.file_exists === false ? " mp-broken" : ""}${playable ? " mp-clickable" : ""}"${dashboard ? ` data-dashboard-recording="${htmlEscape(j.id)}"` : ""}${playAttrs}>
      <div class="mp-title" title="${title}">${title} ${sourceBadge}</div>
      <div class="mp-thumb">${missingOverlay}<img class="mp-thumb-img" loading="lazy" decoding="async" alt=""
        src="/api/v1/recordings/${encodeURIComponent(j.id)}/thumb" onerror="this.remove()"></div>
      <div class="mp-foot">
        <span class="mp-channel">${htmlEscape(j.channel_name || "")}</span>
        <span class="mp-when" title="${htmlEscape(when)}">${htmlEscape(shortWhen(j.started_at))}</span>
        <span class="mp-spacer"></span>
        <span data-rec-state>${statePill}</span>
        <span class="mp-size" data-rec-size>${formatBytes(j.bytes_written || 0)}</span>
        ${stop}
      </div>
    </div>`;
}

/// Timestamp a recording sorts by. started_at is the only time the API
/// exposes for every row, so it is the sort key; guard against nulls so a
/// malformed row sinks rather than poisoning the comparator with NaN.
function recordingTime(r) {
  const t = r && r.started_at ? Date.parse(r.started_at) : NaN;
  return Number.isFinite(t) ? t : 0;
}

/// States worth spending pixels on. "Finished" is the expected outcome and
/// is conveyed well enough by the row simply being playable, so it earns no
/// pill. Everything else — failures, missing files, in-flight work — does.
/// Note this takes the object recordingDisplayState() returns, not a string.
function isNoteworthyState(state) {
  return (state && state.className) !== "finished";
}

/// Compact timestamp for dense rows: a time for today, "Wed 14:05" within
/// the week, "30 Jul" beyond it. The full locale string stays in the
/// tooltip. Seconds are never useful here and cost ~4 characters a row.
function shortWhen(iso) {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const now = new Date();
  const sameDay = d.toDateString() === now.toDateString();
  const hhmm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (sameDay) return hhmm;
  const days = (now - d) / 86400000;
  if (days < 7) return `${d.toLocaleDateString([], { weekday: "short" })} ${hhmm}`;
  return d.toLocaleDateString([], { day: "numeric", month: "short" });
}

function wireDashboard() {
  // Click-to-play on finished recording pills. Routes to the Player
  // tab with this recording loaded as the single tile — no inline
  // modal. fresh=1 forces a single-slot reset so a stale multi-tile
  // layout doesn't eat the click.
  document.querySelectorAll('.media-pill[data-action="play"]').forEach((pill) => {
    const open = () => {
      const id = pill.dataset.jobId;
      if (!id) return;
      window.location.hash = `#/watch?recording=${encodeURIComponent(id)}&fresh=1`;
    };
    pill.addEventListener("click", (e) => {
      if (e.target.closest("button, a, input")) return;
      open();
    });
    pill.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); }
    });
  });
  document.querySelectorAll('[data-action="stop"]').forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!(await confirmDialog("Stop this recording?", { ok: "Stop", danger: true })))
        return;
      await withBusy(btn, "Stopping…", async () => {
        await API.stopRecording(btn.dataset.jobId);
        Toast.success("Recording stopped");
        setTimeout(() => render().catch(() => {}), 500);
      }).catch((e) => Toast.error(`Stop failed: ${e.message}`));
    });
  });
}
