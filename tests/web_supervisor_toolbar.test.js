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
  remove() {}
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
    function ensureTestTab(tabId) {
      if (chatState.tabs[tabId]) return;
      const mode = ["agent", "standalone", "terminal"].includes(tabId) ? tabId : tabId;
      const labels = {
        agent: "Agent",
        standalone: "Agent in Worktree",
        terminal: "Terminal",
      };
      chatState.tabs[tabId] = normalizeInteractiveTerminalTab({
        goalId: null,
        label: labels[tabId] || tabId,
        mode,
        sessionId: null,
      });
    }
    globalThis.toolbarTerminalTest = {
      activate(tabId) {
        ensureStandaloneTab();
        if (tabId === "plan") ensurePlanTab();
        else ensureTestTab(tabId);
        return activateToolbarTab(tabId);
      },
      click(tabId) {
        ensureTestTab(tabId);
        return activateToolbarTab(tabId, { toggleIfActive: true });
      },
      draw: drawToolbar,
      ensurePlan: ensurePlanTab,
      openGoal(goalId) { return openAgentDock({ goalId }); },
      openPlan(prompt = "") { return openPlanChatDock({ initialPrompt: prompt }); },
      create(mode) { return createToolbarTab(mode); },
      restore: loadChatStateFromStorage,
      reset: resetChatForProjectSwitch,
      save: saveChatStateToStorage,
      start(tabId) {
        ensureTestTab(tabId);
        chatState.activeTabId = tabId;
        return startTerminalSession(chatState.tabs[tabId]);
      },
      stop(tabId) {
        ensureTestTab(tabId);
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
        ensureTestTab(tabId);
        terminalStateFor(tabId).term = { resize };
      },
      installTerminalScrollModel(tabId) {
        ensureTestTab(tabId);
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

test("Toolbar starts empty and creates independent general Agent tabs lazily", async () => {
  const browser = browserRuntime();
  const requests = [];
  browser.runtime.setApi(async (_method, requestPath, body) => {
    requests.push({ path: requestPath, body });
    const sequence = requests.length;
    return {
      id: `session-${sequence}`,
      process_id: `process-${sequence}`,
      cwd: "/repo",
      profile: body.profile,
      provider: "codex",
    };
  });

  assert.deepEqual([...browser.runtime.tabIds()], []);
  browser.runtime.draw();
  assert.match(browser.html(), /data-testid="toolbar-add"/);
  assert.match(browser.html(), /Agent in Worktree/);
  assert.match(browser.html(), /Planing Agent/);

  const first = await browser.runtime.create("agent");
  const second = await browser.runtime.create("agent");
  assert.notEqual(first, second);
  assert.equal(browser.runtime.tab(first).label, "Agent");
  assert.equal(browser.runtime.tab(second).label, "Agent 2");
  assert.deepEqual(
    requests.map((request) => request.body.profile),
    ["agent", "agent"],
  );
});

test("Toolbar add button precedes the tab strip and exposes the exact lazy menu", async () => {
  const browser = browserRuntime();
  browser.runtime.setApi(async (_method, requestPath, body) => {
    if (requestPath !== "/api/terminal/session") {
      return { entries: [], entries_by_path: {} };
    }
    return {
      id: `session-${body.profile}`,
      process_id: `process-${body.profile}`,
      cwd: "/repo",
      profile: body.profile,
      provider: body.profile === "terminal" ? null : "codex",
    };
  });
  browser.runtime.draw();
  const initial = browser.html();
  assert.ok(initial.indexOf("toolbar-dock-label") < initial.indexOf("toolbar-add-menu"));
  assert.ok(initial.indexOf("toolbar-add-menu") < initial.indexOf("toolbar-tabs"));
  assert.deepEqual(
    [...initial.matchAll(/data-add-toolbar-tab="[^"]+">([^<]+)<\/button>/g)].map((match) => match[1]),
    ["Agent", "Agent in Worktree", "System", "Files", "Terminal", "Planing Agent"],
  );

  for (const mode of ["agent", "standalone", "system", "files", "terminal", "plan"]) {
    await browser.runtime.create(mode);
  }
  assert.deepEqual(
    [...browser.runtime.tabIds()].map((id) => browser.runtime.tab(id).mode),
    ["agent", "standalone", "system", "files", "terminal", "plan"],
  );
  assert.equal((browser.html().match(/data-testid="toolbar-tab-close"/g) || []).length, 6);
});

test("closing a worktree Agent confirms stop, preserves its worktree, and forgets the tab", async () => {
  const browser = browserRuntime();
  const requests = [];
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push([method, requestPath, body]);
    if (requestPath === "/api/terminal/session") {
      return {
        id: "worktree-session",
        process_id: "worktree-process",
        cwd: "/tmp/refine-worktree",
        profile: "standalone",
        provider: "codex",
        worktree: {
          branch: "refine/standalone/worktree-session",
          path: "/tmp/refine-worktree",
        },
      };
    }
    return { ok: true, termination: { confirmed_exit: true } };
  });

  const tabId = await browser.runtime.create("standalone");
  const worktree = { ...browser.runtime.tab(tabId).worktree };
  await browser.runtime.close(tabId);

  assert.deepEqual(worktree, {
    branch: "refine/standalone/worktree-session",
    path: "/tmp/refine-worktree",
  });
  assert.equal(browser.runtime.tab(tabId), undefined);
  assert.deepEqual(
    requests.filter((request) => request[1].endsWith("/stop")),
    [["POST", "/api/terminal/worktree-session/stop", undefined]],
  );
  assert.equal(requests.some((request) => /delete|discard|remove.*worktree/i.test(request[1])), false);
});

test("a failed close keeps the process-backed tab and shows the actionable error", async () => {
  const browser = browserRuntime();
  browser.runtime.setApi(async (_method, requestPath, body) => {
    if (requestPath === "/api/terminal/session") {
      return {
        id: "agent-session",
        process_id: "agent-process",
        cwd: "/repo",
        profile: body.profile,
        provider: "codex",
      };
    }
    if (requestPath.endsWith("/stop")) throw new Error("termination was not confirmed");
    return { ok: true };
  });

  const tabId = await browser.runtime.create("agent");
  await browser.runtime.close(tabId);

  assert.ok(browser.runtime.tab(tabId));
  assert.match(browser.html(), /termination was not confirmed/);
});

test("an unconfirmed backend stop result cannot remove the tab", async () => {
  const browser = browserRuntime();
  browser.runtime.setApi(async (_method, requestPath, body) => {
    if (requestPath === "/api/terminal/session") {
      return {
        id: "agent-session",
        process_id: "agent-process",
        cwd: "/repo",
        profile: body.profile,
        provider: "codex",
      };
    }
    return { ok: false, termination: { confirmed_exit: false } };
  });

  const tabId = await browser.runtime.create("agent");
  await browser.runtime.close(tabId);

  assert.ok(browser.runtime.tab(tabId));
  assert.match(browser.html(), /Process termination was not confirmed/);
});

test("Agent, Plan, Goal, and Standalone render the shared terminal surface", async () => {
  const browser = browserRuntime();
  await browser.runtime.openPlan("Design a retry queue");
  await browser.runtime.openGoal("GOAL1");

  for (const tabId of ["agent", "plan", "GOAL1", "standalone", "terminal"]) {
    await browser.runtime.activate(tabId);
    assert.match(browser.html(), /data-testid="toolbar-terminal-panel"/);
    assert.match(browser.html(), /data-testid="terminal-start"/);
    assert.doesNotMatch(browser.html(), /id="chat-input"/);
    assert.doesNotMatch(browser.html(), /data-testid="toolbar-agent-panel"/);
  }
});

test("agent terminals follow at the bottom and preserve user scrollback until returned", async () => {
  const browser = browserRuntime();
  await browser.runtime.openPlan("Design a retry queue");
  await browser.runtime.openGoal("GOAL1");

  for (const tabId of ["agent", "plan", "GOAL1", "standalone"]) {
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
  for (const tabId of ["terminal", "agent", "standalone"]) {
    await browser.runtime.activate(tabId);
  }

  const starts = requests.filter((request) => request.path === "/api/terminal/session");
  assert.deepEqual(starts.map((request) => request.body.profile), [
    "plan", "goal", "terminal", "agent", "standalone",
  ]);
  assert.equal(starts.find((request) => request.body.profile === "goal").body.goal_id, "GOAL1");
  assert.equal(starts.find((request) => request.body.profile === "plan").body.initial_prompt, "Design a retry queue");
  assert.equal(browser.runtime.tab("standalone").worktree.path, "/tmp/worktree-5");
  assert.equal(browser.runtime.terminal("agent").processId, "interactive-4");
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
        id: `agent-${sequence}`,
        process_id: `interactive-agent-${sequence}`,
        cwd: "/repo",
        profile: "agent",
        provider: "claude",
      };
    }
    return { ok: true };
  });

  await browser.runtime.start("agent");
  assert.equal(browser.runtime.terminal("agent").connected, true);
  await browser.runtime.stop("agent");
  assert.equal(browser.runtime.terminal("agent").exited, true);
  assert.match(browser.html(), />Restart</);
  await browser.runtime.activate("agent");
  assert.equal(browser.runtime.terminal("agent").sessionId, "agent-2");
  assert.deepEqual(requests.map((request) => request[1]), [
    "/api/terminal/session",
    "/api/terminal/agent-1/stop",
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
  await browser.runtime.start("agent");
  const [terminalEvents, agentEvents] = browser.events();
  terminalEvents.emit("terminal_output", { seq: 1, data: "shell output" });
  agentEvents.emit("terminal_output", { seq: 1, data: "agent output" });
  agentEvents.emit("terminal_exit", { seq: 2, data: "exit 0" });

  assert.equal(browser.runtime.terminal("terminal").display, "shell output");
  assert.equal(browser.runtime.terminal("agent").display, "agent outputexit 0");
  assert.equal(browser.runtime.terminal("terminal").connected, true);
  assert.equal(browser.runtime.terminal("agent").exited, true);
});

test("Goal Agent opens on the latest transcript tail while earlier context loads in the background", async () => {
  const browser = browserRuntime();
  const requests = [];
  let resolveHistory;
  const history = new Promise((resolve) => { resolveHistory = resolve; });
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push({ method, path: requestPath, body });
    if (requestPath === "/api/terminal/session") {
      return {
        id: "goal-session",
        process_id: "goal-agent-process",
        cwd: "/repo/worktree",
        profile: "goal",
        provider: "codex",
        goal_id: body.goal_id,
        transcript_bytes: 120_000,
      };
    }
    if (requestPath.includes("snapshot=1")) return history;
    return { ok: true };
  });

  await browser.runtime.openGoal("GOAL1");

  assert.equal(
    browser.events()[0].url,
    "/api/terminal/goal-session/events?after=104000",
  );
  browser.events()[0].emit("terminal_output", {
    seq: 120_000,
    data: "latest Goal Agent text\n",
  });
  assert.equal(
    browser.runtime.terminal("GOAL1").display,
    "latest Goal Agent text\n",
  );

  resolveHistory({
    events: [{ seq: 104_000, event: "terminal_output", data: "earlier context\n" }],
  });
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.equal(
    browser.runtime.terminal("GOAL1").display,
    "earlier context\nlatest Goal Agent text\n",
  );
  assert.equal(
    requests.find((request) => request.path.includes("snapshot=1")).path,
    "/api/terminal/goal-session/events?snapshot=1&after=70000&before=104000",
  );
});

test("stored custom-chat ids are discarded while managed terminal ids reattach", async () => {
  const storage = new Map();
  storage.set("refine_chat_tabs", JSON.stringify({
    tabs: {
      agent: { label: "Agent", mode: "agent", sessionId: "legacy-chat" },
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
  assert.equal(browser.runtime.tab("agent").sessionId, null);
  assert.equal(browser.runtime.tab("terminal").sessionId, "managed-terminal");
  assert.equal(browser.runtime.terminal("terminal").connected, true);
});

test("refresh preserves and independently reattaches every explicitly opened Agent", async () => {
  const storage = new Map();
  const firstBrowser = browserRuntime(storage);
  let sequence = 0;
  firstBrowser.runtime.setApi(async (_method, requestPath, body) => {
    if (requestPath !== "/api/terminal/session") return { ok: true };
    sequence += 1;
    return {
      id: `agent-session-${sequence}`,
      process_id: `agent-process-${sequence}`,
      cwd: "/repo",
      profile: body.profile,
      provider: "codex",
    };
  });
  const firstId = await firstBrowser.runtime.create("agent");
  const secondId = await firstBrowser.runtime.create("agent");

  const restored = browserRuntime(storage);
  const statusRequests = [];
  restored.runtime.setApi(async (_method, requestPath) => {
    statusRequests.push(requestPath);
    const sessionId = requestPath.split("/").at(-2);
    const sequence = sessionId.endsWith("-1") ? "1" : "2";
    return {
      id: sessionId,
      process_id: `agent-process-${sequence}`,
      profile: "agent",
      provider: "codex",
      cwd: "/repo",
      worktree: null,
      alive: true,
      exited: false,
    };
  });
  restored.runtime.restore();
  restored.runtime.draw();
  await new Promise((resolve) => setTimeout(resolve, 0));
  await restored.runtime.activate(firstId);
  await restored.runtime.activate(secondId);

  assert.deepEqual([...restored.runtime.tabIds()], [firstId, secondId]);
  assert.notEqual(restored.runtime.tab(firstId).sessionId, restored.runtime.tab(secondId).sessionId);
  assert.deepEqual(statusRequests.sort(), [
    "/api/terminal/agent-session-1/status",
    "/api/terminal/agent-session-2/status",
  ]);
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
  await browser.runtime.start("agent");
  browser.runtime.reset();
  await new Promise((resolve) => setTimeout(resolve, 0));

  assert.deepEqual(
    requests.filter((request) => request[1].endsWith("/stop")).map((request) => request[1]).sort(),
    ["/api/terminal/session-1/stop", "/api/terminal/session-2/stop"],
  );
});

test("closing a Goal Agent tab stops it through the workflow-aware backend path", async () => {
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
    [["POST", "/api/terminal/goal-session/stop", undefined]],
  );
});
