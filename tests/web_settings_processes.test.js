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
    renderSettingsGuideLabel(value) { return value; },
    fmtTime(value) { return String(value); },
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
      renderTab: renderProcessesTab,
      buildRows: buildProcessManagerRows,
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
  assert.equal(resume.direction, "unpause");
  assert.equal(resume.status, "paused");
  assert.equal(resume.buttonLabel, "Unpause Workflow");
  assert.equal(resume.busyLabel, "Unpausing…");
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
    assert.deepEqual(busyLabels, [paused ? "Unpausing…" : "Pausing…"]);
    assert.equal(refreshed, 1);
    assert.equal(scheduled, paused ? 1 : 0);
    assert.equal(
      confirmation,
      paused ? null : "Pause workflow automation? Refine will stop admitting new Goal work and quiesce automatic Git sync and inactive-worktree cleanup at safe boundaries. Already active Goal executions continue unless you Stop their Agents separately.",
    );
  }
});

test("Guide states the complete workflow pause and unpause contract", () => {
  const guide = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/guide.js"),
    "utf8",
  );
  assert.match(guide, /Pause or unpause workflow/);
  assert.match(guide, /Pausing blocks new Goal admission and quiesces automatic Git sync and inactive-worktree cleanup at safe boundaries\./);
  assert.match(guide, /Already active Goal executions continue unless stopped separately\./);
  assert.match(guide, /Unpausing makes admission and those repository workers eligible again\./);
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

test("process manager is one six-column table with stable services and dynamic agents", () => {
  const processes = processSettingsRuntime();
  const data = {
    target_app: { state: "running", has_stop_action: true, has_build_action: true, has_status_checks: true },
    repository_disk_usage: {
      target_app: { bytes: 1024 * 1024, includes_git_worktrees: true },
      daemon: { bytes: 2 * 1024 * 1024, includes_git_worktrees: true },
    },
    background_workers: [
      { id: "background-worker-git-sync", kind: "background_worker", worker_kind: "git-sync", status: "running", pid: 42, process_id: "git-sync-42", management_actions: ["stop_background_worker"] },
      { id: "background-worker-workflow", kind: "background_worker", worker_kind: "workflow", status: "stopped", management_actions: ["start_background_worker", "pause_workflow"] },
      { id: "background-worker-worktree-cleanup", kind: "background_worker", worker_kind: "worktree-cleanup", status: "running", management_actions: ["stop_background_worker"] },
      { id: "background-worker-development-requests", kind: "background_worker", worker_kind: "development-requests", status: "stopped", management_actions: ["start_background_worker"] },
    ],
    processes: [
      { id: "daemon-1", kind: "daemon", status: "running", pid: 10, memory_used_bytes: 4096, processor_used_percent: 1.5 },
      { id: "git-sync-42", kind: "runner", worker_kind: "git-sync", status: "running", pid: 42 },
      { id: "agent-1", kind: "agent", goal_id: "GOAL-1", status: "running", pid: 50, management_actions: ["stop_agent"] },
    ],
  };
  const source = { source_update: { enabled: true, update_available: true, title: "Update Refine" } };
  const rows = processes.buildRows(data, source);
  assert.deepEqual(
    JSON.parse(JSON.stringify(rows.map((row) => row.kind))),
    ["target_app", "daemon", "background_worker", "background_worker", "background_worker", "background_worker", "agent"],
  );
  const html = processes.renderTab(data, source);
  assert.equal((html.match(/<table\b/g) || []).length, 1);
  for (const heading of ["Process name", "PID", "Memory used", "Processor used", "Details", "Actions"]) {
    assert.match(html, new RegExp(`<th>${heading}</th>`));
  }
  for (const name of ["Target app", "Refine daemon", "git-sync", "workflow", "worktree-cleanup", "development-requests", "Agent · GOAL-1"]) {
    assert.match(html, new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.match(html, /repository disk 1\.00 MiB \(includes \.git worktrees\)/);
  assert.match(html, /data-testid="process-daemon-update"/);
  assert.match(html, /data-testid="process-daemon-stop"/);
  assert.doesNotMatch(html, /data-supervisor-toggle/);
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
  assert.match(stopHandler, /signal: "kill"/);
  assert.doesNotMatch(stopHandler, /modalConfirm/);
  assert.match(stopHandler, /stopped\?\.worktrees_retained/);
  assert.match(stopHandler, /fresh follow-up Round/);
  assert.match(stopHandler, /stopped\?\.goal\?\.status === "cancelled"/);
  assert.match(stopHandler, /The Goal is now failed/);
  assert.match(stopHandler, /removeToolbarTabsForStoppedProcess/);
  assert.match(stopHandler, /toast\(/);
});

test("background and daemon controls call their process-manager lifecycle APIs", () => {
  const source = fs.readFileSync(
    path.join(__dirname, "../src/surfaces/web/static/js/features/settings_processes.js"),
    "utf8",
  );
  assert.match(source, /\/api\/processes\/background-workers\/\$\{encodeURIComponent\(workerKind\)\}\/\$\{action\}/);
  assert.match(source, /api\("POST", "\/api\/system\/source\/promote", \{\}\)/);
  assert.match(source, /api\("POST", "\/api\/system\/stop", \{\}\)/);
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
