// Deterministic mock backend for the StriVo webui E2E suite (W7).
// Serves the real SPA assets and stubs /api/v1 + /events so the browser
// tests run without a live daemon or platform auth.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ASSETS = join(__dirname, "..", "assets");
const PORT = process.env.PORT || 8199;

const CHANNELS = [
  {
    id: "UClive0000000000000000aa",
    platform: "YouTube",
    name: "livechan",
    display_name: "Live Channel",
    is_live: true,
    stream_title: "Live test stream",
    game_or_category: "Just Chatting",
    viewer_count: 1234,
    started_at: new Date().toISOString(),
    thumbnail_url: null,
    auto_record: false,
  },
  {
    id: "twitch:offlinechan",
    platform: "Twitch",
    name: "offlinechan",
    display_name: "Offline Channel",
    is_live: false,
    stream_title: null,
    game_or_category: null,
    viewer_count: null,
    started_at: null,
    thumbnail_url: null,
    auto_record: true,
    // ~5 hours ago — exercises the "Xh ago" branch of relTime.
    last_live_at: new Date(Date.now() - 5 * 3600_000).toISOString(),
  },
  {
    id: "UCytoffline00000000aa",
    platform: "YouTube",
    name: "ytoffline",
    display_name: "YouTube Past Stream",
    is_live: false,
    stream_title: null,
    game_or_category: null,
    viewer_count: null,
    started_at: null,
    thumbnail_url: null,
    auto_record: false,
    // ~3 days ago — exercises the "Xd ago" branch.
    last_live_at: new Date(Date.now() - 3 * 86400_000).toISOString(),
  },
];

const RECORDINGS = {
  recordings: [
    {
      id: "11111111-1111-1111-1111-111111111111",
      channel_name: "Alpha",
      stream_title: "Zebra stream",
      state: "Finished",
      started_at: "2026-05-20T10:00:00Z",
      bytes_written: 5_000_000_000,
    },
    {
      id: "22222222-2222-2222-2222-222222222222",
      channel_name: "Bravo",
      stream_title: "Apple stream",
      state: "Recording",
      started_at: "2026-05-26T09:00:00Z",
      bytes_written: 1_000_000_000,
    },
    {
      id: "33333333-3333-3333-3333-333333333333",
      channel_name: "Charlie",
      stream_title: "Mango stream",
      state: "Failed",
      started_at: "2026-05-22T12:00:00Z",
      bytes_written: 200_000_000,
    },
  ],
};

const CONTENT_TYPES = { ".js": "text/javascript", ".css": "text/css", ".html": "text/html" };

function json(res, code, body) {
  res.writeHead(code, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

// Open SSE connections, so POSTs that resolve asynchronously (vods,
// playlists) can push their answer like the real daemon does.
const sseClients = new Set();
function broadcast(eventObj) {
  const frame = `data: ${JSON.stringify(eventObj)}\n\n`;
  for (const c of sseClients) c.write(frame);
}

const SCHEDULE = [
  {
    channel: "Alpha",
    cron: "0 20 * * *",
    duration: "4h",
    next_fire: new Date(Date.now() + 3600_000).toISOString(),
  },
];

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  const path = url.pathname;

  if (path === "/events") {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive",
    });
    res.write(": connected\n\n");
    sseClients.add(res);
    const ka = setInterval(() => res.write(": keepalive\n\n"), 1000);
    req.on("close", () => {
      clearInterval(ka);
      sseClients.delete(res);
    });
    return;
  }

  // API surface.
  if (path.startsWith("/api/v1/")) {
    const p = path.slice("/api/v1".length);
    if (p === "/health") return json(res, 200, { status: "ok" });
    if (p === "/history")
      return json(res, 200, {
        history: [
          { id: "1", channel_name: "LilAggy", stream_title: "Elden Ring", platform: "Twitch", state: "Finished", started_at: "2026-05-26T20:00:00Z", bytes_written: 123456789 },
        ],
      });
    if (p === "/blocklist" && req.method === "GET")
      return json(res, 200, {
        blocklist: [
          { platform: "Twitch", channel_id: "ch1", vod_id: "", reason: null, created_at: "2026-05-26T00:00:00Z" },
        ],
      });
    if (p === "/blocklist") return json(res, 201, { status: "ok" });
    if (p === "/backup") return json(res, 201, { name: "2026-05-26T00-00-00Z", files: ["config.toml", "jobs.db"], bytes: 1234 });
    if (p === "/backups")
      return json(res, 200, {
        backups: [{ name: "2026-05-26T00-00-00Z", bytes: 1234, files: ["config.toml", "jobs.db"] }],
      });
    if (p.startsWith("/logs"))
      return json(res, 200, {
        file: "strivo.2026-05-26.log",
        level: "info",
        lines: [
          "2026-05-26T22:00:00Z  INFO strivo_core::daemon: StriVo daemon starting",
          "2026-05-26T22:00:01Z  WARN strivo_core::monitor: example warning",
        ],
      });
    if (p === "/health/checks")
      return json(res, 200, {
        status: "ok",
        checks: [
          { domain: "Network", name: "Daemon IPC", severity: "ok", message: "Daemon reachable.", fix: "" },
          { domain: "Storage", name: "Disk space", severity: "ok", message: "3 TB free.", fix: "" },
        ],
      });
    if (p === "/auth/login") return json(res, 200, { status: "ok" });
    if (p === "/auth/logout") return json(res, 200, { status: "ok" });
    if (p === "/channels") return json(res, 200, { channels: CHANNELS });
    if (p === "/patreon")
      return json(res, 200, {
        creators: [
          {
            id: "camp123",
            platform: "Patreon",
            name: "creatorslug",
            display_name: "Cool Creator",
            is_live: false,
            stream_title: "Premium Tier",
            game_or_category: null,
            viewer_count: null,
            started_at: null,
            thumbnail_url: null,
            auto_record: false,
          },
        ],
        posts: [
          {
            id: "post1",
            campaign_id: "camp123",
            title: "Behind the scenes",
            url: "https://patreon.com/posts/post1",
            published_at: "2026-05-25T00:00:00Z",
            embed_url: "https://example.com/embed/post1",
          },
        ],
      });
    if (p === "/recordings" && req.method === "GET") return json(res, 200, RECORDINGS);
    {
      const m = p.match(/^\/recordings\/([0-9a-fA-F-]{8,})$/);
      if (m && req.method === "GET") {
        const id = m[1];
        const found = RECORDINGS.recordings.find((r) => r.id === id);
        if (!found) {
          res.writeHead(404);
          res.end("not found");
          return;
        }
        // Backfill the optional fields the info modal + player expect.
        return json(res, 200, {
          channel_id: "twitch:alpha",
          platform: "Twitch",
          transcode: false,
          duration_secs: 3600,
          output_path: `/mnt/sda2/strivo/${found.channel_name}/${id}.mkv`,
          source_url: null,
          error: found.state === "Failed" ? "synthetic error for tests" : null,
          ...found,
        });
      }
    }
    if (p === "/storage")
      return json(res, 200, {
        bytes_used_by_recordings: 6_200_000_000,
        filesystem_avail_bytes: 900_000_000_000,
      });
    if (p === "/gantt") return json(res, 200, { items: [] });
    if (p === "/schedule") return json(res, 200, { schedule: SCHEDULE });
    if (p === "/monitor")
      return json(res, 200, {
        auto_record: [
          {
            key: "Twitch:twitch:offlinechan",
            channel_id: "twitch:offlinechan",
            channel_name: "Alpha",
            platform: "Twitch",
            format: "",
            profile: "",
          },
        ],
        auto_download: [],
      });
    if (p === "/settings")
      return json(res, 200, {
        twitch_configured: true,
        youtube_configured: true,
        patreon_configured: false,
        recording_dir: "/mnt/sda2/strivo",
        auto_record_channels: [],
        poll_interval_secs: 60,
        schedule: [],
        creator_enabled: true,
        capture_profiles: [],
        monitor_limits: {
          max_concurrent_recordings: 3,
          disk_budget_reserved_gb: 20,
        },
      });
    if (p === "/pipelines/runs" && req.method === "GET")
      return json(res, 200, {
        runs: [{
          id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
          name: "Ultimate creator publish",
          trigger: "recording-finished",
          state: "Done",
          stages: [{
            id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            name: "Assemble Casebook",
            kind: { Custom: "casebook.compose" },
            state: "Done",
            artifacts: [{
              kind: "casebook_markdown",
              path: "/var/lib/strivo/plugins/artifacts/recording/casebook.md",
              mime: "text/markdown",
            }],
          }],
        }],
      });

    // ── Plugin data surface (read-only) ──────────────────────────────
    if (p === "/plugins")
      return json(res, 200, {
        plugins: [
          { name: "crunchr", display: "Crunchr", description: "AI transcription, diarization & analysis", available: true, stats: { recordings: 2, analyzed: 1 }, verbs: [{ verb: "Re-transcribe", scope: "recording", label: "Re-transcribe" }] },
          { name: "insights", display: "Insights", description: "Word frequency, speaker airtime, topics & sentiment", available: true, stats: { words: 42, topics_videos: 1 }, verbs: [] },
          { name: "archiver", display: "Archiver", description: "Back-catalog VOD pulls & download tracking", available: true, stats: { channels: 1, videos: 2, downloaded: 1 }, verbs: [] },
          { name: "viewguard", display: "Viewguard", description: "Viewbot fraud detection — verdicts & viewer signals", available: true, stats: { verdicts: 1, samples: 4 }, verbs: [] },
        ],
      });
    if (p === "/plugins/crunchr/recordings")
      return json(res, 200, {
        available: true,
        recordings: [
          { recording_id: "rec-1", channel_name: "Alpha", title: "Elden Ring run", status: "complete", segment_count: 120, has_analysis: true, created_at: "2026-05-26 20:00:00" },
          { recording_id: "rec-2", channel_name: "Bravo", title: "Just chatting", status: "transcribing", segment_count: 0, has_analysis: false, created_at: "2026-05-27 09:00:00" },
        ],
      });
    {
      const m = p.match(/^\/plugins\/crunchr\/recordings\/([^/]+)$/);
      if (m)
        return json(res, 200, {
          recording_id: decodeURIComponent(m[1]),
          channel_name: "Alpha",
          title: "Elden Ring run",
          status: "complete",
          summary: "A long boss-fight session with commentary.",
          topics: ["elden ring", "bosses"],
          sentiment: "positive",
          segments: [
            { index: 0, start_sec: 0.0, end_sec: 3.5, text: "hey everyone welcome back", speaker: "Alpha", confidence: null },
            { index: 1, start_sec: 3.5, end_sec: 7.0, text: "today we fight the boss", speaker: "Alpha", confidence: null },
          ],
        });
    }
    if (p === "/plugins/crunchr/search") {
      const q = (url.searchParams.get("q") || "").trim();
      if (!q) return json(res, 200, { results: [] });
      return json(res, 200, {
        available: true,
        results: [
          { chunk_id: 1, video_title: "Elden Ring run", channel_name: "Alpha", snippet: `…we fight the ${q}…`, start_sec: 3.5, end_sec: 7.0, score: 0.9 },
        ],
      });
    }
    if (p === "/plugins/archiver/channels")
      return json(res, 200, {
        available: true,
        channels: [
          { id: 1, name: "Alpha", url: "https://twitch.tv/alpha", platform: "Twitch", archive_dir: "/arc/alpha", last_scan: "2026-05-26 12:00:00", video_count: 2, downloaded_count: 1 },
        ],
      });
    {
      const m = p.match(/^\/plugins\/archiver\/channels\/([^/]+)\/videos$/);
      if (m)
        return json(res, 200, {
          available: true,
          videos: [
            { video_id: "v2", title: "Stream Two", upload_date: "20260102", duration: 7200, playlist: null, downloaded: false },
            { video_id: "v1", title: "Stream One", upload_date: "20260101", duration: 3600, playlist: null, downloaded: true },
          ],
        });
    }
    if (p === "/plugins/viewguard/verdicts")
      return json(res, 200, {
        available: true,
        verdicts: [
          { channel_id: "twitch:suspect", stream_started_at: "2026-05-27T10:00:00Z", stream_ended_at: null, final_score: 0.87, band: "fraudulent", contributors: [{ kind: "SpikeShape", score: 0.9 }, { kind: "PlateauVariance", score: 0.7 }] },
          { channel_id: "twitch:clean", stream_started_at: "2026-05-27T08:00:00Z", stream_ended_at: null, final_score: 0.12, band: "clean", contributors: [] },
        ],
      });
    {
      const m = p.match(/^\/plugins\/viewguard\/channels\/([^/]+)\/samples$/);
      if (m)
        return json(res, 200, {
          available: true,
          samples: [
            { ts: "2026-05-27T10:00:00Z", viewers: 100 },
            { ts: "2026-05-27T10:01:00Z", viewers: 5000 },
          ],
        });
    }
    if (p === "/plugins/insights/words")
      return json(res, 200, {
        available: true,
        words: [
          { word: "stream", count: 40 },
          { word: "recording", count: 25 },
        ],
      });
    if (p === "/plugins/insights/topics")
      return json(res, 200, {
        available: true,
        topics: [
          { topic: "elden ring", count: 3, first_seen: "2026-05-20", last_seen: "2026-05-26" },
        ],
      });
    {
      const m = p.match(/^\/plugins\/insights\/recordings\/([^/]+)\/speakers$/);
      if (m)
        return json(res, 200, {
          available: true,
          speakers: [{ speaker: "Alpha", seconds: 1200, segments: 80 }],
          sentiment: "positive",
        });
    }

    // Channel VODs request → answer asynchronously over SSE, like the daemon.
    const vodsMatch = p.match(/^\/channels\/([^/]+)\/vods$/);
    if (vodsMatch && req.method === "POST") {
      const channelId = decodeURIComponent(vodsMatch[1]);
      setTimeout(() => {
        broadcast({
          ChannelVods: {
            channel_id: channelId,
            vods: [
              {
                id: "stream1",
                platform: "YouTube",
                channel_id: channelId,
                title: "Yesterday's livestream",
                published_at: "2026-05-25T20:00:00Z",
                url: "https://youtu.be/stream1",
                kind: "LiveBroadcast",
              },
              {
                id: "upload1",
                platform: "YouTube",
                channel_id: channelId,
                title: "How I edit my videos",
                published_at: "2026-05-24T12:00:00Z",
                url: "https://youtu.be/upload1",
                kind: "Upload",
              },
            ],
          },
        });
      }, 50);
      return json(res, 202, { status: "requested" });
    }

    // Mutations / verb dispatch — accept and echo queued.
    if (req.method === "POST" || req.method === "PUT" || req.method === "DELETE") {
      return json(res, 202, { status: "queued", path: p });
    }
    return json(res, 200, {});
  }

  // Static assets + SPA shell.
  let file;
  if (path === "/" || path === "/app") file = join(ASSETS, "spa.html");
  else if (path.startsWith("/assets/")) file = join(ASSETS, path.slice("/assets/".length));
  if (file) {
    try {
      const buf = await readFile(file);
      const ext = file.slice(file.lastIndexOf("."));
      res.writeHead(200, { "Content-Type": CONTENT_TYPES[ext] || "application/octet-stream" });
      res.end(buf);
      return;
    } catch {
      res.writeHead(404);
      res.end("not found");
      return;
    }
  }
  res.writeHead(404);
  res.end("not found");
});

server.listen(PORT, () => console.log(`mock server on http://localhost:${PORT}`));
