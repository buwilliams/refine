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
  emitError() { this.onerror?.(new Error("stream interrupted")); }
}

class FakeResizeObserver {
  static instances = [];
  constructor(callback) {
    this.callback = callback;
    this.target = null;
    this.disconnected = false;
    FakeResizeObserver.instances.push(this);
  }
  observe(target) { this.target = target; }
  disconnect() { this.disconnected = true; }
  trigger() {
    if (!this.disconnected && this.target) this.callback([{ target: this.target }]);
  }
}

function browserRuntime(storage = new Map()) {
  FakeEventSource.instances = [];
  FakeResizeObserver.instances = [];
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
    ResizeObserver: FakeResizeObserver,
    URLSearchParams,
    clearInterval() {},
    clearTimeout,
    console,
    document,
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    getComputedStyle: () => ({
      fontFamily: "monospace",
      fontSize: "15px",
      lineHeight: "20.25px",
      paddingBottom: "12px",
      paddingLeft: "16px",
      paddingRight: "16px",
      paddingTop: "12px",
    }),
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
      getComputedStyle: () => ({
        fontFamily: "monospace",
        fontSize: "15px",
        lineHeight: "20.25px",
        paddingBottom: "12px",
        paddingLeft: "16px",
        paddingRight: "16px",
        paddingTop: "12px",
      }),
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
        return activateToolbarTab(tabId);
      },
      click(tabId) { return activateToolbarTab(tabId, { toggleIfActive: true }); },
      draw: drawToolbar,
      ensurePlan: ensurePlanTab,
      openGoal(goalId) { return openAgentDock({ goalId }); },
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
      close(tabId) { return closeChatTab(tabId); },
      tab(tabId) { return chatState.tabs[tabId]; },
      terminal(tabId) {
        const value = terminalStateFor(tabId);
        return {
          sessionId: value?.sessionId,
          processId: value?.processId,
          connected: value?.connected,
          exited: value?.exited,
          statusChecked: value?.statusChecked,
          reattaching: value?.reattaching,
          display: value?.display,
        };
      },
      tabIds() { return Object.keys(chatState.tabs); },
      setApi(nextApi) { api = nextApi; },
      installTerminalResizer(tabId, resize) {
        terminalStateFor(tabId).term = { resize };
      },
      installTerminalScrollModel(tabId) {
        const terminal = terminalStateFor(tabId);
        const buffer = { baseY: 0, viewportY: 0 };
        let forcedScrolls = 0;
        terminal.term = {
          buffer: { active: buffer },
          write() {
            const wasAtBottom = buffer.viewportY === buffer.baseY;
            buffer.baseY += 1;
            if (wasAtBottom) buffer.viewportY = buffer.baseY;
          },
          scrollToBottom() {
            forcedScrolls += 1;
            buffer.viewportY = buffer.baseY;
          },
        };
        return {
          position() {
            return {
              baseY: buffer.baseY,
              viewportY: buffer.viewportY,
              forcedScrolls,
            };
          },
          scrollUp(lines = 1) {
            buffer.viewportY = Math.max(0, buffer.viewportY - lines);
          },
          scrollToBottom() {
            buffer.viewportY = buffer.baseY;
          },
        };
      },
      receive(tabId, text) {
        terminalReceiveOutput(text, terminalStateFor(tabId));
      },
      resizeOutput(width, height) {
        const output = document.querySelector(".terminal-output");
        output.clientWidth = width;
        output.clientHeight = height;
        const terminal = terminalStateFor(chatState.activeTabId);
        terminal.outputResizeObserver?.trigger();
      },
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
  await browser.runtime.openGoal("GOAL1");

  for (const tabId of ["supervisor", "plan", "GOAL1", "standalone", "terminal"]) {
    await browser.runtime.activate(tabId);
    assert.match(browser.html(), /data-testid="toolbar-terminal-panel"/);
    assert.match(browser.html(), /data-testid="terminal-start"/);
    assert.doesNotMatch(browser.html(), /id="chat-input"/);
    assert.doesNotMatch(browser.html(), /data-testid="toolbar-supervisor-panel"/);
  }
});

test("agent terminals follow at the bottom and preserve user scrollback until returned", async () => {
  const browser = browserRuntime();
  await browser.runtime.openPlan("Design a retry queue");
  await browser.runtime.openGoal("GOAL1");

  for (const tabId of ["supervisor", "plan", "GOAL1", "standalone"]) {
    const scroll = browser.runtime.installTerminalScrollModel(tabId);

    browser.runtime.receive(tabId, "first line\n");
    assert.deepEqual({ ...scroll.position() }, { baseY: 1, viewportY: 1, forcedScrolls: 0 });

    scroll.scrollUp();
    browser.runtime.receive(tabId, "second line\n");
    assert.deepEqual({ ...scroll.position() }, { baseY: 2, viewportY: 0, forcedScrolls: 0 });

    scroll.scrollToBottom();
    browser.runtime.receive(tabId, "third line\n");
    assert.deepEqual({ ...scroll.position() }, { baseY: 3, viewportY: 3, forcedScrolls: 0 });
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
  await browser.runtime.openGoal("GOAL1");
  for (const tabId of ["terminal", "supervisor", "standalone"]) {
    await browser.runtime.activate(tabId);
  }

  const starts = requests.filter((request) => request.path === "/api/terminal/session");
  assert.deepEqual(starts.map((request) => request.body.profile), [
    "plan", "goal", "terminal", "supervisor", "standalone",
  ]);
  assert.equal(starts.find((request) => request.body.profile === "goal").body.goal_id, "GOAL1");
  assert.equal(starts.find((request) => request.body.profile === "plan").body.initial_prompt, "Design a retry queue");
  assert.equal(browser.runtime.tab("standalone").worktree.path, "/tmp/worktree-5");
  assert.equal(browser.runtime.terminal("supervisor").processId, "interactive-4");
  assert.equal(browser.runtime.terminal("GOAL1").processId, "interactive-2");

  await browser.runtime.stop("standalone");
  await browser.runtime.activate("standalone");
  const restarted = requests.filter((request) => request.path === "/api/terminal/session").at(-1);
  assert.equal(restarted.body.worktree.path, "/tmp/worktree-5");
  assert.equal(browser.runtime.tab("standalone").worktree.path, "/tmp/worktree-5");
});

test("Stop and tab reactivation use terminal lifecycle routes", async () => {
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
  assert.match(browser.html(), />Restart</);
  await browser.runtime.activate("supervisor");
  assert.equal(browser.runtime.terminal("supervisor").sessionId, "supervisor-2");
  assert.deepEqual(requests.map((request) => request[1]), [
    "/api/terminal/session",
    "/api/terminal/supervisor-1/stop",
    "/api/terminal/session",
  ]);
});

test("clicking a stopped terminal tab starts it once", async () => {
  const browser = browserRuntime();
  const requests = [];
  browser.runtime.setApi(async (_method, requestPath, body) => {
    requests.push(requestPath);
    if (requestPath !== "/api/terminal/session") return { ok: true };
    return {
      id: `session-${body.profile}`,
      process_id: `interactive-${body.profile}`,
      cwd: "/repo",
      profile: body.profile,
      provider: body.profile === "terminal" ? null : "codex",
    };
  });

  await browser.runtime.click("terminal");
  await browser.runtime.click("terminal");

  assert.equal(browser.runtime.terminal("terminal").connected, true);
  assert.deepEqual(requests, ["/api/terminal/session"]);
});

test("terminal columns refit when its rendered width changes", async () => {
  const browser = browserRuntime();
  const requests = [];
  const sizes = [];
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push({ method, path: requestPath, body });
    if (requestPath !== "/api/terminal/session") return { ok: true };
    return {
      id: "responsive-terminal",
      process_id: "interactive-responsive-terminal",
      cwd: "/repo",
      profile: "terminal",
      provider: null,
    };
  });
  await browser.runtime.click("terminal");
  browser.runtime.installTerminalResizer("terminal", (cols, rows) => sizes.push({ cols, rows }));

  browser.runtime.resizeOutput(600, 300);
  browser.runtime.resizeOutput(1000, 300);
  await new Promise((resolve) => setTimeout(resolve, 100));

  assert.equal(sizes.length, 2);
  assert.ok(sizes[1].cols > sizes[0].cols);
  assert.equal(sizes[1].rows, sizes[0].rows);
  const backendResize = requests.filter((request) => request.path.endsWith("/resize")).at(-1);
  assert.equal(backendResize.body.cols, sizes[1].cols);
  assert.equal(backendResize.body.rows, sizes[1].rows);
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

test("stored custom-chat ids are discarded while managed terminal ids reattach", async () => {
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
    activeTabId: "terminal",
    open: true,
  }));
  const browser = browserRuntime(storage);
  browser.runtime.setApi(async (_method, requestPath) => {
    assert.equal(requestPath, "/api/terminal/managed-terminal/status");
    return {
      id: "managed-terminal",
      process_id: "interactive-managed",
      profile: "terminal",
      provider: null,
      cwd: "/repo",
      worktree: null,
      alive: true,
      exited: false,
    };
  });
  browser.runtime.restore();
  browser.runtime.draw();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(browser.runtime.tab("supervisor").sessionId, null);
  assert.equal(browser.runtime.tab("terminal").sessionId, "managed-terminal");
  assert.equal(browser.runtime.terminal("terminal").connected, true);
});

test("refresh reattaches a live terminal and stream errors do not persist process exit", async () => {
  const storage = new Map();
  storage.set("refine_chat_tabs", JSON.stringify({
    tabs: {
      terminal: {
        label: "Terminal",
        mode: "terminal",
        sessionId: "managed-terminal",
        processId: "interactive-managed",
        cwd: "/repo",
        exited: true,
      },
    },
    activeTabId: "terminal",
    open: true,
  }));
  const browser = browserRuntime(storage);
  const requests = [];
  browser.runtime.setApi(async (method, requestPath) => {
    requests.push([method, requestPath]);
    return {
      id: "managed-terminal",
      process_id: "interactive-managed",
      profile: "terminal",
      provider: null,
      cwd: "/repo",
      worktree: null,
      alive: true,
      exited: false,
    };
  });

  browser.runtime.restore();
  browser.runtime.draw();
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(browser.runtime.terminal("terminal").connected, true);
  assert.equal(browser.runtime.terminal("terminal").exited, false);
  assert.doesNotMatch(browser.html(), />Restart</);
  assert.equal(browser.events().length, 1);

  browser.events()[0].emitError();
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(browser.runtime.terminal("terminal").connected, true);
  assert.equal(browser.runtime.terminal("terminal").exited, false);
  assert.equal(browser.runtime.tab("terminal").exited, false);
  assert.doesNotMatch(browser.html(), />Restart</);
  assert.deepEqual(requests, [
    ["GET", "/api/terminal/managed-terminal/status"],
    ["GET", "/api/terminal/managed-terminal/status"],
  ]);
  const persisted = JSON.parse(storage.get("refine_chat_tabs"));
  assert.equal(persisted.tabs.terminal.exited, false);
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

test("closing a Goal Agent tab detaches without stopping workflow-owned work", async () => {
  const browser = browserRuntime();
  const requests = [];
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push([method, requestPath, body]);
    if (requestPath !== "/api/terminal/session") return { ok: true };
    return {
      id: "goal-session",
      process_id: "goal-agent-process",
      cwd: "/repo/worktree",
      profile: body.profile,
      provider: "codex",
      goal_id: body.goal_id,
    };
  });

  await browser.runtime.openGoal("GOAL1");
  await browser.runtime.close("GOAL1");

  assert.equal(browser.runtime.tab("GOAL1"), undefined);
  assert.deepEqual(
    requests.filter((request) => request[1].endsWith("/stop")),
    [],
  );
});
