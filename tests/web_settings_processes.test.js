const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

function processSettingsRuntime() {
  const context = vm.createContext({
    htmlEscape(value) {
      return String(value);
    },
  });
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
      renderActions: renderProcessActions,
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

test("every current agent provider process renders a process-specific Stop action", () => {
  const processes = processSettingsRuntime();
  const rows = [
    {
      id: "goal-agent",
      kind: "agent",
      goal_id: "GOAL-1",
      management_actions: ["stop_agent"],
    },
    {
      id: "unattached-agent",
      kind: "agent",
      actions: ["terminate", "kill"],
    },
    {
      id: "chat-session-chat-1",
      kind: "chat",
      session_id: "chat-1",
      management_actions: ["stop_agent"],
    },
    {
      id: "interactive-agent",
      kind: "interactive_session",
      provider: "codex",
      profile: "goal",
      goal_id: "GOAL-2",
      management_actions: ["stop_agent"],
    },
  ];

  for (const row of rows) {
    const actions = processes.renderActions(row);
    assert.match(actions, /data-testid="process-stop-agent"/);
    assert.match(actions, new RegExp(`data-stop-agent="${row.id}"`));
    assert.match(actions, />Stop<\/button>/);
    assert.doesNotMatch(actions, />Cancel<\/button>/);
  }
  assert.match(
    processes.renderActions(rows[0]),
    /data-stop-agent-goal="GOAL-1"/,
  );
});

test("agent Stop delegates to the shared process-control API route", () => {
  const source = fs.readFileSync(
    path.join(
      __dirname,
      "../src/surfaces/web/static/js/features/settings_processes.js",
    ),
    "utf8",
  );
  assert.match(
    source,
    /api\("POST", `\/api\/processes\/\$\{encodeURIComponent\(processId\)\}\/stop`/,
  );
  const stopHandler = source
    .split('$$("[data-stop-agent]")')[1]
    .split('$$("[data-cancel-agent]")')[0];
  assert.doesNotMatch(stopHandler, /\/api\/goals\//);
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
