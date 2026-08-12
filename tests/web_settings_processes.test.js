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
      workflowPausedFor,
      workflowPauseActionModel,
    };
  `, context);
  return context.processSettingsTest;
}

test("workflow controls use one canonical pause gate", () => {
  const processes = processSettingsRuntime();

  assert.equal(processes.workflowPausedFor({ paused: true }), true);
  assert.equal(processes.workflowPausedFor({
    paused: false,
    workflow_paused: true,
    agents_paused: true,
    background_processes_stopped: true,
  }), true);
  assert.equal(processes.workflowPausedFor({
    paused: true,
    workflow_paused: false,
    agents_paused: true,
    background_processes_stopped: true,
  }), false);
  assert.equal(processes.workflowPausedFor({ workflow_paused: true }), true);
  assert.equal(processes.workflowPausedFor({ agents_paused: true }), true);
});

test("workflow pause action model locks descriptions, confirmation, actions, and payloads", () => {
  const processes = processSettingsRuntime();
  const pause = processes.workflowPauseActionModel({ workflow_paused: false });
  assert.equal(pause.shouldPause, true);
  assert.equal(pause.actionId, "pause_workflow");
  assert.equal(pause.direction, "pause");
  assert.equal(pause.status, "active");
  assert.equal(pause.buttonLabel, "Pause Workflow");
  assert.equal(pause.busyLabel, "Pausing…");
  assert.deepEqual(JSON.parse(JSON.stringify(pause.payload)), { paused: true });
  assert.equal(
    pause.description,
    "New Goal admission, automatic Git sync, and inactive-worktree cleanup are eligible. Active Goal executions continue unless stopped separately.",
  );
  assert.equal(
    pause.confirmation,
    "Pause workflow automation? Refine will stop admitting new Goal work and quiesce automatic Git sync and inactive-worktree cleanup at safe boundaries. Already active Goal executions continue unless you Stop their Agents separately.",
  );

  const resume = processes.workflowPauseActionModel({ workflow_paused: true });
  assert.equal(resume.shouldPause, false);
  assert.equal(resume.actionId, "unpause_workflow");
  assert.equal(resume.direction, "resume");
  assert.equal(resume.status, "paused");
  assert.equal(resume.buttonLabel, "Resume Workflow");
  assert.equal(resume.busyLabel, "Resuming…");
  assert.equal(resume.confirmation, null);
  assert.deepEqual(JSON.parse(JSON.stringify(resume.payload)), { paused: false });
  assert.equal(
    resume.description,
    "New Goal admission is paused; automatic Git sync and inactive-worktree cleanup quiesce at safe boundaries. Active Goal executions continue unless stopped separately.",
  );
});

test("workflow pause handler delegates the action model to the shared API and refreshes state", async () => {
  for (const paused of [false, true]) {
    const button = { dataset: { workflowPaused: String(paused) } };
    const calls = [];
    const busyLabels = [];
    let refreshed = 0;
    let scheduled = 0;
    let confirmation = null;
    const context = vm.createContext({
      htmlEscape: String,
      document: {
        querySelector() { return null; },
        getElementById() { return null; },
      },
      $$(selector) {
        return selector === "[data-toggle-workflow]" ? [button] : [];
      },
      bindOnce(element, event, handler) {
        if (element && event === "click") element.click = handler;
      },
      bindCommand() {},
      async modalConfirm(message) {
        confirmation = message;
        return true;
      },
      async withButtonBusy(_button, label, action) {
        busyLabels.push(label);
        await action();
      },
      async api(...args) { calls.push(args); },
      async refreshProcessesSettingsTab() { refreshed += 1; },
      refreshAgentStatusIndicator() {},
      scheduleProcessesTabRefreshes() { scheduled += 1; },
      async showActionError(error) { throw error; },
    });
    const source = fs.readFileSync(
      path.join(__dirname, "../src/surfaces/web/static/js/features/settings_processes.js"),
      "utf8",
    );
    vm.runInContext(source, context);
    context.refreshProcessesSettingsTab = async () => { refreshed += 1; };
    context.scheduleProcessesTabRefreshes = () => { scheduled += 1; };
    vm.runInContext("refreshTargetAppStatus = () => {}; bindSettingsProcessesTab({});", context);
    await button.click();

    assert.deepEqual(
      JSON.parse(JSON.stringify(calls)),
      [["POST", "/api/workflow/pause", { paused: !paused }]],
    );
    assert.deepEqual(busyLabels, [paused ? "Resuming…" : "Pausing…"]);
    assert.equal(refreshed, 1);
    assert.equal(scheduled, paused ? 1 : 0);
    assert.equal(
      confirmation,
      paused ? null : "Pause workflow automation? Refine will stop admitting new Goal work and quiesce automatic Git sync and inactive-worktree cleanup at safe boundaries. Already active Goal executions continue unless you Stop their Agents separately.",
    );
  }
});

test("Guide states the complete workflow pause and resume contract", () => {
  const guide = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/guide.js"),
    "utf8",
  );
  assert.match(guide, /Pause or resume workflow/);
  assert.match(guide, /Pausing blocks new Goal admission and quiesces automatic Git sync and inactive-worktree cleanup at safe boundaries\./);
  assert.match(guide, /Already active Goal executions continue unless stopped separately\./);
  assert.match(guide, /Resuming makes admission and those repository workers eligible again\./);
});

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
  assert.match(stopHandler, /stopped\?\.worktree_retention\?\.retained/);
  assert.match(stopHandler, /fresh follow-up Round/);
  assert.match(stopHandler, /stopped\?\.goal\?\.status === "cancelled"/);
  assert.match(stopHandler, /Explicit Goal cancellation remains terminal/);
  assert.match(stopHandler, /toast\(/);
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
