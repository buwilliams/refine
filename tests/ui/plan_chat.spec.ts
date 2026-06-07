import { expect, test, type Page } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

async function selectReporter(page: Page) {
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
}

async function closePlanTabIfPresent(page: Page) {
  if (await page.getByTestId("plan-drafts-modal").count()) {
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0);
  }
  const planTab = page.getByTestId("toolbar-tab-plan");
  if (await planTab.count()) {
    await planTab.locator("[data-close-tab]").click();
    await expect(page.getByTestId("toolbar-tab-plan")).toHaveCount(0);
  }
}

test("runs a Plan chat turn and drafts a Feature through Smoke AI", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const featureName = `Smoke AI Plan Existing Feature ${suffix}`;
  const editedActual = `smoke-ai plan edited actual ${suffix}`;
  const editedTarget = `smoke-ai plan edited target ${suffix}`;
  const duplicateActual = "smoke-ai plan actual behavior one";
  const duplicateTarget = "smoke-ai plan target behavior one";
  const createdGapIds = new Set<string>();
  let featureId = "";

  const duplicatePayload = await jsonObject(await request.post("/api/gaps", {
    data: {
      name: `${duplicateTarget} ${duplicateActual} ${duplicateTarget}`,
      reporter: "refine-smoke",
      actual: duplicateActual,
      target: duplicateTarget,
      priority: "low",
    },
  }));
  const duplicateGapId = String((duplicatePayload.gap as { id?: string } | undefined)?.id ?? "");
  expect(duplicateGapId).toBeTruthy();
  createdGapIds.add(duplicateGapId);

  const featurePayload = await jsonObject(await request.post("/api/features", {
    data: {
      name: featureName,
      description: "Existing Feature destination for Plan draft coverage",
      reporter: "refine-smoke",
    },
  }));
  featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
  expect(featureId).toBeTruthy();

  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");
  await closePlanTabIfPresent(page);
  await selectReporter(page);
  await page.evaluate(() => {
    return (window as unknown as { RefineCommands: { run: (id: string) => Promise<unknown> } }).RefineCommands.run("plan.open");
  });

  try {
    const planTab = page.getByTestId("toolbar-tab-plan");
    await expect(planTab).toBeVisible();
    if (!(await planTab.evaluate((el) => el.classList.contains("active")))) {
      await planTab.click();
    }
    await expect(planTab).toHaveClass(/active/);
    await expect(page.getByTestId("plan-draft")).toBeVisible();
    await expect(page.getByTestId("chat-input")).toBeVisible();
    await expect(page.getByTestId("chat-status")).toContainText("active", { timeout: 15_000 });
    await expect(page.getByTestId("chat-toggle")).toHaveText("Stop plan");

    await page.getByTestId("chat-toggle").click();
    await expect(page.getByTestId("chat-status")).toContainText("No active session");
    await expect(page.getByTestId("chat-toggle")).toHaveText("Start plan");
    await expect(page.getByTestId("plan-draft")).toBeDisabled();

    await page.getByTestId("chat-toggle").click();
    await expect(page.getByTestId("chat-status")).toContainText("active", { timeout: 15_000 });
    await expect(page.getByTestId("chat-toggle")).toHaveText("Stop plan");

    await page.getByTestId("chat-input").fill("Draft Feature smoke plan request for deterministic planning workflow.");
    const planInputQueued = page.waitForResponse((response) =>
      /\/api\/chat\/[^/]+\/input$/.test(new URL(response.url()).pathname) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("chat-send").click();
    await planInputQueued;

    await expect(page.getByTestId("chat-output")).toContainText("smoke-ai plan actual behavior one", { timeout: 45_000 });
    await expect(page.getByTestId("plan-draft")).toBeEnabled();

    await page.getByTestId("plan-draft").click();
    await expect(page.getByTestId("plan-drafts-modal")).toBeVisible();
    await expect(page.getByTestId("import-feature-mode-new")).toBeChecked();
    await expect(page.getByTestId("import-draft-actual").first()).toHaveValue(/smoke-ai plan actual behavior one/);
    await expect(page.getByTestId("import-draft-target").first()).toHaveValue(/smoke-ai plan target behavior one/);
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Needs duplicate resolution");

    await page.getByTestId("import-feature-mode-existing").check();
    await expect(page.getByTestId("import-feature-existing").locator("option", { hasText: featureName })).toHaveCount(1);
    await page.getByTestId("import-feature-existing").selectOption(featureId);
    await expect(page.getByTestId("import-feature-summary")).toContainText(featureId);

    await page.getByTestId("import-draft-actual").nth(1).fill(editedActual);
    await page.getByTestId("import-draft-target").nth(1).fill(editedTarget);
    await page.getByTestId("import-select-duplicates").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("1 selected");
    await page.getByTestId("import-dismiss-duplicates").click();
    await expect(page.getByTestId("import-duplicate-decision")).toHaveCount(0);
    await expect(page.getByTestId("import-persist")).toHaveText("Save (1) gap to Feature");
    await expect(page.getByTestId("import-persist")).toBeEnabled();

    await page.getByTestId("import-persist").click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0, { timeout: 30_000 });

    let createdId = "";
    await expect.poll(async () => {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=100&node=current&q=${encodeURIComponent(editedActual)}`));
      createdId = String((((gaps.gaps as Array<{ id?: string }> | undefined) ?? [])[0]?.id) ?? "");
      return createdId;
    }).not.toBe("");
    expect(createdId).toBeTruthy();
    const createdDetail = await jsonObject(await request.get(`/api/gaps/${createdId}`));
    const createdGap = createdDetail.gap as {
      feature_id?: string;
      rounds?: Array<{ actual?: string; target?: string }>;
    };
    expect(createdGap.feature_id).toBe(featureId);
    expect(createdGap.rounds?.some((round) => round.actual === editedActual && round.target === editedTarget)).toBeTruthy();
    createdGapIds.add(createdId);
  } finally {
    await closePlanTabIfPresent(page);
    if (featureId) {
      await request.delete(`/api/features/${featureId}`);
    }
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});
