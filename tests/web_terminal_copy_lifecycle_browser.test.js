const assert = require('node:assert/strict');
const test = require('node:test');
const { openTerminalApp, selectOutput, SKIP } = require('./support/terminal_browser');

for (const profile of ['agent', 'skill']) {
  test(`${profile}: real clipboard contents, disconnected and exited ANSI output remain copyable`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ profile, mockClipboard: false });
    try {
      const { page } = app;
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await selectOutput(page);
      const button = page.getByRole('button', { name: 'Copy selection', exact: true });
      await button.focus();
      await page.keyboard.press('Space');
      await page.waitForFunction(() => terminalStateFor().clipboard?.message === 'Selection copied.');
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'OUTPUT-agent-a');
      assert.equal(await button.evaluate(el => document.activeElement === el), true);
      // Hold the real stream reconnection status request while copying.
      let releaseStatus;
      await page.route('**/api/terminal/*/status', async route => {
        await new Promise(resolve => { releaseStatus = resolve; });
        await route.fulfill({ json: { alive: true } });
      });
      await page.evaluate(() => terminalStateFor().eventSource.onerror());
      await page.waitForFunction(() => terminalStateFor().reattaching && !terminalStateFor().connected);
      await page.evaluate(() => terminalStateFor().term.focus());
      await page.keyboard.press('Control+Shift+c');
      await page.waitForFunction(() => !terminalStateFor().clipboard.pending);
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'OUTPUT-agent-a');
      releaseStatus();
      await page.waitForFunction(() => terminalStateFor().connected);
      await page.evaluate(() => terminalStateFor().eventSource.emit('terminal_exit', {}));
      await page.getByRole('button', { name: 'Restart', exact: true }).waitFor();
      // execCommand dispatches a browser Copy event with writable clipboard data.
      assert.equal(await page.evaluate(() => {
        terminalStateFor().term.focus();
        return document.execCommand('copy');
      }), true);
      assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'OUTPUT-agent-a');
      assert.deepEqual(await page.evaluate(() => ({
        selected: terminalSelection(terminalStateFor()), same: terminalStateFor().term === originalTerm,
        rendered: terminalStateFor().term.buffer.active.getLine(0).translateToString(true),
      })), { selected: 'OUTPUT-agent-a', same: true, rendered: 'OUTPUT-agent-a' });
      assert.deepEqual(app.requests, []);
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });

  test(`${profile}: selection made during reattachment defers restart and forced scrolling`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ profile });
    try {
      const { page } = app;
      await page.evaluate(async () => {
        const terminal = terminalStateFor();
        await new Promise(resolve => terminal.term.write('\r\n' + 'retained line\r\n'.repeat(80), resolve));
        await activateToolbarTab(clipboardTabs.b);
        terminal.statusChecked = false;
        terminal.connected = false;
      });
      let releaseStatus;
      await page.route('**/api/terminal/*/status', async route => {
        await new Promise(resolve => { releaseStatus = resolve; });
        await route.fulfill({ json: { alive: false } });
      });
      await page.evaluate(() => { window.activation = activateToolbarTab(clipboardTabs.a); });
      await page.waitForFunction(() => terminalStateFor().reattaching);
      await page.evaluate(() => {
        terminalStateFor().term.scrollToTop();
        terminalStateFor().term.select(0, 0, 14);
        terminalStateFor().term.clearSelection();
      });
      releaseStatus();
      await page.evaluate(() => activation);
      await page.getByRole('button', { name: 'Restart', exact: true }).waitFor();
      await page.evaluate(async () => {
        await activateToolbarTab(clipboardTabs.b);
        await activateToolbarTab(clipboardTabs.a);
      });
      assert.deepEqual(await page.evaluate(() => ({
        same: terminalStateFor().term === originalTerm,
        selected: terminalSelection(terminalStateFor()), viewport: terminalStateFor().term.buffer.active.viewportY,
      })), { same: true, selected: 'OUTPUT-agent-a', viewport: 0 });
      assert.equal(app.launches.length, 2);
      await page.evaluate(() => terminalStateFor().term.focus());
      await page.keyboard.press('Escape');
      await page.evaluate(async () => {
        await activateToolbarTab(clipboardTabs.b);
        await activateToolbarTab(clipboardTabs.a);
      });
      await page.locator('[data-testid="terminal-stop"]').waitFor();
      assert.equal(app.launches.length, 3, 'invalidation releases automatic-start suppression');
      assert.equal(await page.evaluate(() => terminalStateFor().term !== originalTerm
        && !terminalStateFor().selectionSnapshot), true);
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });

  test(`${profile}: pending copy, history arrival, recovery and explicit Restart retain their own context`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ profile });
    try {
      const { page } = app;
      await selectOutput(page);
      await page.evaluate(() => {
        navigator.clipboard.writeText = () => new Promise((_, reject) => { window.rejectCopy = reject; });
        document.execCommand = () => false;
      });
      await page.keyboard.press('Control+Shift+c');
      assert.equal(await page.evaluate(() => terminalStateFor().clipboard?.pending
        && typeof window.rejectCopy === 'function'), true, 'fallback copy must be pending before history and exit');
      await page.evaluate(async () => {
        terminalPrependOutput('earlier context\r\n', terminalStateFor());
        terminalStateFor().eventSource.emit('terminal_exit', {});
        terminalStateFor().term.clearSelection();
        await activateToolbarTab(clipboardTabs.b);
        await activateToolbarTab(clipboardTabs.a);
      });
      assert.equal(app.launches.length, 2);
      assert.equal(await page.evaluate(() => terminalStateFor().term.buffer.active.getLine(0).translateToString(true)), 'OUTPUT-agent-a');
      await page.evaluate(() => rejectCopy(new Error('denied')));
      await page.waitForFunction(() => terminalStateFor().clipboard.recovery);
      assert.equal(await page.getByRole('textbox', { name: 'Selected terminal text', exact: true }).inputValue(), 'OUTPUT-agent-a');
      await page.getByRole('button', { name: 'Dismiss', exact: true }).click();
      assert.equal(await page.evaluate(() => terminalStateFor().historyReplayPending), true);
      await page.evaluate(() => terminalStateFor().term.focus());
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => terminalStateFor().term.buffer.active.getLine(0).translateToString(true) === 'earlier context');
      await page.evaluate(() => {
        terminalStateFor().term.select(0, 0, 7);
        terminalStateFor().term.clearSelection();
      });
      assert.equal(await page.evaluate(() => terminalSelection(terminalStateFor())), 'earlier');
      await page.getByRole('button', { name: 'Restart', exact: true }).click();
      await page.locator('[data-testid="terminal-stop"]').waitFor();
      assert.equal(app.launches.length, 3);
      assert.equal(await page.evaluate(() => terminalStateFor().selectionSnapshot), null);
      assert.equal(await page.evaluate(() => terminalStateFor().term !== originalTerm && !terminalStateFor().clipboard), true);
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });
}

test('overlapping attempts and late close, session and renderer results cannot overwrite feedback or focus', { skip: SKIP }, async () => {
  const app = await openTerminalApp();
  try {
    const { page } = app;
    for (const replacement of ['attempt', 'session', 'renderer', 'close', 'restart']) {
      // Use the second UI-launched tab for close; the first stays available.
      if (replacement === 'close') await page.evaluate(() => activateToolbarTab(clipboardTabs.b));
      await selectOutput(page);
      await page.evaluate(() => {
        window.copyOrigin = terminalStateFor();
        navigator.clipboard.writeText = () => new Promise((resolve, reject) => {
          window.resolveCopy = resolve; window.rejectCopy = reject;
        });
      });
      await page.keyboard.press('Control+Shift+c');
      assert.equal(await page.evaluate(() => terminalStateFor().clipboard?.pending
        && typeof window.rejectCopy === 'function'), true, `fallback copy must be pending before ${replacement}`);
      await page.evaluate(async replacement => {
        if (replacement === 'attempt') {
          navigator.clipboard.writeText = async () => {};
          terminalStateFor().term.select(0, 1, 6);
          copyTerminalSelection(terminalStateFor());
        }
        if (replacement === 'session') terminalStateFor().sessionId += '-replacement';
        if (replacement === 'renderer') {
          terminalStateFor().term.element.remove();
          terminalStateFor().term.dispose();
          terminalStateFor().term = null;
          terminalStateFor().clipboard = null;
          ensureTerminalRenderer(document.querySelector('.terminal-output'), currentToolbarTab());
          await new Promise(resolve => terminalStateFor().term.write('OUTPUT-agent-a\r\nsecond line', resolve));
        }
      }, replacement);
      if (replacement === 'close') {
        await page.locator(`[data-close-tab="${app.second}"]`).click();
      } else if (replacement === 'restart') {
        await page.evaluate(() => terminalStateFor().eventSource.emit('terminal_exit', {}));
        await page.getByRole('button', { name: 'Restart', exact: true }).click();
        await page.locator('[data-testid="terminal-stop"]').waitFor();
      }
      await page.evaluate(() => {
        window.beforeLate = { clipboard: copyOrigin.clipboard, message: copyOrigin.clipboard?.message, term: copyOrigin.term };
        document.execCommand = () => { throw new Error('obsolete fallback must not run'); };
        rejectCopy(new Error('denied'));
      });
      await page.evaluate(() => new Promise(requestAnimationFrame));
      assert.equal(await page.evaluate(() => copyOrigin.clipboard === beforeLate.clipboard
        && copyOrigin.clipboard?.message === beforeLate.message && copyOrigin.term === beforeLate.term), true, replacement);
      if (replacement === 'attempt') assert.equal(await page.evaluate(() => copyOrigin.clipboard.text), 'second');
    }
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

for (const profile of ['agent', 'skill']) {
  test(`${profile}: keyboard clipboard paste reaches the PTY exactly once with native framing`, { skip: SKIP }, async () => {
    const app = await openTerminalApp({ profile, mockClipboard: false });
    try {
      const { page } = app;
      await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
      await page.evaluate(async () => {
        const term = terminalStateFor().term;
        await new Promise(resolve => term.write('\x1b[?2004h', resolve));
        await navigator.clipboard.writeText('first\r\nsecond\n');
        term.focus();
      });
      await page.keyboard.press('Control+v');
      await page.waitForTimeout(50); // Include the buffered send and any duplicate native paste event.
      assert.deepEqual(app.requests, ['\x1b[200~first\rsecond\r\x1b[201~']);
      app.requests.length = 0;
      await page.keyboard.press('Control+Enter');
      await page.keyboard.press('Control+z');
      await page.keyboard.type('x');
      await page.waitForTimeout(50);
      assert.equal(app.requests.join(''), '\nx');
      assert.deepEqual(app.pageErrors, []);
    } finally { await app.close(); }
  });
}

test('successful delayed copies stay with their originating tab and superseded success is ignored', { skip: SKIP }, async () => {
  const app = await openTerminalApp({ profile: 'skill' });
  try {
    const { page } = app;
    await selectOutput(page);
    await page.evaluate(() => {
      navigator.clipboard.writeText = () => new Promise(resolve => { window.resolveCopy = resolve; });
    });
    await page.keyboard.press('Control+Shift+c');
    assert.equal(await page.evaluate(() => terminalStateFor().clipboard?.pending
      && typeof window.resolveCopy === 'function'), true, 'fallback copy must be pending before switching tabs');
    await page.evaluate(async () => {
      await activateToolbarTab(clipboardTabs.b);
      terminalStateFor().term.focus();
      resolveCopy();
    });
    await page.waitForFunction(() => terminalStates.get(clipboardTabs.a).clipboard.message === 'Selection copied.');
    assert.equal(await page.locator('[data-terminal-copy-status]').innerText(), '');
    assert.equal(await page.evaluate(() => document.activeElement === terminalStateFor().term.textarea), true);
    await page.evaluate(() => activateToolbarTab(clipboardTabs.a));
    await selectOutput(page);
    await page.keyboard.press('Control+Shift+c');
    assert.equal(await page.evaluate(() => terminalStateFor().clipboard?.pending
      && typeof window.resolveCopy === 'function'), true, 'fallback copy must be pending before supersession');
    await page.evaluate(() => {
      navigator.clipboard.writeText = async () => { throw new Error('new attempt denied'); };
      document.execCommand = () => false;
      terminalStateFor().term.select(0, 1, 6);
      copyTerminalSelection(terminalStateFor());
    });
    await page.waitForFunction(() => terminalStateFor().clipboard.recovery);
    await page.evaluate(() => resolveCopy());
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await page.getByRole('textbox', { name: 'Selected terminal text', exact: true }).inputValue(), 'second');
    assert.match(await page.locator('[data-terminal-copy-status]').innerText(), /Automatic copy was blocked/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
