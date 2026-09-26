# PVR web settings coverage

<!-- GENERATED FILE - DO NOT EDIT. -->
<!-- Source of truth: `SETTINGS_KEYS` in `crates/strivo-web/src/routes/settings.rs`. -->
<!-- Regenerate: `scripts/gen-settings-coverage.sh`   Verify: `scripts/gen-settings-coverage.sh --check` -->
<!-- CI regenerates this document and fails on any difference, so a hand-edit -->
<!-- here is a failing build, not a merge conflict waiting to happen. -->

A setting appears in the table below exactly when `POST
/api/v1/settings/update` will accept it. The list is not maintained by hand: the
handler refuses any path outside the const, so a row that is here is a setting
the API serves, and a setting the API serves is a row here.

| Column | Meaning |
| --- | --- |
| Path | The `path` field of the update request body. |
| Value | The validator the handler applies — JSON `null` clears an optional override. |
| Restart | The response's `restart_required`: the daemon holds a startup config snapshot, so new captures need a restart to see the value. |
| Control | PVR SPA modules that reference the path. |

| Path | Value | Restart | Control |
| --- | --- | --- | --- |
| `recording_dir` | server directory path | yes | 012b, 034 |
| `recording.format.format` | string, or `null` to clear | yes | 034 |
| `recording.format.bitrate_kbps` | integer, or `null` to clear | yes | 034 |
| `recording.format.video_codec` | codec string, or `null` to clear | yes | 034 |
| `recording.format.audio_codec` | codec string, or `null` to clear | yes | 034 |
| `recording.transcode` | boolean | no | 034 |
| `recording.twitch_live_from_start` | boolean | no | 034 |
| `recording.auto_vod_backfill` | boolean | no | 034 |
| `recording.auto_trim_ads` | boolean | no | 034 |
| `recording.filename_template` | non-empty string | no | 034 |
| `recording.container` | string, one of `matroska` / `mp4` / `webm` | no | 034 |
| `ui.reduce_motion` | boolean | no | 012, 034, 034b |
| `ui.verbose_status` | boolean | no | 034b |
| `notifications.desktop_enabled` | boolean | no | 034 |
| `notifications.on_go_live` | boolean | no | 034 |
| `notifications.on_recording_finished` | boolean | no | 034 |
| `notifications.on_recording_failed` | boolean | no | 034 |
| `notifications.on_vod_ready` | boolean | no | 034 |
| `notifications.webhook.enabled` | boolean | no | 034 |
| `notifications.webhook.url` | http(s) URL, or `""` to clear | no | 034 |
| `monitor_limits.max_concurrent_recordings` | integer | no | 012, 034d, 036 |
| `monitor_limits.disk_budget_reserved_gb` | integer | no | 034d |
| `plugins.*.enabled` | boolean | no | per-plugin toggle; no single control to point at |
| `archiver.enabled` | boolean | no | Creator Edition pane (absent from a PVR build) |
| `archiver.concurrent_fragments` | integer | no | Creator Edition pane (absent from a PVR build) |
| `archiver.archive_dir` | non-empty string | no | Creator Edition pane (absent from a PVR build) |
| `archiver.format` | non-empty string | no | Creator Edition pane (absent from a PVR build) |

## What this document does not cover

Two further settings write paths are deliberately out of scope, because they are
not the update allow-list and are not generated from it:
`POST /api/v1/settings/poll-interval` (re-arms the monitor immediately, so it
has its own endpoint) and `POST /api/v1/settings/platform/{platform}` (the
credential and cookie fields of the platform wizards). Both are covered by the
route tests in `crates/strivo-web/tests/routes.rs`.

Every configuration field with **no** browser control is TOML-only by design and
is documented in [`config.toml.example`](../config.toml.example), which
`scripts/check-config-example.sh` keeps in step with the `AppConfig` schema.
Recording workers retain their startup configuration, so a restart — not a
browser reload — is what applies a new recording destination or format.

This is the coverage record for the web-writable settings surface, and the
machine-checked half of the claim that the browser UI can reach each one. It
asserts nothing about nested capture-profile and plugin-owned fields beyond the
paths listed, and it does not validate platform or encoder availability.
