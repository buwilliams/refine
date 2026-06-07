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
