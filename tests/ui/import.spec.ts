import { Buffer } from "node:buffer";
import { expect, test } from "@playwright/test";
import { ensureAttachedProject, jsonObject } from "./helpers";

async function selectReporter(page) {
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
}

function csvLine(cells: string[]): string {
  return cells.map((cell) => {
    const value = String(cell);
    return /[",\n\r]/.test(value) ? `"${value.replace(/"/g, "\"\"")}"` : value;
  }).join(",");
}

test("extracts and saves AI Import drafts through Smoke AI", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const createdGapIds: string[] = [];

  try {
    await page.goto("/");
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await expect(page.getByTestId("import-tab-ai")).toHaveAttribute("aria-selected", "true");
    await page.getByTestId("import-text").fill("Please import these deterministic Smoke AI issues.");

    const extracted = page.waitForResponse((response) =>
      response.url().includes("/api/import/extract") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    const extractPayload = await (await extracted).json();
    expect(extractPayload.provider).toBe("smoke-ai");
    expect(extractPayload.source).toBe("provider");
    expect(String(extractPayload.drafts?.[0]?.name ?? "")).toContain("refine-smoke imported gap one");

    await expect(page.getByTestId("import-draft-name").first()).toHaveValue(/refine-smoke imported gap one/);
    await expect(page.getByTestId("import-draft-name").nth(1)).toHaveValue(/refine-smoke imported gap two/);
    await expect(page.getByTestId("import-persist")).toBeEnabled();

    let completedImportResult: {
      count?: number;
      gaps?: Array<{ id?: string; name?: string }>;
    } | null = null;
    const importCompleted = page.waitForResponse(async (response) => {
      if (!/\/api\/jobs\/[^/]+$/.test(new URL(response.url()).pathname)) return false;
      if (response.request().method() !== "GET" || response.status() !== 200) return false;
      const payload = await response.json();
      if (payload.job?.status === "complete") {
        completedImportResult = payload.job.result || null;
        return true;
      }
      return false;
    });
    await page.getByTestId("import-persist").click();
    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
    await importCompleted;

    for (const gap of completedImportResult?.gaps ?? []) {
      if (gap?.name?.startsWith("refine-smoke imported gap")) {
        createdGapIds.push(String(gap.id));
      }
    }
    expect(createdGapIds.length).toBeGreaterThanOrEqual(2);
  } finally {
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});

test("reviews CSV import drafts with pagination and bulk duplicate decisions", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const createdGapIds = new Set<string>();
  const suffix = Date.now();
  const prefix = `import-review-${suffix}`;
  const duplicateName = `${prefix} duplicate`;
  const duplicateActual = `${prefix} duplicate actual`;
  const duplicateTarget = `${prefix} duplicate target`;
  const rows = [
    csvLine(["name", "actual", "target", "reporter", "priority"]),
    csvLine([duplicateName, duplicateActual, duplicateTarget, "refine-smoke", "high"]),
  ];
  for (let i = 2; i <= 30; i += 1) {
    const padded = String(i).padStart(2, "0");
    rows.push(csvLine([
      `${prefix} imported ${padded}`,
      `${prefix} actual ${padded}`,
      `${prefix} target ${padded}`,
      "refine-smoke",
      i % 3 === 0 ? "high" : i % 2 === 0 ? "medium" : "low",
    ]));
  }

  const cleanupMatchingGaps = async () => {
    try {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=100&node=current&q=${encodeURIComponent(prefix)}`));
      for (const gap of (gaps.gaps as Array<{ id?: string }> | undefined) ?? []) {
        if (gap.id) createdGapIds.add(String(gap.id));
      }
    } catch {
      // Best-effort cleanup; individual deletes below still run for known ids.
    }
  };

  try {
    const originalPayload = await jsonObject(await request.post("/api/gaps", {
      data: {
        name: `${duplicateName} ${duplicateActual} ${duplicateTarget}`,
        reporter: "refine-smoke",
        actual: duplicateActual,
        target: duplicateTarget,
        priority: "low",
      },
    }));
    const originalId = String((originalPayload.gap as { id?: string } | undefined)?.id ?? "");
    expect(originalId).toBeTruthy();
    createdGapIds.add(originalId);

    await page.goto("/");
    await page.evaluate(() => localStorage.removeItem("refine_import_session_v1"));
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await page.getByTestId("import-tab-csv").click();
    await expect(page.getByTestId("import-tab-csv")).toHaveAttribute("aria-selected", "true");
    await page.getByTestId("import-csv-text").fill(rows.join("\n"));

    const parsed = page.waitForResponse((response) =>
      response.url().includes("/api/import/csv/parse") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const deduped = page.waitForResponse((response) =>
      response.url().includes("/api/import/dedup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    const parsePayload = await (await parsed).json();
    expect(parsePayload.drafts).toHaveLength(30);
    const dedupPayload = await (await deduped).json();
    expect(dedupPayload.matches).toHaveLength(1);

    await expect(page.getByTestId("import-review-shell")).toBeVisible();
    await expect(page.getByTestId("import-review-range")).toHaveText("Showing 1-25 of 30 of 30");
    await expect(page.getByTestId("import-page-label")).toHaveText("Page 1 of 2");
    await expect(page.getByTestId("import-draft-row")).toHaveCount(25);
    await expect(page.getByTestId("import-duplicate-count")).toHaveText("1 duplicate");
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Needs duplicate resolution");
    await expect(page.getByTestId("import-persist")).toBeEnabled();

    await page.getByTestId("import-page-next").click();
    await expect(page.getByTestId("import-page-label")).toHaveText("Page 2 of 2");
    await expect(page.getByTestId("import-draft-row")).toHaveCount(5);
    await expect(page.getByTestId("import-draft-name").last()).toHaveValue(`${prefix} imported 30`);

    await page.getByTestId("import-page-prev").click();
    await expect(page.getByTestId("import-page-label")).toHaveText("Page 1 of 2");
    await page.getByTestId("import-select-page").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("25 selected");
    await page.getByTestId("import-select-all").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("30 selected");

    await page.getByTestId("import-select-duplicates").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("1 selected");
    await page.getByTestId("import-update-field").selectOption("target");
    await page.getByTestId("import-update-originals").click();
    await expect(page.getByTestId("import-selected-count")).toHaveText("0 selected");
    await expect(page.getByTestId("import-duplicate-decision")).toHaveText("Will update original target");

    await page.getByTestId("import-select-duplicates").click();
    await page.getByTestId("import-dismiss-duplicates").click();
    await expect(page.getByTestId("import-duplicate-decision")).toHaveCount(0);
    await expect(page.getByTestId("import-persist")).toHaveText("Save (29) gaps");
    await expect(page.getByTestId("import-review-range")).toHaveText("Showing 1-25 of 29 of 29");

    const persisted = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const persistStartPayload = await (await persisted).json();
    const jobId = String(persistStartPayload.job?.id ?? "");
    expect(jobId).toBeTruthy();
    await expect.poll(async () => {
      const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
      const job = payload.job as { status?: string; result?: { count?: number } };
      return {
        status: job.status,
        count: job.result?.count,
      };
    }, { timeout: 30_000 }).toMatchObject({
      status: "complete",
      count: 29,
    });
    const finalJobPayload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
    const jobPayload = finalJobPayload.job as { result?: { gaps?: Array<{ id?: string }> } };
    for (const gap of jobPayload.result?.gaps ?? []) {
      if (gap.id) createdGapIds.add(String(gap.id));
    }
    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
  } finally {
    await cleanupMatchingGaps();
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});

test("hides and recovers a background CSV import save", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const createdGapIds = new Set<string>();
  const suffix = Date.now();
  const prefix = `import-save-${suffix}`;
  const rows = [csvLine(["name", "actual", "target", "reporter", "priority"])];
  for (let i = 1; i <= 60; i += 1) {
    const padded = String(i).padStart(2, "0");
    rows.push(csvLine([
      `${prefix} imported ${padded}`,
      `${prefix} actual ${padded}`,
      `${prefix} target ${padded}`,
      "refine-smoke",
      i % 2 === 0 ? "medium" : "low",
    ]));
  }

  const cleanupMatchingGaps = async () => {
    try {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=100&node=current&q=${encodeURIComponent(prefix)}`));
      for (const gap of (gaps.gaps as Array<{ id?: string }> | undefined) ?? []) {
        if (gap.id) createdGapIds.add(String(gap.id));
      }
    } catch {
      // Best-effort cleanup; individual deletes below still run for known ids.
    }
  };

  try {
    await page.goto("/");
    await page.evaluate(() => localStorage.removeItem("refine_import_session_v1"));
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await page.getByTestId("import-tab-csv").click();
    await page.getByTestId("import-csv-text").fill(rows.join("\n"));

    const parsed = page.waitForResponse((response) =>
      response.url().includes("/api/import/csv/parse") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const deduped = page.waitForResponse((response) =>
      response.url().includes("/api/import/dedup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    await parsed;
    await deduped;
    await expect(page.getByTestId("import-persist")).toHaveText("Save (60) gaps");

    const persisted = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const startPayload = await (await persisted).json();
    const jobId = String(startPayload.job?.id ?? "");
    expect(jobId).toBeTruthy();

    await expect(page.getByTestId("import-save-hide")).toBeVisible();
    await page.getByTestId("import-save-hide").click();
    await expect(page.getByTestId("import-modal")).toHaveCount(0);
    const savedSession = await page.evaluate(() => localStorage.getItem("refine_import_session_v1"));
    expect(savedSession).toContain(jobId);

    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();
    await expect.poll(async () => {
      const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
      const job = payload.job as { status?: string; result?: { count?: number } };
      return {
        status: job.status,
        count: job.result?.count,
      };
    }, { timeout: 30_000 }).toMatchObject({
      status: "complete",
      count: 60,
    });
    const finalJobPayload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
    const jobPayload = finalJobPayload.job as { result?: { gaps?: Array<{ id?: string }> } };
    for (const gap of jobPayload.result?.gaps ?? []) {
      if (gap.id) createdGapIds.add(String(gap.id));
    }
    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
    await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_import_session_v1"))).toBeNull();
  } finally {
    await cleanupMatchingGaps();
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});

test("cancels a background CSV import save and rolls back created gaps", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const createdGapIds = new Set<string>();
  const suffix = Date.now();
  const prefix = `import-cancel-${suffix}`;
  const rows = [csvLine(["name", "actual", "target", "reporter", "priority"])];
  for (let i = 1; i <= 240; i += 1) {
    const padded = String(i).padStart(3, "0");
    rows.push(csvLine([
      `${prefix} imported ${padded}`,
      `${prefix} actual ${padded}`,
      `${prefix} target ${padded}`,
      "refine-smoke",
      i % 2 === 0 ? "medium" : "low",
    ]));
  }

  const cleanupMatchingGaps = async () => {
    try {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=1000&node=current&q=${encodeURIComponent(prefix)}`));
      for (const gap of (gaps.gaps as Array<{ id?: string }> | undefined) ?? []) {
        if (gap.id) createdGapIds.add(String(gap.id));
      }
    } catch {
      // Best-effort cleanup; individual deletes below still run for known ids.
    }
  };

  try {
    await page.goto("/");
    await page.evaluate(() => localStorage.removeItem("refine_import_session_v1"));
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await page.getByTestId("import-tab-csv").click();
    await page.getByTestId("import-csv-text").fill(rows.join("\n"));

    const parsed = page.waitForResponse((response) =>
      response.url().includes("/api/import/csv/parse") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const deduped = page.waitForResponse((response) =>
      response.url().includes("/api/import/dedup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    await parsed;
    await deduped;
    await expect(page.getByTestId("import-persist")).toHaveText("Save (240) gaps");

    const persisted = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const startPayload = await (await persisted).json();
    const jobId = String(startPayload.job?.id ?? "");
    expect(jobId).toBeTruthy();

    const cancelled = page.waitForResponse((response) =>
      response.url().includes(`/api/jobs/${jobId}/cancel`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await expect(page.getByTestId("import-save-cancel")).toBeVisible();
    await page.getByTestId("import-save-cancel").click();
    await page.getByTestId("modal-ok").click();
    const cancelPayload = await (await cancelled).json();
    expect(cancelPayload.job?.status).toBe("cancelled");

    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
    await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_import_session_v1"))).toBeNull();
    await expect.poll(async () => {
      const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
      const job = payload.job as { status?: string; progress?: { message?: string; completed?: number; total?: number } };
      return {
        status: job.status,
        message: job.progress?.message,
        completed: job.progress?.completed,
        total: job.progress?.total,
      };
    }, { timeout: 30_000 }).toMatchObject({
      status: "cancelled",
      message: "Import cancelled",
      completed: 0,
      total: 240,
    });
    await expect.poll(async () => {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=1000&node=current&q=${encodeURIComponent(prefix)}`));
      return (gaps.page as { total?: number } | undefined)?.total ?? -1;
    }, { timeout: 30_000 }).toBe(0);
  } finally {
    await cleanupMatchingGaps();
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});

test("recovers failed import drafts and retries after correcting the review", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const createdGapIds = new Set<string>();
  let featureId = "";
  const suffix = Date.now();
  const prefix = `import-retry-${suffix}`;
  const rows = [
    csvLine(["name", "actual", "target", "reporter", "priority"]),
    csvLine([`${prefix} stale feature`, `${prefix} actual`, `${prefix} target`, "refine-smoke", "medium"]),
  ];

  const cleanupMatchingGaps = async () => {
    try {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=1000&node=current&q=${encodeURIComponent(prefix)}`));
      for (const gap of (gaps.gaps as Array<{ id?: string }> | undefined) ?? []) {
        if (gap.id) createdGapIds.add(String(gap.id));
      }
    } catch {
      // Best-effort cleanup; individual deletes below still run for known ids.
    }
  };

  try {
    const featurePayload = await jsonObject(await request.post("/api/features", {
      data: {
        name: `${prefix} destination`,
        description: `${prefix} stale destination`,
        reporter: "refine-smoke",
      },
    }));
    featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
    expect(featureId).toBeTruthy();

    await page.goto("/");
    await page.evaluate(() => localStorage.removeItem("refine_import_session_v1"));
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await page.getByTestId("import-tab-csv").click();
    await page.getByTestId("import-csv-text").fill(rows.join("\n"));

    const parsed = page.waitForResponse((response) =>
      response.url().includes("/api/import/csv/parse") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const deduped = page.waitForResponse((response) =>
      response.url().includes("/api/import/dedup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    await parsed;
    await deduped;
    await expect(page.getByTestId("import-persist")).toHaveText("Save (1) gap");

    await page.getByTestId("import-feature-mode-existing").check();
    await expect(page.getByTestId("import-feature-existing").locator(`option[value="${featureId}"]`)).toHaveCount(1);
    await page.getByTestId("import-feature-existing").selectOption(featureId);
    await expect(page.getByTestId("import-persist")).toHaveText("Save (1) gap to Feature");

    const deletedFeature = await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
    expect(deletedFeature.status()).toBe(200);
    await expect.poll(async () => (
      await request.get(`/api/features/${encodeURIComponent(featureId)}`)
    ).status()).toBe(404);
    featureId = "";

    const failedPersist = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const failedPersistPayload = await (await failedPersist).json();
    const failedJobId = String(failedPersistPayload.job?.id ?? "");
    expect(failedJobId).toBeTruthy();
    await expect(page.getByText("Failed drafts (1)", { exact: false })).toBeVisible({ timeout: 30_000 });
    await expect(page.getByTestId("import-draft-error")).toContainText("was not found");
    await expect.poll(async () => page.evaluate(() => JSON.parse(localStorage.getItem("refine_import_session_v1") || "{}").phase)).toBe("failed");

    await page.reload();
    await expect(page.getByTestId("import-modal")).toBeVisible();
    await expect(page.getByText("Failed drafts (1)", { exact: false })).toBeVisible();
    await expect(page.getByTestId("import-draft-error")).toContainText("was not found");

    await page.getByTestId("import-feature-mode-standalone").check();
    await page.getByTestId("import-draft-name").fill(`${prefix} retried standalone`);
    await expect(page.getByTestId("import-draft-error")).toHaveCount(0);
    await expect(page.getByTestId("import-persist")).toHaveText("Save (1) gap");

    const retried = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const retryPayload = await (await retried).json();
    const retryJobId = String(retryPayload.job?.id ?? "");
    expect(retryJobId).toBeTruthy();
    let retriedGapId = "";
    await expect.poll(async () => {
      const payload = await jsonObject(await request.get(`/api/jobs/${retryJobId}`));
      const job = payload.job as { status?: string; result?: { count?: number; gaps?: Array<{ id?: string }> } };
      for (const gap of job.result?.gaps ?? []) {
        if (gap.id) {
          retriedGapId = String(gap.id);
          createdGapIds.add(retriedGapId);
        }
      }
      return {
        status: job.status,
        count: job.result?.count,
      };
    }, { timeout: 30_000 }).toMatchObject({
      status: "complete",
      count: 1,
    });
    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
    await expect.poll(async () => page.evaluate(() => localStorage.getItem("refine_import_session_v1"))).toBeNull();
    expect(retriedGapId).toBeTruthy();
    await expect.poll(async () => {
      const response = await request.get(`/api/gaps/${encodeURIComponent(retriedGapId)}`);
      return response.status();
    }).toBe(200);
  } finally {
    await cleanupMatchingGaps();
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
    if (featureId) {
      await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
    }
  }
});

test("uploads and persists CSV import drafts", async ({ page, request }) => {
  test.setTimeout(120_000);
  await ensureAttachedProject(request);
  const createdGapIds = new Set<string>();
  const suffix = Date.now();
  const prefix = `import-upload-${suffix}`;
  const rows = [
    csvLine(["name", "actual", "target", "reporter", "priority"]),
    csvLine([`${prefix} one`, `${prefix} actual one`, `${prefix} target one`, "refine-smoke", "low"]),
    csvLine([`${prefix} two`, `${prefix} actual two`, `${prefix} target two`, "refine-smoke", "medium"]),
    csvLine([`${prefix} three`, `${prefix} actual three`, `${prefix} target three`, "refine-smoke", "high"]),
  ];

  const cleanupMatchingGaps = async () => {
    try {
      const gaps = await jsonObject(await request.get(`/api/gaps?limit=100&node=current&q=${encodeURIComponent(prefix)}`));
      for (const gap of (gaps.gaps as Array<{ id?: string }> | undefined) ?? []) {
        if (gap.id) createdGapIds.add(String(gap.id));
      }
    } catch {
      // Best-effort cleanup; individual deletes below still run for known ids.
    }
  };

  try {
    await page.goto("/");
    await page.evaluate(() => localStorage.removeItem("refine_import_session_v1"));
    await selectReporter(page);
    await page.getByTestId("create-menu-toggle").click();
    await page.getByTestId("nav-import-gaps").click();

    await expect(page.getByTestId("import-modal")).toBeVisible();
    await page.getByTestId("import-tab-upload").click();
    await expect(page.getByTestId("import-tab-upload")).toHaveAttribute("aria-selected", "true");
    await page.getByTestId("import-csv-file").setInputFiles({
      name: "refine-import-upload.csv",
      mimeType: "text/csv",
      buffer: Buffer.from(rows.join("\n")),
    });
    await expect(page.getByTestId("import-csv-file-name")).toHaveText("refine-import-upload.csv");

    const parsed = page.waitForResponse((response) =>
      response.url().includes("/api/import/csv/parse") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const deduped = page.waitForResponse((response) =>
      response.url().includes("/api/import/dedup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("import-extract").click();
    const parsePayload = await (await parsed).json();
    expect(parsePayload.drafts).toHaveLength(3);
    await deduped;
    await expect(page.getByTestId("import-persist")).toHaveText("Save (3) gaps");

    const persisted = page.waitForResponse((response) =>
      response.url().includes("/api/import/persist") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await page.getByTestId("import-persist").click();
    const startPayload = await (await persisted).json();
    const jobId = String(startPayload.job?.id ?? "");
    expect(jobId).toBeTruthy();
    await expect.poll(async () => {
      const payload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
      const job = payload.job as { status?: string; result?: { count?: number } };
      return {
        status: job.status,
        count: job.result?.count,
      };
    }, { timeout: 30_000 }).toMatchObject({
      status: "complete",
      count: 3,
    });
    const finalJobPayload = await jsonObject(await request.get(`/api/jobs/${jobId}`));
    const jobPayload = finalJobPayload.job as { result?: { gaps?: Array<{ id?: string }> } };
    for (const gap of jobPayload.result?.gaps ?? []) {
      if (gap.id) createdGapIds.add(String(gap.id));
    }
    await expect(page.getByTestId("import-modal")).toHaveCount(0, { timeout: 30_000 });
  } finally {
    await cleanupMatchingGaps();
    for (const gapId of createdGapIds) {
      await request.delete(`/api/gaps/${gapId}`);
    }
  }
});
