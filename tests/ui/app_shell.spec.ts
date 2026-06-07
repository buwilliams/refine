import { expect, test } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

test("loads the app shell with primary navigation", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("primary-nav")).toBeVisible();
  await expect(page.getByTestId("nav-dashboard")).toBeVisible();
  await expect(page.getByTestId("nav-features")).toBeVisible();
  await expect(page.getByTestId("nav-gaps")).toBeVisible();
  await expect(page.getByTestId("nav-changes")).toBeVisible();
  await expect(page.getByTestId("nav-logs")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Dashboard", level: 2 })).toBeVisible();
});

test("reports version and attached project over public APIs", async ({ request }) => {
  const version = await jsonObject(await request.get("/system/version"));
  expect(version.product).toBe("refine");
  expect(typeof version.version).toBe("string");
  await ensureAttachedProject(request);
});

test("shows app status and persists reporter in the manage dropdown", async ({ page, request }) => {
  const project = await jsonObject(await request.get("/api/project/status"));
  expect(project.attached).toBe(true);
  const currentProject = String(project.client_repo ?? "");
  const apps = project.apps as Array<{ name?: string; path?: string }> | undefined ?? [];
  const activeApp = apps.find((app) => app.path === currentProject);
  const expectedApp = activeApp?.name || currentProject.split(/[\\/]+/).filter(Boolean).pop() || currentProject;

  await page.addInitScript(() => {
    if (!sessionStorage.getItem("manage_dropdown_test_initialized")) {
      localStorage.removeItem("refine_last_reporter");
      sessionStorage.setItem("manage_dropdown_test_initialized", "1");
    }
  });
  await page.goto("/");
  await expect(page.getByTestId("context-app-name")).toHaveText(expectedApp);
  await expect(page.getByTestId("context-reporter-name")).toHaveText("No reporter");
  await expect(page.getByTestId("context-target-app-status")).toContainText(expectedApp);
  await expect.poll(async () =>
    page.getByTestId("context-target-app-status").getAttribute("data-state")
  ).toMatch(/^(unknown|running|degraded|stopped|starting|rebuilding|stopping|failed)$/);

  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
  await expect(page.getByTestId("context-reporter-name")).toHaveText("refine-smoke");
  await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_last_reporter"))).toBe("refine-smoke");

  await page.reload();
  await expect(page.getByTestId("global-reporter")).toHaveValue("refine-smoke");
  await expect(page.getByTestId("context-reporter-name")).toHaveText("refine-smoke");

  await page.getByTestId("context-menu-toggle").click();
  await page.getByTestId("global-reporter").selectOption("");
  await expect(page.getByTestId("context-reporter-name")).toHaveText("No reporter");
  await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_last_reporter"))).toBeNull();
});

test("answers the Gaps projection query", async ({ request }) => {
  const gaps = await jsonObject(await request.get("/api/gaps"));
  expect(Array.isArray(gaps.gaps)).toBe(true);
  expect(typeof gaps.page).toBe("object");
});

test("switches dashboard node scope between current and all", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");
  await expect(page.getByTestId("dashboard-scope-current")).toHaveAttribute("aria-pressed", "true");

  const allDashboard = page.waitForResponse((response) =>
    response.url().includes("/api/dashboard?node=all") &&
    response.request().method() === "GET" &&
    response.status() === 200
  );
  await page.getByTestId("dashboard-scope-all").click();
  const payload = await (await allDashboard).json();
  expect(payload.node_filter).toBe("all");
  await expect(page).toHaveURL(/#\/\?node=all$/);
  await expect(page.getByTestId("dashboard-scope-all")).toHaveAttribute("aria-pressed", "true");

  const currentDashboard = page.waitForResponse((response) =>
    response.url().includes("/api/dashboard?node=current") &&
    response.request().method() === "GET" &&
    response.status() === 200
  );
  await page.getByTestId("dashboard-scope-current").click();
  const currentPayload = await (await currentDashboard).json();
  expect(currentPayload.node_filter).toBe("current");
  await expect(page).toHaveURL(/#\/$/);
  await expect(page.getByTestId("dashboard-scope-current")).toHaveAttribute("aria-pressed", "true");
});

test("renders dashboard workflow status cards for every Gap state", async ({ page, request }) => {
  const statuses = [
    "backlog",
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
    "review",
    "done",
    "failed",
    "cancelled",
  ];
  const agentManagedStatuses = new Set([
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
  ]);
  const createdIds: string[] = [];
  const baseDashboard = await jsonObject(await request.get("/api/dashboard?node=current"));
  const baseCounts = baseDashboard.counts as Record<string, number> | undefined ?? {};

  const createGap = async (status: string) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `Dashboard ${status} actual`,
        target: `Dashboard ${status} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    return id;
  };
  const bulkStatus = async (id: string, status: string) => {
    await jsonObject(await request.post("/api/gaps/bulk", {
      data: {
        selected_ids: [id],
        update: { status },
      },
    }));
  };
  const action = async (id: string, name: string) => {
    await jsonObject(await request.post(`/api/gaps/${encodeURIComponent(id)}/${name}`, { data: {} }));
  };

  try {
    for (const status of statuses) {
      const id = await createGap(status);
      if (status === "todo") {
        await jsonObject(await request.patch(`/api/gaps/${encodeURIComponent(id)}`, {
          data: { status: "todo" },
        }));
      } else if (status === "in-progress") {
        await action(id, "start");
      } else if (status === "qa") {
        await bulkStatus(id, "failed");
        await action(id, "retry-quality");
      } else if (status === "ready-merge") {
        await bulkStatus(id, "failed");
        await action(id, "retry-merge");
      } else if (status !== "backlog") {
        await bulkStatus(id, status);
      }
    }

    await page.goto("/");
    for (const status of statuses) {
      const card = page.getByTestId(`workflow-status-${status}`);
      await expect(card.locator(".workflow-status-count")).toHaveText(String((baseCounts[status] ?? 0) + 1));
      if (agentManagedStatuses.has(status)) {
        await expect(card).toContainText("AI");
      }
    }

    await page.getByTestId("workflow-status-qa").click();
    await expect(page).toHaveURL(/#\/gaps\?.*status=qa.*node=current/);
    await expect(page.getByTestId("gaps-row").filter({ hasText: "Dashboard qa target" })).toHaveCount(1);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
  }
});

test("persists dashboard review panels and completes review actions", async ({ page, request }) => {
  const reporter = `dashboard-review-${Date.now()}`;
  const createdIds: string[] = [];
  const reporterPayload = await jsonObject(await request.post("/api/reporters", {
    data: { name: reporter },
  }));
  const reporterId = String((reporterPayload.reporter as { id?: number | string } | undefined)?.id ?? "");
  expect(reporterId).toBeTruthy();
  const createReviewGap = async (label: string) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter,
        actual: `${label} actual`,
        target: `${label} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
    await jsonObject(await request.post("/api/gaps/bulk", {
      data: {
        selected_ids: [id],
        update: { status: "review" },
      },
    }));
    return id;
  };
  const firstId = await createReviewGap("Dashboard review first");
  const secondId = await createReviewGap("Dashboard review second");

  await page.addInitScript((selectedReporter) => {
    if (!sessionStorage.getItem("dashboard_review_test_initialized")) {
      localStorage.removeItem("refine_dashboard_panel_open:reviews-for-reporter-card");
      localStorage.removeItem("refine_dashboard_panel_open:dashboard-reporter-stats-shell");
      sessionStorage.setItem("dashboard_review_test_initialized", "1");
    }
    localStorage.setItem("refine_last_reporter", selectedReporter);
  }, reporter);

  try {
    await page.goto("/");
    await expect(page.getByTestId("global-reporter")).toHaveValue(reporter);

    const reviewPanel = page.getByTestId("dashboard-review-panel");
    await expect(reviewPanel).toHaveJSProperty("open", true);
    await expect(page.getByTestId("dashboard-review-count")).toHaveText("2");
    await expect(page.getByTestId("dashboard-review-row")).toHaveCount(2);

    await page.getByTestId("dashboard-review-summary").click();
    await expect(reviewPanel).toHaveJSProperty("open", false);

    await page.reload();
    await expect(page.getByTestId("global-reporter")).toHaveValue(reporter);
    await expect(page.getByTestId("dashboard-review-panel")).toHaveJSProperty("open", false);
    await page.getByTestId("dashboard-review-summary").click();
    await expect(page.getByTestId("dashboard-review-panel")).toHaveJSProperty("open", true);

    await page.getByTestId("dashboard-reporter-stats-summary").click();
    const statsRow = page.getByTestId("dashboard-reporter-stats-row").filter({ hasText: reporter });
    await expect(statsRow).toHaveCount(1);
    await expect(statsRow).toContainText("0.0%");

    const firstRow = page.getByTestId("dashboard-review-row").filter({ hasText: "Dashboard review first target" });
    const secondRow = page.getByTestId("dashboard-review-row").filter({ hasText: "Dashboard review second target" });
    await firstRow.getByTestId("dashboard-review-check").check();
    await expect(page.getByTestId("dashboard-review-bulk-verify")).toHaveText("Verify selected (1)");
    await expect(page.getByTestId("dashboard-review-select-all")).toHaveJSProperty("indeterminate", true);

    await page.getByTestId("dashboard-review-select-all").check();
    await expect(page.getByTestId("dashboard-review-bulk-verify")).toHaveText("Verify selected (2)");
    await expect(page.getByTestId("dashboard-review-select-all")).toBeChecked();

    await firstRow.getByTestId("dashboard-review-check").uncheck();
    await expect(page.getByTestId("dashboard-review-bulk-verify")).toHaveText("Verify selected (1)");
    await expect(page.getByTestId("dashboard-review-select-all")).toHaveJSProperty("indeterminate", true);

    await firstRow.getByTestId("dashboard-review-add-round").click();
    await expect(page.getByTestId("dashboard-add-round-modal")).toBeVisible();
    await page.getByTestId("dashboard-add-round-actual").fill("Dashboard follow-up actual");
    await page.getByTestId("dashboard-add-round-target").fill("Dashboard follow-up target");
    const roundAppended = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${encodeURIComponent(firstId)}/rounds`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("dashboard-add-round-submit").click();
    await roundAppended;
    await expect(page.getByTestId("dashboard-add-round-modal")).toHaveCount(0);
    const firstGap = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(firstId)}`));
    expect((firstGap.gap as { round_count?: number } | undefined)?.round_count).toBe(2);

    const verifiedFirst = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${encodeURIComponent(firstId)}/verify`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await firstRow.getByTestId("dashboard-review-verify").click();
    await verifiedFirst;
    await expect(page.getByTestId("dashboard-review-row")).toHaveCount(1);
    await expect(secondRow).toBeVisible();
    await expect(page.getByTestId("dashboard-review-bulk-verify")).toHaveText("Verify selected (1)");

    await page.getByTestId("dashboard-review-bulk-verify").click();
    await expect(page.getByRole("dialog")).toContainText("Verify 1 gap?");
    const verifiedSecond = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${encodeURIComponent(secondId)}/verify`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByRole("button", { name: "Verify all" }).click();
    await verifiedSecond;
    await expect(page.getByTestId("dashboard-review-count")).toHaveText("0");
    await expect(page.getByText("You're clear.")).toBeVisible();
    await expect(statsRow).toContainText("100.0%");

    await statsRow.click();
    await expect(page).toHaveURL(new RegExp(`#/gaps\\?.*reporter=${reporter}.*node=current`));
    await expect(page.getByTestId("gaps-reporter-filter")).toHaveValue(reporter);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
    await request.delete(`/api/reporters/${encodeURIComponent(reporterId)}`);
  }
});

test("opens refine issue request from nav and builds GitHub URL", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("nav-refine-issue").click();
  await expect(page.getByTestId("refine-issue-modal")).toBeVisible();

  await page.getByTestId("refine-issue-submit").click();
  await expect(page.getByText("Provide a title or description first.")).toBeVisible();

  await page.getByTestId("refine-issue-title").fill("Nav smoke request");
  await page.getByTestId("refine-issue-description").fill("Generated from the Refine nav issue button.");
  await page.evaluate(() => {
    const testWindow = window as unknown as { __refineOpenedUrl: string; open: (url?: string | URL) => unknown };
    testWindow.__refineOpenedUrl = "";
    window.open = (url) => {
      testWindow.__refineOpenedUrl = String(url || "");
      return {} as Window;
    };
  });
  await page.getByTestId("refine-issue-submit").click();
  await expect(page.getByTestId("refine-issue-modal")).toHaveCount(0);
  const openedUrl = await page.evaluate(() => (window as unknown as { __refineOpenedUrl: string }).__refineOpenedUrl);
  const issueUrl = new URL(String(openedUrl));
  expect(`${issueUrl.origin}${issueUrl.pathname}`).toBe("https://github.com/buwilliams/refine/issues/new");
  expect(issueUrl.searchParams.get("title")).toBe("Nav smoke request");
  expect(issueUrl.searchParams.get("body")).toBe("Generated from the Refine nav issue button.");
});
