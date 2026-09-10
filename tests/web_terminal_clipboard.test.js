const assert = require("node:assert/strict");
const test = require("node:test");
const { clipboardRuntime, inputRequests, settleInput } = require("./support/terminal_clipboard_runtime");

test("every terminal profile installs the shared copy and paste behavior", async () => {
  const browser = clipboardRuntime();
  const profiles = [
    ["terminal", "terminal", "Terminal"],
    ["agent-one", "agent", "Agent"],
    ["worktree-one", "standalone", "Agent in Worktree"],
    ["goal-one", "goal", "Goal Agent"],
    ["plan-one", "plan", "Planing Agent"],
    ["skill-one", "skill", "Custom Skill"],
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
    profiles.map(([tabId, mode]) => [
      `/api/terminal/session-${tabId}/input`,
      `\x1b[200~pasted-${mode}\x1b[201~`,
    ]),
  );
});

test("Ctrl+Enter inserts a TUI newline without submitting ordinary Enter", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent-a", "agent", "Agent");

  const result = browser.runtime.key("agent-a", {
    key: "Enter",
    code: "Enter",
    ctrlKey: true,
  });
  assert.deepEqual({ ...result }, {
    acceptedByTerminal: false,
    defaultPrevented: true,
  });
  await settleInput();

  assert.deepEqual(
    inputRequests(browser).map(({ path: requestPath, body }) => [requestPath, body.data]),
    [["/api/terminal/session-agent-a/input", "\n"]],
  );
});

test("Ctrl+Z cannot suspend Agent terminals and subsequent input remains usable", async () => {
  const browser = clipboardRuntime();
  const profiles = [
    ["agent", "agent", "Agent"],
    ["worktree", "standalone", "Agent in Worktree"],
    ["goal", "goal", "Goal Agent"],
    ["plan", "plan", "Planing Agent"],
    ["skill", "skill", "Custom Skill"],
  ];

  for (const [tabId, mode, label] of profiles) {
    browser.runtime.add(tabId, mode, label);
    const suspended = browser.runtime.key(tabId, { key: "z", ctrlKey: true });
    assert.deepEqual({ ...suspended }, {
      acceptedByTerminal: false,
      defaultPrevented: true,
    });
    const continued = browser.runtime.key(tabId, { key: "x" });
    assert.deepEqual({ ...continued }, {
      acceptedByTerminal: true,
      defaultPrevented: false,
    });
  }
  await settleInput();

  assert.deepEqual(
    inputRequests(browser).map(({ path: requestPath, body }) => [requestPath, body.data]),
    profiles.map(([tabId]) => [`/api/terminal/session-${tabId}/input`, "x"]),
  );
  assert.equal(
    inputRequests(browser).some(({ body }) => body.data.includes("\x1a")),
    false,
  );
});

test("Ctrl+Z retains shell Terminal job-control semantics", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("shell", "terminal", "Terminal");

  const result = browser.runtime.key("shell", { key: "z", ctrlKey: true });
  assert.deepEqual({ ...result }, {
    acceptedByTerminal: true,
    defaultPrevented: false,
  });
  await settleInput();

  assert.deepEqual(
    inputRequests(browser).map(({ body }) => body.data),
    ["\x1a"],
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

test("Ctrl+V uses terminal-native framing for single-line and multiline text", async () => {
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
  assert.deepEqual(Array.from(browser.runtime.pastedText("agent-a")), values);
  assert.deepEqual(inputs.map((request) => request.body.data), [
    "\x1b[200~single line\x1b[201~",
    "\x1b[200~first\rsecond\rthird\tend\x1b[201~",
  ]);
  assert.equal(inputs.some((request) => request.body.data.includes("\x16")), false);
});

test("native paste is sent exactly once instead of being reprocessed by xterm", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("worktree", "standalone", "Agent in Worktree");
  browser.runtime.select("worktree", "selected\r\ntext");

  const copy = browser.runtime.copyEvent("worktree");
  assert.equal(copy.defaultPrevented, true);
  assert.equal(copy.copied["text/plain"], "selected\r\ntext");
  const paste = browser.runtime.pasteEvent("worktree", "one\r\ntwo\n");
  assert.equal(paste.defaultPrevented, true);
  assert.equal(paste.propagationStopped, true);
  await settleInput();

  assert.deepEqual(Array.from(browser.runtime.pastedText("worktree")), ["one\r\ntwo\n"]);
  assert.deepEqual(inputRequests(browser).map((request) => request.body.data), [
    "\x1b[200~one\rtwo\r\x1b[201~",
  ]);
});

test("copy failures retain text for recovery while paste failures stay visible", async () => {
  const unavailable = clipboardRuntime();
  unavailable.runtime.add("shell", "terminal", "Terminal");
  unavailable.runtime.select("shell", "selection");
  unavailable.unavailable("copy");
  const copy = unavailable.runtime.key("shell", { key: "c", ctrlKey: true });
  assert.equal(copy.acceptedByTerminal, false);
  assert.equal(copy.defaultPrevented, true);
  assert.equal(unavailable.runtime.copied("shell").recovery, true);
  assert.equal(unavailable.runtime.copied("shell").text, "selection");
  assert.match(unavailable.runtime.copied("shell").message, /Automatic copy was blocked/);

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
  assert.equal(denied.runtime.copied("terminal").recovery, true);
  assert.equal(denied.runtime.copied("terminal").text, "selection");

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


test("copy shortcuts and native Copy work after exit and macOS forces selection", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent", "agent", "Agent");
  assert.equal(browser.runtime.options("agent").macOptionClickForcesSelection, true);
  browser.runtime.select("agent", "retained output");
  browser.runtime.exited("agent");
  for (const modifiers of [{ ctrlKey: true }, { ctrlKey: true, shiftKey: true }, { metaKey: true }]) {
    const result = browser.runtime.key("agent", { key: "c", ...modifiers });
    assert.equal(result.acceptedByTerminal, false);
    assert.equal(result.defaultPrevented, true);
    await settleInput();
    assert.equal(browser.runtime.copied("agent").message, "Selection copied.");
  }
  const native = browser.runtime.copyEvent("agent");
  assert.equal(native.copied["text/plain"], "retained output");
  assert.deepEqual(browser.writes, Array(3).fill("retained output"));
  assert.deepEqual(inputRequests(browser), []);
});

test("an active terminal selection cannot capture shortcuts outside its renderer", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent", "agent", "Agent");
  browser.runtime.select("agent", "terminal selection");
  assert.equal(browser.runtime.outsideKey("agent", { key: "c", ctrlKey: true }), false);
  assert.equal(browser.runtime.outsideKey("agent", { key: "v", ctrlKey: true }), false);
  await settleInput();
  assert.deepEqual(browser.writes, []);
  assert.deepEqual(inputRequests(browser), []);
});

test("late and superseded clipboard results cannot replace current copy feedback", async () => {
  const browser = clipboardRuntime();
  browser.runtime.add("agent", "agent", "Agent");
  let rejectFirst;
  browser.setWrite(() => new Promise((_, reject) => { rejectFirst = reject; }));
  browser.runtime.select("agent", "first selection");
  browser.runtime.key("agent", { key: "c", ctrlKey: true });
  assert.equal(browser.runtime.copied("agent").message, "Copying selection…");
  browser.setWrite(async () => {});
  browser.runtime.select("agent", "second selection");
  browser.runtime.key("agent", { key: "c", ctrlKey: true });
  await settleInput();
  rejectFirst(new Error("denied"));
  await settleInput();
  assert.equal(browser.runtime.copied("agent").text, "second selection");
  assert.equal(browser.runtime.copied("agent").message, "Selection copied.");
});
