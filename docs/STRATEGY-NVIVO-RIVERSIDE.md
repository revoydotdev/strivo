# Strategy confab: NVivo meets Riverside

Ongoing positioning and sequencing record for the Strivo Creator Edition
thesis: **a stream-native research platform fused with a creator production
pipeline — NVivo meets Riverside, for streamers and content creators.**

Companion documents: [RESEARCH-PLATFORM-ROADMAP.md](RESEARCH-PLATFORM-ROADMAP.md)
(the research half, phased), [ROADMAP.md](../ROADMAP.md) (north star and the
edition split), [product/index.md](product/index.md) (public pitch). This
document is append-only per round; supersede a position by writing a new round,
not by editing an old one.

---

## Round 1 — 2026-07-29

### 1. The positioning in one paragraph

Riverside (and Descript, and Opus Clip) turned *one recording* into content:
transcribe it, edit it by editing text, cut clips, caption them, publish.
NVivo (and ATLAS.ti, MAXQDA) turned *a corpus* into understanding: code it,
query it, compare across cases, keep every finding traceable to evidence.
Nobody has fused the two, and nobody serves the person who needs both most:
the streamer whose "corpus" grows by four hours a day, arrives with chat and
audience metrics time-locked to the video, and whose livelihood depends on
turning that archive into both **published content** (clips, VODs, highlights)
and **self-knowledge** (what retains viewers, what makes chat erupt, how a
running bit evolved across six months). Strivo already owns the hard
prerequisite neither incumbent has: the acquisition layer. We capture the
streams, locally, with chat and metrics, from the moment of go-live.

### 2. The pitch, per audience

- **To a streamer:** "Everything you've ever streamed, searchable like text,
  clippable like a timeline, and analyzable like a spreadsheet — on your own
  machine."
- **To a researcher:** "The first qualitative-analysis platform that treats
  live multimodal stream data — video, transcript, chat, audience telemetry —
  as a first-class evidence type."
- **To ourselves (the engineering thesis):** one evidence kernel, two
  projections. The research workspaces and the DAW pipeline are both views
  over the same canonical store.

### 3. The unifying insight: a coding and a clip candidate are the same object

This is the load-bearing observation the whole fusion rests on:

> A **coding** (a time-ranged, typed annotation on a source, with provenance)
> and a **clip candidate** (a time range on a recording, tagged with why it's
> interesting) are the *same data structure*.

Consequences:

- The cut-discovery plugins (Chapters, Cuepoints, Clipper, Heatmap,
  Chat-density) are **extractors** in research-kernel terms: they emit
  append-only signals with confidence and provenance. Phase 1 already
  migrates Cuepoints and Viewguard output into the kernel this way.
- The Coding Studio (Phase 6) and the clip-review workflow are the **same
  surface** with different vocabulary: synchronized player + transcript +
  chat + signals + timeline, where a human confirms, adjusts, or rejects
  time-ranged annotations.
- Query Lab (Phase 7) is the clip-sourcing engine: "every moment I talked
  about the speedrun AND chat density spiked" is a hybrid
  lexical/semantic/temporal query whose hits are *renderable* — each result
  resolves to an exact source and time range the Editor can cut.
- Evidence provenance is **content provenance**: the same chain that lets a
  researcher defend a finding lets a creator regenerate a clip after a
  transcript correction, or prove a published quote's context.

The fusion is therefore not "add research features to a clipper" or "add
export to a research tool." It is one kernel with two front-ends, and every
kernel phase pays for both. That is the moat: incumbents would each have to
build the other half *and* the acquisition layer.

### 4. Competitive map

| Product | Has | Lacks (vs us) |
| --- | --- | --- |
| Riverside | Studio-quality remote recording, transcript editing, AI clips, captions, publish | No platform capture (Twitch/YT go-live), no chat/metrics, no corpus memory, cloud-only, subscription |
| Descript | Best-in-class transcript-based editing, overdub, screen recording | Single-project mindset; no live capture, no cross-recording query, no chat |
| Opus Clip / Vizard / Eklipse | Cheap AI clip generation from VODs | No library, no evidence chain, no analysis, shallow "virality score" heuristics |
| NVivo / ATLAS.ti / MAXQDA | Coding rigor, codebooks, inter-rater reliability, matrix queries | Cannot ingest live streams; no time-synced multimodal evidence; no media pipeline; desktop-era file handling; no publishing |
| StreamLadder / Crossclip | Format conversion for shorts | Utility, not platform |
| Twitch/YouTube native tools | Zero-setup clips, chapters | Platform-locked, retention windows, no export of the analytical layer |

Two structural advantages nobody in the table can copy cheaply:

1. **Local-first with the archive already on disk.** Every competitor starts
   from an upload. We start from a library the PVR has been building for
   months, with chat and metrics captured live (some of it — chat
   especially — is expensive or impossible to recover after the fact).
2. **Longitudinal, cross-stream analysis.** All creator tools are
   per-recording. The research kernel is corpus-scale by construction.

### 5. What "Riverside" means here — and what it does not

We take from Riverside the **post-production covenant**: capture once, then
transcript-first editing, clip discovery, captions, loudness, branding,
multi-format publish — the Editor bus and Publish bus already implement most
of this. We explicitly do **not** take:

- **Remote guest studio recording** (Riverside's actual core). Strivo captures
  *platform output*, not local cameras and microphones. Building a WebRTC
  studio is a different company. **Non-goal**, recorded here so the confab
  stops re-litigating it.
- **Cloud rendering / collaboration SaaS.** Local-first is identity, not
  implementation detail. The collaboration roadmap stays parked behind
  explicit demand (per product/index.md).

The seam we leave open instead: **local multitrack import**. A creator who
records locally (OBS, separate mic track) should be able to import that
recording into the same library, kernel, and pipeline as captured streams.
Import-not-capture keeps us out of the studio business while still serving
the podcast-shaped workflow. (Multitrack and Demucs plugins already point
this direction.)

### 6. Vocabulary: one kernel, two skins

Research language will kill creator adoption, and creator language will kill
research credibility. The kernel keeps canonical names; each surface renders
its own. Proposed translation, to be settled before Phase 5/6 UI work:

| Kernel (canonical) | Research surface | Creator surface |
| --- | --- | --- |
| Project | Project | Workspace / Channel |
| Source | Source | Recording |
| Case | Case | Series / Segment type |
| Code / codebook | Code / codebook | Tag / tag set |
| Coding | Coding | Moment |
| Memo | Memo | Note |
| Signal | Signal | Detection |
| Corpus | Corpus | Collection |
| Query Lab | Query Lab | Archive search |
| Evidence Canvas | Evidence Canvas | Insights board |

Rule: the two surfaces are *renderings*, never forks. One schema, one API,
per-surface display strings. A domain pack (Phase 12) selects the skin.

### 7. What this changes about sequencing

The research roadmap's phase order stands, but the confab adds a **dual-value
test** to phase scoping: every phase must ship at least one creator-visible
outcome, or the Creator Edition stalls for a year while we build a research
product streamers can't see. Concretely:

- **Phase 3 (transcription)** is the single highest-leverage phase for both
  personas. Word-level timestamps unlock transcript-based editing (the
  Descript interaction) — the creator payoff should ship *in the same phase*
  as the research payoff, not after Phase 6.
- **Phase 6 (Coding Studio)** must be scoped as the moment/clip review
  surface from day one. If it ships researcher-only, we build the same
  synchronized-player UI twice.
- **Phase 7 (Query Lab)** ships with "search my archive → open in Editor →
  render clip" as the acceptance-test user journey, alongside the research
  golden set.
- **Phase 8–9 (analytics, canvas)** carry the creator-retention story:
  chat-density vs. retention overlays, "what makes chat erupt," best-slot
  analysis feeding the existing Schedule-optimizer.
- **Phase 11 (live research)** is, in creator terms, **live clipping** —
  mark moments during the stream, publish before the stream ends. This may
  be the single most demo-able feature in the whole plan; its priority
  should be revisited once Phase 3 lands rather than left at the end by
  default.

### 8. Pricing tension (flagged, not resolved)

Strivo Pro is a one-time $25 unlock. A research platform with paid
transcription/embedding providers, 10,000-hour corpora, and NVivo-refugee
users (NVivo licenses run hundreds of dollars per seat per year) is a
different willingness-to-pay universe. Options sketched for a future round:

1. Keep $25 Pro for the DAW stack; add a higher one-time "Research" tier.
2. Keep everything one-time; charge for nothing recurring (provider costs are
   the user's own API keys anyway — local-first makes our marginal cost zero).
3. Free research kernel, paid surfaces.

No decision this round. Constraint carried forward: **no subscription for
locally-running code** — it contradicts the identity that differentiates us
from every competitor in §4.

### 9. Open questions for the next round

- Naming: does "Creator Edition" still cover a research persona, or does the
  research skin warrant its own edition name at Phase 5+?
- Local multitrack import (§5): which phase owns it? It touches capture,
  library, and kernel source types.
- Consent and ethics surface for creator use: chat messages are other
  people's speech; the research side has redaction/pseudonymization
  (Phase 5) — what does the *creator* side owe chat participants before a
  clip with chat overlay is published?
- Which two or three NVivo workflows do we deliberately *not* chase (e.g.,
  survey import, literature review) to keep the corpus stream-native?
- Validation: find five streamers with 500+ hour archives and run the
  Phase 3 transcript-search demo past them before Phase 5 UI investment.
