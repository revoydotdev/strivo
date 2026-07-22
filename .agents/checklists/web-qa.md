# Web-surface verification checklist

Read this when a claimed todo is tagged `web` or `a11y`. Adapted from gstack's `/qa`
skill methodology (MIT-licensed; gstack Copyright (c) 2026 Garry Tan) — the underlying
tool is the `playwright` MCP plugin injected via this todo's `Tools:` tag (ADR-0010's
tag-injection mechanism), not gstack's own `browse` binary, which aswarm doesn't
depend on.

## Exploration checklist (per page/view touched by this concern)

1. **Visual scan** — does the page render as expected, no obvious layout breakage?
2. **Interactive elements** — do buttons, links, and controls actually do what they say?
3. **Forms** — fill and submit with valid input, empty input, and one edge case.
4. **Console** — check for JS errors after every interaction, not just on load. An
   error that never surfaces visually is still a bug.
5. **Responsive** — check at least one mobile viewport if the concern touches layout.
6. **Accessibility** — run an axe-core pass (or equivalent) on any touched page; treat
   violations the same as any other finding this todo must address or explicitly defer.

## Rules

- **Verify before documenting.** Retry a suspected issue once before treating it as
  real — don't report a fluke as a bug, and don't treat a real bug as a fluke.
- **Test like a user, not like the developer who wrote it.** Realistic data, complete
  workflows, not just the happy path.
- **Repro is everything.** A finding without a way to reproduce it isn't a finding —
  either reproduce it or don't report it.
- **Depth over breadth.** A handful of well-evidenced issues beats a long list of
  vague ones.
- **Never modify code while exploring.** Establish what's actually broken first, then
  fix it as the todo's own artifact + verify command — same TDD discipline as every
  other concern (watch the check fail on the pre-fix state, then make it pass).
