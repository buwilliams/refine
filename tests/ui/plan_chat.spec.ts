import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
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

function waitForPlanExtractionQueued(page: Page) {
  return page.waitForResponse((response) =>
    response.url().includes("/api/import/extract") &&
    response.request().method() === "POST" &&
    response.status() === 202
  );
}

async function expectPlanExtractionProcess(request: APIRequestContext, jobId: string) {
  const processPayload = await jsonObject(await request.get("/api/processes"));
  const runnerWork = processPayload.runner_work as Array<{
    kind?: string;
    job_id?: string;
    status?: string;
  }> | undefined;
  const planExtractor = (runnerWork ?? []).find((work) => work.kind === "plan_draft_extractor");
  expect(planExtractor?.job_id).toBe(jobId);
  expect(["running", "complete"].includes(String(planExtractor?.status ?? ""))).toBe(true);
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

    const extractionQueued = waitForPlanExtractionQueued(page);
    await page.getByTestId("plan-draft").click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0);
    const extractionPayload = await (await extractionQueued).json();
    const extractionJobId = String(extractionPayload.job?.id ?? "");
    expect(extractionJobId).toBeTruthy();
    await expectPlanExtractionProcess(request, extractionJobId);
    await expect(page.getByTestId("plan-drafts-modal")).toBeVisible();
    await expect(page.getByTestId("import-feature-mode-new")).toBeChecked();
    await expect(page.getByTestId("import-feature-new-name")).toHaveValue("Smoke AI Plan Feature");
    await expect(page.getByTestId("import-feature-new-description")).toHaveValue("A deterministic product capability planned by the Smoke AI fixture.");
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
    await page.getByTestId("import-import-selected").click();
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Will import as original");
    await expect(page.getByTestId("import-persist")).toHaveText("Save (2) gaps to Feature");
    await expect(page.getByTestId("import-persist")).toBeEnabled();

    let completedImportResult: {
      count?: number;
      gaps?: Array<{ id?: string }>;
    } | null = null;
    const importCompleted = page.waitForResponse(async (response) => {
      if (!/\/api\/jobs\/[^/]+$/.test(new URL(response.url()).pathname)) return false;
      if (response.request().method() !== "GET" || response.status() !== 200) return false;
      const payload = await response.json();
      if (payload.job?.status === "complete") {
        completedImportResult = payload.job.result || null;
        return true;
      }
      return false;
    });
    await page.getByTestId("import-persist").click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0, { timeout: 30_000 });
    await importCompleted;
    expect(completedImportResult?.count).toBe(2);

    let createdId = "";
    for (const gap of completedImportResult?.gaps ?? []) {
      const id = String(gap.id ?? "");
      if (!id) continue;
      const detail = await jsonObject(await request.get(`/api/gaps/${id}`));
      const rounds = (detail.gap as { rounds?: Array<{ actual?: string; target?: string }> } | undefined)?.rounds ?? [];
      if (rounds.some((round) => round.actual === editedActual && round.target === editedTarget)) {
        createdId = id;
        break;
      }
    }
    expect(createdId).toBeTruthy();
    const createdDetail = await jsonObject(await request.get(`/api/gaps/${createdId}`));
    const createdGap = createdDetail.gap as {
      feature_id?: string;
      rounds?: Array<{ actual?: string; target?: string }>;
    };
    expect(createdGap.feature_id).toBe(featureId);
    expect(createdGap.rounds?.some((round) => round.actual === editedActual && round.target === editedTarget)).toBeTruthy();
    createdGapIds.add(createdId);

    let importedDuplicateId = "";
    for (const gap of completedImportResult?.gaps ?? []) {
      const id = String(gap.id ?? "");
      if (!id || id === duplicateGapId) continue;
      const detail = await jsonObject(await request.get(`/api/gaps/${id}`));
      const rounds = (detail.gap as { rounds?: Array<{ actual?: string; target?: string }> } | undefined)?.rounds ?? [];
      if (rounds.some((round) => round.actual === duplicateActual && round.target === duplicateTarget)) {
        importedDuplicateId = id;
        break;
      }
    }
    expect(importedDuplicateId).toBeTruthy();
    createdGapIds.add(importedDuplicateId);
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

test("shows agent working state after sending Plan input", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const sessionId = "ui-plan-working";

  await page.route("**/api/chat/start", async (route) => {
    await route.fulfill({
      status: 201,
      contentType: "application/json",
      body: JSON.stringify({
        session_id: sessionId,
        mode: "plan",
        provider: "mock",
      }),
    });
  });
  await page.route(`**/api/chat/${sessionId}/input`, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ok: true,
        in_flight: true,
        queued_messages: [],
      }),
    });
  });

  await page.addInitScript(() => {
    localStorage.removeItem("refine_chat_tabs");
  });
  await page.goto("/");
  await page.evaluate(() => {
    return (window as unknown as { RefineCommands: { run: (id: string) => Promise<unknown> } }).RefineCommands.run("plan.open");
  });

  await expect(page.getByTestId("toolbar-tab-plan")).toHaveClass(/active/);
  await expect(page.getByTestId("chat-status")).toContainText("active");
  await page.getByTestId("chat-input").fill("Long Plan Mode prompt that should visibly start agent work.");
  const inputAccepted = page.waitForResponse((response) =>
    response.url().includes(`/api/chat/${sessionId}/input`) &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("chat-send").click();
  await inputAccepted;

  await expect(page.getByTestId("chat-status")).toContainText("Agent working");
  await expect(page.getByTestId("chat-activity-label")).toHaveText("Agent working");
  await expect(page.locator("#chat-input-pending-dots")).toBeVisible();

  await page.evaluate((id) => {
    (window as any).handleChatSseEvent?.({
      session_id: id,
      mode: "plan",
      provider: "mock",
      in_flight: false,
      closed: false,
      event: {
        id: "event-user-replay",
        role: "user",
        text: "Long Plan Mode prompt that should visibly start agent work.",
        progress: false,
        created_at: new Date().toISOString(),
      },
    });
  }, sessionId);
  await expect(page.getByTestId("chat-status")).toContainText("Agent working");

  await page.evaluate((id) => {
    (window as any).handleChatSseEvent?.({
      session_id: id,
      mode: "plan",
      provider: "mock",
      in_flight: false,
      closed: false,
      event: {
        id: "event-assistant-complete",
        role: "assistant",
        text: "Plan response complete.",
        progress: false,
        created_at: new Date().toISOString(),
      },
    });
  }, sessionId);
  await expect(page.getByTestId("chat-status")).toContainText("active");
  await expect(page.locator("#chat-input-pending-dots")).toBeHidden();
});

test("updates an original Gap from a Plan draft duplicate decision", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const duplicateActual = "smoke-ai plan actual behavior one";
  const duplicateTarget = "smoke-ai plan target behavior one";
  const createdGapIds = new Set<string>();

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
    await expect(page.getByTestId("chat-status")).toContainText("active", { timeout: 15_000 });
    await page.getByTestId("chat-input").fill("Draft Feature smoke plan request for updating an original duplicate.");
    const planInputQueued = page.waitForResponse((response) =>
      /\/api\/chat\/[^/]+\/input$/.test(new URL(response.url()).pathname) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("chat-send").click();
    await planInputQueued;
    await expect(page.getByTestId("chat-output")).toContainText("smoke-ai plan actual behavior one", { timeout: 45_000 });
    await expect(page.getByTestId("plan-draft")).toBeEnabled();

    const extractionQueued = waitForPlanExtractionQueued(page);
    await page.getByTestId("plan-draft").click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0);
    const extractionPayload = await (await extractionQueued).json();
    expect(String(extractionPayload.job?.id ?? "")).toBeTruthy();
    await expect(page.getByTestId("plan-drafts-modal")).toBeVisible();
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Needs duplicate resolution");
    await page.getByTestId("import-draft-priority").first().selectOption("high");
    await page.getByTestId("import-select-duplicates").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("1 selected");
    await page.getByTestId("import-update-field").selectOption("priority");
    await page.getByTestId("import-update-originals").click();
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Will update original priority");
    await expect(page.getByTestId("import-persist")).toHaveText("Save (1) gap to new Feature");

    let completedImportResult: {
      duplicate_actions?: { updated_original?: number };
      gaps?: Array<{ id?: string }>;
    } | null = null;
    const importCompleted = page.waitForResponse(async (response) => {
      if (!/\/api\/jobs\/[^/]+$/.test(new URL(response.url()).pathname)) return false;
      if (response.request().method() !== "GET" || response.status() !== 200) return false;
      const payload = await response.json();
      const job = payload.job || {};
      if (job.status !== "complete") return false;
      completedImportResult = job.result || {};
      return true;
    });
    await page.getByTestId("import-persist").click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0, { timeout: 30_000 });
    await importCompleted;

    await expect.poll(async () => {
      const detail = await jsonObject(await request.get(`/api/gaps/${duplicateGapId}`));
      return String((detail.gap as { priority?: string } | undefined)?.priority ?? "");
    }).toBe("high");

    expect(completedImportResult?.duplicate_actions?.updated_original).toBe(1);
    const createdId = String((completedImportResult?.gaps ?? [])[0]?.id ?? "");
    expect(createdId).toBeTruthy();
    const createdDetail = await jsonObject(await request.get(`/api/gaps/${createdId}`));
    const createdRounds = (createdDetail.gap as { rounds?: Array<{ actual?: string; target?: string }> } | undefined)?.rounds ?? [];
    expect(createdRounds.some((round) =>
      round.actual === "smoke-ai plan actual behavior two" &&
      round.target === "smoke-ai plan target behavior two"
    )).toBeTruthy();
    createdGapIds.add(createdId);
  } finally {
    await closePlanTabIfPresent(page);
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});
