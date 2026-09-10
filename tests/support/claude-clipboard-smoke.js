// Opt-in live provider smoke: node tests/support/claude-clipboard-smoke.js <evidence-dir>
// Requires Playwright/Chromium, Claude authentication, @lydell/node-pty, and
// an evidence directory under a scratch parent already trusted by Claude.
// Uses an isolated scratch directory and leaves the pasted prompt unsubmitted.
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const pty = require("@lydell/node-pty");
const { SKIP, openTerminalApp, selectText } = require("./terminal-clipboard-browser");

async function main() {
  assert.equal(SKIP, false, String(SKIP));
  const evidenceDir = process.argv[2] || fs.mkdtempSync(path.join(os.tmpdir(), "refine-claude-clipboard-"));
  fs.mkdirSync(evidenceDir, { recursive: true });
  const scratch = fs.mkdtempSync(path.join(evidenceDir, "session-"));
  const executable = process.env.CLAUDE_BIN || "claude";
  const version = execFileSync(executable, ["--version"], { encoding: "utf8" }).trim();
  let child;
  let dataSubscription;
  let transcript = "";
  let output = Promise.resolve();
  const app = await openTerminalApp({ onInput: (data) => child.write(data) });
  try {
    await app.page.setViewportSize({ width: 1400, height: 1000 });
    await app.addTab();
    await app.page.evaluate(() => { chatState.bodyHeight = 650; drawToolbar(); });
    const size = await app.page.evaluate(() => ({ cols: terminalStateFor().term.cols, rows: terminalStateFor().term.rows }));
    child = pty.spawn(executable, ["--safe-mode", "--setting-sources", "", "--permission-mode", "dontAsk",
      "--tools", "", "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}', "--no-chrome"], {
      ...size, cwd: scratch, name: "xterm-256color",
      env: { ...process.env, DISABLE_AUTOUPDATER: "1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1" },
    });
    dataSubscription = child.onData((data) => {
      transcript += data;
      output = output.then(() => app.output(data));
    });
    await app.page.evaluate(() => {
      window.claudeSmokeScreen = () => {
        const term = terminalStateFor().term;
        return Array.from({ length: term.rows }, (_, row) =>
          term.buffer.active.getLine(term.buffer.active.viewportY + row)?.translateToString() || "").join("\n");
      };
    });
    await app.page.waitForFunction(() => {
      const text = claudeSmokeScreen();
      return text.includes("Yes, I trust this folder")
        || (text.includes("Claude Code v") && /^\s*❯\s*$/m.test(text));
    });
    const trustPrompt = await app.page.evaluate(() => claudeSmokeScreen().includes("Yes, I trust this folder"));
    assert.equal(trustPrompt, false,
      "Claude requires workspace trust; use an evidence directory under an already trusted scratch parent.");
    // Wait for the ready prompt, then paste through the real browser event and
    // managed-session input fixture into the isolated Claude PTY.
    await app.page.waitForFunction(() => {
      const text = claudeSmokeScreen();
      return text.includes("Claude Code v") && !text.includes("Yes, I trust this folder")
        && /^\s*❯\s*$/m.test(text);
    });
    await output;
    await app.page.screenshot({ path: path.join(evidenceDir, "claude-start.png") });
    await app.settle();
    app.inputs.length = 0;
    const pasted = "clipboard-smoke λ🙂\nsecond line";
    await app.page.evaluate((text) => navigator.clipboard.writeText(text), pasted);
    await app.page.keyboard.press("Control+v");
    await app.settle();
    await app.page.waitForFunction(() => {
      const buffer = terminalStateFor().term.buffer.active;
      return Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i)?.translateToString()).join("\n").includes("clipboard-smoke");
    });
    // Claude can query terminal attributes during startup. xterm's DA reply
    // legitimately shares the managed input queue with the paste.
    const userInput = (inputs) => inputs.map((input) => input.data).join("").replaceAll("\x1b[?1;2c", "");
    assert.equal(userInput(app.inputs), "\x1b[200~clipboard-smoke λ🙂\rsecond line\x1b[201~");
    const pasteInputs = [...app.inputs];
    const promptPosition = await app.page.evaluate(() => {
      const buffer = terminalStateFor().term.buffer.active;
      for (let row = 0; row < terminalStateFor().term.rows; row++) {
        const line = buffer.getLine(buffer.viewportY + row)?.translateToString() || "";
        if (line.includes("clipboard-smoke")) return { row, start: line.indexOf("clipboard-smoke") };
      }
    });
    assert.ok(promptPosition);
    const modifier = await app.page.evaluate(() => terminalStateFor().term.modes.mouseTrackingMode === "none" ? null : "Shift");
    assert.equal(await selectText(app.page, { ...promptPosition, length: 15, modifier }), "clipboard-smoke");
    const beforeCopy = app.inputs.length;
    await app.page.keyboard.press("Control+c");
    await app.settle();
    assert.equal(await app.page.evaluate(() => navigator.clipboard.readText()), "clipboard-smoke");
    assert.equal(userInput(app.inputs.slice(beforeCopy)), "", "copy must not send Ctrl+C to Claude");
    await app.page.screenshot({ path: path.join(evidenceDir, "claude-pasted-selected.png") });
    const browserEvents = await app.page.evaluate(() => window.clipboardEvents.filter((event) => event.type !== "keydown"));
    // Also observe the provider's ordinary mouse gesture, without inventing an
    // OSC protocol dependency from a synthetic terminal fixture.
    await app.page.evaluate(() => terminalStateFor().term.clearSelection());
    const beforeProviderGesture = transcript.length;
    const browserSelection = await selectText(app.page, { ...promptPosition, length: 15 });
    await app.settle();
    await new Promise((resolve) => setTimeout(resolve, 250));
    await output;
    const providerOutput = transcript.slice(beforeProviderGesture);
    const report = {
      version, scratch, size, pasteInputs, browserEvents,
      copy: `${modifier ? modifier + "-drag" : "Drag"} xterm selection -> trusted browser copy event -> text/plain clipboard; no interrupt`,
      pastePath: "trusted browser paste event -> Terminal.paste -> onData -> /api/terminal/session-agent/input fixture -> isolated Claude PTY",
      alternateScreen: transcript.includes("\x1b[?1049h"),
      mouseReporting: transcript.includes("\x1b[?1003h"),
      bracketedPaste: transcript.includes("\x1b[?2004h"),
      providerGesture: { browserSelection, emittedOsc52: /\x1b\]52;/.test(providerOutput) },
      osc52InTranscript: /\x1b\]52;/.test(transcript),
      pageErrors: app.errors,
    };
    assert.deepEqual(app.errors, []);
    fs.writeFileSync(path.join(evidenceDir, "claude-smoke.json"), JSON.stringify(report, null, 2) + "\n");
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    await app.page.screenshot({ path: path.join(evidenceDir, "claude-failure.png") });
    throw error;
  } finally {
    dataSubscription?.dispose();
    if (child) child.kill();
    await output;
    fs.writeFileSync(path.join(evidenceDir, "claude-transcript.ansi"), transcript);
    await app.close();
  }
}

main().catch((error) => { console.error(error); process.exitCode = 1; });
