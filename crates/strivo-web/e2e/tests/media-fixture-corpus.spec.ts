import { test, expect } from "@playwright/test";
import {
  fixture,
  byClass,
  classes,
  integrity,
  CORPUS_DIR,
} from "../../../../fixtures/media/fixtures.mjs";

// Consumes the media fixture corpus (plan todo 32) through the single
// accessor, so the corpus is read by a real test rather than merely sitting in
// the tree. What is proven here is that the corpus is intact and that every
// class the plan names resolves to a distinct, self-consistent record.
//
// What is deliberately NOT proven here: that Chromium can decode these files.
// It cannot — `canPlayType('video/x-matroska; codecs="avc1"')` answers
// "probably" and a `<video>` element then fails with MEDIA_ELEMENT_ERROR (4),
// and the Playwright Chromium build exposes no `VideoDecoder` at all. Decoded
// media is asserted with ffprobe/ffmpeg in
// `scripts/verify_media_fixtures.py`; a browser-playback assertion here would
// be a test that either fails for an environment reason or gets deleted.

type Row = { name: string; ok: boolean; detail: string };

const PLANNED_CLASSES = [
  "discontinuities",
  "landmarks",
  "long-gop",
  "mixed-codecs",
  "multi-audio",
  "silent-video",
  "unicode-path",
  "variable-frame-rate",
];

test("every fixture resolves and matches its manifest bytes", () => {
  const rows = integrity();
  // The subject is asserted non-empty before it is walked: a check over an
  // empty corpus passes, which is the shape of vacuous pass this file is
  // written to avoid repeating.
  expect(rows.length).toBeGreaterThan(0);
  expect(rows.filter((r: Row) => !r.ok).map((r: Row) => `${r.name} (${r.detail})`))
    .toEqual([]);
});

test("the plan's eight classes each resolve to exactly one fixture", () => {
  expect(classes()).toEqual(PLANNED_CLASSES);
  for (const klass of PLANNED_CLASSES) {
    expect(byClass(klass), `${klass} must have exactly one fixture`).toHaveLength(1);
  }
});

test("each fixture records the ffprobe evidence for its own class", () => {
  // A class is a claim; `ffprobe` is the reading that backs it. Asserting the
  // claim alone would pass for a corpus where every file is secretly the same
  // media, so the recorded reading is checked for the properties that make
  // each class different.
  const silent = fixture("silent-video.mkv");
  expect(silent.ffprobe.audio_streams).toBe(0);
  expect(silent.ffprobe.video_streams).toBe(1);

  const multi = fixture("multi-audio.mkv");
  expect(multi.ffprobe.audio_streams).toBe(2);
  expect(multi.ffprobe.audio.map((a: { codec: string }) => a.codec).sort())
    .toEqual(["aac", "opus"]);

  const mixed = fixture("mixed-codecs.mkv");
  expect(mixed.ffprobe.video_codec).toBe("mpeg4");
  expect(mixed.ffprobe.audio[0].codec).toBe("mp3");

  expect(fixture("variable-frame-rate.mkv").ffprobe.video_frames).toBe(15);
  expect(fixture("discontinuities.mkv").ffprobe.video_frames).toBe(30);
  expect(fixture("long-gop.mkv").ffprobe.video_frames).toBe(100);

  const marks = fixture("landmarks.mkv");
  expect(marks.ffprobe.audio[0].codec).toBe("pcm_s16le");
  expect(marks.ffprobe.video_frames).toBe(200);
});

test("the Unicode fixture round-trips through a non-ASCII path", () => {
  const uni = byClass("unicode-path")[0];
  const nonAscii = [...uni.name].some((c) => (c.codePointAt(0) ?? 0) > 127);
  expect(nonAscii).toBe(true);
  expect(uni.name.startsWith("unicode-path/")).toBe(true);
  expect(CORPUS_DIR.endsWith("fixtures/media")).toBe(true);
  // integrity() has already read these bytes; assert the row is a real pass so
  // this test cannot be satisfied by a file that is merely named in UTF-8.
  expect(integrity().filter((r: Row) => r.name === uni.name)[0].ok).toBe(true);
});
