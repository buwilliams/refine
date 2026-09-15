const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, SKIP } = require("./support/web_app");

test("Main collapses independently of Tools, persists, and works in a mobile drawer", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    const header = page.locator("#rail-main-section > summary");
    const box = await header.boundingBox();
    assert.ok(box.height >= 44);
    await header.click({ position: { x: 60, y: 22 } });
    assert.equal(await page.locator('[data-testid="nav-dashboard"]').isVisible(), false);
    assert.equal(await page.locator("#dash").isVisible(), true);
    assert.equal(await page.getByTestId("toolbar-add").isVisible(), true);
    await page.locator("#rail-toggle").click();
    assert.equal(await page.locator("#navigation-rail").evaluate(el => el.offsetWidth), 64);
    await page.reload();
    await page.locator("#dash").waitFor();
    assert.equal(await page.locator("#rail-toggle").getAttribute("aria-expanded"), "false");
    assert.equal(await page.locator("#rail-main-section").getAttribute("open"), null);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.locator("#mobile-rail-toggle").click();
    assert.equal(await page.locator("#rail-scrim").isVisible(), true);
    await header.click();
    await page.locator('[data-testid="nav-features"]').click();
    assert.equal(await page.locator("#rail-scrim").isVisible(), false);
    assert.equal(await page.evaluate(() => document.body.scrollWidth <= innerWidth), true);
    await page.locator("#mobile-rail-toggle").click();
    await page.locator("#btn-command-palette").click();
    await page.locator('[data-testid="command-palette"]').waitFor();
    await page.keyboard.press("Escape");
    assert.equal(await page.locator("#mobile-rail-toggle").evaluate(el => document.activeElement === el), true);
    assert.equal(await page.locator(".workspace").evaluate(el => el.inert), false);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("windows use history and full-height content without stopping sessions or losing the main screen", { skip: SKIP }, async () => {
  let starts = 0;
  const stops = [], inputs = [];
  const app = await openApp({ fixture(path, request) {
    if (path === "/api/terminal/session") {
      starts++;
      return { id: `session-${starts}`, process_id: `process-${starts}`, cwd: "/workspace", provider: "codex" };
    }
    if (path.startsWith("/api/terminal/")) {
      if (path.endsWith("/input")) inputs.push(request.postDataJSON().data);
      if (path.endsWith("/stop")) stops.push(path);
      if (path.endsWith("/status")) return { alive: true };
      return { ok: true, output: "", entries: [], termination: { confirmed_exit: true } };
    }
    if (path.startsWith("/api/files/")) return { entries: [], path: "" };
    return apiFixture(path);
  }});
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    await page.evaluate(() => { window.savedDashboard = document.getElementById("dash"); });
    await page.locator('[data-testid="toolbar-add"]').click();
    assert.deepEqual(await page.locator("[data-add-toolbar-tab]").allTextContents(),
      ["Agent", "Agent in Worktree", "System", "Files", "Todo List", "Terminal", "Planning Agent"]);
    await page.locator('[data-add-toolbar-tab="terminal"]').click();
    await page.locator(".xterm-screen").waitFor();
    await page.waitForFunction(() => terminalStateFor()?.connected);
    assert.match(page.url(), /#\/windows\//);
    assert.equal(await page.locator("#main").isVisible(), false);
    assert.ok((await page.locator("#toolbar-dock").boundingBox()).height > 600);
    assert.equal(await page.locator('[data-testid="toolbar-resize"], [data-testid="toolbar-fullscreen"]').count(), 0);
    await page.evaluate(() => terminalStateFor().term.focus());
    await page.keyboard.press("Control+k");
    await page.locator('[data-testid="command-palette"]').waitFor();
    await page.keyboard.press("Escape");
    assert.equal(await page.evaluate(() => document.activeElement === terminalStateFor().term.textarea), true);
    assert.deepEqual(inputs, []);
    await page.locator('[data-testid="nav-dashboard"]').click();
    await page.locator("#dash").waitFor();
    assert.equal(await page.evaluate(() => savedDashboard === document.getElementById("dash")), true);
    assert.equal(await page.locator('.navigation-rail [aria-current="page"]').count(), 1);
    await page.goBack();
    await page.locator(".xterm-screen").waitFor();
    assert.equal(starts, 1);
    await page.reload();
    await page.locator(".xterm-screen").waitFor();
    await page.waitForFunction(() => terminalStateFor()?.statusChecked);
    assert.equal(starts, 1);
    assert.deepEqual(stops, []);
    await page.locator('[data-testid="toolbar-add"]').click();
    await page.locator('[data-add-toolbar-tab="files"]').click();
    await page.waitForFunction(() => currentToolbarTab()?.mode === "files");
    await page.getByTestId("toolbar-add").click();
    assert.equal(await page.locator('#rail-windows a').filter({ hasText: "Terminal" }).isVisible(), true);
    await page.getByTestId("toolbar-add").click();
    assert.equal(await page.locator('#rail-windows a').filter({ hasText: "Terminal" }).isVisible(), true);
    await page.locator('#rail-windows a').filter({ hasText: "Terminal" }).click();
    await page.waitForFunction(() => currentToolbarTab()?.mode === "terminal");
    await page.locator('#rail-windows .rail-window-row.active').hover();
    await page.getByRole("button", { name: "Close and stop Terminal", exact: true }).click();
    await page.waitForFunction(() => currentToolbarTab()?.mode === "files");
    assert.deepEqual(stops, ["/api/terminal/session-1/stop"]);
    assert.equal(starts, 1);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Search opens over a modal and Escape restores the original dialog", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    await page.evaluate(() => { void modalPrompt("Example", { title: "Keep this dialog" }); });
    const input = page.locator('[data-testid="modal-input"]');
    await input.fill("Unsaved content");
    await page.keyboard.press("Control+k");
    await page.locator('[data-testid="command-palette"]').waitFor();
    await page.keyboard.press("Escape");
    assert.equal(await input.inputValue(), "Unsaved content");
    assert.equal(await input.evaluate(el => document.activeElement === el), true);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Control owns process management, preserves legacy links, and builds the target app with icon actions", { skip: SKIP }, async () => {
  const writes = [];
  let targetState = "stopped";
  const target = () => ({ state: targetState, has_start_action: true, has_stop_action: true, has_build_action: true });
  const app = await openApp({ fixture(path, request) {
    if (path === "/api/processes") return { target_app: target(), processes: [] };
    if (path === "/api/target-app/status") return target();
    if (path === "/api/target-app/build") {
      writes.push(path);
      targetState = "building";
      return { queued: true };
    }
    return apiFixture(path);
  }});
  try {
    const { page } = app;
    await page.goto(`${app.origin}/#/settings/processes`);
    await page.getByTestId("process-manager-table").waitFor();
    assert.equal(new URL(page.url()).hash, "#/control");
    assert.equal(await page.locator("#main > h2").textContent(), "Control");
    assert.equal(await page.getByTestId("nav-control").getAttribute("aria-current"), "page");
    assert.equal(await page.locator("#settings-tabs").count(), 0);
    const build = page.getByRole("button", { name: "Build target app", exact: true });
    assert.equal(await build.textContent(), "");
    assert.equal(await build.locator("svg use").getAttribute("href"), "/static/vendor/lucide/navigation.svg#hammer");
    await page.getByRole("button", { name: "Start target app", exact: true }).waitFor();
    await page.getByRole("button", { name: "Check target app status", exact: true }).waitFor();
    await build.click();
    await page.getByRole("button", { name: "Build", exact: true }).click();
    await page.waitForFunction(() => document.getElementById("s-target-run-build")?.disabled && document.getElementById("s-target-run-build")?.getAttribute("aria-label") === "Build target app");
    assert.deepEqual(writes, ["/api/target-app/build"]);
    assert.equal(await build.locator("svg").count(), 1);
    assert.match(await page.locator('[data-process-id="target-app"] [data-process-details]').textContent(), /building/i);
    await page.waitForFunction(() => systemOperationState.messages.some(message => message.message === "Build target app completed"));
    targetState = "running";
    await page.evaluate(() => applyTargetAppSnapshot({ state: "running", has_start_action: true, has_stop_action: true, has_build_action: true }));
    assert.equal(await build.isEnabled(), true);
    await page.getByRole("button", { name: "Stop target app", exact: true }).waitFor();
    await page.keyboard.press("Control+k");
    await page.locator("#command-palette-input").fill("Control");
    assert.ok(await page.getByTestId("command-palette").getByText("Control", { exact: true }).count());
    await page.keyboard.press("Escape");
    await page.getByTestId("nav-settings").click();
    await page.getByTestId("settings-pane-application").waitFor();
    assert.equal(await page.getByTestId("settings-tab-processes").count(), 0);
    await page.getByTestId("nav-control").click();
    await page.getByTestId("process-manager-table").waitFor();
    await page.reload();
    await page.getByTestId("process-manager-table").waitFor();
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Skills and Hubs retain independent section preferences and open their management screens", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.setViewportSize({ width: 1280, height: 1100 });
    await page.goto(app.origin);
    await page.getByTestId("rail-manage-skills").waitFor();
    await page.getByTestId("rail-manage-hubs").waitFor();
    for (const name of ["skills", "hubs"]) await page.locator(`#rail-${name}-section > summary`).click();
    await page.waitForFunction(() => ["rail-skills-section", "rail-hubs-section"].every(id => localStorage.getItem(id) === "false"));
    await page.reload();
    await page.locator("#dash").waitFor();
    for (const name of ["skills", "hubs"]) assert.equal(await page.locator(`#rail-${name}-section`).getAttribute("open"), null);
    await page.locator("#rail-skills-section > summary").click();
    await page.getByTestId("rail-manage-skills").click();
    await page.getByTestId("settings-templates").waitFor();
    assert.equal(await page.locator("#rail-skills-section").getAttribute("open"), "");
    assert.equal(await page.locator("#rail-hubs-section").getAttribute("open"), null);
    await page.locator("#rail-hubs-section > summary").click();
    await page.getByTestId("rail-manage-hubs").click();
    await page.getByTestId("settings-pane-hubs").waitFor();
    assert.equal(new URL(page.url()).hash, "#/settings/hubs");
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("context menus identify their context, align with their rows, and create a Node from the rail", { skip: SKIP }, async () => {
  const created = [];
  const registry = apiFixture("/api/nodes");
  const app = await openApp({ fixture(path, request) {
    if (path === "/api/nodes") {
      if (request.method() === "POST") {
        const node = { id: "new-node", ...request.postDataJSON() };
        created.push(node.display_name);
        registry.nodes.push(node);
        return { node };
      }
      return registry;
    }
    return apiFixture(path);
  }});
  try {
    const { page } = app;
    await page.setViewportSize({ width: 1280, height: 1200 });
    await page.goto(app.origin, { waitUntil: "networkidle" });
    for (const name of ["node", "reporter"]) {
      const menu = page.locator(`[data-topbar-picker="${name}"]`);
      await menu.locator("summary").click();
      const panel = menu.locator(".nav-menu-panel");
      await panel.waitFor();
      assert.equal(await panel.locator(".rail-picker-heading").textContent(), name === "node" ? "Node" : "Reporter");
      await page.waitForFunction(name => document.querySelector(`[data-topbar-picker="${name}"] .nav-menu-panel`).style.top !== "", name);
      const row = await menu.locator("summary").boundingBox(), box = await panel.boundingBox();
      assert.ok(Math.abs(row.y - box.y) < 2);
      if (name === "reporter") assert.equal(await panel.getByRole("option", { name: /Add new reporter/ }).count(), 1);
      await page.keyboard.press("Escape");
    }
    await page.locator('[data-topbar-picker="node"] summary').click();
    await page.getByRole("button", { name: "Add Node…", exact: true }).click();
    await page.getByTestId("modal-input").fill("Review server");
    await page.getByTestId("modal-ok").click();
    await page.waitForFunction(() => [...document.querySelectorAll("#global-node option")].some(option => option.textContent === "Review server"));
    assert.deepEqual(created, ["Review server"]);
    assert.equal(new URL(page.url()).hash, "");
    assert.equal(await page.locator("#dash").isVisible(), true);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
