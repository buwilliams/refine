import { expect, test } from "@playwright/test";

test("selects Smoke AI in runtime settings and re-checks auth", async ({ page }) => {
  await page.goto("/#/node/runtime");
  await expect(page.getByRole("heading", { name: "Node", level: 2 })).toBeVisible();
  await expect(page.getByTestId("runtime-provider-select")).toBeVisible();

  await page.getByTestId("runtime-provider-select").selectOption("smoke-ai");
  await expect(page.getByTestId("runtime-provider-select")).toHaveValue("smoke-ai");
  await page.getByTestId("runtime-recheck-auth").click();

  await expect(page.getByText("Auth OK")).toBeVisible();
});
