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
  // Livestreams + Uploads sit adjacent under one "Automatic downloads"
  // label, both enabled (functional) toggles in every edition.
  const livestreams = menu.locator('[data-ctx-action="auto-record"]');
  const uploads = menu.locator('[data-ctx-action="auto-download-uploads"]');
  await expect(livestreams).toBeVisible();
  await expect(livestreams).toContainText("Livestreams");
  await expect(livestreams).toBeEnabled();
  await expect(uploads).toBeVisible();
  await expect(uploads).toContainText("Uploads");
  await expect(uploads).toBeEnabled();
  // Livestreams immediately precedes Uploads in the DOM.
  const autoDlSection = menu.locator(".ch-ctx-menu-section", { hasText: "Automatic downloads" });
  const items = autoDlSection.locator("[data-ctx-action]");
  await expect(items).toHaveCount(2);
  await expect(items.nth(0)).toHaveAttribute("data-ctx-action", "auto-record");
  await expect(items.nth(1)).toHaveAttribute("data-ctx-action", "auto-download-uploads");
  // Mock lane serves full (Creator-included) source — see file header —
  // so the honestly-relabeled archiver-tandem row also renders, in its
  // own section below Automatic downloads.
  await expect(menu.getByText("Archive back catalog after capture")).toBeVisible();
});

test("toggling Uploads fires PUT .../auto_download_uploads and flips the check", async ({ page }) => {
  await page.goto("/app#/library");
  const row = page.locator(LIVE_ROW);
  await row.click({ button: "right" });

  const menu = page.locator(".ch-ctx-menu");
  await expect(menu).toBeVisible();
  const uploads = menu.locator('[data-ctx-action="auto-download-uploads"]');
  await expect(uploads).toHaveAttribute("aria-checked", "false");

  const [request] = await Promise.all([
    page.waitForRequest(
      (req) =>
        req.url().includes(`/api/v1/channels/${encodeURIComponent(LIVE_CHANNEL_KEY)}/auto_download_uploads`) &&
        req.method() === "PUT",
    ),
    uploads.click(),
  ]);
  expect(request.postDataJSON()).toMatchObject({ enabled: true });
  await expect(uploads).toHaveAttribute("aria-checked", "true");
});

test("a failed Uploads PUT rolls the checkbox back and shows an error toast", async ({ page }) => {
  await page.route("**/api/v1/channels/*/auto_download_uploads", (route) =>
    route.fulfill({ status: 500, json: { title: "boom" } }),
  );

  await page.goto("/app#/library");
  const row = page.locator(LIVE_ROW);
  await row.click({ button: "right" });

  const menu = page.locator(".ch-ctx-menu");
  await expect(menu).toBeVisible();
  const uploads = menu.locator('[data-ctx-action="auto-download-uploads"]');
  await expect(uploads).toHaveAttribute("aria-checked", "false");

  await uploads.click();

  await expect(uploads).toHaveAttribute("aria-checked", "false");
  await expect(page.locator(".toast.error").last()).toBeVisible();
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
