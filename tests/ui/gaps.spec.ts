import { expect, test } from "@playwright/test";
import { jsonObject } from "./helpers";

test("creates, updates, notes, and deletes a Gap through the browser", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
  await page.getByTestId("nav-new-gap").click();

  await expect(page.getByTestId("new-gap-modal")).toBeVisible();
  await page.getByTestId("new-gap-actual").fill("Browser smoke actual behavior");
  await page.getByTestId("new-gap-target").fill("Browser smoke target behavior");
  const gapCreated = page.waitForResponse((response) =>
    response.url().includes("/api/gaps") &&
    response.request().method() === "POST" &&
    response.status() === 201
  );
  await page.getByTestId("new-gap-submit").click();
  const gapPayload = await (await gapCreated).json();
  const gapId = String(gapPayload.gap?.id ?? "");
  const gapName = String(gapPayload.gap?.name ?? "");
  expect(gapId).toBeTruthy();
  expect(gapName).toContain("Browser smoke");

  await page.getByTestId("nav-gaps").click();
  await expect(page.getByText(gapName)).toBeVisible();
  await page.getByText(gapName).click();
  await expect(page.getByTestId("gap-detail")).toBeVisible();

  const transitioned = page.waitForResponse((response) =>
    response.url().includes(`/api/gaps/${gapId}`) &&
    response.request().method() === "PATCH" &&
    response.status() === 200
  );
  await page.getByTestId("gap-state-forward").click();
  const transitionedPayload = await (await transitioned).json();
  expect(transitionedPayload.gap?.status).toBe("todo");
  await page.goto(`/#/gaps/${gapId}`);
  await expect(page.locator(".gap-detail > .row .status-pill")).toHaveText("To do");
  await page.getByTestId("gap-round-actual").fill("Browser smoke actual behavior edited");
  await page.getByTestId("gap-round-target").fill("Browser smoke target behavior edited");
  const roundEdited = page.waitForResponse((response) =>
    response.url().includes(`/api/gaps/${gapId}/rounds/latest`) &&
    response.request().method() === "PATCH" &&
    response.status() === 200
  );
  await page.getByTestId("gap-round-submit").click();
  await roundEdited;
  await expect(page.getByTestId("gap-round-detail-actual").last()).toContainText("Browser smoke actual behavior edited");
  await expect(page.getByTestId("gap-round-detail-target").last()).toContainText("Browser smoke target behavior edited");

  await page.getByTestId("gap-notes-toggle").click();
  await page.getByTestId("gap-note-composer-toggle").click();
  await page.getByTestId("gap-note-body").fill("Browser smoke note");
  await page.getByTestId("gap-note-submit").click();
  await expect(page.getByTestId("gap-note-preview").filter({ hasText: "Browser smoke note" })).toBeVisible();

  await page.getByTestId("gap-note-summary").click();
  await expect(page.getByTestId("gap-note-detail")).toContainText("Browser smoke note");
  await page.getByTestId("gap-note-edit").click();
  await expect(page.getByTestId("modal-dialog")).toContainText("Edit note");
  await page.getByTestId("modal-input").fill("Browser smoke note edited");
  const noteEdited = page.waitForResponse((response) =>
    response.url().includes(`/api/gaps/${gapId}`) &&
    response.request().method() === "PATCH" &&
    (response.request().postData() || "").includes("Browser smoke note edited") &&
    response.status() === 200
  );
  await page.getByTestId("modal-ok").click();
  await noteEdited;
  await expect(page.getByTestId("gap-note-preview").filter({ hasText: "Browser smoke note edited" })).toBeVisible();

  await page.getByTestId("gap-note-summary").click();
  await page.getByTestId("gap-note-delete").click();
  await expect(page.getByTestId("modal-dialog")).toContainText("Delete note");
  const noteDeleted = page.waitForResponse((response) =>
    response.url().includes(`/api/gaps/${gapId}`) &&
    response.request().method() === "PATCH" &&
    (response.request().postData() || "").includes('"notes":[]') &&
    response.status() === 200
  );
  await page.getByTestId("modal-ok").click();
  await noteDeleted;
  await expect(page.getByTestId("gap-note")).toHaveCount(0);
  await expect(page.getByTestId("gap-notes")).toContainText("No notes yet.");

  await page.getByTestId("gap-action-menu-toggle").click();
  await page.getByTestId("gap-delete").click();
  await expect(page.getByText(`Delete Gap "${gapName}"? This cannot be undone.`)).toBeVisible();
  await page.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("heading", { name: "Gaps", level: 2 })).toBeVisible();
  await expect(page.getByText(gapName)).toHaveCount(0);
});

test("validates New Gap modal focus, priority, and dismissal behavior", async ({ page, request }) => {
  let gapId = "";
  await page.goto("/");
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");

  try {
    await page.getByTestId("nav-new-gap").click();
    await expect(page.getByTestId("new-gap-modal")).toBeVisible();
    await expect(page.getByTestId("new-gap-actual")).toBeFocused();
    await page.getByTestId("new-gap-submit").click();
    await expect(page.locator(".toast.error", { hasText: "Provide actual or target" })).toBeVisible();

    await page.getByTestId("new-gap-target").fill("Modal contract target behavior");
    await page.getByTestId("new-gap-priority").selectOption("high");
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/gaps") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("new-gap-submit").click();
    const createdPayload = await (await created).json();
    gapId = String(createdPayload.gap?.id ?? "");
    expect(gapId).toBeTruthy();
    expect(createdPayload.gap?.priority).toBe("high");
    expect(createdPayload.gap?.name).toContain("Modal contract target");
    await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);

    await page.goto("/#/gaps/new");
    await expect(page.getByTestId("new-gap-modal")).toBeVisible();
    await expect(page).toHaveURL(/#\/gaps\/new$/);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);
    await expect(page).toHaveURL(/#\/gaps$/);

    await page.getByTestId("nav-new-gap").click();
    await expect(page.getByTestId("new-gap-modal")).toBeVisible();
    await page.locator(".modal-backdrop").click({ position: { x: 2, y: 2 } });
    await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);
  } finally {
    if (gapId) await request.delete(`/api/gaps/${encodeURIComponent(gapId)}`);
  }
});

test("handles New Gap duplicate decisions through the browser", async ({ page, request }) => {
  const createdIds: string[] = [];
  const actual = `Duplicate modal actual ${Date.now()}`;
  const target = `Duplicate modal target ${Date.now()}`;
  const originalPayload = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual,
      target,
      priority: "low",
    },
  }));
  const originalId = String((originalPayload.gap as { id?: string } | undefined)?.id ?? "");
  expect(originalId).toBeTruthy();
  createdIds.push(originalId);

  const openDuplicateModal = async () => {
    await page.getByTestId("nav-new-gap").click();
    await expect(page.getByTestId("new-gap-modal")).toBeVisible();
    await page.getByTestId("new-gap-actual").fill(actual);
    await page.getByTestId("new-gap-target").fill(target);
    await page.getByTestId("new-gap-submit").click();
    await expect(page.getByTestId("new-gap-duplicate")).toContainText("Possible duplicate");
    await expect(page.getByTestId("new-gap-duplicate")).toContainText(actual);
    await expect(page.getByTestId("new-gap-duplicate")).toContainText(target);
  };

  try {
    await page.goto("/");
    await page.getByTestId("context-menu-toggle").click();
    await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
    await page.getByTestId("global-reporter").selectOption("refine-smoke");

    await openDuplicateModal();
    await page.getByTestId("new-gap-duplicate-ignore").click();
    await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);
    const afterIgnore = await jsonObject(await request.get(`/api/gaps?q=${encodeURIComponent(actual)}`));
    expect(afterIgnore.page.total).toBe(1);

    await openDuplicateModal();
    await page.getByTestId("new-gap-duplicate-import").click();
    await expect(page.getByTestId("new-gap-submit")).toHaveText("Create anyway");
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/gaps") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("new-gap-submit").click();
    const createdPayload = await (await created).json();
    const duplicateId = String(createdPayload.gap?.id ?? "");
    expect(duplicateId).toBeTruthy();
    expect(duplicateId).not.toBe(originalId);
    createdIds.push(duplicateId);
    await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);
    const afterImport = await jsonObject(await request.get(`/api/gaps?q=${encodeURIComponent(actual)}`));
    expect(afterImport.page.total).toBe(2);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
  }
});

test("submits follow-up and recovery rounds from Gap detail", async ({ page, request }) => {
  const createdIds: string[] = [];
  const createGapInStatus = async (label: string, status: string) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `${label} initial actual`,
        target: `${label} initial target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    await jsonObject(await request.post("/api/gaps/bulk", {
      data: {
        selected_ids: [id],
        update: { status },
      },
    }));
    return id;
  };

  const submitRound = async (
    gapId: string,
    heading: string,
    actual: string,
    target: string,
  ) => {
    await page.goto(`/#/gaps/${encodeURIComponent(gapId)}`);
    await expect(page.getByTestId("gap-detail")).toBeVisible();
    await expect(page.getByRole("heading", { name: heading, level: 3 })).toBeVisible();
    await expect(page.getByTestId("gap-round-submit")).toHaveText("Submit new round");
    await page.getByTestId("gap-round-actual").fill(actual);
    await page.getByTestId("gap-round-target").fill(target);
    const submitted = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${gapId}/rounds`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("gap-round-submit").click();
    await submitted;
    await expect(page.getByTestId("gap-round")).toHaveCount(2);
    await expect(page.getByTestId("gap-round-detail-actual").last()).toContainText(actual);
    await expect(page.getByTestId("gap-round-detail-target").last()).toContainText(target);
    const detail = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(gapId)}`));
    expect((detail.gap as { round_count?: number } | undefined)?.round_count).toBe(2);
  };

  try {
    await page.goto("/");
    await page.getByTestId("context-menu-toggle").click();
    await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
    await page.getByTestId("global-reporter").selectOption("refine-smoke");

    const reviewId = await createGapInStatus("Follow-up round", "review");
    await submitRound(
      reviewId,
      "Submit follow-up round",
      "Follow-up round actual",
      "Follow-up round target",
    );

    const failedId = await createGapInStatus("Recovery round", "failed");
    await submitRound(
      failedId,
      "Submit recovery round",
      "Recovery round actual",
      "Recovery round target",
    );
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
  }
});

test("filters and sorts Gaps through URL-backed controls", async ({ page, request }) => {
  const createdIds: string[] = [];
  let featureId = "";
  const createGap = async (actual: string, target: string, priority: string) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual,
        target,
        priority,
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    return id;
  };

  await createGap("Alpha gaps filter actual", "Alpha gaps filter target", "low");
  const betaId = await createGap("Beta gaps filter actual", "Beta gaps filter target", "high");
  const featurePayload = await jsonObject(await request.post("/api/features", {
    data: {
      name: `Gaps filter feature ${Date.now()}`,
      description: "Seeded for Gaps feature filter coverage",
      reporter: "refine-smoke",
    },
  }));
  featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
  expect(featureId).toBeTruthy();
  await jsonObject(await request.post(`/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(betaId)}`));
  await jsonObject(await request.post("/api/activity/ui-error", {
    data: {
      message: "Beta gaps filter activity",
      gap_id: betaId,
      marker: "gaps-filter-activity",
    },
  }));

  try {
    await page.goto(
      `/#/gaps?q=Beta&status=backlog&reporter=refine-smoke&node=current&feature=${encodeURIComponent(featureId)}` +
      "&rounds_gte=0&rounds_lte=1&severity=error&category=ui&actor=browser&sort=priority&dir=asc",
    );
    await expect(page.getByTestId("gaps-search")).toHaveValue("Beta");
    await expect(page.getByTestId("gaps-status-filter")).toHaveValue("backlog");
    await expect(page.getByTestId("gaps-reporter-filter")).toHaveValue("refine-smoke");
    await expect(page.getByTestId("gaps-node-filter")).toHaveValue("current");
    await expect(page.getByTestId("gaps-feature-filter")).toHaveValue(featureId);
    await expect(page.getByTestId("gaps-rounds-gte-filter")).toHaveValue("0");
    await expect(page.getByTestId("gaps-rounds-lte-filter")).toHaveValue("1");
    await expect(page.getByTestId("gaps-severity-filter")).toHaveValue("error");
    await expect(page.getByTestId("gaps-category-filter")).toHaveValue("ui");
    await expect(page.getByTestId("gaps-actor-filter")).toHaveValue("browser");
    await expect(page.getByTestId("gaps-filtered-pill")).toBeVisible();
    await expect(page.getByTestId("gaps-row")).toHaveCount(1);
    await expect(page.getByTestId("gaps-row")).toContainText("Beta gaps filter");

    await page.getByTestId("gaps-sort-priority").click();
    await expect(page).toHaveURL(/#\/gaps\?.*sort=priority.*dir=desc/);
    await expect(page.getByTestId("gaps-sort-priority")).toHaveClass(/active/);

    if (!(await page.getByTestId("gaps-filter-shell").evaluate((el) => (el as HTMLDetailsElement).open))) {
      await page.getByTestId("gaps-filter-shell").click();
    }
    await page.getByTestId("gaps-clear-filters").click();
    await expect(page).toHaveURL(/#\/gaps$/);
    await expect(page.getByTestId("gaps-search")).toHaveValue("");
    await expect(page.getByTestId("gaps-filtered-pill")).toBeHidden();

    await page.goto(`/#/gaps/${encodeURIComponent(betaId)}`);
    await expect(page.getByTestId("gap-detail")).toBeVisible();
    await expect(page.getByTestId("gap-detail")).toContainText("Beta gaps filter actual");
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
    if (featureId) await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
  }
});

test("scopes Gaps workflow visualization to current filters", async ({ page, request }) => {
  const createdIds: string[] = [];
  const createGap = async (actual: string) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual,
        target: `${actual} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    return id;
  };

  const backlogId = await createGap("Workflow scoped backlog actual");
  const todoId = await createGap("Workflow scoped todo actual");
  await jsonObject(await request.patch(`/api/gaps/${encodeURIComponent(todoId)}`, {
    data: { status: "todo" },
  }));

  try {
    await page.goto("/#/gaps?q=Workflow%20scoped&node=current");
    await expect(page.getByTestId("workflow-status-backlog")).toContainText("1");
    await expect(page.getByTestId("workflow-status-todo")).toContainText("AI");
    await expect(page.getByTestId("workflow-status-todo")).toContainText("1");

    await page.getByTestId("workflow-status-todo").click();
    await expect(page).toHaveURL(/#\/gaps\?.*q=Workflow\+scoped.*status=todo/);
    await expect(page.getByTestId("gaps-row")).toHaveCount(1);
    await expect(page.getByTestId("gaps-row")).toContainText("Workflow scoped todo");
    await expect(page.getByTestId("gaps-row")).not.toContainText("Workflow scoped backlog");
  } finally {
    for (const id of [todoId, backlogId, ...createdIds.filter((id) => id !== todoId && id !== backlogId)]) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
  }
});

test("bulk sets Gap status, priority, and reporter through modals", async ({ page, request }) => {
  const createdIds: string[] = [];
  const suffix = Date.now();
  const prefix = `Gaps bulk modal ${suffix}`;
  const reporterName = `bulk-reporter-${suffix}`;
  let reporterId = "";
  const createGap = async (index: number) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `${prefix} ${index} actual`,
        target: `${prefix} ${index} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    return id;
  };
  const selectBulkPage = async () => {
    if (!(await page.getByTestId("gaps-filter-shell").evaluate((el) => (el as HTMLDetailsElement).open))) {
      await page.getByTestId("gaps-filter-summary").click();
    }
    await page.getByTestId("gaps-select-page").click();
    await expect(page.getByTestId("gaps-row-select")).toHaveCount(3);
    await expect(page.getByTestId("gaps-row-select").first()).toBeChecked();
  };
  const applyBulk = async (commandId: string, valueTestId: string, value: string) => {
    await selectBulkPage();
    await expect.poll(async () =>
      page.evaluate(() => (window as unknown as { RefineCommands: { context: () => { route: string } } }).RefineCommands.context().route)
    ).toBe("gaps");
    await page.evaluate((id) => {
      void (window as unknown as { RefineCommands: { run: (commandId: string) => Promise<unknown> } }).RefineCommands.run(id);
    }, commandId);
    await expect(page.getByTestId(valueTestId)).toBeVisible();
    await page.getByTestId(valueTestId).selectOption(value);
    const updated = page.waitForResponse((response) =>
      response.url().includes("/api/gaps/bulk") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("bulk-apply").click();
    const payload = await (await updated).json();
    expect(payload.updated).toBe(3);
  };
  const gap = async (id: string) => {
    const payload = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(id)}`));
    return payload.gap as Record<string, unknown>;
  };

  await Promise.all([createGap(1), createGap(2), createGap(3)]);
  const reporterPayload = await jsonObject(await request.post("/api/reporters", {
    data: { name: reporterName },
  }));
  reporterId = String((reporterPayload.reporter as { id?: number | string } | undefined)?.id ?? "");
  expect(reporterId).toBeTruthy();

  try {
    await page.goto(`/#/gaps?q=${encodeURIComponent(prefix)}&node=current&limit=50&sort=name&dir=asc`);
    await expect(page.getByTestId("gaps-row")).toHaveCount(3);

    await applyBulk("gaps.bulk.status", "bulk-value-status", "todo");
    for (const id of createdIds) {
      expect((await gap(id)).status).toBe("todo");
    }

    await applyBulk("gaps.bulk.priority", "bulk-value-priority", "high");
    for (const id of createdIds) {
      expect((await gap(id)).priority).toBe("high");
    }

    await applyBulk("gaps.bulk.reporter", "bulk-value-reporter", reporterName);
    for (const id of createdIds) {
      expect((await gap(id)).reporter).toBe(reporterName);
    }
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
    if (reporterId) await request.delete(`/api/reporters/${encodeURIComponent(reporterId)}`);
  }
});

test("bulk assigns Features, transfers nodes, and deletes selected Gaps", async ({ page, request }) => {
  const createdGapIds = new Set<string>();
  let featureId = "";
  let transferNodeId = "";
  const suffix = Date.now();
  const createGap = async (prefix: string, index: number) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `${prefix} ${index} actual`,
        target: `${prefix} ${index} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdGapIds.add(id);
    return id;
  };
  const gap = async (id: string) => {
    const payload = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(id)}`));
    return payload.gap as Record<string, unknown>;
  };
  const openFilteredSelection = async (prefix: string, count: number) => {
    await page.goto(`/#/gaps?q=${encodeURIComponent(prefix)}&node=current&limit=50&sort=name&dir=asc`);
    await expect(page.getByTestId("gaps-row")).toHaveCount(count);
    if (!(await page.getByTestId("gaps-filter-shell").evaluate((el) => (el as HTMLDetailsElement).open))) {
      await page.getByTestId("gaps-filter-summary").click();
    }
    await page.getByTestId("gaps-select-page").click();
    await expect(page.getByTestId("gaps-row-select")).toHaveCount(count);
    await expect(page.getByTestId("gaps-row-select").first()).toBeChecked();
  };
  const runGapsCommand = async (commandId: string) => {
    await expect.poll(async () =>
      page.evaluate(() => (window as unknown as { RefineCommands: { context: () => { route: string } } }).RefineCommands.context().route)
    ).toBe("gaps");
    await page.evaluate((id) => {
      void (window as unknown as { RefineCommands: { run: (commandId: string) => Promise<unknown> } }).RefineCommands.run(id);
    }, commandId);
  };

  const featurePrefix = `Bulk feature ${suffix}`;
  const transferPrefix = `Bulk transfer ${suffix}`;
  const deletePrefix = `Bulk delete ${suffix}`;
  const featureGapIds = await Promise.all([createGap(featurePrefix, 1), createGap(featurePrefix, 2)]);
  const transferGapIds = await Promise.all([createGap(transferPrefix, 1), createGap(transferPrefix, 2)]);
  const deleteGapIds = await Promise.all([createGap(deletePrefix, 1), createGap(deletePrefix, 2)]);
  const featurePayload = await jsonObject(await request.post("/api/features", {
    data: {
      name: `Bulk assign feature ${suffix}`,
      description: "Seeded for bulk assign UI coverage",
      reporter: "refine-smoke",
    },
  }));
  featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
  expect(featureId).toBeTruthy();
  const nodePayload = await jsonObject(await request.post("/api/nodes", {
    data: { id: `bulk-transfer-${suffix}` },
  }));
  const nodes = nodePayload.nodes as Array<{ id?: string }> | undefined ?? [];
  transferNodeId = String(nodes.find((node) => node.id === `bulk-transfer-${suffix}`)?.id ?? "");
  expect(transferNodeId).toBe(`bulk-transfer-${suffix}`);

  try {
    await openFilteredSelection(featurePrefix, 2);
    await runGapsCommand("gaps.bulk.feature");
    await expect(page.getByTestId("bulk-assign-feature-value")).toBeVisible();
    await page.getByTestId("bulk-assign-feature-value").selectOption(featureId);
    const assigned = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}/gaps/bulk`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("bulk-feature-apply").click();
    const assignedPayload = await (await assigned).json();
    expect(assignedPayload.updated).toBe(2);
    for (const id of featureGapIds) {
      expect((await gap(id)).feature_id).toBe(featureId);
    }

    await openFilteredSelection(transferPrefix, 2);
    await runGapsCommand("gaps.bulk.transfer_node");
    await expect(page.getByTestId("bulk-transfer-node-value")).toBeVisible();
    await page.getByTestId("bulk-transfer-node-value").selectOption(transferNodeId);
    const transferred = page.waitForResponse((response) =>
      response.url().includes("/api/nodes/transfer-gaps") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("bulk-transfer-apply").click();
    const transferredPayload = await (await transferred).json();
    expect(transferredPayload.updated).toBe(2);
    for (const id of transferGapIds) {
      expect((await gap(id)).node_id).toBe(transferNodeId);
    }

    await openFilteredSelection(deletePrefix, 2);
    await runGapsCommand("gaps.bulk.delete");
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete Gaps");
    const deleted = page.waitForResponse((response) =>
      response.url().includes("/api/gaps/bulk/delete") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    const deletedPayload = await (await deleted).json();
    expect(deletedPayload.deleted).toBe(2);
    for (const id of deleteGapIds) {
      createdGapIds.delete(id);
      expect((await request.get(`/api/gaps/${encodeURIComponent(id)}`)).ok()).toBe(false);
    }
  } finally {
    for (const id of Array.from(createdGapIds).reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
    if (featureId) await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
    if (transferNodeId) {
      await request.patch(`/api/nodes/${encodeURIComponent(transferNodeId)}`, {
        data: { archived: true },
      });
    }
  }
});

test("paginates Gaps and tracks filter-scoped selection across pages", async ({ page, request }) => {
  test.setTimeout(60_000);
  const createdIds: string[] = [];
  const prefix = `Gaps pagination ${Date.now()}`;
  for (let i = 0; i < 52; i++) {
    const label = `${prefix} ${String(i).padStart(2, "0")}`;
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `${label} actual`,
        target: `${label} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
  }

  try {
    await page.goto(`/#/gaps?q=${encodeURIComponent(prefix)}&node=current&limit=50&sort=name&dir=asc`);
    await expect(page.getByTestId("gaps-search")).toHaveValue(prefix);
    await expect(page.getByTestId("gaps-filtered-pill")).toBeVisible();
    await expect(page.getByTestId("gaps-count")).toHaveText("50 gaps");
    await expect(page.getByTestId("gaps-row")).toHaveCount(50);
    await expect(page.getByTestId("gaps-pagination")).toContainText("1-50 gaps");
    await expect(page.getByTestId("gaps-page-current")).toHaveText("Page 1");
    await expect(page.getByTestId("gaps-page-prev")).toBeDisabled();

    await page.getByTestId("gaps-filter-summary").click();
    await expect(page.getByTestId("gaps-filter-shell")).toHaveJSProperty("open", true);
    await expect(page.getByTestId("gaps-select-all")).toBeChecked();
    await expect(page.getByTestId("gaps-row-select")).toHaveCount(50);

    await page.getByTestId("gaps-row-select").first().uncheck();
    await expect(page.getByTestId("gaps-select-all")).toHaveJSProperty("indeterminate", true);

    await page.getByTestId("gaps-select-all").check();
    await expect(page.getByTestId("gaps-select-all")).toBeChecked();

    await page.getByTestId("gaps-select-page").click();
    await expect(page.getByTestId("gaps-select-all")).toHaveJSProperty("indeterminate", true);
    await expect(page.getByTestId("gaps-row-select").first()).toBeChecked();

    await page.getByTestId("gaps-page-next").click();
    await expect(page).toHaveURL(/#\/gaps\?.*page=2/);
    await expect(page.getByTestId("gaps-page-current")).toHaveText("Page 2");
    await expect(page.getByTestId("gaps-page-next")).toBeDisabled();
    await expect(page.getByTestId("gaps-count")).toHaveText("2 gaps");
    await expect(page.getByTestId("gaps-row")).toHaveCount(2);
    await expect(page.getByTestId("gaps-row-select").first()).not.toBeChecked();
    await expect(page.getByTestId("gaps-select-all")).toHaveJSProperty("indeterminate", true);

    await page.getByTestId("gaps-row").filter({ hasText: `${prefix} 51 target` }).click();
    await expect(page.getByTestId("gap-detail")).toBeVisible();
    await expect(page.getByTestId("gap-detail")).toContainText(`${prefix} 51 target`);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
  }
});
