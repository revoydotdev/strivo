import { test, expect } from "@playwright/test";

// Right-click context menu for a channel (rail row `.ch-row` and multiview
// live tile `.ms-leaf`) — 039-pvr.js / 040-creator.js / 021-pvr.css.
//
// The mock backend always serves the full (Creator-included) source, so
// the Auto-download section (gated on `typeof chCtxSetAutoDownload ===
// "function"`, only defined in 040-creator.js) is expected to render here.
// The complementary proof that a real PVR-only build never defines that
// symbol — so a genuine PVR bundle never shows the section — is
// `check-pvr-bundle.mjs` (wired as this package's `pretest`), which builds
// the real stripped artifact and inspects it directly; this spec cannot
// exercise that by itself, per the same limitation pvr-edition-gating.spec.ts
// documents for the runtime `creator_enabled` gate.

const LIVE_ROW = '.ch-row[data-channel-id="UClive0000000000000000aa"]';
const LIVE_CHANNEL_KEY = "YouTube:UClive0000000000000000aa";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
  // The mock server has no handler for the new alerts route; fulfill it
  // ourselves so every test gets a deterministic "no override yet" state
  // and PUTs can be asserted without depending on mock-server.mjs (owned
  // by a concurrent workstream).
  await page.route("**/api/v1/channels/*/alerts", async (route) => {
    if (route.request().method() === "GET") {
      return route.fulfill({
        json: { channel_key: LIVE_CHANNEL_KEY, on_live: null, on_upload: null },
      });
    }
    return route.fulfill({ json: { status: "ok" } });
  });
});

test("right-click a channel rail row opens the context menu", async ({ page }) => {
  await page.goto("/app#/library");
  const row = page.locator(LIVE_ROW);
  await expect(row).toBeVisible();

  await row.click({ button: "right" });

  const menu = page.locator(".ch-ctx-menu");
  await expect(menu).toBeVisible();
  await expect(menu).toHaveAttribute("data-channel-key", LIVE_CHANNEL_KEY);
  await expect(menu.getByText("Alert on live")).toBeVisible();
  await expect(menu.getByText("Alert on new upload")).toBeVisible();
  await expect(menu.getByText("Download livestreams")).toBeVisible();
  // Mock lane serves full (Creator-included) source — see file header.
  await expect(menu.getByText("Download uploads")).toBeVisible();
});

test("context menu dismisses on outside click and Escape", async ({ page }) => {
  await page.goto("/app#/library");
  const row = page.locator(LIVE_ROW);

  await row.click({ button: "right" });
  await expect(page.locator(".ch-ctx-menu")).toBeVisible();
  await page.mouse.click(5, 5);
  await expect(page.locator(".ch-ctx-menu")).toHaveCount(0);

  await row.click({ button: "right" });
  await expect(page.locator(".ch-ctx-menu")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".ch-ctx-menu")).toHaveCount(0);
});

test("toggling Alert on live fires PUT .../alerts with the expected body", async ({ page }) => {
  await page.goto("/app#/library");
  const row = page.locator(LIVE_ROW);
  await row.click({ button: "right" });

  const menu = page.locator(".ch-ctx-menu");
  await expect(menu).toBeVisible();

  const [request] = await Promise.all([
    page.waitForRequest(
      (req) =>
        req.url().includes(`/api/v1/channels/${encodeURIComponent(LIVE_CHANNEL_KEY)}/alerts`) &&
        req.method() === "PUT",
    ),
    menu.locator('[data-ctx-action="alert-live"]').click(),
  ]);
  const body = request.postDataJSON();
  // Starting state is "no override" (on_live: null → treated as allowed),
  // so the first toggle turns it off.
  expect(body).toMatchObject({ on_live: false });
});

test("right-click a populated multiview tile opens a stream-scoped menu", async ({ page }) => {
  // Test hooks (incl. setPlayerControllerFactory) only exist opted-in — see
  // 036-pvr.js — matching multiview-dnd.spec.ts's own setup.
  await page.addInitScript(() => localStorage.setItem("strivo:e2e", "1"));
  await page.addInitScript(() => {
    (window as any).__fakePlayerFactoryReady = false;
    const install = () => {
      const h = (window as any).__strivoTestHooks;
      if (!h || !h.setPlayerControllerFactory) return false;
      h.setPlayerControllerFactory((kind: string) => {
        const el = document.createElement("div");
        el.className = "watch-tile-iframe ms-iframe fake-player";
        el.dataset.kind = kind;
        return {
          kind,
          root: el,
          mount(c: HTMLElement) {
            if (el.parentElement !== c) c.appendChild(el);
          },
          destroy() {
            el.remove();
          },
          setMuted() {},
          setVolume() {},
          setQuality() {},
          repoint() {},
          isReady() {
            return true;
          },
        };
      });
      (window as any).__fakePlayerFactoryReady = true;
      return true;
    };
    if (!install()) {
      const t = setInterval(() => {
        if (install()) clearInterval(t);
      }, 10);
    }
  });

  await page.goto("/app#/watch");
  await page.waitForFunction(() => (window as any).__fakePlayerFactoryReady === true);

  const row = page.locator(LIVE_ROW);
  await expect(row).toBeVisible();
  const target = page.locator(".ms-leaf.ms-empty");
  await expect(target).toHaveCount(1);
  await row.dragTo(target);

  const tile = page.locator(`.ms-leaf[data-stream-id="${LIVE_CHANNEL_KEY}"]`);
  await expect(tile).toHaveCount(1);

  await tile.click({ button: "right" });

  const menu = page.locator(".ch-ctx-menu");
  await expect(menu).toBeVisible();
  await expect(menu).toHaveAttribute("data-channel-key", LIVE_CHANNEL_KEY);
});
