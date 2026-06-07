import { expect, test } from "@playwright/test";

test("selects Smoke AI in runtime settings and re-checks auth", async ({ page }) => {
  await page.goto("/#/node/runtime");
  await expect(page.getByRole("heading", { name: "Node", level: 2 })).toBeVisible();
  await expect(page.locator("#s-cli")).toBeVisible();

  await page.locator("#s-cli").selectOption("smoke-ai");
  await expect(page.locator("#s-cli")).toHaveValue("smoke-ai");
  await page.locator("#s-recheck").click();

  await expect(page.getByText("Auth OK")).toBeVisible();
});
