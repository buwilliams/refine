// Render-time smoke coverage for the screens the vm-based suites cannot reach.
//
// Those suites load one file into a `vm` with hand-written stubs, which is fine
// for logic but has no DOM: `document.querySelector` is never really called, so an
// invalid selector never throws and a screen can be completely broken while every
// test passes. That is exactly how a corrupted selector in goal detail shipped.
//
// This boots the real `index.html` in a browser against the real static tree, with
// the daemon API intercepted, and asserts each screen actually paints. It runs the
// genuine bootstrap path — init, routing, render, bind — rather than a stand-in.
//
// Served over http rather than `setContent`, because `common.js` reads
// `localStorage` at load and an opaque-origin document denies access, so the app
// would fail before defining any state.
//
// Skipped when no browser is available, so a machine without one still runs the
// rest of the suite.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const test = require("node:test");

const STATIC = path.join(__dirname, "../src/surfaces/web/static");

const CONTENT_TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".json": "application/json",
};

function loadChromium() {
  let chromium;
  try {
    ({ chromium } = require("playwright"));
  } catch {
    return null;
  }
  const candidates = [];
  try {
    candidates.push(chromium.executablePath());
  } catch {}
  // Playwright pins one browser build; a cache holding a different one is still
  // perfectly good for rendering a page, so fall back to whatever is installed.
  const cache = path.join(
    process.env.HOME || "",
    process.platform === "darwin" ? "Library/Caches/ms-playwright" : ".cache/ms-playwright",
  );
  if (fs.existsSync(cache)) {
    for (const entry of fs.readdirSync(cache)) {
      if (!entry.startsWith("chromium-")) continue;
      for (const rel of [
        "chrome-linux64/chrome",
        "chrome-linux/chrome",
        "chrome-mac/Chromium.app/Contents/MacOS/Chromium",
        "chrome-win/chrome.exe",
      ]) {
        candidates.push(path.join(cache, entry, rel));
      }
    }
  }
  const executablePath = candidates.find((candidate) => candidate && fs.existsSync(candidate));
  return executablePath ? { chromium, executablePath } : null;
}

const BROWSER = loadChromium();
const SKIP = BROWSER ? false : "no Playwright chromium build is available";

const GOAL = {
  id: "GOAL1",
  name: "Smoke goal",
  status: "review",
  priority: "high",
  reporter: "Reporter",
  assignee: "Reporter",
  node_id: "node-a",
  created: "2026-07-01T00:00:00Z",
  updated: "2026-07-02T00:00:00Z",
  notes: [{ id: "NOTE1", author: "Reviewer", body: "A note." }],
  rounds: [
    {
      prompt: "Do the thing",
      reporter: "Reporter",
      assignee: "Reporter",
      created: "2026-07-01T00:00:00Z",
      logs: [{ message: "started" }],
    },
  ],
};

const FEATURE = {
  id: "FEAT1",
  name: "Smoke feature",
  status: "todo",
  priority: "medium",
  reporter: "Reporter",
  assignee: "Reporter",
  node_id: "node-a",
  created: "2026-07-01T00:00:00Z",
  updated: "2026-07-02T00:00:00Z",
  goals: [GOAL],
};

// Enough shape for each screen to render. A screen that needs a field this omits
// should fail loudly here rather than only in a browser.
function apiFixture(pathname) {
  if (pathname.startsWith("/api/goals/")) return { goal: GOAL };
  if (pathname.startsWith("/api/goals")) {
    return { goals: [GOAL], facets: { status_counts: {} }, page: { page: 1, total: 1 } };
  }
  if (pathname.startsWith("/api/features/")) return { feature: FEATURE };
  if (pathname.startsWith("/api/features")) {
    return { features: [FEATURE], page: { page: 1, total: 1 } };
  }
  if (pathname.startsWith("/api/project/status")) {
    return {
      attached: true,
      target_root: "/tmp/app",
      registry_enabled: true,
      apps: [],
      nodes: [{ id: "node-a", display_name: "Node A" }],
      active_node_id: "node-a",
    };
  }
  if (pathname.startsWith("/api/reporters")) return { reporters: [{ name: "Reporter" }] };
  if (pathname.startsWith("/api/dashboard")) return { counts: {}, needs_attention: [] };
  if (pathname.startsWith("/api/nodes")) {
    return { nodes: [{ id: "node-a", display_name: "Node A" }], active_node_id: "node-a" };
  }
  if (pathname.startsWith("/api/settings")) return { settings: {} };
  if (pathname.startsWith("/api/activity")) {
    return { entries: [], facets: { categories: [], actors: [] }, page: { page: 1, total: 0 } };
  }
  if (pathname.startsWith("/api/changes")) {
    return { branch: "main", changes: [], page: { page: 1, total: 0 } };
  }
  if (pathname.startsWith("/api/diagnostics")) return {};
  if (pathname.startsWith("/api/quality")) return {};
  if (pathname.startsWith("/api/governance")) return {};
  if (pathname.startsWith("/api/guidance")) return { guidance: [] };
  if (pathname.startsWith("/api/processes")) return {};
  if (pathname.startsWith("/api/performance")) {
    return { events: [], summary: {}, backend: { store: "jsonl" } };
  }
  if (pathname.startsWith("/api/system/releases")) {
    return { releases: { operations: [] } };
  }
  if (pathname.startsWith("/api/system/source")) {
    return {
      source: {
        clean: true,
        fast_forward: true,
        update_available: false,
        active_work: [],
        checkout_path: "/tmp/refine",
        current_commit: "1111111111111111111111111111111111111111",
        available_commit: "1111111111111111111111111111111111111111",
        remote: "origin",
        branch: "main",
      },
      source_update: { visible: true, enabled: false, state: "current" },
    };
  }
  if (pathname.startsWith("/api/target-app/status")) return { state: "stopped" };
  if (pathname.startsWith("/api/upgrade")) return { upgrade: {} };
  return {};
}

function serveStaticTree() {
  const server = http.createServer((request, response) => {
    const requested = request.url.split("?")[0];
    const relative =
      requested === "/" ? "index.html" : requested.replace(/^\/static\//, "").replace(/^\//, "");
    const file = path.join(STATIC, relative);
    if (!file.startsWith(STATIC) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      response.writeHead(404).end("not found");
      return;
    }
    response.writeHead(200, {
      "content-type": CONTENT_TYPES[path.extname(file)] || "application/octet-stream",
    });
    response.end(fs.readFileSync(file));
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

async function openApp({ fixture = apiFixture, onRequest = null } = {}) {
  const server = await serveStaticTree();
  const browser = await BROWSER.chromium.launch({ executablePath: BROWSER.executablePath });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(String(error.message).split("\n")[0]));
  await page.route("**/api/**", async (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname === "/api/sse") {
      route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      return;
    }
    if (onRequest) await onRequest(pathname, route.request());
    const body = await fixture(pathname, route.request());
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(body),
    });
  });
  return {
    page,
    pageErrors,
    origin: `http://127.0.0.1:${server.address().port}`,
    async close() {
      await browser.close();
      server.close();
    },
  };
}

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

test("terminal tabs swap one mounted xterm and retain inactive scrollback", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, { route: "#/", marker: "#dash" });
    const result = await app.page.evaluate(async () => {
      const firstId = "renderer-agent";
      const secondId = "renderer-plan";
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
        [secondId]: makeTab("Planing Agent", "plan"),
      };
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

      chatState.activeTabId = secondId;
      drawToolbar();
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

      chatState.activeTabId = firstId;
      drawToolbar();
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
          && firstTerm.buffer.active.baseY === firstBase
          && firstTerm.buffer.active.viewportY === firstViewport
          && firstBuffer.includes("FIRST-SCROLLBACK-00")
          && firstBuffer.includes("FIRST-SCROLLBACK-END"),
        firstVisible:
          firstHost.querySelector(".xterm-rows")?.textContent.includes("FIRST-SCROLLBACK-00")
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
      firstVisible: true,
      firstExcludesSecond: true,
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

test("every Node, Project, and legacy Settings tab renders and refreshes", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    for (const [route, marker] of [
      ["#/node/application", '[data-testid="project-app-select"]'],
      ["#/node/reporters", '[data-testid="reporters-table"]'],
      ["#/node/processes", '[data-testid="settings-pane-processes"].active'],
      ["#/node/performance", '[data-testid="performance-refresh"]'],
      ["#/node/target-app", '[data-testid="target-app-copy-node"]'],
      ["#/node/runtime", '[data-testid="runtime-recheck-auth"]'],
      ["#/node/releases", '[data-testid="source-promotion-section"]'],
      ["#/project/governance", '[data-testid="governance-explanation"]'],
      ["#/project/quality", '[data-testid="quality-explanation"]'],
      ["#/project/guidance", '[data-testid="guidance-list"]'],
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

test("Refine dev source refreshes morph identical and changed status without replacement", { skip: SKIP }, async () => {
  const app = await openApp();
  try {
    await assertScreenRenders(app, {
      route: "#/node/releases",
      marker: '[data-testid="source-promotion-readiness"]',
    });
    const result = await app.page.evaluate(async () => {
      const source = {
        clean: true,
        fast_forward: true,
        update_available: false,
        active_work: [],
        checkout_path: "/tmp/refine",
        current_commit: "1111111111111111111111111111111111111111",
        available_commit: "1111111111111111111111111111111111111111",
        remote: "origin",
        branch: "main",
      };
      applySourcePromotionStatus(source);
      const root = document.getElementById("source-promotion-status");
      const facts = root.querySelector(".source-promotion-facts");
      const readiness = root.querySelector('[data-testid="source-promotion-readiness"]');
      let identicalMutations = 0;
      const observer = new MutationObserver((records) => {
        identicalMutations += records.length;
      });
      observer.observe(root, {
        childList: true,
        characterData: true,
        subtree: true,
      });
      applySourcePromotionStatus(source);
      await Promise.resolve();
      observer.disconnect();
      const identicalFacts = facts === root.querySelector(".source-promotion-facts");
      const identicalReadiness =
        readiness === root.querySelector('[data-testid="source-promotion-readiness"]');

      applySourcePromotionStatus({
        ...source,
        update_available: true,
        available_commit: "2222222222222222222222222222222222222222",
      });
      return {
        identicalFacts,
        identicalReadiness,
        identicalMutations,
        changedFacts: facts === root.querySelector(".source-promotion-facts"),
        changedReadiness:
          readiness === root.querySelector('[data-testid="source-promotion-readiness"]'),
        changedText: root.textContent,
      };
    });

    assert.deepEqual(
      {
        identicalFacts: result.identicalFacts,
        identicalReadiness: result.identicalReadiness,
        identicalMutations: result.identicalMutations,
        changedFacts: result.changedFacts,
        changedReadiness: result.changedReadiness,
      },
      {
        identicalFacts: true,
        identicalReadiness: true,
        identicalMutations: 0,
        changedFacts: true,
        changedReadiness: true,
      },
    );
    assert.match(result.changedText, /222222222222/);
    assert.match(result.changedText, /Ready to build, promote, and restart/);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test("settings refresh never clears the Upgrade banner while its read is pending", { skip: SKIP }, async () => {
  const fixture = (pathname, request) => {
    if (pathname.startsWith("/api/upgrade")) {
      return {
        upgrade: {
          current_version: "4.0.0",
          latest_version: "4.1.0",
          upgrade_available: true,
          local_development: false,
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
            current_version: "4.0.0",
            latest_version: "4.2.0",
            upgrade_available: true,
            local_development: false,
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
