import { expect, test } from "@playwright/test";

test("creates, updates, notes, and deletes a Gap through the browser", async ({ page }) => {
  await page.goto("/");
  await page.locator("#nav-context-menu summary").click();
  await expect(page.locator("#global-reporter option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.locator("#global-reporter").selectOption("refine-smoke");
  await page.locator("#btn-new-gap").click();

  await expect(page.getByRole("dialog", { name: "New Gap" })).toBeVisible();
  await page.locator("textarea[name='actual']").fill("Browser smoke actual behavior");
  await page.locator("textarea[name='target']").fill("Browser smoke target behavior");
  const gapCreated = page.waitForResponse((response) =>
    response.url().includes("/api/gaps") &&
    response.request().method() === "POST" &&
    response.status() === 201
  );
  await page.getByRole("button", { name: "Create Gap" }).click();
  const gapPayload = await (await gapCreated).json();
  const gapId = String(gapPayload.gap?.id ?? "");
  const gapName = String(gapPayload.gap?.name ?? "");
  expect(gapId).toBeTruthy();
  expect(gapName).toContain("Browser smoke");

  await page.getByRole("navigation").getByRole("link", { name: "Gaps" }).click();
  await expect(page.getByText(gapName)).toBeVisible();
  await page.getByText(gapName).click();
  await expect(page.locator(".gap-detail")).toBeVisible();

  const transitioned = page.waitForResponse((response) =>
    response.url().includes(`/api/gaps/${gapId}`) &&
    response.request().method() === "PATCH" &&
    response.status() === 200
  );
  await page.locator("#btn-state-forward").click();
  const transitionedPayload = await (await transitioned).json();
  expect(transitionedPayload.gap?.status).toBe("todo");
  await page.goto(`/#/gaps/${gapId}`);
  await expect(page.locator(".gap-detail > .row .status-pill")).toHaveText("To do");

  await page.locator(".notes-card > summary").click();
  await page.locator(".note-composer summary").click();
  await page.locator("#new-note-body").fill("Browser smoke note");
  await page.locator("#btn-add-note").click();
  await expect(page.locator(".note-preview", { hasText: "Browser smoke note" })).toBeVisible();

  await page.locator("#gap-action-menu summary").click();
  await page.locator("#btn-delete").click();
  await expect(page.getByText(`Delete Gap "${gapName}"? This cannot be undone.`)).toBeVisible();
  await page.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("heading", { name: "Gaps", level: 2 })).toBeVisible();
  await expect(page.getByText(gapName)).toHaveCount(0);
});
