const { spawn } = require("node:child_process");
const path = require("node:path");
const { BROWSER, SKIP } = require("./web_app");

// Use the production static serving path, including query-string handling.
// REFINE_BIN lets isolated worktrees reuse an already-built Refine executable.
async function openWebsite() {
  const root = path.join(__dirname, "../..");
  const server = spawn(process.env.REFINE_BIN || path.join(root, "target/debug/refine"),
    ["website", "--port", "0", "--static-root", root], { stdio: ["ignore", "ignore", "pipe"] });
  const stopped = new Promise(resolve => server.once("close", resolve));
  let browser;
  async function close() {
    try { if (browser) await browser.close(); }
    finally {
      server.kill();
      await stopped;
    }
  }
  try {
    const origin = await new Promise((resolve, reject) => {
      let output = "";
      const timer = setTimeout(() => reject(new Error(`Website server startup timed out: ${output}`)), 10000);
      server.once("error", error => {
        clearTimeout(timer);
        reject(new Error(`Build Refine or set REFINE_BIN before running website browser tests: ${error.message}`));
      });
      server.once("exit", code => {
        clearTimeout(timer);
        reject(new Error(`Website server exited (${code}): ${output}`));
      });
      server.stderr.on("data", chunk => {
        output += chunk;
        const match = output.match(/serving website at (http:\/\/127\.0\.0\.1:\d+)/);
        if (match) { clearTimeout(timer); resolve(match[1]); }
      });
    });
    browser = await BROWSER.chromium.launch({ executablePath: BROWSER.executablePath });
    const page = await browser.newPage();
    const pageErrors = [];
    page.on("pageerror", error => pageErrors.push(error.message));
    return { page, pageErrors, origin, close };
  } catch (error) {
    await close();
    throw error;
  }
}

module.exports = { openWebsite, SKIP };
