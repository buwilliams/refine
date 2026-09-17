const { selectMain } = require("./support/web_app");
const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, SKIP } = require("./support/web_app");

test("Main opens a menu while screen rows persist, and works in a mobile drawer", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    const header = page.locator("#main-screen-menu > summary");
    const box = await header.boundingBox();
    assert.ok(box.height >= 44);
    await header.click({ position: { x: 60, y: 22 } });
    assert.equal(await page.locator('[data-testid="nav-dashboard"]').isVisible(), true);
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
    await page.locator('#main-screen-menu [data-main-open="features"]').click();
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
    await page.getByTestId("dashboard-scope-all").click();
    await page.waitForFunction(() => location.hash === "#/?node=all");
    await page.evaluate(() => {
      window.savedDashboard = document.getElementById("dash");
      savedDashboard.style.minHeight = "2200px";
      document.getElementById("main").scrollTop = 240;
    });
    await page.locator('[data-testid="toolbar-add"]').click();
    assert.deepEqual(await page.locator("[data-add-toolbar-tab]").allTextContents(),
      ["Agent", "Agent in Worktree", "System", "Files", "Terminal", "Planning Agent"]);
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
    await selectMain(page, "dashboard");
    await page.locator("#dash").waitFor();
    assert.equal(await page.evaluate(() => savedDashboard === document.getElementById("dash")), true);
    assert.equal(new URL(page.url()).hash, "#/?node=all");
    assert.equal(await page.locator("#main").evaluate(el => el.scrollTop), 240);
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
    await selectMain(page, "settings");
    await page.getByTestId("settings-pane-application").waitFor();
    assert.equal(await page.getByTestId("settings-tab-processes").count(), 0);
    await selectMain(page, "control");
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

test("New stays below Search and supports keyboard, dismissal, and viewport positioning", { skip: SKIP }, async () => {
  const writes = [];
  const app = await openApp({ onRequest(path, request) {
    if (request.method() === "POST") writes.push(path);
  }});
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    const toggle = page.locator("#rail-new-toggle");
    const menu = page.locator("#rail-new-menu");
    const panel = page.getByRole("menu", { name: "New", exact: true });
    const items = panel.getByRole("menuitem");
    assert.deepEqual(await page.locator(".rail-global > *").evaluateAll(elements =>
      elements.map(el => el.id || el.dataset.topbarPicker)),
    ["btn-command-palette", "rail-new-menu", "node", "reporter"]);
    assert.deepEqual(await page.locator("[id]").evaluateAll(elements => {
      const ids = elements.map(el => el.id);
      return ids.filter((id, index) => ids.indexOf(id) !== index);
    }), []);
    assert.equal(await toggle.getAttribute("aria-label"), "New");
    assert.equal(await toggle.getAttribute("title"), "New");
    assert.equal(await toggle.locator("use").getAttribute("href"), "/static/vendor/lucide/navigation.svg#plus");
    await page.locator("#main-screen-menu > summary").click();
    await toggle.focus();
    await page.keyboard.press("Enter");
    assert.deepEqual(await items.allTextContents(), ["New Goal", "New Plan", "New Feature", "Import"]);
    assert.equal(await items.nth(0).evaluate(el => el === document.activeElement), true);
    for (const [key, index] of [["ArrowDown", 1], ["End", 3], ["ArrowDown", 0], ["ArrowUp", 3], ["Home", 0]]) {
      await page.keyboard.press(key);
      assert.equal(await items.nth(index).evaluate(el => el === document.activeElement), true);
    }
    assert.notEqual(await items.first().evaluate(el => getComputedStyle(el).outlineStyle), "none");
    await page.keyboard.press("Escape");
    assert.equal(await toggle.evaluate(el => el === document.activeElement), true);
    await page.waitForFunction(() => document.getElementById("rail-new-toggle").getAttribute("aria-expanded") === "false");
    await page.keyboard.press("Space");
    await panel.waitFor();
    await page.keyboard.press("Tab");
    assert.equal(await menu.getAttribute("open"), null);
    assert.equal(await page.locator('[data-topbar-picker="node"] summary').evaluate(el => el === document.activeElement), true);
    // Keyboard opening also excludes a menu opened without a pointer click.
    await page.evaluate(() => { document.querySelector('[data-topbar-picker="node"]').open = true; });
    await page.locator('[data-topbar-picker="node"] .nav-menu-panel').waitFor();
    await toggle.focus();
    await page.keyboard.press("ArrowUp");
    assert.equal(await items.last().evaluate(el => el === document.activeElement), true);
    assert.equal(await page.locator(".navigation-rail .nav-menu[open]").count(), 1);
    await page.locator("#dash").click({ position: { x: 400, y: 20 } });
    assert.equal(await menu.getAttribute("open"), null);
    await page.getByTestId("toolbar-add").click();
    await toggle.click();
    assert.equal(await page.locator(".navigation-rail .nav-menu[open]").count(), 1);
    await page.getByTestId("toolbar-add").click();
    assert.equal(await menu.getAttribute("open"), null);
    for (const viewport of [{ width: 1280, height: 800 }, { width: 720, height: 300 }, { width: 320, height: 240 }]) {
      await page.setViewportSize(viewport);
      if (viewport.width <= 700) await page.locator("#mobile-rail-toggle").click();
      else if (viewport.width === 720) await page.locator("#rail-toggle").click();
      await toggle.click();
      await page.waitForFunction(() => {
        const box = document.getElementById("rail-new-options").getBoundingClientRect();
        return box.width > 0 && box.x >= 0 && box.y >= 0 && box.right <= innerWidth && box.bottom <= innerHeight;
      });
      const box = await panel.boundingBox();
      assert.ok(box.x >= 0 && box.y >= 0 && box.x + box.width <= viewport.width && box.y + box.height <= viewport.height, JSON.stringify(box));
      assert.equal(await items.last().isVisible(), true);
      if (viewport.width === 720) {
        assert.equal(await toggle.locator(".rail-copy").isVisible(), false);
        await page.locator("#rail-navigation").evaluate(el => { el.scrollTop = 32; });
        await page.waitForTimeout(50);
        assert.ok((await panel.boundingBox()).y >= 8);
      }
      if (viewport.width <= 700) {
        await page.setViewportSize({ width: viewport.width, height: 120 });
        await page.waitForFunction(() => {
          const panel = document.getElementById("rail-new-options");
          return panel.scrollHeight > panel.clientHeight && panel.getBoundingClientRect().bottom <= innerHeight;
        });
        await items.first().focus();
        await page.keyboard.press("End");
        const last = await items.last().boundingBox();
        assert.ok(last.y >= 0 && last.y + last.height <= 120);
        await page.setViewportSize(viewport);
      }
      await toggle.focus();
      await page.keyboard.press("Escape");
      assert.equal(await toggle.evaluate(el => el === document.activeElement), true);
      if (viewport.width <= 700) {
        assert.equal(await page.locator("#rail-scrim").isVisible(), true);
        assert.equal(await page.locator(".workspace").evaluate(el => el.inert), true);
      }
    }
    assert.deepEqual(writes, []);
    assert.equal(await page.evaluate(() => Object.keys(chatState.tabs).length), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

for (const theme of ["light", "dark"]) for (const layout of ["desktop", "collapsed", "mobile"]) {
  test(`New chevron and menu icons remain aligned and readable (${layout}, ${theme})`, { skip: SKIP }, async () => {
    const app = await openApp();
    try {
      const { page } = app;
      const viewport = layout === "mobile" ? { width: 390, height: 844 } : { width: 1280, height: 900 };
      await page.setViewportSize(viewport);
      await page.emulateMedia({ colorScheme: theme });
      await page.goto(app.origin, { waitUntil: "networkidle" });
      assert.equal(await page.locator("html").getAttribute("data-theme"), theme);
      if (layout === "collapsed") await page.locator("#rail-toggle").click();
      if (layout === "mobile") await page.locator("#mobile-rail-toggle").click();
      const chevrons = await page.locator('#rail-new-toggle .nav-context-more, [data-topbar-picker] .nav-context-more, [data-testid="toolbar-add"] .nav-context-more').evaluateAll(elements => elements.map(el => {
        const style = getComputedStyle(el), arrow = getComputedStyle(el, "::after");
        const row = el.parentElement.getBoundingClientRect(), box = el.getBoundingClientRect();
        return { display: style.display, rightInset: row.right - box.right,
          arrow: [arrow.content, arrow.width, arrow.height, arrow.borderRight, arrow.borderBottom, arrow.transform, arrow.color] };
      }));
      assert.equal(chevrons.length, 4);
      for (const chevron of chevrons) {
        assert.deepEqual(chevron.arrow, chevrons[0].arrow);
        assert.equal(chevron.display === "none", layout === "collapsed");
        if (layout !== "collapsed") assert.ok(Math.abs(chevron.rightInset - chevrons[0].rightInset) < 2);
      }
      await page.locator("#rail-new-toggle").click();
      const panel = page.getByRole("menu", { name: "New", exact: true });
      await panel.waitFor();
      const panelBox = await panel.boundingBox();
      assert.ok(panelBox.x >= 0 && panelBox.y >= 0 && panelBox.x + panelBox.width <= viewport.width && panelBox.y + panelBox.height <= viewport.height);
      const items = panel.getByRole("menuitem");
      const labels = ["New Goal", "New Plan", "New Feature", "Import"];
      let previousIconX, previousTextX;
      for (let i = 0; i < labels.length; i++) {
        const item = panel.getByRole("menuitem", { name: labels[i], exact: true });
        assert.equal(await item.isVisible(), true);
        const geometry = await item.evaluate(el => {
          const svg = el.querySelector("svg"), icon = svg.getBoundingClientRect();
          const ink = svg.querySelector("use").getBBox();
          const range = document.createRange();
          range.selectNodeContents(el.lastChild);
          const label = range.getBoundingClientRect(), row = el.getBoundingClientRect();
          return { icon: icon.toJSON(), label: label.toJSON(), row: row.toJSON(), ink: { width: ink.width, height: ink.height },
            hidden: svg.getAttribute("aria-hidden"), focusable: svg.getAttribute("focusable"),
            stroke: getComputedStyle(svg).stroke, color: getComputedStyle(el).color };
        });
        const { icon, label, row, ink } = geometry;
        assert.ok(ink.width > 0 && ink.height > 0, "sprite symbol must render");
        assert.equal(geometry.hidden, "true");
        assert.equal(geometry.focusable, "false");
        assert.equal(geometry.stroke, geometry.color);
        assert.ok(icon.width >= 16 && icon.height >= 16);
        assert.ok(label.width > 0 && label.height > 0 && label.right <= row.right);
        assert.ok(label.left > icon.right, "label must have a gap after its icon");
        assert.ok(Math.abs((icon.top + icon.bottom) / 2 - (label.top + label.bottom) / 2) < 2);
        if (i > 0) {
          assert.equal(icon.x, previousIconX);
          assert.equal(label.x, previousTextX);
        }
        previousIconX = icon.x;
        previousTextX = label.x;
      }
      assert.equal(await items.count(), 4);
      if (process.env.REFINE_NEW_MENU_SCREENSHOTS) {
        await page.screenshot({ path: `${process.env.REFINE_NEW_MENU_SCREENSHOTS}/${layout}-${theme}.png` });
      }
      await page.keyboard.press("Escape");
      assert.equal(await panel.isVisible(), false);
      assert.equal(await page.locator("#rail-new-toggle").evaluate(el => el === document.activeElement), true);
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });
}

for (const layout of ["desktop", "collapsed", "mobile"]) test(`New opens shared creation flows and fresh Plans (${layout})`, { skip: SKIP }, async () => {
  const mobile = layout === "mobile";
  let starts = 0;
  let activeNode = "node-a";
  const stops = [], activations = [];
  const nodes = [{ id: "node-a", display_name: "Node A" }, { id: "node-b", display_name: "Node B" }];
  const app = await openApp({ fixture(path, request) {
    if (path === "/api/nodes/activate") {
      activeNode = request.postDataJSON().node_id;
      activations.push(activeNode);
      return { ok: true };
    }
    if (path === "/api/project/status" || path === "/api/nodes") return { ...apiFixture(path), nodes, active_node_id: activeNode };
    if (path === "/api/reporters") return { reporters: [{ name: "Reporter" }, { name: "Selected Reporter" }] };
    if (path === "/api/terminal/session") return { id: `plan-${++starts}`, process_id: `process-${starts}`, cwd: "/workspace", provider: "codex" };
    if (path.startsWith("/api/terminal/")) {
      if (path.endsWith("/stop")) stops.push(path);
      if (path.endsWith("/status")) return { alive: true };
      return { ok: true, output: "", entries: [] };
    }
    return apiFixture(path);
  }});
  try {
    const { page } = app;
    if (mobile) await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    if (mobile) await page.locator("#mobile-rail-toggle").click();
    await page.locator('[data-topbar-picker="node"] summary').click();
    await page.getByRole("option", { name: "Node B", exact: true }).click();
    await page.waitForFunction(() => state.project.active_node_id === "node-b" && !nodeContextSwitchPromise);
    await page.locator('[data-topbar-picker="reporter"] summary').click();
    await page.getByRole("option", { name: "Selected Reporter", exact: true }).click();
    if (mobile) await page.locator("#rail-toggle").click();
    if (layout === "collapsed") await page.locator("#rail-toggle").click();
    const context = await page.evaluate(() => ({ node: state.project.active_node_id, reporter: state.lastReporter }));
    assert.deepEqual(context, { node: "node-b", reporter: "Selected Reporter" });
    await page.evaluate(() => {
      window.newCommands = [];
      const original = runCommand;
      runCommand = (id, options) => {
        newCommands.push({ id, inert: document.querySelector(".workspace").inert,
          drawer: document.getElementById("navigation-rail").classList.contains("mobile-open"),
          menus: document.querySelectorAll(".navigation-rail .nav-menu[open]").length });
        return original(id, options);
      };
    });
    async function select(name) {
      if (mobile) await page.locator("#mobile-rail-toggle").click();
      await page.locator("#rail-new-toggle").click();
      const item = page.getByRole("menuitem", { name, exact: true });
      if (mobile) {
        await item.focus();
        await page.keyboard.press(name === "New Plan" ? "Space" : "Enter");
      } else if (layout === "collapsed") await item.locator("svg").click();
      else {
        const position = await item.evaluate(el => {
          const range = document.createRange();
          range.selectNodeContents(el.lastChild);
          const label = range.getBoundingClientRect(), row = el.getBoundingClientRect();
          return { x: label.x + label.width / 2 - row.x, y: label.y + label.height / 2 - row.y };
        });
        await item.click({ position });
      }
      assert.equal(await page.locator("#rail-new-menu").getAttribute("open"), null);
    }
    await select("New Goal");
    await page.getByTestId("new-goal-modal").waitFor();
    assert.equal(await page.getByTestId("new-goal-modal").locator(".js-reporter-name").textContent(), context.reporter);
    assert.equal(await page.getByTestId("new-goal-prompt").evaluate(el => el === document.activeElement), true);
    const submissions = [];
    await page.route("**/api/goals", async route => {
      if (route.request().method() !== "POST") return route.fallback();
      submissions.push(route.request().postDataJSON());
      await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({ error: { message: "Creation temporarily unavailable" } }) });
    });
    await page.getByTestId("new-goal-submit").click();
    await page.getByText("Provide a prompt", { exact: true }).waitFor();
    assert.deepEqual(submissions, []);
    await page.getByTestId("new-goal-prompt").fill("Keep this draft");
    await page.getByTestId("new-goal-submit").click();
    await page.getByText("Creation temporarily unavailable", { exact: true }).waitFor();
    assert.equal(await page.getByTestId("new-goal-prompt").inputValue(), "Keep this draft");
    assert.deepEqual(submissions, [{ reporter: "Selected Reporter", prompt: "Keep this draft", priority: "low", duplicate_decision: "" }]);
    await page.keyboard.press("Escape");
    await page.getByTestId("modal-cancel").click();
    assert.equal(await page.getByTestId("new-goal-prompt").inputValue(), "Keep this draft");
    await page.keyboard.press("Escape");
    await page.getByTestId("modal-ok").click();
    await select("New Feature");
    await page.getByTestId("feature-create-modal").waitFor();
    assert.equal(await page.getByTestId("feature-reporter").inputValue(), context.reporter);
    await page.getByTestId("feature-modal-close").click();
    await select("Import");
    await page.getByTestId("import-modal").waitFor();
    assert.equal(await page.getByTestId("import-modal").locator(".js-reporter-name").first().textContent(), context.reporter);
    await page.keyboard.press("Escape");
    for (let count = 1; count <= 2; count++) {
      await select("New Plan");
      await page.waitForFunction(count => Object.values(chatState.tabs).filter(tab => tab.mode === "plan").length === count && terminalStateFor()?.connected, count);
      assert.equal(starts, count);
    }
    const plans = await page.evaluate(() => Object.entries(chatState.tabs).map(([id]) => ({ id, session: terminalStateFor(id)?.sessionId })));
    assert.equal(new Set(plans.map(tab => tab.id)).size, 2);
    assert.equal(new Set(plans.map(tab => tab.session)).size, 2);
    assert.deepEqual(stops, []);
    assert.deepEqual(activations, ["node-b"]);
    assert.deepEqual(await page.evaluate(() => ({ node: state.project.active_node_id, reporter: state.lastReporter })), context);
    assert.deepEqual(await page.evaluate(() => newCommands), ["goal.new", "feature.new", "goal.import", "plan.open", "plan.open"].map(id => ({ id, inert: false, drawer: false, menus: 0 })));
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

for (const theme of ["light", "dark"]) test(`Main retains interacted Dashboard, filters, and scroll across screens (${theme})`, { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator(".dashboard-status-grid").waitFor();
    await page.evaluate(theme => { document.documentElement.dataset.theme = theme; }, theme);
    await page.getByTestId("dashboard-scope-all").click();
    await page.waitForFunction(() => location.hash === "#/?node=all");
    await page.evaluate(() => {
      window.savedMain = document.getElementById("main");
      window.savedDashboard = document.getElementById("dash");
      const panel = savedDashboard.querySelector("details");
      if (panel) { panel.open = true; window.savedPanel = panel; }
      // Make scroll deterministic independently of fixture row count.
      savedDashboard.style.minHeight = "2200px";
      savedMain.scrollTop = 320;
    });
    await selectMain(page, "goals");
    await page.getByTestId("goals-table").waitFor();
    await page.evaluate(() => {
      window.savedGoals = document.getElementById("main");
      document.getElementById("goals-filter-shell").open = true;
      goalsExcludedIds.add("GOAL1");
    });
    await selectMain(page, "dashboard");
    await page.locator("#dash").waitFor();
    assert.equal(new URL(page.url()).hash, "#/?node=all");
    assert.deepEqual(await page.evaluate(() => ({ same: savedMain === document.getElementById("main"), dash: savedDashboard === document.getElementById("dash"), scroll: savedMain.scrollTop, panel: !window.savedPanel || savedPanel.open })), { same: true, dash: true, scroll: 320, panel: true });
    await page.getByTestId("main-menu").click();
    await page.getByRole("menuitem", { name: "Dashboard", exact: true }).click();
    assert.equal(await page.getByTestId("nav-dashboard").count(), 1);
    await selectMain(page, "goals");
    await page.getByTestId("goals-table").waitFor();
    assert.equal(await page.evaluate(() => savedGoals === document.getElementById("main") && goalsExcludedIds.has("GOAL1") && document.getElementById("goals-filter-shell").open), true);
    await page.evaluate(() => { location.hash = "#/goals?status=review&node=current"; });
    await page.waitForFunction(() => document.getElementById("filter-status")?.value === "review");
    await selectMain(page, "dashboard");
    await selectMain(page, "goals");
    await page.getByTestId("goals-table").waitFor();
    assert.equal(new URL(page.url()).hash, "#/goals?status=review&node=current");
    await page.getByRole("button", { name: "Close Goals", exact: true }).click();
    await page.locator("#dash").waitFor();
    assert.equal(await page.getByTestId("nav-dashboard").evaluate(el => el === document.activeElement), true);
    await selectMain(page, "goals");
    await page.getByTestId("goals-table").waitFor();
    assert.equal(await page.evaluate(() => savedGoals === document.getElementById("main")), false);
    assert.equal(await page.evaluate(() => goalsExcludedIds.size), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Settings draft and tab survive Control, command search, and cancelled close", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(`${app.origin}/#/settings/target-app`);
    await page.getByTestId("settings-pane-target-app").waitFor();
    await page.getByTestId("s-target-start-instructions-edit").click();
    const input = page.locator('#main textarea').first();
    await input.fill("Retain this unsaved instruction");
    await page.evaluate(() => { window.savedSettings = document.getElementById("main"); });
    await selectMain(page, "control");
    await page.getByTestId("process-manager-table").waitFor();
    await page.keyboard.press("Control+k");
    await page.locator("#command-palette-input").fill("Settings");
    await page.locator('[data-command-id="nav.settings"]').click();
    await page.getByTestId("settings-pane-target-app").waitFor();
    assert.equal(new URL(page.url()).hash, "#/settings/target-app");
    assert.equal(await input.inputValue(), "Retain this unsaved instruction");
    assert.equal(await page.evaluate(() => savedSettings === document.getElementById("main")), true);
    await page.getByTestId("settings-tab-reporters").click();
    await page.getByTestId("settings-pane-reporters").waitFor();
    await page.waitForFunction(() => document.querySelector('[data-tab-pane="reporters"] .settings-tab-card')?.textContent.trim());
    await page.getByTestId("settings-tab-target-app").click();
    await page.getByTestId("settings-pane-target-app").waitFor();
    assert.equal(await input.inputValue(), "Retain this unsaved instruction");
    await page.getByRole("button", { name: "Close Settings", exact: true }).click();
    await page.getByTestId("modal-cancel").click();
    assert.equal(await input.inputValue(), "Retain this unsaved instruction");
    await page.getByRole("button", { name: "Close Settings", exact: true }).click();
    await page.getByTestId("modal-ok").click();
    await page.getByTestId("process-manager-table").waitFor();
    assert.equal(await page.getByTestId("nav-settings").count(), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Main menu keyboard dismissal restores focus and leaves open screens visible", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    const toggle = page.getByTestId("main-menu");
    await toggle.focus();
    await page.keyboard.press("ArrowDown");
    assert.equal(await page.getByRole("menuitem", { name: "Dashboard", exact: true }).evaluate(el => el === document.activeElement), true);
    await page.keyboard.press("End");
    await page.keyboard.press("Escape");
    assert.equal(await toggle.evaluate(el => el === document.activeElement), true);
    assert.equal(await page.getByTestId("nav-dashboard").isVisible(), true);
    await toggle.click();
    await page.locator("#dash").click();
    await page.waitForFunction(() => document.querySelector('[data-testid="main-menu"]').getAttribute("aria-expanded") === "false");
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("late Goals response cannot paint a reopened screen or another Main screen", { skip: SKIP }, async () => {
  const app = await openApp();
  let release;
  try {
    const { page } = app;
    await page.goto(app.origin);
    await page.locator("#dash").waitFor();
    let pending;
    await page.route("**/api/goals?**", async route => {
      if (!route.request().url().includes("exclude_draft")) return route.fallback();
      if (!pending) {
        pending = route;
        await new Promise(resolve => { release = resolve; });
        return route.fulfill({ json: { goals: [{ id: "OLD", name: "Superseded response", status: "todo" }], facets: {}, page: {} } });
      }
      return route.fallback();
    });
    await selectMain(page, "goals");
    await page.waitForFunction(() => document.getElementById("goals-table"));
    while (!release) await new Promise(resolve => setTimeout(resolve, 10));
    await selectMain(page, "features");
    await page.locator("#features-table").waitFor();
    await page.getByRole("button", { name: "Close Goals", exact: true }).click();
    await selectMain(page, "goals");
    await page.getByText("Smoke goal", { exact: true }).first().waitFor();
    release();
    await page.waitForTimeout(150);
    assert.equal(await page.getByText("Superseded response").count(), 0);
    assert.equal(await page.locator("#main").count(), 1);
    assert.deepEqual(app.pageErrors, []);
  } finally { release?.(); await app.close(); }
});

test("context changes invalidate clean screens and protect an inactive Settings draft", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(`${app.origin}/#/settings/target-app`);
    await page.getByTestId("settings-pane-target-app").waitFor();
    await page.getByTestId("s-target-start-instructions-edit").click();
    await page.locator('#main textarea').first().fill("Prior target draft");
    await selectMain(page, "dashboard");
    await page.locator("#dash").waitFor();
    await page.evaluate(() => { window.priorDashboard = document.getElementById("dash"); });
    const dirty = await page.evaluate(() => nodeContextDirtySurfaces().map(item => item.label));
    assert.ok(dirty.includes("Settings"));
    await page.evaluate(() => { window.contextSwitch = activateNodeContext("node-b"); });
    await page.getByTestId("modal-cancel").click();
    assert.equal(await page.evaluate(() => contextSwitch), false);
    assert.equal(await page.evaluate(() => mainScreens.open.get("settings").dirty), true);
    // Use the authoritative coordinator as SSE target-root changes do.
    await page.evaluate(async () => {
      await applyAuthoritativeNodeContext({ ...state.project, target_root: "/another-app" }, { nodes: state.project.nodes }, { changed: true, external: true });
    });
    await page.locator(".dashboard-status-grid").waitFor();
    assert.equal(await page.evaluate(() => priorDashboard === document.getElementById("dash")), false);
    await selectMain(page, "settings");
    await page.getByTestId("settings-pane-target-app").waitFor();
    assert.equal(await page.locator('#main textarea').first().inputValue(), "Prior target draft");
    assert.equal(await page.locator('#main textarea').first().isDisabled(), true);
    assert.equal(await page.getByTestId("node-context-stale-warning").first().isVisible(), true);
    await page.evaluate(async () => {
      await applyAuthoritativeNodeContext({ ...state.project, target_root: "/third-app" }, { nodes: state.project.nodes }, { changed: true });
    });
    await page.getByTestId("settings-pane-target-app").waitFor();
    assert.equal(await page.getByTestId("node-context-stale-warning").count(), 0);
    assert.equal(await page.locator('#main textarea').first().isDisabled(), false);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("Goal and Feature modal dismissal restores the owning retained screen", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(`${app.origin}/#/?node=all`);
    await page.locator(".dashboard-status-grid").waitFor();
    await page.evaluate(() => { window.owner = document.getElementById("main"); });
    for (const [hash, marker] of [["#/goals/GOAL1", ".goal-detail-modal"], ["#/features/FEAT1", ".feature-modal"]]) {
      await page.evaluate(hash => { location.hash = hash; }, hash);
      await page.locator(marker).waitFor();
      await page.locator(`${marker} .modal-close`).click();
      await page.waitForFunction(() => location.hash === "#/?node=all");
      assert.equal(await page.evaluate(() => owner === document.getElementById("main")), true);
      assert.equal(await page.locator('.rail-main [aria-current="page"]').count(), 1);
    }
    await selectMain(page, "features");
    await page.locator("#features-table").waitFor();
    await page.goBack();
    await page.locator("#dash").waitFor();
    assert.equal(await page.evaluate(() => owner === document.getElementById("main")), true);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test("a clean Settings screen reconstructs after an authoritative Node change", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    const { page } = app;
    await page.goto(`${app.origin}/#/settings/application`);
    await page.getByTestId("settings-pane-application").waitFor();
    await page.evaluate(async () => {
      await applyAuthoritativeNodeContext({ ...state.project, active_node_id: "node-b" }, { nodes: state.project.nodes }, { changed: true, surfacesPrepared: true });
    });
    await page.getByTestId("settings-pane-application").waitFor();
    assert.equal(await page.locator("#main > h2").textContent(), "Settings");
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
