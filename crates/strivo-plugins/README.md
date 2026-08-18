# strivo-plugins

First-party plugins for [StriVo](https://github.com/revoydotdev/strivo),
shipped in-tree as part of the workspace. The separate `strivo-plugins`
repo was retired and folded into the host repo — plugin changes now
land in the same PR as the host changes that depend on them.

| Plugin     | Purpose                                                                 |
|------------|-------------------------------------------------------------------------|
| `crunchr`  | AI transcription + diarization + analysis (Voxtral via OpenRouter [default], Mistral direct, WhisperX/pyannote local, self-hosted Voxtral, Whisper CLI). Speaker editor, voice-sample auditioning, SRT/VTT export, mkvmerge soft-sub embed. |
| `archiver` | Recording organization + gallery rendering                              |
| `insights` | Cross-stream word frequency, topic shifts, retention proxy             |
| `editor`   | Transcript-driven cut composer (timeline / compilation / filter views) |
| `viewguard`| Live viewbot fraud-signal scoring during captures                      |

## Building

Plain workspace build — no separate clone, no submodule init:

```bash
cargo build -p strivo-plugins
```

Or just build the binary, which pulls these in:

```bash
cargo build -p strivo-bin
```

## Writing your own plugin

Implement the `strivo_core::plugin::Plugin` trait in a new crate that
depends on `strivo-core`. The trait surface is being narrowed during
the TUI deprecation; see the host CHANGELOG for the active contract.

## Licence

[MIT](LICENSE)
