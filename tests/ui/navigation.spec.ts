import { expect, test } from "@playwright/test";

test("navigates between the primary sections from the nav", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();

  await page.getByTestId("nav-features").click();
  await expect(page.getByRole("heading", { name: "Features", level: 2 })).toBeVisible();

  await page.getByTestId("nav-gaps").click();
  await expect(page.getByRole("heading", { name: "Gaps", level: 2 })).toBeVisible();

  await page.getByTestId("nav-changes").click();
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();

  await page.getByTestId("nav-logs").click();
  await expect(page.getByRole("heading", { name: "Logs", level: 2 })).toBeVisible();

  await page.getByTestId("nav-dashboard").click();
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();
});

test("shows create dropdown actions and opens refine request from the menu", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("create-menu-toggle").click();
  await expect(page.getByTestId("nav-new-feature")).toBeVisible();
  await expect(page.getByTestId("nav-plan-mode")).toBeVisible();
  await expect(page.getByTestId("nav-import-gaps")).toBeVisible();
  await expect(page.getByTestId("nav-refine-issue-menu")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(page.getByTestId("nav-new-feature")).not.toBeVisible();

  await page.getByTestId("create-menu-toggle").click();
  await page.getByTestId("nav-refine-issue-menu").click();
  await expect(page.getByTestId("refine-issue-modal")).toBeVisible();
  await expect(page.getByTestId("nav-new-feature")).not.toBeVisible();
  await page.getByTestId("refine-issue-cancel").click();
  await expect(page.getByTestId("refine-issue-modal")).toHaveCount(0);
});
