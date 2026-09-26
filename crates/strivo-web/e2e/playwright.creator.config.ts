import { defineConfig, devices } from "@playwright/test";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// The Creator-edition sibling of playwright.real.config.ts (plan todo 16).
//
// It drives a **Creator-featured** binary — `strivo serve` built with
// `--features creator` — so the Pro journey (licence → transcribe →
// pipelines → editor → publish) is exercised against the build that actually
// contains the Creator routes. Against a PVR binary every one of those
// endpoints is 404, so a PVR lane can say nothing about the Pro path at all.
//
// WHY A SEPARATE CONFIG rather than parameterising playwright.real.config.ts:
// that file hardcodes `PORT = 8281` and resolves its binary from
// `target/{release,debug}/strivo`, which does not exist in this checkout
// because `.cargo/config.toml` redirects `target-dir` to
// `/scratch/cargo-targets/strivo-main`. Editing it would change the PVR lane
// that already landed (todo 15), so both knobs here are read from the
// environment instead and both have explicit, working defaults.
//
// `webServer` is a deliberate copy of the PVR config's shape, including its
// reason for probing `/app` rather than `/api/v1/health`:
//
//   Not /api/v1/health: it is a readiness probe of its own (daemon IPC + jobs
//   DB + disk) and can legitimately 503 for reasons unrelated to "is the HTTP
//   server up", which is all this webServer check needs. /app is the SPA
//   shell — 200 the instant the listener is bound.
//
// `testMatch` is what keeps the two lanes from colliding. Both configs point
// `testDir` at ./tests-real, and Playwright's default testMatch is recursive,
// so the creator-journey-*.spec.ts files added here are ALSO collected by
// `npm run test:real`. They are not left to fail there: every spec's
// beforeEach reads the served descriptor and skips when the edition is not
// `creator`, so the PVR lane reports them as skipped against a PVR binary
// (a true statement about that binary) and the Creator lane can never skip,
// because the same read is asserted to be `creator` in the first test of
// each spec. That guard is asserted, not assumed — see
// docs/audits/2026-09-25-pro-journey/REPORT.md.
const __dirname = path.dirname(fileURLToPath(import.meta.url));

// Granted to this lane, and overridable so two Creator lanes can coexist.
const PORT = Number(process.env.CREATOR_E2E_PORT ?? 8281);

const repoRoot = path.resolve(__dirname, "../../..");

// Order matters: a Creator-featured build is the whole point of this lane, so
// an explicitly supplied binary wins, then the conventional out-of-tree
// release location, and only then a `cargo install --root` style prefix.
const explicit = process.env.STRIVO_BIN?.trim();
const candidate = explicit
  ? [explicit]
  : [
      // CARGO_TARGET_DIR a Creator build was made into.
      process.env.CREATOR_STRIVO_BIN?.trim(),
      "/tmp/opencode/creator-target/release/strivo",
      path.join(repoRoot, "target/release/strivo"),
      path.join(repoRoot, "target/debug/strivo"),
    ].filter((p): p is string => !!p);

const bin = candidate.find((p) => existsSync(p));

export default defineConfig({
  testDir: "./tests-real",
  testMatch: "**/creator-journey-*.spec.ts",
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? "github" : "list",
  // A boot that waits for a daemon + web listener, seeds a recording with
  // ffmpeg, and answers 90 JIT-compiled Rust routes is not a 30 s job on a
  // loaded 24-CPU box. Bounded, not unbounded.
  timeout: 45_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "on-first-retry",
  },
  projects: [{ name: "creator-chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: "bash ./real-server.sh",
    url: `http://127.0.0.1:${PORT}/app`,
    reuseExistingServer: false,
    timeout: 120_000,
    env: {
      PORT: String(PORT),
      STRIVO_BIN: bin ?? "",
      // A Creator daemon initialises a six-plugin host before the IPC
      // listener binds. Uncontended that is 0.6 s; on this shared 24-CPU box
      // at load average ~20 it measured 12-55 s, so the shared harness's 10 s
      // default flakes. Widen-only, so the PVR lane is unaffected, and free
      // when the boot is fast.
      STRIVO_E2E_DAEMON_PROBES: process.env.STRIVO_E2E_DAEMON_PROBES ?? "900",
    },
  },
});
