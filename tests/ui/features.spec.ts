import { expect, test } from "@playwright/test";
import { jsonObject } from "./helpers";

async function selectReporter(page) {
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
}

test("creates a Feature and adds a Feature Gap through the browser", async ({ page }) => {
  await page.goto("/");
  await selectReporter(page);

  await page.getByTestId("create-menu-toggle").click();
  await page.getByTestId("nav-new-feature").click();
  await expect(page.getByTestId("feature-create-modal")).toBeVisible();
  await page.getByTestId("feature-name").fill("Browser smoke feature");
  await page.getByTestId("feature-description").fill("Created by the Rust integration UI harness.");
  await page.getByTestId("feature-create-submit").click();

  await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
  await expect(page.getByTestId("feature-name")).toHaveValue("Browser smoke feature");
  const featureId = /#\/features\/([^?]+)/.exec(page.url())?.[1];
  expect(featureId).toBeTruthy();

  await page.getByTestId("feature-new-gap").click();
  await expect(page.getByTestId("new-gap-modal")).toBeVisible();
  await page.getByTestId("new-gap-actual").fill("Feature browser smoke actual");
  await page.getByTestId("new-gap-target").fill("Feature browser smoke target");
  const gapCreated = page.waitForResponse((response) =>
    response.url().includes("/api/gaps") &&
    response.request().method() === "POST" &&
    response.status() === 201
  );
  await page.getByTestId("new-gap-submit").click();
  const gapResponse = await gapCreated;
  const gapPayload = await gapResponse.json();
  const gapName = String(gapPayload.gap?.name ?? "");
  expect(gapName).toContain("Feature browser smoke");
  expect(String(gapPayload.gap?.feature_id ?? "")).toBe(decodeURIComponent(featureId!));

  await page.getByTestId("feature-modal-close").click();
  await expect(page.getByTestId("feature-detail-modal")).toHaveCount(0);
  const featureReloaded = page.waitForResponse((response) =>
    response.url().includes(`/api/features/${decodeURIComponent(featureId!)}`) &&
    response.request().method() === "GET" &&
    response.status() === 200
  );
  await page.goto(`/#/features/${featureId}`);
  const featurePayload = await (await featureReloaded).json();
  expect(
    (featurePayload.gap_ids ?? []).includes(String(gapPayload.gap?.id ?? ""))
  ).toBeTruthy();
  await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
  await expect(page.getByText(gapName)).toBeVisible();
});

test("validates New Feature modal name and creates with description", async ({ page, request }) => {
  let featureId = "";
  const featureName = `Feature modal validation ${Date.now()}`;
  await page.goto("/");
  await selectReporter(page);

  try {
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-new-feature").click();
    await expect(page.getByTestId("feature-create-modal")).toBeVisible();
    await expect(page.getByTestId("feature-name")).toBeFocused();
    await page.getByTestId("feature-create-submit").click();
    await expect(page.locator(".toast.error", { hasText: "Feature name is required" })).toBeVisible();
    await expect(page.getByTestId("feature-create-modal")).toBeVisible();

    await page.getByTestId("feature-name").fill(featureName);
    await page.getByTestId("feature-description").fill("Feature modal validation description");
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/features") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("feature-create-submit").click();
    const payload = await (await created).json();
    featureId = String(payload.feature?.id ?? "");
    expect(featureId).toBeTruthy();
    expect(payload.feature?.name).toBe(featureName);
    expect(payload.feature?.description).toBe("Feature modal validation description");
    expect(payload.feature?.reporter).toBe("refine-smoke");
    await expect(page).toHaveURL(new RegExp(`#\\/features\\/${featureId}$`));
    await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
    await expect(page.getByTestId("feature-name")).toHaveValue(featureName);
  } finally {
    if (featureId) await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
  }
});

test("filters and sorts Features through URL-backed controls", async ({ page, request }) => {
  const createdIds: string[] = [];
  const createFeature = async (name: string) => {
    const payload = await jsonObject(await request.post("/api/features", {
      data: {
        name,
        description: `Seeded feature ${name}`,
        reporter: "refine-smoke",
      },
    }));
    const id = String((payload.feature as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    return id;
  };

  await createFeature("Alpha filter smoke");
  const betaId = await createFeature("Beta filter smoke");

  try {
    await page.goto("/#/features?q=Beta&status=backlog&reporter=refine-smoke&node=current&sort=name&dir=asc");
    await expect(page.getByTestId("features-search")).toHaveValue("Beta");
    await expect(page.getByTestId("features-status-filter")).toHaveValue("backlog");
    await expect(page.getByTestId("features-reporter-filter")).toHaveValue("refine-smoke");
    await expect(page.getByTestId("features-node-filter")).toHaveValue("current");
    await expect(page.getByTestId("features-filtered-pill")).toBeVisible();
    await expect(page.getByTestId("features-row")).toHaveCount(1);
    await expect(page.getByTestId("features-row")).toContainText("Beta filter smoke");

    await page.getByTestId("features-sort-name").click();
    await expect(page).toHaveURL(/#\/features\?.*sort=name.*dir=desc/);
    await expect(page.getByTestId("features-sort-name")).toHaveClass(/active/);

    if (!(await page.getByTestId("features-filter-shell").evaluate((el) => (el as HTMLDetailsElement).open))) {
      await page.getByTestId("features-filter-shell").click();
    }
    await page.getByTestId("features-clear-filters").click();
    await expect(page).toHaveURL(/#\/features$/);
    await expect(page.getByTestId("features-search")).toHaveValue("");
    await expect(page.getByTestId("features-filtered-pill")).toBeHidden();

    await page.goto(`/#/features/${encodeURIComponent(betaId)}`);
    await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
    await expect(page.getByTestId("feature-name")).toHaveValue("Beta filter smoke");
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/features/${encodeURIComponent(id)}`);
    }
  }
});

test("paginates Features and preserves URL-backed filter state", async ({ page, request }) => {
  const createdIds: string[] = [];
  const prefix = `Feature pagination ${Date.now()}`;
  for (let i = 0; i < 52; i++) {
    const name = `${prefix} ${String(i).padStart(2, "0")}`;
    const payload = await jsonObject(await request.post("/api/features", {
      data: {
        name,
        description: `Seeded pagination feature ${i}`,
        reporter: "refine-smoke",
      },
    }));
    const id = String((payload.feature as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
  }

  try {
    await page.goto(`/#/features?q=${encodeURIComponent(prefix)}&limit=50&sort=name&dir=asc`);
    await expect(page.getByTestId("features-search")).toHaveValue(prefix);
    await expect(page.getByTestId("features-filtered-pill")).toBeVisible();
    await expect(page.getByTestId("features-count")).toHaveText("52 features");
    await expect(page.getByTestId("features-row")).toHaveCount(50);
    await expect(page.getByTestId("features-pagination")).toContainText("1-50 features");
    await expect(page.getByTestId("features-page-current")).toHaveText("Page 1");
    await expect(page.getByTestId("features-page-prev")).toBeDisabled();

    await page.getByTestId("features-filter-summary").click();
    await expect(page.getByTestId("features-filter-shell")).toHaveJSProperty("open", true);
    await page.getByTestId("features-filter-summary").click();
    await expect(page.getByTestId("features-filter-shell")).toHaveJSProperty("open", false);

    await page.getByTestId("features-page-next").click();
    await expect(page).toHaveURL(/#\/features\?.*page=2/);
    await expect(page.getByTestId("features-page-current")).toHaveText("Page 2");
    await expect(page.getByTestId("features-page-next")).toBeDisabled();
    await expect(page.getByTestId("features-row")).toHaveCount(2);
    await expect(page.getByTestId("features-row").first()).toContainText(`${prefix} 50`);

    await page.getByTestId("features-row").filter({ hasText: `${prefix} 51` }).click();
    await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
    await expect(page.getByTestId("feature-name")).toHaveValue(`${prefix} 51`);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/features/${encodeURIComponent(id)}`);
    }
  }
});

test("manages Feature detail actions and ordered Gaps through the browser", async ({ page, request }) => {
  test.setTimeout(60_000);
  const suffix = Date.now();
  let featureId = "";
  let featureDeleted = false;
  const gapIds: string[] = [];
  const gapNames: string[] = [];

  const featurePayload = await jsonObject(await request.post("/api/features", {
    data: {
      name: `Feature detail actions ${suffix}`,
      description: "Seeded for browser Feature detail actions.",
      reporter: "refine-smoke",
    },
  }));
  featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
  expect(featureId).toBeTruthy();

  for (let i = 0; i < 26; i++) {
    const created = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `Feature detail gap ${suffix} ${String(i).padStart(2, "0")} actual`,
        target: `Feature detail gap ${suffix} ${String(i).padStart(2, "0")} target`,
        priority: i % 3 === 0 ? "high" : i % 3 === 1 ? "medium" : "low",
      },
    }));
    const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
    const gapName = String((created.gap as { name?: string } | undefined)?.name ?? "");
    expect(gapId).toBeTruthy();
    expect(gapName).toBeTruthy();
    gapIds.push(gapId);
    gapNames.push(gapName);
    await jsonObject(await request.post(
      `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(gapId)}`,
    ));
  }

  try {
    const featureDetail = async () => jsonObject(await request.get(`/api/features/${encodeURIComponent(featureId)}`));
    const featureGapStatus = async (index = 0) => {
      const detail = await featureDetail();
      const feature = detail.feature as { gaps?: Array<{ status?: string }> } | undefined;
      return String((feature?.gaps ?? [])[index]?.status ?? "");
    };
    const refreshFeatureDetail = async () => {
      await page.goto(`/#/features/${encodeURIComponent(featureId)}`);
      await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
    };

    await page.goto(`/#/features/${encodeURIComponent(featureId)}`);
    await expect(page.getByTestId("feature-detail-modal")).toBeVisible();
    await expect(page.getByTestId("feature-status-pill")).toHaveText("Backlog");
    await expect(page.getByTestId("feature-progress")).toHaveText("0 / 26 done");
    await expect(page.getByTestId("feature-metadata")).toContainText(featureId);
    await expect(page.getByTestId("feature-gap-row")).toHaveCount(25);
    await expect(page.getByTestId("feature-modal-gaps-pagination")).toContainText("1-25 gaps");

    await page.getByTestId("feature-modal-gaps-page-next").click();
    await expect(page.getByTestId("feature-modal-gaps-page-current")).toHaveText("Page 2");
    await expect(page.getByTestId("feature-gap-row")).toHaveCount(1);
    await expect(page.getByTestId("feature-gap-link")).toContainText([gapNames[25]]);
    await page.getByTestId("feature-modal-gaps-page-prev").click();
    await expect(page.getByTestId("feature-modal-gaps-page-current")).toHaveText("Page 1");
    await expect(page.getByTestId("feature-gap-link").first()).toHaveText(gapNames[0]);

    await page.getByTestId("feature-name").fill("");
    await expect(page.locator(".toast.error", { hasText: "Feature name is required" })).toBeVisible();
    await expect(page.getByTestId("feature-name")).toHaveValue(`Feature detail actions ${suffix}`);

    const renamedName = `Feature detail renamed ${suffix}`;
    const renamedDescription = "Autosaved from Feature detail Playwright coverage.";
    const renamed = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("feature-name").fill(renamedName);
    await page.getByTestId("feature-description").fill(renamedDescription);
    await renamed;
    await expect.poll(async () => {
      const detail = await jsonObject(await request.get(`/api/features/${encodeURIComponent(featureId)}`));
      return [
        String((detail.feature as { name?: string } | undefined)?.name ?? ""),
        String((detail.feature as { description?: string } | undefined)?.description ?? ""),
      ].join("\n");
    }).toBe(`${renamedName}\n${renamedDescription}`);

    const movedTodo = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}/workflow`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("feature-workflow-todo").click();
    await movedTodo;
    await expect.poll(async () => featureGapStatus()).toBe("todo");
    await refreshFeatureDetail();
    await expect(page.getByTestId("feature-gap-status").first()).toHaveText("To do");
    await expect(page.getByTestId("feature-workflow-todo")).toBeDisabled();

    const movedBacklog = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}/workflow`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("feature-workflow-backlog").click();
    await movedBacklog;
    await expect.poll(async () => featureGapStatus()).toBe("backlog");
    await refreshFeatureDetail();
    await expect(page.getByTestId("feature-gap-status").first()).toHaveText("Backlog");

    await page.getByTestId("feature-gap-row").nth(1).getByTestId("feature-gap-move-up").click();
    await expect(page.getByTestId("feature-gap-link").first()).toHaveText(gapNames[1]);
    await expect(page.getByTestId("feature-gap-order").first()).toHaveText("1");

    const deletedGap = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${encodeURIComponent(gapIds[1])}`) &&
      response.request().method() === "DELETE" &&
      response.status() === 200
    );
    await page.getByTestId("feature-gap-row").first().getByTestId("feature-gap-delete").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete Gap");
    await page.getByTestId("modal-ok").click();
    await deletedGap;
    await expect.poll(async () => {
      const detail = await featureDetail();
      return Number((detail.feature as { gap_count?: number } | undefined)?.gap_count ?? 0);
    }).toBe(25);
    await refreshFeatureDetail();
    await expect(page.getByTestId("feature-progress")).toHaveText("0 / 25 done");
    await expect(page.getByTestId("feature-gap-link").first()).toHaveText(gapNames[0]);
    await expect(await request.get(`/api/gaps/${encodeURIComponent(gapIds[1])}`)).not.toBeOK();

    const cancelled = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}/cancel`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("feature-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Cancel Feature");
    await page.getByTestId("modal-ok").click();
    await cancelled;
    await expect.poll(async () => {
      const detail = await featureDetail();
      return String((detail.feature as { status?: string } | undefined)?.status ?? "");
    }).toBe("cancelled");
    await refreshFeatureDetail();
    await expect(page.getByTestId("feature-status-pill")).toHaveText("Cancelled");

    const deletedFeature = page.waitForResponse((response) =>
      response.url().includes(`/api/features/${encodeURIComponent(featureId)}`) &&
      response.request().method() === "DELETE" &&
      response.status() === 200
    );
    await page.getByTestId("feature-delete").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete Feature");
    await page.getByTestId("modal-ok").click();
    await deletedFeature;
    featureDeleted = true;
    await expect(page).toHaveURL(/#\/features$/);
    await expect(page.getByTestId("feature-detail-modal")).toHaveCount(0);
    await expect.poll(async () => {
      const response = await request.get(`/api/features/${encodeURIComponent(featureId)}`);
      return response.ok();
    }).toBe(false);
  } finally {
    let cleanedByFeatureDelete = featureDeleted;
    if (!featureDeleted && featureId) {
      const cleanup = await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
      cleanedByFeatureDelete = cleanup.ok();
    }
    if (!cleanedByFeatureDelete) {
      for (const gapId of gapIds.reverse()) {
        await request.delete(`/api/gaps/${encodeURIComponent(gapId)}`);
      }
    }
  }
});
