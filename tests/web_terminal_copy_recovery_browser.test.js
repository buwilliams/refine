const assert = require("node:assert/strict");
const test = require("node:test");
const { openTerminalApp, selectOutput, SKIP } = require("./support/terminal_browser");

for (const profile of ["agent", "skill"]) {

test(`${profile}: unavailable and rejected writes use the browser copy fallback and restore focus`, { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile });
  try {
    await selectOutput(app.page);
    await app.page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await app.page.evaluate(() => {
      delete navigator.clipboard;
      window.realClipboard = navigator.clipboard;
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: {} });
    });
    await app.page.getByRole("button", { name: "Copy selection", exact: true }).click();
    await app.page.waitForFunction(() => terminalStateFor().clipboard.message === "Selection copied.");
    assert.equal(await app.page.evaluate(() => realClipboard.readText()), "OUTPUT-agent-a");
    assert.equal(await app.page.evaluate(() => document.activeElement === terminalStateFor().term.textarea), true);
    await app.page.evaluate(() => {
      navigator.clipboard.writeText = async () => { throw new Error("denied"); };
      terminalStateFor().term.select(0, 1, 6);
    });
    await app.page.getByRole("button", { name: "Copy selection", exact: true }).click();
    await app.page.waitForFunction(() => terminalStateFor().clipboard.message === "Selection copied.");
    assert.equal(await app.page.evaluate(() => realClipboard.readText()), "second");
    assert.equal(await app.page.evaluate(() => terminalStateFor().term === originalTerm), true);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test(`${profile}: blocked copying offers a standard field scoped to the originating tab`, { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile });
  try {
    await selectOutput(app.page);
    await app.page.evaluate(() => {
      window.execCalls = 0;
      document.execCommand = () => { execCalls += 1; return false; };
      navigator.clipboard.writeText = () => new Promise((_, reject) => { window.rejectCopy = reject; });
    });
    await app.page.keyboard.press("Control+Shift+c");
    assert.equal(await app.page.evaluate(() => terminalStateFor().clipboard?.pending
      && typeof window.rejectCopy === "function"), true, "fallback copy must be pending before changing tabs");
    await app.page.evaluate(async () => {
      finishTerminalExit(currentToolbarTab(), terminalStateFor());
      terminalStateFor().term.clearSelection();
      await activateToolbarTab(clipboardTabs.b);
      terminalStateFor().term.select(0, 0, 6);
      terminalStateFor().term.focus();
    });
    await app.page.evaluate(() => rejectCopy(new Error("denied")));
    await app.page.waitForFunction(() => terminalStates.get(clipboardTabs.a).clipboard.recovery);
    assert.deepEqual(await app.page.evaluate(() => ({
      tab: chatState.activeTabId, focus: document.activeElement === terminalStateFor().term.textarea,
      calls: execCalls, selection: terminalSelection(terminalStateFor()),
    })), { tab: app.second, focus: true, calls: 0, selection: "OUTPUT" });
    assert.equal(await app.page.locator("[data-terminal-copy-recovery]").isVisible(), false);
    await app.page.evaluate(() => activateToolbarTab(clipboardTabs.a));
    const field = app.page.getByRole("textbox", { name: "Selected terminal text", exact: true });
    assert.equal(await field.inputValue(), "OUTPUT-agent-a");
    await app.page.getByRole("button", { name: "Select text", exact: true }).click();
    assert.deepEqual(await field.evaluate((el) => [el.selectionStart, el.selectionEnd]), [0, 14]);
    await app.page.evaluate(() => {
      drawToolbar();
      return new Promise(requestAnimationFrame);
    });
    assert.equal(await field.evaluate((el) => document.activeElement === el), true);
    assert.deepEqual(await field.evaluate((el) => [el.selectionStart, el.selectionEnd]), [0, 14]);
    assert.equal(await app.page.evaluate(() => terminalStateFor().term === originalTerm), true);
    const copied = await field.evaluate((el) => {
      const e = new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true });
      el.dispatchEvent(e);
      return e.defaultPrevented;
    });
    assert.equal(copied, false, "manual field keeps native browser copying");
    await app.page.getByRole("button", { name: "Dismiss", exact: true }).click();
    assert.equal(await field.isVisible(), false);
    assert.deepEqual(app.requests, []);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});

test(`${profile}: failed browser fallbacks expose captured text and never claim success`, { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile });
  try {
    for (const failure of ["missing", "throw", "reject", "native", "native-throw", "getter"]) {
      await selectOutput(app.page);
      await app.page.evaluate((mode) => {
        document.execCommand = () => {
          if (mode === "throw") throw new Error("copy disabled");
          return false;
        };
        navigator.clipboard.writeText = mode === "missing" || mode === "native" ? undefined
          : mode === "throw" ? () => { throw new Error("unavailable"); }
          : async () => { throw new Error("denied"); };
        if (mode === "getter") Object.defineProperty(navigator, "clipboard", {
          configurable: true, get() { throw new Error("clipboard unavailable"); },
        });
        if (mode.startsWith("native")) {
          const data = new DataTransfer();
          if (mode === "native-throw") data.setData = () => { throw new Error("native copy unavailable"); };
          terminalStateFor().term.textarea.dispatchEvent(new ClipboardEvent("copy", {
            clipboardData: mode === "native-throw" ? data : undefined, bubbles: true, cancelable: true,
          }));
        }
      }, failure);
      if (!failure.startsWith("native")) await app.page.keyboard.press("Control+Shift+c");
      await app.page.waitForFunction(() => terminalStateFor().clipboard.recovery);
      const field = app.page.getByRole("textbox", { name: "Selected terminal text", exact: true });
      assert.equal(await field.inputValue(), "OUTPUT-agent-a");
      assert.match(await app.page.locator("[data-terminal-copy-status]").innerText(), /Automatic copy was blocked/);
      assert.equal(await app.page.evaluate(() => document.activeElement === terminalStateFor().term.textarea), true);
    }
    assert.deepEqual(app.requests, []);
    assert.deepEqual(app.pageErrors, []);
  } finally {
    await app.close();
  }
});


}

test('delayed rejection preserves unrelated input and manual recovery selection through redraws', { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile: 'skill' });
  try {
    const { page } = app;
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await selectOutput(page);
    await page.evaluate(() => {
      navigator.clipboard.writeText = () => new Promise((_, reject) => { window.rejectCopy = reject; });
      window.fallbackCalls = 0;
      document.execCommand = () => { fallbackCalls++; return false; };
    });
    await page.keyboard.press('Control+Shift+c');
    assert.equal(await page.evaluate(() => terminalStateFor().clipboard?.pending
      && typeof window.rejectCopy === 'function'), true, 'fallback copy must be pending before moving focus');
    await page.evaluate(() => {
      const field = document.createElement('textarea');
      field.id = 'unrelated-input';
      field.value = 'ordinary input';
      document.body.appendChild(field);
      field.focus();
      field.setSelectionRange(2, 8, 'backward');
      rejectCopy(new Error('denied'));
    });
    await page.waitForFunction(() => terminalStateFor().clipboard.recovery);
    await page.evaluate(() => { drawToolbar(); });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.deepEqual(await page.locator('#unrelated-input').evaluate(el => ({
      focus: document.activeElement === el, start: el.selectionStart, end: el.selectionEnd,
      direction: el.selectionDirection, fallbackCalls,
    })), { focus: true, start: 2, end: 8, direction: 'backward', fallbackCalls: 0 });
    await page.keyboard.type('replacement');
    assert.equal(await page.locator('#unrelated-input').inputValue(), 'orreplacement input');
    assert.deepEqual(app.requests, []);
    await page.getByRole('button', { name: 'Select text', exact: true }).click();
    const recovery = page.getByRole('textbox', { name: 'Selected terminal text', exact: true });
    await recovery.evaluate(el => el.setSelectionRange(2, 8, 'backward'));
    await page.evaluate(() => { drawToolbar(); });
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.deepEqual(await recovery.evaluate(el => [document.activeElement === el, el.selectionStart, el.selectionEnd, el.selectionDirection]), [true, 2, 8, 'backward']);
    await page.keyboard.press('Control+c');
    assert.equal(await page.evaluate(() => realClipboard.readText()), 'TPUT-a');
    assert.match(await page.locator('[data-terminal-copy-status]').innerText(), /Automatic copy was blocked/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('selection fallback restores both ordinary field carets and DOM ranges', { skip: SKIP }, async () => {
  const app = await openTerminalApp();
  try {
    const { page } = app;
    await selectOutput(page);
    await page.evaluate(() => {
      navigator.clipboard.writeText = undefined;
      document.execCommand = () => false;
      const field = document.createElement('textarea');
      field.id = 'outside'; field.value = 'outside selection';
      document.body.appendChild(field); field.focus(); field.setSelectionRange(3, 9, 'backward');
    });
    await page.getByRole('button', { name: 'Copy selection', exact: true }).click();
    assert.deepEqual(await page.locator('#outside').evaluate(el => [document.activeElement === el, el.selectionStart, el.selectionEnd, el.selectionDirection]), [true, 3, 9, 'backward']);
    await page.evaluate(() => {
      const paragraph = document.createElement('p');
      paragraph.textContent = 'DOM selection'; document.body.appendChild(paragraph);
      const range = document.createRange(); range.selectNodeContents(paragraph);
      terminalStateFor().term.focus();
      getSelection().removeAllRanges(); getSelection().addRange(range);
    });
    await page.getByRole('button', { name: 'Copy selection', exact: true }).click();
    assert.equal(await page.evaluate(() => getSelection().toString()), 'DOM selection');
    assert.equal(await page.evaluate(() => terminalSelection(terminalStateFor())), 'OUTPUT-agent-a');
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

for (const profile of ['agent', 'skill']) {
  test(`${profile}: browser-enforced write denial falls back and then offers manual recovery`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ profile, mockClipboard: false });
    try {
      const { page } = app;
      await page.context().grantPermissions([]);
      assert.equal(await page.evaluate(async () => {
        try { await navigator.clipboard.writeText('denied probe'); return 'unexpected success'; }
        catch (error) { return error.name; }
      }), 'NotAllowedError');
      await selectOutput(page);
      await page.getByRole('button', { name: 'Copy selection', exact: true }).click();
      await page.waitForFunction(() => terminalStateFor().clipboard.message === 'Selection copied.');
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'OUTPUT-agent-a');
      await page.context().clearPermissions();
      await page.context().grantPermissions([]);
      await page.evaluate(() => { document.execCommand = () => false; });
      await page.getByRole('button', { name: 'Copy selection', exact: true }).click();
      await page.waitForFunction(() => terminalStateFor().clipboard.recovery);
      assert.equal(await page.getByRole('textbox', { name: 'Selected terminal text', exact: true }).inputValue(), 'OUTPUT-agent-a');
      assert.deepEqual(app.requests, []);
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });
}
