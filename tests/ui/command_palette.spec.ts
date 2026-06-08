import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { expect, test, type Page } from "@playwright/test";
import { ensureAttachedProject, jsonObject, waitForJobResult } from "./helpers";

function testAppRoot(): string {
  return process.env.REFINE_TEST_APP_ROOT ||
    path.join(process.cwd(), "target/refine-integration/apps/rust-test-app");
}

async function runPaletteCommand(page: Page, input: string, options: { commandId?: string } = {}) {
  await page.keyboard.press(process.platform === "darwin" ? "Meta+K" : "Control+K");
  if (await page.getByTestId("command-palette").count() === 0) {
    await page.getByTestId("command-palette-button").click();
  }
  await expect(page.getByTestId("command-palette")).toBeVisible();
  await page.getByTestId("command-palette-input").fill(input);
  if (options.commandId) {
    await page
      .getByTestId("command-palette-row")
      .and(page.locator(`[data-command-id="${options.commandId}"]`))
      .click();
  } else {
    await page.keyboard.press("Enter");
  }
  await expect(page.getByTestId("command-palette")).toHaveCount(0);
}

async function selectReporter(page: Page) {
  await page.getByTestId("context-menu-toggle").click();
  await expect(page.getByTestId("global-reporter").locator("option", { hasText: "refine-smoke" })).toHaveCount(1);
  await page.getByTestId("global-reporter").selectOption("refine-smoke");
}

test("opens, searches, navigates, executes, and closes from the command palette", async ({ page }) => {
  await page.goto("/");

  await page.getByTestId("command-palette-button").click();
  await expect(page.getByTestId("command-palette")).toBeVisible();
  await expect(page.getByTestId("command-palette-input")).toBeFocused();
  await expect(page.getByTestId("command-palette-row")).toHaveCount(12);

  await page.getByTestId("command-palette-input").fill("logs");
  await expect(page.getByTestId("command-palette-row").first()).toContainText("Logs");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("command-palette")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Logs", level: 2 })).toBeVisible();

  await page.keyboard.press(process.platform === "darwin" ? "Meta+K" : "Control+K");
  await expect(page.getByTestId("command-palette")).toBeVisible();
  await page.getByTestId("command-palette-input").fill("zzzz-no-command");
  await expect(page.getByTestId("command-palette-empty")).toHaveText("No commands found.");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("command-palette")).toHaveCount(0);

  await page.getByTestId("command-palette-button").click();
  await page.getByTestId("command-palette-input").fill("nav");
  await page.keyboard.press("ArrowDown");
  await expect(page.getByTestId("command-palette-row").nth(1)).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("ArrowUp");
  await expect(page.getByTestId("command-palette-row").first()).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("command-palette")).toHaveCount(0);
});

test("shows disabled command palette rows", async ({ page }) => {
  await page.goto("/");
  await page.evaluate(() => {
    const commands = (window as unknown as { RefineCommands: { register: (command: Record<string, unknown>) => void } }).RefineCommands;
    commands.register({
      id: "test.disabled",
      title: "Disabled smoke command",
      group: "Test",
      aliases: ["disabled-smoke"],
      enabled: () => false,
      run: () => {},
    });
    commands.register({
      id: "test.parse_error",
      title: "Parse error smoke command",
      group: "Test",
      aliases: ["parse-error-smoke"],
      parse: () => {
        throw new Error("Smoke parse error");
      },
      run: () => {},
    });
  });

  await page.keyboard.press(process.platform === "darwin" ? "Meta+K" : "Control+K");
  await expect(page.getByTestId("command-palette")).toBeVisible();
  await page.getByTestId("command-palette-input").fill("disabled-smoke");
  const row = page.getByTestId("command-palette-row").first();
  await expect(row).toContainText("Disabled smoke command");
  await expect(row).toBeDisabled();

  await page.getByTestId("command-palette-input").fill("parse-error-smoke");
  const parseRow = page.getByTestId("command-palette-row").first();
  await expect(parseRow).toContainText("Parse error smoke command");
  await expect(parseRow).toBeDisabled();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("command-palette")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("command-palette")).toHaveCount(0);
});

test("opens create modals and re-checks runtime auth from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");
  await selectReporter(page);

  await runPaletteCommand(page, "new-gap");
  await expect(page.getByTestId("new-gap-modal")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("new-gap-modal")).toHaveCount(0);

  await runPaletteCommand(page, "import-gaps");
  await expect(page.getByTestId("import-modal")).toBeVisible();
  await expect(page.getByTestId("import-tab-ai")).toHaveAttribute("aria-selected", "true");
  await page.getByTestId("import-cancel").click();
  await expect(page.getByTestId("import-modal")).toHaveCount(0);

  const rechecked = page.waitForResponse((response) =>
    response.url().includes("/api/settings/recheck-auth") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await runPaletteCommand(page, "recheck-auth");
  const payload = await (await rechecked).json();
  expect(payload.ok).toBe(true);
  await expect(page.getByText("Auth OK")).toBeVisible();
});

test("clears Changes and Logs filters from the command palette", async ({ page }) => {
  await page.goto("/#/logs?severity=error&q=smoke");
  await expect(page.getByRole("heading", { name: "Logs", level: 2 })).toBeVisible();
  await runPaletteCommand(page, "clear-logs");
  await expect(page).toHaveURL(/#\/logs$/);

  await page.goto("/#/changes?kind=merge&q=smoke");
  await expect(page.getByRole("heading", { name: "Changes", level: 2 })).toBeVisible();
  await runPaletteCommand(page, "clear-changes");
  await expect(page).toHaveURL(/#\/changes$/);
});

test("rebuilds the projection cache from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const created = await jsonObject(await request.post("/api/gaps", {
    data: {
      reporter: "refine-smoke",
      actual: `Palette cache rebuild actual ${suffix}`,
      target: `Palette cache rebuild target ${suffix}`,
      priority: "low",
    },
  }));
  const gapId = String((created.gap as { id?: string } | undefined)?.id ?? "");
  expect(gapId).toBeTruthy();

  try {
    await page.goto("/");
    await runPaletteCommand(page, "rebuild-cache", { commandId: "system.cache.rebuild" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Rebuild projection cache");
    const rebuilt = page.waitForResponse((response) =>
      response.url().includes("/api/cache/rebuild") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    const payload = await (await rebuilt).json();
    expect(payload.ok).toBe(true);
    expect(Number(payload.gaps ?? 0)).toBeGreaterThanOrEqual(1);
    expect(String(payload.cache ?? "")).toContain("target/refine-integration/run");
    await expect(page.getByTestId("toast").filter({ hasText: "Projection cache rebuilt" })).toBeVisible();
  } finally {
    await request.delete(`/api/gaps/${gapId}`);
  }
});

test("cleans up activity logs from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const prefix = `Palette cleanup ${Date.now()}`;
  await jsonObject(await request.post("/api/activity/ui-error", {
    data: {
      message: `${prefix} seeded activity`,
      marker: prefix,
      source: "command-palette-cleanup.spec",
    },
  }));

  await page.goto(`/#/logs?q=${encodeURIComponent(prefix)}`);
  await expect(page.getByTestId("logs-row").filter({ hasText: prefix })).toHaveCount(1);
  await runPaletteCommand(page, "cleanup-logs 0", { commandId: "system.logs.cleanup" });
  await expect(page.getByTestId("modal-dialog")).toContainText("Clean up old logs");
  await expect(page.getByTestId("modal-dialog")).toContainText("Delete ALL activity log entries");
  const cleaned = page.waitForResponse((response) =>
    response.url().includes("/api/activity/cleanup") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("modal-ok").click();
  const payload = await (await cleaned).json();
  expect(payload.ok).toBe(true);
  expect(payload.cleared).toBe(true);
  expect(Number(payload.deleted ?? 0)).toBeGreaterThanOrEqual(1);
  await expect(page.getByTestId("toast").filter({ hasText: "Deleted" })).toBeVisible();
});

test("hard resets the target worktree from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const appRoot = testAppRoot();
  const trackedPath = path.join(appRoot, "app.py");
  const untrackedPath = path.join(appRoot, `palette-hard-reset-${Date.now()}.tmp`);
  const original = fs.readFileSync(trackedPath, "utf-8");
  fs.writeFileSync(trackedPath, "def health() -> str:\n    return \"dirty\"\n");
  fs.writeFileSync(untrackedPath, "remove me\n");

  try {
    await page.goto("/");
    await runPaletteCommand(page, "hard-reset", { commandId: "system.worktree.hard_reset" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Hard reset worktree");
    const reset = page.waitForResponse((response) =>
      response.url().includes("/api/runner-workers/merger/hard-reset-worktree") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    const payload = await (await reset).json();
    expect(payload.ok).toBe(true);
    expect(String(payload.message ?? "")).toContain("HEAD is now at");
    expect(fs.readFileSync(trackedPath, "utf-8")).toBe(original);
    expect(fs.existsSync(untrackedPath)).toBe(false);
    await expect(page.getByTestId("toast").filter({ hasText: "HEAD is now at" })).toBeVisible();
  } finally {
    if (fs.existsSync(trackedPath) && fs.readFileSync(trackedPath, "utf-8") !== original) {
      fs.writeFileSync(trackedPath, original);
    }
    fs.rmSync(untrackedPath, { force: true });
  }
});

test("pauses and unpauses agents from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await jsonObject(await request.post("/api/processes/agents", { data: { paused: false } }));
  await page.goto("/");

  const paused = page.waitForResponse((response) =>
    response.url().includes("/api/processes/agents") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await runPaletteCommand(page, "pause-agents", { commandId: "system.agents.pause_toggle" });
  const pausedPayload = await (await paused).json();
  expect(pausedPayload.agents_paused).toBe(true);

  const unpaused = page.waitForResponse((response) =>
    response.url().includes("/api/processes/agents") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await runPaletteCommand(page, "unpause-agents", { commandId: "system.agents.pause_toggle" });
  const unpausedPayload = await (await unpaused).json();
  expect(unpausedPayload.agents_paused).toBe(false);
});

test("runs Gaps filter and bulk modal commands from the command palette", async ({ page, request }) => {
  const createdIds: string[] = [];
  let featureId = "";
  let nodeId = "";
  const suffix = Date.now();
  const prefix = `Palette bulk ${suffix}`;
  const createGap = async (index: number) => {
    const payload = await jsonObject(await request.post("/api/gaps", {
      data: {
        reporter: "refine-smoke",
        actual: `${prefix} ${index} actual`,
        target: `${prefix} ${index} target`,
        priority: "low",
      },
    }));
    const id = String((payload.gap as { id?: string } | undefined)?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.push(id);
  };
  await Promise.all([createGap(1), createGap(2)]);
  const featurePayload = await jsonObject(await request.post("/api/features", {
    data: {
      name: `Palette bulk feature ${suffix}`,
      description: "Seeded for command-palette bulk coverage",
      reporter: "refine-smoke",
    },
  }));
  featureId = String((featurePayload.feature as { id?: string } | undefined)?.id ?? "");
  expect(featureId).toBeTruthy();
  nodeId = `palette-bulk-${suffix}`;
  await jsonObject(await request.post("/api/nodes", { data: { id: nodeId } }));

  try {
    await page.goto(`/#/gaps?q=${encodeURIComponent(prefix)}&node=current&limit=50`);
    await expect(page.getByTestId("gaps-row")).toHaveCount(2);

    await runPaletteCommand(page, "clear-gaps", { commandId: "gaps.clear_filters" });
    await expect(page).toHaveURL(/#\/gaps$/);

    await page.goto(`/#/gaps?q=${encodeURIComponent(prefix)}&node=current&limit=50`);
    await expect(page.getByTestId("gaps-row")).toHaveCount(2);
    await runPaletteCommand(page, "select-page", { commandId: "gaps.select_page" });
    if (!(await page.getByTestId("gaps-filter-shell").evaluate((el) => (el as HTMLDetailsElement).open))) {
      await page.getByTestId("gaps-filter-summary").click();
    }
    await expect(page.getByTestId("gaps-row-select")).toHaveCount(2);
    await expect(page.getByTestId("gaps-row-select").first()).toBeChecked();

    await runPaletteCommand(page, "bulk-status", { commandId: "gaps.bulk.status" });
    await expect(page.getByTestId("bulk-value-status")).toBeVisible();
    await page.getByTestId("bulk-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);

    await runPaletteCommand(page, "bulk-priority", { commandId: "gaps.bulk.priority" });
    await expect(page.getByTestId("bulk-value-priority")).toBeVisible();
    await page.getByTestId("bulk-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);

    await runPaletteCommand(page, "bulk-reporter", { commandId: "gaps.bulk.reporter" });
    await expect(page.getByTestId("bulk-value-reporter")).toBeVisible();
    await page.getByTestId("bulk-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);

    await runPaletteCommand(page, "bulk-feature", { commandId: "gaps.bulk.feature" });
    await expect(page.getByTestId("bulk-assign-feature-value")).toHaveValue(featureId);
    await page.getByTestId("bulk-feature-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);

    await runPaletteCommand(page, "bulk-node", { commandId: "gaps.bulk.transfer_node" });
    await expect(page.getByTestId("bulk-transfer-node-value").locator("option", { hasText: nodeId })).toHaveCount(1);
    await page.getByTestId("bulk-transfer-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);

    await runPaletteCommand(page, "bulk-delete", { commandId: "gaps.bulk.delete" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete Gaps");
    await page.getByTestId("modal-cancel").click();
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);
  } finally {
    for (const id of createdIds.reverse()) {
      await request.delete(`/api/gaps/${encodeURIComponent(id)}`);
    }
    if (featureId) await request.delete(`/api/features/${encodeURIComponent(featureId)}`);
    if (nodeId) {
      await request.patch(`/api/nodes/${encodeURIComponent(nodeId)}`, {
        data: { archived: true },
      });
    }
  }
});

test("navigates primary and settings routes from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");

  const routeCommands: Array<[string, string, RegExp, string]> = [
    ["Dashboard", "nav.dashboard", /#\/$/, "Dashboard"],
    ["Features", "nav.features", /#\/features$/, "Features"],
    ["Gaps", "nav.gaps", /#\/gaps$/, "Gaps"],
    ["Changes", "nav.changes", /#\/changes$/, "Changes"],
    ["Logs", "nav.logs", /#\/logs$/, "Logs"],
    ["Node: Application", "nav.node.application", /#\/node\/application$/, "Node"],
    ["Node: Reporters", "nav.node.reporters", /#\/node\/reporters$/, "Node"],
    ["Node: Processes", "nav.node.processes", /#\/node\/processes$/, "Node"],
    ["Node: Performance", "nav.node.performance", /#\/node\/performance$/, "Node"],
    ["Node: Target App Config", "nav.node.target-app", /#\/node\/target-app$/, "Node"],
    ["Node: Refine Runtime Config", "nav.node.runtime", /#\/node\/runtime$/, "Node"],
    ["Governance: Governance", "nav.project.governance", /#\/project\/governance$/, "Governance"],
    ["Governance: Quality", "nav.project.quality", /#\/project\/quality$/, "Governance"],
    ["Governance: Guidance", "nav.project.guidance", /#\/project\/guidance$/, "Governance"],
  ];

  for (const [command, commandId, url, heading] of routeCommands) {
    await runPaletteCommand(page, command, { commandId });
    await expect(page).toHaveURL(url);
    await expect(page.getByRole("heading", { name: heading, level: 2 })).toBeVisible();
  }
});

test("runs settings copy-from-node commands from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const nodeId = `palette-copy-${Date.now()}`;
  const created = await jsonObject(await request.post("/api/nodes", {
    data: { id: nodeId },
  }));
  expect((created.nodes as Array<{ id?: string }> | undefined) ?? [])
    .toEqual(expect.arrayContaining([expect.objectContaining({ id: nodeId })]));

  try {
    await page.goto("/#/node/target-app");
    for (const [input, commandId, title, section] of [
      ["copy-application-settings", "settings.application.copy_node", "Copy application settings", "application"],
      ["copy-runtime-settings", "settings.runtime.copy_node", "Copy runtime settings", "runtime"],
    ] as const) {
      const copied = page.waitForResponse((response) =>
        response.url().includes("/api/nodes/copy-settings") &&
        response.request().method() === "POST" &&
        response.status() === 200
      );
      await runPaletteCommand(page, input, { commandId });
      await expect(page.getByTestId("modal-dialog")).toContainText(title);
      await page.getByTestId("copy-settings-source-node").selectOption(nodeId);
      await page.getByTestId("copy-settings-submit").click();
      const copiedPayload = await (await copied).json();
      expect(copiedPayload).toEqual(expect.objectContaining({
        ok: true,
        source_node_id: nodeId,
        section,
        copied_count: 0,
      }));
      await expect(page.getByTestId("toast").filter({ hasText: "Copied 0 settings." }).last()).toBeVisible();
      await expect(page.getByTestId("modal-dialog")).toHaveCount(0);
    }
  } finally {
    await request.patch(`/api/nodes/${encodeURIComponent(nodeId)}`, {
      data: { archived: true },
    });
  }
});

test("runs toolbar and file commands from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/");

  await runPaletteCommand(page, "toolbar");
  await expect(page.getByTestId("toolbar-dock")).toHaveClass(/open/);

  await runPaletteCommand(page, "fullscreen-toolbar");
  await expect(page.getByTestId("toolbar-dock")).toHaveClass(/fullscreen/);
  await expect(page.getByTestId("toolbar-fullscreen")).toHaveAttribute("aria-pressed", "true");
  await runPaletteCommand(page, "fullscreen-toolbar");
  await expect(page.getByTestId("toolbar-dock")).not.toHaveClass(/fullscreen/);

  await runPaletteCommand(page, "files README.md");
  await expect(page.getByTestId("toolbar-files-panel")).toBeVisible();
  await expect(page.getByTestId("files-status")).toHaveText("README.md");
  await expect(page.getByTestId("files-source")).toContainText("Disposable target app");

  await runPaletteCommand(page, "search-files app.py");
  await expect(page.getByTestId("toolbar-files-panel")).toBeVisible();
  await expect(page.getByTestId("files-search-input")).toHaveValue("app.py");
  await expect(page.getByTestId("files-search-result").filter({ hasText: "app.py" })).toBeVisible();

  await runPaletteCommand(page, "toolbar");
  await expect(page.getByTestId("toolbar-dock")).not.toHaveClass(/open/);
});

test("runs target-app action commands from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await request.patch("/api/settings", {
    data: {
      target_app_start_command: "printf palette-target-start",
      target_app_stop_command: "printf palette-target-stop",
      target_app_rebuild_command: "printf palette-target-rebuild",
      target_app_status_command: "printf palette-target-status",
      target_app_cwd: "",
      target_app_env_json: "{}",
      target_app_start_timeout_seconds: "5",
      target_app_stop_timeout_seconds: "5",
      target_app_rebuild_timeout_seconds: "5",
      target_app_status_timeout_seconds: "5",
      target_app_http_check_url: "",
      target_app_tcp_check_host: "",
      target_app_tcp_check_port: "",
      target_app_process_check_command: "",
    },
  });

  try {
    await page.goto("/");

    const health = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/health") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "check-app", { commandId: "target_app.health" });
    const healthPayload = await (await health).json();
    expect(healthPayload.last_check_ok).toBe(true);
    expect(String(healthPayload.last_check_message ?? "")).toContain("palette-target-status");
    await expect(page.getByTestId("toast").filter({ hasText: "Status check OK" })).toBeVisible();

    const start = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/start") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "app-start", { commandId: "target_app.start" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Start the target application now?");
    await page.getByTestId("modal-ok").click();
    const startPayload = await (await start).json();
    expect(startPayload.ok).toBe(true);
    expect(startPayload.state).toBe("running");

    const rebuild = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/rebuild") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "app-rebuild", { commandId: "target_app.rebuild" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Rebuild the target application now?");
    await page.getByTestId("modal-ok").click();
    const rebuildPayload = await (await rebuild).json();
    expect(rebuildPayload.ok).toBe(true);
    expect(rebuildPayload.state).toBe("stopped");
    expect(String(rebuildPayload.last_operation?.stdout ?? "")).toBe("palette-target-rebuild");

    const stop = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/stop") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "app-stop", { commandId: "target_app.stop" });
    await expect(page.getByTestId("modal-dialog")).toContainText("Stop the target application now?");
    await page.getByTestId("modal-ok").click();
    const stopPayload = await (await stop).json();
    expect(stopPayload.ok).toBe(true);
    expect(stopPayload.state).toBe("stopped");
    expect(String(stopPayload.last_operation?.stdout ?? "")).toBe("palette-target-stop");
  } finally {
    await request.patch("/api/settings", {
      data: {
        target_app_start_command: "",
        target_app_stop_command: "",
        target_app_rebuild_command: "",
        target_app_status_command: "",
        target_app_cwd: "",
        target_app_env_json: "{}",
        target_app_start_timeout_seconds: "120",
        target_app_stop_timeout_seconds: "60",
        target_app_rebuild_timeout_seconds: "300",
        target_app_status_timeout_seconds: "10",
        target_app_http_check_url: "",
        target_app_tcp_check_host: "",
        target_app_tcp_check_port: "",
        target_app_process_check_command: "",
      },
    });
  }
});

test("creates and runs quality regressions from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const title = `Palette regression ${suffix}`;
  const prompt = "Open the attached Refine dashboard and capture a screenshot.";
  const targetPage = path.join(testAppRoot(), `quality-regression-${suffix}.html`);
  fs.writeFileSync(targetPage, `<!doctype html><title>${title}</title><main>${prompt}</main>\n`);
  const targetURL = pathToFileURL(targetPage).toString();
  const originalQuality = await jsonObject(await request.get("/api/quality"));
  const originalSettingsPayload = await jsonObject(await request.get("/api/settings"));
  const originalSettings = (originalSettingsPayload.settings as Record<string, unknown> | undefined) ?? {};
  let regressionId = "";

  await jsonObject(await request.patch("/api/settings", {
    data: {
      target_app_url: targetURL,
    },
  }));
  await jsonObject(await request.patch("/api/quality", {
    data: {
      enabled: "1",
      regressions_enabled: "1",
      business_requirements: `Palette quality requirements ${suffix}`,
      instructions: `Palette quality instructions ${suffix}`,
    },
  }));

  try {
    await page.goto("/");
    await runPaletteCommand(page, `new-regression ${prompt}`, { commandId: "quality.regression.new" });
    await expect(page.getByTestId("quality-regression-modal")).toBeVisible();
    await expect(page.getByTestId("quality-regression-prompt-input")).toHaveValue(prompt);
    await page.getByTestId("quality-regression-title-input").fill(title);
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/quality/regressions") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("quality-regression-create").click();
    const createdPayload = await (await created).json();
    regressionId = String(createdPayload.regression?.id ?? "");
    expect(regressionId).toBeTruthy();

    const row = page.getByTestId("quality-regression-row").filter({ hasText: title });
    await expect(row).toBeVisible();
    await expect(row.getByTestId("quality-regression-last-run")).toContainText("not run");

    const run = page.waitForResponse((response) =>
      response.url().includes("/api/quality/regressions/run") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "run-regressions", { commandId: "quality.regression.run" });
    const runPayload = await (await run).json();
    expect(runPayload.ok).toBe(true);
    expect(String(runPayload.message ?? "")).toContain("regression checks passed");
    expect(String(runPayload.runs?.[0]?.message ?? "")).toBe("Playwright regression passed");
    await expect(row.getByTestId("quality-regression-last-run")).toContainText("passed");

    const disabled = page.waitForResponse((response) =>
      response.url().includes(`/api/quality/regressions/${regressionId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("quality-regression-toggle").click();
    const disabledPayload = await (await disabled).json();
    expect(disabledPayload.regression?.enabled).toBe(false);
    await expect(row.getByTestId("quality-regression-toggle")).toHaveText("Enable");

    const enabled = page.waitForResponse((response) =>
      response.url().includes(`/api/quality/regressions/${regressionId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("quality-regression-toggle").click();
    const enabledPayload = await (await enabled).json();
    expect(enabledPayload.regression?.enabled).toBe(true);
    await expect(row.getByTestId("quality-regression-toggle")).toHaveText("Disable");

    await row.getByTestId("quality-regression-delete").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete this managed regression?");
    const deleted = page.waitForResponse((response) =>
      response.url().includes(`/api/quality/regressions/${regressionId}`) &&
      response.request().method() === "DELETE" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    await deleted;
    await expect(row).toHaveCount(0);
    regressionId = "";
  } finally {
    if (regressionId) {
      await request.delete(`/api/quality/regressions/${encodeURIComponent(regressionId)}`);
    }
    await request.patch("/api/settings", {
      data: {
        target_app_url: String(originalSettings.target_app_url ?? ""),
      },
    });
    await request.patch("/api/quality", {
      data: {
        business_requirements: String(originalQuality.business_requirements ?? ""),
        instructions: String(originalQuality.instructions ?? ""),
        enabled: String(originalQuality.enabled ?? "0"),
        timing: String(originalQuality.timing ?? "pre_merge"),
        regressions_enabled: String(originalQuality.regressions_enabled ?? "0"),
      },
    });
  }
});

test("opens refine issue request from the command palette", async ({ page }) => {
  await page.goto("/");
  await runPaletteCommand(page, "request-feature");
  await expect(page.getByTestId("refine-issue-modal")).toBeVisible();

  await page.getByTestId("refine-issue-title").fill("Palette smoke request");
  await page.evaluate(() => {
    window.open = () => null;
  });
  await page.getByTestId("refine-issue-submit").click();
  await expect(page.getByText("GitHub did not open. Allow popups for this site and try again.")).toBeVisible();
  await page.getByTestId("refine-issue-cancel").click();
  await expect(page.getByTestId("refine-issue-modal")).toHaveCount(0);
});

test("runs AI plan, draft, and target-app generation from the command palette", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const featureName = "Smoke AI Palette Draft Feature";

  try {
    await page.goto("/");

    await runPaletteCommand(page, "plan Draft Feature smoke plan request from palette.");
    await expect(page.getByTestId("toolbar-tab-plan")).toHaveClass(/active/);
    await expect(page.getByTestId("chat-output")).toContainText("smoke-ai plan actual behavior one", { timeout: 45_000 });

    const extracted = page.waitForResponse((response) =>
      response.url().includes("/api/import/extract") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await runPaletteCommand(page, "draft-gaps");
    await expect(page.getByTestId("plan-drafts-modal")).toBeVisible();
    const extractPayload = await (await extracted).json();
    expect(extractPayload.provider).toBe("smoke-ai");
    expect(extractPayload.source).toBe("provider");
    await page.getByTestId("import-feature-new-name").fill(featureName);
    await expect(page.getByTestId("import-draft-actual").first()).toHaveValue(/smoke-ai plan actual behavior one/);

    await page.getByTestId("plan-drafts-modal").getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("button", { name: "Cancel import", exact: true }).click();
    await expect(page.getByTestId("plan-drafts-modal")).toHaveCount(0);

    const generated = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/generate-instructions") &&
      response.request().method() === "POST" &&
      response.status() === 202
    );
    await runPaletteCommand(page, "target-generate");
    await page.getByRole("button", { name: "Generate", exact: true }).click();
    const generatePayload = await (await generated).json();
    const generateJobId = String(generatePayload.job?.id ?? "");
    expect(generatePayload.job?.owner).toBe("target-app:generate");
    expect(generateJobId).toBeTruthy();
    const generateResult = await waitForJobResult(request, generateJobId);
    expect(generateResult.provider).toBe("smoke-ai");
    expect(generateResult.source).toBe("provider");
    const generateConfig = (generateResult.config as Record<string, unknown> | undefined) ?? {};
    expect(generateConfig.start_command).toBe("./.refine/manage-app.sh start");
    await expect(page.getByTestId("target-app-start-command")).toHaveValue("./.refine/manage-app.sh start");
  } finally {
    const planTab = page.getByTestId("toolbar-tab-plan");
    if (await planTab.count()) {
      await planTab.locator("[data-close-tab]").click();
      await expect(page.getByTestId("toolbar-tab-plan")).toHaveCount(0);
    }
    await request.patch("/api/settings", {
      data: {
        target_app_start_command: "",
        target_app_stop_command: "",
        target_app_rebuild_command: "",
        target_app_status_command: "",
        target_app_cwd: "",
        target_app_env_json: "{}",
        target_app_start_timeout_seconds: "120",
        target_app_stop_timeout_seconds: "60",
        target_app_rebuild_timeout_seconds: "300",
        target_app_status_timeout_seconds: "10",
        target_app_log_path: "",
        target_app_http_check_url: "",
        target_app_tcp_check_host: "",
        target_app_tcp_check_port: "",
        target_app_process_check_command: "",
      },
    });
    const features = await jsonObject(await request.get("/api/features?limit=100&node=current"));
    const feature = ((features.features as Array<{ feature?: { id?: string; name?: string } }> | undefined) ?? [])
      .find((item) => item.feature?.name === featureName)?.feature;
    if (feature?.id) {
      await request.delete(`/api/features/${feature.id}`);
    }
  }
});
