const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

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

function browserRuntime() {
  const storage = new Map();
  const context = vm.createContext({
    AbortController,
    EventSource: FakeEventSource,
    URLSearchParams,
    clearTimeout,
    console,
    document: {
      addEventListener() {},
      querySelector() { return null; },
      querySelectorAll() { return []; },
    },
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    location: { hash: "#/", pathname: "/" },
    localStorage: {
      getItem(key) { return storage.get(key) ?? null; },
      setItem(key, value) { storage.set(key, String(value)); },
    },
    setTimeout,
    window: {
      addEventListener() {},
      innerHeight: 800,
    },
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "common.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/toolbar.js"), "utf8"), context);
  vm.runInContext(`
    globalThis.goalLogTest = {
      chatState,
      goalLogTabId,
      initSSE,
      loadGoalLogTail,
      setApi(nextApi) { api = nextApi; },
    };
  `, context);
  return context.goalLogTest;
}

function goalLog(index) {
  return {
    id: `round-log:GOAL1:0:${String(index).padStart(3, "0")}`,
    datetime: `2026-07-21T12:${String(Math.floor(index / 60)).padStart(2, "0")}:${String(index % 60).padStart(2, "0")}Z`,
    severity: "info",
    category: "agent",
    message: `Goal log ${String(index).padStart(3, "0")}`,
    goal_id: "GOAL1",
    actor: "codex",
  };
}

function installGoalLogTab(runtime) {
  const tab = {
    goalId: "GOAL1",
    mode: "goal_logs",
    logEntries: [],
    logsLoaded: false,
    logsLoading: false,
    logsError: "",
  };
  runtime.chatState.tabs[runtime.goalLogTabId("GOAL1")] = tab;
  runtime.chatState.activeTabId = "standalone";
  runtime.chatState.open = false;
  return tab;
}

test("the initial Goal log request fetches the newest page and displays it chronologically", async () => {
  const runtime = browserRuntime();
  const tab = installGoalLogTab(runtime);
  let requestedPath = "";
  runtime.setApi(async (_method, requestPath) => {
    requestedPath = requestPath;
    return { activity: Array.from({ length: 200 }, (_, offset) => goalLog(249 - offset)) };
  });

  await runtime.loadGoalLogTail(tab, { redraw: false });

  assert.match(requestedPath, /(?:\?|&)limit=200(?:&|$)/);
  assert.match(requestedPath, /(?:\?|&)offset=0(?:&|$)/);
  assert.match(requestedPath, /(?:\?|&)dir=desc(?:&|$)/);
  assert.equal(tab.logEntries.length, 200);
  assert.equal(tab.logEntries[0].message, "Goal log 050");
  assert.equal(tab.logEntries[199].message, "Goal log 249");
});

test("the first Goal SSE log reaches the toolbar and replayed entries stay deduplicated", () => {
  const runtime = browserRuntime();
  const tab = installGoalLogTab(runtime);
  runtime.initSSE();
  assert.equal(FakeEventSource.latest.url, "/api/sse");

  FakeEventSource.latest.emit("goal_log_added", goalLog(1));
  assert.deepEqual(Array.from(tab.logEntries, (entry) => entry.message), ["Goal log 001"]);

  FakeEventSource.latest.emit("goal_log_added", goalLog(1));
  FakeEventSource.latest.emit("goal_log_added", goalLog(2));
  assert.deepEqual(
    Array.from(tab.logEntries, (entry) => entry.message),
    ["Goal log 001", "Goal log 002"],
  );
});
