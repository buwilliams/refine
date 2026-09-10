const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

// Policy tests inspect hook decisions, never simulate xterm's key encoder.
// Input-route bytes and queue fencing are covered by the real-browser suite.
function keyboardPolicy() {
  const inputs = [];
  const focused = {};
  const term = { element: { contains: node => node === focused } };
  const terminal = { tabId: "agent", sessionId: "session-agent", term, exited: false };
  const tab = { mode: "agent" };
  const context = vm.createContext({
    document: { activeElement: focused },
    chatState: { tabs: { agent: tab }, activeTabId: "agent", open: true },
    terminalStates: new Map([["agent", terminal]]),
    terminalStateFor: () => terminal,
    currentToolbarTab: () => tab,
    toolbarTabUsesTerminal: () => true,
    queueTerminalInput: (data, target, sessionId) => inputs.push({ data, target, sessionId }),
  });
  for (const name of ["terminal-clipboard.js", "terminal-keyboard.js"]) {
    vm.runInContext(fs.readFileSync(path.join(__dirname,
      "../src/surfaces/web/static/js/features", name), "utf8"), context);
  }
  return { context, terminal, tab, term, inputs, key(init = {}) {
    const event = { type: "keydown", key: "ArrowUp", altKey: true, ctrlKey: false,
      shiftKey: false, metaKey: false, isComposing: false, repeat: false,
      defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...init };
    const accepted = context.handleTerminalCustomKeyEvent(event, terminal, tab, term);
    return { accepted, prevented: event.defaultPrevented };
  } };
}

test("the custom-key policy consumes Alt vertical keydowns and deliberate repeats exactly once", () => {
  const policy = keyboardPolicy();
  for (const [key, data] of [["ArrowUp", "\x1b[1;3A"], ["ArrowDown", "\x1b[1;3B"]]) {
    for (const repeat of [false, true, true]) {
      assert.deepEqual(policy.key({ key, repeat }), { accepted: false, prevented: true });
      assert.deepEqual(policy.inputs.splice(0), [{ data, target: policy.terminal, sessionId: "session-agent" }]);
    }
  }
});

test("the navigation override ignores other event phases, composition and modifier combinations", () => {
  const policy = keyboardPolicy();
  for (const key of ["ArrowUp", "ArrowDown"]) {
    for (const init of [{ type: "keyup" }, { type: "keypress" }, { type: undefined },
      { isComposing: true }, { ctrlKey: true }, { shiftKey: true }, { metaKey: true },
      { altKey: false }]) {
      assert.deepEqual(policy.key({ key, ...init }), { accepted: true, prevented: false }, JSON.stringify(init));
    }
  }
  for (const key of ["ArrowLeft", "ArrowRight", "Home", "End", "a"]) {
    assert.deepEqual(policy.key({ key }), { accepted: true, prevented: false });
  }
  assert.deepEqual(policy.inputs, []);
});

test("the navigation override requires focus, visibility and current terminal state", () => {
  for (const change of ["focus", "tab", "closed", "removed", "state"]) {
    const policy = keyboardPolicy();
    const { context } = policy;
    if (change === "focus") context.document.activeElement = {};
    if (change === "tab") context.chatState.activeTabId = "other";
    if (change === "closed") context.chatState.open = false;
    if (change === "removed") delete context.chatState.tabs.agent;
    if (change === "state") context.terminalStates.set("agent", {});
    assert.deepEqual(policy.key(), { accepted: true, prevented: false }, change);
    assert.deepEqual(policy.inputs, []);
  }
});

test("a replaced renderer cannot invoke keyboard policy on the current session", () => {
  const policy = keyboardPolicy();
  policy.terminal.term = {};
  assert.deepEqual(policy.key(), { accepted: false, prevented: false });
  assert.deepEqual(policy.inputs, []);
});

test("retained output consumes Alt navigation without emitting after exit or session loss", () => {
  for (const change of ["exit", "session"]) {
    const policy = keyboardPolicy();
    if (change === "exit") policy.terminal.exited = true;
    else policy.terminal.sessionId = "";
    for (const key of ["ArrowUp", "ArrowDown"]) {
      assert.deepEqual(policy.key({ key }), { accepted: false, prevented: true });
    }
    assert.deepEqual(policy.inputs, []);
  }
});
