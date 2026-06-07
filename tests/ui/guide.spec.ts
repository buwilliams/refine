import { expect, test } from "@playwright/test";
import { ensureAttachedProject } from "./helpers";

test("opens, resizes, persists checklist state, and searches Guide", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");
  await page.evaluate(() => {
    for (const key of Object.keys(localStorage)) {
      if (key.startsWith("refine_guide")) localStorage.removeItem(key);
    }
  });
  await page.reload();

  await page.getByTestId("context-menu-toggle").click();
  await page.getByTestId("nav-guide-open").click();
  await expect(page.getByTestId("guide-panel")).toHaveAttribute("aria-hidden", "false");
  await expect(page.getByTestId("guide-tab-get-started")).toHaveAttribute("aria-selected", "true");
  await expect(page).toHaveURL(/#\/node\/application$/);

  await expect(page.getByTestId("guide-prev-quickstart-add-app")).toBeDisabled();
  await expect(page.getByTestId("guide-default-quickstart-add-app")).toHaveCount(0);
  await page.getByTestId("guide-complete-quickstart-add-app").click();
  await expect(page.getByTestId("guide-status-quickstart-add-app")).toHaveAttribute("aria-label", /Checked:/);
  await expect(page.getByTestId("guide-open-item-quickstart-create-node")).toHaveAttribute("aria-expanded", "true");

  await page.getByTestId("guide-prev-quickstart-create-node").click();
  await expect(page.getByTestId("guide-open-item-quickstart-add-app")).toHaveAttribute("aria-expanded", "true");

  await page.getByTestId("guide-open-item-quickstart-create-node").click();
  await page.getByTestId("guide-skip-quickstart-create-node").click();
  await expect(page.getByTestId("guide-status-quickstart-create-node")).toHaveAttribute("aria-label", /Skipped:/);
  await expect(page.getByTestId("guide-open-item-quickstart-generate-ai")).toHaveAttribute("aria-expanded", "true");

  const widthBefore = await page.getByTestId("guide-panel").evaluate((el) => el.getBoundingClientRect().width);
  const handle = await page.getByTestId("guide-resize").boundingBox();
  expect(handle).toBeTruthy();
  await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle!.x - 80, handle!.y + handle!.height / 2);
  await page.mouse.up();
  const widthAfter = await page.getByTestId("guide-panel").evaluate((el) => el.getBoundingClientRect().width);
  expect(widthAfter).toBeGreaterThan(widthBefore);
  await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_guide_width") || "")).not.toBe("");

  await page.reload();
  await page.getByTestId("context-menu-toggle").click();
  await page.getByTestId("nav-guide-open").click();
  await expect(page.getByTestId("guide-status-quickstart-add-app")).toHaveAttribute("aria-label", /Checked:/);
  await expect(page.getByTestId("guide-status-quickstart-create-node")).toHaveAttribute("aria-label", /Skipped:/);

  await page.getByTestId("guide-tab-reference").click();
  await expect(page.getByTestId("guide-tab-reference")).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("guide-reference-search").fill("AI provider");
  await expect(page.getByTestId("guide-item-runtime-ai-provider")).toBeVisible();
  await expect(page.getByTestId("guide-item-nav-create-gap")).toHaveCount(0);

  await page.getByTestId("guide-open-item-runtime-ai-provider").click();
  await expect(page).toHaveURL(/#\/node\/runtime$/);
  await expect(page.getByTestId("runtime-provider-select")).toHaveClass(/guide-target-highlight/);

  await page.getByTestId("guide-close").click();
  await page.getByTestId("settings-guide-runtime-ai-provider").click();
  await expect(page.getByTestId("guide-panel")).toHaveAttribute("aria-hidden", "false");
  await expect(page.getByTestId("guide-tab-reference")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("guide-open-item-runtime-ai-provider")).toHaveAttribute("aria-expanded", "true");

  await page.getByTestId("guide-close").click();
  await expect(page.getByTestId("guide-panel")).toHaveAttribute("aria-hidden", "true");
});
