# Playback and download ownership

## Playback

The web multistream view owns one controller per tile mount. A source/channel
identifier is provenance, not a universal player identity: duplicate views of
the same source receive separate controller instances. Layout repainting may
rebuild chrome, but controller reconciliation reuses unaffected mounts and
destroys only controllers no longer present. Tile volume/preferences remain
keyed by source so duplicate views retain the same saved preference.

The Rust `playback::MpvController` is a separate single-player desktop
surface. Its `play` operation intentionally replaces its own mpv process; it
is not used by the web multistream route. Rust playback-session coordination
that is needed by future backend consumers is represented by the generation-
scoped `stream::SessionRegistry`: stale cleanup can release only the generation
it created, and releasing one lease cannot cancel an unrelated lease.

## Download and recording jobs

The web UI submits VOD/upload downloads through `POST /api/v1/vods/download`.
The request is validated as `{url, channel_name, platform, post_title?}` and is
translated by the daemon into the canonical `DownloadVod` intent. Live capture
uses the corresponding `POST /api/v1/recordings` intent. Both become daemon-
owned recording jobs and their state is reconstructed from the durable jobs
store/SSE snapshot after reconnect.

Uploaded/VOD actions use the VOD's persisted `channel_id` and `platform` when
the channel rail cache is unavailable, and the channel context menu exposes a
download action only for a concrete resolved finite VOD/upload. A channel tab
URL is never guessed as a video URL.

The current IPC submission is fire-and-forget and acknowledges queueing with a
202 response; the response does not yet carry a client idempotency key or job
UUID. The daemon emits the durable job identity shortly after acceptance.
Callers that need intentional parallel downloads should wait for the current
job model to grow an explicit idempotency/duplicate policy rather than relying
on button disabling.

On restart, active jobs are reconciled by the existing recording persistence
policy. A live capture that cannot be resumed is represented as interrupted or
failed; completion is emitted only after output finalization.
