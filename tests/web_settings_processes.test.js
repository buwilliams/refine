const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function processSettingsRuntime() {
  const context = vm.createContext({});
  const source = fs.readFileSync(
    path.join(
      __dirname,
      "../src/surfaces/web/static/js/features/settings_processes.js",
    ),
    "utf8",
  );
  vm.runInContext(source, context);
  vm.runInContext(`
    globalThis.processSettingsTest = {
      isAgent: isCurrentAgentProviderProcessRecord,
    };
  `, context);
  return context.processSettingsTest;
}

test("Agents includes background and foreground provider processes", () => {
  const processes = processSettingsRuntime();

  assert.equal(processes.isAgent({ kind: "agent", status: "running" }), true);
  assert.equal(processes.isAgent({ kind: "chat", status: "idle" }), true);
  assert.equal(processes.isAgent({
    kind: "interactive_session",
    provider: "codex",
    profile: "standalone",
    status: "running",
  }), true);
});

test("Agents excludes terminals and completed provider processes", () => {
  const processes = processSettingsRuntime();

  assert.equal(processes.isAgent({
    kind: "interactive_session",
    profile: "terminal",
    status: "running",
  }), false);
  assert.equal(processes.isAgent({
    kind: "agent",
    status: "completed",
  }), false);
});
