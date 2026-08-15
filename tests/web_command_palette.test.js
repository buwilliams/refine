const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const { BrowserEvent, createBrowserDom } = require("./support/browser_dom");

function commandPaletteDomRuntime() {
  const dom = createBrowserDom(`
    <button id="btn-command-palette"><span data-command-shortcut></span></button>
    <section id="toolbar-dock"><div><input data-testid="nested-toolbar-target"></div></section>
    <button data-testid="outside-target">Outside</button>
  `);
  const querySelector = dom.document.querySelector.bind(dom.document);
  dom.document.getElementById = (id) => querySelector(`#${id}`);
  dom.document.querySelector = (selector) => {
    if (selector === ".modal-backdrop:not(.command-palette-backdrop)") {
      return dom.document.querySelectorAll(".modal-backdrop")
        .find((element) => !element.className.split(/\s+/).includes("command-palette-backdrop")) || null;
    }
    return querySelector(selector);
  };

  const context = vm.createContext({
    console,
    document: dom.document,
    htmlEscape: (value) => String(value ?? ""),
    navigator: { platform: "Linux" },
    runCommand: async () => {},
    searchCommands: () => [],
    commandShortcutLabel: () => "Ctrl+K",
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(
    fs.readFileSync(path.join(staticRoot, "command-palette.js"), "utf8"),
    context,
  );
  vm.runInContext("initCommandPalette();", context);

  return {
    document: dom.document,
    dispatchShortcut(target, modifiers = { ctrlKey: true }) {
      const event = new BrowserEvent("keydown", { key: "k", ...modifiers });
      event.target = target;
      dom.document.dispatchEvent(event);
      return event;
    },
  };
}

function commandRuntime() {
  const openedToolbarTabs = [];
  const window = {};
  const context = vm.createContext({
    SETTINGS_SURFACES: {},
    URLSearchParams,

    SYSTEM_TAB_ID: "system",
    TERMINAL_TAB_ID: "terminal",
    chatState: { tabs: {} },
    console,
    location: { hash: "#/" },
    navigator: { platform: "Linux" },
    createToolbarTab: (mode) => openedToolbarTabs.push(mode),
    showActionError: async () => {},
    state: {
      currentRoute: "dashboard",
      lastReporter: "",
      project: { attached: true },
      underlayHash: "#/",
    },
    toast: () => {},
    window,
  });
  const staticRoot = path.join(__dirname, "../src/surfaces/web/static/js");
  vm.runInContext(
    fs.readFileSync(path.join(staticRoot, "node-scope-navigation.js"), "utf8"),
    context,
  );
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
    state: context.state,
  };
}

test("palette discovers every lazy Toolbar surface", () => {
  const browser = commandRuntime();

  assert.equal(browser.commands.ids("agent")[0], "agent.open");
  assert.equal(browser.commands.ids("system operations")[0], "system.open");
  assert.equal(browser.commands.ids("terminal")[0], "terminal.open");
  assert.ok(browser.commands.ids("agent worktree").includes("agent-worktree.open"));
  assert.equal(browser.commands.ids("files")[0], "files.open");
  assert.equal(browser.commands.ids("todo list")[0], "todo.open");
});

test("Toolbar palette commands open the requested tab", async () => {
  const browser = commandRuntime();

  await browser.commands.run("agent.open");
  await browser.commands.run("system.open");
  await browser.commands.run("todo.open");
  await browser.commands.run("terminal.open");
  await browser.commands.run("agent-worktree.open");

  assert.deepEqual(browser.openedToolbarTabs, [
    "agent",
    "system",
    "todo",
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

test("Dashboard and Goals palette navigation carries shared node scope", async () => {
  const browser = commandRuntime();

  browser.location.hash = "#/";
  await browser.commands.run("nav.goals");
  assert.equal(browser.location.hash, "#/goals?node=current");
  await browser.commands.run("nav.dashboard");
  assert.equal(browser.location.hash, "#/");

  browser.location.hash = "#/goals?node=all";
  await browser.commands.run("nav.dashboard");
  assert.equal(browser.location.hash, "#/?node=all");
  await browser.commands.run("nav.goals");
  assert.equal(browser.location.hash, "#/goals?node=all");

  browser.state.currentRoute = "goals_detail";
  browser.state.underlayHash = "#/goals?status=review&node=current";
  browser.location.hash = "#/goals/GOAL1";
  await browser.commands.run("nav.dashboard");
  assert.equal(browser.location.hash, "#/");
});

test("palette shortcut yields to nested Toolbar targets", () => {
  for (const modifiers of [{ ctrlKey: true }, { metaKey: true }]) {
    const browser = commandPaletteDomRuntime();
    const target = browser.document.querySelector('[data-testid="nested-toolbar-target"]');

    const event = browser.dispatchShortcut(target, modifiers);

    assert.equal(event.defaultPrevented, false);
    assert.equal(event.propagationStopped, false);
    assert.equal(browser.document.querySelector('[data-testid="command-palette"]'), null);
  }
});

test("palette shortcut consumes an outside target and opens the palette", () => {
  const browser = commandPaletteDomRuntime();
  const target = browser.document.querySelector('[data-testid="outside-target"]');

  const event = browser.dispatchShortcut(target);

  assert.equal(event.defaultPrevented, true);
  assert.equal(event.propagationStopped, true);
  assert.ok(browser.document.querySelector('[data-testid="command-palette"]'));
});

test("an existing non-palette modal still suppresses the palette shortcut", () => {
  const browser = commandPaletteDomRuntime();
  const modal = browser.document.createElement("div");
  modal.className = "modal-backdrop";
  browser.document.body.appendChild(modal);
  const target = browser.document.querySelector('[data-testid="outside-target"]');

  const event = browser.dispatchShortcut(target);

  assert.equal(event.defaultPrevented, false);
  assert.equal(event.propagationStopped, false);
  assert.equal(browser.document.querySelector('[data-testid="command-palette"]'), null);
});
