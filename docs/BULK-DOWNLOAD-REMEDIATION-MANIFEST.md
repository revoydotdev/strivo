# Bulk/download UX remediation manifest

## Surface inventory
- Channel detail: recent VOD cards, per-item Download VOD, bulk button, playlist picker.
- Channel rail: bulk download toggle and progress badge.
- Recordings page: active VOD rows, progress bars, stop/delete actions.
- Monitor/settings: auto-record and Creator tandem auto-download controls.
- Backend: `/channels/{id}/bulk`, `/channels/{id}/playlists`, `/channels/{id}/vods`, `/vods/download`; SSE `BulkProgress`, `RecordingProgress`, `PlaylistList`, `ChannelVods`.

## Remediation A — contract and backend
Unify bulk/download request and response contracts, add explicit operation IDs and cancellation, expose playlist items as read-only data, ensure deterministic single-file merge/remux and final extension metadata. Add tests for round trips, cancellation, and output naming. Do not modify SPA layout except API compatibility shims.

## Remediation B — SPA surfaces
Make every bulk/download button use shared labels and semantics. Playlist navigation opens a read-only thumbnail row viewer; downloads require explicit whole-playlist, whole-channel, or multi-select confirmation. Add visible operation status/cancel affordances and make context-menu VOD download available to all editions. Do not change backend contracts except consuming A's documented endpoints.

## Review gates
- No click on a viewer/playlist row starts a download.
- Every started operation is visible, cancellable, and reports progress.
- One logical media item produces one finalized file with matching extension.
- Existing PVR and Creator builds compile; focused API/UI tests pass.
