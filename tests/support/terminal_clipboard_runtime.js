const fs = require("node:fs");
const path = require("node:path");
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
  contains(child) { return this === child || this.children.includes(child); }
  focus() {}
  querySelector() { return null; }
  querySelectorAll() { return []; }
  replaceChildren(...children) { this.children = children; }
}

class FakeTerminal {
  constructor(options) {
    this.options = options;
    this.element = new FakeElement();
    this.selection = "";
    this.customKeyHandler = null;
    this.dataHandler = null;
    this.pasteCalls = [];
  }
  attachCustomKeyEventHandler(handler) { this.customKeyHandler = handler; }
  dispose() {}
  focus() {}
  getSelection() { return this.selection; }
  hasSelection() { return this.selection.length > 0; }
  onData(handler) { this.dataHandler = handler; }
  onSelectionChange(handler) { this.selectionHandler = handler; }
  open(output) { output.replaceChildren(this.element); }
  paste(text) {
    this.pasteCalls.push(text);
    const normalized = text.replace(/\r?\n/g, "\r");
    this.dataHandler(`\x1b[200~${normalized}\x1b[201~`);
  }
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
    // The real helpers live in dom-morph.js and need a browser DOM plus
    // Idiomorph. These stand in with the pre-morph semantics this fake DOM
    // models: replace the content, then run the bind step.
    renderInto(root, html, bind) {
      if (!root) return;
      root.innerHTML = html;
      if (typeof bind === "function") bind();
    },
    bindOnce(el, event, handler) {
      if (!el) return;
      el.addEventListener(event, handler);
    },
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
  const staticRoot = path.join(__dirname, "../../src/surfaces/web/static/js");
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "common.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/terminal-clipboard.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(staticRoot, "features/terminal-keyboard.js"), "utf8"), context);
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
        propagationStopped: false,
        preventDefault() { this.defaultPrevented = true; },
        stopPropagation() { this.propagationStopped = true; },
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
        document.activeElement = terminal.term.element;
        return terminal.term?.customKeyHandler != null
          && terminal.term?.element?.listeners?.has("copy")
          && terminal.term?.element?.listeners?.has("paste");
      },
      key(tabId, init) {
        chatState.activeTabId = tabId;
        const terminal = terminalStateFor(tabId);
        document.activeElement = terminal.term.element;
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
        document.activeElement = terminalStateFor(tabId).term.element;
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
        const terminal = terminalStateFor(tabId);
        document.activeElement = terminal.term.element;
        const event = clipboardTestEvent({
          type: "paste",
          clipboardData: {
            getData(type) { return type === "text/plain" ? text : ""; },
          },
        });
        terminal.term.element.listeners.get("paste")(event);
        // xterm owns a paste listener below the shared terminal element. Model
        // its onData fallback when the capture listener lets the event through.
        if (!event.propagationStopped) terminal.term.dataHandler(text);
        return {
          defaultPrevented: event.defaultPrevented,
          propagationStopped: event.propagationStopped,
        };
      },
      pastedText(tabId) {
        return [...terminalStateFor(tabId).term.pasteCalls];
      },
      select(tabId, text) {
        terminalStateFor(tabId).term.selection = text;
        terminalStateFor(tabId).term.selectionHandler?.();
      },
      copyControl(tabId) { return copyTerminalSelection(terminalStateFor(tabId)); },
      pointer(tabId) {
        terminalStateFor(tabId).term.element.listeners.get("pointerdown")({ button: 0 });
      },
      selected(tabId) { return terminalSelection(terminalStateFor(tabId)); },
      copied(tabId) { return terminalStateFor(tabId).clipboard; },
      exited(tabId) { terminalStateFor(tabId).exited = true; },
      options(tabId) { return terminalStateFor(tabId).term.options; },
      outsideKey(tabId, init) {
        document.activeElement = document.body;
        const event = clipboardTestEvent(init);
        handleTerminalClipboardKeydown(event, terminalStateFor(tabId));
        return event.defaultPrevented;
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

module.exports = { clipboardRuntime, inputRequests, settleInput };
