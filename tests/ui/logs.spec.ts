import { expect, test } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

test("filters Logs, visualizes severity buckets, and expands details", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const prefix = `Logs visualization ${Date.now()}`;
  for (const label of ["first", "second", "third"]) {
    await jsonObject(await request.post("/api/activity/ui-error", {
      data: {
        message: `${prefix} ${label}`,
        marker: prefix,
        source: "logs.spec",
        detail: `detail ${label}`,
      },
    }));
  }

  await page.goto(`/#/logs?q=${encodeURIComponent(prefix)}&severity=error&category=ui&actor=browser&period=day`);
  await expect(page.getByRole("heading", { name: "Logs", level: 2 })).toBeVisible();
  await expect(page.getByTestId("logs-visualization-panel")).toBeVisible();
  await expect(
    page.getByTestId("logs-visualization-panel").getByTestId("logs-period-control"),
  ).toBeVisible();
  await expect(page.getByTestId("logs-filter-shell")).toBeVisible();
  expect(await page.evaluate(() => {
    const visualization = document.querySelector("[data-testid='logs-visualization-panel']");
    const filters = document.querySelector("[data-testid='logs-filter-shell']");
    return !!(
      visualization &&
      filters &&
      (visualization.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING)
    );
  })).toBe(true);
  await expect(page.getByTestId("logs-search")).toHaveValue(prefix);
  await expect(page.getByTestId("logs-severity-filter")).toHaveValue("error");
  await expect(page.getByTestId("logs-category-filter")).toHaveValue("ui");
  await expect(page.getByTestId("logs-actor-filter")).toHaveValue("browser");
  await expect(page.getByTestId("logs-filtered-pill")).toBeVisible();
  await expect(page.getByTestId("logs-count")).toHaveText("3 entries");
  await expect(page.getByTestId("logs-row")).toHaveCount(3);
  await expect(page.getByTestId("logs-visualization-panel")).toHaveCSS("overflow", "visible");
  await expect(page.getByTestId("logs-visualization-grid")).toHaveCSS("display", "grid");
  await expect(page.getByTestId("logs-visualization-grid")).toHaveCSS("overflow-x", "hidden");
  await expect(page.getByTestId("logs-bucket")).toHaveCount(1);
  await expect(page.getByTestId("logs-bucket").first()).not.toHaveCSS("white-space", "nowrap");
  const gridOverflow = await page.getByTestId("logs-visualization-grid").evaluate((el) => ({
    clientWidth: el.clientWidth,
    scrollWidth: el.scrollWidth,
  }));
  expect(gridOverflow.scrollWidth).toBeLessThanOrEqual(gridOverflow.clientWidth + 1);
  const firstBucketBox = await page.getByTestId("logs-bucket").first().boundingBox();
  expect(firstBucketBox?.height ?? 0).toBeGreaterThanOrEqual(100);
  expect(firstBucketBox?.width ?? 0).toBeGreaterThan(firstBucketBox?.height ?? 0);
  await expect(page.getByTestId("logs-severity-error")).toHaveText("error 3");

  const firstRow = page.getByTestId("logs-row").filter({ hasText: `${prefix} first` });
  await expect(firstRow).toHaveCount(1);
  await firstRow.getByTestId("logs-show-details").click();
  await expect(firstRow.getByTestId("logs-details")).toContainText('"source": "logs.spec"');
  await expect(firstRow.getByTestId("logs-details")).toContainText(prefix);

  await page.getByTestId("logs-period-week").click();
  await expect(page).toHaveURL(/#\/logs\?.*period=week/);
  await expect(page.getByTestId("logs-period-week")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("logs-severity-error")).toHaveText("error 3");

  await page.getByTestId("logs-period-month").click();
  await expect(page).toHaveURL(/#\/logs\?.*period=month/);
  await expect(page.getByTestId("logs-period-month")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("logs-severity-error")).toHaveText("error 3");

  await page.getByTestId("logs-filter-summary").click();
  await page.getByTestId("logs-clear-filters").click();
  await expect(page).toHaveURL(/#\/logs$/);
  await expect(page.getByTestId("logs-search")).toHaveValue("");
  await expect(page.getByTestId("logs-filtered-pill")).toBeHidden();
});

test("paginates and sorts Logs through URL-backed controls", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const prefix = `Logs pagination ${Date.now()}`;
  for (let i = 0; i < 52; i++) {
    await jsonObject(await request.post("/api/activity/ui-error", {
      data: {
        message: `${prefix} ${String(i).padStart(2, "0")}`,
        marker: prefix,
        source: "logs-pagination.spec",
        index: i,
      },
    }));
  }

  await page.goto(`/#/logs?q=${encodeURIComponent(prefix)}&limit=50&sort=datetime&dir=desc`);
  await expect(page.getByTestId("logs-search")).toHaveValue(prefix);
  await expect(page.getByTestId("logs-row")).toHaveCount(50);
  await expect(page.getByTestId("logs-pagination")).toContainText("1-50 entries");
  await expect(page.getByTestId("logs-page-current")).toHaveText("Page 1");
  await expect(page.getByTestId("logs-page-prev")).toBeDisabled();
  await expect(page.getByTestId("logs-page-next")).toBeEnabled();
  await expect(page.getByTestId("logs-page-last")).toBeEnabled();

  await page.getByTestId("logs-sort-category").click();
  await expect(page).toHaveURL(/#\/logs\?.*sort=category.*dir=asc/);
  await expect(page.getByTestId("logs-sort-category")).toHaveClass(/active/);

  await page.getByTestId("logs-page-next").click();
  await expect(page).toHaveURL(/#\/logs\?.*page=2/);
  await expect(page.getByTestId("logs-page-current")).toHaveText("Page 2");
  await expect(page.getByTestId("logs-row")).toHaveCount(2);
  await expect(page.getByTestId("logs-page-prev")).toBeEnabled();
  await expect(page.getByTestId("logs-page-next")).toBeDisabled();
});
