# Security policy

## Supported versions

strivo is in alpha. Only the latest tagged release is supported with
security fixes; older tags are not patched.

| Version | Supported |
|---------|-----------|
| 0.3.x   | ✅ (current) |
| < 0.3   | ❌ |

## Reporting a vulnerability

Please report security issues **privately**, not as a public GitHub issue.

Preferred channel: open a [private security advisory](https://github.com/revoydotdev/strivo/security/advisories/new)
on the repository. The advisory form lets us discuss and patch before the
issue becomes public.

If that is unavailable to you, email **slrevoy@mailbox.org** with a subject
line that starts with `strivo security:`.

### What to include

- A description of the issue and the impact you believe it has.
- A minimal reproducer or proof-of-concept where possible.
- Affected strivo version, OS, and any relevant configuration.

### What to expect

- Acknowledgement within **7 days**.
- A triage outcome (accepted / not-a-vulnerability / duplicate) within
  **14 days**.
- Coordinated disclosure: once a fix is ready, we agree on a release date
  and only then make the advisory public.

## Out of scope

- Issues in third-party tools strivo invokes (`ffmpeg`, `mpv`, `streamlink`,
  `yt-dlp`) — report those to the respective upstreams.
- Issues that require a malicious local user with shell access to your
  account, since they could already read your config and keyring.
- Plugin crashes from third-party `cdylib` plugins — see
  [docs/PLUGIN-MANIFEST.md](./docs/PLUGIN-MANIFEST.md); third-party plugins
  are explicitly not recommended for end users in alpha.
