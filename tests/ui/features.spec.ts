import { expect, test } from "@playwright/test";

async function selectReporter(page) {
  await page.locator("#nav-context-menu summary").click();
  await expect(page.locator("#global-reporter option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.locator("#global-reporter").selectOption("refine-smoke");
}

test("creates a Feature and adds a Feature Gap through the browser", async ({ page }) => {
  await page.goto("/");
  await selectReporter(page);

  await page.locator("#nav-create-menu summary").click();
  await page.locator("#btn-new-feature").click();
  await expect(page.getByRole("dialog", { name: "New Feature" })).toBeVisible();
  await page.locator("#feature-name").fill("Browser smoke feature");
  await page.locator("#feature-description").fill("Created by the Rust integration UI harness.");
  await page.getByRole("button", { name: "Create" }).click();

  await expect(page.locator(".feature-detail-modal")).toBeVisible();
  await expect(page.locator("#feature-name")).toHaveValue("Browser smoke feature");
  const featureId = /#\/features\/([^?]+)/.exec(page.url())?.[1];
  expect(featureId).toBeTruthy();

  await page.locator("[data-feature-new-gap]").click();
  await expect(page.getByRole("dialog", { name: "New Feature Gap" })).toBeVisible();
  await page.locator("textarea[name='actual']").fill("Feature browser smoke actual");
  await page.locator("textarea[name='target']").fill("Feature browser smoke target");
  const gapCreated = page.waitForResponse((response) =>
    response.url().includes("/api/gaps") &&
    response.request().method() === "POST" &&
    response.status() === 201
  );
  await page.getByRole("button", { name: "Create Gap" }).click();
  const gapResponse = await gapCreated;
  const gapPayload = await gapResponse.json();
  const gapName = String(gapPayload.gap?.name ?? "");
  expect(gapName).toContain("Feature browser smoke");
  expect(String(gapPayload.gap?.feature_id ?? "")).toBe(decodeURIComponent(featureId!));

  await page.locator(".feature-detail-modal .modal-close").click();
  await expect(page.locator(".feature-detail-modal")).toHaveCount(0);
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
  await expect(page.locator(".feature-detail-modal")).toBeVisible();
  await expect(page.getByText(gapName)).toBeVisible();
});
