#!/usr/bin/env python3
"""Seed the throwaway benchmark library used by scripts/bench_library.sh.

Two artefacts come out of one run:

  1. `<data_dir>/jobs.db` — N synthetic `Recording` rows in the exact
     schema/JSON shape `strivo-core`'s PersistDb reads (src/recording/
     persist.rs, src/recording/job.rs). Same schema the V07 seeder
     (scripts/seed_jobs_db.py) writes; the difference is that this one also
     creates a small pool of REAL media files, one group per characteristic
     the plan names (multi-audio, silent, VFR, mixed codec, long, Unicode
     path), and points rows at them. A row whose `output_path` is a real
     file exercises `augment_recording`'s `Path::exists()` the way a real
     library does; a labelled-only library would measure nothing there.

  2. `<media_dir>/../library-manifest.json` — ffprobe's OWN reading of each
     generated file (stream count, codecs, duration, frame rate) plus the
     per-class row counts. This is the proof that "multi-audio" really is
     two audio streams rather than a class name we invented. The harness
     copies it into the baseline; nothing is reported that ffprobe did not
     confirm.

Every row carries its class in `stream_title`, so a reader of the baseline
can see which characteristic produced which number. Nothing here touches a
real installation: the caller owns the sandbox HOME (bench_library.sh
refuses a non-throwaway one).
"""
import json
import os
import random
import sqlite3
import subprocess
import sys
import uuid
from datetime import datetime, timedelta, timezone

SCHEMA = """
CREATE TABLE IF NOT EXISTS jobs (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      TEXT NOT NULL,
    state        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    episode_dir  TEXT
);
CREATE INDEX IF NOT EXISTS idx_jobs_state ON jobs(state);
CREATE INDEX IF NOT EXISTS idx_jobs_kind_updated ON jobs(kind, updated_at DESC);
CREATE TABLE IF NOT EXISTS catalog (
    platform TEXT NOT NULL, channel_id TEXT NOT NULL, vod_id TEXT NOT NULL,
    title TEXT NOT NULL, published_at TEXT, episode_dir TEXT,
    recorded_at TEXT, transcribed_at TEXT,
    PRIMARY KEY (platform, channel_id, vod_id)
);
CREATE TABLE IF NOT EXISTS blocklist (
    platform TEXT NOT NULL, channel_id TEXT NOT NULL, vod_id TEXT NOT NULL DEFAULT '',
    reason TEXT, created_at TEXT NOT NULL,
    PRIMARY KEY (platform, channel_id, vod_id)
);
"""

PLATFORMS = ["Twitch", "YouTube", "Patreon"]
STATES = ["finished"] * 8 + ["failed"] + ["interrupted"]

# class -> (ffmpeg args, seconds, extension). Every entry is a REAL encoding,
# not a rename: `multi_audio` muxes two independent sine sources, `silent`
# emits no audio stream at all, `vfr` uses fps_mode vfr over irregular
# frame durations, `mixed_codec` deliberately pairs a different encoder
# from the other classes, `long` is simply 40x the wall time of the rest,
# and `unicode` is the same content under a non-ASCII path.
CLASSES = {
    "multi-audio": {
        "seconds": 3,
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
                 "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
                 "-f", "lavfi", "-i", "sine=frequency=880:duration=3",
                 "-map", "0:v", "-map", "1:a", "-map", "2:a"],
        "video_codec": "h264", "audio_codec": "aac", "ext": "mkv",
    },
    "silent": {
        "seconds": 3,
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10", "-an"],
        "video_codec": "h264", "audio_codec": None, "ext": "mkv",
    },
    "vfr": {
        "seconds": 3,
        # A constant-rate lavfi source dropped by a modulo selector is
        # genuinely variable frame rate: ffprobe then reports a frame count
        # that does not divide evenly into the duration.
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
                 "-f", "lavfi", "-i", "sine=frequency=330:duration=3",
                 "-vf", "select=not(mod(n\\,3))", "-fps_mode", "vfr",
                 "-map", "0:v", "-map", "1:a"],
        "video_codec": "h264", "audio_codec": "aac", "ext": "mkv",
    },
    "mixed-codec": {
        "seconds": 3,
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
                 "-f", "lavfi", "-i", "sine=frequency=220:duration=3",
                 "-map", "0:v", "-map", "1:a", "-c:v", "mpeg4", "-c:a", "mp3"],
        "video_codec": "mpeg4", "audio_codec": "mp3", "ext": "mkv",
        # Applied after the shared encoder flags (ffmpeg's codec selection is
        # last-wins) but BEFORE the output path, because ffmpeg treats
        # trailing options as belonging to a next output.
        "tail": ["-c:v", "mpeg4", "-c:a", "libmp3lame"],
    },
    "long": {
        "seconds": 120,
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
                 "-f", "lavfi", "-i", "sine=frequency=110:duration=120",
                 "-map", "0:v", "-map", "1:a"],
        "video_codec": "h264", "audio_codec": "aac", "ext": "mkv",
    },
    "unicode": {
        "seconds": 3,
        "args": ["-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10",
                 "-f", "lavfi", "-i", "sine=frequency=660:duration=3",
                 "-map", "0:v", "-map", "1:a"],
        "video_codec": "h264", "audio_codec": "aac", "ext": "mkv",
    },
}
UNICODE_STEMS = [
    "日本語 セッション 🎬", "Ωμέγα café", "Ελληνικά βίντεο", "русский архив",
    "שלום הקלטה", "한국어 녹화", "🎥 emoji-only", "ñandú ÆØÅ stream",
]
PLAIN_STEM = "synthetic-capture"

ENCODER = ["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
           "-b:v", "64k", "-threads", "1", "-y"]


def rfc3339(dt):
    return dt.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def probe(path):
    """ffprobe's own reading of the file. Returns None if ffprobe is absent."""
    try:
        out = subprocess.run(
            # -count_frames makes ffprobe decode the whole file to report a
            # real frame count. For the VFR class that count is what proves
            # the frame rate is genuinely irregular: the container's nominal
            # avg_frame_rate still claims the source rate either way.
            ["ffprobe", "-v", "error", "-count_frames", "-show_streams",
             "-show_format", "-of", "json", path],
            capture_output=True, text=True, timeout=60, check=True).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    data = json.loads(out)
    streams = data.get("streams", [])
    video = [s for s in streams if s.get("codec_type") == "video"]
    audio = [s for s in streams if s.get("codec_type") == "audio"]
    rate = video[0].get("avg_frame_rate", "0/1") if video else "0/0"
    try:
        num, den = rate.split("/")
        fps = round(int(num) / int(den), 3) if int(den) else None
    except (ValueError, ZeroDivisionError):
        fps = None
    fmt = data.get("format", {})
    return {
        "audio_streams": len(audio),
        "video_streams": len(video),
        "video_codec": video[0].get("codec_name") if video else None,
        "audio_codec": audio[0].get("codec_name") if audio else None,
        "duration_secs": round(float(fmt.get("duration", 0)), 2),
        "avg_fps": fps,
        "video_frames": int(video[0].get("nb_read_frames", 0)) if video else None,
        "bytes": int(fmt.get("size", 0)),
    }


def build_pool(media_dir, per_class):
    """Encode the real files. Returns (pool, classes) where pool is the flat
    list of {path, klass, probed} and classes maps class -> its members."""
    os.makedirs(media_dir, exist_ok=True)
    pool, classes = [], {k: [] for k in CLASSES}
    for klass, spec in CLASSES.items():
        for i in range(per_class):
            stem = UNICODE_STEMS[i % len(UNICODE_STEMS)] if klass == "unicode" else PLAIN_STEM
            name = f"{klass}-{i}-{stem}.{spec['ext']}"
            path = os.path.join(media_dir, name)
            cmd = (["ffmpeg", "-hide_banner", "-loglevel", "error"] + spec["args"]
                   + ENCODER + ["-t", str(spec["seconds"])]
                   + spec.get("tail", []) + [path])
            subprocess.run(cmd, check=True, timeout=600)
            entry = {"path": path, "class": klass, "file": name, "probed": probe(path)}
            pool.append(entry)
            classes[klass].append(entry)
    return pool, classes


def seed_db(db_path, pool, n, missing_fraction, seed):
    rng = random.Random(seed)
    conn = sqlite3.connect(db_path)
    conn.executescript(SCHEMA)
    conn.execute("PRAGMA journal_mode=WAL")
    base = datetime(2024, 1, 1, tzinfo=timezone.utc)
    rows = []
    class_counts = {}
    for i in range(n):
        entry = pool[i % len(pool)]
        klass = entry["class"]
        class_counts[klass] = class_counts.get(klass, 0) + 1
        spec = CLASSES[klass]
        job_id = str(uuid.uuid4())
        started = base + timedelta(minutes=i * 3)
        updated = started + timedelta(minutes=rng.randint(5, 240))
        state = STATES[i % len(STATES)]
        # A deliberate slice of rows point at a path that does not exist, so
        # `file_exists: false` (the dead-journal overlay the SPA renders) is
        # part of the measured library rather than a hypothetical. They are
        # restricted to non-finished states on purpose: the daemon's
        # `prune_missing_recordings` (src/recording/persist.rs:169) deletes
        # finished rows whose file is gone, and its doc comment says failed
        # and interrupted rows are deliberately spared. Letting finished rows
        # point at a missing path silently shrinks the library by ~5% before
        # the first request, which would make every count in the baseline a
        # lie about its own size.
        missing = state != "finished" and rng.random() < missing_fraction
        output = (f"{os.path.join(os.path.dirname(db_path), 'gone', f'{job_id}.mkv')}"
                  if missing else entry["path"])
        payload = {
            "id": job_id,
            "channel_id": f"channel-{i % 500}",
            "channel_name": f"Channel {i % 500}",
            "platform": PLATFORMS[i % len(PLATFORMS)],
            "state": "Finished" if state == "finished" else "Failed",
            "output_path": output,
            "started_at": rfc3339(started),
            "bytes_written": rng.randint(10_000_000, 5_000_000_000),
            "duration_secs": float(spec["seconds"]) if klass == "long" else rng.uniform(300, 14400),
            "transcode": bool(i % 3 == 0),
            "error": "stream ended unexpectedly" if state == "failed" else None,
            "stream_title": f"[{klass}] synthetic stream #{i}",
            "watched": bool(i % 5 == 0),
            "playlist": None,
            "thumbnail_url": None,
            "source_url": None,
            "video_codec": spec["video_codec"],
            "audio_codec": spec["audio_codec"],
        }
        rows.append((job_id, "Recording", json.dumps(payload), state,
                     rfc3339(started), rfc3339(updated), 0, payload["error"], None))
    conn.executemany(
        "INSERT INTO jobs (id, kind, payload, state, created_at, updated_at,"
        " attempts, last_error, episode_dir) VALUES (?,?,?,?,?,?,?,?,?)", rows)
    conn.commit()
    conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")
    count = conn.execute("SELECT COUNT(*) FROM jobs").fetchone()[0]
    conn.close()
    return count, class_counts


def main():
    if len(sys.argv) < 3:
        print("usage: seed_bench_library.py <data_dir> <media_dir> <rows> "
              "[per_class] [missing_fraction] [seed]", file=sys.stderr)
        return 2
    data_dir, media_dir = sys.argv[1], sys.argv[2]
    rows_n = int(sys.argv[3])
    per_class = int(sys.argv[4]) if len(sys.argv) > 4 else 2
    missing_fraction = float(sys.argv[5]) if len(sys.argv) > 5 else 0.05
    seed = int(sys.argv[6]) if len(sys.argv) > 6 else 9

    pool, classes = build_pool(media_dir, per_class)
    count, class_counts = seed_db(os.path.join(data_dir, "jobs.db"), pool,
                                  rows_n, missing_fraction, seed)
    manifest = {
        "rows": count,
        "requested_rows": rows_n,
        "seed": seed,
        "missing_path_fraction": missing_fraction,
        "classes": {k: [e["file"] for e in v] for k, v in classes.items()},
        "class_row_counts": class_counts,
        "media_files": pool,
        "ffprobe_available": any(e["probed"] for e in pool),
        "not_captured": ([] if any(e["probed"] for e in pool)
                         else ["media_file_characteristics (ffprobe unavailable)"]),
    }
    out = os.path.join(os.path.dirname(media_dir), "library-manifest.json")
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=2, ensure_ascii=False)
    print(f"seeded {count} rows, {len(pool)} real media files; manifest -> {out}",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
