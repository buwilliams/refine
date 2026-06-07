import { expect, test } from "@playwright/test";

test("navigates between the primary sections from the nav", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();

  await page.getByRole("navigation").getByRole("link", { name: "Features" }).click();
  await expect(page.getByRole("heading", { name: "Features", level: 2 })).toBeVisible();

  await page.getByRole("navigation").getByRole("link", { name: "Gaps" }).click();
  await expect(page.getByRole("heading", { name: "Gaps", level: 2 })).toBeVisible();

  await page.getByRole("navigation").getByRole("link", { name: "Changes" }).click();
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();

  await page.getByRole("navigation").getByRole("link", { name: "Logs" }).click();
  await expect(page.getByRole("heading", { name: "Logs", level: 2 })).toBeVisible();

  await page.getByRole("navigation").getByRole("link", { name: "Dashboard" }).click();
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();
});
