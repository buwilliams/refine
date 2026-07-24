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
    this.children = [];
    this._innerHTML = "";
    this.clientWidth = 1000;
    this.clientHeight = 400;
  }
  get innerHTML() { return this._innerHTML; }
  set innerHTML(value) {
    this._innerHTML = String(value);
    this.children = [];
  }
  addEventListener(type, listener) { this.listeners.set(type, listener); }
  contains(child) { return this.children.includes(child); }
  focus() {}
  querySelector() { return null; }
  querySelectorAll() { return []; }
  replaceChildren(...children) { this.children = children; }
}

class FakeTerminal {
  constructor() {
    this.element = new FakeElement();
    this.selection = "";
    this.customKeyHandler = null;
    this.dataHandler = null;
  }
  attachCustomKeyEventHandler(handler) { this.customKeyHandler = handler; }
  dispose() {}
  focus() {}
  getSelection() { return this.selection; }
  hasSelection() { return this.selection.length > 0; }
  onData(handler) { this.dataHandler = handler; }
  open(output) { output.replaceChildren(this.element); }
  resize() {}
  write() {}
}

function clipboardRuntime() {
  const requests = [];
  const writes = [];
  let readText = async () => "";
  let writeText = async (text) => { writes.push(text); };
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
      if (selector === ".terminal-output" && toolbar.innerHTML.includes("terminal-output")) {
        return terminalOutput;
      }
      return null;
    },
    querySelectorAll() { return []; },
  };
  toolbar.querySelector = (selector) => {
    if (selector === ".terminal-output" && toolbar.innerHTML.includes("terminal-output")) {
      return terminalOutput;
    }
    return null;
  };
  const navigator = {
    clipboard: {
      readText: (...args) => readText(...args),
      writeText: (...args) => writeText(...args),
    },
  };
  const context = vm.createContext({
    AbortController,
    EventSource: class {
      addEventListener() {}
      close() {}
    },
    ResizeObserver: class {
      disconnect() {}
      observe() {}
    },
    URLSearchParams,
    clearInterval() {},
    clearTimeout,
    console,
    document,
    fetch: async () => ({ ok: true, json: async () => ({}) }),
    getComputedStyle: () => ({
      fontFamily: "monospace",
      fontSize: "15px",
      lineHeight: "20px",
      paddingBottom: "12px",
      paddingLeft: "16px",
      paddingRight: "16px",
      paddingTop: "12px",
    }),
    location: { hash: "#/dashboard", pathname: "/" },
    localStorage: {
      getItem() { return null; },
      setItem() {},
    },
    navigator,
    requestAnimationFrame(callback) { callback(); },
    sessionStorage: {
      getItem() { return null; },
      setItem() {},
    },
    setInterval() { return 1; },
    setTimeout,
    window: {
      addEventListener() {},
      CSS: { escape: (value) => String(value) },
      getComputedStyle: () => ({
        fontFamily: "monospace",
        fontSize: "15px",
        lineHeight: "20px",
        paddingBottom: "12px",
        paddingLeft: "16px",
        paddingRight: "16px",
        paddingTop: "12px",
      }),
      innerHeight: 800,
      Terminal: FakeTerminal,
    },
    withButtonBusy: async (_button, _label, action) => action(),
    __recordRequest(method, requestPath, body) {
      requests.push({ method, path: requestPath, body });
      return Promise.resolve({ ok: true });
    },
    __terminalOutput: terminalOutput,
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "common.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/toolbar.js"), "utf8"), context);
  vm.runInContext(`
    api = (method, requestPath, body) => globalThis.__recordRequest(method, requestPath, body);

    function clipboardTestEvent(init = {}) {
      return {
        altKey: false,
        ctrlKey: false,
        metaKey: false,
        shiftKey: false,
        type: "keydown",
        ...init,
        defaultPrevented: false,
        preventDefault() { this.defaultPrevented = true; },
      };
    }

    globalThis.terminalClipboardTest = {
      add(tabId, mode, label) {
        chatState.tabs[tabId] = normalizeInteractiveTerminalTab({
          goalId: mode === "goal" ? tabId : null,
          label,
          mode,
          sessionId: null,
        });
        chatState.activeTabId = tabId;
        chatState.open = true;
        const terminal = terminalStateFor(tabId);
        terminal.sessionId = "session-" + tabId;
        terminal.connected = false;
        terminal.exited = false;
        drawToolbar();
        ensureTerminalRenderer(globalThis.__terminalOutput, chatState.tabs[tabId]);
        terminal.connected = true;
        return terminal.term?.customKeyHandler != null
          && terminal.term?.element?.listeners?.has("copy")
          && terminal.term?.element?.listeners?.has("paste");
      },
      key(tabId, init) {
        chatState.activeTabId = tabId;
        const terminal = terminalStateFor(tabId);
        const event = clipboardTestEvent(init);
        const acceptedByTerminal = terminal.term.customKeyHandler(event);
        if (acceptedByTerminal) {
          const data = terminalKeyData(event);
          if (data != null) terminal.term.dataHandler(data);
        }
        return { acceptedByTerminal, defaultPrevented: event.defaultPrevented };
      },
      copyEvent(tabId) {
        chatState.activeTabId = tabId;
        const copied = {};
        const event = clipboardTestEvent({
          type: "copy",
          clipboardData: {
            setData(type, text) { copied[type] = text; },
          },
        });
        terminalStateFor(tabId).term.element.listeners.get("copy")(event);
        return { copied, defaultPrevented: event.defaultPrevented };
      },
      pasteEvent(tabId, text) {
        chatState.activeTabId = tabId;
        const event = clipboardTestEvent({
          type: "paste",
          clipboardData: {
            getData(type) { return type === "text/plain" ? text : ""; },
          },
        });
        terminalStateFor(tabId).term.element.listeners.get("paste")(event);
        return { defaultPrevented: event.defaultPrevented };
      },
      select(tabId, text) {
        terminalStateFor(tabId).term.selection = text;
      },
      error(tabId) { return terminalStateFor(tabId)?.error || ""; },
      rotateSession(tabId, sessionId) {
        terminalStateFor(tabId).sessionId = sessionId;
      },
      nonTerminalKey(init) {
        chatState.tabs.files = { label: "Files", mode: "files", sessionId: null };
        chatState.activeTabId = "files";
        const event = clipboardTestEvent(init);
        handleTerminalKeydown(event);
        return event.defaultPrevented;
      },
    };
  `, context);

  return {
    html: () => toolbar.innerHTML,
    requests,
    runtime: context.terminalClipboardTest,
    setRead(nextRead) {
      readText = nextRead;
      navigator.clipboard.readText = (...args) => readText(...args);
    },
    setWrite(nextWrite) {
      writeText = nextWrite;
      navigator.clipboard.writeText = (...args) => writeText(...args);
    },
    unavailable(action) {
      delete navigator.clipboard[action === "paste" ? "readText" : "writeText"];
    },
    writes,
  };
}

function inputRequests(browser) {
  return browser.requests.filter((request) => request.path.endsWith("/input"));
}

function settleInput() {
  return new Promise((resolve) => setTimeout(resolve, 25));
}

test("every terminal profile installs the shared copy and paste behavior", async () => {
  const browser = clipboardRuntime();
  const profiles = [
    ["terminal", "terminal", "Terminal"],
    ["agent-one", "agent", "Agent"],
    ["worktree-one", "standalone", "Agent in Worktree"],
    ["goal-one", "goal", "Goal Agent"],
    ["plan-one", "plan", "Planing Agent"],
  ];

  for (const [tabId, mode, label] of profiles) {
    assert.equal(browser.runtime.add(tabId, mode, label), true);
    browser.runtime.select(tabId, `selected-${mode}`);
    const copy = browser.runtime.key(tabId, { key: "c", ctrlKey: true });
    assert.deepEqual({ ...copy }, {
      acceptedByTerminal: false,
      defaultPrevented: true,
    });
    browser.setRead(async () => `pasted-${mode}`);
    const paste = browser.runtime.key(tabId, { key: "v", ctrlKey: true });
    assert.deepEqual({ ...paste }, {
      acceptedByTerminal: false,
      defaultPrevented: true,
    });
    await settleInput();
  }

  assert.deepEqual(browser.writes, profiles.map(([, mode]) => `selected-${mode}`));
  assert.deepEqual(
    inputRequests(browser).map(({ path: requestPath, body }) => [requestPath, body.data]),
    profiles.map(([tabId, mode]) => [`/api/terminal/session-${tabId}/input`, `pasted-${mode}`]),
  );
});

test("Ctrl+C without a selection keeps terminal SIGINT semantics", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("shell", "terminal", "Terminal");

  const result = browser.runtime.key("shell", { key: "c", ctrlKey: true });
  assert.deepEqual({ ...result }, {
    acceptedByTerminal: true,
    defaultPrevented: false,
  });
  await settleInput();

  assert.deepEqual(
    inputRequests(browser).map(({ path: requestPath, body }) => [requestPath, body.data]),
    [["/api/terminal/session-shell/input", "\x03"]],
  );
  assert.deepEqual(browser.writes, []);
});

test("Ctrl+V pastes single-line and multiline clipboard text exactly once without raw control-V", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent-a", "agent", "Agent");
  const values = ["single line", "first\r\nsecond\nthird\tend"];

  for (const value of values) {
    browser.setRead(async () => value);
    const result = browser.runtime.key("agent-a", { key: "v", ctrlKey: true });
    assert.equal(result.acceptedByTerminal, false);
    assert.equal(result.defaultPrevented, true);
    await settleInput();
  }

  const inputs = inputRequests(browser);
  assert.equal(inputs.length, 2);
  assert.deepEqual(inputs.map((request) => request.body.data), values);
  assert.equal(inputs.some((request) => request.body.data.includes("\x16")), false);
});

test("native copy and paste events preserve exact text for context-menu shortcuts", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("worktree", "standalone", "Agent in Worktree");
  browser.runtime.select("worktree", "selected\r\ntext");

  const copy = browser.runtime.copyEvent("worktree");
  assert.equal(copy.defaultPrevented, true);
  assert.equal(copy.copied["text/plain"], "selected\r\ntext");
  const paste = browser.runtime.pasteEvent("worktree", "one\r\ntwo\n");
  assert.equal(paste.defaultPrevented, true);
  await settleInput();

  assert.deepEqual(inputRequests(browser).map((request) => request.body.data), ["one\r\ntwo\n"]);
});

test("clipboard failures are visible and unavailable APIs do not claim browser handling", async () => {
  const unavailable = clipboardRuntime();
  unavailable.runtime.add("shell", "terminal", "Terminal");
  unavailable.runtime.select("shell", "selection");
  unavailable.unavailable("copy");
  const copy = unavailable.runtime.key("shell", { key: "c", ctrlKey: true });
  assert.equal(copy.acceptedByTerminal, false);
  assert.equal(copy.defaultPrevented, false);
  assert.match(unavailable.runtime.error("shell"), /clipboard write access is unavailable/i);
  assert.match(unavailable.html(), /clipboard write access is unavailable/i);

  unavailable.unavailable("paste");
  const paste = unavailable.runtime.key("shell", { key: "v", ctrlKey: true });
  assert.equal(paste.acceptedByTerminal, false);
  assert.equal(paste.defaultPrevented, false);
  assert.match(unavailable.runtime.error("shell"), /clipboard read access is unavailable/i);
  await settleInput();
  assert.deepEqual(inputRequests(unavailable), []);

  const denied = clipboardRuntime();
  denied.runtime.add("terminal", "terminal", "Terminal");
  denied.runtime.select("terminal", "selection");
  denied.setWrite(async () => { throw new Error("clipboard write permission denied"); });
  denied.runtime.key("terminal", { key: "c", metaKey: true });
  await settleInput();
  assert.match(denied.runtime.error("terminal"), /write permission denied/i);

  denied.setRead(async () => { throw new Error("clipboard read permission denied"); });
  denied.runtime.key("terminal", { key: "v", metaKey: true });
  await settleInput();
  assert.match(denied.runtime.error("terminal"), /read permission denied/i);
  assert.match(denied.html(), /read permission denied/i);
});

test("an asynchronous paste cannot cross into a replacement managed session", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent", "agent", "Agent");
  let resolveClipboard;
  browser.setRead(() => new Promise((resolve) => { resolveClipboard = resolve; }));

  browser.runtime.key("agent", { key: "v", ctrlKey: true });
  browser.runtime.rotateSession("agent", "replacement-session");
  resolveClipboard("must not cross sessions");
  await settleInput();

  assert.deepEqual(inputRequests(browser), []);
});

test("clipboard text buffered before replacement cannot cross managed sessions", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent", "agent", "Agent");
  let resolveClipboard;
  browser.setRead(() => new Promise((resolve) => { resolveClipboard = resolve; }));

  browser.runtime.key("agent", { key: "v", ctrlKey: true });
  resolveClipboard("buffered for the original session");
  await Promise.resolve();
  browser.runtime.rotateSession("agent", "replacement-session");
  await settleInput();

  assert.deepEqual(inputRequests(browser), []);
});

test("clipboard shortcuts remain untouched when focus is outside a terminal", async () => {
  const browser = clipboardRuntime();
  const prevented = browser.runtime.nonTerminalKey({ key: "v", ctrlKey: true });
  await settleInput();

  assert.equal(prevented, false);
  assert.deepEqual(inputRequests(browser), []);
});
