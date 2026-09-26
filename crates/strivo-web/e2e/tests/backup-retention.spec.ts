import { test, expect, type Page } from "@playwright/test";

// Plan todo 40 — a restore must not report success for a project whose sources
// did not come back, and a cleanup must not drop an original.
//
// The trap this file is written against: **"no green toast appeared" passes for
// any page whose toasts are broken.** So the first test is a control — a restore
// the daemon reports complete MUST produce the green toast — and only after
// that is established do the two refusal cases assert its absence. Without the
// control, every assertion below would be satisfied by a page that never shows
// a toast at all.
//
// What is deliberately NOT asserted here: that the daemon really reconciles a
// restore. That is `crates/research/src/creator/backup.rs`'s
// `a_restore_missing_one_source_is_not_reported_as_a_success`, driven against a
// real store. This lane proves the *UI* refuses the wording, which is a
// different claim: a correct model behind a green toast is still a false success.

const BACKUP_INCOMPLETE = "2026-05-26T00-00-00Z";
const BACKUP_NO_RECORD = "2026-05-20T00-00-00Z";
const MISSING_SOURCE_TITLE = "b.mkv";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

/** Open the System page and wait for the backup list it paints itself. */
async function openSystem(page: Page) {
  await page.goto("/app#/system");
  await expect(page.getByRole("heading", { name: "Backup" })).toBeVisible();
  await expect(page.locator(".restore-backup").first()).toBeVisible();
}

/** Click Restore on `name` and accept the confirm dialog the app renders. */
async function restore(page: Page, name: string) {
  await page.locator(`.restore-backup[data-name="${name}"]`).click();
  await page.locator(".confirm-ok").click();
}

test("a complete restore DOES say so — the control the refusals depend on", async ({ page }) => {
  await openSystem(page);

  await restore(page, "2026-05-27T00-00-00Z");

  // Reachable only from the `report.complete === true` branch, so its presence
  // proves the green path is live and the refusal tests below are meaningful.
  await expect(page.locator(".toast.success")).toContainText("came back whole", {
    timeout: 5_000,
  });
  await expect(page.locator("#restore-report")).toHaveCount(0);
});

test("a restore missing one source names it and is not reported as a success", async ({ page }) => {
  await openSystem(page);

  await restore(page, BACKUP_INCOMPLETE);

  // The claim. `complete` was false, so the success wording must be absent…
  await expect(page.locator(".toast.success")).toHaveCount(0);
  // …the refusal must name how many did not come back…
  await expect(page.locator(".toast.error")).toContainText(
    "1 source did not come back — not a complete restore",
  );
  // …and the missing source must be named in a panel, not only counted.
  const report = page.locator("#restore-report");
  await expect(report).toHaveAttribute("role", "alert");
  await expect(report).toContainText("1 source did not come back");
  const missing = report.locator('[data-restore-missing="1"]');
  await expect(missing).toHaveCount(1);
  await expect(missing).toContainText(MISSING_SOURCE_TITLE);
  // Named by the title a person recognises, and with the reason it is not whole.
  await expect(missing).toContainText("launch stream");
  await expect(missing).toContainText("row missing from the restored library");
  // The source that DID come back is not dragged into the failure list.
  await expect(missing).not.toContainText("a.mkv");
});

test("a backup with no project record is not called a success either", async ({ page }) => {
  await openSystem(page);

  await restore(page, BACKUP_NO_RECORD);

  // "Nothing was checked" is its own answer, not "nothing is wrong": the old
  // wording here was a green `Restored config.toml, jobs.db`.
  await expect(page.locator(".toast.success")).toHaveCount(0);
  await expect(page.locator(".toast.error")).toContainText(
    "no project record, so nothing about projects was verified",
  );
  await expect(page.locator("#restore-report")).toHaveCount(0);
});

test("cleanup reports an unreferenced output and protects what is referenced", async ({ page }) => {
  await openSystem(page);

  // The panel only appears from a restore report, so drive one first.
  await restore(page, BACKUP_INCOMPLETE);
  const panel = page.locator("#retention-panel");
  await expect(panel).toContainText("Cleanup for launch stream");

  // Offered by name and class, not as a count of rows.
  const plan = panel.locator("[data-retention-action]");
  await expect(plan).toHaveCount(2);
  await expect(plan.nth(0)).toHaveAttribute("data-retention-action", "unreferenced");
  await expect(plan.nth(0)).toContainText("proxy");
  await expect(plan.nth(0)).toContainText("referenced by nothing");
  await expect(plan.nth(1)).toHaveAttribute("data-retention-action", "protected");
  await expect(plan.nth(1)).toContainText("referenced by release package");

  // The collect control states the rule it is about to apply, so the guarantee
  // is visible before the button is pressed rather than after.
  const collect = panel.locator("#retention-collect");
  await expect(collect).toBeVisible();
  await collect.click();
  await expect(page.locator(".confirm-msg")).toContainText(
    "Originals and anything referenced are kept",
  );
  await page.locator(".confirm-ok").click();
  await expect(page.locator(".toast.success")).toContainText("Removed 1 unreferenced output");
});
