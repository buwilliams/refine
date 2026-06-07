import { expect, test } from "@playwright/test";

test("runs a Plan chat turn through Smoke AI", async ({ page }) => {
  await page.goto("/#/gaps/plan");

  await expect(page.locator(".toolbar-tab.active")).toContainText("Plan");
  await expect(page.locator("#chat-input")).toBeVisible();

  await page.locator("#chat-input").fill("Start a chat conversation about a deterministic planning workflow.");
  await page.locator("#btn-chat-send").click();

  await expect(page.locator("#chat-output")).toContainText("smoke-ai chat response", { timeout: 45_000 });
  await expect(page.locator("#btn-plan-draft")).toBeEnabled();
});
