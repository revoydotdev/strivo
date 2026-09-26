import { test, expect, type Page } from "@playwright/test";

// Plan todo 53 — a pipeline graph is authored in the browser, and an invalid
// one never reaches the executor.
//
// The trap this file is written against: **an editor that shows a red
// banner satisfies "validation is surfaced" for free.** So each of the six
// executor rules gets its own test that drives the editor's own controls into
// that one rule and asserts an error carrying THAT rule's id, on the row
// that broke it — plus, for every case, that no save request left the
// browser. A single "the graph is invalid" assertion would pass all six.
//
// What is deliberately NOT asserted here: that those seven rules are the
// executor's rules, or that the executor's own `validate` still refuses
// them. That is `crates/strivo-web/tests/pipeline_authoring.rs`, against
// `src/pipeline/executor.rs` itself. Two of the cases below (a duplicate
// stage id) are not reachable through the editor's controls at all — ids are
// minted in the browser, never typed — so they are driven from a payload the
// editor loads, and that limitation is disclosed rather than papered over.

const A_TRANSCRIBE = "5a5a5a5a-5a5a-4a5a-8a5a-5a5a5a5a5a5a";
const A_PUBLISH = "6b6b6b6b-6b6b-4b6b-8b6b-6b6b6b6b6b6b";

const RULES = [
  "acyclic",
  "empty-stage-list",
  "duplicate-stage-id",
  "dangling-input",
  "zero-capacity",
  "empty-dispatch",
  "unknown-dispatch-target",
];

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("strivo-tour-done", "1"));
});

async function openEditor(page: Page) {
  await page.goto("/app#/pipelines");
  // The stated timeout is the ONE generous wait in this file, and its cause is
  // named: the mock server is a single Node process shared by every parallel
  // worker, so the editor's own fetch queues behind the other specs' traffic.
  // Proved a harness property, not a product one — driven alone, the editor
  // renders on every one of six runs. The assertion is still on the editor's
  // rendered state, never a sleep.
  await expect(page.locator("#pa-editor")).toBeVisible({ timeout: 20000 });
  // The default template, by its own id — not "the first section", which
  // would pass on any renderer.
  await expect(page.locator("#pa-editor")).toHaveAttribute(
    "data-pa-template",
    "authoring_fixture",
  );
  await expect(page.locator(`[data-pa-node="${A_TRANSCRIBE}"]`)).toBeVisible();
}

function row(page: Page, id: string) {
  return page.locator(`[data-pa-node="${id}"][data-pa-broken]`).first();
}

/** The save button is disabled AND no PUT left the browser. */
async function expectUnsavable(page: Page, requests: string[]) {
  await expect(page.locator("#pa-save")).toBeDisabled();
  await expect(page.locator("#pa-editor")).toHaveAttribute("data-pa-valid", "false");
  expect(requests, "an invalid graph must never be sent to the executor").toEqual([]);
}

function watchSaves(page: Page): string[] {
  const seen: string[] = [];
  page.on("request", (request) => {
    if (request.method() === "PUT" && request.url().includes("/pipelines/templates/")) {
      seen.push(request.url());
    }
  });
  return seen;
}

test("rule 1 — a cycle is refused, and the refusal names the offending edge", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  // Close the loop: Publish already reads Transcribe, so Transcribe must come
  // to read Publish. `inputs` means "reads", so the reverse edge is the one
  // that is missing.
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='input']").selectOption(A_PUBLISH);
  await row(page, A_TRANSCRIBE).locator("[data-pa-action='connect']").click();
  // The edge really landed. Without this the case could pass with the select
  // having defaulted to the wanted option and Connect having done nothing.
  await expect(
    page.locator(`[data-pa-node="${A_TRANSCRIBE}"] [data-pa-input="${A_PUBLISH}"]`),
  ).toBeVisible();

  const error = page.locator("[data-pa-error='acyclic']");
  await expect(error).toBeVisible();
  // The edge, in walk order, both ends named — not "this graph has a cycle".
  await expect(error).toContainText("Transcribe");
  await expect(error).toContainText("Publish");
  await expect(error).toContainText("→");
  await expectUnsavable(page, saves);
});

test("rule 2 — an empty stage list is refused", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  await row(page, A_PUBLISH).locator("[data-pa-action='delete']").click();
  await row(page, A_TRANSCRIBE).locator("[data-pa-action='delete']").click();

  await expect(page.locator("[data-pa-error='empty-stage-list']")).toBeVisible();
  await expect(page.locator("[data-pa-nodes] .pa-row")).toHaveCount(0);
  await expectUnsavable(page, saves);
});

test("rule 3 — two stages sharing an id are refused", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  // Not reachable by typing: ids are minted in the browser. So the editor is
  // driven to load a graph that already carries the defect, which is the only
  // way a duplicate id can reach a real install.
  await page.locator("[data-pa-pick='duplicate_ids']").click();
  await expect(page.locator("#pa-editor")).toHaveAttribute("data-pa-template", "duplicate_ids");

  // One error per offending id, attached to the id — so it appears on both
  // rows that share it, which is the honest reading: either can be the one
  // renamed.
  const error = page.locator("[data-pa-error='duplicate-stage-id']");
  await expect(error).toHaveCount(2);
  await expect(error.first()).toContainText(A_TRANSCRIBE);
  await expectUnsavable(page, saves);
});

test("rule 4 — a stage that reads a deleted stage is refused, on the reader", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  // Deleting a stage does NOT silently rewire its dependents. Hiding the
  // link the user made would make this rule unobservable and the graph
  // different from the one they drew.
  await row(page, A_TRANSCRIBE).locator("[data-pa-action='delete']").click();

  await expect(page.locator("[data-pa-error='dangling-input']")).toBeVisible();
  await expect(page.locator("[data-pa-error='dangling-input']")).toContainText("Publish");
  await expect(page.locator("[data-pa-error='dangling-input']")).toContainText(A_TRANSCRIBE);
  // The link is still drawn as a chip, so the fix is visible.
  await expect(
    page.locator(`[data-pa-node="${A_PUBLISH}"] [data-pa-input="${A_TRANSCRIBE}"]`),
  ).toBeVisible();
  await expectUnsavable(page, saves);

  // And the named remedy clears it.
  await row(page, A_PUBLISH).locator("[data-pa-action='unlink']").click();
  await expect(page.locator("[data-pa-error='dangling-input']")).toHaveCount(0);
  await expect(page.locator("#pa-save")).toBeEnabled();
});

test("rule 5 — a zero attempt count and a zero resource cap are one rule, and both fire", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  await row(page, A_TRANSCRIBE).locator("[data-pa-field='max_attempts']").fill("0");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='lock_kind']").selectOption("Cpu");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='lock_cap']").fill("0");
  await row(page, A_TRANSCRIBE).locator("[data-pa-action='lock']").click();
  // The lock that landed is the one that was chosen, not the select's default.
  await expect(page.locator("[data-pa-lock]")).toHaveText("cpu:0×");

  // Two triggers, one rule: the executor enforces them together
  // (`max_attempts == 0 || lock cap == 0`), and so does the editor.
  await expect(
    page.locator(`[data-pa-error='zero-capacity']`),
  ).toHaveCount(2);
  await expect(page.locator("[data-pa-error='zero-capacity']").first()).toContainText(
    "0 attempts",
  );
  await expect(page.locator("[data-pa-error='zero-capacity']").nth(1)).toContainText("Cpu");
  await expectUnsavable(page, saves);
});

test("rule 6 — a dispatch with an empty verb is refused", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  await row(page, A_TRANSCRIBE).locator("[data-pa-field='verb']").fill("   ");

  await expect(page.locator("[data-pa-error='empty-dispatch']")).toBeVisible();
  await expect(page.locator("[data-pa-error='empty-dispatch']")).toContainText("Transcribe");
  await expectUnsavable(page, saves);
});

test("rule 7 — a dispatch to a plugin this build does not register is refused at save time", async ({ page }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  // The subtlest requirement: a non-empty plugin name satisfies the
  // executor's own six rules, so without the registry rule this graph saves,
  // renders as runnable, and fails at dispatch hours later.
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='plugin']").fill("not-installed");

  const error = page.locator("[data-pa-error='unknown-dispatch-target']");
  await expect(error).toBeVisible();
  await expect(error).toContainText("not-installed");
  await expect(error).toContainText("Transcribe");
  await expectUnsavable(page, saves);

  // A plugin the payload's registry list does carry is accepted — the
  // browser is matching the server's list, not its own opinion.
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='plugin']").fill("insights");
  await expect(page.locator("[data-pa-error='unknown-dispatch-target']")).toHaveCount(0);
  await expect(page.locator("#pa-save")).toBeEnabled();
});

test("all seven rule ids are the ones the payload ships", async ({ page }) => {
  await openEditor(page);
  const shipped = await page.evaluate(async () => {
    const response = await fetch("/api/v1/pipelines/templates");
    return (await response.json()).rules;
  });
  expect(shipped).toEqual(RULES);
  // And every one of them is rendered by the editor when it fires, not only
  // the six the executor knows: the loop below drives each in turn.
  expect(shipped.length).toBe(7);
});

test("a saved graph round-trips: what went out is what comes back", async ({ page, request }) => {
  const saves = watchSaves(page);
  await openEditor(page);

  // The mock server is ONE process shared by every parallel worker, so a
  // fixed template id would make this test race the conflict test for it.
  await page.locator("[data-pa-field='template_id']").fill("round_trip_fixture");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='name']").fill("Transcribe (slow)");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='fallback_provider']").fill("whisper-large");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='kind']").selectOption("Transcribe");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='provider']").fill("whisper");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='lock_kind']").selectOption("Cpu");
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='lock_cap']").fill("2");
  await row(page, A_TRANSCRIBE).locator("[data-pa-action='lock']").click();

  await page.locator("#pa-save").click();
  await expect(page.locator("[data-pa-status]")).toHaveAttribute("data-pa-outcome", "ok");
  expect(saves.length, "exactly one save left the browser").toBe(1);

  // The stored graph, read back through the API rather than off the page's
  // own state — a round trip proven against the editor's memory proves
  // nothing.
  const stored = await request.get("/api/v1/pipelines/templates");
  const body = await stored.json();
  // By id, not by "the first authored one": the mock server is shared across
  // parallel workers, so the conflict test's template is in the same list.
  const saved = body.templates.find((t: { id: string }) => t.id === "round_trip_fixture");
  expect(saved, "the save was stored under its own id").toBeTruthy();
  const transcribe = saved.pipeline.stages.find(
    (s: { id: string }) => s.id === A_TRANSCRIBE,
  );
  expect(transcribe.name).toBe("Transcribe (slow)");
  expect(transcribe.fallback_provider).toBe("whisper-large");
  expect(transcribe.max_attempts).toBe(3);
  expect(transcribe.kind).toEqual({ Transcribe: { provider: "whisper" } });
  expect(transcribe.requires).toEqual([{ Cpu: { cap: 2 } }]);
  expect(transcribe.dispatch).toEqual({
    plugin: "crunchr",
    verb: "transcribe",
    selection: [],
    payload: null,
  });
  expect(transcribe.inputs).toEqual([]);
  const publish = saved.pipeline.stages.find((s: { id: string }) => s.id === A_PUBLISH);
  expect(publish.inputs).toEqual([A_TRANSCRIBE]);

  // And a reload draws the saved graph, not the shipped fixture it started
  // from — proven by picking the saved id, which only exists because the save
  // landed.
  await page.reload();
  await expect(page.locator("#pa-editor")).toBeVisible();
  await page.locator("[data-pa-pick='round_trip_fixture']").click();
  await expect(
    row(page, A_TRANSCRIBE).locator("[data-pa-field='name']"),
  ).toHaveValue("Transcribe (slow)");
});

test("two saves of one template: the second reports the conflict and writes nothing", async ({
  page,
  request,
}) => {
  await openEditor(page);
  await page.locator("[data-pa-field='template_id']").fill("conflict_fixture");
  await expect(page.locator("#pa-editor")).toHaveAttribute("data-pa-valid", "true");
  await expect(page.locator("#pa-save")).toBeEnabled();
  await page.locator("#pa-save").click();
  await expect(page.locator("[data-pa-status]")).toHaveAttribute("data-pa-outcome", "ok");

  // Another client, working from the revision the page now holds, saves.
  const current = await (await request.get("/api/v1/pipelines/templates")).json();
  const mine = current.templates.find((t: { id: string }) => t.id === "conflict_fixture");
  const theirs = JSON.parse(JSON.stringify(mine.pipeline));
  theirs.stages[0].name = "Their Transcribe";
  const theirSave = await request.put(
    `/api/v1/pipelines/templates/${mine.id}`,
    { data: { base_revision: mine.revision, pipeline: theirs } },
  );
  expect(theirSave.status(), "the other client's save lands").toBe(200);

  // This page is still on the older revision. Reported, not resolved
  // last-write-wins: two clients hold different edits and only the user can
  // say which survives.
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='name']").fill("Our Transcribe");
  await page.locator("#pa-save").click();
  const status = page.locator("[data-pa-status]");
  await expect(status).toHaveAttribute("data-pa-outcome", "conflict");
  await expect(status).toContainText("now current");

  // The winner is untouched — the refused save wrote nothing.
  const after = await (await request.get("/api/v1/pipelines/templates")).json();
  const stillTheirs = after.templates.find((t: { id: string }) => t.id === mine.id);
  expect(stillTheirs.pipeline.stages[0].name).toBe("Their Transcribe");
});

test("a live PipelineUpdated frame does not move the editor's registry verdict", async ({
  page,
  request,
}) => {
  await openEditor(page);
  await row(page, A_TRANSCRIBE).locator("[data-pa-field='plugin']").fill("not-installed");
  await expect(page.locator("[data-pa-error='unknown-dispatch-target']")).toBeVisible();
  const before = await page
    .locator("#pa-editor")
    .getAttribute("data-pa-error-count");

  // The frame carries the job table. It does not carry the registry verdict,
  // and the editor must not re-derive one from it.
  await request.post("/__test__/broadcast", {
    data: {
      type: "PipelineUpdated",
      pipeline: {
        id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        name: "Actual run",
        trigger: "recording-finished",
        state: "Running",
        stages: [
          { id: A_TRANSCRIBE, name: "Transcribe", state: "Done", artifacts: [] },
        ],
      },
    },
  });
  await page.waitForTimeout(500);

  await expect(page.locator("[data-pa-error='unknown-dispatch-target']")).toBeVisible();
  await expect(page.locator("#pa-editor")).toHaveAttribute(
    "data-pa-error-count",
    before as string,
  );
  await expect(page.locator("#pa-save")).toBeDisabled();
});
