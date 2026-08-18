# Contributing to strivo

Thanks for taking the time to look at the code. strivo is in alpha and the
internals are still moving, so the most useful contributions right now are
bug reports with reproducers and small, focused fixes.

## Reporting bugs

File an issue using the **Bug report** template. Include:

- OS and architecture (`uname -a` on Unix, `winver` on Windows).
- `strivo --version` and `rustc --version` if you built from source.
- Steps to reproduce, expected vs actual behaviour.
- Relevant lines from `~/.local/state/strivo/strivo.log`
  (or `strivo log path` to find it). Re-run with `-l debug` if the log is
  thin.

Please redact OAuth client IDs / secrets and cookie blobs from any log
excerpts you paste.

## Security issues

Please do **not** open a public issue. See [SECURITY.md](./SECURITY.md) for
the private-disclosure path.

## Local build

```bash
git clone https://github.com/revoydotdev/strivo.git
cd strivo
cargo build
```

The first-party plugins live in the workspace at
[`crates/strivo-plugins/`](./crates/strivo-plugins). They were folded
into this repo when the separate `strivo-plugins` submodule was retired;
plugin changes land in the same PR as host changes that depend on them.

## Before opening a pull request

Run the same gates CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If any of those fail locally, CI will fail too. Style fixes can be pre-applied
with `cargo fmt --all`.

## Commit style

Conventional-commit prefixes (`feat:`, `fix:`, `chore:`, `refactor:`, `ci:`,
`docs:`, `test:`, `perf:`). Keep the subject short (≤ 70 chars) and put the
"why" in the body when it's non-obvious. One concern per commit; prefer a
rebased, linear history.

Do **not** include `Co-Authored-By: Claude`, `Generated with Claude Code`, or
any AI / Anthropic attribution in commit messages, PR descriptions, or code
comments — the human author is the sole author of record.

## CHANGELOG entries

User-visible changes belong under `## [Unreleased]` in
[CHANGELOG.md](./CHANGELOG.md), in the appropriate `Added / Changed / Fixed /
Removed / Deprecated / Security` bucket. Internal refactors that do not
affect users do not need an entry.

## Scope and design

The architectural shape of the project (platform trait, plugin event bus,
daemon / client split, ratatui-first UI) is described in the README's
*Architecture* and *Design rationale* sections. Larger changes that move any
of those load-bearing pieces should be discussed in an issue first so we can
agree on the approach before the diff lands.
