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

Run the same gates CI runs (`.github/workflows/ci.yml`). Locally:

```bash
# fmt
cargo fmt --all -- --check
# clippy, default edition and Creator edition
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p strivo-core -p strivo-web -p strivo-bin --features creator --all-targets --locked -- -D warnings
# tests: release lane, then debug lane (debug-only paths such as STRIVO_DEV_UNLOCK_ALL)
cargo test --workspace --release --all-targets --locked
cargo test --workspace --all-targets --locked
# Creator edition tests
cargo test --release -p strivo-web -p strivo-bin --features creator --all-targets --locked
cargo test -p strivo-web -p strivo-bin --features creator --all-targets --locked
# licence backend typecheck and tests
(cd licence-backend && npm ci && npm run typecheck && npm test)
# browser mock lane (Playwright)
(cd crates/strivo-web/e2e && npm ci && npx playwright install chromium && npm test)
```

CI also runs `npm run test:real` in `crates/strivo-web/e2e`, which needs a PVR
release binary: `cargo build --release --locked`, then
`STRIVO_BIN=$PWD/target/release/strivo npm run test:real` from that directory.
The workflow additionally runs the packaging and docs lints under `scripts/`,
an MSRV check, a Windows cross-check and a performance lane; read `ci.yml` for
their exact commands.

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

## Docs discipline

- **ADRs are immutable.** Supersede, never rewrite: add a new ADR under
  `docs/adr/` and update the status table in `docs/adr/README.md`.
- **Audit manifests use stable IDs.** In `docs/audits/*/MANIFEST.{json,md}`,
  never renumber or delete an item; close it and append the evidence.

## Scope and design

The architectural shape of the project (platform trait, plugin event bus,
daemon / client split, browser-served SPA UI) is described in the README's
*Architecture* and *Design rationale* sections. Larger changes that move any
of those load-bearing pieces should be discussed in an issue first so we can
agree on the approach before the diff lands.

The plan of record is `.keel/plan.md` (keel), dated 2026-10-14: validate it with `keel validate strivo` and read the schedule with `keel dag strivo --milestone <n>`. The revoy ledger block in ROADMAP.md and the `.omo/` plan are history.
