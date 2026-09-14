// ── Channel detail (center) ──────────────────────────────────────────
function channelDetailHtml(c) {
  const key = `${c.platform}:${c.id}`;
  const isPatreon = c.platform === "Patreon";
  const liveBadge = c.is_live
    ? '<span class="status live">LIVE</span>'
    : '<span class="status">offline</span>';
  const actions = `
    <div class="actions">
      ${c.is_live ? `
        <button class="primary" data-action="record" data-channel-id="${c.id}"
                data-channel-name="${htmlEscape(c.name)}"
                data-display-name="${htmlEscape(c.display_name || c.name)}"
                data-platform="${c.platform}"
                data-thumbnail="${htmlEscape(c.thumbnail_url || "")}"
                data-stream-title="${htmlEscape(c.stream_title || "")}">● Record</button>
        <button data-action="record" data-from-start="true" data-channel-id="${c.id}"
                data-channel-name="${htmlEscape(c.name)}"
                data-display-name="${htmlEscape(c.display_name || c.name)}"
                data-platform="${c.platform}"
                data-thumbnail="${htmlEscape(c.thumbnail_url || "")}"
                data-stream-title="${htmlEscape(c.stream_title || "")}">● From start</button>
      ` : ""}
      ${!isPatreon ? `
        <button data-action="auto-record" data-channel-key="${key}"
                data-enabled="${!c.auto_record}">
          ${c.auto_record ? "Disable auto" : "Enable auto"}
        </button>
        ${bulkButton(c)}
        ${c.platform === "YouTube" ? `
          <button data-action="bulk-playlist" data-channel-id="${c.id}"
                  data-channel-name="${htmlEscape(c.display_name || c.name)}">⛁ Playlist…</button>` : ""}
        <button data-action="block-channel" data-channel-id="${c.id}"
                data-platform="${c.platform}"
                data-channel-name="${htmlEscape(c.display_name || c.name)}"
                title="Stop auto-grabbing this channel">⊘ Block</button>
      ` : ""}
    </div>`;

  // Section placeholders filled by loadChannelDetailData (async).
  let sections;
  if (isPatreon) {
    sections = `<div id="cd-posts" class="cd-section"></div>`;
  } else if (c.platform === "YouTube") {
    sections = `
      <div id="cd-playlists" class="cd-section"></div>
      <div id="cd-streams" class="cd-section"></div>
      <div id="cd-uploads" class="cd-section"></div>`;
  } else {
    sections = `<div id="cd-streams" class="cd-section"></div>`;
  }

  return `
    <div class="channel-detail">
      <div class="cd-header">
        <span class="platform-icon ${c.platform.toLowerCase()}">${c.platform}</span>
        <h1 class="cd-name">${htmlEscape(c.display_name || c.name)}</h1>
        ${liveBadge}
        ${c.viewer_count ? `<span class="cd-viewers">${formatCount(c.viewer_count)} viewers</span>` : ""}
        <button class="cd-close" data-action="cd-close" title="Close">×</button>
      </div>
      ${c.stream_title ? `<div class="stream-title">${htmlEscape(c.stream_title)}</div>` : ""}
      ${livePreviewHtml(c)}
      ${actions}
      ${sections}
    </div>`;
}

// Live preview when a live channel is opened (items 4 + 23). Progressive
// model: show a refreshing thumbnail poster first, upgrade to the platform's
// embed player on click (tap-to-play — avoids auto-spinning a player for every
// open and works on mobile). Patreon has no live concept (thumbnail-only).
/// Derive Twitch's `parent=` value from a host string.
///
/// Twitch accepts a HOSTNAME ONLY — a scheme or port produces "embed
/// misconfigured" / "refused to connect". It also rejects bare IPv4, so a
/// LAN address is rewritten to the matching `<ip-dashed>.nip.io`, which
/// resolves to the same IP through wildcard DNS.
///
/// This mirrors `strivo_multistream::embed_url`'s host handling
/// (crates/multistream/src/lib.rs) so every embed surface derives the same
/// parent. Three call sites used to do this independently and disagree:
/// the wall went through the Rust builder, while the channel-detail preview
/// used a bare `location.hostname` (no port stripping, no nip.io) and would
/// break for anyone reaching strivo over a LAN IP.
function embedParentHost(host) {
  const raw = host || location.host || "127.0.0.1";
  const bare = raw
    .replace(/^https?:\/\//, "")
    .split("/")[0]
    .split(":")[0];
  return /^\d+\.\d+\.\d+\.\d+$/.test(bare)
    ? `${bare.replace(/\./g, "-")}.nip.io`
    : bare;
}

/// Single builder for live embed URLs. `muted`/`autoplay` are omitted from
/// the URL entirely when left undefined, so callers that manage playback
/// through a player API do not bake a conflicting state into the src.
function buildEmbedUrl(platform, embedKey, opts = {}) {
  const { host, muted, autoplay } = opts;
  const key = encodeURIComponent(embedKey || "");
  if (platform === "Twitch") {
    let u = `https://player.twitch.tv/?channel=${key}` +
      `&parent=${encodeURIComponent(embedParentHost(host))}`;
    if (muted != null) u += `&muted=${!!muted}`;
    if (autoplay != null) u += `&autoplay=${!!autoplay}`;
    return u;
  }
  if (platform === "YouTube") {
    let u = `https://www.youtube.com/embed/live_stream?channel=${key}`;
    if (muted != null) u += `&mute=${muted ? 1 : 0}`;
    if (autoplay != null) u += `&autoplay=${autoplay ? 1 : 0}`;
    return u + "&playsinline=1";
  }
  return null;
}

function liveEmbedSrc(c) {
  const key = isYouTubePlatform(c.platform) ? c.id : c.name;
  return buildEmbedUrl(c.platform, key, { muted: true, autoplay: true });
}

// Substitute Twitch's {width}x{height} placeholders and cache-bust so the
// poster refreshes to a near-live frame.
function liveThumbUrl(c) {
  if (!c.thumbnail_url) return null;
  const sized = c.thumbnail_url
    .replace("{width}", "440")
    .replace("{height}", "248");
  return `${sized}${sized.includes("?") ? "&" : "?"}t=${Date.now()}`;
}

/// The channel-detail preview owns ONE player at a time, tracked here
/// rather than in playerState.controllers. That registry is pruned against
/// the multi-view layout, so a preview registered in it would be destroyed
/// the moment the wall reconciled — and vice versa.
let _cdPreviewController = null;
function destroyChannelDetailPreview() {
  if (_cdPreviewController) {
    try {
      _cdPreviewController.destroy();
    } catch (_) {
      /* best effort */
    }
    _cdPreviewController = null;
  }
}
/// Mount (or replace) the preview player for the currently-open channel.
function mountChannelDetailPreview(root) {
  destroyChannelDetailPreview();
  const mount = (root || document).querySelector(".cd-preview .cd-mount");
  if (!mount) return;
  try {
    _cdPreviewController = _playerControllerFactory(mount.dataset.kind || "twitch", {
      embedUrl: mount.dataset.embedBase || "",
      muted: true, // a preview that starts talking over you is hostile
      playing: true,
      volume: 0,
    });
    _cdPreviewController.mount(mount);
  } catch (_) {
    _cdPreviewController = null;
  }
}

function livePreviewHtml(c) {
  if (!c.is_live) return "";
  const src = liveEmbedSrc(c);
  const thumb = liveThumbUrl(c);
  // Stream id matches the backend's `{Platform:?}:{id}` shape so a
  // ▶ click on this poster routes to the Player and pre-fills the
  // single slot with this channel.
  const focus = `${c.platform}:${c.id}`;
  // No thumbnail but we have an embed → mount the player directly.
  // No `loading="lazy"`: this iframe is the live player. Chromium
  // viewport-throttles lazy iframes during the top-layer transition that
  // fullscreen triggers on cross-origin embeds, which stalls Twitch playback.
  if (!thumb && src) {
    // Mount point rather than a raw iframe, so this preview goes through the
    // same controller as the wall and inherits the vendor JS API (and the
    // iframe fallback when that fails to load).
    return `<div class="cd-preview" data-embed-src="${htmlEscape(src)}" data-focus="${htmlEscape(focus)}">
      <div class="ms-mount cd-mount" data-kind="${isYouTubePlatform(c.platform) ? "youtube" : "twitch"}"
           data-embed-base="${htmlEscape(src)}"></div>
    </div>`;
  }
  if (!thumb) return "";
  // Poster + (if embeddable) a play overlay that routes to the Player
  // tab with this channel pre-loaded — no in-place upgrade. The user
  // already 'hit play' so we don't make them pick the stream again.
  return `<div class="cd-preview poster" ${src ? `data-embed-src="${htmlEscape(src)}" data-focus="${htmlEscape(focus)}"` : ""}>
    <img id="cd-poster-img" src="${htmlEscape(thumb)}" alt="Live thumbnail" />
    ${src ? `<button class="cd-play" id="cd-play" aria-label="Open in Player">▶</button>` : ""}
  </div>`;
}

let cdPosterTimer = null;
function teardownLivePreview() {
  if (cdPosterTimer) {
    clearInterval(cdPosterTimer);
    cdPosterTimer = null;
  }
}

// Bug fix: cross-origin embed iframes (Twitch / YouTube) freeze when
// fullscreened from inside an .cd-preview parent that has
// overflow:hidden + aspect-ratio. The iframe can't match the parent's
// :fullscreen pseudo (different document scope), so we toggle a class
// on the parent via the document's fullscreenchange event and use
// .is-fullscreen + :has(iframe:fullscreen) in CSS to drop the clip.
function attachFullscreenBugfix(previewEl) {
  if (!previewEl || previewEl.dataset.fsBound === "1") return;
  previewEl.dataset.fsBound = "1";
  const onChange = () => {
    const fsEl = document.fullscreenElement || document.webkitFullscreenElement;
    const ours = !!fsEl && previewEl.contains(fsEl);
    previewEl.classList.toggle("is-fullscreen", ours);
  };
  document.addEventListener("fullscreenchange", onChange);
  document.addEventListener("webkitfullscreenchange", onChange);
  // W6: ESC exits a fullscreened embed cleanly. Browsers handle the
  // native fullscreen ESC themselves, but when the user has NOT
  // fullscreened we let ESC back out of the embed to the channel
  // detail's poster mode (parent toggles handled in wireChannelDetail).
  previewEl.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (document.fullscreenElement) document.exitFullscreen?.();
  });
}

function wireChannelDetail() {
  // Clear any preview refresh timer from a previously-open detail (item 23).
  teardownLivePreview();
  // Bring up this channel's preview player (no-op unless the poster-less
  // branch rendered a mount point).
  mountChannelDetailPreview(document);
  document.querySelector('[data-action="cd-close"]')?.addEventListener("click", () => {
    destroyChannelDetailPreview();
    teardownLivePreview();
    selectedChannelKey = null;
    render();
  });

  // Live preview: refresh the poster thumbnail every 30s, and upgrade to the
  // embed player on click (tap-to-play). Tears down when detail re-renders.
  // Cover BOTH the poster-mode preview and the always-embedded preview
  // — the freeze bug affects either form once the iframe is mounted.
  document.querySelectorAll(".cd-preview").forEach(attachFullscreenBugfix);
  const poster = document.querySelector(".cd-preview.poster");
  if (poster) {
    const img = poster.querySelector("#cd-poster-img");
    if (img) {
      const base = img.src.split(/[?&]t=/)[0];
      cdPosterTimer = setInterval(() => {
        if (document.hidden) return;
        // Only refresh while still on-screen (cheap visibility guard).
        if (!document.body.contains(img)) {
          teardownLivePreview();
          return;
        }
        img.src = `${base}${base.includes("?") ? "&" : "?"}t=${Date.now()}`;
      }, 30000);
    }
    const playBtn = poster.querySelector("#cd-play");
    const focus = poster.dataset.focus;
    if (playBtn && focus) {
      // ▶ on a channel poster routes straight to the Player tab with
      // this channel as the single-slot stream. User already clicked
      // play; don't open the 'pick a stream' picker (audit follow-up).
      playBtn.addEventListener("click", () => {
        teardownLivePreview();
        window.location.hash = `#/watch?focus=${encodeURIComponent(focus)}&fresh=1`;
      });
    }
  }
  document.querySelectorAll("[data-action=record]").forEach((btn) =>
    btn.addEventListener("click", () => startRecordingFromCard(btn.dataset)),
  );
  document.querySelectorAll("[data-action=auto-record]").forEach((btn) =>
    btn.addEventListener("click", () => toggleAutoRecord(btn.dataset)),
  );
  document.querySelectorAll("[data-action=bulk]").forEach((btn) =>
    btn.addEventListener("click", () => toggleBulk(btn.dataset)),
  );
  document.querySelectorAll("[data-action=bulk-playlist]").forEach((btn) =>
    btn.addEventListener("click", () => openPlaylistPicker(btn.dataset)),
  );
  document.querySelectorAll("[data-action=block-channel]").forEach((btn) =>
    btn.addEventListener("click", async () => {
      const d = btn.dataset;
      if (
        !(await confirmDialog(
          `Block ${d.channelName}? StriVo will stop auto-grabbing this channel's VODs.`,
          { ok: "Block", danger: true },
        ))
      )
        return;
      try {
        await API.blockAdd({ platform: d.platform, channel_id: d.channelId });
        Toast.success(`Blocked ${d.channelName}`);
      } catch (e) {
        Toast.error(`Block failed: ${e.message}`);
      }
    }),
  );
}

// Fetch + render the per-channel VOD lists. Patreon uses cached posts;
// YouTube/Twitch request VODs over IPC (result arrives via SSE) and also
// request playlists for YouTube.
function loadChannelDetailData(c) {
  if (c.platform === "Patreon") {
    renderPatreonPosts(c);
    return;
  }
  // Render from cache immediately if we have it, then (re)request.
  paintChannelVods(c.id, c.platform);
  API.requestChannelVods(c.id, c.platform).catch(() => {});
  if (c.platform === "YouTube") {
    API.requestPlaylists(c.id).catch(() => {});
  }
  // Don't hang on "Loading…" forever — if the channel-vods SSE answer
  // hasn't arrived in 15s (slow/failed platform fetch), show an error
  // state for whichever sections are still loading.
  const id = c.id;
  setTimeout(() => {
    if (!channelVods[id] && `${c.platform}:${id}` === selectedChannelKey) {
      for (const sid of ["cd-streams", "cd-uploads"]) {
        const el = document.getElementById(sid);
        if (el && el.textContent.includes("Loading")) {
          const title = sid === "cd-streams"
            ? "Past Broadcasts"
            : "Recent uploads";
          el.innerHTML = `<h2 class="cd-section-title">${title}</h2>` +
            `<div class="empty sm">Still loading VODs from the platform — this can take up to 15 seconds the first time. <a href="#" data-action="cd-retry">Retry now</a></div>`;
        }
      }
      document.querySelector('[data-action="cd-retry"]')?.addEventListener("click", (e) => {
        e.preventDefault();
        loadChannelDetailData(c);
      });
    }
  }, 15000);
}

function paintChannelVods(channelId, platform) {
  const vods = channelVods[channelId];
  const streamsEl = document.getElementById("cd-streams");
  const uploadsEl = document.getElementById("cd-uploads");
  // Look up channel context once so each VOD pill can carry the
  // channel_name + platform the download route needs. A9: also
  // search patreonState.creators so Patreon channels' VOD pills
  // resolve their display name + platform tag instead of falling
  // back to "".
  const channel = channelCache.find((c) => c.id === channelId)
    || (patreonState.creators || []).find((c) => c.id === channelId);
  const ctx = {
    channelName: (channel && (channel.display_name || channel.name)) || "",
    platform: platform || (channel && channel.platform) || "",
  };
  if (!vods) {
    if (streamsEl) streamsEl.innerHTML = vodSectionHtml("Past Broadcasts", null, ctx);
    if (uploadsEl) uploadsEl.innerHTML = vodSectionHtml("Recent uploads", null, ctx);
    return;
  }
  const streams = vods.filter((v) => v.kind === "LiveBroadcast");
  const uploads = vods.filter((v) => v.kind !== "LiveBroadcast");
  if (streamsEl) {
    streamsEl.innerHTML = vodSectionHtml("Past Broadcasts", streams, ctx);
  }
  if (uploadsEl) uploadsEl.innerHTML = vodSectionHtml("Recent uploads", uploads, ctx);
  wireVodDownloadButtons();
}

// Click handler for [data-action=vod-download] buttons inside the media-list
// pills. Optimistically flips to "downloading"; the SSE RecordingFinished
// handler flips to "downloaded" when a matching recording completes.
function wireVodDownloadButtons() {
  document.querySelectorAll("[data-action=vod-download]").forEach((btn) => {
    if (btn.dataset.wired === "1") return;
    btn.dataset.wired = "1";
    btn.addEventListener("click", async (e) => {
      e.preventDefault();
      e.stopPropagation();
      const url = btn.dataset.url;
      const channel_name = btn.dataset.channel;
      const platform = btn.dataset.platform;
      const post_title = btn.dataset.title || null;
      if (!url || vodDownloadState[url] === "downloading" || vodDownloadState[url] === "downloaded") {
        return;
      }
      vodDownloadState[url] = "downloading";
      setVodButtonState(btn, "downloading");
      try {
        // `data-via=patreon` routes through PatreonPull (its IPC arm
        // builds the Patreon-shaped output path + threads the patron
        // cookies); everything else lands on the generic DownloadVod
        // path. Both produce a RecordingJob with `source_url == url`,
        // so the state map + progress bar pipeline are identical.
        if (btn.dataset.via === "patreon") {
          await API.patreonPull({
            embed_url: url,
            creator_name: channel_name,
            post_title: post_title || "",
          });
        } else {
          await API.vodDownload({ url, channel_name, platform, post_title });
        }
        Toast.success(`Downloading: ${post_title || url}`);
        // The RecordingStarted SSE that follows will land in recCache with
        // source_url == this url; seedVodDownloadStateFromRecCache() then
        // confirms our optimistic state. When the recording reaches
        // Finished, the same path flips the pill to Downloaded by exact
        // source_url match — no FIFO guess.
      } catch (err) {
        // Roll back to idle so the user can retry.
        delete vodDownloadState[url];
        setVodButtonState(btn, "idle");
        Toast.error(`Download failed: ${err.message}`);
      }
    });
  });
  // Past Broadcasts: clicking a downloaded entry opens in-app playback
  // instead of the source platform — these are already-recorded VODs,
  // there's no reason to send the click to YouTube.
  document.querySelectorAll('[data-action="open-vod-recording"]').forEach((el) => {
    if (el.dataset.wired === "1") return;
    el.dataset.wired = "1";
    const open = () => {
      const id = el.dataset.jobId;
      if (id) openRecordingPlayer(id);
    };
    el.addEventListener("click", open);
    el.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(); }
    });
  });
}

// Walk recCache and reflect each recording whose source_url points at a VOD
// into vodDownloadState. Called whenever recCache is refreshed so the
// channel-detail view (and a fresh page reload) shows correct button state
// without any FIFO guess.
function seedVodDownloadStateFromRecCache() {
  for (const r of recCache) {
    if (!r.source_url) continue;
    if (r.state === "Finished") {
      vodDownloadState[r.source_url] = "downloaded";
    } else if (isInProgress(r.state)) {
      // Don't downgrade a "downloaded" entry if a stale in-progress row
      // sneaks in (rare, but be safe).
      if (vodDownloadState[r.source_url] !== "downloaded") {
        vodDownloadState[r.source_url] = "downloading";
      }
    }
  }
}

function setVodButtonState(btn, state) {
  btn.classList.remove("vod-dl-idle", "vod-dl-downloading", "vod-dl-downloaded");
  btn.classList.add(`vod-dl-${state}`);
  btn.disabled = state !== "idle";
  if (state === "downloading") {
    // Try to seed initial bar from any cached progress on the matching job.
    const url = btn.dataset.url;
    const job = recCache.find((r) => r.source_url === url);
    btn.innerHTML = vodProgressHtml(
      job && job.download_pct,
      job && job.download_eta_secs,
      job && job.download_rate_bps,
    );
  } else {
    btn.textContent = state === "downloaded" ? "Downloaded" : "Download";
  }
}

// Inner HTML for the in-flight download widget: gradient-filled bar +
// "NN% · Xm Ys left · R MB/s" label. Bar gradient runs amber → green so the
// rightmost fill colour shifts greener as the pull completes.
function vodProgressHtml(pct, etaSecs, rateBps) {
  // Mirrors renderStatePill's convention (below, ~3149): an unknown
  // percent is omitted from the label rather than shown as a literal
  // "0%" — the bar fill can still default to empty, but the text must
  // not lie about progress it hasn't actually observed yet.
  const hasPct = pct != null && Number.isFinite(pct);
  const p = hasPct ? Math.max(0, Math.min(100, Math.round(pct))) : 0;
  const eta = etaSecs == null ? "" : fmtEta(etaSecs);
  const rate = rateBps == null ? "" : `${formatBytes(rateBps)}/s`;
  const meta = [eta && `${eta} left`, rate].filter(Boolean).join(" · ");
  const label = hasPct ? `${p}%${meta ? " · " + meta : ""}` : (meta || "Downloading…");
  return `
    <span class="vod-dl-bar"><span class="vod-dl-fill" style="width:${p}%"></span></span>
    <span class="vod-dl-label">${label}</span>
  `;
}

function fmtEta(secs) {
  const s = Math.max(0, Math.floor(secs));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  if (m < 60) return r ? `${m}m ${r}s` : `${m}m`;
  const h = Math.floor(m / 60);
  const mm = m % 60;
  return mm ? `${h}h ${mm}m` : `${h}h`;
}

// Surgical DOM patch: find every visible VOD pill bound to this job's
// source_url and refresh its progress widget. Skips pills that have
// transitioned to "downloaded" (which the seed function will finalize).
function updateVodProgressDom(job) {
  if (!job || !job.source_url) return;
  if (vodDownloadState[job.source_url] !== "downloading") return;
  const esc = (typeof CSS !== "undefined" && CSS.escape) ? CSS.escape(job.source_url) : job.source_url.replace(/([\\"'])/g, "\\$1");
  const sel = `[data-action=vod-download][data-url="${esc}"]`;
  document.querySelectorAll(sel).forEach((btn) => {
    btn.innerHTML = vodProgressHtml(
      job.download_pct,
      job.download_eta_secs,
      job.download_rate_bps,
    );
  });
}

// Resolve a VOD/stream thumbnail URL, substituting Twitch's templated
// dimension placeholders ({width}/%{width}). VOD thumbnails are static.
function vodThumb(url) {
  if (!url) return null;
  return url
    .replace(/%?\{width\}/g, "440")
    .replace(/%?\{height\}/g, "248");
}
// Compact duration from a serde std::time::Duration ({secs, nanos}) or number.
function fmtDur(d) {
  const s = typeof d === "number" ? d : d && d.secs;
  if (!s || s <= 0) return "";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m`;
}

function vodSectionHtml(title, vods, ctx) {
  // Past Broadcasts gets the larger, centered treatment. Uploads keep the
  // smaller original style.
  const isPast = title === "Past Broadcasts";
  const titleCls = isPast ? "cd-section-title past-broadcasts" : "cd-section-title";
  if (vods === null) {
    return `<h2 class="${titleCls}">${title}</h2><div class="empty sm">Loading…</div>`;
  }
  if (vods.length === 0) {
    return `<h2 class="${titleCls}">${title}</h2><div class="empty sm">None</div>`;
  }
  const channelName = (ctx && ctx.channelName) || "";
  const platform = (ctx && ctx.platform) || "";
  // Jellyseerr/*arr-style horizontal media pills: thumbnail + rich info block,
  // with a sibling download button. The link wraps thumb+info; the button
  // sits next to it so we don't nest interactive elements.
  const rows = vods
    .map((v) => {
      const thumb = vodThumb(v.thumbnail_url);
      const date = (v.published_at || "").slice(0, 10);
      const dur = fmtDur(v.duration);
      const live = v.kind === "Live" || v.kind === "live";
      const meta = [date, dur].filter(Boolean).map(htmlEscape).join(" · ");
      // A VOD carries authoritative provenance (`channel_id` + `platform`).
      // The channel rail cache can be cold after a reconnect/navigation, so
      // requiring the optional display context used to hide Download for
      // uploaded videos even though the backend had everything it needed.
      const downloadChannel = channelName || v.channel_id || "";
      const downloadPlatform = platform || v.platform || "";
      const downloadable = !!(v.url && downloadChannel && downloadPlatform);
      // Past Broadcasts are already-recorded livestreams, not arbitrary
      // uploads — once downloaded, the pill should play the local
      // recording, never send the click out to YouTube. A match is a
      // finished recording whose source_url is this exact VOD's URL.
      const matchingJob = isPast
        ? recCache.find((r) => r.source_url === v.url && r.state === "Finished" && r.file_exists !== false)
        : null;
      const linkTag = isPast ? "div" : "a";
      const linkAttrs = isPast
        ? matchingJob
          ? `class="mp-link" data-action="open-vod-recording" data-job-id="${htmlEscape(matchingJob.id)}" role="button" tabindex="0"`
          : `class="mp-link mp-link-inert" style="cursor:default"`
        : (() => {
            const href = /^https?:\/\//i.test(v.url || "") ? htmlEscape(v.url) : "#";
            return `class="mp-link" href="${href}" target="_blank" rel="noopener"`;
          })();
      const state = vodDownloadState[v.url] || "idle";
      // For the downloading state, embed a live progress widget instead of
      // plain text. Seed pct/eta/rate from any matching cached job so a
      // re-render between SSE ticks doesn't reset the bar to 0%.
      let inner;
      if (state === "downloading") {
        const job = recCache.find((r) => r.source_url === v.url);
        inner = vodProgressHtml(
          job && job.download_pct,
          job && job.download_eta_secs,
          job && job.download_rate_bps,
        );
      } else if (state === "downloaded") {
        inner = "Downloaded";
      } else {
        inner = "Download";
      }
      const btn = downloadable
        ? `<button class="vod-dl vod-dl-${state}" data-action="vod-download"
              data-url="${htmlEscape(v.url)}"
              data-channel="${htmlEscape(downloadChannel)}"
              data-platform="${htmlEscape(downloadPlatform)}"
              data-title="${htmlEscape(v.title || "")}"
              ${state !== "idle" ? "disabled" : ""}>${inner}</button>`
        : "";
      // "Upload" was a confusing label on Past Broadcasts — every entry
      // there is a recorded livestream, not an upload; only flag the
      // ones that were actually captured live.
      const badge = live
        ? '<span class="mp-badge micro live">LIVE VOD</span>'
        : (isPast ? "" : '<span class="mp-badge micro">Upload</span>');
      return `
    <div class="media-pill">
      <${linkTag} ${linkAttrs}>
        <div class="mp-thumb">${thumb ? `<img class="mp-thumb-img" loading="lazy" decoding="async" alt="" src="${htmlEscape(thumb)}" onerror="this.remove()">` : ""}</div>
        <div class="mp-info">
          <div class="mp-title">${htmlEscape(niceTitle(v.title))}</div>
          <div class="mp-sub">${meta}</div>
        </div>
        <div class="mp-meta">${badge}</div>
      </${linkTag}>
      ${btn}
    </div>`;
    })
    .join("");
  return `<h2 class="${titleCls}">${title}</h2>
    <div class="media-list">${rows}</div>`;
}

// Patreon channel detail: render cached posts with a pull action.
function renderPatreonPosts(c) {
  const el = document.getElementById("cd-posts");
  if (!el) return;
  const posts = patreonState.posts[c.id] || [];
  const channelName = c.display_name || c.name;
  // Each post pill carries the same `.vod-dl` button the past-broadcasts
  // list uses; state is keyed by embed_url (== source_url on the resulting
  // RecordingJob), so seedVodDownloadStateFromRecCache surfaces in-flight /
  // completed pulls across navigation just like past broadcasts.
  const rows = posts.length
    ? posts
        .map((p) => {
          const thumb = p.thumbnail_url
            ? `<img class="mp-thumb-img" loading="lazy" decoding="async" alt="" src="${htmlEscape(p.thumbnail_url)}" onerror="this.remove()">`
            : "";
          const url = p.embed_url || "";
          const state = vodDownloadState[url] || "idle";
          const cachedJob = recCache.find((r) => r.source_url === url);
          const inner = state === "downloading"
            ? vodProgressHtml(
                cachedJob && cachedJob.download_pct,
                cachedJob && cachedJob.download_eta_secs,
                cachedJob && cachedJob.download_rate_bps,
              )
            : state === "downloaded" ? "Downloaded" : "Download";
          const btn = url
            ? `<button class="vod-dl vod-dl-${state}" data-action="vod-download"
                  data-via="patreon"
                  data-url="${htmlEscape(url)}"
                  data-channel="${htmlEscape(channelName)}"
                  data-platform="Patreon"
                  data-title="${htmlEscape(p.title)}"
                  ${state !== "idle" ? "disabled" : ""}>${inner}</button>`
            : "";
          return `
      <div class="media-pill">
        <div class="mp-link" style="cursor: default;">
          <div class="mp-thumb">${thumb}</div>
          <div class="mp-info">
            <div class="mp-title">${htmlEscape(p.title)}</div>
            <div class="mp-sub">${htmlEscape((p.published_at || "").slice(0, 10))}</div>
          </div>
          <div class="mp-meta"></div>
        </div>
        ${btn}
      </div>`;
        })
        .join("")
    : '<div class="empty sm">No video posts.</div>';
  el.innerHTML = `<h2 class="cd-section-title">Posts</h2><div class="media-list">${rows}</div>`;
  wireVodDownloadButtons();
}

// #74 — start/stop a per-channel bulk download.
async function toggleBulk(ds) {
  const active = ds.bulkActive === "true";
  try {
    await API.bulkDownload(ds.channelId, {
      channel_name: ds.channelName,
      platform: ds.platform,
      action: active ? "stop" : "start",
    });
    // Optimistic: flip local state; SSE bulk-progress will correct it.
    bulkStatus[ds.channelId] = active
      ? { done: 0, total: 0, active: false }
      : { done: 0, total: 0, active: true };
    Toast.success(
      active
        ? `Stopped bulk download — ${ds.channelName}`
        : `Bulk download started — ${ds.channelName}`,
    );
    if (currentRoute() === "library") render();
  } catch (e) {
    Toast.error(`Bulk download failed: ${e.message}`);
  }
}

// #74 / #73 — request the channel's playlists; the picker modal opens
// when the `playlist-list` SSE event arrives.
let pendingPlaylistChannel = null;
async function openPlaylistPicker(ds) {
  pendingPlaylistChannel = { id: ds.channelId, name: ds.channelName };
  try {
    await API.requestPlaylists(ds.channelId);
    showPlaylistModal({ loading: true, name: ds.channelName, playlists: [] });
  } catch (e) {
    Toast.error(`Couldn't load playlists: ${e.message}`);
  }
}
