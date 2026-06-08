import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { expect, test, type APIRequestContext } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

function testAppRoot(): string {
  return process.env.REFINE_TEST_APP_ROOT ||
    path.join(process.cwd(), "target/refine-integration/apps/rust-test-app");
}

function git(args: string[], env: Record<string, string> = {}) {
  const result = spawnSync("git", args, {
    cwd: testAppRoot(),
    encoding: "utf-8",
    env: { ...process.env, ...env },
  });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
  }
}

function gitOutput(args: string[]): string {
  const result = spawnSync("git", args, { cwd: testAppRoot(), encoding: "utf-8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`);
  }
  return result.stdout;
}

async function syncProject(request: APIRequestContext) {
  await jsonObject(await request.post("/api/project/sync", { data: {} }));
}

async function waitForChangeTotal(
  request: APIRequestContext,
  query: string,
  expected: number,
) {
  const encoded = encodeURIComponent(query);
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const payload = await jsonObject(
      await request.get(`/api/changes?q=${encoded}&status=backlog&priority=high&limit=50&offset=0`),
    );
    const total = Number((payload.page as { total?: number } | undefined)?.total ?? 0);
    if (total >= expected) return payload;
    await syncProject(request);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return jsonObject(
    await request.get(`/api/changes?q=${encoded}&status=backlog&priority=high&limit=50&offset=0`),
  );
}

test("filters, sorts, and paginates Changes through URL-backed controls", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const prefix = `changes-ui-${suffix}`;
  const created = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual: `Changes route actual ${suffix}`,
      target: `Changes route target ${suffix}`,
      priority: "high",
    },
  }));
  const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
  expect(gapId).toBeTruthy();

  const appRoot = testAppRoot();
  const seedPath = path.join(appRoot, "changes-ui-seed.txt");
  for (let i = 0; i < 52; i += 1) {
    fs.appendFileSync(seedPath, `${prefix}-${String(i).padStart(2, "0")}\n`);
    git(["add", "changes-ui-seed.txt"]);
    git(["commit", "-q", "-m", `${prefix}-${String(i).padStart(2, "0")} ${gapId}`]);
  }
  await syncProject(request);
  const seeded = await waitForChangeTotal(request, prefix, 52);
  expect(Number((seeded.page as { total?: number } | undefined)?.total ?? 0)).toBeGreaterThanOrEqual(52);

  await page.goto(`/#/changes?q=${encodeURIComponent(prefix)}&status=backlog&priority=high&limit=50`);
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();
  await expect(page.getByTestId("changes-branch-info")).toContainText(
    gitOutput(["branch", "--show-current"]).trim(),
  );
  await expect(page.getByTestId("changes-visualization-panel")).toBeVisible();
  await expect(
    page.getByTestId("changes-visualization-panel").getByTestId("changes-period-control"),
  ).toBeVisible();
  await expect(page.getByTestId("changes-filter-shell")).toBeVisible();
  expect(await page.evaluate(() => {
    const visualization = document.querySelector("[data-testid='changes-visualization-panel']");
    const filters = document.querySelector("[data-testid='changes-filter-shell']");
    return !!(
      visualization &&
      filters &&
      (visualization.compareDocumentPosition(filters) & Node.DOCUMENT_POSITION_FOLLOWING)
    );
  })).toBe(true);
  await expect(page.getByTestId("changes-search")).toHaveValue(prefix);
  await expect(page.getByTestId("changes-status-filter")).toHaveValue("backlog");
  await expect(page.getByTestId("changes-priority-filter")).toHaveValue("high");
  await expect(page.getByTestId("changes-filtered-pill")).toBeVisible();
  await expect(page.getByTestId("changes-row")).toHaveCount(50);
  await expect(page.getByTestId("changes-status-cell").first()).toContainText("backlog");
  await expect(page.getByTestId("changes-priority-cell").first()).toContainText("high");
  await expect(page.getByTestId("changes-pagination")).toContainText("1-50 changes");
  await expect(page.getByTestId("changes-page-current")).toHaveText("Page 1");
  await expect(page.getByTestId("changes-page-prev")).toBeDisabled();
  await expect(page.getByTestId("changes-page-next")).toBeEnabled();

  const sorted = page.waitForResponse((response) =>
    response.url().includes("/api/changes?") &&
    response.url().includes("sort=commit") &&
    response.url().includes("dir=asc") &&
    response.status() === 200
  );
  await page.getByTestId("changes-sort-commit").click();
  await sorted;
  await expect(page).toHaveURL(/#\/changes\?.*sort=commit/);
  await expect(page).toHaveURL(/#\/changes\?.*dir=asc/);
  await expect(page.getByTestId("changes-sort-commit")).toContainText("↑");

  const nextPage = page.waitForResponse((response) =>
    response.url().includes("/api/changes?") &&
    response.url().includes("offset=50") &&
    response.status() === 200
  );
  await page.getByTestId("changes-page-next").click();
  await nextPage;
  await expect(page).toHaveURL(/page=2/);
  await expect(page.getByTestId("changes-page-current")).toHaveText("Page 2");
  await expect(page.getByTestId("changes-row")).toHaveCount(2);
  await expect(page.getByTestId("changes-page-prev")).toBeEnabled();
  await expect(page.getByTestId("changes-page-next")).toBeDisabled();

  await page.getByTestId("changes-filter-shell").evaluate((el: HTMLDetailsElement) => {
    el.open = true;
  });
  await page.getByTestId("changes-clear-filters").click();
  await expect(page).toHaveURL(/#\/changes$/);
});

test("visualizes Git changes by day, week, month, and year", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const prefix = `changes-viz-${suffix}`;
  const created = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual: `Changes visualization actual ${suffix}`,
      target: `Changes visualization target ${suffix}`,
      priority: "high",
    },
  }));
  const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
  expect(gapId).toBeTruthy();

  const vizPath = path.join(testAppRoot(), `changes-viz-${suffix}.txt`);
  const commits = [
    ["2024-01-03T12:00:00Z", "day-one"],
    ["2024-01-10T12:00:00Z", "day-two"],
    ["2024-02-05T12:00:00Z", "day-three"],
    ["2025-03-06T12:00:00Z", "day-four"],
  ];
  for (const [date, label] of commits) {
    fs.appendFileSync(vizPath, `${prefix}-${label}\n`);
    git(["add", path.basename(vizPath)]);
    git(["commit", "-q", "-m", `${prefix} ${label} ${gapId}`], {
      GIT_AUTHOR_DATE: date,
      GIT_COMMITTER_DATE: date,
    });
  }
  await syncProject(request);
  const seeded = await waitForChangeTotal(request, prefix, 4);
  expect(Number((seeded.page as { total?: number } | undefined)?.total ?? 0)).toBeGreaterThanOrEqual(4);

  await page.goto(`/#/changes?q=${encodeURIComponent(prefix)}&status=backlog&priority=high&limit=50`);
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();
  await expect(page.getByTestId("changes-period-day")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("changes-visualization-panel").getByTestId("changes-period-control")).toBeVisible();
  await expect(page.getByTestId("changes-visualization-grid")).toHaveCSS("flex-wrap", "nowrap");
  await expect(page.getByTestId("changes-bucket").first()).toHaveCSS("white-space", "nowrap");
  await expect(page.getByTestId("changes-bucket")).toHaveCount(4);
  await expect(page.getByTestId("changes-bucket-label")).toContainText([
    "2025-03-06",
    "2024-02-05",
    "2024-01-10",
    "2024-01-03",
  ]);

  await page.getByTestId("changes-period-week").click();
  await expect(page).toHaveURL(/#\/changes\?.*period=week/);
  await expect(page.getByTestId("changes-period-week")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("changes-bucket-label")).toContainText([
    "2025-03-02",
    "2024-02-04",
    "2024-01-07",
    "2023-12-31",
  ]);

  await page.getByTestId("changes-period-month").click();
  await expect(page).toHaveURL(/#\/changes\?.*period=month/);
  await expect(page.getByTestId("changes-period-month")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("changes-bucket")).toHaveCount(3);
  const januaryBucket = page.getByTestId("changes-bucket").filter({
    has: page.getByTestId("changes-bucket-label").filter({ hasText: "2024-01" }),
  });
  await expect(januaryBucket.getByTestId("changes-bucket-total")).toHaveText("2 changes");
  await expect(januaryBucket.getByTestId("changes-bucket-linked")).toHaveText("2 linked Gaps");

  await page.getByTestId("changes-period-year").click();
  await expect(page).toHaveURL(/#\/changes\?.*period=year/);
  await expect(page.getByTestId("changes-period-year")).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("changes-bucket")).toHaveCount(2);
  const yearBucket = page.getByTestId("changes-bucket").filter({
    has: page.getByTestId("changes-bucket-label").filter({ hasText: "2024" }),
  });
  await expect(yearBucket.getByTestId("changes-bucket-total")).toHaveText("3 changes");
  await expect(yearBucket.getByTestId("changes-bucket-linked")).toHaveText("3 linked Gaps");
});

test("confirms Changes undo, reverts git, and cancels the linked Gap", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const prefix = `changes-undo-${suffix}`;
  const created = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual: `Changes undo actual ${suffix}`,
      target: `Changes undo target ${suffix}`,
      priority: "high",
    },
  }));
  const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
  expect(gapId).toBeTruthy();

  const undoPath = path.join(testAppRoot(), `changes-undo-${suffix}.txt`);
  fs.writeFileSync(undoPath, `${prefix}\n`);
  git(["add", path.basename(undoPath)]);
  git(["commit", "-q", "-m", `${prefix} ${gapId}`]);
  await syncProject(request);
  const seeded = await waitForChangeTotal(request, prefix, 1);
  expect(Number((seeded.page as { total?: number } | undefined)?.total ?? 0)).toBeGreaterThanOrEqual(1);

  await page.goto(`/#/changes?q=${encodeURIComponent(prefix)}&status=backlog&priority=high&limit=50`);
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();
  await expect(page.getByTestId("changes-row")).toHaveCount(1);
  await expect(page.getByTestId("changes-gap-cell").first().locator("a")).toHaveAttribute(
    "href",
    `#/gaps/${gapId}`,
  );

  await page.getByTestId("changes-undo").first().click();
  await expect(page.getByTestId("modal-dialog")).toContainText("Undo Gap");
  await expect(page.getByTestId("modal-dialog")).toContainText("Revert the merge commit");
  const undo = page.waitForResponse((response) =>
    response.url().includes("/api/changes/undo") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  const refreshed = page.waitForResponse((response) =>
    response.url().includes("/api/changes?") &&
    response.url().includes(`q=${encodeURIComponent(prefix)}`) &&
    response.status() === 200
  );
  await page.getByTestId("modal-ok").click();
  const undoPayload = await (await undo).json();
  expect(undoPayload.ok).toBe(true);
  expect(undoPayload.cancelled_gap).toBe(gapId);
  await refreshed;

  await expect(page.getByTestId("changes-row")).toHaveCount(0);
  await expect(page.getByTestId("changes-body")).toContainText("No changes match the current filters");
  expect(fs.existsSync(undoPath)).toBe(false);
  const gap = await jsonObject(await request.get(`/api/gaps/${gapId}`));
  expect((gap.gap as { status?: string } | undefined)?.status).toBe("cancelled");
});

test("shows Changes branch empty states for resolved and detached repositories", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const originalBranch = gitOutput(["branch", "--show-current"]).trim();
  expect(originalBranch).toBeTruthy();

  await page.goto(`/#/changes?q=${encodeURIComponent(`branch-empty-${Date.now()}`)}&limit=50`);
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();
  await expect(page.getByTestId("changes-empty-state")).toContainText(`No changes match the current filters on ${originalBranch}.`);

  try {
    git(["checkout", "--detach", "HEAD"]);
    await syncProject(request);
    await page.goto(`/#/changes?q=${encodeURIComponent(`branch-detached-${Date.now()}`)}&limit=50`);
    await expect(page.getByTestId("changes-branch-unresolved")).toContainText("No merge target branch resolved");
  } finally {
    git(["switch", originalBranch]);
    await syncProject(request);
  }
});
