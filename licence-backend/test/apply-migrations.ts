/**
 * Runs once per test file, before Miniflare snapshots isolated storage
 * for each individual test. See:
 * https://developers.cloudflare.com/workers/testing/vitest-integration/get-started/write-your-first-test/#d1
 */
import { applyD1Migrations, env } from "cloudflare:test";

await applyD1Migrations(env.LICENCE_DB, env.TEST_MIGRATIONS);
