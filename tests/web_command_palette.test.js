const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function commandRuntime() {
  const openedToolbarTabs = [];
  const window = {};
  const context = vm.createContext({
    SETTINGS_SURFACES: {},
    SUPERVISOR_TAB_ID: "supervisor",
    SYSTEM_TAB_ID: "system",
    TERMINAL_TAB_ID: "terminal",
    chatState: { tabs: {} },
    console,
    location: { hash: "#/" },
    navigator: { platform: "Linux" },
    openToolbarTab: (tabId) => openedToolbarTabs.push(tabId),
    showActionError: async () => {},
    state: {
      currentRoute: "dashboard",
      lastReporter: "",
      project: { attached: true },
    },
    toast: () => {},
    window,
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(
    fs.readFileSync(path.join(staticRoot, "command-registry.js"), "utf8"),
    context,
  );
  vm.runInContext(
    fs.readFileSync(path.join(staticRoot, "commands.js"), "utf8"),
    context,
  );
  vm.runInContext(`
    globalThis.commandPaletteTest = {
      ids(query) {
        return searchCommands(query).map((item) => item.command.id);
      },
      run(id) {
        return runCommand(id, { skipConfirm: true });
      },
    };
  `, context);
  return {
    commands: context.commandPaletteTest,
    location: context.location,
    openedToolbarTabs,
  };
}

test("palette discovers every permanent Toolbar surface", () => {
  const browser = commandRuntime();

  assert.equal(browser.commands.ids("supervisor")[0], "supervisor.open");
  assert.equal(browser.commands.ids("system operations")[0], "system.open");
  assert.equal(browser.commands.ids("terminal")[0], "terminal.open");
  assert.equal(browser.commands.ids("standalone")[0], "standalone.open");
  assert.equal(browser.commands.ids("files")[0], "files.open");
});

test("Toolbar palette commands open the requested tab", async () => {
  const browser = commandRuntime();

  await browser.commands.run("supervisor.open");
  await browser.commands.run("system.open");
  await browser.commands.run("terminal.open");
  await browser.commands.run("standalone.open");

  assert.deepEqual(browser.openedToolbarTabs, [
    "supervisor",
    "system",
    "terminal",
    "standalone",
  ]);
});

test("palette includes the existing New Feature flow", async () => {
  const browser = commandRuntime();

  assert.equal(browser.commands.ids("new feature")[0], "feature.new");
  await browser.commands.run("feature.new");
  assert.equal(browser.location.hash, "#/features/new");
});
