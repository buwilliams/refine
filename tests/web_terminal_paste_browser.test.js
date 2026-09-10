const assert = require("node:assert/strict");
const test = require("node:test");
const { SKIP, openTerminalApp, selectText } = require("./support/terminal-clipboard-browser");

test("paste gestures cannot send input after exit or loss of a managed session", { skip: SKIP }, async () => {
  const app = await openTerminalApp();
  try {
    for (const state of ["exited", "missing-session"]) {
      await app.addTab({ id: state });
      await app.output("retained output");
      await app.page.evaluate(async (state) => {
        await navigator.clipboard.writeText("stale clipboard");
        if (state === "exited") finishTerminalExit(currentToolbarTab(), terminalStateFor());
        else terminalStateFor().sessionId = "";
        terminalStateFor().term.focus();
        window.stoppedInput = [];
        window.clipboardEvents = [];
        terminalStateFor().term.onData((data) => stoppedInput.push(data));
        window.pasteReads = 0;
        navigator.clipboard.readText = async () => { pasteReads++; return "stale clipboard"; };
      }, state);
      for (const shortcut of ["Control+v", "Shift+Insert", "Control+Shift+v"]) {
        await app.page.keyboard.press(shortcut);
        await app.settle();
        assert.deepEqual(app.inputs, [], `${shortcut} must not send input in ${state}`);
      }
      const cdp = await app.context.newCDPSession(app.page);
      await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Unidentified", commands: ["paste"] });
      await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Unidentified" });
      await cdp.detach();
      await app.settle();
      assert.deepEqual(app.inputs, [], `native Paste must not send input in ${state}`);
      assert.deepEqual(await app.page.evaluate(() => stoppedInput), []);
      assert.deepEqual(await app.page.evaluate(() => clipboardEvents
        .filter((event) => event.type === "paste").map((event) => event.trusted)), [true]);
      assert.equal(await app.page.evaluate(() => pasteReads), 0);
      assert.equal(await selectText(app.page, { length: 15 }), "retained output");
      await app.page.keyboard.press("Control+c");
      assert.equal(await app.page.evaluate(() => terminalStateFor().clipboard.message), "Selection copied.");
    }
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("empty and unreadable native paste events cannot fall through to xterm", { skip: SKIP }, async () => {
  const app = await openTerminalApp();
  try {
    for (const failure of ["empty", "missing", "throws"]) {
      await app.addTab({ id: failure });
      const observed = await app.page.evaluate((failure) => {
        const terminal = terminalStateFor();
        let downstream = 0;
        terminal.term.textarea.addEventListener("paste", () => { downstream++; });
        const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
        if (failure !== "missing") Object.defineProperty(event, "clipboardData", { value: {
          getData() {
            if (failure === "throws") throw new Error("clipboard data denied");
            return "";
          },
        } });
        terminal.term.textarea.dispatchEvent(event);
        return { prevented: event.defaultPrevented, downstream, error: terminal.error };
      }, failure);
      assert.equal(observed.prevented, true);
      assert.equal(observed.downstream, 0);
      if (failure === "empty") assert.equal(observed.error, "");
      else {
        assert.match(observed.error, /Unable to paste.*browser Copy\/Paste/);
        assert.match(await app.page.locator('[data-testid="terminal-status"]').textContent(), /Unable to paste/);
      }
      await app.settle();
      assert.deepEqual(app.inputs, []);
    }
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});
