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

async function openApp() {
  const server = await serveStaticTree();
  const browser = await BROWSER.chromium.launch({ executablePath: BROWSER.executablePath });
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(String(error.message).split("\n")[0]));
  await page.route("**/api/**", (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname === "/api/sse") {
      route.fulfill({ status: 200, contentType: "text/event-stream", body: "" });
      return;
    }
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(apiFixture(pathname)),
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
