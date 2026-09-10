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
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");

const STATIC = path.join(__dirname, "../../src/surfaces/web/static");

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
  node_display_name: "A very long remote node name that must stay inside its column",
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
  if (pathname === "/api/skills/catalog") return {sources:["custom", "workflow.quality.enter", "workflow.quality.exit", "node.startup.ready"]};
  if (pathname === "/api/event-definitions/catalog") return {sources: ["workflow.quality.enter"], roles: ["task", "plan", "implement", "quality", "governance"]};
  if (pathname === "/api/event-definitions" || pathname === "/api/skills") return {revision:1, items:[]};
  if (pathname === "/api/event-invocations") return {items:[], offset:0, total:0};
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

async function openApp({
  fixture = apiFixture,
  onRequest = null,
  selectedReporter = "Reporter",
} = {}) {
  const server = await serveStaticTree();
  const browser = await BROWSER.chromium.launch({ executablePath: BROWSER.executablePath });
  const page = await browser.newPage();
  if (selectedReporter !== null) {
    await page.addInitScript((reporter) => {
      localStorage.setItem("refine_last_reporter", reporter);
    }, selectedReporter);
  }
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

module.exports = { openApp, apiFixture, GOAL, FEATURE, SKIP };
