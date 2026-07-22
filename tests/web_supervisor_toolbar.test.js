const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

class FakeClassList {
  constructor() { this.values = new Set(); }
  add(...names) { names.forEach((name) => this.values.add(name)); }
  remove(...names) { names.forEach((name) => this.values.delete(name)); }
  toggle(name, force) {
    const enabled = force === undefined ? !this.values.has(name) : !!force;
    if (enabled) this.values.add(name);
    else this.values.delete(name);
    return enabled;
  }
}

class FakeElement {
  constructor() {
    this.classList = new FakeClassList();
    this.dataset = {};
    this.style = {};
    this.listeners = new Map();
    this._innerHTML = "";
    this.clientWidth = 1000;
    this.clientHeight = 400;
    this.scrollHeight = 0;
    this.scrollTop = 0;
  }
  get innerHTML() { return this._innerHTML; }
  set innerHTML(value) { this._innerHTML = String(value); }
  addEventListener(type, listener) { this.listeners.set(type, listener); }
  focus() {}
  querySelector() { return null; }
  querySelectorAll() { return []; }
}

class FakeEventSource {
  static instances = [];
  constructor(url) {
    this.url = url;
    this.listeners = new Map();
    this.closed = false;
    FakeEventSource.instances.push(this);
  }
  addEventListener(name, listener) { this.listeners.set(name, listener); }
  close() { this.closed = true; }
  emit(name, payload) { this.listeners.get(name)?.({ data: JSON.stringify(payload) }); }
}

function browserRuntime(storage = new Map()) {
  FakeEventSource.instances = [];
  const toolbar = new FakeElement();
  const terminalOutput = new FakeElement();
  const document = {
    activeElement: null,
    body: { appendChild() {} },
    documentElement: { style: { setProperty() {} } },
    addEventListener() {},
    createElement() { return new FakeElement(); },
    getElementById() { return null; },
    querySelector(selector) {
      if (selector === "#toolbar-dock") return toolbar;
      if (selector === ".terminal-output" && toolbar.innerHTML.includes("terminal-output")) return terminalOutput;
      return null;
    },
    querySelectorAll() { return []; },
  };
  toolbar.querySelector = (selector) => {
    if (selector === ".terminal-output" && toolbar.innerHTML.includes("terminal-output")) return terminalOutput;
    return null;
  };
  const context = vm.createContext({
    AbortController,
    EventSource: FakeEventSource,
    URLSearchParams,
    clearInterval() {},
    clearTimeout,
    console,
    document,
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    getComputedStyle: () => ({ fontSize: "13", lineHeight: "18" }),
    location: { hash: "#/dashboard", pathname: "/" },
    localStorage: {
      getItem(key) { return storage.get(key) ?? null; },
      setItem(key, value) { storage.set(key, String(value)); },
    },
    requestAnimationFrame(callback) { callback(); },
    setInterval() { return 1; },
    setTimeout,
    window: {
      addEventListener() {},
      CSS: { escape: (value) => String(value) },
      getComputedStyle: () => ({ fontSize: "13", lineHeight: "18" }),
      innerHeight: 800,
    },
    withButtonBusy: async (_button, _label, action) => action(),
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "common.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/toolbar.js"), "utf8"), context);
  vm.runInContext(`
    globalThis.toolbarTerminalTest = {
      activate(tabId) {
        ensureStandaloneTab();
        if (tabId === "plan") ensurePlanTab();
        chatState.activeTabId = tabId;
        chatState.open = true;
        drawToolbar();
      },
      draw: drawToolbar,
      ensurePlan: ensurePlanTab,
      openGoal(goalId) { openChatDock({ goalId }); },
      openPlan(prompt = "") { return openPlanChatDock({ initialPrompt: prompt }); },
      restore: loadChatStateFromStorage,
      reset: resetChatForProjectSwitch,
      save: saveChatStateToStorage,
      start(tabId) {
        chatState.activeTabId = tabId;
        return startTerminalSession(chatState.tabs[tabId]);
      },
      stop(tabId) {
        chatState.activeTabId = tabId;
        return stopTerminalSession(chatState.tabs[tabId]);
      },
      tab(tabId) { return chatState.tabs[tabId]; },
      terminal(tabId) {
        const value = terminalStateFor(tabId);
        return {
          sessionId: value?.sessionId,
          processId: value?.processId,
          connected: value?.connected,
          exited: value?.exited,
          display: value?.display,
        };
      },
      tabIds() { return Object.keys(chatState.tabs); },
      setApi(nextApi) { api = nextApi; },
    };
  `, context);
  return {
    events: () => [...FakeEventSource.instances],
    html: () => toolbar.innerHTML,
    runtime: context.toolbarTerminalTest,
  };
}

test("Supervisor, Plan, Goal, and Standalone render the shared terminal surface", async () => {
  const browser = browserRuntime();
  await browser.runtime.openPlan("Design a retry queue");
  browser.runtime.openGoal("GOAL1");

  for (const tabId of ["supervisor", "plan", "GOAL1", "standalone", "terminal"]) {
    browser.runtime.activate(tabId);
    assert.match(browser.html(), /data-testid="toolbar-terminal-panel"/);
    assert.match(browser.html(), /data-testid="terminal-start"/);
    assert.doesNotMatch(browser.html(), /id="chat-input"/);
    assert.doesNotMatch(browser.html(), /data-testid="toolbar-supervisor-panel"/);
  }
});

test("each terminal profile sends its launch context and keeps an independent managed session", async () => {
  const browser = browserRuntime();
  const requests = [];
  let sequence = 0;
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push({ method, path: requestPath, body });
    if (requestPath !== "/api/terminal/session") return { ok: true };
    sequence += 1;
    const resumedWorktree = body.worktree || null;
    return {
      id: `session-${sequence}`,
      process_id: `interactive-${sequence}`,
      cwd: resumedWorktree?.path || (body.profile === "standalone" ? `/tmp/worktree-${sequence}` : "/repo"),
      profile: body.profile,
      provider: body.profile === "terminal" ? null : "codex",
      worktree: resumedWorktree || (body.profile === "standalone"
        ? { branch: `refine/standalone/${sequence}`, path: `/tmp/worktree-${sequence}` }
        : null),
    };
  });

  await browser.runtime.openPlan("Design a retry queue");
  browser.runtime.openGoal("GOAL1");
  for (const tabId of ["terminal", "supervisor", "plan", "GOAL1", "standalone"]) {
    await browser.runtime.start(tabId);
  }

  const starts = requests.filter((request) => request.path === "/api/terminal/session");
  assert.deepEqual(starts.map((request) => request.body.profile), [
    "terminal", "supervisor", "plan", "goal", "standalone",
  ]);
  assert.equal(starts.find((request) => request.body.profile === "goal").body.goal_id, "GOAL1");
  assert.equal(starts.find((request) => request.body.profile === "plan").body.initial_prompt, "Design a retry queue");
  assert.equal(browser.runtime.tab("standalone").worktree.path, "/tmp/worktree-5");
  assert.equal(browser.runtime.terminal("supervisor").processId, "interactive-2");
  assert.equal(browser.runtime.terminal("GOAL1").processId, "interactive-4");

  await browser.runtime.stop("standalone");
  await browser.runtime.start("standalone");
  const restarted = requests.filter((request) => request.path === "/api/terminal/session").at(-1);
  assert.equal(restarted.body.worktree.path, "/tmp/worktree-5");
  assert.equal(browser.runtime.tab("standalone").worktree.path, "/tmp/worktree-5");
});

test("Start, Stop, and Restart use terminal lifecycle routes", async () => {
  const browser = browserRuntime();
  const requests = [];
  let sequence = 0;
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push([method, requestPath, body]);
    if (requestPath === "/api/terminal/session") {
      sequence += 1;
      return {
        id: `supervisor-${sequence}`,
        process_id: `interactive-supervisor-${sequence}`,
        cwd: "/repo",
        profile: "supervisor",
        provider: "claude",
      };
    }
    return { ok: true };
  });

  await browser.runtime.start("supervisor");
  assert.equal(browser.runtime.terminal("supervisor").connected, true);
  await browser.runtime.stop("supervisor");
  assert.equal(browser.runtime.terminal("supervisor").exited, true);
  browser.runtime.activate("supervisor");
  assert.match(browser.html(), />Restart</);
  await browser.runtime.start("supervisor");
  assert.equal(browser.runtime.terminal("supervisor").sessionId, "supervisor-2");
  assert.deepEqual(requests.map((request) => request[1]), [
    "/api/terminal/session",
    "/api/terminal/supervisor-1/stop",
    "/api/terminal/session",
  ]);
});

test("terminal output and exit events remain scoped to their tab", async () => {
  const browser = browserRuntime();
  let sequence = 0;
  browser.runtime.setApi(async (_method, requestPath, body) => {
    if (requestPath !== "/api/terminal/session") return { ok: true };
    sequence += 1;
    return {
      id: `session-${sequence}`,
      process_id: `interactive-${sequence}`,
      cwd: "/repo",
      profile: body.profile,
      provider: body.profile === "terminal" ? null : "codex",
    };
  });
  await browser.runtime.start("terminal");
  await browser.runtime.start("supervisor");
  const [terminalEvents, supervisorEvents] = browser.events();
  terminalEvents.emit("terminal_output", { seq: 1, data: "shell output" });
  supervisorEvents.emit("terminal_output", { seq: 1, data: "agent output" });
  supervisorEvents.emit("terminal_exit", { seq: 2, data: "exit 0" });

  assert.equal(browser.runtime.terminal("terminal").display, "shell output");
  assert.equal(browser.runtime.terminal("supervisor").display, "agent outputexit 0");
  assert.equal(browser.runtime.terminal("terminal").connected, true);
  assert.equal(browser.runtime.terminal("supervisor").exited, true);
});

test("stored custom-chat ids are discarded while managed terminal ids reattach", () => {
  const storage = new Map();
  storage.set("refine_chat_tabs", JSON.stringify({
    tabs: {
      supervisor: { label: "Supervisor", mode: "supervisor", sessionId: "legacy-chat" },
      terminal: {
        label: "Terminal",
        mode: "terminal",
        sessionId: "managed-terminal",
        processId: "interactive-managed",
        cwd: "/repo",
      },
    },
    activeTabId: "supervisor",
    open: true,
  }));
  const browser = browserRuntime(storage);
  browser.runtime.restore();
  assert.equal(browser.runtime.tab("supervisor").sessionId, null);
  assert.equal(browser.runtime.tab("terminal").sessionId, "managed-terminal");
  assert.equal(browser.runtime.terminal("terminal").connected, true);
});

test("switching projects stops live terminal profiles before clearing the toolbar", async () => {
  const browser = browserRuntime();
  const requests = [];
  let sequence = 0;
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push([method, requestPath, body]);
    if (requestPath !== "/api/terminal/session") return { ok: true };
    sequence += 1;
    return {
      id: `session-${sequence}`,
      process_id: `interactive-${sequence}`,
      cwd: "/repo",
      profile: body.profile,
      provider: body.profile === "terminal" ? null : "codex",
    };
  });
  await browser.runtime.start("terminal");
  await browser.runtime.start("supervisor");
  browser.runtime.reset();
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.deepEqual(
    requests.filter((request) => request[1].endsWith("/stop")).map((request) => request[1]).sort(),
    ["/api/terminal/session-1/stop", "/api/terminal/session-2/stop"],
  );
});
