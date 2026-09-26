      ].join("");
  }
}

// Each platform's wizard form spec: the fields it needs + a docs link
// the modal renders below the inputs. Kept tiny so it's obvious what
// each platform asks for; if it grows we lift it to its own module.
const PLATFORM_SPECS = {
  twitch: {
    title: "Configure Twitch",
    docsLabel: "Twitch Developer Console",
    docsUrl: "https://dev.twitch.tv/console/apps",
    fields: [
      { name: "client_id", label: "Client ID", type: "text", required: true },
      { name: "client_secret", label: "Client Secret", type: "password", required: true },
    ],
    notes: "Register Your Application → type 'Other', OAuth Redirect URL <code>http://localhost:8181/oauth/twitch</code>.",
  },
  youtube: {
    title: "Configure YouTube",
    docsLabel: "Google Cloud Console",
    docsUrl: "https://console.cloud.google.com/apis/credentials",
    fields: [
      { name: "client_id", label: "OAuth Client ID", type: "text", required: true },
      { name: "client_secret", label: "OAuth Client Secret", type: "password", required: true },
      { name: "cookies_path", label: "Cookies file (optional)", type: "text", placeholder: "/path/to/cookies.txt" },
      { name: "websub_callback_url", label: "WebSub callback URL (optional)", type: "url", placeholder: "https://your.tld/yt-websub" },
    ],
    notes: "Create OAuth 2.0 client ID, application type <em>Desktop app</em>. Cookies file enables age-restricted + member-only VODs.",
  },
  patreon: {
    title: "Configure Patreon",
    docsLabel: "Patreon Platform",
    docsUrl: "https://www.patreon.com/portal/registration/register-clients",
    fields: [
      { name: "client_id", label: "Client ID", type: "text", required: true },
      { name: "client_secret", label: "Client Secret", type: "password", required: true },
      { name: "cookies_path", label: "Cookies file (optional)", type: "text", placeholder: "/path/to/cookies.txt" },
    ],
    notes: "Cookies file is your logged-in patreon.com session — required to download VOD posts.",
  },
};

function openPlatformWizard(platform) {
  const spec = PLATFORM_SPECS[platform];
  if (!spec) return;
  const fieldHtml = spec.fields
    .map(
      (f) => `
        <label class="modal-field">
          <span class="modal-field-label">${htmlEscape(f.label)}${f.required ? " *" : ""}</span>
          <input class="modal-input" name="${htmlEscape(f.name)}" type="${htmlEscape(f.type)}"
            ${f.required ? "required" : ""}
            ${f.placeholder ? `placeholder="${htmlEscape(f.placeholder)}"` : ""} />
        </label>`,
    )
    .join("");
  const dlg = document.createElement("div");
  dlg.className = "modal-backdrop";
  dlg.innerHTML = `
    <form class="modal" role="dialog" aria-labelledby="pf-title">
      <header class="modal-head">
        <h2 id="pf-title">${htmlEscape(spec.title)}</h2>
        <button type="button" class="modal-close" aria-label="Close">×</button>
      </header>
      <div class="modal-body">
        ${fieldHtml}
        <p class="modal-notes">${spec.notes}
          <a href="${htmlEscape(spec.docsUrl)}" target="_blank" rel="noopener">${htmlEscape(spec.docsLabel)} →</a>
        </p>
      </div>
      <footer class="modal-foot">
        <button type="button" class="btn-ghost modal-cancel">Cancel</button>
        <button type="submit" class="btn-primary">Save</button>
      </footer>
    </form>`;
  document.body.appendChild(dlg);
  const close = () => dlg.remove();
  dlg.querySelector(".modal-close").addEventListener("click", close);
  dlg.querySelector(".modal-cancel").addEventListener("click", close);
  dlg.addEventListener("click", (e) => { if (e.target === dlg) close(); });
  dlg.querySelector(".modal").addEventListener("submit", async (e) => {
    e.preventDefault();
    const body = {};
    spec.fields.forEach((f) => {
      body[f.name] = e.target.elements[f.name].value.trim();
    });
    try {
      const name = spec.title.replace("Configure ", "");
      const res = await API.setPlatform(platform, body);
      // `applied` is false when the daemon isn't reachable, or when it started
      // without this platform configured. The credentials are on disk either
      // way, but they aren't live yet — say so rather than letting the green
      // badge imply the change took effect.
      if (res && res.applied === false) {
        Toast.info(`${name} saved — restart the daemon to start using it.`);
      } else {
        Toast.success(`${name} saved`);
      }
      close();
      // Re-render the Settings page so the status badge flips green.
      render();
    } catch (err) {
      Toast.error(`Couldn't save: ${err.message}`);
    }
  });
  // Autofocus the first field for a keyboard-driven flow.
  dlg.querySelector(".modal-input")?.focus();
}

// envOrDefault is a UI-side helper: the daemon doesn't expose env vars
// to the client (it shouldn't — it's behind auth on the local box, but
// minimising attack surface anyway). Until we add a /api/v1/env route
// in Phase 3, surface the placeholder.
function envOrDefault(_name, dflt) {
  return `<span class="muted">${htmlEscape(dflt)}</span>`;
}

// ── System (item 7) — version, daemon connectivity, severity-tiered
// health checks, disk gauge, tasks. (research §E)
async function renderSystem(context = captureRouteContext()) {
  if (!mountRouteShell(context)) return;
  const [health, storage, checksResp, settings] = await Promise.all([
    API.health().catch(() => null),
    API.storage().catch(() => null),
    API.healthChecks().catch(() => null),
    API.settings().catch(() => null),
  ]);
  if (!isRouteCurrent(context)) return;

  // Server-side health-check registry is the single source of truth
  // (roadmap item 13): {domain, name, severity, message, fix}.
  const serverChecks = (checksResp && checksResp.checks) || [
    { domain: "Network", name: "Daemon IPC", severity: "error", message: "not reachable", fix: "" },
  ];
  const checks = serverChecks.map((c) => ({ sev: c.severity, label: c.name, msg: c.message }));
  const activeRec = recCache.filter((r) => isInProgress(r.state)).length;

  const sevGlyph = { ok: "✓", warn: "▲", error: "✕" };
  // Group rows by domain so related checks (Storage / Platform Auth /
  // Network) sit together, each with its remediation hint.
  const domains = [...new Set(serverChecks.map((c) => c.domain))];
  const healthRows = domains
    .map((domain) => {
      const rows = serverChecks
        .filter((c) => c.domain === domain)
        .map(
          (c) => {
            // Surface a Re-authenticate link on platform-auth checks
            // (audit M9). When the token's healthy, the link is hidden;
            // either way the Settings wizard is one click away.
            const lc = c.name.toLowerCase();
            const reauth =
              c.domain === "Platform Auth" && ["twitch", "youtube", "patreon"].includes(lc)
                ? ` <a class="sys-reauth" href="#/settings/platforms" title="Open the ${lc} setup wizard">Re-authenticate →</a>`
                : "";
            return `
    <div class="sys-check ${c.severity}">
      <span class="sys-sev">${sevGlyph[c.severity] || "•"}</span>
      <span class="sys-label">${htmlEscape(c.name)}</span>
      <span class="sys-msg">${htmlEscape(c.message)}${c.fix ? ` <span class="sys-fix">— ${htmlEscape(c.fix)}</span>` : ""}${reauth}</span>
    </div>`;
          },
        )
        .join("");
      return `<div class="sys-domain"><h3 class="sys-domain-title">${htmlEscape(domain)}</h3>${rows}</div>`;
    })
    .join("");

  // Segmented disk-usage gauge (Task 2 — storage gauge near disk-space health row).
  // Three segments: recordings (accent), other filesystem usage (muted), free (transparent).
  const gauge = storage && storage.filesystem_total_bytes
    ? (() => {
        const total = storage.filesystem_total_bytes;
        const recBytes = storage.bytes_used_by_recordings || 0;
        const avail = storage.filesystem_avail_bytes || 0;
        const other = Math.max(0, total - avail - recBytes);
        const recPct = Math.min(100, (recBytes / total) * 100);
        const otherPct = Math.min(100 - recPct, (other / total) * 100);
        const freePct = Math.max(0, (avail / total) * 100);
        const warnClass = (avail / total) < 0.05 ? "gauge-crit" : (avail / total) < 0.15 ? "gauge-warn" : "";
        return `
          <div class="sys-gauge-wrap${warnClass ? ` ${warnClass}` : ""}" title="Disk usage — ${freePct.toFixed(0)}% free">
            <div class="sys-gauge-seg" style="width:${recPct.toFixed(1)}%;background:var(--accent)" title="Recordings: ${formatBytes(recBytes)}"></div>
            <div class="sys-gauge-seg" style="width:${otherPct.toFixed(1)}%;background:var(--muted);opacity:0.5" title="Other: ${formatBytes(other)}"></div>
          </div>
          <div class="sys-gauge-legend">
            <span class="sys-gauge-dot" style="background:var(--accent)"></span> Recordings: ${formatBytes(recBytes)}
            &ensp;<span class="sys-gauge-dot" style="background:var(--muted);opacity:0.6"></span> Other: ${formatBytes(other)}
            &ensp;<span class="sys-gauge-dot" style="background:var(--border-light)"></span> Free: ${formatBytes(avail)} of ${formatBytes(total)}
          </div>`;
      })()
    : '<div class="empty sm">Disk stats unavailable</div>';

  const worst = checks.some((c) => c.sev === "error")
    ? "error"
    : checks.some((c) => c.sev === "warn")
    ? "warn"
    : "ok";

  if (!mountPage(`
    <h1 class="page-title">System</h1>
    <p class="page-subtitle">StriVo v${health ? htmlEscape(health.version || "?") : "?"} ·
      overall <span class="cfg-badge ${worst === "ok" ? "ok" : worst === "warn" ? "warn" : "err"}">${worst}</span></p>
    <div class="cfg-grid">
      <section class="cfg-card">
        <h2 class="cfg-title">Health</h2>
        <div class="sys-checks">${healthRows}</div>
        ${gauge}
      </section>
      <section class="cfg-card" id="backup-card">
        <h2 class="cfg-title">Backup</h2>
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Config + jobs DB</span>
            <span class="task-cadence">on-demand snapshot</span>
          </div>
          <button id="backup-now" class="sm">＋ Backup now</button>
        </div>
        <div id="backup-list"><div class="empty sm">Loading backups…</div></div>
        <div id="retention-panel"></div>
        <p class="pg-cap-hint">After restoring, <a href="https://github.com/revoydotdev/strivo/blob/main/docs/DAEMON.md#lifecycle" target="_blank" rel="noopener">restart the daemon manually</a> to apply the restored files. A restore is only complete when every recorded source comes back; the report below names any that did not.</p>
      </section>
      <section class="cfg-card" id="blocklist-card">
        <h2 class="cfg-title">Blocklist</h2>
        <div id="blocklist-list"><div class="empty sm">Loading blocklist…</div></div>
      </section>
      <section class="cfg-card">
        <h2 class="cfg-title">Tasks</h2>
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Channel poll</span>
            <span class="task-cadence">every
              <input id="poll-interval" type="number" min="15" max="86400" step="5"
                     value="${settings ? settings.poll_interval_secs : 60}"
                     aria-label="Poll interval seconds" /> s
              <button id="poll-interval-save" class="sm" title="Apply poll interval">Save</button>
            </span>
          </div>
          <button id="task-poll-now" class="sm" title="Run the channel poll now">↻ Run now</button>
        </div>
        ${(settings && settings.schedule && settings.schedule.length
          ? settings.schedule
          : []
        )
          .map(
            (s) => `
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">⏱ ${htmlEscape(s.channel || "scheduled")}</span>
            <span class="task-cadence">${htmlEscape(s.cron || "")}${s.duration ? ` · ${htmlEscape(s.duration)}` : ""}</span>
          </div>
        </div>`,
          )
          .join("")}
        <div class="task-row">
          <div class="task-info">
            <span class="task-name">Active recordings</span>
            <span class="task-cadence">${activeRec} running${activeRec ? " · stop from the dashboard" : ""}</span>
          </div>
          <a class="sm" href="#/library">View</a>
        </div>
      </section>
    </div>
  `, context)) return;
  setupChromeHandlers();
  // Run-now duality: poll task enqueues the same command as the scheduled poll.
  document.getElementById("task-poll-now")?.addEventListener("click", async (e) => {
    await withBusy(e.currentTarget, "Polling…", async () => {
      await API.pollNow();
      Toast.success("Channel poll triggered");
    }).catch((err) => Toast.error(`Poll failed: ${err.message}`));
  });
  // Live-editable poll interval (item 14b) + inline field validation (item 25).
  document.getElementById("poll-interval-save")?.addEventListener("click", async (e) => {
    const input = document.getElementById("poll-interval");
    const raw = parseInt(input?.value, 10);
    if (!Number.isFinite(raw) || raw < 15) {
      input?.setAttribute("aria-invalid", "true");
      Toast.error("Poll interval must be at least 15 seconds");
      return;
    }
    input?.removeAttribute("aria-invalid");
    await withBusy(e.currentTarget, "Saving…", async () => {
      const r = await API.setPollInterval(raw);
      Toast.success(`Poll interval set to ${r.poll_interval_secs}s`);
    }).catch((err) => Toast.error(`Failed: ${err.message}`));
  });
  // Backup/restore (item 16).
  document.getElementById("backup-now")?.addEventListener("click", async (e) => {
    await withBusy(e.currentTarget, "Backing up…", async () => {
      const r = await API.backupCreate();
      Toast.success(`Backup created — ${r.name}`);
      await paintBackups();
    }).catch((err) => Toast.error(`Backup failed: ${err.message}`));
  });
  paintBackups();
  paintBlocklist();
}

async function paintBlocklist() {
  const el = document.getElementById("blocklist-list");
  if (!el) return;
  try {
    const r = await API.blocklist();
    const rows = r.blocklist || [];
    if (!rows.length) {
      el.innerHTML = '<div class="empty sm">Nothing blocked.</div>';
      return;
    }
    el.innerHTML = rows
      .map((b) => {
        const scope = b.vod_id ? `VOD ${htmlEscape(b.vod_id)}` : "whole channel";
        return `
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(b.platform)} · ${htmlEscape(b.channel_id)}</span>
          <span class="task-cadence">${scope}${b.reason ? ` · ${htmlEscape(b.reason)}` : ""}</span>
        </div>
        <button class="sm unblock" data-platform="${htmlEscape(b.platform)}"
                data-channel="${htmlEscape(b.channel_id)}" data-vod="${htmlEscape(b.vod_id || "")}">Unblock</button>
      </div>`;
      })
      .join("");
    el.querySelectorAll(".unblock").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const d = btn.dataset;
        try {
          await API.blockRemove({
            platform: d.platform,
            channel_id: d.channel,
            vod_id: d.vod || null,
          });
          Toast.success("Unblocked");
          paintBlocklist();
        } catch (e) {
          Toast.error(`Unblock failed: ${e.message}`);
        }
      });
    });
  } catch (e) {
    el.innerHTML = `<div class="empty sm">Could not load blocklist: ${htmlEscape(e.message)}</div>`;
  }
}

async function paintBackups() {
  const el = document.getElementById("backup-list");
  if (!el) return;
  try {
    const r = await API.backups();
    const rows = r.backups || [];
    if (!rows.length) {
      el.innerHTML = '<div class="empty sm">No backups yet.</div>';
      return;
    }
    el.innerHTML = rows
      .map(
        (b) => `
      <div class="task-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(b.name)}</span>
          <span class="task-cadence">${formatBytes(b.bytes || 0)} · ${(b.files || []).map(htmlEscape).join(", ")}</span>
        </div>
        <a class="sm" href="/api/v1/backups/${encodeURIComponent(b.name)}/download"
           download="strivo-backup-${htmlEscape(b.name)}.tar.gz"
           title="Download backup as tarball">Download</a>
        <button class="sm restore-backup" data-name="${htmlEscape(b.name)}">Restore</button>
      </div>`,
      )
      .join("");
    el.querySelectorAll(".restore-backup").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const name = btn.dataset.name;
        if (
          !(await confirmDialog(
            `Restore config + jobs DB from ${name}? This overwrites the current files; restart the daemon to apply.`,
            { ok: "Restore", danger: true },
          ))
        )
          return;
        try {
          const res = await API.backupRestore(name);
          reportRestore(el, name, res);
        } catch (err) {
          Toast.error(`Restore failed: ${err.message}`);
        }
      });
    });
  } catch (e) {
    el.innerHTML = `<div class="empty sm">Could not load backups: ${htmlEscape(e.message)}</div>`;
  }
}

// A restore is only a success when the daemon says every recorded project came
// back whole. Three answers, kept apart on purpose:
//
//   complete            — every recorded source came back, rows and bytes.
//   incomplete + report — the daemon named what is missing; show it.
//   no report           — the backup carried no project record, so nothing about
//                         projects was checked. That is NOT "nothing is wrong",
//                         and the previous wording ("Restored a, b") said so.
//
// The green toast is reachable only from the first branch. A restore that
// reported success for a project whose sources did not come back is this
// function's one unforgivable bug, so the fallback branch is not a default —
// the success text is the thing that had to be earned.
function reportRestore(listEl, name, res) {
  const report = res && res.report;
  if (report && report.complete === true) {
    const n = (report.projects || []).length;
    // "all 0 projects came back whole" is a green that asserts nothing, so the
    // empty case gets its own words.
    Toast.success(
      n
        ? `Restored ${(res.restored || []).join(", ")} — all ${n} project${n === 1 ? "" : "s"} came back whole`
        : `Restored ${(res.restored || []).join(", ")} — this backup held no projects to verify`,
    );
    listEl.scrollIntoView({ block: "nearest" });
    return;
  }
  if (!report) {
    Toast.error(
      `Restored ${(res.restored || []).join(", ") || "nothing"} from ${name} — this backup has no project record, so nothing about projects was verified`,
    );
    return;
  }
  // Incomplete, and the whole point: say which, by name, never as success.
  const missing = [];
  for (const project of report.projects || []) {
    for (const source of project.sources || []) {
      if (source.outcome && source.outcome !== "restored") {
        missing.push({ project, source });
      }
    }
  }
  Toast.error(
    missing.length
      ? `Restored files from ${name}, but ${missing.length} source${missing.length === 1 ? "" : "s"} did not come back — not a complete restore`
      : `Restored files from ${name}, but the restore is not verified complete`,
  );
  const rows = missing
    .map(({ project, source }) => {
      const why = {
        missing_row: "row missing from the restored library",
        unresolved: "row restored, bytes not found — relink it",
        identity_mismatch: "different content came back under the same id",
      }[source.outcome] || source.outcome;
      return `
        <div class="sys-check error" data-restore-missing="1">
          <span class="sys-sev">✕</span>
          <span class="sys-label">${htmlEscape(source.title || source.id)}</span>
          <span class="sys-msg">${htmlEscape(project.name)} — ${htmlEscape(why)}</span>
        </div>`;
    })
    .join("");
  const firstProject = (report.projects || []).find((p) => p.present && p.id);
  if (firstProject) {
    if (document.getElementById("retention-panel")) {
      paintRetention(firstProject.id, firstProject.name);
    }
  }
  listEl.insertAdjacentHTML(
    "afterend",
    `<div class="sys-domain" id="restore-report" role="alert">
       <h3 class="sys-domain-title">Restore from ${htmlEscape(name)}: ${missing.length} source${missing.length === 1 ? "" : "s"} did not come back</h3>
       ${rows || '<div class="sys-check warn"><span class="sys-sev">▲</span><span class="sys-msg">No project was verified complete.</span></div>'}
     </div>`,
  );
}

// Retention: what a cleanup would drop, reported before anything is dropped.
// Driven entirely by a project's id, which only a restore report supplies, so a
// PVR install — which has no projects and answers 404 with an empty body — never
// reaches this code path.
async function paintRetention(projectId, projectName) {
  const el = document.getElementById("retention-panel");
  if (!el) return;
  el.innerHTML = '<div class="empty sm">Loading cleanup plan…</div>';
  const render = (plan) => {
    const head = `<h3 class="sys-domain-title">Cleanup for ${htmlEscape(projectName || "this project")}</h3>`;
    const entries = (plan && plan.entries) || [];
    if (!entries.length) {
      el.innerHTML = head + '<div class="empty sm">This project has no outputs to review.</div>';
      return;
    }
    el.innerHTML =
      head +
      entries
        .map(
          (e) => `
        <div class="sys-check ${e.protected ? "warn" : "ok"}" data-retention-action="${htmlEscape(e.action)}">
          <span class="sys-sev">${e.protected ? "▲" : "✓"}</span>
          <span class="sys-label">${htmlEscape(e.kind || "output")}</span>
          <span class="sys-msg">${htmlEscape(e.explain || e.reason || "")}${
            e.bytes ? ` · ${formatBytes(e.bytes)}` : ""
          }</span>
        </div>`,
        )
        .join("") +
      (plan.collectable
        ? `<div class="task-row">
             <div class="task-info">
               <span class="task-name">${plan.collectable} unreferenced</span>
               <span class="task-cadence">Referenced and original material is never collected.</span>
             </div>
             <button id="retention-collect" class="sm" data-project="${htmlEscape(plan.project)}">Collect</button>
           </div>`
        : '<p class="pg-cap-hint">Nothing here is unreferenced, so there is nothing to collect.</p>');
    document.getElementById("retention-collect")?.addEventListener("click", async (e) => {
      if (
        !(await confirmDialog(
          `Remove ${plan.collectable} unreferenced output${plan.collectable === 1 ? "" : "s"} and their files? Originals and anything referenced are kept.`,
          { ok: "Collect", danger: true },
        ))
      )
        return;
      await withBusy(e.currentTarget, "Collecting…", async () => {
        const done = await API._fetch("/creator/retention/collect", {
          method: "POST",
          body: { project: plan.project, scope: "project" },
        });
        const n = (done.removed || []).length;
        if (n) Toast.success(`Removed ${n} unreferenced output${n === 1 ? "" : "s"}`);
        else Toast.error("Nothing was removed — the plan had changed; re-read it");
        render(done.plan);
      }).catch((err) => Toast.error(`Cleanup failed: ${err.message}`));
    });
  };
  try {
    render(
      await API._fetch(
        `/creator/retention/plan?project=${encodeURIComponent(projectId)}&scope=project`,
      ),
    );
  } catch (e) {
    el.innerHTML = `<div class="empty sm">No cleanup surface here: ${htmlEscape(e.message)}</div>`;
  }
}

// ── Logs viewer ──────────────────────────────────────────────────────
// Tails the rolling log file. Level dropdown + per-source filter chips +
// free-text search + multi-line entry collapse (audit U5, M7, R5).
let logsLevel = "info";
let logsQuery = "";
let logsSourceFilter = ""; // crate/module substring; "" = all sources
let logsFollow = localStorage.getItem("strivo-logs-follow") === "1";
let logsRegex = localStorage.getItem("strivo-logs-regex") === "1";
let logsFollowTimer = null;

// A "log entry" is a starting line (parsable timestamp + level) plus any
// following indented/JSON-blob continuation lines. We collapse those
// continuation lines into a single click-to-expand block so YouTube
// quota 403s stop dominating the viewport.
function parseLogEntries(lines) {
  const entries = [];
  const startRe = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}/;
  for (const raw of lines) {
    if (startRe.test(raw)) {
      entries.push({ head: raw, tail: [] });
    } else if (entries.length) {
      entries[entries.length - 1].tail.push(raw);
    } else {
      entries.push({ head: raw, tail: [] });
    }
  }
  return entries;
}

// Pull a coarse "source" tag (crate::module) out of a log line head.
function logSource(line) {
  const m = line.match(/\s+(strivo_[a-zA-Z_]+(::[a-zA-Z_]+)*)/);
  return m ? m[1] : "";
}

async function renderLogs(context = captureRouteContext()) {
  if (!mountRouteShell(context)) return;
  const levels = ["error", "warn", "info", "debug", "trace"];
  const options = levels
    .map((l) => `<option value="${l}"${l === logsLevel ? " selected" : ""}>${l.toUpperCase()}</option>`)
    .join("");
  // Stop any prior tail-follow timer before mounting the page (route
  // navigation, theme change, hot reload, etc.).
  if (logsFollowTimer) { clearInterval(logsFollowTimer); logsFollowTimer = null; }
  if (!mountPage(`
    <h1 class="page-title">Logs</h1>
    <div class="logs-toolbar">
      <label>Min level <select id="logs-level">${options}</select></label>
      <input id="logs-search" class="logs-search" type="search"
             placeholder="${logsRegex ? "Regex (case-insensitive)…" : "Search log text…"}"
             value="${htmlEscape(logsQuery)}" />
      <label class="logs-daterange" title="Filter log lines by ISO-8601 timestamp prefix. Inclusive of the bounds.">
        from <input id="logs-from" class="logs-date" type="datetime-local" step="1" value="${htmlEscape(logsFrom || "")}"/>
        to <input id="logs-to" class="logs-date" type="datetime-local" step="1" value="${htmlEscape(logsTo || "")}"/>
        <button id="logs-clear-range" class="sm" type="button" title="Clear date range">✕</button>
      </label>
      <label class="logs-toggle" title="Search as case-insensitive regex">
        <input type="checkbox" id="logs-regex" ${logsRegex ? "checked" : ""}/> regex
      </label>
      <label class="logs-toggle" title="Auto-refresh every 4s and pin scroll to bottom">
        <input type="checkbox" id="logs-follow" ${logsFollow ? "checked" : ""}/> follow
      </label>
      <span id="logs-sources" class="logs-sources"></span>
      <button id="logs-refresh" class="sm" title="Reload now">↻ Refresh</button>
      <button id="logs-copy" class="sm" title="Copy filtered log lines to clipboard">⧉ Copy</button>
      <button id="logs-download" class="sm" title="Save filtered log lines as a .log file">⬇ Download</button>
      <span id="logs-file" class="logs-file"></span>
    </div>
    <div id="logs-output" class="logs-output" aria-live="polite">Loading…</div>
  `, context)) return;
  setupChromeHandlers();

  async function load() {
    const out = document.getElementById("logs-output");
    const fileEl = document.getElementById("logs-file");
    try {
      const r = await API.logs(logsLevel, 500);
      if (!isRouteCurrent(context)) return;
      const allLines = r.lines || [];
      const allEntries = parseLogEntries(allLines);
      // Build the source-filter chip set from what's currently in view.
      const sources = [...new Set(allEntries.map((e) => logSource(e.head)).filter(Boolean))].sort();
      const chips = document.getElementById("logs-sources");
      if (chips) {
        chips.innerHTML = ['<button class="logs-chip" data-src="">all</button>']
          .concat(
            sources.map(
              (s) =>
                `<button class="logs-chip${s === logsSourceFilter ? " is-active" : ""}" data-src="${htmlEscape(s)}">${htmlEscape(s.replace(/^strivo_/, ""))}</button>`,
            ),
          )
          .join("");
        chips.querySelectorAll(".logs-chip").forEach((b) => {
          b.addEventListener("click", () => {
            logsSourceFilter = b.dataset.src || "";
            load();
          });
        });
      }
      const q = logsQuery.trim();
      // Regex compile once per load. Invalid pattern → tooltip via input
      // border colour + skip the filter (don't silently exclude
      // everything when the user mistypes).
      let pattern = null;
      let patternBad = false;
      if (q && logsRegex) {
        try { pattern = new RegExp(q, "i"); }
        catch (_) { patternBad = true; }
      }
      const searchInput = document.getElementById("logs-search");
      if (searchInput) searchInput.classList.toggle("logs-search-bad", patternBad);
      const qLower = q.toLowerCase();
      const filtered = allEntries.filter((e) => {
        if (logsSourceFilter && !e.head.includes(logsSourceFilter)) return false;
        if (!logInRange(e.head)) return false;
        if (!q || patternBad) return true;
        const hay = e.head + "\n" + e.tail.join("\n");
        if (pattern) return pattern.test(hay);
        return hay.toLowerCase().includes(qLower);
      });
      // Linkify UUID-shaped trace ids in escaped head HTML so users can
      // click one to filter the view. We do the escape first, then
      // splice in <a> elements; safe because the UUID regex contains
      // no HTML metachars.
      const TRACE_RE = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi;
      const linkifyTraces = (escapedHead) =>
        escapedHead.replace(TRACE_RE, (id) =>
          `<a class="logs-trace" href="#" data-trace="${id}" title="Filter by trace id ${id}">${id}</a>`,
        );
      out.innerHTML = filtered.length
        ? filtered
            .map((e) => {
              const head = linkifyTraces(htmlEscape(e.head));
              if (!e.tail.length) return `<div class="log-line">${head}</div>`;
              const tail = htmlEscape(e.tail.join("\n"));
              return `<details class="log-line log-multi"><summary>${head} <span class="log-more">+${e.tail.length}</span></summary><pre>${tail}</pre></details>`;
            })
            .join("")
        : "<div class='empty sm'>No log lines match the current filters.</div>";
      out.querySelectorAll(".logs-trace").forEach((a) => {
        a.addEventListener("click", (e) => {
          e.preventDefault();
          logsTraceId = a.dataset.trace || "";
          logsQuery = logsTraceId;
          const si = document.getElementById("logs-search");
          if (si) si.value = logsTraceId;
          load();
          Toast.success(`Filtering by trace id ${logsTraceId.slice(0, 8)}…`);
        });
      });
      if (fileEl) fileEl.textContent = r.file ? `· ${r.file} · ${filtered.length}/${allEntries.length} entries` : "";
      // Pin scroll to bottom in follow mode UNLESS the user has
      // intentionally scrolled up (we treat "within 80px of bottom"
      // as still-following so the auto-pin doesn't fight a fast scroll
      // recovery).
      const userPaused = out.scrollHeight - out.scrollTop - out.clientHeight > 80;
      if (!logsFollow || !userPaused) out.scrollTop = out.scrollHeight;
      // Stash for Copy/Download handlers.
      logsLastFilteredText = filtered
        .map((e) => e.tail.length ? `${e.head}\n${e.tail.join("\n")}` : e.head)
        .join("\n");
      logsLastFile = r.file || "strivo.log";
    } catch (e) {
      if (!isRouteCurrent(context)) return;
      out.textContent = `Failed to load logs: ${e.message}`;
    }
  }
  document.getElementById("logs-level")?.addEventListener("change", (e) => {
    logsLevel = e.target.value;
    load();
  });
  document.getElementById("logs-search")?.addEventListener("input", (e) => {
    logsQuery = e.target.value;
    load();
  });
  document.getElementById("logs-from")?.addEventListener("change", (e) => {
    logsFrom = e.target.value; load();
  });
  document.getElementById("logs-to")?.addEventListener("change", (e) => {
    logsTo = e.target.value; load();
  });
  document.getElementById("logs-clear-range")?.addEventListener("click", () => {
    logsFrom = ""; logsTo = "";
    const f = document.getElementById("logs-from");
    const t = document.getElementById("logs-to");
    if (f) f.value = "";
    if (t) t.value = "";
    load();
  });
  document.getElementById("logs-regex")?.addEventListener("change", (e) => {
    logsRegex = e.target.checked;
    localStorage.setItem("strivo-logs-regex", logsRegex ? "1" : "0");
    // Re-render so the placeholder copy updates; load() also re-runs to
    // apply the new pattern interpretation against the cached entries.
    renderLogs().catch(() => {});
  });
  document.getElementById("logs-follow")?.addEventListener("change", (e) => {
    logsFollow = e.target.checked;
    localStorage.setItem("strivo-logs-follow", logsFollow ? "1" : "0");
    if (logsFollow) {
      if (logsFollowTimer) clearInterval(logsFollowTimer);
      logsFollowTimer = setInterval(() => { if (!document.hidden) load(); }, 4000);
    } else if (logsFollowTimer) {
      clearInterval(logsFollowTimer);
      logsFollowTimer = null;
    }
  });
  document.getElementById("logs-refresh")?.addEventListener("click", load);
  document.getElementById("logs-copy")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(logsLastFilteredText || "");
      Toast.success("Logs copied to clipboard");
    } catch (err) {
      Toast.error(`Copy failed: ${err.message}`);
    }
  });
  document.getElementById("logs-download")?.addEventListener("click", () => {
    const blob = new Blob([logsLastFilteredText || ""], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = logsLastFile || "strivo.log";
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  await load();
  // Auto-arm follow if it was previously enabled.
  if (logsFollow) {
    logsFollowTimer = setInterval(() => { if (!document.hidden) load(); }, 4000);
  }
}

// Cache the last-rendered filtered text so Copy/Download don't have to
// re-walk the DOM. Updated inside renderLogs.load().
let logsLastFilteredText = "";
// Date-range filter for /logs. Both are ISO-prefix strings the user
// picked from the datetime-local inputs; empty string = unbounded.
let logsFrom = "";
let logsTo = "";
// Trace-id click-to-filter: when a clickable token is clicked we
// set logsQuery to the trace id and re-render. Stored separately so
// the user can clear it independently.
let logsTraceId = "";

// Parse a log line head into an ISO timestamp prefix (e.g.
// "2026-05-28T22:13:01"). Returns null when nothing recognisable.
function logLineIsoStamp(head) {
  const m = head.match(/(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})/);
  return m ? m[1] : null;
}

// Match a single log line against the active date range (inclusive).
// Empty bounds skip the check on that side.
function logInRange(head) {
  if (!logsFrom && !logsTo) return true;
  const stamp = logLineIsoStamp(head);
  if (!stamp) return false; // structureless lines drop when range is set
  if (logsFrom && stamp < logsFrom) return false;
  if (logsTo && stamp > logsTo) return false;
  return true;
}
let logsLastFile = "strivo.log";

// ── Upcoming agenda (item 18) — first-class calendar of known upcoming
// recordings. Source = scheduled (cron) entries with their server-computed
// next_fire. (Platform-side scheduled broadcasts aren't available via API.) ──
function dayBucket(d) {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const that = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diff = Math.round((that - today) / 86400000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Tomorrow";
  return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
}

async function renderSchedule(context = captureRouteContext()) {
  if (!mountRouteShell(context)) return;
  // Page lives at #/schedule for back-compat with bookmarks, but is now
  // the Monitor page — record-when-live and auto-download new uploads
  // replace the cron form that 95% of users found foreign. (Power users
  // can still add cron entries via config.toml's [[schedule]] table;
  // they show up in the "Cron schedule" group below when present.)
  let monitor = { auto_record: [], auto_download: [] };
  let channels = [];
  let cronEntries = [];
  let settings = {};
  let health = {};
  try {
    const [m, c, s, st, h] = await Promise.all([
      API.monitor().catch(() => ({ auto_record: [], auto_download: [] })),
      API.channels().then((r) => r.channels || []).catch(() => []),
      API.schedule().then((r) => r.schedule || []).catch(() => []),
      API.settings().catch(() => ({})),
      API.health().catch(() => ({})),
    ]);
    monitor = {
      auto_record: Array.isArray(m?.auto_record) ? m.auto_record : [],
      auto_download: Array.isArray(m?.auto_download) ? m.auto_download : [],
    };
    channels = c;
    cronEntries = s;
    settings = st;
    health = h;
  } catch (_) {}
  if (!isRouteCurrent(context)) return;

  // Build a channel lookup so we can show display_name + platform.
  const channelByKey = new Map(
    channels.map((c) => [`${c.platform}:${c.id}`, c]),
  );
  const channelByName = new Map(
    channels.map((c) => [(c.display_name || c.name || "").toLowerCase(), c]),
  );
  const channelsAvailableForDownload = channels.filter(
    (c) => c.platform === "YouTube",
  );

  // Capture profiles drive the per-channel override selects + tier badges
  // below; the option lists are built per-row (with the current value
  // pre-selected) inside the map.
  const captureProfiles = (settings.capture_profiles || []);

  // Quality-tier lookup map from capture profiles (Task 1).
  const profileTierMap = new Map(captureProfiles.map((p) => [p.name, p.quality_tier || ""]));
  const TIER_LABEL = { best: "Best", "1080p": "1080p", "720p": "720p", "480p": "480p", audio_only: "Audio only" };

  // Section 1 — record when live (existing auto-record list).
  const recordRows = monitor.auto_record
    .map(
      (e) => {
        const curContainer = e.format || "";
        const curProfile = e.profile || "";
        const curTier = curProfile ? (profileTierMap.get(curProfile) || "") : "";
        // Rebuild option lists with selected=true on the current values.
        const containerOpts = ["", "mkv", "mp4", "ts"]
          .map((c) => `<option value="${htmlEscape(c)}"${c === curContainer ? " selected" : ""}>${htmlEscape(c || "(default)")}</option>`)
          .join("");
        const profileOpts = ["", ...captureProfiles.map((p) => p.name)]
          .map((p) => `<option value="${htmlEscape(p)}"${p === curProfile ? " selected" : ""}>${htmlEscape(p || "(default)")}</option>`)
          .join("");
        const tierBadge = curTier
          ? `<span class="mon-tier-badge" data-key="${htmlEscape(e.key)}">${htmlEscape(TIER_LABEL[curTier] || curTier)}</span>`
          : `<span class="mon-tier-badge" data-key="${htmlEscape(e.key)}"></span>`;
        return `
    <div class="task-row">
      <div class="task-info">
        <span class="task-name">${htmlEscape(e.channel_name || e.channel_id)} <span class="mon-plat micro plat-${htmlEscape(e.platform.toLowerCase())}">${htmlEscape(e.platform)}</span></span>
        <span class="task-cadence">${htmlEscape(e.key)}</span>
        <span class="mon-fmt-row">
          <label class="mon-fmt-label" title="Container override for this channel">Container</label>
          <select class="mon-fmt-sel" data-key="${htmlEscape(e.key)}" data-field="format"
                  title="Per-channel container override (empty = global default)">${containerOpts}</select>
          <label class="mon-fmt-label" title="Named capture profile for this channel">Profile</label>
          <select class="mon-fmt-sel" data-key="${htmlEscape(e.key)}" data-field="profile"
                  title="Per-channel capture profile (empty = global default)">${profileOpts}</select>
          ${tierBadge}
        </span>
      </div>
      <button class="sm mon-rec-rm" data-key="${htmlEscape(e.key)}" title="Stop auto-recording this channel">✕</button>
    </div>`;
      },
    )
    .join("");

  // Section 2 — auto-download new uploads (YouTube only).
  const downloadRows = monitor.auto_download
    .map((e) => {
      const ch = channelByKey.get(e.key);
      const name = ch ? (ch.display_name || ch.name) : e.channel_id;
      const playlistsValue = (e.playlists || []).join(", ");
      return `
      <div class="task-row mon-dl-row">
        <div class="task-info">
          <span class="task-name">${htmlEscape(name)} <span class="mon-plat micro plat-${htmlEscape(e.platform.toLowerCase())}">${htmlEscape(e.platform)}</span></span>
          <span class="task-cadence">
            <label class="mon-scope">
              <span>Limit to playlists (optional, comma-separated)</span>
              <input class="mon-playlists" type="text" data-key="${htmlEscape(e.key)}"
                     placeholder="PLxxx, PLyyy — leave empty for whole channel"
                     value="${htmlEscape(playlistsValue)}" />
            </label>
          </span>
        </div>
        <button class="sm mon-dl-rm" data-key="${htmlEscape(e.key)}" title="Stop auto-downloading uploads from this channel">✕</button>
      </div>`;
    })
    .join("");

  // Auto-download (Archiver tandem) is Creator Edition only — the PVR daemon
  // mounts no archiver routes, so the whole section is hidden there.
  // buildCreatorDownloadSection is defined only in 037-creator.js (a hoisted
  // top-level `function`, callable here regardless of file order).
  const downloadSectionHtml = typeof buildCreatorDownloadSection === "function"
    ? buildCreatorDownloadSection(downloadRows, channelsAvailableForDownload)
    : "";

  // Cron schedule section — kept for power users who already use it,
  // collapsed by default. Empty unless config.toml has entries.
  const cronGroup = cronEntries.length
    ? `<details class="mon-cron"><summary>Advanced cron schedule (${cronEntries.length})</summary>
        ${cronEntries
          .map(
            (e, i) => `
          <div class="task-row">
            <div class="task-info">
              <span class="task-name">${htmlEscape(e.channel || "scheduled")}</span>
              <span class="task-cadence"><code>${htmlEscape(e.cron || "")}</code>${e.duration ? ` · ${htmlEscape(e.duration)}` : ""}${e.next_fire ? ` · next: ${htmlEscape(new Date(e.next_fire).toLocaleString())}` : ""}</span>
            </div>
            <button class="sm sch-del" data-i="${i}" title="Delete this cron entry">✕</button>
          </div>`,
          )
          .join("")}
        <p class="mon-cron-hint">Cron entries are added via <code>~/.config/strivo/config.toml</code> under <code>[[schedule]]</code>. They fire at the cron expression's next match regardless of live state — useful for predictable shows on platforms without a live API. Most users want the simpler primitives above.</p>
      </details>`
    : "";

  // Get-channel-name helper for the Add forms — match by case-insensitive
  // display name, fall back to "Platform:id" parsing.
  const resolveChannelKey = (raw) => {
    const t = raw.trim();
    if (!t) return null;
    if (t.includes(":")) return t;
    const c = channelByName.get(t.toLowerCase());
    return c ? `${c.platform}:${c.id}` : null;
  };

  // Live capture status: active recordings + disk free + current limits.
  // Recordings cache is shared across the SPA so we can read in-progress
  // count without a separate fetch.
  const activeCount = recCache.filter((r) => isInProgress(r.state)).length;
  const limits = settings.monitor_limits || {};
  const maxConcurrent = limits.max_concurrent_recordings || 0;
  const diskBudgetGb = limits.disk_budget_reserved_gb || 0;
  const diskAvailBytes = (health.disk && health.disk.filesystem_avail_bytes) || 0;
  const diskTotalBytes = (health.disk && health.disk.filesystem_total_bytes) || 0;
  const availPct = diskTotalBytes > 0 ? (diskAvailBytes / diskTotalBytes) * 100 : 0;
  // Reserve budget vs free: warn when free < reserved + 5 GB headroom.
  const reservedBytes = diskBudgetGb * 1024 * 1024 * 1024;
  const diskOverBudget = reservedBytes > 0 && diskAvailBytes < reservedBytes;
  const concurrentSaturated = maxConcurrent > 0 && activeCount >= maxConcurrent;
  const statusBanner = (concurrentSaturated || diskOverBudget)
    ? `<div class="mon-status-banner warn">
         ${concurrentSaturated ? `<span>⚠ Concurrent cap hit: ${activeCount}/${maxConcurrent} recordings in flight — new live captures will queue.</span>` : ""}
         ${diskOverBudget ? `<span>⚠ Disk free (${formatBytes(diskAvailBytes)}) is below the reserved ${diskBudgetGb} GB — new captures will defer.</span>` : ""}
       </div>`
    : `<div class="mon-status-banner ok">
         <span>✓ ${activeCount} recording${activeCount === 1 ? "" : "s"} in flight${maxConcurrent ? ` / ${maxConcurrent}` : ""} · ${formatBytes(diskAvailBytes)} free</span>
       </div>`;

  if (!mountPage(`
    <h1 class="page-title">Monitor</h1>
    <p class="page-subtitle">Channels StriVo is watching. Record live broadcasts as they happen, or auto-download new YouTube uploads.</p>

    ${buildCalStrip(cronEntries)}

    ${statusBanner}

    <section class="cfg-card">
      <h2 class="cfg-title">Capture limits <a href="#/settings/notifications" class="stg-linkbtn" style="margin-left:auto;font-size:var(--text-xs, 12px)">Configure go-live banners →</a></h2>
      <p class="mon-help">Safety knobs that defer new captures when StriVo is already busy or disk is tight. Zero in either field disables that cap.</p>
      <div class="mon-limits-grid">
        <label class="mon-limit">
          <span class="mon-limit-label">Max concurrent recordings</span>
          <input class="mon-limit-input" type="number" min="0" max="64" step="1"
                 id="mon-limit-concurrent" value="${maxConcurrent}" />
          <span class="mon-limit-hint">${maxConcurrent === 0 ? "unlimited" : `${activeCount} of ${maxConcurrent} in use`}</span>
        </label>
        <label class="mon-limit">
          <span class="mon-limit-label">Reserved disk budget (GB)</span>
          <input class="mon-limit-input" type="number" min="0" max="100000" step="1"
                 id="mon-limit-disk" value="${diskBudgetGb}" />
          <span class="mon-limit-hint">${diskBudgetGb === 0 ? "no circuit breaker" : diskOverBudget ? "ENGAGED" : "armed"}</span>
        </label>
        <div class="mon-disk-gauge" title="Recording filesystem usage">
          <span class="mon-disk-label">Free disk</span>
          <div class="mon-disk-bar"><div class="mon-disk-fill" style="width:${(100 - availPct).toFixed(1)}%"></div></div>
          <span class="mon-disk-meta">${formatBytes(diskAvailBytes)} free of ${formatBytes(diskTotalBytes)}</span>
        </div>
      </div>
    </section>

    <section class="cfg-card">
      <h2 class="cfg-title">Record when live</h2>
      <p class="mon-help">Twitch and YouTube live broadcasts capture automatically. Add channels from the topbar's <em>+ Add channel</em>, then enable Auto-record on the channel card.</p>
      ${recordRows || '<div class="empty sm">No channels are set to record-when-live yet.</div>'}
    </section>

    ${downloadSectionHtml}

    ${cronGroup}
  `, context)) return;
  setupChromeHandlers();

  // Capture-limit inputs — debounced save to /settings/update so each
  // keystroke doesn't fire a round-trip. Repaint on save so the gauge
  // and banner reflect the new state.
  const wireLimit = (id, path, max) => {
    const el = document.getElementById(id);
    if (!el) return;
    let timer;
    el.addEventListener("input", () => {
      clearTimeout(timer);
      timer = setTimeout(async () => {
        const v = Math.max(0, Math.min(max, parseInt(el.value, 10) || 0));
        if (v !== parseInt(el.value, 10)) el.value = v;
        try {
          await API.updateSetting(path, v);
          Toast.success(`Saved · ${path}`);
          renderSchedule().catch(() => {});
        } catch (err) {
          Toast.error(`Save failed: ${err.message}`);
        }
      }, 600);
    });
  };
  wireLimit("mon-limit-concurrent", "monitor_limits.max_concurrent_recordings", 64);
  wireLimit("mon-limit-disk", "monitor_limits.disk_budget_reserved_gb", 100000);

  // Record-when-live row delete.
  document.querySelectorAll(".mon-rec-rm").forEach((btn) => {
    btn.addEventListener("click", async () => {
      if (!(await confirmDialog("Stop auto-recording this channel?", { danger: true, ok: "Stop" }))) return;
      try {
        await API.toggleAutoRecord(btn.dataset.key, false);
        Toast.success("Stopped");
        renderSchedule();
      } catch (e) {
        Toast.error(`Couldn't stop: ${e.message}`);
      }
    });
  });

  // Per-channel format/profile override — save on change (Task 4).
  // Quality-tier badge data is embedded as JSON for JS lookup after render.
  const monProfileTierData = {};
  document.querySelectorAll(".mon-tier-badge").forEach((badge) => {
    const profSel = badge.closest(".mon-fmt-row")?.querySelector('.mon-fmt-sel[data-field="profile"]');
    if (profSel) monProfileTierData[badge.dataset.key] = { badge, profSel };
  });
  document.querySelectorAll(".mon-fmt-sel").forEach((sel) => {
    sel.addEventListener("change", async () => {
      const key = sel.dataset.key;
      const taskRow = sel.closest(".task-row");
      if (!taskRow) return;
      const fmtSel = taskRow.querySelector('.mon-fmt-sel[data-field="format"]');
      const profSel = taskRow.querySelector('.mon-fmt-sel[data-field="profile"]');
      // Update quality-tier badge if profile changed.
      if (sel.dataset.field === "profile") {
        const tierBadge = taskRow.querySelector(".mon-tier-badge");
        if (tierBadge) {
          const tierRaw = profSel ? (window._monProfileTiers || {})[profSel.value] || "" : "";
          const TIER_LABEL_JS = { best: "Best", "1080p": "1080p", "720p": "720p", "480p": "480p", audio_only: "Audio only" };
          tierBadge.textContent = tierRaw ? (TIER_LABEL_JS[tierRaw] || tierRaw) : "";
        }
      }
      try {
        await API.setAutoRecordFormat(key, fmtSel ? fmtSel.value : "", profSel ? profSel.value : "");
        Toast.success("Format saved");
      } catch (err) {
        Toast.error(`Format save failed: ${err.message}`);
      }
    });
  });
  // Expose profile→tier map for the change handler above.
  window._monProfileTiers = Object.fromEntries(
    (settings?.capture_profiles || []).map((p) => [p.name, p.quality_tier || ""])
  );
