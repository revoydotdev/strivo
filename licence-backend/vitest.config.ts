import path from "node:path";
import { cloudflareTest, readD1Migrations } from "@cloudflare/vitest-pool-workers";
import { defineConfig } from "vitest/config";
import {
  TEST_PRIVATE_KEY_PKCS8,
  TEST_WEBHOOK_SECRET,
} from "./test/fixtures/test-keys";

export default defineConfig(async () => {
  const migrationsPath = path.join(import.meta.dirname, "migrations");
  const migrations = await readD1Migrations(migrationsPath);

  return {
    plugins: [
      cloudflareTest({
        wrangler: { configPath: "./wrangler.toml" },
        miniflare: {
          // Override the `[vars]`/secrets from wrangler.toml with
          // deterministic test values so assertions don't depend on
          // whatever a developer's local wrangler.toml happens to say.
          bindings: {
            TEST_MIGRATIONS: migrations,
            PUBLIC_BASE_URL: "https://licence.test.invalid",
            TRIAL_DURATION_HOURS: "72",
            REFRESH_INTERVAL_HOURS: "72",
            LEMONSQUEEZY_STORE_ID: "123",
            LEMONSQUEEZY_PRODUCT_ID: "456",
            JWT_PRIVATE_KEY: TEST_PRIVATE_KEY_PKCS8,
            LEMONSQUEEZY_WEBHOOK_SECRET: TEST_WEBHOOK_SECRET,
            LEMONSQUEEZY_API_KEY: "unused-in-current-code",
            RESEND_API_KEY: "test-resend-key",
            RESEND_FROM: "StriVo <licence@licence.test.invalid>",
          },
        },
      }),
    ],
    test: {
      setupFiles: ["./test/apply-migrations.ts"],
    },
  };
});
