const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");

const STATIC = path.resolve(__dirname, "../../src/surfaces/web/static");

function loadChromium() {
  let chromium;
  try { ({ chromium } = require("playwright")); } catch { return null; }
  const candidates = [chromium.executablePath()];
  const cache = path.join(process.env.HOME || "", process.platform === "darwin"
    ? "Library/Caches/ms-playwright" : ".cache/ms-playwright");
  if (fs.existsSync(cache)) {
    for (const entry of fs.readdirSync(cache)) {
      if (!entry.startsWith("chromium-")) continue;
      for (const rel of ["chrome-linux64/chrome", "chrome-linux/chrome",
        "chrome-mac/Chromium.app/Contents/MacOS/Chromium", "chrome-win/chrome.exe"]) {
        candidates.push(path.join(cache, entry, rel));
      }
    }
  }
  const executablePath = candidates.find((file) => fs.existsSync(file));
  return executablePath ? { chromium, executablePath } : null;
}

const BROWSER = loadChromium();
const SKIP = BROWSER ? false : "no Playwright chromium build is available";

async function openTerminalApp({ platform = "Linux x86_64", onInput } = {}) {
  const server = http.createServer((request, response) => {
    const pathname = new URL(request.url, "http://localhost").pathname;
    const file = path.resolve(STATIC, pathname === "/" ? "index.html"
      : pathname.replace(/^\/static\//, "").replace(/^\//, ""));
    if (!file.startsWith(STATIC + path.sep) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      response.writeHead(404).end();
      return;
    }
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
    response.writeHead(200, { "content-type": contentType[path.extname(file)] || "application/octet-stream" });
    response.end(fs.readFileSync(file));
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const browser = await BROWSER.chromium.launch({ executablePath: BROWSER.executablePath });
  const context = await browser.newContext({ permissions: ["clipboard-read", "clipboard-write"] });
  const page = await context.newPage();
  const errors = [];
  const inputs = [];
  await page.addInitScript((platform) => {
    Object.defineProperty(navigator, "platform", { get: () => platform });
    localStorage.setItem("refine_last_reporter", "Clipboard test");
  }, platform);
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const { pathname } = new URL(request.url());
    if (pathname.endsWith("/sse") || pathname.endsWith("/events")) {
      await route.fulfill({ contentType: "text/event-stream", body: "" });
      return;
    }
    if (pathname.endsWith("/input")) {
      inputs.push({ path: pathname, data: request.postDataJSON().data });
      if (onInput) await onInput(request.postDataJSON().data);
    }
    let body = {};
    if (pathname.startsWith("/api/terminal/") && pathname.endsWith("/status")) body = { alive: true };
    if (pathname === "/api/project/status") body = { attached: true, target_root: "/tmp/clipboard-test", nodes: [] };
    if (pathname === "/api/reporters") body = { reporters: [{ name: "Clipboard test" }] };
    if (pathname === "/api/settings") body = { settings: {} };
    if (pathname === "/api/nodes") body = { nodes: [], active_node_id: "default" };
    if (pathname === "/api/dashboard") body = { counts: {}, needs_attention: [] };
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(body) });
  });
  await page.goto(`http://127.0.0.1:${server.address().port}/#/dashboard`);
  await page.waitForFunction(() => typeof ensureTerminalRenderer === "function");
  await page.evaluate(() => {
    window.clipboardEvents = [];
    for (const type of ["keydown", "copy", "paste"]) {
      document.addEventListener(type, (event) => {
        const entry = { type, key: event.key, trusted: event.isTrusted };
        window.clipboardEvents.push(entry);
        // Native events can run microtasks between listeners. Sample after the
        // entire dispatch so a later capture listener's cancellation is visible.
        setTimeout(() => { entry.prevented = event.defaultPrevented; }, 0);
      }, true);
    }
  });
  return {
    page, context, errors, inputs,
    async addTab({ id = "agent", mode = "agent", provider = "claude" } = {}) {
      await page.evaluate(({ id, mode, provider }) => {
        chatState.tabs[id] = normalizeInteractiveTerminalTab({ label: id, mode, provider });
        chatState.activeTabId = id;
        chatState.open = true;
        chatState.bodyHeight = 350;
        const terminal = terminalStateFor(id);
        terminal.sessionId = `session-${id}`;
        terminal.statusChecked = true;
        terminal.connected = true;
        terminal.exited = false;
        drawToolbar();
        terminal.term.focus();
      }, { id, mode, provider });
      await page.waitForSelector(".xterm-screen");
    },
    async output(data) {
      await page.evaluate((data) => new Promise((resolve) => terminalStateFor().term.write(data, resolve)), data);
    },
    async settle() {
      await page.evaluate(async () => {
        for (const terminal of terminalStates.values()) {
          flushTerminalInput(terminal);
          await terminal.inputSendPromise;
        }
      });
    },
    async close() {
      await browser.close();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

async function selectText(page, { row = 0, start = 0, length, modifier = null }) {
  const geometry = await page.evaluate(() => {
    const term = terminalStateFor().term;
    const rect = term.element.querySelector(".xterm-screen").getBoundingClientRect();
    return { x: rect.x, y: rect.y, width: rect.width / term.cols, height: rect.height / term.rows };
  });
  if (modifier) await page.keyboard.down(modifier);
  await page.mouse.move(geometry.x + (start + 0.1) * geometry.width, geometry.y + (row + 0.5) * geometry.height);
  await page.mouse.down();
  await page.mouse.move(geometry.x + (start + length + 0.1) * geometry.width,
    geometry.y + (row + 0.5) * geometry.height, { steps: 10 });
  await page.mouse.up();
  if (modifier) await page.keyboard.up(modifier);
  return page.evaluate(() => terminalStateFor().term.getSelection());
}

module.exports = { SKIP, openTerminalApp, selectText };
