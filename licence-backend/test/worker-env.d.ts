/**
 * Augments `Cloudflare.Env` (the type behind `cloudflare:test`'s `env`
 * export) with the Worker's real bindings plus the migrations array
 * injected only for tests. See vitest.config.ts / test/apply-migrations.ts.
 */
import type { D1Migration } from "@cloudflare/vitest-pool-workers";
import type { Env as WorkerEnv } from "../src/env";

declare global {
  namespace Cloudflare {
    interface Env extends WorkerEnv {
      TEST_MIGRATIONS: D1Migration[];
    }
  }
}
