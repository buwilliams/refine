// ---- System / Processes -----------------------------------------------------

let supervisorProcessExpanded = false;

function renderProcessesTab(processData, settings, diag, dash) {
  const backgroundStopped = typeof processData.background_processes_stopped === "boolean"
    ? processData.background_processes_stopped
    : settings.paused === "1";
  const agentsPaused = typeof processData.agents_paused === "boolean"
    ? processData.agents_paused
    : settings.agents_paused === "1";
  const backend = processData.backend || diag.backend || dash.backend || {};
  const processes = (processData.processes || []).map(normalizeManagedProcess);
  const runnerWork = processData.runner_work || [];
  const runnerReachable = typeof processData.runner_reachable === "boolean"
    ? processData.runner_reachable
    : !!diag.reachable;
  const anchorMs = Date.now();
  const rows = buildManagedProcessRows(
    processes, { backgroundStopped, agentsPaused }, backend, runnerReachable, diag,
  ).map((proc) => renderManagedProcessRow(proc)).join("");
  const agentRows = (processes || [])
    .filter(isCurrentAgentProviderProcessRecord)
    .map((proc) => renderAgentProcessRow(proc, anchorMs)).join("");
  const subprocessRows = (processes || [])
    .filter(isCurrentSubprocessRecord)
    .map((proc) => renderSubprocessProcessRow(proc, anchorMs)).join("");
  const workRows = runnerWork.map((work) => renderRunnerWorkRow(work, anchorMs)).join("");
  const subprocessBody = [subprocessRows, workRows].filter(Boolean).join("");
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Process management", "process-management")}</h3>
      ${rows ? `
        <table class="table process-table managed-process-table mobile-card-table" data-testid="managed-process-table">
          <colgroup>
            <col class="process-col">
            <col class="status-col">
            <col class="pid-col">
            <col class="cpu-col">
            <col class="memory-col">
            <col class="details-col">
            <col class="actions-col">
          </colgroup>
          <thead><tr>
            <th>Process</th><th>Status</th><th>PID</th>
            <th>CPU priority</th><th>Max memory</th><th>Details</th><th></th>
          </tr></thead>
          <tbody>${rows}</tbody>
        </table>` : `<p class="muted">No managed processes found.</p>`}
    </section>

    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Agents", "process-agent-processes")}</h3>
      ${agentRows ? `
        <table class="table process-table agents-process-table mobile-card-table" data-testid="agent-process-table">
          <colgroup>
            <col class="agent-col">
            <col class="status-col">
            <col class="pid-col">
            <col class="round-col">
            <col class="cpu-col">
            <col class="memory-col">
            <col class="elapsed-col">
            <col class="idle-col">
            <col class="agent-actions-col">
          </colgroup>
          <thead><tr>
            <th>Agent</th><th>Status</th><th>PID</th><th>Context</th>
            <th>CPU priority</th><th>Max memory</th><th>Elapsed</th><th>Idle</th><th></th>
          </tr></thead>
          <tbody>${agentRows}</tbody>
        </table>` : `<p class="muted">No agent provider calls running.</p>`}
    </section>

    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Subprocesses", "process-runner-processes")}</h3>
      ${subprocessBody ? `
        <table class="table process-table runner-workers-table mobile-card-table" data-testid="subprocess-table">
          <colgroup>
            <col class="worker-col">
            <col class="status-col">
            <col class="pid-col">
            <col class="cpu-col">
            <col class="memory-col">
            <col class="elapsed-col">
            <col class="details-col">
            <col class="worker-actions-col">
          </colgroup>
          <thead><tr>
            <th>Subprocess</th><th>Status</th><th>PID</th>
            <th>CPU priority</th><th>Max memory</th><th>Elapsed</th><th>Details</th><th></th>
          </tr></thead>
          <tbody>${subprocessBody}</tbody>
        </table>` : `<p class="muted">No subprocesses recorded.</p>`}
      <div id="sqlite-cache-progress" style="display:none;margin-top:12px"></div>
    </section>`;
}

function buildManagedProcessRows(processes, pauseState, backend, runnerReachable, diag) {
  const backgroundStopped = !!pauseState.backgroundStopped;
  const agentsPaused = !!pauseState.agentsPaused;
  const rows = (processes || [])
    .filter(isCurrentLongLivedManagedProcess)
    .map((proc) => {
      if (proc.kind !== "runner") return proc;
      return {
        ...proc,
        details: runnerProcessDetails(backend, runnerReachable, diag),
      };
    });
  const supervised = backend.process_model === "supervisor";
  const supervisorOwnsWorkflowToggle = supervised && rows.some((proc) => proc.kind === "supervisor");
  if (!rows.some((proc) => proc.kind === "target_app")) {
    rows.push(syntheticTargetAppProcess());
  }
  const workflowPaused = backgroundStopped || agentsPaused;
  const workflowAutomation = {
    id: "workflow-automation",
    kind: "workflow_automation",
    label: "Workflow automation",
    status: backgroundStopped || agentsPaused ? "paused" : "active",
    runner_reachable: runnerReachable,
    pid: null,
    details: workflowPaused
      ? "Workflow automation is paused; Goal agents, QA, and builds wait."
      : "Workflow automation can run Goal agents, QA, and builds.",
    agents_paused: agentsPaused,
    background_processes_stopped: backgroundStopped,
    actions: workflowAutomationActionIds(workflowPaused, supervisorOwnsWorkflowToggle),
    cpu_priority: { label: "-" },
    max_memory: { label: "-" },
  };
  return orderManagedProcessRows(rows, workflowAutomation, supervised);
}

function normalizeManagedProcess(proc = {}) {
  const label = String(proc.label || "");
  if (proc.kind === "daemon") {
    return { ...proc, kind: "supervisor", label: "Supervisor" };
  }
  if (label === "setsid" && (!proc.kind || proc.kind === "process")) {
    return { ...proc, kind: "supervisor", label: "Supervisor" };
  }
  return proc;
}

function isLongLivedManagedProcess(proc = {}) {
  return new Set(["supervisor", "ui", "runner", "target_app"]).has(proc.kind);
}

function isCurrentLongLivedManagedProcess(proc = {}) {
  if (!isLongLivedManagedProcess(proc)) return false;
  return isCurrentProcessStatus(proc.status);
}

function syntheticTargetAppProcess() {
  const snap = {
    state: "unknown",
    has_start_command: false,
    has_stop_command: false,
    has_build_command: false,
    has_start_instructions: false,
    has_stop_instructions: false,
    has_build_instructions: false,
    has_start_action: false,
    has_stop_action: false,
    has_build_action: false,
    has_status_checks: false,
  };
  return {
    id: "target-app",
    kind: "target_app",
    label: "Application",
    status: snap.state,
    pid: null,
    target_app: snap,
    details: targetAppProcessDetails(snap),
    actions: [],
    cpu_priority: { label: "-" },
    max_memory: { label: "-" },
  };
}

function isSubprocessRecord(proc = {}) {
  return !isLongLivedManagedProcess(proc) && !isAgentProviderProcessRecord(proc);
}

function isCurrentSubprocessRecord(proc = {}) {
  return isSubprocessRecord(proc) && isCurrentProcessStatus(proc.status);
}

function isAgentProviderProcessRecord(proc = {}) {
  return new Set(["agent", "chat"]).has(proc.kind)
    || (proc.kind === "interactive_session" && !!proc.provider);
}

function isCurrentAgentProviderProcessRecord(proc = {}) {
  return isAgentProviderProcessRecord(proc) && isCurrentProcessStatus(proc.status);
}

function isCurrentProcessStatus(status = "") {
  return !new Set(["exited", "failed", "stopped", "cancelled", "complete", "completed"]).has(status);
}

function orderManagedProcessRows(rows, workflowAutomation, supervised) {
  const targetApp = rows.find((proc) => proc.kind === "target_app");
  const targetAppId = targetApp ? targetApp.id : null;
  if (!supervised) {
    return [
      ...rows.filter((proc) => !targetApp || proc.id !== targetAppId),
      workflowAutomation,
      ...(targetApp ? [targetApp] : []),
    ];
  }
  const supervisor = rows.find((proc) => proc.kind === "supervisor");
  if (!supervisor) {
    return [
      ...rows.filter((proc) => !targetApp || proc.id !== targetAppId),
      workflowAutomation,
      ...(targetApp ? [targetApp] : []),
    ];
  }
  const childKinds = new Set(["ui", "runner"]);
  const children = [
    ...rows.filter((proc) => childKinds.has(proc.kind)),
    workflowAutomation,
  ].map((proc) => ({
    ...proc,
    supervisor_child: true,
    supervisor_child_hidden: !supervisorProcessExpanded,
  }));
  const childIds = new Set(children.map((proc) => proc.id));
  const remaining = rows.filter((proc) => (
    proc.id !== supervisor.id
    && (!targetApp || proc.id !== targetAppId)
    && !childIds.has(proc.id)
  ));
  return [
    {
      ...supervisor,
      agents_paused: workflowAutomation.agents_paused,
      background_processes_stopped: workflowAutomation.background_processes_stopped,
      runner_reachable: workflowAutomation.runner_reachable,
      supervisor_parent: true,
      supervisor_expanded: supervisorProcessExpanded,
      supervisor_child_count: children.length,
    },
    ...children,
    ...remaining,
    ...(targetApp ? [targetApp] : []),
  ];
}

function runnerProcessDetails(backend = {}, runnerReachable, diag = {}) {
  const bits = [
    backendProcessLabel(backend),
    backendTransportLabel(backend),
    runnerReachable ? "runner reachable" : "runner unreachable",
  ];
  if (diag.mode) bits.push(`mode ${diag.mode}`);
  if (diag.last_call_at) bits.push(`last call ${fmtTime(diag.last_call_at)}`);
  if (backend.socket_path) bits.push(`socket ${shortPath(backend.socket_path)}`);
  if (diag.error?.message) bits.push(`error ${diag.error.message}`);
  return bits.filter(Boolean).join(" · ");
}

function renderManagedProcessRow(proc) {
  const kind = proc.kind || "process";
  const pid = proc.pid ? htmlEscape(String(proc.pid)) : `<span class="muted small">-</span>`;
  const rawLabel = proc.session_id
    ? `<code>${htmlEscape(proc.session_id)}</code>`
    : htmlEscape(proc.label || processKindLabel(kind));
  const label = renderManagedProcessLabel(proc, rawLabel);
  const details = managedProcessDetails(proc);
  const detailsAttrs = details
    ? ` class="process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Process details" title="${htmlEscape(details)}"`
    : "";
  const rowClasses = [
    proc.supervisor_parent ? "supervisor-parent" : "",
    proc.supervisor_child ? "supervisor-child" : "",
    proc.supervisor_child_hidden ? "supervisor-child-hidden" : "",
  ].filter(Boolean).join(" ");
  const rowClassAttr = rowClasses ? ` class="${htmlEscape(rowClasses)}"` : "";
  const supervisorChildAttr = proc.supervisor_child ? ` data-supervisor-child="1"` : "";
  const hiddenAttr = proc.supervisor_child_hidden ? " hidden" : "";
  return `
    <tr${rowClassAttr} data-testid="managed-process-row" data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}"${supervisorChildAttr}${hiddenAttr}>
      <td data-label="Process">${label}</td>
      <td data-label="Status" data-testid="managed-process-status" data-process-status>${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td data-label="PID">${pid}</td>
      <td data-label="CPU priority">${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td data-label="Max memory">${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td data-label="Details" data-testid="managed-process-details" data-process-details${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
}

function renderManagedProcessLabel(proc, rawLabel) {
  if (proc.supervisor_parent && Number(proc.supervisor_child_count || 0) > 0) {
    const expanded = !!proc.supervisor_expanded;
    return `
      <span class="process-tree-label">
        <button type="button" class="process-tree-toggle" data-supervisor-toggle
                data-testid="process-supervisor-toggle"
                aria-expanded="${expanded ? "true" : "false"}"
                aria-label="${expanded ? "Collapse supervisor processes" : "Expand supervisor processes"}"
                title="${expanded ? "Collapse supervisor processes" : "Expand supervisor processes"}">
          <span aria-hidden="true">${expanded ? "▾" : "▸"}</span>
        </button>
        <span>${rawLabel}</span>
      </span>`;
  }
  if (proc.supervisor_child) {
    return `
      <span class="process-tree-label supervisor-child-label">
        <span class="process-tree-spacer" aria-hidden="true"></span>
        <span>${rawLabel}</span>
      </span>`;
  }
  return rawLabel;
}

function renderAgentProcessRow(proc, anchorMs) {
  const kind = proc.kind || "agent";
  const interactive = kind === "interactive_session";
  const pid = proc.pid ? htmlEscape(String(proc.pid)) : `<span class="muted small">-</span>`;
  const elapsed = Number.isFinite(Number(proc.elapsed_seconds))
    ? `<span class="js-elapsed-tick" data-base="${Number(proc.elapsed_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(proc.elapsed_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const idle = Number.isFinite(Number(proc.idle_seconds))
    ? `<span class="js-idle-tick" data-base="${Number(proc.idle_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(proc.idle_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const label = kind === "chat"
    ? `${htmlEscape(proc.mode === "goal" ? "Goal agent session" : proc.mode === "plan" ? "Plan chat" : "Standalone chat")}<br><code>${htmlEscape(proc.session_id || "")}</code>`
    : proc.goal_id
    ? `<a href="#/goals/${htmlEscape(proc.goal_id)}">${htmlEscape(proc.goal_id.slice(0, 10))}...</a>`
    : htmlEscape(proc.label || "Agent");
  const context = kind === "chat"
    ? proc.goal_id
      ? `<a href="#/goals/${htmlEscape(proc.goal_id)}">${htmlEscape(proc.goal_id.slice(0, 10))}...</a>`
      : "standalone"
    : interactive
    ? proc.goal_id
      ? `<a href="#/goals/${htmlEscape(proc.goal_id)}">${htmlEscape(proc.goal_id.slice(0, 10))}...</a>`
      : htmlEscape(proc.profile || proc.role || "interactive")
    : proc.round_idx != null
    ? String(Number(proc.round_idx) + 1)
    : "";
  return `
    <tr data-testid="agent-process-row" data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}">
      <td data-label="Agent">${label}</td>
      <td data-label="Status">${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td data-label="PID">${pid}</td>
      <td data-label="Context">${context ? context : `<span class="muted small">-</span>`}</td>
      <td data-label="CPU priority">${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td data-label="Max memory">${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td data-label="Elapsed">${elapsed}</td>
      <td data-label="Idle">${idle}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
}

function renderSubprocessProcessRow(proc, anchorMs) {
  const kind = proc.kind || "process";
  const pid = proc.pid ? htmlEscape(String(proc.pid)) : `<span class="muted small">-</span>`;
  const elapsed = Number.isFinite(Number(proc.elapsed_seconds))
    ? `<span class="js-elapsed-tick" data-base="${Number(proc.elapsed_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(proc.elapsed_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const label = kind === "chat"
    ? `${htmlEscape(proc.mode === "goal" ? "Goal agent session" : proc.mode === "plan" ? "Plan chat" : "Standalone chat")}<br><code>${htmlEscape(proc.session_id || "")}</code>`
    : kind === "agent" && proc.goal_id
      ? `<a href="#/goals/${htmlEscape(proc.goal_id)}">${htmlEscape(proc.goal_id.slice(0, 10))}...</a>`
      : htmlEscape(proc.label || processKindLabel(kind));
  const details = [
    proc.goal_id ? `Goal ${proc.goal_id}` : "",
    proc.round_idx != null ? `round ${Number(proc.round_idx) + 1}` : "",
    managedProcessDetails(proc),
  ].filter(Boolean).join(" · ");
  const detailsAttrs = details
    ? ` class="process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Subprocess details" title="${htmlEscape(details)}"`
    : "";
  return `
    <tr data-testid="subprocess-row" data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}">
      <td data-label="Subprocess">${label}</td>
      <td data-label="Status">${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td data-label="PID">${pid}</td>
      <td data-label="CPU priority">${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td data-label="Max memory">${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td data-label="Elapsed">${elapsed}</td>
      <td data-label="Details" data-process-details${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
}

function managedProcessDetails(proc) {
  if (proc.details) return proc.details;
  if (proc.kind === "target_app") return targetAppProcessDetails(proc.target_app || {});
  if (proc.kind === "chat") {
    return [proc.provider, proc.mode].filter(Boolean).join(" · ");
  }
  return "";
}

function processResourceLabel(resource) {
  if (!resource) return "-";
  if (typeof resource === "string") return resource;
  return resource.label || "-";
}

function targetAppProcessDetails(snap = {}) {
  const bits = [];
  if (snap.has_status_checks) {
    const checkAt = snap.last_check_at || snap.last_health_at || "";
    const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
    bits.push(checkAt
      ? `last status check ${checkOk ? "OK" : "FAIL"} ${fmtTime(checkAt)}`
      : "status checks configured");
  } else {
    bits.push("no status checks configured");
  }
  if (snap.last_operation?.kind) {
    bits.push(`last operation ${snap.last_operation.kind} ${snap.last_operation.state || ""}`.trim());
  }
  if (snap.last_error) bits.push(`last error ${snap.last_error}`);
  if (snap.legacy_config_present) bits.push("legacy target-app settings detected");
  return bits.join(" · ");
}

function processKindLabel(kind) {
  return {
    ui: "UI",
    supervisor: "Supervisor",
    daemon: "Supervisor",
    runner: "Runner",
    target_app: "Application",
    workflow_automation: "workflow automation",
    agent_automation: "workflow automation",
    background_processes: "workflow automation",
    agent: "Agent",
    chat: "Chat",
    quality: "Quality check",
    import: "Import",
    maintenance: "Maintenance",
    user_helper: "Helper",
  }[kind] || "Process";
}

function processStatusLabel(status) {
  return {
    running: "running",
    unreachable: "unreachable",
    reviewing: "reviewing",
    merging: "merging",
    queued: "queued",
    degraded: "degraded",
    starting: "starting",
    building: "building",
    stopping: "stopping",
    stopped: "stopped",
    failed: "failed",
    unknown: "unknown",
    active: "active",
    paused: "paused",
    idle: "idle",
    exited: "exited",
    interrupted: "interrupted",
  }[status] || status || "unknown";
}

function workflowAutomationActionIds(workflowPaused, supervisorOwnsWorkflowToggle) {
  const actions = ["hard_reset_worktree"];
  if (!supervisorOwnsWorkflowToggle) {
    actions.unshift(workflowPaused ? "unpause_workflow" : "pause_workflow");
  }
  return actions;
}

function processActionIds(proc) {
  if (Array.isArray(proc.management_actions)) return proc.management_actions;
  if (Array.isArray(proc.actions)) {
    const supported = proc.actions.filter((actionId) => isSupportedProcessActionId(proc, actionId));
    if (supported.length) return supported;
  }
  if (proc.kind === "supervisor") {
    const workflowPaused = !!proc.background_processes_stopped || !!proc.agents_paused;
    return [workflowPaused ? "unpause_workflow" : "pause_workflow"];
  }
  if (proc.kind === "workflow_automation" || proc.kind === "agent_automation") {
    const workflowPaused = !!proc.background_processes_stopped || !!proc.agents_paused;
    return [workflowPaused ? "unpause_workflow" : "pause_workflow", "hard_reset_worktree"];
  }
  if (proc.kind === "background_processes") {
    const stopped = !!proc.background_processes_stopped;
    return [stopped ? "unpause_workflow" : "pause_workflow", "hard_reset_worktree"];
  }
  return null;
}

function isSupportedProcessActionId(proc, actionId) {
  if (["pause_workflow", "unpause_workflow", "hard_reset_worktree"].includes(actionId)) return true;
  if (actionId === "cancel_agent") return proc.kind === "agent" && !!proc.goal_id;
  if (actionId === "stop_chat" || actionId === "stop") return proc.kind === "chat" && !!proc.session_id;
  return false;
}

function renderProcessActions(proc) {
  const actionIds = processActionIds(proc);
  if (actionIds) return renderProcessActionButtons(proc, actionIds);
  if (proc.kind === "agent" && proc.goal_id) {
    return `<button class="danger" data-testid="process-cancel-agent" data-cancel-agent="${htmlEscape(proc.goal_id)}">Cancel</button>`;
  }
  if (proc.kind === "chat" && proc.session_id) {
    return `<button class="danger" data-testid="process-stop-chat" data-stop-chat="${htmlEscape(proc.session_id)}">Stop</button>`;
  }
  if (proc.kind === "target_app") {
    const snap = proc.target_app || {};
    const inFlight = ["starting", "stopping", "building"].includes(snap.state);
    const isRunning = snap.state === "running" || snap.state === "degraded";
    const isStopped = snap.state === "stopped" || snap.state === "unknown" || snap.state === "failed";
    const showStop = targetAppShowsStopAction(snap.state);
    const hasStartAction = snap.has_start_action ?? snap.has_start_instructions ?? snap.has_start_command;
    const hasStopAction = snap.has_stop_action ?? snap.has_stop_instructions ?? snap.has_stop_command;
    return `
      <span class="target-app-action-slot">
        <button id="s-target-run-start" data-testid="process-target-app-start" class="${showStop ? "target-app-action-hidden" : ""}" ${showStop || isRunning || inFlight || !hasStartAction ? "disabled" : ""} ${showStop ? `aria-hidden="true" tabindex="-1"` : ""}>Start</button>
        <button class="danger ${showStop ? "" : "target-app-action-hidden"}" id="s-target-run-stop" data-testid="process-target-app-stop" ${!showStop || isStopped || inFlight || !hasStopAction ? "disabled" : ""} ${showStop ? "" : `aria-hidden="true" tabindex="-1"`}>Stop</button>
      </span>
      <button class="secondary" id="s-target-run-build" data-testid="process-target-app-build" ${inFlight ? "disabled" : ""}>Build</button>
      <button class="secondary" id="s-target-health-now" data-testid="process-target-app-health">Check</button>`;
  }
  return `<span class="muted small">-</span>`;
}

function renderProcessActionButtons(proc, actionIds) {
  const buttons = actionIds
    .map((actionId) => renderProcessActionButton(proc, actionId))
    .filter(Boolean)
    .join("\n");
  return buttons || `<span class="muted small">-</span>`;
}

function renderProcessActionButton(proc, actionId) {
  if (actionId === "pause_workflow" || actionId === "unpause_workflow") {
    const paused = actionId === "unpause_workflow";
    const disabled = workflowToggleDisabled(proc);
    return `<button class="${paused ? "" : "secondary"}" data-testid="process-workflow-toggle" data-toggle-workflow="${paused ? "unpause" : "pause"}" ${disabled ? "disabled" : ""}>${paused ? "Unpause Workflow" : "Pause Workflow"}</button>`;
  }
  if (actionId === "hard_reset_worktree") {
    const disabled = !proc.runner_reachable || hardResetWorktreeDisabled(proc);
    return `<button class="danger" data-testid="process-hard-reset-worktree" data-hard-reset-worktree ${disabled ? "disabled" : ""}>Hard reset worktree</button>`;
  }
  if (actionId === "cancel_agent" && proc.kind === "agent" && proc.goal_id) {
    return `<button class="danger" data-testid="process-cancel-agent" data-cancel-agent="${htmlEscape(proc.goal_id)}">Cancel</button>`;
  }
  if ((actionId === "stop_chat" || actionId === "stop") && proc.kind === "chat" && proc.session_id) {
    return `<button class="danger" data-testid="process-stop-chat" data-stop-chat="${htmlEscape(proc.session_id)}">Stop</button>`;
  }
  return "";
}

function workflowToggleDisabled(proc) {
  return (proc.kind === "workflow_automation" || proc.kind === "agent_automation") && !proc.runner_reachable;
}

function hardResetWorktreeDisabled(proc) {
  if (proc.kind === "background_processes") return proc.status === "paused";
  return !!proc.background_processes_stopped || !!proc.agents_paused;
}

function targetAppShowsStopAction(state) {
  return ["running", "degraded", "stopping", "building"].includes(state);
}

function setTargetAppActionVisible(button, visible) {
  button.classList.toggle("target-app-action-hidden", !visible);
  if (visible) {
    button.removeAttribute("aria-hidden");
    button.removeAttribute("tabindex");
  } else {
    button.setAttribute("aria-hidden", "true");
    button.tabIndex = -1;
  }
}

function setSupervisorProcessExpanded(expanded) {
  supervisorProcessExpanded = !!expanded;
  const button = document.querySelector("[data-supervisor-toggle]");
  if (button) {
    button.setAttribute("aria-expanded", supervisorProcessExpanded ? "true" : "false");
    button.setAttribute(
      "aria-label",
      supervisorProcessExpanded ? "Collapse supervisor processes" : "Expand supervisor processes",
    );
    button.title = supervisorProcessExpanded
      ? "Collapse supervisor processes"
      : "Expand supervisor processes";
    const icon = button.querySelector("span");
    if (icon) icon.textContent = supervisorProcessExpanded ? "▾" : "▸";
  }
  $$("[data-supervisor-child]").forEach((row) => {
    row.hidden = !supervisorProcessExpanded;
    row.classList.toggle("supervisor-child-hidden", !supervisorProcessExpanded);
  });
}

function bindSupervisorProcessToggle() {
  document.querySelector("[data-supervisor-toggle]")?.addEventListener("click", () => {
    setSupervisorProcessExpanded(!supervisorProcessExpanded);
  });
}

function renderRunnerWorkRow(work, anchorMs) {
  const elapsed = Number.isFinite(Number(work.elapsed_seconds))
    ? `<span class="js-elapsed-tick" data-base="${Number(work.elapsed_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(work.elapsed_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const queued = Number(work.queued || 0);
  const details = [
    work.goal_id ? `Goal ${work.goal_id}` : "",
    queued ? `queue ${fmtCount(queued)}` : "",
    work.details || work.last_outcome || "",
  ].filter(Boolean).join(" · ");
  const detailsAttrs = details
    ? ` class="runner-work-details process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Runner worker details" title="${htmlEscape(details)}"`
    : ` class="runner-work-details"`;
  return `
    <tr data-testid="runner-work-row" data-runner-work-kind="${htmlEscape(work.kind || "")}">
      <td data-label="Worker">${htmlEscape(runnerWorkKindLabel(work.kind))}</td>
      <td data-label="Status">${htmlEscape(processStatusLabel(work.status || ""))}</td>
      <td data-label="PID"><span class="muted small">-</span></td>
      <td data-label="CPU priority"><span class="muted small">-</span></td>
      <td data-label="Max memory"><span class="muted small">-</span></td>
      <td data-label="Elapsed">${elapsed}</td>
      <td data-label="Details"${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderRunnerWorkActions(work)}</div></td>
    </tr>`;
}

function renderRunnerWorkActions(work) {
  if (work.kind === "target_app_builder") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="secondary" data-testid="runner-target-app-build" data-runner-target-app-build ${busy ? "disabled" : ""}>Build</button>`;
  }
  if (work.kind === "target_app_config_generator") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="secondary" data-testid="runner-target-app-generate" data-runner-target-app-generate ${busy ? "disabled" : ""}>Generate</button>`;
  }
  if (work.kind === "sqlite_cache_rebuild") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="danger" data-testid="runner-cache-rebuild" data-runner-cache-rebuild ${busy ? "disabled" : ""}>Rebuild</button>`;
  }
  if (work.kind === "activity_log_cleanup") {
    const paused = work.status === "paused";
    return `
      <select data-testid="runner-log-cleanup-days" data-runner-log-cleanup-days aria-label="Activity log retention" ${paused ? "disabled" : ""}>
        ${[0, 7, 30, 60, 90, 365].map((n) =>
          `<option value="${n}" ${n === 7 ? "selected" : ""}>${n === 0 ? "0 days" : `${n} days`}</option>`).join("")}
      </select>
      <button class="danger" data-testid="runner-log-cleanup" data-runner-log-cleanup ${paused ? "disabled" : ""}>Clean up</button>`;
  }
  return `<span class="muted small">-</span>`;
}

function runnerWorkKindLabel(kind) {
  return {
    merger: "merger",
    governance: "governance",
    plan_draft_extractor: "Plan Draft extractor",
    target_app_builder: "target-app builder",
    target_app_config_generator: "target-app config generator",
    sqlite_cache_rebuild: "projection cache rebuilder",
    activity_log_cleanup: "activity log cleanup",
    import_prepare: "import preparer",
    import_persist: "import persister",
    bulk_update_goals: "bulk Goal updater",
    bulk_delete_goals: "bulk Goal deleter",
  }[kind] || "worker";
}

function bindProcessDetailCells() {
  updateProcessDetailAffordances();
  $$(".process-details-cell").forEach((cell) => {
    cell.addEventListener("click", () => openProcessDetailsIfOverflowing(cell));
    cell.addEventListener("keydown", (ev) => {
      if (ev.key !== "Enter" && ev.key !== " ") return;
      if (!cell.classList.contains("is-overflowing")) return;
      ev.preventDefault();
      openProcessDetailsIfOverflowing(cell);
    });
  });
}

function updateProcessDetailAffordances() {
  $$(".process-details-cell").forEach((cell) => {
    const overflow = !!cell.dataset.fullDetails
      && cell.scrollWidth > cell.clientWidth + 1;
    cell.classList.toggle("is-overflowing", overflow);
    if (overflow) {
      cell.tabIndex = 0;
      cell.setAttribute("role", "button");
      cell.setAttribute("aria-label", "View full details");
      cell.title = "Click to view full details";
    } else {
      cell.removeAttribute("tabindex");
      cell.removeAttribute("role");
      cell.removeAttribute("aria-label");
      cell.title = cell.dataset.fullDetails || "";
    }
  });
}

async function openProcessDetailsIfOverflowing(cell) {
  if (!cell.classList.contains("is-overflowing")) return;
  const details = cell.dataset.fullDetails || "";
  if (!details) return;
  await modalAlert(details, {
    title: cell.dataset.detailTitle || "Details",
    okLabel: "Close",
  });
}

function backendProcessLabel(backend = {}) {
  if (backend.process_model === "supervisor") return "Supervisor: UI + worker process";
  return "Unknown";
}

function backendTransportLabel(backend = {}) {
  if (backend.transport === "unix_socket") return "Unix socket";
  return "Unknown";
}

function shortPath(path) {
  const text = String(path || "");
  return text.split(/[\\/]/).pop() || text;
}


async function refreshTargetAppStatus() {
  const block = document.getElementById("target-app-status-block");
  const hasControls = document.getElementById("s-target-run-start")
    || document.getElementById("s-target-run-build")
    || document.getElementById("s-target-run-stop");
  if (!block && !hasControls) return;
  try {
    const r = await api("GET", "/api/target-app/status");
    drawTargetAppStatusBlock(r);
  } catch (e) {
    if (block) {
      block.innerHTML = `<span class="muted">Status unavailable: ${htmlEscape(e.message)}</span>`;
    }
  }
}

function drawTargetAppStatusBlock(snap) {
  const stateLabel = {
    running:  "Running",
    degraded: "Degraded",
    starting: "Starting…",
    building: "Building…",
    stopping: "Stopping…",
    stopped:  "Stopped",
    failed:   "Failed",
    unknown:  "Unknown",
  }[snap.state] || snap.state || "Unknown";
  const checkAt = snap.last_check_at || snap.last_health_at || "";
  const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
  const checkMessage = snap.last_check_message || snap.last_health_message || "";
  const healthBits = checkAt
    ? `Last status check: ${checkOk ? "OK" : "FAIL"} · ${fmtTime(checkAt)}`
    : "No status checks yet.";
  const healthDetail = checkMessage && !checkOk
    ? `<p class="muted small" style="margin-top:6px;color:var(--error)">Check: ${htmlEscape(checkMessage)}</p>`
    : "";
  const op = snap.last_operation
    ? `<p class="muted small" style="margin-top:6px">Last operation: ${htmlEscape(snap.last_operation.kind)} → ${htmlEscape(snap.last_operation.state)} · ${fmtTime(snap.last_operation.finished_at)}</p>`
    : "";
  const autoBuildMode = snap.auto_build === "nightly" ? "daily" : snap.auto_build;
  const autoBuildLabel = {
    never: "Never",
    on_worktree_merge: "On worktree merge",
    hourly: "Hourly",
    daily: `Daily (${String(snap.auto_build_hour_utc || "0").padStart(2, "0")}:00 UTC)`,
  }[autoBuildMode || "never"] || "Never";
  const autoBuild = `<p class="muted small" style="margin-top:6px">Automatic build: ${htmlEscape(autoBuildLabel)}${
    snap.auto_build_last_finished_at
      ? ` · last ${snap.auto_build_last_ok ? "OK" : "failed"} at ${fmtTime(snap.auto_build_last_finished_at)}`
      : ""
  }</p>`;
  const block = document.getElementById("target-app-status-block");
  if (block) {
    block.innerHTML = `
      <div style="display:flex;align-items:center;gap:10px">
        <span class="target-app-dot" data-status-dot></span>
        <strong>${htmlEscape(stateLabel)}</strong>
        ${snap.has_status_checks ? `<span class="muted small">status checks configured</span>` : `<span class="muted small">No status checks configured</span>`}
      </div>
      <p class="muted small" style="margin:8px 0 0">${htmlEscape(healthBits)}</p>
      ${healthDetail}
      ${op}
      ${autoBuild}
      ${snap.last_error ? `<p class="muted small" style="margin-top:6px;color:var(--error)">Last error: ${htmlEscape(snap.last_error)}</p>` : ""}
      ${snap.legacy_config_present ? `<p class="muted small" style="margin-top:6px;color:var(--warn)">Legacy target-app settings detected.</p>` : ""}
    `;
    // Apply dot colour from the parent state via a CSS hook — the .target-app-dot
    // colour rules key off `data-state` on an ancestor, so set it here too.
    const dot = block.querySelector("[data-status-dot]");
    if (dot) {
      dot.style.background = ({
        running:  "#1f9d4d",
        degraded: "#d4a106",
        stopped:  "#c63838",
        starting: "#d4a106",
        building: "#d4a106",
        stopping: "#d4a106",
        failed:   "#c63838",
      }[snap.state]) || "#b8bcc6";
    }
  }
  // Keep the target-app action set visually stable. State changes only
  // enable/disable buttons so the action column does not flicker.
  const startBtn = document.getElementById("s-target-run-start");
  const buildBtn = document.getElementById("s-target-run-build");
  const stopBtn  = document.getElementById("s-target-run-stop");
  if (startBtn && stopBtn && buildBtn) {
    const isRunning  = snap.state === "running" || snap.state === "degraded";
    const isStopped  = snap.state === "stopped" || snap.state === "unknown" || snap.state === "failed";
    const inFlight   = snap.state === "starting" || snap.state === "stopping" || snap.state === "building";
    const showStop = targetAppShowsStopAction(snap.state);
    const hasStartAction = snap.has_start_action ?? snap.has_start_instructions ?? snap.has_start_command;
    const hasStopAction = snap.has_stop_action ?? snap.has_stop_instructions ?? snap.has_stop_command;
    const hasBuildAction = snap.has_build_action ?? snap.has_build_instructions ?? snap.has_build_command;
    setTargetAppActionVisible(startBtn, !showStop);
    setTargetAppActionVisible(stopBtn, showStop);
    startBtn.disabled = showStop || isRunning || inFlight || !hasStartAction;
    buildBtn.disabled = inFlight;
    stopBtn.disabled  = !showStop || isStopped || inFlight || !hasStopAction;
    if (!hasStartAction) {
      startBtn.title = "Configure start instructions first.";
    } else if (isRunning) {
      startBtn.title = "Application is already running.";
    } else if (inFlight) {
      startBtn.title = "Application state is changing.";
    } else {
      startBtn.title = "";
    }
    if (!hasStopAction) {
      stopBtn.title = "Configure stop instructions first.";
    } else if (isStopped) {
      stopBtn.title = "Application is already stopped.";
    } else if (inFlight) {
      stopBtn.title = "Application state is changing.";
    } else {
      stopBtn.title = "";
    }
    if (inFlight) {
      buildBtn.title = "Application state is changing.";
    } else if (!hasBuildAction) {
      buildBtn.title = "No build instructions configured; build is a no-op.";
    } else {
      buildBtn.title = "";
    }
  }
  const targetRow = document.querySelector('[data-process-id="target-app"]');
  if (targetRow) {
    const statusCell = targetRow.querySelector("[data-process-status]");
    const detailsCell = targetRow.querySelector("[data-process-details]");
    if (statusCell) statusCell.textContent = processStatusLabel(snap.state || "unknown");
    if (detailsCell) {
      const details = targetAppProcessDetails(snap);
      detailsCell.textContent = details || "-";
      detailsCell.classList.toggle("muted", !details);
      detailsCell.classList.toggle("small", !details);
      detailsCell.classList.toggle("process-details-cell", !!details);
      if (details) {
        detailsCell.dataset.fullDetails = details;
        detailsCell.dataset.detailTitle = "Process details";
        detailsCell.title = details;
      } else {
        delete detailsCell.dataset.fullDetails;
        delete detailsCell.dataset.detailTitle;
        detailsCell.title = "";
      }
      updateProcessDetailAffordances();
    }
  }
}

function bindSettingsProcessesTab(s) {
  bindProcessDetailCells();
  bindSupervisorProcessToggle();
  $$("[data-toggle-workflow]").forEach((b) => {
    b.addEventListener("click", async () => {
      const shouldPause = b.dataset.toggleWorkflow === "pause";
      const ok = shouldPause
        ? await modalConfirm(
            "Pause workflow automation? Refine will stop launching Goal agents, QA, builds, and merges until you unpause.",
            { title: "Pause Workflow", okLabel: "Pause Workflow", danger: true },
          )
        : true;
      if (!ok) return;
      await withButtonBusy(b, shouldPause ? "Pausing…" : "Unpausing…", async () => {
        try {
          await api("POST", "/api/workflow/pause", { paused: shouldPause });
          await refreshProcessesSettingsTab({ force: true });
          if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
          if (!shouldPause) scheduleProcessesTabRefreshes();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  bindCommand("[data-hard-reset-worktree]", "system.worktree.hard_reset");
  $$("[data-cancel-agent]").forEach((b) => {
    b.addEventListener("click", async () => {
      const id = b.dataset.cancelAgent;
      const ok = await modalConfirm(
        "Cancel this Goal's running subprocess?",
        { title: "Cancel run", okLabel: "Cancel run", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Cancelling…", async () => {
        try {
          await api("POST", `/api/goals/${id}/cancel`);
          await refreshProcessesSettingsTab();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-stop-chat]").forEach((b) => {
    b.addEventListener("click", async () => {
      const id = b.dataset.stopChat;
      const ok = await modalConfirm(
        "Stop this chat session?",
        { title: "Stop chat", okLabel: "Stop chat", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Stopping…", async () => {
        try {
          await api("POST", `/api/chat/${id}/stop`);
          await refreshProcessesSettingsTab();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-runner-target-app-build]").forEach((b) => {
    b.addEventListener("click", async () => {
      await withButtonBusy(b, "Queueing…", async () => {
        try {
          await api("POST", "/api/runner-workers/target-app-builder/build");
          await refreshProcessesSettingsTab({ force: true });
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-runner-target-app-generate]").forEach((b) => {
    b.addEventListener("click", async () => {
      await runCommand("target_app.generate", {
        context: { button: b },
      });
    });
  });
  $$("[data-runner-cache-rebuild]").forEach((b) => {
    b.addEventListener("click", async () => {
      await runCommand("system.cache.rebuild", { context: { button: b } });
    });
  });
  $$("[data-runner-log-cleanup]").forEach((b) => {
    b.addEventListener("click", async () => {
      const select = b.parentElement?.querySelector("[data-runner-log-cleanup-days]");
      const days = parseInt(select?.value || "7", 10);
      await runCommand("system.logs.cleanup", {
        context: { button: b },
        params: { days },
      });
    });
  });
  bindCommand("#s-target-run-start", "target_app.start");
  bindCommand("#s-target-run-stop", "target_app.stop");
  bindCommand("#s-target-run-build", "target_app.build");
  bindCommand("#s-target-health-now", "target_app.health");
  // Kick off the initial status load (and let SSE refresh later).
  refreshTargetAppStatus();
}

function scheduleProcessesTabRefreshes() {
  for (const delay of [750, 2000]) {
    setTimeout(() => {
      if (state.currentRoute !== "node") return;
      if (!document.querySelector('[data-tab-pane="processes"].active')) return;
      if (typeof refreshActiveSettingsTab === "function") {
        refreshActiveSettingsTab({ force: true });
      } else {
        refreshSettings({ force: true });
      }
    }, delay);
  }
}

async function refreshProcessesSettingsTab(options = {}) {
  if (typeof refreshSettingsTab === "function") {
    await refreshSettingsTab("processes", options);
  } else {
    await refreshSettings(options);
  }
}
