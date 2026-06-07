import { expect, test } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

test("loads the app shell with primary navigation", async ({ page }) => {
  await page.goto("/");
  const nav = page.getByRole("navigation");
  await expect(nav.getByRole("link", { name: "Dashboard" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Features" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Gaps" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Changes" })).toBeVisible();
  await expect(nav.getByRole("link", { name: "Logs" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();
});

test("reports version and attached project over public APIs", async ({ request }) => {
  const version = await jsonObject(await request.get("/system/version"));
  expect(version.product).toBe("refine");
  expect(typeof version.version).toBe("string");
  await ensureAttachedProject(request);
});

test("answers the Gaps projection query", async ({ request }) => {
  const gaps = await jsonObject(await request.get("/api/gaps"));
  expect(Array.isArray(gaps.gaps)).toBe(true);
  expect(typeof gaps.page).toBe("object");
});
