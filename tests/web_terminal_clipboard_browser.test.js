const assert = require("node:assert/strict");
const test = require("node:test");
const { openTerminalApp, selectOutput, SKIP } = require("./support/terminal_browser");

for (const profile of ["agent", "skill"]) {
for (const platform of ["Linux x86_64", "Win32", "MacIntel"]) {
  const mac = platform === "MacIntel";
  test(`${profile} ${platform}: ${mac ? "Option" : "Shift"}-drag selects real xterm output under application mouse capture`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ platform, profile });
    try {
      assert.equal(await app.page.locator('[data-testid="terminal-selection-hint"]').innerText(),
        `${mac ? "Option" : "Shift"}-drag to select text`);
      await app.page.evaluate(() => new Promise((resolve) => {
        terminalStateFor().term.write("\x1b[?1000h\x1b[?1006h", resolve);
      }));
      const cell = await app.page.evaluate(() => {
        const term = terminalStateFor().term;
        const rect = term.element.querySelector(".xterm-screen").getBoundingClientRect();
        const size = term._core._renderService.dimensions.css.cell;
        return { x: rect.x, y: rect.y, width: size.width, height: size.height };
      });
      const drag = async () => {
        await app.page.mouse.move(cell.x + cell.width * 0.2, cell.y + cell.height * 0.5);
        await app.page.mouse.down();
        await app.page.mouse.move(cell.x + cell.width * 6.2, cell.y + cell.height * 0.5, { steps: 8 });
        await app.page.mouse.up();
      };
      await drag();
      assert.equal(await app.page.evaluate(() => terminalSelection(terminalStateFor())), "");
      await app.page.waitForTimeout(30);
      assert.ok(app.requests.join("").includes("\x1b[<"), "ordinary mouse input reaches the application");
      app.requests.length = 0;
      await app.page.keyboard.down(mac ? "Alt" : "Shift");
      await drag();
      await app.page.keyboard.up(mac ? "Alt" : "Shift");
      await app.page.waitForFunction(() => terminalSelection(terminalStateFor()) === "OUTPUT");
      await app.page.getByRole("button", { name: "Copy selection", exact: true }).click();
      assert.deepEqual(await app.page.evaluate(() => copyWrites), ["OUTPUT"]);
      assert.deepEqual(app.requests, []);
      assert.deepEqual(app.pageErrors, []);
    } finally {
      await app.close();
    }
  });
}

test(`${profile}: Copy control and focused shortcuts preserve selection and copy retained output`, { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile });
  try {
    const button = app.page.getByRole("button", { name: "Copy selection", exact: true });
    assert.equal(await button.isDisabled(), true);
    await selectOutput(app.page);
    await button.focus();
    await app.page.keyboard.press("Enter");
    await app.page.waitForFunction(() => copyWrites.length === 1);
    await app.page.evaluate(() => {
      const terminal = terminalStateFor();
      finishTerminalExit(currentToolbarTab(), terminal);
      drawToolbar();
      terminal.term.focus();
    });
    for (const shortcut of ["Control+c", "Control+Shift+c"]) {
      await app.page.keyboard.press(shortcut);
      await app.page.waitForFunction(() => terminalStateFor().clipboard.message === "Selection copied.");
    }
    // Native Ctrl+C writes through its Copy event; only the button and the
    // terminal-specific shortcut use the mocked asynchronous write API.
    await app.page.waitForFunction(() => copyWrites.length === 2);
    const native = await app.page.evaluate(() => {
      const data = new DataTransfer();
      const event = new ClipboardEvent("copy", { clipboardData: data, bubbles: true, cancelable: true });
      terminalStateFor().term.textarea.dispatchEvent(event);
      return { text: data.getData("text/plain"), prevented: event.defaultPrevented };
    });
    assert.deepEqual(native, { text: "OUTPUT-agent-a", prevented: true });
    assert.deepEqual(await app.page.evaluate(() => ({
      same: terminalStateFor().term === originalTerm,
      text: terminalSelection(terminalStateFor()), tab: chatState.activeTabId,
    })), { same: true, text: "OUTPUT-agent-a", tab: app.first });
    await app.page.evaluate(async () => {
      await activateToolbarTab(clipboardTabs.b);
      await activateToolbarTab(clipboardTabs.a);
    });
    assert.equal(await app.page.evaluate(() => terminalStateFor().term === originalTerm), true);
    await app.page.evaluate(() => { terminalStateFor().sessionId = ""; });
    await button.click();
    await app.page.waitForFunction(() => copyWrites.length === 3);
    assert.deepEqual(app.requests, []);
    await app.page.evaluate(() => terminalStateFor().term.clearSelection());
    await app.page.waitForFunction(() => document.querySelector("[data-terminal-copy]").disabled);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test(`${profile}: real xterm preserves interruption and exactly-once bracketed paste`, { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile });
  try {
    await app.page.evaluate(() => {
      terminalStateFor().term.focus();
      return new Promise((resolve) => terminalStateFor().term.write("\x1b[?2004h", resolve));
    });
    await app.page.keyboard.press("Control+c");
    await app.page.waitForTimeout(30);
    assert.deepEqual(app.requests, ["\x03"]);
    app.requests.length = 0;
    await app.page.evaluate(() => {
      const data = new DataTransfer();
      data.setData("text/plain", "one\r\ntwo\n");
      terminalStateFor().term.textarea.dispatchEvent(new ClipboardEvent("paste", {
        clipboardData: data, bubbles: true, cancelable: true,
      }));
    });
    await app.page.waitForTimeout(30);
    assert.deepEqual(app.requests, ["\x1b[200~one\rtwo\r\x1b[201~"]);
    await selectOutput(app.page);
    const outside = await app.page.evaluate(() => {
      const field = document.createElement("textarea");
      document.body.appendChild(field);
      field.value = "outside selection";
      field.focus();
      field.select();
      const event = new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true });
      field.dispatchEvent(event);
      const prevented = event.defaultPrevented;
      field.remove();
      return prevented;
    });
    assert.equal(outside, false);
    assert.deepEqual(await app.page.evaluate(() => copyWrites), []);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

}

const { SKIP: CLIPBOARD_SKIP, openTerminalApp: openClipboardApp, selectText } = require("./support/terminal-clipboard-browser");

for (const access of ["allowed", "denied", "unavailable"]) {
  test(`native keyboard clipboard works with async access ${access}`, { skip: CLIPBOARD_SKIP }, async () => {
    const app = await openClipboardApp();
    try {
      await app.addTab();
      await app.output("clipboard selection");
      assert.equal(await selectText(app.page, { length: 19 }), "clipboard selection");
      await app.page.evaluate((access) => {
        window.originalClipboard = navigator.clipboard;
        window.asyncCalls = [];
        window.copyWrites = [];
        const setData = DataTransfer.prototype.setData;
        DataTransfer.prototype.setData = function(type, text) {
          window.copyWrites.push([type, text]);
          return setData.call(this, type, text);
        };
        if (access === "unavailable") Object.defineProperty(navigator, "clipboard", { value: undefined });
        else Object.defineProperty(navigator, "clipboard", { value: {
          readText() {
            window.asyncCalls.push("read");
            return access === "denied" ? Promise.reject(new Error("denied")) : window.originalClipboard.readText();
          },
          writeText(text) {
            window.asyncCalls.push("write");
            return access === "denied" ? Promise.reject(new Error("denied")) : window.originalClipboard.writeText(text);
          },
        } });
      }, access);
      await app.page.keyboard.press("Control+c");
      assert.equal(await app.page.evaluate(() => window.originalClipboard.readText()), "clipboard selection");
      await app.page.evaluate(() => window.originalClipboard.writeText("Unicode λ🙂\r\nsecond\nthird"));
      await app.output("\x1b[?2004h");
      await app.page.keyboard.press("Control+v");
      await app.settle();
      assert.deepEqual(app.inputs.map((input) => input.data), ["\x1b[200~Unicode λ🙂\rsecond\rthird\x1b[201~"]);
      assert.deepEqual(await app.page.evaluate(() => window.asyncCalls), []);
      assert.deepEqual(await app.page.evaluate(() => window.copyWrites), [["text/plain", "clipboard selection"]]);
      const events = await app.page.evaluate(() => window.clipboardEvents);
      assert.equal(events.filter((event) => event.type === "copy" && event.trusted).length, 1);
      assert.equal(events.filter((event) => event.type === "paste" && event.trusted).length, 1);
      assert.deepEqual(app.errors, []);
    } finally { await app.close(); }
  });
}

for (const platform of ["Linux x86_64", "Win32", "MacIntel"]) {
  test(`selection overrides mouse capture on ${platform}`, { skip: CLIPBOARD_SKIP }, async () => {
    const app = await openClipboardApp({ platform });
    try {
      await app.addTab();
      const hint = app.page.locator('#terminal-selection-hint');
      assert.match(await hint.textContent(), platform === 'MacIntel' ? /Option-drag/ : /Shift-drag/);
      assert.equal(await app.page.locator('.terminal-output').getAttribute('aria-describedby'), 'terminal-selection-hint');
      await app.output("\x1b[?1049h\x1b[?1000h\x1b[?1006hmouse captured");
      assert.equal(await selectText(app.page, { length: 14, modifier: null }), "");
      await app.settle();
      assert.ok(app.inputs.some((input) => input.data.includes("\x1b[<")), "ordinary drag reaches the TUI");
      app.inputs.length = 0;
      assert.equal(await selectText(app.page, {
        length: 14, modifier: platform === "MacIntel" ? "Alt" : "Shift",
      }), "mouse captured");
      await app.page.keyboard.press("Control+c");
      await app.settle();
      assert.equal(await app.page.evaluate(() => navigator.clipboard.readText()), "mouse captured");
      assert.deepEqual(app.inputs, [], "selection and copy do not interrupt or report mouse input");
    } finally { await app.close(); }
  });
}

for (const access of ["allowed", "denied", "unavailable"]) {
  test(`context-menu clipboard path with async access ${access}`, { skip: CLIPBOARD_SKIP }, async () => {
    const app = await openClipboardApp();
    try {
      await app.addTab();
      await app.output("menu selection");
      assert.equal(await selectText(app.page, { length: 14 }), "menu selection");
      await app.page.evaluate(() => { window.originalClipboard = navigator.clipboard; });
      if (access === "denied") {
        await app.context.clearPermissions();
        await app.context.grantPermissions([]);
        assert.equal(await app.page.evaluate(async () =>
          (await navigator.permissions.query({ name: "clipboard-read" })).state), "denied");
        await assert.rejects(app.page.evaluate(() => navigator.clipboard.readText()), /denied/i);
      }
      if (access === "unavailable") await app.page.evaluate(() =>
        Object.defineProperty(navigator, "clipboard", { value: undefined }));
      // Headless Chromium cannot click the OS menu. Right-click the real xterm
      // textarea, then invoke Chromium's native editing commands (the same
      // trusted clipboard events used by the menu), without a JS ClipboardEvent.
      const cdp = await app.context.newCDPSession(app.page);
      await app.page.locator(".xterm-screen").click({ button: "right", position: { x: 10, y: 10 } });
      for (const command of ["copy", "paste"]) {
        await cdp.send("Input.dispatchKeyEvent", { type: "keyDown", key: "Unidentified", commands: [command] });
        await cdp.send("Input.dispatchKeyEvent", { type: "keyUp", key: "Unidentified" });
      }
      await app.settle();
      assert.deepEqual(app.inputs.map((input) => input.data), ["menu selection"]);
      if (access === "denied") await app.context.grantPermissions(["clipboard-read", "clipboard-write"]);
      assert.equal(await app.page.evaluate(() => window.originalClipboard.readText()), "menu selection");
      const events = await app.page.evaluate(() => window.clipboardEvents.filter((event) => event.type !== "keydown"));
      assert.deepEqual(events.map((event) => [event.type, event.trusted]), [["copy", true], ["paste", true]]);
      assert.deepEqual(app.errors, []);
    } finally { await app.close(); }
  });
}

test("native keyboard gestures survive actual browser permission denial", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    await app.addTab();
    await app.output("permission test");
    await selectText(app.page, { length: 15 });
    await app.context.clearPermissions();
    await app.context.grantPermissions([]);
    await assert.rejects(app.page.evaluate(() => navigator.clipboard.writeText("blocked")), /denied/i);
    await app.page.keyboard.press("Control+c");
    await app.page.keyboard.press("Control+v");
    await app.settle();
    assert.deepEqual(app.inputs.map((input) => input.data), ["permission test"]);
    assert.equal(await app.page.evaluate(() => terminalStateFor().error), "");
  } finally { await app.close(); }
});

test("terminal-specific fallback is singular and reports actual permission denial", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    await app.addTab();
    await app.output("fallback text");
    await selectText(app.page, { length: 13 });
    await app.page.keyboard.press("Control+Shift+c");
    await app.page.waitForFunction(async () => await navigator.clipboard.readText() === "fallback text");
    const pasted = app.page.waitForResponse((response) => response.url().endsWith("/input"));
    await app.page.keyboard.press("Control+Shift+v");
    await pasted;
    await app.settle();
    assert.deepEqual(app.inputs.splice(0).map((input) => input.data), ["fallback text"]);
    assert.equal(await app.page.evaluate(() => window.clipboardEvents.filter((event) => event.type !== "keydown").length), 0);
    await app.context.clearPermissions();
    await app.context.grantPermissions([]);
    await app.page.keyboard.press("Control+Shift+v");
    await app.page.waitForFunction(() => terminalStateFor().error.includes("Unable to paste"));
    assert.match(await app.page.locator('[data-testid="terminal-status"]').textContent(), /denied.*browser Copy\/Paste/i);
    await app.settle();
    assert.deepEqual(app.inputs, []);
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("Ctrl+Insert and Shift+Insert use native clipboard events exactly once", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    await app.addTab();
    await app.output("insert clipboard");
    assert.equal(await selectText(app.page, { length: 16 }), "insert clipboard");
    await app.page.evaluate(() => Object.defineProperty(navigator, "clipboard", { value: undefined }));
    await app.page.keyboard.press("Control+Insert");
    await app.page.keyboard.press("Shift+Insert");
    await app.settle();
    assert.deepEqual(app.inputs, [{ path: "/api/terminal/session-agent/input", data: "insert clipboard" }]);
    const events = await app.page.evaluate(() => window.clipboardEvents.filter((event) => event.type !== "keydown"));
    assert.deepEqual(events.map((event) => [event.type, event.trusted]), [["copy", true], ["paste", true]]);
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("all profiles and both provider fixtures retain native clipboard and key semantics", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    for (const provider of ["claude", "codex"]) {
      for (const mode of ["terminal", "agent", "plan", "goal", "standalone", "skill"]) {
        const id = `${mode}-${provider}`;
        await app.addTab({ id, mode, provider });
        await app.output(`selected ${id}`);
        assert.equal(await selectText(app.page, { length: `selected ${id}`.length }), `selected ${id}`);
        await app.page.keyboard.press("Control+c");
        assert.equal(await app.page.evaluate(() => navigator.clipboard.readText()), `selected ${id}`);
        for (const bracketed of [false, true]) {
          await app.output(`\x1b[?2004${bracketed ? "h" : "l"}`);
          await app.page.evaluate(() => navigator.clipboard.writeText("λ🙂\r\nline\nend"));
          await app.page.keyboard.press("Control+v");
          await app.settle();
          assert.deepEqual(app.inputs.splice(0), [{
            path: `/api/terminal/session-${id}/input`,
            data: bracketed ? "\x1b[200~λ🙂\rline\rend\x1b[201~" : "λ🙂\rline\rend",
          }]);
        }
        await app.page.keyboard.press("Control+c"); // paste cleared the selection
        await app.page.keyboard.press("Control+Enter");
        await app.page.keyboard.press("Control+z");
        await app.page.keyboard.press("Control+k");
        await app.settle();
        assert.equal(app.inputs.splice(0).map((input) => input.data).join(""),
          mode === "terminal" ? "\x03\n\x1a\x0b" : "\x03\n\x0b");
        assert.equal(await app.page.evaluate(() => !!document.querySelector(".command-palette-backdrop")), false);
      }
    }
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("retained renderer remounts and outside focus preserve one clipboard operation", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    await app.addTab({ id: "first" });
    await app.output("retained text");
    await app.page.evaluate(() => { window.firstRenderer = terminalStateFor().term; });
    await app.addTab({ id: "second", provider: "codex" });
    for (let n = 0; n < 3; n++) {
      await app.page.evaluate(() => {
        chatState.activeTabId = "first";
        drawToolbar();
        terminalStateFor().term.focus();
      });
      assert.equal(await app.page.evaluate(() => terminalStateFor().term === window.firstRenderer), true);
      assert.equal(await selectText(app.page, { length: 13 }), "retained text");
      await app.page.keyboard.press("Control+c");
      await app.page.keyboard.press("Control+v");
      await app.settle();
      assert.deepEqual(app.inputs.splice(0), [{ path: "/api/terminal/session-first/input", data: "retained text" }]);
      await app.page.evaluate(() => { chatState.activeTabId = "second"; drawToolbar(); });
    }
    // Finish the remount's scheduled terminal focus before the user moves to
    // an outside field. Input-queue settlement does not wait for animation frames.
    await app.page.evaluate(() => new Promise(requestAnimationFrame));
    await app.page.evaluate(() => {
      const input = document.createElement("textarea");
      input.id = "outside-clipboard";
      document.body.prepend(input);
    });
    await app.page.locator("#outside-clipboard").click();
    assert.equal(await app.page.evaluate(() => document.activeElement.id), "outside-clipboard");
    await app.page.keyboard.press("Control+v");
    await app.settle();
    assert.equal(await app.page.locator("#outside-clipboard").inputValue(), "retained text");
    assert.deepEqual(app.inputs, []);
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("pending fallback paste is fenced by tab, focus, exit, and session replacement", { skip: CLIPBOARD_SKIP }, async () => {
  const app = await openClipboardApp();
  try {
    for (const change of ["tab", "focus", "exit", "session", "renderer"]) {
      await app.addTab({ id: change });
      await app.page.evaluate(() => {
        Object.defineProperty(navigator, "clipboard", { configurable: true, value: {
          readText: () => new Promise((resolve) => { window.resolvePaste = resolve; }),
        } });
      });
      await app.page.keyboard.press("Control+Shift+v");
      await app.page.evaluate((change) => {
        const terminal = terminalStateFor();
        if (change === "session") terminal.sessionId = "replacement";
        if (change === "exit") terminal.exited = true;
        if (change === "tab") chatState.activeTabId = "absent";
        if (change === "focus") terminal.term.blur();
        if (change === "renderer") { terminal.term.dispose(); terminal.term = null; }
        window.resolvePaste("stale paste");
      }, change);
      await app.settle();
      assert.deepEqual(app.inputs, []);
    }
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});
