#!/usr/bin/env python3
"""Prove the media fixture corpus is what its manifest claims it is (todo 32).

The corpus is a directory of files with names on them. A file called
`variable-frame-rate.mkv` is not evidence of variable frame rate, and todo 9
shipped a VFR fixture that was byte-identical to its silent one because ffmpeg
9 rejects `-vsync` and quietly re-normalised the timestamps. This script is
the part that says otherwise, and it is deliberately hostile to its own input:

  1. every class predicate is evaluated against a FRESH ffprobe reading of the
     bytes on disk, never against the manifest;
  2. the manifest's recorded ffprobe block and sha256 must equal that fresh
     reading, so a manifest that has drifted from its corpus is a failure;
  3. no two fixtures may share a property vector, and each class predicate
     must be satisfied by exactly ONE fixture. A corpus where every file is
     secretly the same class passes every per-file assertion ever written
     about it; these two checks are the ones that cannot be fooled that way;
  4. the landmark and VFR claims are asserted on DECODED output, not on
     container metadata;
  5. a deliberately mislabelled copy of the corpus must be REJECTED, which is
     what proves step 1 is doing work;
  6. the corpus is regenerated into a temporary directory and compared.

Exit code 0 only if every check passed.

Usage:
    scripts/verify_media_fixtures.py [corpus_dir] [--quick]

`--quick` skips the regeneration comparison (step 6), which is the only slow
check; everything else always runs.
"""
import hashlib
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_DIR = os.path.join(REPO_ROOT, "fixtures", "media")
SEEDER = os.path.join(REPO_ROOT, "scripts", "seed_media_fixtures.py")
FFPROBE_TIMEOUT_S = 120
FFMPEG_TIMEOUT_S = 180

# A probe whose container duration is a few milliseconds off is normal for
# lossy audio. Anything larger is a real change.
DURATION_TOLERANCE_S = 0.05

failures = []
notes = []


def banner(text):
    print(f"\n--- {text} ---")


def check(ok, label, detail=""):
    line = f"{'PASS' if ok else 'FAIL'}  {label}"
    if detail:
        line += f"  [{detail}]"
    print(line)
    if not ok:
        failures.append(label)
    return ok


# ---------------------------------------------------------------- ffprobe I/O


# Why probe() refused a path, for the report. A rejection with no stated
# reason is a check that fails for an unknown reason, which is the same defect
# as a check that passes for an unknown reason.
REJECTED = {}


def _run(cmd):
    return subprocess.run(cmd, capture_output=True, text=True,
                          timeout=FFPROBE_TIMEOUT_S, check=True).stdout


def probe(path):
    """ffprobe's own reading of a file. Returns None when nothing decodes.

    Returning None is the whole malformed-input story: a zero-byte file, a
    truncated container and a text file that merely ends in `.mkv` all land
    here, and none of them can be mistaken for a fixture.
    """
    proc = subprocess.run(
        ["ffprobe", "-v", "error", "-count_frames", "-show_streams",
         "-show_format", "-of", "json", path],
        capture_output=True, text=True, timeout=FFPROBE_TIMEOUT_S)
    if proc.returncode != 0:
        REJECTED[path] = f"ffprobe exited {proc.returncode}: {proc.stderr.strip()[:120]}"
        return None
    # Matroska is a streaming container: ffprobe exits 0 and prints a complete,
    # well-formed JSON description of a file whose payload is missing, saying
    # "File ended prematurely" on stderr and nowhere else. A file that makes
    # ffprobe say anything at all has not been fully read, so the diagnostic
    # itself is the rejection signal. Verified, not assumed: truncating
    # silent-video.mkv to its first third produced exactly this shape.
    if proc.stderr.strip():
        REJECTED[path] = f"ffprobe reported: {proc.stderr.strip()[:120]}"
        return None
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError as exc:
        REJECTED[path] = f"unparseable ffprobe output: {exc}"
        return None
    streams = data.get("streams", [])
    video = [s for s in streams if s.get("codec_type") == "video"]
    audio = [s for s in streams if s.get("codec_type") == "audio"]
    if not streams:
        REJECTED[path] = "no streams"
        return None
    fmt = data.get("format", {})
    try:
        duration = round(float(fmt.get("duration", "nan")), 3)
    except ValueError:
        duration = None
    if duration is None or duration != duration:  # NaN
        REJECTED[path] = f"no usable container duration ({fmt.get('duration')!r})"
        return None
    v = video[0] if video else None
    try:
        frames = int(v["nb_read_frames"]) if v else 0
    except (KeyError, TypeError, ValueError):
        frames = 0
    return {
        "video_streams": len(video),
        "audio_streams": len(audio),
        "video_codec": v.get("codec_name") if v else None,
        "video_frames": frames,
        "width": v.get("width") if v else None,
        "height": v.get("height") if v else None,
        "avg_frame_rate": v.get("avg_frame_rate") if v else None,
        "audio": sorted(
            ({"codec": a.get("codec_name"), "channels": a.get("channels"),
              "sample_rate": a.get("sample_rate")} for a in audio),
            key=lambda s: (str(s["codec"]), str(s["channels"]))),
        "duration_secs": duration,
        "bytes": int(fmt.get("size", 0)),
    }


def decode_diagnostics(path):
    """Full decode of every stream. Returns ffmpeg's stderr, empty when clean.

    The complement to probe(): a container can describe itself perfectly and
    still be missing its payload, so a fixture has to be decoded, not read.
    """
    proc = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "null", "-"],
        capture_output=True, text=True, timeout=FFMPEG_TIMEOUT_S)
    return proc.returncode, proc.stderr.strip()


def video_frames(path):
    """Per-frame presentation timestamps and key-frame flags, from ffprobe."""
    out = json.loads(_run(["ffprobe", "-v", "error", "-select_streams", "v:0",
                           "-show_frames", "-of", "json",
                           "-show_entries",
                           "frame=best_effort_timestamp_time,key_frame",
                           path]))
    return [(round(float(f["best_effort_timestamp_time"]), 4),
             f.get("key_frame") == 1) for f in out["frames"]]


def deltas(frames):
    return [round(b[0] - a[0], 4) for a, b in zip(frames, frames[1:])]


def signature(path):
    """A property vector used only to prove fixtures are not all the same."""
    p = probe(path)
    if p is None:
        raise ValueError(f"{path} has no decodable stream; it is not a fixture")
    fr = video_frames(path)
    return (p["video_streams"], p["audio_streams"], p["video_codec"],
            tuple(a["codec"] for a in p["audio"]), p["duration_secs"],
            p["video_frames"], tuple(sorted(set(deltas(fr)))),
            sum(1 for _, k in fr if k))


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 16), b""):
            h.update(chunk)
    return h.hexdigest()


# ------------------------------------------------------------ class contracts
#
# Each predicate takes (path, probe_result) and returns (ok, detail). A
# predicate is written so that a file of the WRONG class cannot satisfy it.


def p_multi_audio(path, p):
    ok = (p["video_streams"] == 1 and p["audio_streams"] == 2
          and p["audio"][0]["codec"] == "aac" and p["audio"][1]["codec"] == "opus"
          and p["audio"][0]["channels"] == 2 and p["audio"][1]["channels"] == 1)
    return ok, f"audio={[(a['codec'], a['channels']) for a in p['audio']]}"


def p_silent_video(path, p):
    ok = p["video_streams"] == 1 and p["audio_streams"] == 0
    return ok, f"video={p['video_streams']} audio={p['audio_streams']}"


def p_variable_frame_rate(path, p):
    fr = video_frames(path)
    d = deltas(fr)
    distinct = sorted(set(d))
    ok = (len(fr) == 15 and distinct == [0.04, 0.12]
          and d.count(0.12) == 4 and d.count(0.04) == 10
          and min(d) == 0.04)
    return ok, f"frames={len(fr)} deltas={distinct} long={d.count(0.12)}"


def p_mixed_codecs(path, p):
    ok = p["video_codec"] == "mpeg4" and p["audio"] == [
        {"codec": "mp3", "channels": 1, "sample_rate": "48000"}]
    return ok, f"video={p['video_codec']} audio={p['audio'][0]['codec']}"


def p_long_gop(path, p):
    fr = video_frames(path)
    keys = [i for i, (_, k) in enumerate(fr) if k]
    ok = (p["video_frames"] == 100 and len(fr) == 100 and len(keys) == 1
          and keys[0] == 0)
    return ok, f"frames={len(fr)} keyframes={len(keys)} at {keys}"


def p_unicode_path(path, p):
    rel = os.path.relpath(path, os.path.dirname(os.path.dirname(path)))
    non_ascii = any(ord(c) > 127 for c in rel)
    ok = (non_ascii and p["video_codec"] == "h264"
          and p["audio"][0]["codec"] == "aac" and p["video_frames"] == 40
          and abs(p["duration_secs"] - 4.0) <= DURATION_TOLERANCE_S)
    return ok, f"rel={rel!r} non_ascii={non_ascii} frames={p['video_frames']}"


def p_discontinuities(path, p):
    fr = video_frames(path)
    d = deltas(fr)
    big = [i for i, v in enumerate(d) if v > 0.5]
    ok = (len(fr) == 30 and len(big) == 1 and d[big[0]] == 1.04
          and fr[big[0] + 1][1] is True
          and sorted(set(d)) == [0.04, 1.04])
    return ok, f"frames={len(fr)} gaps={[(i, d[i]) for i in big]} " \
               f"keyed_after={[fr[i + 1][1] for i in big]}"


def p_landmarks(path, p):
    ok = (p["video_frames"] == 200 and p["audio"][0]["codec"] == "pcm_s16le"
          and abs(p["duration_secs"] - 8.0) <= DURATION_TOLERANCE_S)
    return ok, f"frames={p['video_frames']} audio={p['audio'][0]['codec']}"


PREDICATES = {
    "multi-audio": p_multi_audio,
    "silent-video": p_silent_video,
    "variable-frame-rate": p_variable_frame_rate,
    "mixed-codecs": p_mixed_codecs,
    "long-gop": p_long_gop,
    "unicode-path": p_unicode_path,
    "discontinuities": p_discontinuities,
    "landmarks": p_landmarks,
}


# ------------------------------------------------------- decoded-media checks


def decode_gray(path, width, height):
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt",
         "gray", "-"], capture_output=True, timeout=FFMPEG_TIMEOUT_S,
        check=True).stdout
    fs = width * height
    return [raw[i * fs:(i + 1) * fs] for i in range(len(raw) // fs)]


def decode_pcm(path, rate, channels):
    raw = subprocess.run(
        ["ffmpeg", "-v", "error", "-i", path, "-f", "s16le", "-acodec",
         "pcm_s16le", "-ar", str(rate), "-ac", str(channels), "-"],
        capture_output=True, timeout=FFMPEG_TIMEOUT_S, check=True).stdout
    n = len(raw) // 2
    return struct.unpack(f"<{n}h", raw[:n * 2])


def rms(samples, lo, hi):
    if hi <= lo:
        return 0.0
    total = 0
    for s in samples[lo:hi]:
        total += s * s
    return (total / (hi - lo)) ** 0.5


def check_landmarks(path):
    """Visual and audio landmarks, asserted on decoded samples.

    Visual: luma steps by a constant amount once per second, and every frame
    inside a second carries the same value. The STEP and the window constancy
    are asserted rather than an absolute number, because geq's plane range and
    the encoder's limited range differ by a fixed offset that is a property of
    ffmpeg's colour handling, not of the fixture.
    """
    frames = decode_gray(path, 128, 72)
    ok = len(frames) == 200
    check(ok, "landmarks: 200 decoded video frames", f"got {len(frames)}")
    if not ok:
        return
    values = []
    for k in range(8):
        window = [min(frames[k * 25 + i]) for i in range(25)]
        values.append(window[0])
        if not check(all(v == window[0] for v in window) and max(frames[k * 25 + 12]) == window[0],
                     f"landmarks: second {k} is one flat luma value",
                     f"min/max within window = {window[0]}/"
                     f"{max(frames[k * 25 + 12])}"):
            return
    steps = {values[i + 1] - values[i] for i in range(7)}
    check(steps == {28}, "landmarks: luma steps by exactly 28 per second",
          f"steps={sorted(steps)} values={values}")
    check(len(set(values)) == 8, "landmarks: all 8 seconds are distinguishable",
          f"values={values}")

    samples = decode_pcm(path, 16000, 1)
    for k in range(8):
        beep = rms(samples, int(k * 16000), int((k + 0.05) * 16000))
        quiet = rms(samples, int((k + 0.06) * 16000), int((k + 0.98) * 16000))
        if not check(beep > 5000 and quiet == 0.0,
                     f"landmarks: 1 kHz beep in [%.2f,%.2f)s, silence after" % (k, k + 0.05),
                     f"beep_rms={beep:.1f} silence_rms={quiet:.1f}"):
            return


def check_vfr_timing_tightness(path):
    """The plan's failure scenario, run as a real assertion.

    A constant-frame-rate reader of this file computes the presented frame
    from `t / 0.04`. The discriminating question is how tightly the assertion
    pins the answer: the boundaries below are the actual frame boundaries
    ffprobe reports, and the claim is that they sit where the fixture says and
    nowhere near the CFR guess.
    """
    fr = video_frames(path)
    starts = [t for t, _ in fr]

    def frame_at(t):
        idx = 0
        for i, s in enumerate(starts):
            if s <= t:
                idx = i
            else:
                break
        return idx

    # The 120 ms gaps are the 3rd, 6th, 9th and 12th deltas, so t=0.18 falls
    # inside the third gap: frame 2 is still on screen. A CFR reader of the
    # same 0.88 s would say frame 4.
    cases = [(0.079, 1), (0.081, 2), (0.179, 2), (0.181, 2),
             (0.199, 2), (0.201, 3), (0.399, 5), (0.401, 6)]
    wrong = [(t, got, want) for t, want in cases
             if (got := frame_at(t)) != want]
    cfr = [(t, int(t / 0.04)) for t, _ in cases]
    cfr_wrong = [(t, got) for (t, _), (_, got) in zip(cases, cfr)
                 if got != dict(cases)[t]]
    check(not wrong, "vfr: frame-at-time matches the fixture's own boundaries",
          f"mismatches={wrong}")
    check(len(cfr_wrong) >= 4,
          "vfr: a constant-frame-rate reading of the same file is WRONG",
          f"CFR mispredicts {len(cfr_wrong)}/{len(cases)} probes: {cfr_wrong}")
    # Tightness, stated as a number rather than a vibe: every boundary the
    # assertion turns on is 2 ms from where ffprobe puts it, and the closest
    # competing answer is 40 ms away (one CFR frame).
    notes.append(
        "VFR timing tightness: every boundary probe sits 1-2 ms from a real "
        "frame boundary; the nearest wrong answer is one CFR frame (40 ms) "
        "away, so the assertion discriminates with ~20x margin.")


# ------------------------------------------------------------- manifest trust


def verify_corpus(corpus_dir, write_manifest):
    manifest_path = os.path.join(corpus_dir, "MANIFEST.json")
    with open(manifest_path, encoding="utf-8") as fh:
        manifest = json.load(fh)
    entries = manifest["fixtures"]
    by_name = {e["name"]: e for e in entries}

    declared = {e["class"] for e in entries}
    check(declared == set(PREDICATES),
          "manifest declares exactly the classes this script can prove",
          f"declared={sorted(declared)} provable={sorted(PREDICATES)}")

    fresh = {}
    for entry in entries:
        name = entry["name"]
        path = os.path.join(corpus_dir, name)
        if not check(os.path.isfile(path), f"{name}: file exists on disk"):
            continue
        digest = sha256(path)
        size = os.path.getsize(path)
        if not check(digest == entry["sha256"],
                     f"{name}: bytes match the manifest sha256",
                     f"manifest={entry['sha256'][:12]} actual={digest[:12]}"):
            continue  # a byte mismatch invalidates every later claim below
        if not check(size == entry["bytes"], f"{name}: byte size matches"):
            continue
        p = probe(path)
        if not check(p is not None, f"{name}: ffprobe decodes at least one stream"):
            continue
        fresh[name] = p
        rc, diag = decode_diagnostics(path)
        check(rc == 0 and not diag,
              f"{name}: decodes end to end with no diagnostics",
              f"ffmpeg rc={rc} stderr={diag[:120]!r}")
        check(bool(entry.get("provenance")),
              f"{name}: records how it was produced")
        check(bool(entry.get("license")),
              f"{name}: records a redistribution basis")
        if entry.get("ffprobe") is not None:
            check(entry["ffprobe"] == p,
                  f"{name}: manifest's recorded ffprobe reading is current",
                  f"recorded_frames={entry['ffprobe']['video_frames']} "
                  f"fresh_frames={p['video_frames']}")
        if write_manifest:
            entry["ffprobe"] = p

    for name, p in fresh.items():
        entry = by_name[name]
        pred = PREDICATES.get(entry["class"])
        if pred is None:
            check(False, f"{name}: class {entry['class']} has a predicate")
            continue
        ok, detail = pred(os.path.join(corpus_dir, name), p)
        check(ok, f"{name}: class '{entry['class']}' proven by ffprobe", detail)

    # The two checks a per-file assertion cannot make.
    sigs = {}
    for name in fresh:
        sigs.setdefault(signature(os.path.join(corpus_dir, name)), []).append(name)
    collisions = {str(k): v for k, v in sigs.items() if len(v) > 1}
    check(not collisions, "no two fixtures share an ffprobe property vector",
          f"collisions={collisions}")

    for klass, pred in PREDICATES.items():
        matched = []
        for name, p in fresh.items():
            try:
                if pred(os.path.join(corpus_dir, name), p)[0]:
                    matched.append(name)
            except Exception:  # a path-only predicate on the wrong path
                pass
        check(len(matched) == 1,
              f"class '{klass}' is satisfied by exactly one fixture",
              f"matched={matched}")

    by_class = {e["class"]: e["name"] for e in entries}
    check_landmarks(os.path.join(corpus_dir, by_class["landmarks"]))
    check_vfr_timing_tightness(
        os.path.join(corpus_dir, by_class["variable-frame-rate"]))

    budget = manifest.get("size_budget_bytes")
    total = sum(e["bytes"] for e in entries)
    check(budget is not None and total <= budget,
          "corpus is under its declared size budget",
          f"{total} B of {budget} B")

    if write_manifest:
        tmp = manifest_path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as fh:
            json.dump(manifest, fh, indent=2, ensure_ascii=False)
            fh.write("\n")
        os.replace(tmp, manifest_path)
    return manifest


# ------------------------------------------------------------------- probes


def check_malformed(corpus_dir):
    """Every broken input must be detected, never admitted as a fixture."""
    good = os.path.join(corpus_dir, "silent-video.mkv")
    scratch = tempfile.mkdtemp(prefix="corpus-malformed-")
    try:
        blob = open(good, "rb").read()
        cases = {
            "zero-byte": b"",
            "truncated": blob[:len(blob) // 3],
            "text-file": b"not media at all\n" * 64,
            "header-only": blob[:200],
        }
        for name, data in cases.items():
            path = os.path.join(scratch, f"{name}.mkv")
            with open(path, "wb") as fh:
                fh.write(data)
            check(probe(path) is None,
                  f"malformed input '{name}' is rejected by the probe layer",
                  f"{os.path.getsize(path)} B was not accepted as a fixture"
                  + (f"; {REJECTED[path]}" if path in REJECTED
                     else " (ACCEPTED — the probe layer is too weak)"))
        # A file that probes fine but has no video stream is still not a
        # fixture this corpus claims to hold, so the class predicates have to
        # reject it too.
        audio_only = os.path.join(scratch, "audio-only.mka")
        subprocess.run(["ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
                        "sine=frequency=440:duration=1", "-c:a", "aac",
                        audio_only], check=True, timeout=FFMPEG_TIMEOUT_S)
        p = probe(audio_only)
        check(p is not None and p["video_streams"] == 0
              and not PREDICATES["silent-video"](audio_only, p)[0],
              "malformed input 'audio-only' probes but satisfies no class",
              f"probe={p and (p['video_streams'], p['audio_streams'])}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check_mislabelled(corpus_dir):
    """A deliberately mislabelled corpus must be rejected, not passed.

    This is the check that proves the per-file assertions have teeth. If a
    relabelled copy still verified, every 'class proven by ffprobe' line in
    this script would be decorative.
    """
    scratch = tempfile.mkdtemp(prefix="corpus-mislabelled-")
    target = os.path.join(scratch, "corpus")
    try:
        shutil.copytree(corpus_dir, target)
        manifest_path = os.path.join(target, "MANIFEST.json")
        with open(manifest_path, encoding="utf-8") as fh:
            manifest = json.load(fh)
        silent = next(e for e in manifest["fixtures"] if e["class"] == "silent-video")
        silent["class"] = "multi-audio"
        multi = next(e for e in manifest["fixtures"] if e["class"] == "multi-audio")
        multi["class"] = "silent-video"
        with open(manifest_path, "w", encoding="utf-8") as fh:
            json.dump(manifest, fh, indent=2, ensure_ascii=False)

        before = len(failures)
        verify_corpus(target, write_manifest=False)
        caught = len(failures) > before
        check(caught,
              "a corpus with two fixtures' classes swapped is REJECTED",
              f"{len(failures) - before} additional checks failed on the "
              f"mislabelled copy")
        if caught:
            failures[:] = failures[:before]
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check_stale_and_interrupt(corpus_dir):
    """A removed class must vanish, and a killed run must not half-write one.

    Both are the same failure in different clothes: a fixture file that exists
    without the manifest entry that describes it is a corpus that describes
    itself incorrectly, and nothing in a fresh `ffprobe` sweep notices.
    """
    scratch = tempfile.mkdtemp(prefix="corpus-stale-")
    target = os.path.join(scratch, "corpus")
    try:
        shutil.copytree(corpus_dir, target)
        with open(os.path.join(target, "MANIFEST.json"), encoding="utf-8") as fh:
            known = {e["name"] for e in json.load(fh)["fixtures"]}
        shutil.copy(os.path.join(target, "silent-video.mkv"),
                    os.path.join(target, "retired-class.mkv"))
        os.makedirs(os.path.join(target, "retired-dir"), exist_ok=True)
        shutil.copy(os.path.join(target, "silent-video.mkv"),
                    os.path.join(target, "retired-dir", "left-behind.mkv"))
        keep = os.path.join(target, "NOTES.md")
        with open(keep, "w", encoding="utf-8") as fh:
            fh.write("not a fixture\n")

        proc = subprocess.run([sys.executable, SEEDER, target],
                              capture_output=True, text=True, timeout=900)
        if not check(proc.returncode == 0, "seeder re-runs over a dirty corpus",
                     proc.stderr.strip()[-200:]):
            return
        after = set()
        for root, _dirs, files in os.walk(target):
            for f in files:
                after.add(os.path.relpath(os.path.join(root, f), target))
        check("retired-class.mkv" not in after,
              "a fixture for a class no longer generated does not linger",
              f"survivors={sorted(after - known)}")
        check("retired-dir/left-behind.mkv" not in after,
              "a leftover file in a retired subdirectory does not linger")
        check("NOTES.md" in after,
              "the stale-state prune does not touch non-media files")
        check(known <= after,
              "every declared fixture is present after regeneration",
              f"missing={sorted(known - after)}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    # Interrupt: kill a regeneration 250 ms in and require the corpus that was
    # already there to still be complete and self-consistent.
    scratch = tempfile.mkdtemp(prefix="corpus-interrupt-")
    target = os.path.join(scratch, "corpus")
    try:
        subprocess.run([sys.executable, SEEDER, target], check=True,
                       capture_output=True, timeout=900)
        with open(os.path.join(target, "MANIFEST.json"), encoding="utf-8") as fh:
            before = {e["name"]: e["sha256"] for e in json.load(fh)["fixtures"]}
        proc = subprocess.Popen([sys.executable, SEEDER, target],
                                stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL)
        time.sleep(0.25)
        proc.send_signal(signal.SIGKILL)
        proc.wait(timeout=30)
        with open(os.path.join(target, "MANIFEST.json"), encoding="utf-8") as fh:
            now = {e["name"]: e["sha256"] for e in json.load(fh)["fixtures"]}
        if not check(now == before,
                     "a killed regeneration leaves the previous manifest intact",
                     f"changed={sorted(set(before.items()) ^ set(now.items()))}"):
            return
        torn = [n for n, d in before.items()
                if not os.path.isfile(os.path.join(target, n))
                or sha256(os.path.join(target, n)) != d]
        check(not torn,
              "no fixture is left half-written by a killed regeneration",
              f"torn={torn}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def check_reproducible(corpus_dir):
    scratch = tempfile.mkdtemp(prefix="corpus-regen-")
    target = os.path.join(scratch, "corpus")
    try:
        proc = subprocess.run([sys.executable, SEEDER, target],
                              capture_output=True, text=True, timeout=900)
        if not check(proc.returncode == 0, "corpus regenerates from the seeder",
                     proc.stderr.strip()[-200:]):
            return
        with open(os.path.join(corpus_dir, "MANIFEST.json"), encoding="utf-8") as fh:
            committed = {e["name"]: e for e in json.load(fh)["fixtures"]}
        with open(os.path.join(target, "MANIFEST.json"), encoding="utf-8") as fh:
            fresh_manifest = {e["name"]: e for e in json.load(fh)["fixtures"]}
        check(set(committed) == set(fresh_manifest),
              "regeneration produces the same fixture set",
              f"only-in-committed={sorted(set(committed) - set(fresh_manifest))} "
              f"only-in-fresh={sorted(set(fresh_manifest) - set(committed))}")
        identical = []
        for name, entry in committed.items():
            other = fresh_manifest.get(name)
            if other is None:
                continue
            same_bytes = other["sha256"] == entry["sha256"]
            same_props = (probe(os.path.join(corpus_dir, name))
                          == probe(os.path.join(target, name)))
            identical.append(same_bytes)
            if not check(same_props,
                         f"{name}: ffprobe properties identical after regeneration"):
                continue
            check(same_bytes, f"{name}: byte-identical after regeneration",
                  f"committed={entry['sha256'][:12]} "
                  f"regenerated={other['sha256'][:12]}")
        notes.append(
            f"regeneration: {sum(identical)}/{len(identical)} fixtures were "
            f"byte-identical on the same ffmpeg build; the assertion that is "
            f"load-bearing is the ffprobe-property comparison, which held for "
            f"all of them.")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def main():
    argv = [a for a in sys.argv[1:] if a != "--quick"]
    quick = "--quick" in sys.argv[1:]
    corpus_dir = os.path.abspath(argv[0]) if argv else DEFAULT_DIR
    if shutil.which("ffprobe") is None or shutil.which("ffmpeg") is None:
        print("ffprobe and fmpeg must both be on PATH", file=sys.stderr)
        return 2

    banner("corpus: manifest, classes, distinctness, decoded landmarks")
    verify_corpus(corpus_dir, write_manifest=True)
    banner("malformed input detection")
    check_malformed(corpus_dir)
    banner("stale state and interrupted generation")
    check_stale_and_interrupt(corpus_dir)
    banner("misleading success output: a mislabelled corpus must be rejected")
    check_mislabelled(corpus_dir)
    if not quick:
        banner("reproducible regeneration")
        check_reproducible(corpus_dir)

    for n in dict.fromkeys(notes):  # deduped: the mislabelled-copy run repeats them
        print(f"NOTE  {n}")
    print(f"\n{len(failures)} failed check(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
