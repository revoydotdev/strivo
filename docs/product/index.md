# StriVo

**A self-hosted live-stream PVR for Twitch, YouTube, and Patreon — plus a
DAW-class plugin toolkit for everything you do with the captures
after.**

StriVo runs on your machine, captures every live stream from the
channels you follow, organises the results, and ships a 35-plugin
pipeline that turns each recording into a finished, normalised,
published artefact.

## What it does

- **Captures live.** Twitch, YouTube live, Patreon. Auto-record on
  go-live, scheduled captures, manual one-shots.
- **Recovers VODs.** Past broadcasts, uploads, members-only posts
  (with cookies) — backfilled into the same library.
- **Edits non-destructively.** EDL editor with split, ripple-delete,
  trim-dead-air, voice-gate, sidechain duck, insert FX chain,
  pitch / time-stretch, branding overlay, loudness normalisation,
  styled captions, scene snapshots.
- **Publishes.** Cross-format publish queue, chapters, thumbnails,
  reuse drafter, schedule-optimizer for "post at the slot that
  retains people".
- **Watches.** Multi-stream auto-tile viewer, Chatterino-class chat
  client, multi-stream layouts (Quadrant / Highlight / Theatre).

## DAW-class plugin toolkit

35 in-tree plugins, all pure-data + unit-tested, all composing into
the same render pipeline:

| Bus | Plugins |
|---|---|
| Capture · transcribe · catalog | Crunchr · Archiver · Viewguard · Insights |
| Cut-discovery | Chapters · Cuepoints · Clipper · Thumbnails · Heatmap · Chat-density · Broll |
| Editor (the DAW core) | Editor · Deadair · Branding · Automation · Loudness · Captions · Multitrack · Structure · Beat-detect · VAD · Scenes · Sidechain · Insert FX · Pitch / time |
| Publish · view · meta | Reuse · Casebook · Multistream · Chat · Pipelines-DAG · Marketplace · Schedule-optimizer |
| Audio | Demucs (source separation) |

The editor's `⚡ Render to MKV` stitches every active bus' filter
into a single ffmpeg pass — sidechain ducking + insert chain +
pitch warp all compose through the existing `-af` slot, none of
them stomp on the others.

## Who it's for

Streamers who archive their own content, want it normalised against
platform loudness targets, want chapters / captions / thumbnails
produced once at capture time rather than dialed up after the fact,
and want a single tool that does it all locally instead of stitching
together five SaaS services.

## What it isn't

Not a streaming source. Not a cloud service. Not multi-tenant — the
collaboration roadmap (per-segment comments, CRDT EDL, review-
request workflow) is parked behind explicit user demand.

## Pricing

- **Free** — every capture-side feature: monitor, record, organise,
  watch, chat client, multi-stream viewer, cut-discovery plugins.
- **Strivo Pro** — one-time $25 — unlocks the DAW editor stack:
  Crunchr transcription, Archiver back-catalog, Viewguard,
  Insights, plus every editor-stack plugin (Branding, Automation,
  Loudness, Sidechain, Insert FX, Pitch, Scenes, Captions).

Verified local licence cache; no internet kill switch; trials require the activation backend
fully offline.

## Install

Linux:

```bash
cargo install --git https://github.com/revoydotdev/strivo --locked
```

Per-platform tarballs / zips ship at every tag — see the
[Releases](https://github.com/revoydotdev/strivo/releases) page for
x86_64-linux, x86_64-macOS, and x86_64-windows builds.

## Docs

- [README](https://github.com/revoydotdev/strivo/blob/main/README.md) — quick start
- [ROADMAP](https://github.com/revoydotdev/strivo/blob/main/ROADMAP.md) — shipped + planned
- [Plugin manifest spec](https://github.com/revoydotdev/strivo/blob/main/docs/PLUGIN-MANIFEST.md)
- [Writing a plugin](https://github.com/revoydotdev/strivo/blob/main/docs/WRITING-A-PLUGIN.md) — author guide
- [CHANGELOG](https://github.com/revoydotdev/strivo/blob/main/CHANGELOG.md)

## Source

[github.com/revoydotdev/strivo](https://github.com/revoydotdev/strivo)
— MIT. First-party plugins live in-tree at `crates/strivo-plugins/`
(folded in from the retired separate `strivo-plugins` repo).

---

This page is intended to be served at <https://chorosyne.com/strivo>
once the product website goes live; it currently 404s there.
