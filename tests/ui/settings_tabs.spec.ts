import fs from "node:fs";
import path from "node:path";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";
import { attachProject, ensureAttachedProject, jsonObject, projectStatus } from "./helpers";

function testAppRoot(): string {
  return process.env.REFINE_TEST_APP_ROOT ||
    path.join(process.cwd(), "target/refine-integration/apps/rust-test-app");
}

function testRuntimeRoot(): string {
  return process.env.REFINE_TEST_RUNTIME_ROOT ||
    path.join(process.cwd(), "target/refine-integration/run");
}

function testRuntimeProcessDirs(): string[] {
  const runtimeRoot = testRuntimeRoot();
  const port = process.env.REFINE_TEST_PORT || "18080";
  return [
    path.join(runtimeRoot, "processes"),
    path.join(runtimeRoot, port, "processes"),
  ];
}

async function answerModalPrompt(page: Page, title: string, value: string): Promise<void> {
  await expect(page.getByTestId("modal-dialog")).toContainText(title);
  await page.getByTestId("modal-input").fill(value);
  await page.getByTestId("modal-ok").click();
}

function nodesFromPayload(payload: Record<string, unknown>): Array<Record<string, unknown>> {
  return (payload.nodes as Array<Record<string, unknown>> | undefined) ?? [];
}

test("navigates Node settings tabs from the tab strip", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/#/node/application");
  await expect(page.getByRole("heading", { name: "Node", level: 2 })).toBeVisible();

  const tabs = ["application", "reporters", "processes", "performance", "target-app", "runtime"];
  for (const tab of tabs) {
    await page.getByTestId(`settings-tab-${tab}`).click();
    await expect(page).toHaveURL(new RegExp(`#/node/${tab}$`));
    await expect(page.getByTestId(`settings-tab-${tab}`)).toHaveClass(/active/);
    await expect(page.getByTestId(`settings-pane-${tab}`)).toHaveClass(/active/);
  }
});

test("shows runtime upgrade status on the settings surface", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const upgrade = await jsonObject(await request.get("/api/upgrade"));
  const upgradePayload = upgrade.upgrade as Record<string, unknown>;
  expect(upgradePayload.local_development).toBe(false);
  expect(upgradePayload.upgrade_available).toBe(true);
  expect(upgradePayload.latest_version).toBe("999.0.0-test");

  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          (window as unknown as { __copiedRuntimeUpgrade?: string }).__copiedRuntimeUpgrade = text;
        },
      },
    });
  });

  await page.goto("/#/settings/processes");
  await expect(page.getByTestId("settings-pane-processes")).toHaveClass(/active/);
  await expect(page.getByTestId("runtime-upgrade-status")).toBeVisible();
  await expect(page.getByTestId("runtime-upgrade-message")).toHaveText("Upgrade available 999.0.0-test");
  await expect(page.getByTestId("runtime-copy-upgrade")).toBeVisible();
  await page.getByTestId("runtime-copy-upgrade").click();
  await expect(page.getByText("Upgrade command copied")).toBeVisible();
  await expect.poll(async () => page.evaluate(() =>
    (window as unknown as { __copiedRuntimeUpgrade?: string }).__copiedRuntimeUpgrade ?? "",
  )).toBe("./r update");
});

test("manages known apps from the Application tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const original = await projectStatus(request);
  const originalPath = String(original.client_repo ?? testAppRoot());
  const disposablePath = path.join(
    process.cwd(),
    `target/refine-integration/apps/application-tab-app-${Date.now()}`,
  );
  await request.delete("/api/apps", { data: { path: disposablePath } }).catch(() => undefined);
  fs.rmSync(disposablePath, { recursive: true, force: true });

  try {
    await page.goto("/#/node/application");
    await expect(page.getByTestId("project-app-select")).toContainText("rust-test-app");
    await page.getByTestId("project-add-app").click();
    await expect(page.getByTestId("project-setup-modal")).toBeVisible();
    await expect(page.getByTestId("project-setup-path")).toBeFocused();
    await page.getByTestId("project-setup-path").fill(disposablePath);
    await expect(page.getByTestId("project-setup-path-preview")).toContainText(disposablePath);
    const attachedNewApp = page.waitForResponse((response) =>
      response.url().includes("/api/project/attach") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("project-setup-submit").click();
    const attachedPayload = await (await attachedNewApp).json();
    expect(attachedPayload.client_repo).toBe(disposablePath);
    await expect(page.getByTestId("project-setup-modal")).toHaveCount(0);
    await expect.poll(async () =>
      String((await projectStatus(request)).client_repo ?? "")
    ).toBe(disposablePath);
    await expect(page.getByTestId("project-app-select")).toHaveValue(disposablePath);

    const templates = page.waitForResponse((response) =>
      response.url().includes("/api/project/templates") &&
      response.request().method() === "GET" &&
      response.status() === 200
    );
    await page.getByTestId("project-template").click();
    await templates;
    await expect(page.locator(".toast.warn", { hasText: "No app templates are available" })).toBeVisible();

    await page.getByTestId("project-app-select").selectOption(originalPath);
    const switchedBack = page.waitForResponse((response) =>
      response.url().includes("/api/project/attach") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("project-switch-app").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Switch refine to the selected app?");
    await page.getByTestId("modal-ok").click();
    const switchedPayload = await (await switchedBack).json();
    expect(switchedPayload.client_repo).toBe(originalPath);
    await expect.poll(async () =>
      String((await projectStatus(request)).client_repo ?? "")
    ).toBe(originalPath);
    await expect(page.getByTestId("project-app-select")).toHaveValue(originalPath);
    await expect(page.getByTestId("project-app-select").locator(`option[value="${disposablePath}"]`)).toHaveCount(1);

    await attachProject(request, disposablePath);
    await page.goto("/#/node/application");
    await expect(page.getByTestId("project-app-select")).toHaveValue(disposablePath);
    await page.getByTestId("project-app-select").selectOption(disposablePath);
    await expect(page.getByTestId("project-app-select")).toHaveValue(disposablePath);
    const removed = page.waitForResponse((response) =>
      response.url().includes("/api/apps") &&
      response.request().method() === "DELETE" &&
      response.status() === 200
    );
    await page.getByTestId("project-remove-app").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Remove this app");
    await page.getByTestId("modal-ok").click();
    const removedPayload = await (await removed).json();
    expect((removedPayload.apps as Array<{ path?: string }> | undefined) ?? [])
      .not.toEqual(expect.arrayContaining([expect.objectContaining({ path: disposablePath })]));
    await expect.poll(async () => (await projectStatus(request)).attached).toBe(false);
    await expect(page.getByTestId("project-app-select")).not.toContainText(path.basename(disposablePath));
  } finally {
    await attachProject(request, originalPath);
    await request.delete("/api/apps", { data: { path: disposablePath } }).catch(() => undefined);
    fs.rmSync(disposablePath, { recursive: true, force: true });
  }
});

test("manages application nodes from the Application tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const originalNodes = await jsonObject(await request.get("/api/nodes"));
  const originalActiveNodeId = String(originalNodes.active_node_id ?? "default");
  const suffix = Date.now();
  const nodeName = `Application UI Node ${suffix}`;
  const renamedNodeName = `Application UI Node Renamed ${suffix}`;
  let createdNodeId = "";

  try {
    await page.goto("/#/node/application");
    await expect(page.getByTestId("node-settings-table")).toBeVisible();
    await expect(page.getByTestId("node-add")).toBeVisible();

    const created = page.waitForResponse((response) =>
      response.url().includes("/api/nodes") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("node-add").click();
    await answerModalPrompt(page, "Create node", nodeName);
    const createdPayload = await (await created).json() as Record<string, unknown>;
    const createdNode = nodesFromPayload(createdPayload)
      .find((node) => node.display_name === nodeName);
    expect(createdNode).toBeTruthy();
    createdNodeId = String(createdNode?.id ?? "");
    expect(createdNodeId).toBeTruthy();

    const row = page.locator(`[data-testid="node-settings-row"][data-node-id="${createdNodeId}"]`);
    await expect(row).toBeVisible();
    await expect(row.getByTestId("node-settings-name")).toContainText(nodeName);
    await expect(row.getByTestId("node-settings-id")).toContainText(createdNodeId);
    await expect(row.getByTestId("node-activate")).toBeEnabled();
    await expect(row.getByTestId("node-archive")).toBeEnabled();

    const activated = page.waitForResponse((response) =>
      response.url().includes("/api/nodes/activate") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await row.getByTestId("node-activate").click();
    const activatedPayload = await (await activated).json() as Record<string, unknown>;
    expect(activatedPayload.active_node_id).toBe(createdNodeId);
    await expect(row.getByTestId("node-settings-name")).toContainText("active");
    await expect(row.getByTestId("node-activate")).toBeDisabled();
    await expect(row.getByTestId("node-archive")).toBeDisabled();

    const renamed = page.waitForResponse((response) =>
      response.url().includes(`/api/nodes/${createdNodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("node-rename").click();
    await answerModalPrompt(page, "Rename node", renamedNodeName);
    const renamedPayload = await (await renamed).json() as Record<string, unknown>;
    expect(nodesFromPayload(renamedPayload)).toEqual(expect.arrayContaining([
      expect.objectContaining({
        id: createdNodeId,
        display_name: renamedNodeName,
      }),
    ]));
    await expect(row.getByTestId("node-settings-name")).toContainText(renamedNodeName);

    const restored = page.waitForResponse((response) =>
      response.url().includes("/api/nodes/activate") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    const originalRow = page.locator(`[data-testid="node-settings-row"][data-node-id="${originalActiveNodeId}"]`);
    await originalRow.getByTestId("node-activate").click();
    const restoredPayload = await (await restored).json() as Record<string, unknown>;
    expect(restoredPayload.active_node_id).toBe(originalActiveNodeId);
    await expect(originalRow.getByTestId("node-settings-name")).toContainText("active");
    await expect(row.getByTestId("node-archive")).toBeEnabled();

    const archived = page.waitForResponse((response) =>
      response.url().includes(`/api/nodes/${createdNodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("node-archive").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Archive node");
    await page.getByTestId("modal-ok").click();
    const archivedPayload = await (await archived).json() as Record<string, unknown>;
    expect(nodesFromPayload(archivedPayload)).toEqual(expect.arrayContaining([
      expect.objectContaining({
        id: createdNodeId,
        archived: true,
      }),
    ]));
    await expect(row.getByTestId("node-settings-name")).toContainText("archived");
    await expect(row.getByTestId("node-activate")).toBeDisabled();
  } finally {
    if (createdNodeId) {
      await request.post("/api/nodes/activate", { data: { node_id: originalActiveNodeId } }).catch(() => undefined);
      await request.patch(`/api/nodes/${encodeURIComponent(createdNodeId)}`, {
        data: { archived: true },
      }).catch(() => undefined);
    }
  }
});

test("honors shared modal keyboard, focus, backdrop, and danger contracts", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const cancelledName = `Modal Cancelled Node ${suffix}`;
  const nodeName = `Modal Contract Node ${suffix}`;
  const originalNodes = await jsonObject(await request.get("/api/nodes"));
  const originalActiveNodeId = String(originalNodes.active_node_id ?? "default");
  let createdNodeId = "";

  try {
    await page.goto("/#/node/application");
    await page.getByTestId("node-add").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Create node");
    await expect(page.getByTestId("modal-input")).toBeFocused();
    await page.getByTestId("modal-input").fill(cancelledName);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);
    await expect(page.getByTestId("node-settings-table")).not.toContainText(cancelledName);

    const created = page.waitForResponse((response) =>
      response.url().includes("/api/nodes") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("node-add").click();
    await page.getByTestId("modal-input").fill(nodeName);
    await page.keyboard.press("Enter");
    const createdPayload = await (await created).json() as Record<string, unknown>;
    const createdNode = nodesFromPayload(createdPayload)
      .find((node) => node.display_name === nodeName);
    createdNodeId = String(createdNode?.id ?? "");
    expect(createdNodeId).toBeTruthy();

    const row = page.locator(`[data-testid="node-settings-row"][data-node-id="${createdNodeId}"]`);
    await expect(row).toBeVisible();
    await expect(row.getByTestId("node-settings-name")).toContainText(nodeName);

    await row.getByTestId("node-archive").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Archive node");
    await expect(page.getByTestId("modal-ok")).toHaveClass(/danger/);
    await page.getByTestId("modal-backdrop").click({ position: { x: 4, y: 4 } });
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);
    await expect(row.getByTestId("node-settings-name")).not.toContainText("archived");

    await row.getByTestId("node-archive").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Archive node");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("modal-dialog")).toHaveCount(0);
    await expect(row.getByTestId("node-settings-name")).not.toContainText("archived");

    const archived = page.waitForResponse((response) =>
      response.url().includes(`/api/nodes/${createdNodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("node-archive").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Archive node");
    await page.keyboard.press("Enter");
    const archivedPayload = await (await archived).json() as Record<string, unknown>;
    expect(nodesFromPayload(archivedPayload)).toEqual(expect.arrayContaining([
      expect.objectContaining({
        id: createdNodeId,
        archived: true,
      }),
    ]));
    await expect(row.getByTestId("node-settings-name")).toContainText("archived");
  } finally {
    await request.post("/api/nodes/activate", { data: { node_id: originalActiveNodeId } }).catch(() => undefined);
    if (createdNodeId) {
      await request.patch(`/api/nodes/${encodeURIComponent(createdNodeId)}`, {
        data: { archived: true },
      }).catch(() => undefined);
    }
  }
});

test("manages cluster nodes from the Application tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const nodeId = `cluster-ui-${suffix}`;
  const targetPath = `/srv/refine-target-${suffix}`;
  await request.delete(`/api/cluster/nodes/${encodeURIComponent(nodeId)}`).catch(() => undefined);

  try {
    await page.goto("/#/node/application");
    await expect(page.getByTestId("cluster-node-table")).toBeVisible();
    await expect(page.getByTestId("cluster-node-add")).toBeVisible();

    const registered = page.waitForResponse((response) =>
      response.url().includes("/api/cluster/nodes") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("cluster-node-add").click();
    await answerModalPrompt(page, "Register cluster node", nodeId);
    await answerModalPrompt(page, "Register cluster node", "cluster.example.test");
    await answerModalPrompt(page, "Register cluster node", targetPath);
    const registeredPayload = await (await registered).json();
    expect(registeredPayload.nodes).toEqual(expect.arrayContaining([
      expect.objectContaining({
        id: nodeId,
        ssh_host: "cluster.example.test",
        target_app_path: targetPath,
        enabled: true,
      }),
    ]));

    const row = page.locator(`[data-testid="cluster-node-row"][data-cluster-node-id="${nodeId}"]`);
    await expect(row).toBeVisible();
    await expect(row.getByTestId("cluster-node-name")).toContainText(nodeId);
    await expect(row.getByTestId("cluster-node-host")).toHaveText("cluster.example.test");
    await expect(row.getByTestId("cluster-node-status")).toHaveText("enabled");
    await expect(row.getByTestId("cluster-node-toggle")).toHaveText("Disable");

    const configured = page.waitForResponse((response) =>
      response.url().includes(`/api/cluster/nodes/${nodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("cluster-node-configure").click();
    await answerModalPrompt(page, "Configure cluster node", "Cluster UI Renamed");
    await answerModalPrompt(page, "Configure cluster node", "cluster-renamed.example.test");
    await answerModalPrompt(page, "Configure cluster node", "2222");
    await answerModalPrompt(page, "Configure cluster node", "~/refine-cluster");
    await answerModalPrompt(page, "Configure cluster node", `${targetPath}/renamed`);
    await answerModalPrompt(page, "Configure cluster node", "19090");
    const configuredPayload = await (await configured).json();
    expect(configuredPayload.nodes).toEqual(expect.arrayContaining([
      expect.objectContaining({
        id: nodeId,
        display_name: "Cluster UI Renamed",
        ssh_host: "cluster-renamed.example.test",
        ssh_port: 2222,
        refine_checkout: "~/refine-cluster",
        target_app_path: `${targetPath}/renamed`,
        refine_port: 19090,
      }),
    ]));
    await expect(row.getByTestId("cluster-node-name")).toContainText("Cluster UI Renamed");
    await expect(row.getByTestId("cluster-node-host")).toHaveText("cluster-renamed.example.test");
    await expect(row.getByTestId("cluster-node-ssh-port")).toHaveText("2222");
    await expect(row.getByTestId("cluster-node-refine-port")).toHaveText("19090");

    const disabled = page.waitForResponse((response) =>
      response.url().includes(`/api/cluster/nodes/${nodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("cluster-node-toggle").click();
    const disabledPayload = await (await disabled).json();
    expect(disabledPayload.nodes).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: nodeId, enabled: false }),
    ]));
    await expect(row.getByTestId("cluster-node-status")).toHaveText("disabled");
    await expect(row.getByTestId("cluster-node-toggle")).toHaveText("Enable");

    const enabled = page.waitForResponse((response) =>
      response.url().includes(`/api/cluster/nodes/${nodeId}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await row.getByTestId("cluster-node-toggle").click();
    const enabledPayload = await (await enabled).json();
    expect(enabledPayload.nodes).toEqual(expect.arrayContaining([
      expect.objectContaining({ id: nodeId, enabled: true }),
    ]));
    await expect(row.getByTestId("cluster-node-status")).toHaveText("enabled");
    await expect(row.getByTestId("cluster-node-toggle")).toHaveText("Disable");
  } finally {
    await request.delete(`/api/cluster/nodes/${encodeURIComponent(nodeId)}`).catch(() => undefined);
  }
});

test("expands supervisor child processes from the Processes tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const processDirs = testRuntimeProcessDirs();
  const supervisorId = "000-supervisor-tree";
  const uiId = "001-ui-child";
  const runnerId = "002-runner-child";
  const processPaths = processDirs.flatMap((processDir) => [
    path.join(processDir, `${supervisorId}.json`),
    path.join(processDir, `${uiId}.json`),
    path.join(processDir, `${runnerId}.json`),
  ]);

  try {
    for (const processDir of processDirs) fs.mkdirSync(processDir, { recursive: true });
    for (const processDir of processDirs) {
      fs.writeFileSync(path.join(processDir, `${supervisorId}.json`), JSON.stringify({
        id: supervisorId,
        owner: "daemon",
        pid: null,
        state: "running",
        label: "Supervisor tree",
        details: "",
        started_at: new Date().toISOString(),
      }, null, 2));
      fs.writeFileSync(path.join(processDir, `${uiId}.json`), JSON.stringify({
        id: uiId,
        owner: "user_helper",
        pid: null,
        state: "running",
        label: "UI child",
        details: JSON.stringify({ kind: "ui" }),
        started_at: new Date().toISOString(),
      }, null, 2));
      fs.writeFileSync(path.join(processDir, `${runnerId}.json`), JSON.stringify({
        id: runnerId,
        owner: "maintenance",
        pid: null,
        state: "running",
        label: "Runner child",
        details: JSON.stringify({ kind: "runner" }),
        started_at: new Date().toISOString(),
      }, null, 2));
    }

    await expect.poll(async () => {
      const summary = await jsonObject(await request.get("/api/processes"));
      return (summary.processes as Array<{ id?: string; kind?: string }> | undefined ?? [])
        .map((process) => `${process.id}:${process.kind}`);
    }).toEqual(expect.arrayContaining([
      `${supervisorId}:daemon`,
      `${uiId}:ui`,
      `${runnerId}:runner`,
    ]));

    await page.goto("/#/node/processes");
    const supervisorRow = page.locator(
      `[data-testid="managed-process-row"][data-process-id="${supervisorId}"]`,
    );
    const uiRow = page.locator(
      `[data-testid="managed-process-row"][data-process-id="${uiId}"]`,
    );
    const runnerRow = page.locator(
      `[data-testid="managed-process-row"][data-process-id="${runnerId}"]`,
    );
    await expect(supervisorRow).toBeVisible();
    await expect(supervisorRow).toHaveAttribute("data-process-kind", "supervisor");
    await expect(supervisorRow.getByTestId("process-supervisor-toggle")).toHaveAttribute("aria-expanded", "false");
    await expect(uiRow).toBeHidden();
    await expect(runnerRow).toBeHidden();

    await supervisorRow.getByTestId("process-supervisor-toggle").click();
    await expect(supervisorRow.getByTestId("process-supervisor-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(uiRow).toBeVisible();
    await expect(uiRow).toHaveAttribute("data-supervisor-child", "1");
    await expect(runnerRow).toBeVisible();
    await expect(runnerRow).toHaveAttribute("data-supervisor-child", "1");

    await supervisorRow.getByTestId("process-supervisor-toggle").click();
    await expect(supervisorRow.getByTestId("process-supervisor-toggle")).toHaveAttribute("aria-expanded", "false");
    await expect(uiRow).toBeHidden();
    await expect(runnerRow).toBeHidden();
  } finally {
    for (const processPath of processPaths) fs.rmSync(processPath, { force: true });
  }
});

test("controls background and agent processes from the Processes tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await jsonObject(await request.post("/api/processes/background", { data: { stopped: false } }));
  await jsonObject(await request.post("/api/processes/agents", { data: { paused: false } }));

  try {
    await page.goto("/#/node/processes");
    await expect(page.getByTestId("settings-pane-processes")).toHaveClass(/active/);
    await expect(page.getByTestId("managed-process-table")).toBeVisible();

    const backgroundRow = page.locator(
      '[data-testid="managed-process-row"][data-process-kind="background_processes"]',
    );
    const agentRow = page.locator(
      '[data-testid="managed-process-row"][data-process-kind="agent_scheduler"]',
    );
    await expect(backgroundRow.getByTestId("managed-process-status")).toHaveText("active");
    await expect(backgroundRow.getByTestId("process-background-toggle")).toHaveText("Stop Background");
    await expect(backgroundRow.getByTestId("process-hard-reset-worktree")).toBeEnabled();
    await expect(agentRow.getByTestId("managed-process-status")).toHaveText("active");
    await expect(agentRow.getByTestId("process-agent-toggle")).toHaveText("Pause agents");

    const backgroundStopped = page.waitForResponse((response) =>
      response.url().includes("/api/processes/background") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await backgroundRow.getByTestId("process-background-toggle").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Stop background processes?");
    await page.getByTestId("modal-ok").click();
    const backgroundStoppedPayload = await (await backgroundStopped).json();
    expect(backgroundStoppedPayload.background_processes_stopped).toBe(true);
    await expect(backgroundRow.getByTestId("managed-process-status")).toHaveText("paused");
    await expect(backgroundRow.getByTestId("process-background-toggle")).toHaveText("Start Background");
    await expect(backgroundRow.getByTestId("process-hard-reset-worktree")).toBeDisabled();

    const backgroundStarted = page.waitForResponse((response) =>
      response.url().includes("/api/processes/background") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await backgroundRow.getByTestId("process-background-toggle").click();
    const backgroundStartedPayload = await (await backgroundStarted).json();
    expect(backgroundStartedPayload.background_processes_stopped).toBe(false);
    await expect(backgroundRow.getByTestId("managed-process-status")).toHaveText("active");

    const agentsPaused = page.waitForResponse((response) =>
      response.url().includes("/api/processes/agents") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await agentRow.getByTestId("process-agent-toggle").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Pause agent scheduling?");
    await page.getByTestId("modal-ok").click();
    const agentsPausedPayload = await (await agentsPaused).json();
    expect(agentsPausedPayload.agents_paused).toBe(true);
    await expect(agentRow.getByTestId("managed-process-status")).toHaveText("paused");
    await expect(agentRow.getByTestId("process-agent-toggle")).toHaveText("Unpause agents");

    const agentsUnpaused = page.waitForResponse((response) =>
      response.url().includes("/api/processes/agents") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await agentRow.getByTestId("process-agent-toggle").click();
    const agentsUnpausedPayload = await (await agentsUnpaused).json();
    expect(agentsUnpausedPayload.agents_paused).toBe(false);
    await expect(agentRow.getByTestId("managed-process-status")).toHaveText("active");
  } finally {
    await request.post("/api/processes/background", { data: { stopped: false } }).catch(() => undefined);
    await request.post("/api/processes/agents", { data: { paused: false } }).catch(() => undefined);
  }
});

test("runs subprocess worker actions from the Processes tab", async ({ page, request }) => {
  test.setTimeout(60_000);
  await ensureAttachedProject(request);
  await jsonObject(await request.post("/api/processes/background", { data: { stopped: false } }));
  const original = await jsonObject(await request.get("/api/settings"));
  const originalSettings = (original.settings as Record<string, unknown> | undefined) ?? {};
  const restoreKeys = [
    "agent_cli",
    "target_app_start_command",
    "target_app_stop_command",
    "target_app_rebuild_command",
    "target_app_status_command",
    "target_app_cwd",
    "target_app_env_json",
    "target_app_start_timeout_seconds",
    "target_app_stop_timeout_seconds",
    "target_app_rebuild_timeout_seconds",
    "target_app_status_timeout_seconds",
    "target_app_log_path",
    "target_app_http_check_url",
    "target_app_tcp_check_host",
    "target_app_tcp_check_port",
    "target_app_process_check_command",
  ];
  const restore = Object.fromEntries(
    restoreKeys.map((key) => [key, String(originalSettings[key] ?? "")]),
  );
  const prefix = `Processes worker cleanup ${Date.now()}`;

  try {
    await request.patch("/api/settings", {
      data: {
        agent_cli: "smoke-ai",
        target_app_start_command: "",
        target_app_stop_command: "",
        target_app_rebuild_command: "printf processes-worker-rebuild",
        target_app_status_command: "",
        target_app_cwd: "",
        target_app_env_json: "{}",
        target_app_rebuild_timeout_seconds: "5",
        target_app_http_check_url: "",
        target_app_tcp_check_host: "",
        target_app_tcp_check_port: "",
        target_app_process_check_command: "",
      },
    });
    await jsonObject(await request.post("/api/activity/ui-error", {
      data: {
        message: `${prefix} seeded activity`,
        marker: prefix,
        source: "settings-tabs-processes.spec",
      },
    }));

    await page.goto("/#/node/processes");
    await expect(page.getByTestId("settings-pane-processes")).toHaveClass(/active/);
    await expect(page.getByTestId("subprocess-table")).toBeVisible();
    const row = (kind: string) => page.locator(
      `[data-testid="runner-work-row"][data-runner-work-kind="${kind}"]`,
    );
    for (const kind of [
      "target_app_rebuilder",
      "target_app_config_generator",
      "sqlite_cache_rebuild",
      "activity_log_cleanup",
    ]) {
      await expect(row(kind)).toBeVisible();
      await expect(row(kind).locator("td").nth(1)).toHaveText("idle");
    }

    const rebuiltTargetApp = page.waitForResponse((response) =>
      response.url().includes("/api/runner-workers/target-app-rebuilder/rebuild") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await row("target_app_rebuilder").getByTestId("runner-target-app-rebuild").click();
    const rebuildPayload = await (await rebuiltTargetApp).json();
    expect(rebuildPayload.ok).toBe(true);
    expect(rebuildPayload.queued).toBe(true);
    expect(String(rebuildPayload.last_operation?.stdout ?? "")).toBe("processes-worker-rebuild");

    const generated = page.waitForResponse((response) =>
      response.url().includes("/api/target-app/generate-instructions") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await row("target_app_config_generator").getByTestId("runner-target-app-generate").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Generate target-app config");
    await page.getByTestId("modal-ok").click();
    const generatedPayload = await (await generated).json();
    expect(generatedPayload.provider).toBe("smoke-ai");
    expect(generatedPayload.source).toBe("provider");
    expect(generatedPayload.config.start_command).toBe("printf smoke-ai-target-start");
    await expect(page.getByTestId("target-app-start-command")).toHaveValue("printf smoke-ai-target-start");

    await page.getByTestId("settings-tab-processes").click();
    await expect(page.getByTestId("settings-pane-processes")).toHaveClass(/active/);
    const rebuiltCache = page.waitForResponse((response) =>
      response.url().includes("/api/cache/rebuild") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await row("sqlite_cache_rebuild").getByTestId("runner-cache-rebuild").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Rebuild projection cache");
    await page.getByTestId("modal-ok").click();
    const cachePayload = await (await rebuiltCache).json();
    expect(cachePayload.ok).toBe(true);
    expect(String(cachePayload.cache ?? "")).toContain("target/refine-integration/run");

    const cleaned = page.waitForResponse((response) =>
      response.url().includes("/api/activity/cleanup") &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await row("activity_log_cleanup").getByTestId("runner-log-cleanup").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Clean up old logs");
    await expect(page.getByTestId("modal-dialog")).toContainText("Delete activity log entries older than 7 days");
    await page.getByTestId("modal-ok").click();
    const cleanupPayload = await (await cleaned).json();
    expect(cleanupPayload.ok).toBe(true);
    expect(cleanupPayload.cleared).toBe(false);
    expect(cleanupPayload.retention_days).toBe(7);
    expect(Number(cleanupPayload.deleted ?? 0)).toBeGreaterThanOrEqual(0);
  } finally {
    await request.patch("/api/settings", { data: restore }).catch(() => undefined);
    await request.post("/api/processes/background", { data: { stopped: false } }).catch(() => undefined);
  }
});

test("cancels agent and stops chat subprocesses from the Processes tab", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const processDirs = testRuntimeProcessDirs();
  const processPaths = processDirs.flatMap((processDir) => [
    path.join(processDir, "ui-agent-process.json"),
    path.join(processDir, "ui-chat-process.json"),
  ]);
  let gapId = "";
  let sessionId = "";

  try {
    const gapPayload = await jsonObject(await request.post("/api/gaps", {
      data: {
        name: `process action gap ${Date.now()}`,
        reporter: "refine-smoke",
      },
    }));
    gapId = String((gapPayload.gap as { id?: string } | undefined)?.id ?? "");
    expect(gapId).toBeTruthy();
    const chatPayload = await jsonObject(await request.post("/api/chat/start", {
      data: {
        provider: "smoke-ai",
        mode: "standalone",
      },
    }));
    sessionId = String(chatPayload.session_id ?? "");
    expect(sessionId).toBeTruthy();

    for (const processDir of processDirs) fs.mkdirSync(processDir, { recursive: true });
    for (const processDir of processDirs) {
      fs.writeFileSync(path.join(processDir, "ui-agent-process.json"), JSON.stringify({
      id: "ui-agent-process",
      owner: "agent",
      pid: null,
      state: "running",
      label: "UI test agent",
      details: JSON.stringify({ gap_id: gapId, round_idx: 0 }),
      started_at: new Date().toISOString(),
      }, null, 2));
      fs.writeFileSync(path.join(processDir, "ui-chat-process.json"), JSON.stringify({
      id: "ui-chat-process",
      owner: "user_helper",
      pid: null,
      state: "running",
      label: "UI test chat",
      details: JSON.stringify({ session_id: sessionId, mode: "standalone" }),
      started_at: new Date().toISOString(),
      }, null, 2));
    }

    await expect.poll(async () => {
      const summary = await jsonObject(await request.get("/api/processes"));
      return (summary.processes as Array<{ id?: string }> | undefined ?? []).map((process) => process.id);
    }).toEqual(expect.arrayContaining(["ui-agent-process", "ui-chat-process"]));

    await page.goto("/#/node/processes");
    const agentRow = page.locator(
      '[data-testid="subprocess-row"][data-process-id="ui-agent-process"]',
    );
    const chatRow = page.locator(
      '[data-testid="subprocess-row"][data-process-id="ui-chat-process"]',
    );
    await expect(agentRow).toBeVisible();
    await expect(agentRow.getByTestId("process-cancel-agent")).toBeVisible();
    await expect(chatRow).toBeVisible();
    await expect(chatRow.getByTestId("process-stop-chat")).toBeVisible();

    const cancelled = page.waitForResponse((response) =>
      response.url().includes(`/api/gaps/${encodeURIComponent(gapId)}/cancel`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await agentRow.getByTestId("process-cancel-agent").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Cancel this Gap's running subprocess?");
    await page.getByTestId("modal-ok").click();
    const cancelledPayload = await (await cancelled).json();
    expect(cancelledPayload.gap.status).toBe("cancelled");
    const cancelledGap = await jsonObject(await request.get(`/api/gaps/${encodeURIComponent(gapId)}`));
    expect((cancelledGap.gap as { status?: string } | undefined)?.status).toBe("cancelled");

    const stopped = page.waitForResponse((response) =>
      response.url().includes(`/api/chat/${encodeURIComponent(sessionId)}/stop`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await chatRow.getByTestId("process-stop-chat").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Stop this chat session?");
    await page.getByTestId("modal-ok").click();
    const stoppedPayload = await (await stopped).json();
    expect(stoppedPayload.alive).toBe(false);
  } finally {
    for (const processPath of processPaths) fs.rmSync(processPath, { force: true });
    if (sessionId) {
      await request.post(`/api/chat/${encodeURIComponent(sessionId)}/stop`).catch(() => undefined);
    }
    if (gapId) {
      await request.delete(`/api/gaps/${encodeURIComponent(gapId)}`).catch(() => undefined);
    }
  }
});

test("autosaves Runtime Config fields", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const original = await jsonObject(await request.get("/api/settings"));
  const originalSettings = original.settings as Record<string, unknown>;
  const suffix = Date.now();
  const expected = {
    parallel_run_cap: "7",
    branch_name_pattern: `ui-runtime-${suffix}/{gap_id}`,
    agent_idle_timeout_seconds: "777",
    agent_hard_cap_seconds: "9999",
    worker_memory_limit_mb: "1536",
    ui_memory_limit_mb: "768",
    worker_cpu_priority: "very_low",
    resource_isolation_mode: "best_effort",
    agent_limit_pause_seconds: "3600",
    chat_idle_timeout_seconds: "123",
    backlog_promote_after_seconds: "300",
    project_update_pulse_interval_seconds: "900",
    file_browser_ignore_patterns: `node_modules, .git, ui-runtime-${suffix}`,
  };
  const restoreKeys = [...Object.keys(expected), "agent_cli"];
  const restore = Object.fromEntries(
    restoreKeys.map((key) => [key, String(originalSettings[key] ?? "")]),
  );
  const patchMatches = (responseUrl: string) => responseUrl.includes("/api/settings");
  const fillRuntimeInput = async (testId: string, value: string) => {
    const saved = page.waitForResponse((response) =>
      patchMatches(response.url()) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId(testId).fill(value);
    await page.getByTestId(testId).blur();
    await saved;
  };
  const selectRuntimeOption = async (testId: string, value: string) => {
    const saved = page.waitForResponse((response) =>
      patchMatches(response.url()) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId(testId).selectOption(value);
    await saved;
  };

  try {
    await page.goto("/#/node/runtime");
    await expect(page.getByTestId("settings-pane-runtime")).toHaveClass(/active/);
    await fillRuntimeInput("runtime-parallel-run-cap", expected.parallel_run_cap);
    await fillRuntimeInput("runtime-branch-name-pattern", expected.branch_name_pattern);
    await fillRuntimeInput("runtime-agent-idle-timeout", expected.agent_idle_timeout_seconds);
    await fillRuntimeInput("runtime-agent-hard-cap", expected.agent_hard_cap_seconds);
    await fillRuntimeInput("runtime-worker-memory-limit", expected.worker_memory_limit_mb);
    await fillRuntimeInput("runtime-ui-memory-limit", expected.ui_memory_limit_mb);
    await selectRuntimeOption("runtime-worker-cpu-priority", expected.worker_cpu_priority);
    await selectRuntimeOption("runtime-resource-isolation", expected.resource_isolation_mode);
    await selectRuntimeOption("runtime-agent-limit-pause", expected.agent_limit_pause_seconds);
    await fillRuntimeInput("runtime-chat-idle-timeout", expected.chat_idle_timeout_seconds);
    await selectRuntimeOption("runtime-backlog-promote", expected.backlog_promote_after_seconds);
    await selectRuntimeOption("runtime-project-update-pulse", expected.project_update_pulse_interval_seconds);
    await fillRuntimeInput("runtime-file-browser-ignore", expected.file_browser_ignore_patterns);

    const savedPayload = await jsonObject(await request.get("/api/settings"));
    expect(savedPayload.settings).toEqual(expect.objectContaining(expected));

    await page.reload();
    for (const [testId, value] of [
      ["runtime-parallel-run-cap", expected.parallel_run_cap],
      ["runtime-branch-name-pattern", expected.branch_name_pattern],
      ["runtime-agent-idle-timeout", expected.agent_idle_timeout_seconds],
      ["runtime-agent-hard-cap", expected.agent_hard_cap_seconds],
      ["runtime-worker-memory-limit", expected.worker_memory_limit_mb],
      ["runtime-ui-memory-limit", expected.ui_memory_limit_mb],
      ["runtime-worker-cpu-priority", expected.worker_cpu_priority],
      ["runtime-resource-isolation", expected.resource_isolation_mode],
      ["runtime-agent-limit-pause", expected.agent_limit_pause_seconds],
      ["runtime-chat-idle-timeout", expected.chat_idle_timeout_seconds],
      ["runtime-backlog-promote", expected.backlog_promote_after_seconds],
      ["runtime-project-update-pulse", expected.project_update_pulse_interval_seconds],
      ["runtime-file-browser-ignore", expected.file_browser_ignore_patterns],
    ] as const) {
      await expect(page.getByTestId(testId)).toHaveValue(value);
    }
  } finally {
    await request.patch("/api/settings", { data: restore });
  }
});

test("navigates Governance settings tabs from the tab strip", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await page.goto("/#/project/governance");
  await expect(page.getByRole("heading", { name: "Governance", level: 2 })).toBeVisible();

  for (const tab of ["governance", "quality", "guidance"]) {
    await page.getByTestId(`settings-tab-${tab}`).click();
    await expect(page).toHaveURL(new RegExp(`#/project/${tab}$`));
    await expect(page.getByTestId(`settings-tab-${tab}`)).toHaveClass(/active/);
    await expect(page.getByTestId(`settings-pane-${tab}`)).toHaveClass(/active/);
  }
});

async function seedPerformanceMetrics(request: APIRequestContext) {
  for (const path of ["/", "/api/project/status", "/api/settings", "/api/reporters"]) {
    await request.get(path);
  }
}

async function waitForPerformanceMetrics(
  request: APIRequestContext,
  minEvents = 1,
) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const payload = await jsonObject(
      await request.get("/api/performance?operation=http.request&limit=50&offset=0"),
    );
    if (Number(payload.total_event_count ?? 0) >= minEvents) return payload;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return jsonObject(await request.get("/api/performance?operation=http.request&limit=50&offset=0"));
}

test("filters, refreshes, prunes, and clears Performance metrics", async ({ page, request }) => {
  await ensureAttachedProject(request);
  await jsonObject(await request.post("/api/performance/cleanup", { data: { clear: true } }));
  await seedPerformanceMetrics(request);
  const seeded = await waitForPerformanceMetrics(request, 2);
  expect(seeded.operations).toEqual(expect.arrayContaining(["http.request"]));

  await page.goto("/#/node/performance");
  await expect(page.getByTestId("settings-pane-performance")).toHaveClass(/active/);
  await expect(page.getByTestId("performance-summary-table")).toBeVisible();
  await expect(
    page.getByTestId("performance-summary-row").filter({ hasText: "http.request" }),
  ).toHaveCount(1);
  await expect(page.getByTestId("performance-events-table")).toBeVisible();
  await expect(page.getByTestId("performance-event-row").first()).toContainText("http.request");

  await page.getByTestId("performance-filter-shell").locator("summary").click();
  const operationFiltered = page.waitForResponse((response) =>
    response.url().includes("/api/performance?") &&
    response.url().includes("operation=http.request") &&
    response.status() === 200
  );
  await page.getByTestId("performance-operation-filter").selectOption("http.request");
  await operationFiltered;
  await expect(page).toHaveURL(/#\/node\/performance\?operation=http\.request/);
  await expect(page.getByTestId("performance-filtered-pill")).toBeVisible();
  await expect(page.getByTestId("performance-event-operation").first()).toHaveText("http.request");

  const successFiltered = page.waitForResponse((response) =>
    response.url().includes("/api/performance?") &&
    response.url().includes("success=1") &&
    response.status() === 200
  );
  await page.getByTestId("performance-success-filter").selectOption("1");
  await successFiltered;
  await expect(page).toHaveURL(/success=1/);
  await expect(page.getByTestId("performance-event-outcome").first()).toHaveText("success");

  const limitFiltered = page.waitForResponse((response) =>
    response.url().includes("/api/performance?") &&
    response.url().includes("limit=100") &&
    response.status() === 200
  );
  await page.getByTestId("performance-limit-filter").selectOption("100");
  await limitFiltered;
  await expect(page).toHaveURL(/limit=100/);

  await page.getByTestId("performance-clear-filters").click();
  await expect(page).toHaveURL(/#\/node\/performance$/);
  await expect(page.getByTestId("performance-filtered-pill")).toBeHidden();

  const refreshed = page.waitForResponse((response) =>
    response.url().includes("/api/performance?") && response.status() === 200
  );
  await page.getByTestId("performance-refresh").click();
  await refreshed;
  await expect(page.getByTestId("performance-events-table")).toBeVisible();

  const pruned = page.waitForResponse((response) =>
    response.url().includes("/api/performance/cleanup") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("performance-prune").click();
  await pruned;
  await expect(page.getByTestId("performance-events-table")).toBeVisible();

  await page.getByTestId("performance-clear").click();
  await expect(page.getByTestId("modal-dialog")).toContainText("Clear metrics");
  const cleared = page.waitForResponse((response) =>
    response.url().includes("/api/performance/cleanup") &&
    response.request().method() === "POST" &&
    response.status() === 200
  );
  await page.getByTestId("modal-ok").click();
  const clearPayload = await (await cleared).json();
  expect(clearPayload.cleared).toBe(true);
  expect(clearPayload.retained).toBe(0);
  expect(Number(clearPayload.deleted ?? 0)).toBeGreaterThan(0);
  await expect(page.getByTestId("performance-total-stored")).toBeVisible();
});

test("manages reporters from Node settings", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const suffix = Date.now();
  const sourceName = `Reporter source ${suffix}`;
  const renamedSource = `Reporter renamed ${suffix}`;
  const targetName = `Reporter target ${suffix}`;
  const createdIds = new Set<string>();

  const reporterNames = async () => {
    const payload = await jsonObject(await request.get("/api/reporters"));
    const reporters = payload.reporters as Array<{ id?: number | string; name?: string }> | undefined ?? [];
    return reporters.map((reporter) => String(reporter.name ?? ""));
  };
  const rowFor = (name: string) => page.getByTestId("reporter-row").filter({ hasText: name });
  const addReporter = async (name: string) => {
    await page.getByTestId("reporter-add").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Add reporter");
    await page.getByTestId("modal-input").fill(name);
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/reporters") &&
      response.request().method() === "POST" &&
      response.status() === 201
    );
    await page.getByTestId("modal-ok").click();
    const payload = await (await created).json();
    const id = String(payload.reporter?.id ?? "");
    expect(id).toBeTruthy();
    createdIds.add(id);
    await expect(rowFor(name)).toHaveCount(1);
    return id;
  };

  try {
    await page.goto("/#/node/reporters");
    await expect(page.getByTestId("settings-pane-reporters")).toHaveClass(/active/);

    const sourceId = await addReporter(sourceName);
    const targetId = await addReporter(targetName);
    expect(await reporterNames()).toEqual(expect.arrayContaining([sourceName, targetName]));

    await rowFor(sourceName).getByTestId("reporter-rename").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Rename reporter");
    await page.getByTestId("modal-input").fill(renamedSource);
    const renamed = page.waitForResponse((response) =>
      response.url().includes(`/api/reporters/${encodeURIComponent(sourceId)}`) &&
      response.request().method() === "PATCH" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    await renamed;
    await expect(rowFor(renamedSource)).toHaveCount(1);
    expect(await reporterNames()).toEqual(expect.arrayContaining([renamedSource, targetName]));

    await rowFor(renamedSource).getByTestId("reporter-merge").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Merge reporter");
    await page.getByTestId("reporter-merge-target").selectOption(targetId);
    const merged = page.waitForResponse((response) =>
      response.url().includes(`/api/reporters/${encodeURIComponent(sourceId)}/merge`) &&
      response.request().method() === "POST" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    await merged;
    createdIds.delete(sourceId);
    await expect(rowFor(renamedSource)).toHaveCount(0);
    await expect(rowFor(targetName)).toHaveCount(1);
    expect(await reporterNames()).not.toContain(renamedSource);

    await rowFor(targetName).getByTestId("reporter-remove").click();
    await expect(page.getByTestId("modal-dialog")).toContainText("Remove reporter");
    const removed = page.waitForResponse((response) =>
      response.url().includes(`/api/reporters/${encodeURIComponent(targetId)}`) &&
      response.request().method() === "DELETE" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    await removed;
    createdIds.delete(targetId);
    await expect(rowFor(targetName)).toHaveCount(0);
    expect(await reporterNames()).not.toContain(targetName);
  } finally {
    for (const id of Array.from(createdIds)) {
      await request.delete(`/api/reporters/${encodeURIComponent(id)}`);
    }
  }
});

test("creates, edits, disables, and deletes Guidance entries", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const originalPayload = await jsonObject(await request.get("/api/guidance"));
  const originalGuidance = originalPayload.guidance as Array<Record<string, unknown>> | undefined ?? [];
  const suffix = Date.now();
  const name = `Guidance smoke ${suffix}`;
  const renamed = `Guidance smoke renamed ${suffix}`;
  const rule = `Apply to smoke guidance ${suffix}`;
  const instructions = `Use deterministic guidance instructions ${suffix}.`;
  const updatedRule = `Apply to renamed smoke guidance ${suffix}`;
  const updatedInstructions = `Use updated deterministic guidance instructions ${suffix}.`;
  const rowFor = (label: string) => page.getByTestId("guidance-row").filter({ hasText: label });

  try {
    await page.goto("/#/project/guidance");
    await expect(page.getByTestId("settings-pane-guidance")).toHaveClass(/active/);

    await page.getByTestId("guidance-add").click();
    await expect(page.getByTestId("guidance-modal")).toContainText("New guidance");
    await page.getByTestId("guidance-name-input").fill(name);
    await page.getByTestId("guidance-rule-input").fill(rule);
    await page.getByTestId("guidance-instructions-input").fill(instructions);
    const created = page.waitForResponse((response) =>
      response.url().includes("/api/guidance") &&
      response.request().method() === "PUT" &&
      response.status() === 200
    );
    await page.getByTestId("guidance-submit").click();
    await created;
    await expect(rowFor(name)).toHaveCount(1);
    await expect(rowFor(name).getByTestId("guidance-row-status")).toHaveText("Enabled");

    await rowFor(name).click();
    await expect(page.getByTestId("guidance-modal")).toContainText("Edit guidance");
    await page.getByTestId("guidance-name-input").fill(renamed);
    await page.getByTestId("guidance-rule-input").fill(updatedRule);
    await page.getByTestId("guidance-instructions-input").fill(updatedInstructions);
    await page.getByTestId("guidance-status-toggle").click();
    const edited = page.waitForResponse((response) =>
      response.url().includes("/api/guidance") &&
      response.request().method() === "PUT" &&
      response.status() === 200
    );
    await page.getByTestId("guidance-submit").click();
    await edited;
    await expect(rowFor(renamed)).toHaveCount(1);
    await expect(rowFor(renamed).getByTestId("guidance-row-status")).toHaveText("Disabled");
    await expect(rowFor(renamed).getByTestId("guidance-row-rule")).toHaveText(updatedRule);

    const guidancePayload = await jsonObject(await request.get("/api/guidance"));
    const guidanceItems = guidancePayload.guidance as Array<{ name?: string; enabled?: boolean; instructions?: string }> | undefined ?? [];
    expect(guidanceItems).toEqual(expect.arrayContaining([
      expect.objectContaining({
        name: renamed,
        enabled: false,
        instructions: updatedInstructions,
      }),
    ]));

    await rowFor(renamed).click();
    await page.getByTestId("guidance-delete").click();
    await expect(page.getByTestId("modal-dialog")).toContainText(`Delete guidance "${renamed}"?`);
    const deleted = page.waitForResponse((response) =>
      response.url().includes("/api/guidance") &&
      response.request().method() === "PUT" &&
      response.status() === 200
    );
    await page.getByTestId("modal-ok").click();
    await deleted;
    await expect(rowFor(renamed)).toHaveCount(0);
  } finally {
    await request.put("/api/guidance", { data: { guidance: originalGuidance } });
  }
});

test("shows and clears the Quality requirements warning", async ({ page, request }) => {
  await ensureAttachedProject(request);
  const original = await jsonObject(await request.get("/api/quality"));
  const suffix = Date.now();
  const requirements = `Quality requirements ${suffix}`;
  const instructions = `Quality instructions ${suffix}`;

  await jsonObject(await request.patch("/api/quality", {
    data: {
      business_requirements: "",
      instructions: "Temporary quality instructions",
      enabled: "0",
      timing: "pre_merge",
      regressions_enabled: "0",
    },
  }));

  try {
    await page.goto("/#/project/quality");
    await expect(page.getByTestId("settings-pane-quality")).toHaveClass(/active/);
    await expect(page.getByTestId("quality-enabled-toggle")).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByTestId("quality-timing-select")).toHaveValue("pre_merge");
    await expect(page.getByTestId("quality-regressions-toggle")).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByTestId("quality-config-warning")).toContainText(
      "Quality can run once business requirements and instructions are both filled in.",
    );

    await page.getByTestId("s-quality-business-requirements-edit").click();
    await page.getByTestId("s-quality-business-requirements").fill(requirements);
    const requirementsSaved = page.waitForResponse((response) =>
      response.url().includes("/api/quality") &&
      response.request().method() === "PATCH" &&
      (response.request().postData() || "").includes(requirements) &&
      response.status() === 200
    );
    await page.getByTestId("s-quality-business-requirements-edit").click();
    await requirementsSaved;

    await page.getByTestId("s-quality-instructions-edit").click();
    await page.getByTestId("s-quality-instructions").fill(instructions);
    const instructionsSaved = page.waitForResponse((response) =>
      response.url().includes("/api/quality") &&
      response.request().method() === "PATCH" &&
      (response.request().postData() || "").includes(instructions) &&
      response.status() === 200
    );
    await page.getByTestId("s-quality-instructions-edit").click();
    await instructionsSaved;

    const saved = await jsonObject(await request.get("/api/quality"));
    expect(saved.configured).toBe(true);
    expect(saved.business_requirements).toBe(requirements);
    expect(saved.instructions).toBe(instructions);

    await page.reload();
    await expect(page.getByTestId("quality-config-warning")).toHaveCount(0);
    await expect(page.getByTestId("s-quality-business-requirements")).toHaveValue(requirements);
    await expect(page.getByTestId("s-quality-instructions")).toHaveValue(instructions);
  } finally {
    await request.patch("/api/quality", {
      data: {
        business_requirements: String(original.business_requirements ?? ""),
        instructions: String(original.instructions ?? ""),
        enabled: String(original.enabled ?? "0"),
        timing: String(original.timing ?? "pre_merge"),
        regressions_enabled: String(original.regressions_enabled ?? "0"),
      },
    });
  }
});
