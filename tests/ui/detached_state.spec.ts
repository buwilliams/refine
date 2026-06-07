import { expect, test } from "@playwright/test";
import { attachProject, detachProject, projectStatus } from "./helpers";

test("shows detached no-project state and reattaches the test app", async ({ page, request }) => {
  const before = await projectStatus(request);
  const appPath = String(before.client_repo ?? "");
  expect(before.attached).toBe(true);
  expect(appPath).toMatch(/rust-test-app$/);

  try {
    const detached = await detachProject(request);
    expect(detached.attached).toBe(false);

    await page.goto("/#/node/application");
    await expect(page.getByRole("heading", { name: "Node", level: 2 })).toBeVisible();
    await expect(page.getByText("Current app:")).toBeVisible();
    await expect(page.getByText("Not attached")).toBeVisible();
    await expect(page.getByTestId("project-app-select")).toBeEnabled();
    await expect(page.getByTestId("project-add-app")).toBeEnabled();
    await expect(page.getByTestId("project-switch-app")).toBeEnabled();

    await page.getByTestId("nav-dashboard").click();
    await expect(page).toHaveURL(/#\/$/);
    await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();
    await expect(page.getByTestId("no-project-empty")).toContainText("No app configured.");
    await expect(page.getByTestId("no-project-open-guide")).toBeVisible();

    await page.goto("/#/node/reporters");
    await expect(page.getByTestId("settings-no-project")).toContainText("No app configured.");
    await expect(page.getByTestId("settings-open-guide")).toBeVisible();

    await page.goto("/#/node/runtime");
    await expect(page.getByTestId("settings-detached-config")).toContainText("No app attached.");
    await expect(page.getByTestId("runtime-provider-select")).toBeDisabled();
  } finally {
    await attachProject(request, appPath);
    await expect.poll(async () => (await projectStatus(request)).attached).toBe(true);
  }
});
