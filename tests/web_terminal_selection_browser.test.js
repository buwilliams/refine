const assert = require("node:assert/strict");
const test = require("node:test");
const { SKIP, openTerminalApp, selectText } = require("./support/terminal-clipboard-browser");

// All-motion SGR tracking reports unmodified movement after a forced selection.
// That report is user input and clears xterm's visible selection.
async function mouseClearedSelection(app, repeated = false) {
  const { page } = app;
  await app.output("\x1b[?1003h\x1b[?1006h");
  await page.waitForTimeout(550); // Separate gestures and allow scheduled renderer fitting.
  const cell = await page.evaluate(() => {
    const term = terminalStateFor().term;
    window.selectionChanges = [];
    term.onSelectionChange(() => selectionChanges.push(term.getSelection()));
    const rect = term.element.querySelector(".xterm-screen").getBoundingClientRect();
    const size = term._core._renderService.dimensions.css.cell;
    return { x: rect.x, y: rect.y, w: size.width, h: size.height };
  });
  await page.keyboard.down("Shift");
  await page.mouse.move(cell.x + cell.w * 0.1, cell.y + cell.h * 0.5);
  await page.mouse.down();
  await page.mouse.move(cell.x + cell.w * 8.1, cell.y + cell.h * 0.5, { steps: 8 });
  assert.equal(await page.evaluate(() => terminalStateFor().term.getSelection()), "captured",
    JSON.stringify(await page.evaluate(() => ({ viewport: terminalStateFor().term.buffer.active.viewportY,
      rows: terminalStateFor().term.rows, changes: selectionChanges, selection: terminalStateFor().term.getSelectionPosition() }))));
  await page.keyboard.up("Shift");
  await page.mouse.up();
  await page.mouse.move(cell.x + cell.w * 9.1, cell.y + cell.h * 0.5);
  await app.settle();
  assert.deepEqual(await page.evaluate(() => ({
    live: terminalStateFor().term.getSelection(), captured: terminalSelection(terminalStateFor()),
    last: selectionChanges.at(-1), sawText: selectionChanges.includes("captured"),
  })), { live: "", captured: "captured", last: "", sawText: !repeated });
  assert.match(app.inputs.splice(0).map(({ data }) => data).join(""), /\x1b\[</);
  assert.equal(await page.locator("[data-terminal-copy]").isEnabled(), true);
}

test("non-secure HTTP: SGR-cleared text supports trusted native copy/paste and Copy fallback", { skip: SKIP }, async () => {
  const app = await openTerminalApp({ insecure: true });
  try {
    const { page } = app;
    await app.addTab();
    assert.deepEqual(await page.evaluate(() => ({ secure: isSecureContext, clipboard: typeof navigator.clipboard })),
      { secure: false, clipboard: "undefined" });
    await app.output("captured text\r\n");
    await mouseClearedSelection(app);
    await page.keyboard.press("Control+c");
    await page.waitForFunction(() => terminalStateFor().clipboard?.message === "Selection copied.");
    assert.equal(await page.evaluate(() => terminalSelection(terminalStateFor())), "");
    await app.output("\x1b[?2004h");
    await page.keyboard.press("Control+v");
    await app.settle();
    assert.deepEqual(app.inputs.splice(0), [{ path: "/api/terminal/session-agent/input", data: "\x1b[200~captured\x1b[201~" }]);
    assert.deepEqual(await page.evaluate(() => clipboardEvents.filter(e => e.type !== "keydown").map(e => [e.type, e.trusted])),
      [["copy", true], ["paste", true]]);
    await page.keyboard.press("Control+c");
    await app.settle();
    assert.deepEqual(app.inputs.splice(0).map(e => e.data), ["\x03"]);

    await mouseClearedSelection(app, true);
    await page.locator("[data-terminal-copy]").click();
    await page.waitForFunction(() => terminalStateFor().clipboard?.message === "Selection copied.");
    await page.evaluate(() => terminalStateFor().term.focus());
    await page.keyboard.press("Control+v");
    await app.settle();
    assert.deepEqual(app.inputs.splice(0).map(e => e.data), ["\x1b[200~captured\x1b[201~"]);

    await mouseClearedSelection(app, true);
    await page.evaluate(() => { document.execCommand = () => false; });
    await page.locator("[data-terminal-copy]").click();
    const recovery = page.getByRole("textbox", { name: "Selected terminal text", exact: true });
    assert.equal(await recovery.inputValue(), "captured");
    await page.getByRole("button", { name: "Select text", exact: true }).click();
    await page.keyboard.press("Control+c");
    await page.evaluate(() => terminalStateFor().term.focus());
    await page.keyboard.press("Control+v");
    await app.settle();
    assert.deepEqual(app.inputs.splice(0).map(e => e.data), ["\x1b[200~captured\x1b[201~"]);
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});

test("snapshots survive output and remount, but renderer replacement and disposal release them", { skip: SKIP }, async () => {
  const app = await openTerminalApp();
  try {
    const { page } = app;
    await app.addTab({ id: "first" });
    await app.output("captured text");
    await selectText(page, { length: 8 });
    await page.evaluate(() => {
      window.original = terminalStateFor();
      window.renderer = original.term;
      renderer.clearSelection();
    });
    await app.output("\r\nmore output");
    await app.addTab({ id: "second" });
    await page.evaluate(() => {
      chatState.activeTabId = "first";
      original.connected = false;
      original.exited = true;
      drawToolbar();
    });
    assert.deepEqual(await page.evaluate(() => ({ same: original.term === renderer, text: terminalSelection(original) })),
      { same: true, text: "captured" });
    await page.evaluate(() => {
      original.term.dispose();
      original.term = null;
      ensureTerminalRenderer(document.querySelector(".terminal-output"));
    });
    assert.equal(await page.evaluate(() => terminalSelection(original)), "");
    await page.evaluate(() => {
      original.term.select(0, 0, 4);
      original.term.clearSelection();
      removeToolbarTab("first");
    });
    assert.equal(await page.evaluate(() => original.selectionSnapshot), null);
    assert.deepEqual(app.errors, []);
  } finally { await app.close(); }
});
