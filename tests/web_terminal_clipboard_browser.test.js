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
    for (const shortcut of ["Control+c", "Control+Shift+c", "Meta+c"]) {
      await app.page.keyboard.press(shortcut);
    }
    await app.page.waitForFunction(() => copyWrites.length === 4);
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
    await app.page.waitForFunction(() => copyWrites.length === 5);
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
