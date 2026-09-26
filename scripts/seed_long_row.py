"""Journal one finished recording pointing at an already-encoded media file.

`scripts/bench_media_pool.sh` needs a journal row for a source it generated
itself, in the same shape as the row it writes for the wedge FIFO. It is three
lines of `sqlite3` and it was inline in that script twice already, so it is one
file here instead of a third copy.

Usage: scripts/seed_long_row.py <jobs.db> <media path> <duration secs>

Prints the new job id. Exits non-zero if the row cannot be written, so a
harness run cannot silently measure nothing.
"""

import json
import os
import sqlite3
import sys
import uuid
from datetime import datetime, timezone


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    db, media, secs = sys.argv[1], sys.argv[2], int(sys.argv[3])
    size = os.path.getsize(media)
    if size == 0:
        print(f"seed_long_row: {media} is empty", file=sys.stderr)
        return 1
    now = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")
    job_id = str(uuid.uuid4())
    payload = json.dumps({
        "id": job_id, "channel_id": "", "channel_name": "long",
        "platform": "Twitch", "state": "Finished", "output_path": media,
        "started_at": now, "bytes_written": size, "duration_secs": secs,
        "transcode": False, "error": None, "stream_title": "long",
        "watched": False, "playlist": None, "thumbnail_url": None,
        "source_url": None,
    })
    conn = sqlite3.connect(db)
    conn.execute(
        "INSERT INTO jobs (id, kind, payload, state, created_at, updated_at,"
        " attempts, last_error, episode_dir)"
        " VALUES (?, 'Recording', ?, 'finished', ?, ?, 0, NULL, NULL)",
        (job_id, payload, now, now),
    )
    conn.commit()
    conn.close()
    print(job_id)
    return 0


if __name__ == "__main__":
    sys.exit(main())
