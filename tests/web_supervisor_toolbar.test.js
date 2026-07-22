const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

class FakeClassList {
  constructor() {
    this.values = new Set();
  }

  add(...names) {
    names.forEach((name) => this.values.add(name));
  }

  remove(...names) {
    names.forEach((name) => this.values.delete(name));
  }

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
    this.disabled = false;
    this.hidden = false;
    this._innerHTML = "";
    this.innerHTMLWrites = 0;
    this.textContent = "";
    this.value = "";
    this.style = {};
    this.scrollTop = 0;
  }

  get innerHTML() { return this._innerHTML; }
  set innerHTML(value) {
    this._innerHTML = String(value);
    this.innerHTMLWrites += 1;
  }

  addEventListener() {}
  appendChild() {}
  focus() {}
  insertBefore() {}
  querySelector() { return null; }
  querySelectorAll() { return []; }
  remove() {}
  setAttribute(name, value) { this[name] = String(value); }
}

class FakeEventSource {
  static latest = null;

  constructor(url) {
    this.url = url;
    this.listeners = new Map();
    FakeEventSource.latest = this;
  }

  addEventListener(name, listener) {
    this.listeners.set(name, listener);
  }

  close() {}

  emit(name, payload) {
    this.listeners.get(name)?.({ data: JSON.stringify(payload) });
  }
}

function browserRuntime(storage = new Map()) {
  const toolbar = new FakeElement();
  const toggleButton = new FakeElement();
  const supervisorPanel = new FakeElement();
  const toasts = [];
  const busyLabels = [];
  const body = {
    appendChild(element) {
      toasts.push(element);
    },
  };
  const document = {
    body,
    documentElement: { style: { setProperty() {} } },
    addEventListener() {},
    createElement() { return new FakeElement(); },
    getElementById() { return null; },
    querySelector(selector) {
      if (selector === "#toolbar-dock") return toolbar;
      if (selector === "#btn-chat-toggle") return toggleButton;
      return null;
    },
    querySelectorAll() { return []; },
  };
  toolbar.querySelector = (selector) => {
    if (
      selector === '[data-testid="toolbar-supervisor-panel"]'
      && toolbar.innerHTML.includes('data-testid="toolbar-supervisor-panel"')
    ) return supervisorPanel;
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
    location: { hash: "#/dashboard", pathname: "/" },
    localStorage: {
      getItem(key) { return storage.get(key) ?? null; },
      setItem(key, value) { storage.set(key, String(value)); },
    },
    setInterval() { return 1; },
    setTimeout,
    window: {
      addEventListener() {},
      CSS: { escape: (value) => String(value) },
      innerHeight: 800,
    },
    withButtonBusy: async (button, label, action) => {
      busyLabels.push(label);
      const previous = button?.textContent || "";
      if (button) {
        button.disabled = true;
        button.textContent = label;
      }
      try {
        return await action();
      } finally {
        if (button) {
          button.disabled = false;
          button.textContent = previous;
        }
      }
    },
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "common.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/toolbar.js"), "utf8"), context);
  vm.runInContext(`
    globalThis.supervisorToolbarTest = {
      activate() {
        ensureStandaloneTab();
        chatState.activeTabId = SUPERVISOR_TAB_ID;
        chatState.open = true;
      },
      activateTab(tabId) {
        chatState.activeTabId = tabId;
        chatState.open = true;
      },
      activateSystem() {
        ensureStandaloneTab();
        chatState.activeTabId = SYSTEM_TAB_ID;
        chatState.open = true;
      },
      draw: drawToolbar,
      emitChat: handleChatSseEvent,
      ensurePlan: ensurePlanTab,
      initSSE,
      load: loadSupervisorAgentState,
      openGoal(goalId) { openChatDock({ goalId }); },
      restore: loadChatStateFromStorage,
      save: saveChatStateToStorage,
      send: sendChatText,
      supervisorTab() { return chatState.tabs[SUPERVISOR_TAB_ID]; },
      tab(tabId) { return chatState.tabs[tabId]; },
      systemMessageCount() { return systemOperationState.messages.length; },
      tabIds() { return Object.keys(chatState.tabs); },
      toggleActivity: toggleChatProgress,
      setApi(nextApi) { api = nextApi; },
      setAttached(attached = true) { state.project = { attached }; },
      setRoute(route) { state.currentRoute = route; },
    };
  `, context);
  return {
    busyLabels,
    html: () => toolbar.innerHTML + supervisorPanel.innerHTML,
    runtime: context.supervisorToolbarTest,
    supervisorStatusHtml: () => supervisorPanel.innerHTML,
    toolbarRenderCount: () => toolbar.innerHTMLWrites,
    toasts,
  };
}

function snapshot(overrides = {}) {
  return {
    lifecycle: "idle",
    health: "healthy",
    active_work: 0,
    queued_work: 0,
    failed_work: 0,
    supervisor_process: "daemon-supervised workflow runner",
    updated_at: "2026-07-21T22:00:00Z",
    events: [],
    session_id: null,
    ...overrides,
  };
}

function queuedMessage(id, text) {
  return {
    id,
    text,
    created_at: "2026-07-21T22:00:00Z",
    updated_at: "2026-07-21T22:00:00Z",
  };
}

test("Supervisor stays discoverable and prompt-ready while idle without an event stream", () => {
  const browser = browserRuntime();
  browser.runtime.activate();
  browser.runtime.draw();

  assert.equal(browser.runtime.tabIds().filter((id) => id === "supervisor").length, 1);
  assert.match(browser.html(), /data-testid="toolbar-tab-supervisor"/);
  assert.match(browser.html(), /data-testid="toolbar-supervisor-panel"/);
  assert.match(browser.html(), /Supervisor is idle/);
  assert.match(browser.html(), /Type to queue a message/);
  assert.doesNotMatch(browser.html(), /data-testid="chat-toggle"/);
  assert.doesNotMatch(browser.html(), /data-testid="supervisor-agent-events"/);
  assert.doesNotMatch(browser.html(), /data-close-tab="supervisor"/);
});

test("every chat-capable toolbar tab defaults Activity to collapsed", () => {
  const browser = browserRuntime();
  browser.runtime.activate();
  browser.runtime.ensurePlan();
  browser.runtime.setApi(async () => ({ session_id: "goal-session" }));
  browser.runtime.openGoal("GOAL1");

  for (const tabId of ["standalone", "supervisor", "plan", "GOAL1"]) {
    assert.equal(browser.runtime.tab(tabId).showProgress, false, `${tabId} Activity default`);
  }

  Object.assign(browser.runtime.supervisorTab(), {
    sessionId: "supervisor-shared",
    progress: "Observed active Goal work",
  });
  browser.runtime.activateTab("supervisor");
  browser.runtime.draw();

  assert.match(browser.html(), /data-testid="chat-activity-toggle"[\s\S]*aria-expanded="false"/);
  assert.match(browser.html(), /data-testid="chat-progress-panel" hidden/);

  browser.runtime.toggleActivity();
  assert.match(browser.html(), /data-testid="chat-activity-toggle"[\s\S]*aria-expanded="true"/);
  assert.doesNotMatch(browser.html(), /data-testid="chat-progress-panel" hidden/);
});

test("restored toolbar state defaults Activity collapsed and preserves explicit expansion", () => {
  const storage = new Map([["refine_chat_tabs", JSON.stringify({
    tabs: {
      standalone: {
        goalId: null,
        label: "Standalone",
        mode: "standalone",
        sessionId: "standalone-session",
        output: "",
        progress: "Agent working",
      },
    },
    activeTabId: "standalone",
    open: true,
  })]]);
  const browser = browserRuntime(storage);

  browser.runtime.restore();
  browser.runtime.activateTab("standalone");
  browser.runtime.draw();

  assert.match(browser.html(), /data-testid="chat-activity-toggle"[\s\S]*aria-expanded="false"/);
  assert.match(browser.html(), /data-testid="chat-progress-panel" hidden/);

  browser.runtime.toggleActivity();
  const restoredExpanded = browserRuntime(storage);
  restoredExpanded.runtime.restore();
  restoredExpanded.runtime.activateTab("standalone");
  restoredExpanded.runtime.draw();

  assert.match(restoredExpanded.html(), /data-testid="chat-activity-toggle"[\s\S]*aria-expanded="true"/);
  assert.doesNotMatch(restoredExpanded.html(), /data-testid="chat-progress-panel" hidden/);
});

test("navigation reinitialization restores the singleton session and transcript", async () => {
  const storage = new Map();
  const first = browserRuntime(storage);
  first.runtime.activate();
  Object.assign(first.runtime.supervisorTab(), {
    sessionId: "supervisor-shared",
    output: "> investigate queue\nSupervisor: queue is healthy",
    progress: "Observed active Goal work",
  });
  first.runtime.save();

  const requests = [];
  const restored = browserRuntime(storage);
  restored.runtime.setAttached();
  restored.runtime.restore();
  restored.runtime.activate();
  restored.runtime.setApi(async (method, requestPath) => {
    requests.push([method, requestPath]);
    return { supervisor_agent: snapshot({ session_id: "supervisor-shared" }) };
  });
  await restored.runtime.load();
  restored.runtime.draw();

  assert.equal(restored.runtime.supervisorTab().sessionId, "supervisor-shared");
  assert.match(restored.html(), /investigate queue/);
  assert.match(restored.html(), /queue is healthy/);
  assert.match(restored.html(), /Observed active Goal work/);
  assert.doesNotMatch(restored.html(), /data-testid="chat-toggle"/);
  assert.deepEqual(requests, [["GET", "/api/supervisor-agent"]]);
  assert.equal(restored.runtime.tabIds().filter((id) => id === "supervisor").length, 1);
});

test("polling and SSE reconnect route deduplicated supervisor events to System", async () => {
  const browser = browserRuntime();
  browser.runtime.setAttached();
  browser.runtime.setRoute("dashboard");
  browser.runtime.activate();
  const delayedEvent = {
    id: "supervisor-delayed",
    kind: "observation",
    status: "running",
    message: "Worker heartbeat delayed",
    goal_id: "00000000000000000000000001",
    created_at: "2026-07-21T22:00:01Z",
  };
  const responses = [
    snapshot({
      lifecycle: "supervising",
      health: "degraded",
      active_work: 1,
      session_id: "supervisor-shared",
      events: [delayedEvent],
    }),
    snapshot({
      lifecycle: "idle",
      health: "healthy",
      session_id: "supervisor-shared",
      events: [delayedEvent, {
        id: "supervisor-recovered",
        kind: "recovery",
        status: "completed",
        message: "Worker heartbeat recovered",
        created_at: "2026-07-21T22:00:02Z",
      }],
    }),
  ];
  const requests = [];
  browser.runtime.setApi(async (method, requestPath) => {
    requests.push([method, requestPath]);
    return { supervisor_agent: responses.shift() };
  });

  await browser.runtime.load();
  browser.runtime.draw();
  assert.match(browser.html(), /supervisor-agent-health-degraded/);
  assert.match(browser.html(), /supervising/);
  assert.doesNotMatch(browser.html(), /Worker heartbeat delayed/);
  assert.equal(browser.runtime.systemMessageCount(), 1);

  browser.runtime.activateSystem();
  browser.runtime.draw();
  assert.match(browser.html(), /data-testid="toolbar-system-panel"/);
  assert.match(browser.html(), /Worker heartbeat delayed/);
  assert.match(browser.html(), /supervisor/);
  assert.match(browser.html(), /00000000000000000000000001/);

  browser.runtime.activate();
  browser.runtime.initSSE();
  assert.equal(FakeEventSource.latest.url, "/api/sse");
  FakeEventSource.latest.emit("chat_event", {
    session_id: "supervisor-shared",
    in_flight: false,
    closed: false,
    event: {
      id: "assistant-reconnect-1",
      role: "assistant",
      text: "Conversation refreshed after reconnect",
    },
  });
  browser.runtime.draw();
  assert.match(browser.html(), /Conversation refreshed after reconnect/);

  await browser.runtime.load();
  browser.runtime.activate();
  browser.runtime.draw();
  assert.match(browser.html(), /supervisor-agent-health-healthy/);
  assert.doesNotMatch(browser.html(), /Worker heartbeat recovered/);
  assert.equal(browser.runtime.systemMessageCount(), 2);
  browser.runtime.activateSystem();
  browser.runtime.draw();
  assert.match(browser.html(), /Worker heartbeat delayed/);
  assert.match(browser.html(), /Worker heartbeat recovered/);
  assert.equal(browser.runtime.supervisorTab().sessionId, "supervisor-shared");
  assert.equal(browser.runtime.tabIds().filter((id) => id === "supervisor").length, 1);
  assert.deepEqual(requests, [
    ["GET", "/api/supervisor-agent"],
    ["GET", "/api/supervisor-agent"],
  ]);
});

test("polling refreshes supervisor status without replacing the prompt and routes events to System", async () => {
  const browser = browserRuntime();
  browser.runtime.setAttached();
  browser.runtime.activate();
  browser.runtime.draw();
  const toolbarRenderCount = browser.toolbarRenderCount();
  browser.runtime.setApi(async () => ({
    supervisor_agent: snapshot({
      lifecycle: "supervising",
      active_work: 1,
      events: [{
        kind: "observation",
        status: "running",
        message: "Queue observation refreshed",
        created_at: "2026-07-21T22:00:01Z",
      }],
    }),
  }));

  await browser.runtime.load();

  assert.equal(browser.toolbarRenderCount(), toolbarRenderCount);
  assert.match(browser.supervisorStatusHtml(), /supervising/);
  assert.doesNotMatch(browser.supervisorStatusHtml(), /Queue observation refreshed/);
  assert.equal(browser.runtime.systemMessageCount(), 1);
  browser.runtime.activateSystem();
  browser.runtime.draw();
  assert.match(browser.html(), /Queue observation refreshed/);
});

test("polling an unchanged full supervisor event window does not duplicate System entries", async () => {
  const browser = browserRuntime();
  browser.runtime.setAttached();
  const eventStart = Date.parse("2026-07-21T22:00:00Z");
  const events = Array.from({ length: 80 }, (_, index) => ({
    id: `supervisor-event-${index}`,
    kind: index === 79 ? "recovery" : "observation",
    status: index === 79 ? "completed" : "running",
    message: `Supervisor event ${index}`,
    created_at: new Date(eventStart + index * 1000).toISOString(),
  }));
  browser.runtime.setApi(async () => ({ supervisor_agent: snapshot({ events }) }));

  await browser.runtime.load();
  assert.equal(browser.runtime.systemMessageCount(), 80);

  await browser.runtime.load();
  assert.equal(browser.runtime.systemMessageCount(), 80);
});

test("initial prompts and active-work follow-ups share chat APIs and render every outcome", async () => {
  const browser = browserRuntime();
  browser.runtime.setAttached();
  browser.runtime.activate();
  const requests = [];
  let inputCount = 0;
  const events = [
    ["queued", "queued", "Investigation queued"],
    ["decision", "running", "Investigation running"],
    ["recovery", "completed", "Bounded recovery completed"],
    ["failure", "failed", "Worker restart failed"],
    ["provider", "blocked", "Provider authentication required"],
  ].map(([kind, status, message], index) => ({
    kind,
    status,
    message,
    retryable: status === "failed" || status === "blocked",
    created_at: `2026-07-21T22:00:0${index}Z`,
  }));
  browser.runtime.setApi(async (method, requestPath, body) => {
    requests.push({ method, path: requestPath, body });
    if (requestPath === "/api/supervisor-agent/session") {
      return { session_id: "supervisor-shared" };
    }
    if (requestPath === "/api/supervisor-agent") {
      return {
        supervisor_agent: snapshot({
          lifecycle: "supervising",
          health: "degraded",
          active_work: 1,
          queued_work: 1,
          failed_work: 1,
          session_id: "supervisor-shared",
          events,
        }),
      };
    }
    if (requestPath === "/api/chat/supervisor-shared/input") {
      inputCount += 1;
      return {
        in_flight: true,
        queued_messages: inputCount === 1
          ? [queuedMessage("q1", "Investigate the delayed worker")]
          : [
              queuedMessage("q1", "Investigate the delayed worker"),
              queuedMessage("q2", "Check its latest heartbeat"),
            ],
      };
    }
    throw new Error(`unexpected request ${method} ${requestPath}`);
  });

  await browser.runtime.send("Investigate the delayed worker", browser.runtime.supervisorTab());
  await browser.runtime.send("Check its latest heartbeat", browser.runtime.supervisorTab());

  const inputs = requests.filter((request) => request.path.endsWith("/input"));
  assert.deepEqual(inputs.map((request) => request.path), [
    "/api/chat/supervisor-shared/input",
    "/api/chat/supervisor-shared/input",
  ]);
  assert.deepEqual(inputs.map((request) => request.body.text), [
    "Investigate the delayed worker",
    "Check its latest heartbeat",
  ]);
  assert.equal(
    requests.filter((request) => request.path === "/api/supervisor-agent/session").length,
    1,
  );
  assert.match(browser.html(), /Queued messages/);
  assert.match(browser.html(), /Agent working in session supervisor-shared/);
  assert.doesNotMatch(browser.html(), /Provider authentication required/);

  browser.runtime.activateSystem();
  browser.runtime.draw();
  for (const status of ["queued", "running", "completed", "failed", "blocked"]) {
    assert.match(browser.html(), new RegExp(`data-system-log-status="${status}"`));
  }
  assert.match(browser.html(), /Provider authentication required/);
  assert.match(browser.html(), /retryable/);

  browser.runtime.activate();
  browser.runtime.draw();

  browser.runtime.emitChat({
    session_id: "supervisor-shared",
    in_flight: false,
    closed: false,
    event: {
      id: "assistant-complete-1",
      role: "assistant",
      text: "Investigation completed without destructive recovery",
    },
  });
  browser.runtime.draw();
  assert.match(browser.html(), /Investigation completed without destructive recovery/);
  assert.match(browser.html(), /Session supervisor-shared active/);

  browser.runtime.emitChat({
    session_id: "supervisor-shared",
    in_flight: false,
    closed: true,
    event: {
      id: "provider-auth-1",
      role: "system",
      text: "Provider authentication blocked; sign in and retry",
    },
  });
  assert.match(browser.html(), /Session ended — Provider authentication blocked; sign in and retry/);
  assert.match(browser.html(), /Provider authentication blocked; sign in and retry/);
});

test("a pending refresh does not block the toolbar or require a Goal page", async () => {
  const browser = browserRuntime();
  browser.runtime.setAttached();
  browser.runtime.setRoute("dashboard");
  browser.runtime.activate();
  let resolveRefresh;
  browser.runtime.setApi(() => new Promise((resolve) => {
    resolveRefresh = resolve;
  }));

  let settled = false;
  const refresh = browser.runtime.load().then(() => { settled = true; });
  await Promise.resolve();
  assert.equal(settled, false);
  browser.runtime.draw();
  assert.match(browser.html(), /data-testid="toolbar-supervisor-panel"/);
  assert.match(browser.html(), /loading/);

  resolveRefresh({
    supervisor_agent: snapshot({
      lifecycle: "supervising",
      health: "healthy",
      active_work: 1,
    }),
  });
  await refresh;
  assert.match(browser.html(), /supervising/);
  assert.doesNotMatch(browser.html(), /Goal [A-Z0-9]/);
});

test("Supervisor never offers manual Start or Stop controls", () => {
  const browser = browserRuntime();
  browser.runtime.activate();
  browser.runtime.draw();

  assert.doesNotMatch(browser.html(), /data-testid="chat-toggle"/);
  assert.doesNotMatch(browser.html(), /Start supervisor conversation/);

  Object.assign(browser.runtime.supervisorTab(), {
    sessionId: "supervisor-shared",
    pending: true,
  });
  browser.runtime.draw();

  assert.match(browser.html(), /Agent working in session supervisor-shared/);
  assert.doesNotMatch(browser.html(), /data-testid="chat-toggle"/);
  assert.doesNotMatch(browser.html(), /Stop supervisor conversation/);
});
