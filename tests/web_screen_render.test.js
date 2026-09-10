const assert = require("node:assert/strict");
const test = require("node:test");
const { openApp, apiFixture, GOAL, FEATURE, SKIP } = require("./support/web_app");

test("Reporter onboarding selects an existing Reporter before routed modal navigation", { skip: SKIP }, async () => {
  const app = await openApp({ selectedReporter: null });
  try {
    await app.page.goto(`${app.origin}/#/goals/new`);
    const onboarding = app.page.locator('[data-testid="reporter-onboarding-dialog"]');
    await onboarding.waitFor();
    assert.equal(await onboarding.getAttribute("aria-labelledby"), "reporter-onboarding-title");
    assert.match(await onboarding.innerText(), /Who are you\?/);
    assert.match(await onboarding.innerText(), /Controls > Reporter/);

    await onboarding.getByRole("button", { name: "Reporter", exact: true }).click();
    await app.page.locator('[data-testid="new-goal-modal"]').waitFor();
    assert.equal(await onboarding.count(), 0);
    assert.equal(await app.page.locator('[aria-modal="true"]').count(), 1);
    assert.equal(
      await app.page.evaluate(() => localStorage.getItem("refine_last_reporter")),
      "Reporter",
    );

    await app.page.reload();
    await app.page.locator('[data-testid="new-goal-modal"]').waitFor();
    assert.equal(await onboarding.count(), 0, "a persisted valid selection suppresses reload onboarding");
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Reporter onboarding creates and persists the canonical Reporter", { skip: SKIP }, async () => {
  const requests = [];
  const reporters = [{ id: 1, name: "Reporter" }];
  const app = await openApp({
    selectedReporter: null,
    async fixture(pathname, request) {
      if (pathname === "/api/reporters" && request.method() === "POST") {
        const body = request.postDataJSON();
        const reporter = { id: 42, name: body.name.trim() };
        reporters.push(reporter);
        return { reporter };
      }
      if (pathname === "/api/reporters") return { reporters };
      return apiFixture(pathname);
    },
    onRequest(pathname, request) {
      if (pathname === "/api/reporters") requests.push([request.method(), pathname]);
    },
  });
  try {
    await app.page.goto(`${app.origin}/#/`);
    const onboarding = app.page.locator('[data-testid="reporter-onboarding-dialog"]');
    await onboarding.waitFor();
    await onboarding.locator("#reporter-onboarding-name").fill("Grace Hopper");
    await onboarding.locator('[data-testid="reporter-onboarding-create"]').click();
    await onboarding.waitFor({ state: "detached" });

    assert.equal(
      await app.page.evaluate(() => localStorage.getItem("refine_last_reporter")),
      "Grace Hopper",
    );
    assert.deepEqual(requests.slice(-2), [
      ["POST", "/api/reporters"],
      ["GET", "/api/reporters"],
    ]);
    await app.page.reload();
    await app.page.locator("#dash").waitFor();
    assert.equal(await onboarding.count(), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("asynchronous Feature detail remains the only modal and onboarding safely resumes", { skip: SKIP }, async () => {
  const app = await openApp({
    selectedReporter: null,
    async fixture(pathname) {
      if (pathname === "/api/features/FEAT1") {
        await new Promise((resolve) => setTimeout(resolve, 700));
        return { feature: FEATURE };
      }
      return apiFixture(pathname);
    },
  });
  try {
    await app.page.goto(`${app.origin}/#/features/FEAT1`);
    const onboarding = app.page.locator('[data-testid="reporter-onboarding-dialog"]');
    await onboarding.waitFor();
    await app.page.locator('[data-testid="feature-detail-modal"]').waitFor();
    assert.equal(await onboarding.count(), 0);
    assert.equal(await app.page.locator('[aria-modal="true"]').count(), 1);
    assert.equal(
      await app.page.evaluate(() => document.activeElement?.closest('[data-testid="feature-detail-modal"]') !== null),
      true,
    );

    await app.page.locator('[data-testid="feature-modal-close"]').click();
    await onboarding.waitFor();
    assert.equal(await app.page.locator('[aria-modal="true"]').count(), 1);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

// A screen that throws while rendering is caught by its own error handler and
// replaced with a placeholder, which is why the marker has to be asserted: the
// throw produces no page error at all. That is precisely how the goal-detail
// regression looked — every existing test green, screen entirely broken.
async function assertScreenRenders(app, { route, marker, forbiddenText }) {
  const before = app.pageErrors.length;
  await app.page.goto(`${app.origin}/${route}`);
  let rendered = true;
  try {
    await app.page.waitForSelector(marker, { timeout: 10000 });
  } catch {
    rendered = false;
  }
  const body = await app.page.evaluate(() => document.body.innerText.slice(0, 400));
  assert.ok(
    rendered,
    `${route} did not render ${marker}. Visible text was:\n${body}`,
  );
  // The failure placeholders a screen paints when its render throws. Matching on
  // text rather than a class, because the placeholders reuse the same `.muted`
  // class the screens use for ordinary labels.
  for (const phrase of forbiddenText || []) {
    assert.ok(
      !body.includes(phrase),
      `${route} rendered its failure state ("${phrase}"). Visible text was:\n${body}`,
    );
  }
  assert.deepEqual(
    app.pageErrors.slice(before),
    [],
    `${route} raised uncaught page errors`,
  );
}

test("Controls switches to dark mode and restores the stored theme on reload", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await app.page.emulateMedia({ colorScheme: "light" });
    await app.page.goto(`${app.origin}/#/`);
    await app.page.waitForSelector('[data-testid="context-menu-toggle"]');
    await app.page.evaluate(() => localStorage.removeItem("refine_color_theme"));
    await app.page.reload();
    await app.page.locator('[data-testid="context-menu-toggle"]').click();
    await app.page.locator('[data-testid="nav-theme-toggle"]').click();

    const dark = await app.page.evaluate(() => {
      const toggle = document.getElementById("btn-theme-toggle");
      const bodyStyle = getComputedStyle(document.body);
      return {
        theme: document.documentElement.dataset.theme,
        stored: localStorage.getItem("refine_color_theme"),
        pressed: toggle.getAttribute("aria-pressed"),
        label: toggle.getAttribute("aria-label"),
        status: toggle.querySelector(".nav-theme-status").textContent,
        background: bodyStyle.backgroundColor,
        color: bodyStyle.color,
      };
    });
    assert.deepEqual(dark, {
      theme: "dark",
      stored: "dark",
      pressed: "true",
      label: "Use light mode",
      status: "On",
      background: "rgb(11, 17, 32)",
      color: "rgb(229, 231, 235)",
    });

    await app.page.reload();
    assert.equal(
      await app.page.getAttribute("html", "data-theme"),
      "dark",
      "reload should apply the stored theme before app initialization",
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("goal detail renders from the routed URL", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, {
      route: "#/goals/GOAL1",
      marker: '[data-testid="goal-detail"]',
      forbiddenText: ["Could not load Goal"],
    });
    // The controls whose wiring the redraw pattern rewrote, and where the
    // corrupted selectors were.
    for (const testId of ["goal-title", "goal-status-pill", "goal-action-menu-toggle"]) {
      assert.equal(
        await app.page.locator(`[data-testid="${testId}"]`).count(),
        1,
        `goal detail is missing ${testId}`,
      );
    }
  } finally {
    await app.close();
  }
});

test("project status active node drives the browser title, navigation, and Goal label", { skip: SKIP }, async () => {
  const activeGoal = {
    ...GOAL,
    node_id: "port-owner",
    node_display_name: "Port Owner",
  };
  const nodes = [
    { id: "stale-base", display_name: "Stale Base Node" },
    { id: "port-owner", display_name: "Port Owner" },
  ];
  const fixture = (pathname, request) => {
    if (pathname.startsWith("/api/project/status")) {
      return {
        attached: true,
        target_root: "/tmp/app",
        registry_enabled: true,
        apps: [],
        nodes,
        active_node_id: "port-owner",
        active_node: "Port Owner",
      };
    }
    if (pathname.startsWith("/api/nodes")) {
      return {
        nodes,
        active_node_id: "port-owner",
        active_node: "Port Owner",
      };
    }
    if (pathname.startsWith("/api/goals/")) return { goal: activeGoal };
    if (pathname.startsWith("/api/goals")) {
      return {
        goals: [activeGoal],
        facets: { status_counts: {} },
        page: { page: 1, total: 1 },
      };
    }
    return apiFixture(pathname, request);
  };
  const app = await openApp({ fixture });
  try {
    await assertScreenRenders(app, {
      route: "#/goals?node=current",
      marker: '[data-testid="goals-table"]',
    });
    await app.page.waitForFunction(() => document.title === "Port Owner - refine");
    const activeNodeLabel = app.page.locator("#active-node-label");
    assert.equal(await app.page.title(), "Port Owner - refine");
    assert.equal(await activeNodeLabel.textContent(), "Port Owner");
    assert.equal(await activeNodeLabel.getAttribute("title"), "Port Owner");
    assert.equal(await app.page.locator(".goals-node-cell").textContent(), "Port Owner");
    assert.doesNotMatch(
      [
        await app.page.title(),
        await activeNodeLabel.textContent(),
        await activeNodeLabel.getAttribute("title"),
        await app.page.locator(".goals-node-cell").textContent(),
      ].join(" "),
      /Stale Base Node/,
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Controls Node selector switches by ID and refreshes the visible Node context", { skip: SKIP }, async () => {
  let activeNodeId = "node-a";
  const nodes = [
    { id: "node-a", display_name: "Alpha" },
    { id: "node-b", display_name: "Beta" },
    { id: "node-old", display_name: "Archived", archived: true },
  ];
  const requests = [];
  const fixture = (pathname, request) => {
    if (pathname === "/api/nodes/activate") {
      const body = request.postDataJSON();
      requests.push([request.method(), pathname, body]);
      activeNodeId = body.node_id;
      return { active_node_id: activeNodeId };
    }
    if (pathname === "/api/project/status") {
      return {
        attached: true,
        target_root: "/tmp/app",
        registry_enabled: true,
        apps: [],
        active_node_id: activeNodeId,
        active_node: nodes.find((node) => node.id === activeNodeId)?.display_name,
      };
    }
    if (pathname === "/api/nodes") {
      return { nodes, active_node_id: activeNodeId };
    }
    return apiFixture(pathname, request);
  };
  const app = await openApp({ fixture });
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.locator('[data-testid="context-menu-toggle"]').click();
    const selector = app.page.locator('[data-testid="global-node"]');
    assert.deepEqual(await selector.locator("option").allTextContents(), ["Alpha", "Beta"]);
    assert.equal(await selector.inputValue(), "node-a");
    await selector.selectOption("node-b");
    await app.page.waitForFunction(() => document.title === "Beta - refine");
    assert.equal(new URL(app.page.url()).hash, "#/");
    assert.equal(await selector.inputValue(), "node-b");
    assert.equal(await app.page.locator("#active-node-label").textContent(), "Beta");
    assert.deepEqual(requests, [["POST", "/api/nodes/activate", { node_id: "node-b" }]]);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("features list renders from the routed URL", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, {
      route: "#/features",
      marker: ".features-table",
      forbiddenText: ["No Features match the current filters"],
    });
    assert.equal(await app.page.locator("#features-table tbody tr").count(), 1);
  } finally {
    await app.close();
  }
});

test("shared table columns wrap long unbroken content within their cells", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    const layout = await app.page.evaluate(() => {
      const fixture = document.createElement("div");
      fixture.style.width = "260px";
      fixture.innerHTML = `
        <table class="table" data-testid="table-overflow-fixture">
          <thead><tr>
            <th>UnbrokenHeaderThatMustStayInsideItsColumn</th>
            <th>Other</th>
          </tr></thead>
          <tbody><tr>
            <td>averylongassigneeemailaddress@example-with-a-long-domain.invalid</td>
            <td><code>unbroken-code-value-that-also-must-wrap</code></td>
          </tr></tbody>
        </table>`;
      document.body.appendChild(fixture);

      const table = fixture.querySelector("table");
      const cells = [...fixture.querySelectorAll("th, td")];
      return {
        tableFitsContainer: table.getBoundingClientRect().right
          <= fixture.getBoundingClientRect().right + 0.5,
        cells: cells.map((cell) => {
          const range = document.createRange();
          range.selectNodeContents(cell);
          const cellRect = cell.getBoundingClientRect();
          const contentRect = range.getBoundingClientRect();
          return {
            overflowWrap: getComputedStyle(cell).overflowWrap,
            contentFits: contentRect.left >= cellRect.left - 0.5
              && contentRect.right <= cellRect.right + 0.5,
          };
        }),
      };
    });

    assert.equal(layout.tableFitsContainer, true);
    assert.ok(layout.cells.length > 0);
    assert.ok(layout.cells.every((cell) => cell.overflowWrap === "anywhere"));
    assert.ok(layout.cells.every((cell) => cell.contentFits));
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Goals cold-load hydrates Reporter and Assignee filters before rendering", { skip: SKIP }, async () => {
  const requests = [];
  const app = await openApp({
    onRequest(pathname) {
      requests.push(pathname);
    },
  });
  try {
    await assertScreenRenders(app, {
      route: "#/goals",
      marker: '[data-testid="goals-table"]',
    });
    assert.ok(requests.includes("/api/reporters"));
    assert.deepEqual(
      await app.page.locator('[data-testid="goals-reporter-filter"] option').allTextContents(),
      ["all reporters", "Reporter"],
    );
    assert.deepEqual(
      await app.page.locator('[data-testid="goals-assignee-filter"] option').allTextContents(),
      ["all assignees", "Reporter"],
    );
  } finally {
    await app.close();
  }
});

test("primary Dashboard and Goals navigation preserves current and all node scope", { skip: SKIP }, async () => {
  const app = await openApp();
  const hash = () => new URL(app.page.url()).hash;
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });

    // Keyboard activation follows the real primary Goals link and makes the
    // Dashboard's default current scope explicit in the Goals URL.
    await app.page.locator('[data-testid="nav-goals"]').focus();
    await app.page.locator('[data-testid="nav-goals"]').press("Enter");
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(hash(), "#/goals?node=current");
    assert.equal(await app.page.locator('[data-testid="goals-node-filter"]').inputValue(), "current");

    await app.page.locator('[data-testid="nav-dashboard"]').click();
    await app.page.waitForSelector("#dash");
    assert.equal(hash(), "#/");
    await app.page.locator('[data-testid="nav-goals"]').click();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(hash(), "#/goals?node=current");

    await app.page.goBack();
    await app.page.waitForSelector("#dash");
    assert.equal(hash(), "#/");
    await app.page.goForward();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(hash(), "#/goals?node=current");
    await app.page.reload();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(await app.page.locator('[data-testid="goals-node-filter"]').inputValue(), "current");

    await app.page.locator('[data-testid="nav-dashboard"]').click();
    await app.page.waitForSelector("#dash");
    await app.page.locator('[data-testid="dashboard-scope-all"]').click();
    await app.page.waitForFunction(() => location.hash === "#/?node=all");
    assert.equal(hash(), "#/?node=all");

    await app.page.locator('[data-testid="nav-goals"]').click();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(hash(), "#/goals?node=all");
    assert.equal(await app.page.locator('[data-testid="goals-node-filter"]').inputValue(), "all");

    // The brand is another ordinary Dashboard entry point and must carry All.
    await app.page.locator(".brand").click();
    await app.page.waitForSelector("#dash");
    assert.equal(hash(), "#/?node=all");
    await app.page.locator('[data-testid="nav-goals"]').click();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(hash(), "#/goals?node=all");
    await app.page.reload();
    await app.page.waitForSelector('[data-testid="goals-table"]');
    assert.equal(await app.page.locator('[data-testid="goals-node-filter"]').inputValue(), "all");
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("feature detail renders from the routed URL", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, {
      route: "#/features/FEAT1",
      marker: '[data-testid="feature-detail-modal"]',
    });
  } finally {
    await app.close();
  }
});

test("Toolbar add menu closes when the user clicks outside it", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    const menu = app.page.locator('[data-testid="toolbar-add-menu"]');

    await app.page.locator('[data-testid="toolbar-add"]').click();
    assert.equal(await menu.evaluate((element) => element.open), true);

    await app.page.locator(".toolbar-dock-label").click();
    assert.equal(await menu.evaluate((element) => element.open), false);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("a toolbar Agent's morphed Start button dispatches Stop", { skip: SKIP }, async () => {
  const requests = [];
  const app = await openApp({
    fixture(pathname, request) {
      if (pathname === "/api/terminal/session" && request.method() === "POST") {
        return {
          id: "browser-agent-session",
          process_id: "browser-agent-process",
          cwd: "/repo",
          profile: "agent",
          provider: "codex",
        };
      }
      if (pathname === "/api/terminal/browser-agent-session/stop") return { ok: true };
      return apiFixture(pathname);
    },
    onRequest(pathname, request) {
      requests.push([request.method(), pathname]);
    },
  });
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.evaluate(() => {
      window.EventSource = class {
        addEventListener() {}
        close() {}
      };
      chatState.tabs = {
        agent: normalizeInteractiveTerminalTab({
          goalId: null,
          label: "Agent",
          mode: "agent",
          sessionId: null,
        }),
      };
      chatState.activeTabId = "agent";
      chatState.open = true;
      chatState.bodyHeight = 420;
      drawToolbar();
    });

    await app.page.locator('[data-testid="terminal-start"]').click();
    await app.page.waitForSelector('[data-testid="terminal-stop"]');
    await app.page.locator('[data-testid="terminal-stop"]').click();
    await app.page.waitForSelector('[data-testid="terminal-start"]');

    assert.equal(
      await app.page.locator('[data-testid="terminal-start"]').textContent(),
      "Restart",
    );
    assert.deepEqual(
      requests.filter(([, pathname]) => (
        pathname === "/api/terminal/session"
        || pathname === "/api/terminal/browser-agent-session/stop"
      )),
      [
        ["POST", "/api/terminal/session"],
        ["POST", "/api/terminal/browser-agent-session/stop"],
      ],
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Agent terminal renders transported ANSI control sequences through xterm", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.evaluate(async () => {
      chatState.tabs = {
        agent: normalizeInteractiveTerminalTab({
          goalId: null,
          label: "Agent",
          mode: "agent",
          sessionId: null,
        }),
      };
      chatState.activeTabId = "agent";
      chatState.open = true;
      chatState.bodyHeight = 420;
      drawToolbar();
      const terminal = terminalStateFor("agent");
      terminalReceiveOutput("\\u001b[31mANSI-RED\\u001b[0m plain", terminal);
      await new Promise((resolve) => terminal.term.write("", resolve));
    });
    // The write callback confirms parsing; xterm paints the DOM on a later frame.
    await app.page.waitForFunction(() =>
      document.querySelector(".terminal-output .xterm-rows")?.textContent.includes("ANSI-RED plain"),
    );
    const rendered = await app.page.locator(".terminal-output .xterm-rows").evaluate((rows) => ({
      text: rows.textContent,
      colors: [...rows.querySelectorAll("span")].map((span) => ({
        text: span.textContent,
        color: getComputedStyle(span).color,
      })),
    }));

    assert.match(rendered.text, /ANSI-RED plain/);
    assert.doesNotMatch(rendered.text, /(?:\\u001b|\[31m|\[0m)/);
    assert.equal(rendered.colors.find((span) => span.text === "ANSI-RED")?.color, "rgb(185, 28, 28)");
    assert.equal(rendered.colors.find((span) => span.text === " plain")?.color, "rgb(17, 24, 39)");
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Agent terminal refits from visible geometry after minimize and fullscreen", { skip: SKIP }, async () => {
  const backendSizes = [];
  const app = await openApp({
    fixture(pathname) {
      if (pathname === "/api/terminal/browser-responsive-agent/resize") return { ok: true };
      return apiFixture(pathname);
    },
    onRequest(pathname, request) {
      if (pathname === "/api/terminal/browser-responsive-agent/resize") {
        backendSizes.push(request.postDataJSON());
      }
    },
  });
  const terminalGeometry = async (action = null) => app.page.evaluate(async (nextAction) => {
    if (nextAction === "minimize") minimizeToolbar();
    if (nextAction === "restore") toggleToolbar();
    if (nextAction === "fullscreen") toggleToolbarFullscreen();
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    await new Promise((resolve) => setTimeout(resolve, 120));
    const terminal = terminalStateFor("responsive-agent");
    return {
      cols: terminal.term.cols,
      rows: terminal.term.rows,
      backendCols: terminal.lastCols,
      backendRows: terminal.lastRows,
    };
  }, action);

  try {
    await app.page.setViewportSize({ width: 760, height: 600 });
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.evaluate(() => {
      window.EventSource = class {
        addEventListener() {}
        close() {}
      };
      chatState.tabs = {
        "responsive-agent": normalizeInteractiveTerminalTab({
          goalId: null,
          label: "Agent",
          mode: "agent",
          sessionId: "browser-responsive-agent",
          processId: "browser-responsive-agent-process",
        }),
      };
      chatState.activeTabId = "responsive-agent";
      chatState.open = true;
      chatState.bodyHeight = 320;
      const terminal = terminalStateFor("responsive-agent");
      terminal.sessionId = "browser-responsive-agent";
      terminal.processId = "browser-responsive-agent-process";
      terminal.connected = true;
      terminal.statusChecked = true;
      terminal.reattaching = false;
      drawToolbar();
    });

    const initial = await terminalGeometry();
    const visibleResizeCount = backendSizes.length;
    assert.ok(initial.cols > 20);
    assert.equal(initial.backendCols, initial.cols);
    assert.equal(initial.backendRows, initial.rows);

    const hidden = await terminalGeometry("minimize");
    assert.deepEqual(hidden, initial);
    assert.equal(backendSizes.length, visibleResizeCount);

    await app.page.setViewportSize({ width: 1400, height: 900 });
    const restored = await terminalGeometry("restore");
    assert.ok(restored.cols > initial.cols);
    assert.equal(restored.backendCols, restored.cols);
    assert.equal(restored.backendRows, restored.rows);

    const fullscreen = await terminalGeometry("fullscreen");
    assert.ok(fullscreen.rows > restored.rows);
    assert.equal(fullscreen.backendCols, fullscreen.cols);
    assert.equal(fullscreen.backendRows, fullscreen.rows);
    assert.deepEqual(backendSizes.at(-1), {
      cols: fullscreen.cols,
      rows: fullscreen.rows,
    });

    const exitedFullscreen = await terminalGeometry("fullscreen");
    assert.ok(exitedFullscreen.rows < fullscreen.rows);
    assert.equal(exitedFullscreen.backendCols, exitedFullscreen.cols);
    assert.equal(exitedFullscreen.backendRows, exitedFullscreen.rows);
    assert.deepEqual(backendSizes.at(-1), {
      cols: exitedFullscreen.cols,
      rows: exitedFullscreen.rows,
    });
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Todo List renders an item-first workspace with responsive list navigation", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.setViewportSize({ width: 1100, height: 800 });
    await app.page.evaluate(() => {
      state.lastReporter = "Reporter";
      chatState.tabs = {
        todo: {
          goalId: null,
          label: "Todo List",
          mode: "todo",
          sessionId: null,
        },
      };
      chatState.activeTabId = "todo";
      chatState.open = true;
      chatState.bodyHeight = 430;
      todoState.reporter = "Reporter";
      todoState.selectedListId = "release";
      todoState.lists = [
        {
          id: "release",
          name: "Release",
          items: [
            { id: "ship", text: "Ship the candidate", done: false },
            { id: "notes", text: "Write release notes", done: true },
          ],
        },
        {
          id: "later",
          name: "Later",
          items: [],
        },
      ];
      drawToolbar();
    });

    const wideScreen = await app.page.evaluate(() => {
      const rail = document.querySelector(".todo-list-rail").getBoundingClientRect();
      const workspace = document.querySelector(".todo-workspace").getBoundingClientRect();
      const composer = document.querySelector(".todo-add-form").getBoundingClientRect();
      const items = document.querySelector(".todo-item-scroll").getBoundingClientRect();
      return {
        railBeforeWorkspace: rail.right <= workspace.left,
        composerBeforeItems: composer.bottom <= items.bottom && composer.top < items.top,
        title: document.querySelector('[data-testid="todo-list-title"]').textContent,
        openCount: document.querySelector(".todo-list-nav-item.active .todo-list-nav-count").textContent,
        completedCount: document.querySelector(".todo-completed-section h4 span").textContent,
      };
    });
    assert.deepEqual(wideScreen, {
      railBeforeWorkspace: true,
      composerBeforeItems: true,
      title: "Release",
      openCount: "1",
      completedCount: "1",
    });

    await app.page.locator('[data-todo-item-id="ship"] [data-todo-edit]').click();
    await app.page.waitForSelector('[data-todo-item-id="ship"] [data-todo-edit-form]');
    assert.equal(
      await app.page.locator('[data-todo-item-id="ship"] [data-todo-edit-text]').inputValue(),
      "Ship the candidate",
    );

    await app.page.setViewportSize({ width: 700, height: 800 });
    const mobile = await app.page.evaluate(() => {
      const rail = document.querySelector(".todo-list-rail").getBoundingClientRect();
      const workspace = document.querySelector(".todo-workspace").getBoundingClientRect();
      return {
        railAboveWorkspace: rail.bottom <= workspace.top,
        listFlow: getComputedStyle(document.querySelector(".todo-list-nav")).display,
      };
    });
    assert.deepEqual(mobile, {
      railAboveWorkspace: true,
      listFlow: "flex",
    });
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Agent tab round trip retains xterm and scrollback while returning to latest output", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    const result = await app.page.evaluate(async () => {
      const firstId = "renderer-agent";
      const secondId = "renderer-agent-2";
      const makeTab = (label, mode) => normalizeInteractiveTerminalTab({
        goalId: null,
        label,
        mode,
        sessionId: null,
      });
      const nextFrame = () => new Promise((resolve) => requestAnimationFrame(resolve));
      const flushWrites = (term) => new Promise((resolve) => term.write("", resolve));
      const bufferText = (term) => {
        const buffer = term.buffer.active;
        const lines = [];
        for (let index = 0; index < buffer.length; index += 1) {
          lines.push(buffer.getLine(index)?.translateToString(true) || "");
        }
        return lines.join("\n");
      };

      chatState.tabs = {
        [firstId]: makeTab("Agent", "agent"),
        [secondId]: makeTab("Agent 2", "agent"),
      };
      for (const tabId of [firstId, secondId]) {
        const tab = chatState.tabs[tabId];
        tab.sessionId = `${tabId}-session`;
        tab.processId = `${tabId}-process`;
        const terminal = terminalStateFor(tabId);
        terminal.sessionId = tab.sessionId;
        terminal.processId = tab.processId;
        terminal.connected = true;
        terminal.statusChecked = true;
        terminal.reattaching = false;
        terminal.eventSource = { close() {} };
      }
      chatState.activeTabId = firstId;
      chatState.open = true;
      chatState.bodyHeight = 420;
      drawToolbar();
      await nextFrame();

      const first = terminalStateFor(firstId);
      const firstTerm = first.term;
      const firstOutput = Array.from(
        { length: 80 },
        (_, index) => `FIRST-SCROLLBACK-${String(index).padStart(2, "0")}`,
      ).join("\r\n");
      terminalReceiveOutput(`${firstOutput}\r\nFIRST-SCROLLBACK-END`, first);
      await flushWrites(firstTerm);
      firstTerm.scrollToTop();
      const firstViewport = firstTerm.buffer.active.viewportY;
      const firstBase = firstTerm.buffer.active.baseY;

      await activateToolbarTab(secondId);
      await nextFrame();
      const second = terminalStateFor(secondId);
      const secondTerm = second.term;
      terminalReceiveOutput("SECOND-ACTIVE-ONLY", second);
      await flushWrites(secondTerm);
      await nextFrame();
      const secondHost = document.querySelector(".terminal-output");
      const secondMount = {
        count: secondHost.querySelectorAll(":scope > .xterm").length,
        showsSecond: secondHost.querySelector(".xterm-rows")?.textContent
          .includes("SECOND-ACTIVE-ONLY") || false,
        firstDetached: !firstTerm.element.isConnected,
        secondMounted: secondTerm.element.parentElement === secondHost,
      };

      terminalReceiveOutput("\r\nFIRST-LATEST-MARKER", first);
      await flushWrites(firstTerm);
      const firstBaseAfterLatest = firstTerm.buffer.active.baseY;
      const firstViewportBeforeReturn = firstTerm.buffer.active.viewportY;

      await activateToolbarTab(firstId);
      await nextFrame();
      const firstHost = document.querySelector(".terminal-output");
      const firstBuffer = bufferText(firstTerm);
      return {
        secondMount,
        firstMountCount: firstHost.querySelectorAll(":scope > .xterm").length,
        firstMounted: firstTerm.element.parentElement === firstHost,
        secondDetached: !secondTerm.element.isConnected,
        firstInstanceRetained: terminalStateFor(firstId).term === firstTerm,
        secondInstanceRetained: terminalStateFor(secondId).term === secondTerm,
        firstScrollbackRetained:
          firstBase > 0
          && firstBaseAfterLatest >= firstBase
          && firstViewportBeforeReturn === firstViewport
          && firstBuffer.includes("FIRST-SCROLLBACK-00")
          && firstBuffer.includes("FIRST-SCROLLBACK-END")
          && firstBuffer.includes("FIRST-LATEST-MARKER"),
        firstBaseUnchanged: firstTerm.buffer.active.baseY === firstBaseAfterLatest,
        firstViewportAtBottom:
          firstTerm.buffer.active.viewportY === firstTerm.buffer.active.baseY,
        latestMarkerVisible:
          firstHost.querySelector(".xterm-rows")?.textContent.includes("FIRST-LATEST-MARKER")
          || false,
        firstExcludesSecond:
          !firstHost.querySelector(".xterm-rows")?.textContent.includes("SECOND-ACTIVE-ONLY"),
      };
    });

    assert.deepEqual(result, {
      secondMount: {
        count: 1,
        showsSecond: true,
        firstDetached: true,
        secondMounted: true,
      },
      firstMountCount: 1,
      firstMounted: true,
      secondDetached: true,
      firstInstanceRetained: true,
      secondInstanceRetained: true,
      firstScrollbackRetained: true,
      firstBaseUnchanged: true,
      firstViewportAtBottom: true,
      latestMarkerVisible: true,
      firstExcludesSecond: true,
    });
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("switching from Agent to Files and back restores the terminal renderer", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    await app.page.evaluate(async () => {
      chatState.tabs = {
        agent: normalizeInteractiveTerminalTab({
          goalId: null,
          label: "Agent",
          mode: "agent",
          sessionId: null,
        }),
        files: {
          goalId: null,
          label: "Files",
          mode: "files",
          sessionId: null,
        },
      };
      chatState.activeTabId = "agent";
      chatState.open = true;
      chatState.bodyHeight = 420;
      filesState.entriesByPath[""] = [];
      const terminal = terminalStateFor("agent");
      terminal.sessionId = "agent-session";
      terminal.connected = true;
      terminal.statusChecked = true;
      terminal.reattaching = false;
      terminal.eventSource = { close() {} };
      drawToolbar();
      window.__agentTermBeforeFiles = terminal.term;
      terminalReceiveOutput("AGENT-CONTENT-BEFORE-FILES", terminal);
      await new Promise((resolve) => terminal.term.write("", resolve));
    });

    assert.equal(await app.page.locator('[data-testid="toolbar-terminal-panel"]').count(), 1);
    await app.page.locator('[data-testid="toolbar-tab-files"]').click();
    await app.page.waitForSelector('[data-testid="toolbar-files-panel"]');

    assert.equal(await app.page.locator('[data-testid="toolbar-files-panel"]').count(), 1);
    assert.equal(await app.page.locator('[data-testid="toolbar-terminal-panel"]').count(), 0);
    assert.equal(await app.page.locator('[data-testid="terminal-output"]').count(), 0);

    await app.page.locator('[data-testid="toolbar-tab-agent"]').click();
    await app.page.waitForSelector('[data-testid="toolbar-terminal-panel"]');
    const restored = await app.page.evaluate(() => {
      const terminal = terminalStateFor("agent");
      const host = document.querySelector('[data-testid="terminal-output"]');
      return {
        sameInstance: terminal.term === window.__agentTermBeforeFiles,
        mounted: terminal.term.element?.parentElement === host,
        mountCount: host.querySelectorAll(":scope > .xterm").length,
        showsAgentContent:
          host.querySelector(".xterm-rows")?.textContent.includes("AGENT-CONTENT-BEFORE-FILES")
          || false,
        filesPanelCount: document.querySelectorAll('[data-testid="toolbar-files-panel"]').length,
      };
    });
    assert.deepEqual(restored, {
      sameInstance: true,
      mounted: true,
      mountCount: 1,
      showsAgentContent: true,
      filesPanelCount: 0,
    });
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

// The remaining screens moved onto the redraw pattern. One browser for all of
// them: each assertion is a scaffold element that paints regardless of whether the
// screen has data, so an empty fixture still proves the route booted, rendered,
// and bound without throwing.
test("every converted screen boots and paints", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    for (const [route, marker] of [
      ["#/", "#dash"],
      ["#/goals", '[data-testid="goals-table"]'],
      ["#/features", "#features-table"],
      ["#/changes", '[data-testid="changes-visualization-panel"]'],
      ["#/logs", "#logs-visualization"],
      ["#/node", "#settings-content"],
    ]) {
      await assertScreenRenders(app, { route, marker });
    }
  } finally {
    await app.close();
  }
});

test("Goals Node column has a readable default without overflowing Updated", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await app.page.setViewportSize({ width: 1280, height: 800 });
    await assertScreenRenders(app, {
      route: "#/goals",
      marker: ".goals-node-value",
    });
    const layout = await app.page.locator(".goals-node-value").evaluate((node) => {
      const nodeRect = node.getBoundingClientRect();
      const updatedRect = node.closest("tr").querySelector('[data-label="Updated"]')
        .getBoundingClientRect();
      const cellRect = node.closest("td").getBoundingClientRect();
      return {
        fullName: node.title,
        nodeColumnWidth: Math.round(cellRect.width),
        textOverflow: getComputedStyle(node).textOverflow,
        whiteSpace: getComputedStyle(node).whiteSpace,
        isTruncated: node.scrollWidth > node.clientWidth,
        staysBeforeUpdated: nodeRect.right <= updatedRect.left,
      };
    });
    assert.equal(layout.fullName, GOAL.node_display_name);
    assert.ok(layout.nodeColumnWidth >= 220, `Node column was only ${layout.nodeColumnWidth}px`);
    assert.equal(layout.textOverflow, "ellipsis");
    assert.equal(layout.whiteSpace, "nowrap");
    assert.equal(layout.isTruncated, true);
    assert.equal(layout.staysBeforeUpdated, true);
    assert.equal(
      await app.page.locator('[data-testid="goals-node-resize"]').getAttribute("aria-valuenow"),
      "220",
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Goals Node column widens accessibly, stays bounded, and survives rerenders", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await app.page.setViewportSize({ width: 1280, height: 800 });
    await assertScreenRenders(app, {
      route: "#/goals",
      marker: '[data-testid="goals-node-resize"]',
    });
    const handle = app.page.locator('[data-testid="goals-node-resize"]');

    const box = await handle.boundingBox();
    assert.ok(box, "Node resize handle has no pointer target");
    const hitTarget = await app.page.evaluate(({ x, y }) => {
      const element = document.elementFromPoint(x, y);
      return `${element?.tagName || "none"}.${element?.className || ""}`;
    }, { x: box.x + box.width / 2, y: box.y + box.height / 2 });
    assert.match(hitTarget, /table-column-resize-handle/);
    await app.page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await app.page.mouse.down();
    await app.page.mouse.move(box.x + box.width / 2 + 96, box.y + box.height / 2);
    await app.page.mouse.up();
    assert.equal(await handle.getAttribute("aria-valuenow"), "316");

    await handle.focus();
    await handle.press("End");
    assert.equal(await handle.getAttribute("aria-valuenow"), "480");
    await handle.press("ArrowRight");
    assert.equal(await handle.getAttribute("aria-valuenow"), "480");
    assert.equal(new URL(app.page.url()).hash, "#/goals");

    const widened = await app.page.locator(".goals-node-value").evaluate((node) => {
      const nodeRect = node.getBoundingClientRect();
      const updatedRect = node.closest("tr").querySelector('[data-label="Updated"]')
        .getBoundingClientRect();
      const scroll = node.closest(".goals-table-scroll");
      return {
        isTruncated: node.scrollWidth > node.clientWidth,
        staysBeforeUpdated: nodeRect.right <= updatedRect.left,
        scrollsHorizontally: scroll.scrollWidth > scroll.clientWidth,
      };
    });
    assert.deepEqual(widened, {
      isTruncated: false,
      staysBeforeUpdated: true,
      scrollsHorizontally: true,
    });

    await handle.press("Home");
    assert.equal(await handle.getAttribute("aria-valuenow"), "144");
    await handle.press("ArrowLeft");
    assert.equal(await handle.getAttribute("aria-valuenow"), "144");
    await handle.press("End");

    // Expanding selection and changing page both redraw the table. The width and
    // selection controls must survive alongside the existing list behavior.
    await app.page.locator('[data-testid="goals-filter-summary"]').click();
    assert.equal(
      await app.page.locator('[data-testid="goals-node-resize"]').getAttribute("aria-valuenow"),
      "480",
    );
    await app.page.waitForSelector('[data-testid="goals-row-select"]');
    assert.equal(await app.page.locator('[data-testid="goals-row-select"]').count(), 1);
    await app.page.evaluate(() => updateGoalsFilter({ page: 2 }));
    await app.page.waitForFunction(() => (
      location.hash.includes("page=2")
      &&
      document.querySelector('[data-testid="goals-node-resize"]')?.getAttribute("aria-valuenow") === "480"
    ));

    await app.page.locator('[data-testid="goals-sort-node"] .goals-column-heading').click();
    await app.page.waitForFunction(() => location.hash.includes("sort=node"));
    assert.equal(
      await app.page.locator('[data-testid="goals-node-resize"]').getAttribute("aria-valuenow"),
      "480",
    );

    // sessionStorage also carries the preference across a same-tab refresh.
    await app.page.reload();
    await app.page.waitForSelector('[data-testid="goals-node-resize"]');
    assert.equal(
      await app.page.locator('[data-testid="goals-node-resize"]').getAttribute("aria-valuenow"),
      "480",
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Goals mobile cards ignore wide-screen Node width and show the complete label", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await app.page.setViewportSize({ width: 700, height: 800 });
    await assertScreenRenders(app, {
      route: "#/goals",
      marker: ".goals-node-value",
    });
    const mobile = await app.page.locator(".goals-node-value").evaluate((node) => {
      const table = node.closest("table");
      const scroll = node.closest(".goals-table-scroll");
      return {
        text: node.textContent,
        overflow: getComputedStyle(node).overflow,
        textOverflow: getComputedStyle(node).textOverflow,
        whiteSpace: getComputedStyle(node).whiteSpace,
        tableFitsViewport: table.getBoundingClientRect().width <= scroll.getBoundingClientRect().width,
      };
    });
    assert.deepEqual(mobile, {
      text: GOAL.node_display_name,
      overflow: "visible",
      textOverflow: "clip",
      whiteSpace: "normal",
      tableFitsViewport: true,
    });
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("every Node, Project, and legacy Settings tab renders and refreshes", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    for (const [route, marker] of [
      ["#/node/application", '[data-testid="project-app-select"]'],
      ["#/node/reporters", '[data-testid="reporters-table"]'],
      ["#/node/processes", '[data-testid="settings-pane-processes"].active'],
      ["#/node/target-app", '[data-testid="target-app-copy-node"]'],
      ["#/node/runtime", '[data-testid="runtime-recheck-auth"]'],
      ["#/node/releases", '[data-testid="settings-skills"]'],
      ["#/project/governance", '[data-testid="settings-skills"]'],
      ["#/settings/events", '[data-testid="settings-skills"]'],
      ["#/settings/skills", '[data-testid="settings-skills"]'],
      ["#/settings", '[data-testid="settings-pane-processes"].active'],
    ]) {
      await assertScreenRenders(app, { route, marker });
      await app.page.evaluate(() => refreshActiveSettingsTab({ force: true }));
      await app.page.waitForSelector(marker, { timeout: 10000 });
      assert.deepEqual(
        app.pageErrors,
        [],
        `${route} raised an error while refreshing`,
      );
    }
  } finally {
    await app.close();
  }
});


test("settings refresh never clears the Upgrade banner while its read is pending", { skip: SKIP }, async () => {
  const fixture = (pathname, request) => {
    if (pathname.startsWith("/api/upgrade")) {
      return {
        upgrade: {
          runtime_kind: "published_release",
          current_version: "4.0.0",
          latest_version: "4.1.0",
          upgrade_available: true,
        },
      };
    }
    return apiFixture(pathname, request);
  };
  const app = await openApp({ fixture });
  try {
    await assertScreenRenders(app, {
      route: "#/node/releases",
      marker: '[data-testid="runtime-upgrade-status"]',
    });
    const result = await app.page.evaluate(async () => {
      const root = document.getElementById("runtime-upgrade-banner");
      const status = root.querySelector('[data-testid="runtime-upgrade-status"]');
      const originalApi = api;
      let releaseUpgrade;
      let pendingUpgradeRead;
      let mutations = 0;
      const observer = new MutationObserver((records) => {
        mutations += records.length;
      });
      observer.observe(root, {
        childList: true,
        characterData: true,
        subtree: true,
      });
      api = async (method, path, body, options) => {
        if (path === "/api/upgrade") {
          pendingUpgradeRead = new Promise((resolve) => {
            releaseUpgrade = resolve;
          });
          return pendingUpgradeRead;
        }
        return originalApi(method, path, body, options);
      };

      try {
        await refreshSettings({ force: true });
        await Promise.resolve();
        const whilePending = {
          statusIdentity:
            root.querySelector('[data-testid="runtime-upgrade-status"]') === status,
          text: root.textContent,
          mutations,
        };
        releaseUpgrade({
          upgrade: {
            runtime_kind: "published_release",
            current_version: "4.0.0",
            latest_version: "4.2.0",
            upgrade_available: true,
          },
        });
        await pendingUpgradeRead;
        await Promise.resolve();
        return {
          whilePending,
          afterRead: root.textContent,
        };
      } finally {
        api = originalApi;
        observer.disconnect();
      }
    });

    assert.equal(result.whilePending.statusIdentity, true);
    assert.match(result.whilePending.text, /Upgrade available 4\.1\.0/);
    assert.equal(result.whilePending.mutations, 0);
    assert.match(result.afterRead, /Upgrade available 4\.2\.0/);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("runtime banner distinguishes trusted source relationships from unknown evidence", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, {
      route: "#/settings/runtime",
      marker: '[data-testid="runtime-recheck-auth"]',
    });
    const messages = await app.page.evaluate(() => {
      const text = (upgrade) => {
        const root = document.createElement("div");
        root.innerHTML = renderRuntimeUpgradeBanner(upgrade);
        return root.textContent.replace(/\s+/g, " ").trim();
      };
      return {
        current: text({
          runtime_kind: "source",
          source: {
            running_from_head: true,
            relationship: "current",
            upstream: { remote: "origin", branch: "main" },
          },
        }),
        diverged: text({
          runtime_kind: "source",
          source: {
            running_from_head: true,
            relationship: "diverged",
            upstream: { remote: "origin", branch: "main" },
          },
        }),
        unknown: text({
          runtime_kind: "source",
          source: {
            running_from_head: null,
            relationship: "unknown",
            unknown_reason: "executable_checkout_mismatch",
            upstream: { freshness: "fresh" },
          },
        }),
      };
    });
    assert.equal(messages.current, "Running from HEAD · Up to date with origin/main");
    assert.equal(messages.diverged, "Running from HEAD · Diverged from origin/main");
    assert.equal(
      messages.unknown,
      "Source runtime status unknown · the running executable does not match checkout HEAD",
    );
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("changed settings refresh preserves focus, dirty controls, scroll, and one live handler", { skip: SKIP }, async () => {
  let settingsVersion = 1;
  const settingsWrites = [];
  const fixture = (pathname, request) => {
    if (pathname.startsWith("/api/settings")) {
      if (request.method() === "PATCH") return {};
      return {
        settings: {
          parallel_run_cap: settingsVersion === 1 ? 2 : 7,
          branch_name_pattern: settingsVersion === 1
            ? "refine/{goal_id}"
            : "server/{goal_id}",
          agent_cli: "codex",
        },
      };
    }
    return apiFixture(pathname);
  };
  const app = await openApp({
    fixture,
    onRequest(pathname, request) {
      if (pathname.startsWith("/api/settings") && request.method() === "PATCH") {
        settingsWrites.push(request.postDataJSON());
      }
    },
  });
  try {
    await assertScreenRenders(app, {
      route: "#/node/runtime",
      marker: '[data-testid="runtime-recheck-auth"]',
    });
    await app.page.locator(
      '[data-settings-editable-field]:has(#s-pattern) [data-settings-editable-toggle]',
    ).click();
    await app.page.evaluate(() => {
      const style = document.createElement("style");
      style.textContent = ".settings-tab-card{height:120px;overflow:auto}";
      document.head.appendChild(style);
      const field = document.getElementById("s-pattern");
      const card = field.closest(".settings-tab-card");
      field.value = "mine/{goal_id}";
      field.focus();
      field.setSelectionRange(5, 5);
      card.scrollTop = 120;
      window.__settingsMorphBefore = {
        card,
        field,
        save: field.closest("[data-settings-editable-field]")
          .querySelector("[data-settings-editable-toggle]"),
      };
    });

    settingsVersion = 2;
    const preserved = await app.page.evaluate(async () => {
      // SSE invalidates the screen cache before requesting a settings redraw.
      invalidateScreenDataCache();
      await refreshSettingsTab("runtime", { force: true });
      const before = window.__settingsMorphBefore;
      const field = document.getElementById("s-pattern");
      const card = field.closest(".settings-tab-card");
      return {
        cardIdentity: card === before.card,
        fieldIdentity: field === before.field,
        handlerNodeIdentity:
          field.closest("[data-settings-editable-field]")
            .querySelector("[data-settings-editable-toggle]") === before.save,
        focused: document.activeElement === field,
        value: field.value,
        selectionStart: field.selectionStart,
        scrollTop: card.scrollTop,
        cleanControlValue: document.getElementById("s-cap").value,
      };
    });

    assert.deepEqual(
      preserved,
      {
        cardIdentity: true,
        fieldIdentity: true,
        handlerNodeIdentity: true,
        focused: true,
        value: "mine/{goal_id}",
        selectionStart: 5,
        scrollTop: 120,
        cleanControlValue: "7",
      },
    );

    await app.page.locator(
      '[data-settings-editable-field]:has(#s-pattern) [data-settings-editable-toggle]',
    ).click();
    await app.page.waitForTimeout(50);
    assert.equal(settingsWrites.length, 1, "the surviving Save handler should fire once");
    assert.equal(settingsWrites[0].branch_name_pattern, "mine/{goal_id}");
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("Goal Skill runs open scoped invocation evidence during workflow execution", {skip: SKIP}, async () => {
  const goal = {...GOAL,status:"implement",rounds:[{...GOAL.rounds[0],event_configuration:{revision:1}}]};
  let historyGoal = null;
  const app = await openApp({fixture: (pathname, request) => {
    if (pathname === "/api/goals/GOAL1") return {goal};
    if (pathname === "/api/event-invocations") { historyGoal = new URL(request.url()).searchParams.get("goal_id"); return {items:[],offset:0,total:0}; }
    return apiFixture(pathname);
  }});
  try {
    await app.page.goto(`${app.origin}/#/goals/GOAL1`);
    await app.page.locator('[data-testid="goal-open-agent"]').click();
    await app.page.locator('[data-testid="automation-modal"]').waitFor();
    assert.equal(historyGoal,"GOAL1");
    assert.match(await app.page.locator('[data-testid="automation-modal"]').textContent(),/Skill runs · GOAL1/);
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});

function skillFixture() {
  let revision = 7;
  const records = [{item:{id:"inspect",name:"Inspect release",prompt:"Inspect the release.",enabled:true,scope:{node_id:null},parameters:[{name:"count",kind:"number",required:true,default:3}]},trigger:{id:"inspect-trigger",source:"custom",enabled:true,mode:"blocking",order:0,scope:{node_id:null},inputs:{}}}];
  const writes = [], launches = [];
  const fixture = (pathname, request) => {
    if (pathname === "/api/skills") return {revision, items:records.map(r => ({...r.item,trigger_source:r.trigger.source})), manual_skill_ids:records.filter(r => r.item.enabled && r.trigger.source === "custom").map(r => r.item.id)};
    if (pathname.startsWith("/api/skills/") && pathname !== "/api/skills/catalog") {
      const id = pathname.split("/")[3]; const record = records.find(r => r.item.id === id);
      if (pathname.endsWith("/inputs")) { assert.equal(new URL(request.url()).searchParams.has("goal_id"),false); return {revision,parameters:record.item.parameters}; }
      if (request.method() === "PUT") {
        const body = request.postDataJSON(); assert.equal(body.revision,revision); writes.push(body);
        const saved = record || {}; saved.item = body.item;
        if (body.trigger) saved.trigger = {...body.trigger,id:body.trigger.id || "new-trigger"};
        if (!record) records.push(saved);
        return {revision:++revision,item:saved.item};
      }
      return {revision,...record};
    }
    if (pathname === "/api/terminal/session") {
      launches.push(request.postDataJSON());
      return {id:"skill-session",process_id:"skill-process",profile:"skill",provider:"codex",cwd:"/tmp/app"};
    }
    if (pathname === "/api/terminal/skill-session") return {id:"skill-session",process_id:"skill-process",state:"running",profile:"skill",provider:"codex",cwd:"/tmp/app"};
    return apiFixture(pathname);
  };
  return {fixture,records,writes,launches};
}

test("Custom Skills open a selected agent tab with typed inputs and no Goal context", {skip:SKIP}, async () => {
  const data = skillFixture();
  data.records.push({item:{...data.records[0].item,id:"automatic",name:"Quality check"},trigger:{...data.records[0].trigger,source:"workflow.quality.enter"}});
  const app = await openApp({fixture:data.fixture});
  try {
    const page = app.page;
    await page.goto(`${app.origin}/#/settings/skills`);
    await page.evaluate(() => { state.currentGoal = "GOAL1"; });
    await page.locator('#nav-context-menu > summary').click();
    const nav = page.locator('#nav-manual-skills');
    await nav.locator('[data-manual-skill="inspect"]').waitFor();
    assert.equal(await nav.locator('.nav-menu-label.nav-context-section-label').textContent(),"Skills");
    assert.equal(await nav.locator('[data-manual-skill]').count(),1);
    assert.equal(await nav.locator('button').last().textContent(),"Add skill...");
    await nav.locator('[data-manual-skill="inspect"]').click();
    const modal = page.locator('[data-testid="automation-modal"]');
    assert.equal(await modal.locator('[data-parameter-index="0"]').inputValue(),"3");
    await modal.locator('[data-parameter-index="0"]').fill("5");
    await modal.locator('[data-save]').click();
    await modal.waitFor({state:"detached"});
    await page.locator('[data-testid="terminal-profile"]').filter({hasText:"Inspect release"}).waitFor();
    assert.equal(data.launches.length,1);
    assert.equal(data.launches[0].profile,"skill");
    assert.equal(data.launches[0].surface,"toolbar");
    assert.equal(data.launches[0].skill_id,"inspect");
    assert.deepEqual(data.launches[0].parameters,{count:5});
    assert.equal(data.launches[0].goal_id,undefined);
    assert.equal(data.launches[0].feature_id,undefined);
    assert.equal(await page.evaluate(() => commandRegistry.has('skill.manual.inspect')),true);
    const saved = await page.evaluate(() => JSON.parse(sessionStorage.getItem('refine_chat_tabs')));
    assert.equal(Object.values(saved.tabs).find(tab => tab.mode === 'skill').sessionId,"skill-session");
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});

test("Skills use one trigger, shared modal controls and clickable rows with cloning", {skip:SKIP}, async () => {
  const data = skillFixture();
  const app = await openApp({fixture:data.fixture});
  try {
    const page = app.page;
    await page.goto(`${app.origin}/#/settings/events`);
    await page.locator('[data-testid="settings-skills"]').waitFor();
    assert.equal(new URL(page.url()).hash,"#/settings/skills");
    assert.deepEqual(await page.locator('.settings-tab').allTextContents().then(labels=>labels.map(s=>s.trim())),["Processes","Application","Reporters","Skills","Target App","Runtime"]);
    assert.equal(await page.locator('[data-testid="automation-table"] td:first-child button').count(),0);
    await page.locator('[data-automation-row]').focus(); await page.keyboard.press('Enter');
    const modal = page.locator('[data-testid="automation-modal"]');
    await modal.waitFor();
    assert.equal(await modal.locator('.modal-title').textContent(), 'Inspect release — Edit Skill');
    assert.equal(await modal.locator('#automation-name').isVisible(), false);
    assert.equal(await modal.locator('[data-prompt]').isVisible(), false);
    assert.equal(await modal.locator('[data-settings-markdown-preview]').textContent().then(s => s.trim()), 'Inspect the release.');
    await modal.locator('[data-settings-markdown-edit]').click();
    await modal.locator('[data-prompt]').fill('## Release instructions\n\nVerify **all changes**.');
    await modal.locator('[data-settings-markdown-edit]').click();
    assert.equal(await modal.locator('[data-settings-markdown-preview] h2').textContent(), 'Release instructions');
    assert.equal(await modal.locator('[data-settings-markdown-preview] strong').textContent(), 'all changes');
    await modal.locator('[data-skill-settings] > summary').click();
    await modal.locator('#automation-name').waitFor();
    assert.equal(await modal.locator('[data-trigger-source]').count(),1);
    assert.equal(await modal.locator('[data-role], [data-add-binding], [data-overrides]').count(),0);
    assert.doesNotMatch(await modal.textContent(),/Result role|Override project assignment/);
    assert.equal(await modal.locator('[data-automatic-options]').isVisible(),false);
    await modal.locator('[data-trigger-source]').selectOption('workflow.quality.enter');
    assert.equal(await modal.locator('[data-automatic-options]').isVisible(),true);
    await modal.locator('[data-mode] [data-choice="background"]').click();
    await modal.locator('[data-order]').fill('3');
    await modal.locator('[data-scope] [data-choice="node"]').click();
    assert.equal(await modal.locator('[data-scope-node]').inputValue(),'node-a');
    const parameter = modal.locator('[data-parameter]');
    await parameter.locator('[data-name]').fill('channel'); await parameter.locator('[data-name]').press('Tab');
    await parameter.locator('[data-kind]').selectOption('choice');
    await parameter.locator('[data-choices]').fill('stable, beta'); await parameter.locator('[data-choices]').press('Tab');
    await parameter.locator('[data-default]').selectOption('beta');
    await modal.locator('[data-input-name="channel"]').fill('system.node_id');
    await modal.locator('#automation-name').focus();
    const style = await modal.locator('#automation-name').evaluate(input => {const s=getComputedStyle(input); return {height:s.height,weight:s.fontWeight,border:s.borderColor,outline:s.outlineColor};});
    assert.deepEqual(style,{height:'34px',weight:'400',border:style.border,outline:style.border});
    await modal.locator('[data-save]').click(); await modal.waitFor({state:'detached'});
    assert.match(data.writes[0].item.prompt, /## Release instructions/);
    assert.equal(data.writes[0].trigger.source,'workflow.quality.enter');
    assert.equal(data.writes[0].trigger.mode,'background');
    assert.equal(data.writes[0].trigger.order,3);
    assert.equal(data.writes[0].item.scope.node_id,'node-a');
    assert.equal(data.writes[0].item.role,undefined);
    assert.equal(data.records[0].item.parameters[0].default,'beta');
    await page.locator('[data-automation-row]').click();
    await modal.locator('[data-clone-skill]').click();
    await modal.locator('[data-skill-settings] > summary').click();
    await modal.locator('#automation-name').waitFor();
    assert.equal(await modal.locator('#automation-name').inputValue(),'Inspect release copy');
    assert.equal(await modal.locator('[data-delete]').isVisible(),false);
    assert.equal(await modal.locator('[data-kind]').inputValue(),'choice');
    await modal.locator('[data-trigger-source]').selectOption('custom');
    await modal.locator('[data-save]').click(); await modal.waitFor({state:'detached'});
    assert.equal(data.records.length,2);
    assert.notEqual(data.records[0].item.id,data.records[1].item.id);
    assert.equal(data.records[1].trigger.source,'custom');
    assert.equal(data.records[0].trigger.source,'workflow.quality.enter');
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});

test("Skill status toggles preserve the trigger, fence duplicate saves and refresh conflicts", {skip:SKIP}, async () => {
  const data = skillFixture(); let releaseWrite;
  const pending = new Promise(resolve=>{releaseWrite=resolve;});
  let held = false;
  const app = await openApp({fixture:async (pathname,request)=>{
    if (request.method()==='PUT' && !held) {held=true;await pending;}
    return data.fixture(pathname,request);
  }});
  try {
    const page = app.page; await page.goto(`${app.origin}/#/settings/skills`);
    const status = page.locator('[data-automation-status="inspect"]');
    await status.locator('[data-choice="false"]').click();
    assert.equal(await status.locator('[data-choice="true"]').isDisabled(),true);
    await status.locator('[data-choice="false"]').evaluate(button=>button.click());
    releaseWrite();
    await status.locator('[data-choice="false"][aria-pressed="true"]').waitFor();
    assert.equal(data.writes.length,1); assert.equal(data.writes[0].trigger,undefined);
    assert.equal(data.records[0].trigger.source,'custom');
    await page.waitForFunction(()=>!commandRegistry.has('skill.manual.inspect'));
    assert.equal(await page.locator('[data-testid="automation-modal"]').count(),0);
    await page.reload(); await status.locator('[data-choice="false"][aria-pressed="true"]').waitFor();
    await status.locator('[data-choice="true"]').focus(); await page.keyboard.press('Enter');
    await page.waitForFunction(()=>commandRegistry.has('skill.manual.inspect'));
    await page.route('**/api/skills/inspect',route=>{
      if(route.request().method()!=='PUT') return route.fallback();
      data.records[0].item.name='Renamed concurrently';
      return route.fulfill({status:409,contentType:'application/json',body:JSON.stringify({error:{code:'conflict',message:'Configuration changed. Refresh before saving.'}})});
    });
    await status.locator('[data-choice="false"]').click();
    await page.locator('[data-automation-row]').filter({hasText:'Renamed concurrently'}).waitFor();
    assert.equal(await status.locator('[data-choice="true"]').getAttribute('aria-pressed'),'true');
    assert.equal(data.writes.length,2);
    assert.deepEqual(app.pageErrors,[]);
  } finally {releaseWrite();await app.close();}
});


test("New Skill starts with instructions and reveals required settings before saving", {skip:SKIP}, async () => {
  const data = skillFixture();
  const app = await openApp({fixture:data.fixture});
  try {
    await app.page.goto(`${app.origin}/#/settings/skills`);
    await app.page.locator('[data-automation-new]').click();
    const modal = app.page.locator('[data-testid="automation-modal"]');
    await modal.locator('[data-prompt]').waitFor();
    assert.equal(await modal.locator('[data-prompt]').isVisible(), true);
    assert.equal(await modal.locator('#automation-name').isVisible(), false);
    await modal.locator('[data-prompt]').fill('## Check the release\n\nSummarize changes.');
    await modal.locator('[data-save]').click();
    assert.equal(data.writes.length, 0);
    assert.equal(await modal.locator('#automation-name').isVisible(), true);
    await modal.locator('#automation-name').fill('New release check');
    await modal.locator('[data-save]').click();
    await modal.waitFor({state:'detached'});
    assert.equal(data.writes[0].item.name, 'New release check');
    assert.match(data.writes[0].item.prompt, /## Check the release/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});


test("Skill History opens once after refreshes and repeated clicks with spaced pagination", {skip:SKIP}, async () => {
  const data = skillFixture(); let historyReads = 0;
  const app = await openApp({fixture:async (pathname,request) => {
    if (pathname === '/api/event-invocations') {
      historyReads++;
      await new Promise(resolve=>setTimeout(resolve,100));
      return {items:[],offset:0,total:0};
    }
    return data.fixture(pathname,request);
  }});
  try {
    const page = app.page;
    await page.goto(`${app.origin}/#/settings/skills`);
    await page.locator('[data-event-history]').waitFor();
    assert.deepEqual(await page.locator('[data-testid="settings-skills"] > .actions > button').allTextContents(), ['History','New Skill']);
    await page.evaluate(async () => {
      await refreshSettings({force:true}); await refreshSettings({force:true});
      const button=document.querySelector('[data-event-history]'); button.click(); button.click(); button.click();
    });
    const modal=page.locator('[data-testid="automation-modal"]');
    await modal.waitFor();
    assert.equal(historyReads,1); assert.equal(await modal.count(),1);
    const bounds=await modal.evaluate(el=>({table:el.querySelector('table').getBoundingClientRect().bottom,previous:el.querySelector('[data-previous]').getBoundingClientRect().toJSON(),next:el.querySelector('[data-next]').getBoundingClientRect().toJSON()}));
    assert.ok(bounds.previous.top-bounds.table>=12);
    assert.ok(bounds.next.left-bounds.previous.right>=8);
    await modal.locator('[data-save]').click();
    await page.waitForFunction(()=>document.querySelectorAll('[data-testid="automation-modal"]').length===1);
    assert.equal(historyReads,2);
    await modal.locator('[data-close]').click();
    assert.equal(await modal.count(),0);
    assert.deepEqual(app.pageErrors,[]);
  } finally { await app.close(); }
});
