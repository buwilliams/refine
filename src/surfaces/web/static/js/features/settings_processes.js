// ---- System / Processes -----------------------------------------------------

function renderProcessesTab(processData = {}, sourceData = {}) {
  const rows = buildProcessManagerRows(processData, sourceData)
    .map(renderProcessManagerRow)
    .join("");
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Refine process manager", "process-management")}</h3>
      <table class="table process-table process-manager-table mobile-card-table" data-testid="process-manager-table">
        <colgroup>
          <col class="process-col">
          <col class="pid-col">
          <col class="memory-col">
          <col class="cpu-col">
          <col class="details-col">
          <col class="actions-col">
        </colgroup>
        <thead><tr>
          <th>Process name</th><th>PID</th><th>Memory used</th>
          <th>Processor used</th><th>Details</th><th>Actions</th>
        </tr></thead>
        <tbody>${rows || `<tr><td colspan="6" class="muted">No Refine processes found.</td></tr>`}</tbody>
      </table>
    </section>`;
}

function buildProcessManagerRows(processData = {}, sourceData = {}) {
  const processes = (processData.processes || []).filter((proc) =>
    isCurrentProcessStatus(proc.status),
  );
  const disk = processData.repository_disk_usage || {};
  const targetRecord = processes.find((proc) => proc.kind === "target_app") || {};
  const targetSnap = processData.target_app || { state: targetRecord.status || "unknown" };
  const targetApp = {
    ...targetRecord,
    id: "target-app",
    kind: "target_app",
    label: "Target app",
    status: targetSnap.state || targetRecord.status || "unknown",
    pid: targetRecord.pid || targetSnap.pid || null,
    target_app: targetSnap,
    repository_disk_usage: disk.target_app || null,
  };

  const daemonRecord = processes.find((proc) => proc.kind === "daemon")
    || processes.find((proc) => proc.kind === "supervisor")
    || {};
  const daemon = {
    ...daemonRecord,
    id: daemonRecord.id || "refine-daemon",
    kind: "daemon",
    label: "Refine daemon",
    status: daemonRecord.status || "running",
    source_update: sourceData.source_update || {},
    repository_disk_usage: disk.daemon || null,
    management_actions: ["update_refine", "stop_daemon"],
  };

  const backgroundWorkers = (processData.background_workers || []).map((worker) => ({
    ...worker,
    kind: "background_worker",
    label: worker.worker_kind || worker.label || "background worker",
  }));
  const representedWorkerKinds = new Set(backgroundWorkers.map((worker) => worker.worker_kind));
  const dynamicBackgroundWorkers = processes
    .filter((proc) => proc.kind === "runner" && proc.worker_kind
      && !representedWorkerKinds.has(proc.worker_kind))
    .map((proc) => ({
      ...proc,
      kind: "background_worker",
      label: proc.worker_kind,
      management_actions: ["stop_process"],
    }));
  const agents = processes.filter(isCurrentAgentProviderProcessRecord);
  const representedIds = new Set([
    targetRecord.id,
    daemonRecord.id,
    ...backgroundWorkers.map((worker) => worker.process_id),
    ...dynamicBackgroundWorkers.map((worker) => worker.id),
    ...agents.map((agent) => agent.id),
  ].filter(Boolean));
  const otherProcesses = processes.filter((proc) => !representedIds.has(proc.id));
  return [targetApp, daemon, ...backgroundWorkers, ...dynamicBackgroundWorkers, ...agents, ...otherProcesses];
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

function renderProcessManagerRow(proc) {
  const kind = proc.kind || "process";
  const pid = proc.pid ? htmlEscape(String(proc.pid)) : `<span class="muted small">-</span>`;
  const label = renderProcessManagerLabel(proc);
  const details = processManagerDetails(proc);
  const detailsAttrs = details
    ? ` class="process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Process details" title="${htmlEscape(details)}"`
    : "";
  return `
    <tr data-testid="process-manager-row" data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}">
      <td data-label="Process name">${label}</td>
      <td data-label="PID">${pid}</td>
      <td data-label="Memory used">${htmlEscape(formatProcessBytes(proc.memory_used_bytes))}</td>
      <td data-label="Processor used">${htmlEscape(formatProcessorUsed(proc.processor_used_percent))}</td>
      <td data-label="Details" data-testid="process-manager-details" data-process-details${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
}

function renderProcessManagerLabel(proc) {
  const attachedGoalId = proc.goal_id || proc.attached_goal_id || "";
  if (isAgentProviderProcessRecord(proc) && attachedGoalId) {
    return `<a href="#/goals/${htmlEscape(attachedGoalId)}">Agent · ${htmlEscape(attachedGoalId.slice(0, 10))}…</a>`;
  }
  if (isAgentProviderProcessRecord(proc)) {
    return htmlEscape(proc.label || (proc.kind === "chat" ? "Agent session" : "Agent"));
  }
  return htmlEscape(proc.label || processKindLabel(proc.kind));
}

function processManagerDetails(proc) {
  const details = [
    processStatusLabel(proc.status || "unknown"),
    proc.goal_id ? `Goal ${proc.goal_id}` : "",
    proc.round_idx != null ? `round ${Number(proc.round_idx) + 1}` : "",
    proc.provider || "",
    proc.profile || proc.mode || "",
    proc.kind === "target_app" ? targetAppProcessDetails(proc.target_app || {}) : "",
    readableProcessDetails(proc.details),
    repositoryDiskUsageDetails(proc.repository_disk_usage),
    proc.kind === "daemon" && proc.source_update?.title ? proc.source_update.title : "",
  ].filter(Boolean).join(" · ");
  return details;
}

function readableProcessDetails(details) {
  if (!details || typeof details !== "string") return "";
  try {
    const parsed = JSON.parse(details);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return details;
    return [parsed.workflow_state, parsed.behavior, parsed.attention_message]
      .filter(Boolean)
      .join(" · ");
  } catch (_) {
    return details;
  }
}

function repositoryDiskUsageDetails(usage) {
  if (!usage || !Number.isFinite(Number(usage.bytes))) return "";
  const worktrees = usage.includes_git_worktrees === true
    ? " (includes .git worktrees)"
    : "";
  return `repository disk ${formatProcessBytes(usage.bytes)}${worktrees}`;
}

function formatProcessBytes(value) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes) || bytes < 0) return "-";
  if (bytes < 1024) return `${bytes.toFixed(0)} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let amount = bytes / 1024;
  let unit = units[0];
  for (let i = 1; i < units.length && amount >= 1024; i += 1) {
    amount /= 1024;
    unit = units[i];
  }
  return `${amount >= 100 ? amount.toFixed(0) : amount >= 10 ? amount.toFixed(1) : amount.toFixed(2)} ${unit}`;
}

function formatProcessorUsed(value) {
  const percent = Number(value);
  return Number.isFinite(percent) ? `${percent.toFixed(1)}%` : "-";
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
    supervisor: "Refine daemon",
    daemon: "Refine daemon",
    runner: "Runner",
    target_app: "Target app",
    background_worker: "Background worker",
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

function workflowPausedFor(value = {}) {
  if (typeof value.workflow_paused === "boolean") return value.workflow_paused;
  if (typeof value.paused === "boolean") return value.paused;
  return !!value.background_processes_stopped || !!value.agents_paused;
}

const WORKFLOW_PAUSE_CONFIRMATION = "Pause workflow automation? Refine will stop admitting new Goal work and quiesce automatic Git sync and inactive-worktree cleanup at safe boundaries. Already active Goal executions continue unless you Stop their Agents separately.";

function workflowPauseActionModel(value = {}) {
  const paused = workflowPausedFor(value);
  const shouldPause = !paused;
  return {
    shouldPause,
    actionId: shouldPause ? "pause_workflow" : "unpause_workflow",
    direction: shouldPause ? "pause" : "unpause",
    status: paused ? "paused" : "active",
    description: paused
      ? "New Goal admission is paused; automatic Git sync and inactive-worktree cleanup quiesce at safe boundaries. Active Goal executions continue unless stopped separately."
      : "New Goal admission, automatic Git sync, and inactive-worktree cleanup are eligible. Active Goal executions continue unless stopped separately.",
    buttonLabel: shouldPause ? "Pause Workflow" : "Unpause Workflow",
    busyLabel: shouldPause ? "Pausing…" : "Unpausing…",
    confirmation: shouldPause ? WORKFLOW_PAUSE_CONFIRMATION : null,
    payload: { paused: shouldPause },
  };
}

function processActionIds(proc) {
  if (Array.isArray(proc.management_actions)) return proc.management_actions;
  if (Array.isArray(proc.actions)) {
    const supported = proc.actions.filter((actionId) => isSupportedProcessActionId(proc, actionId));
    if (supported.length) return supported;
  }
  return null;
}

function isSupportedProcessActionId(proc, actionId) {
  if ([
    "pause_workflow", "unpause_workflow", "start_background_worker",
    "stop_background_worker", "stop_process", "update_refine", "stop_daemon",
  ].includes(actionId)) return true;
  if (actionId === "stop_agent") return isAgentProviderProcessRecord(proc) && !!proc.id;
  if (actionId === "cancel_agent") return proc.kind === "agent" && !!proc.goal_id;
  if (actionId === "stop_chat" || actionId === "stop") return proc.kind === "chat" && !!proc.session_id;
  return false;
}

function renderProcessActions(proc) {
  const actionIds = processActionIds(proc);
  if (actionIds) return renderProcessActionButtons(proc, actionIds);
  if (isAgentProviderProcessRecord(proc) && proc.id) {
    return renderStopAgentButton(proc);
  }
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
  if (proc.id) {
    return `<button class="danger" data-testid="process-stop" data-stop-process="${htmlEscape(proc.id)}">Stop</button>`;
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
    const action = workflowPauseActionModel({
      workflow_paused: actionId === "unpause_workflow",
    });
    const disabled = workflowToggleDisabled(proc);
    return `<button class="${action.shouldPause ? "secondary" : ""}" data-testid="process-workflow-toggle" data-toggle-workflow="${action.direction}" data-workflow-paused="${action.shouldPause ? "false" : "true"}" ${disabled ? "disabled" : ""}>${action.buttonLabel}</button>`;
  }
  if (actionId === "start_background_worker" || actionId === "stop_background_worker") {
    const start = actionId === "start_background_worker";
    return `<button class="${start ? "" : "danger"}" data-testid="process-background-worker-${start ? "start" : "stop"}" data-background-worker-action="${start ? "start" : "stop"}" data-worker-kind="${htmlEscape(proc.worker_kind || "")}">${start ? "Start" : "Stop"}</button>`;
  }
  if (actionId === "stop_process" && proc.id) {
    return `<button class="danger" data-testid="process-stop" data-stop-process="${htmlEscape(proc.id)}">Stop</button>`;
  }
  if (actionId === "update_refine") {
    const update = proc.source_update || {};
    const disabled = update.enabled !== true || update.update_available !== true;
    return `<button data-testid="process-daemon-update" data-update-refine ${disabled ? "disabled" : ""} title="${htmlEscape(update.title || "Refine update status unavailable")}">Update</button>`;
  }
  if (actionId === "stop_daemon") {
    return `<button class="danger" data-testid="process-daemon-stop" data-stop-daemon>Stop</button>`;
  }
  if (actionId === "stop_agent" && isAgentProviderProcessRecord(proc) && proc.id) {
    return renderStopAgentButton(proc);
  }
  if (actionId === "cancel_agent" && proc.kind === "agent" && proc.goal_id) {
    return `<button class="danger" data-testid="process-cancel-agent" data-cancel-agent="${htmlEscape(proc.goal_id)}">Cancel</button>`;
  }
  if ((actionId === "stop_chat" || actionId === "stop") && proc.kind === "chat" && proc.session_id) {
    return `<button class="danger" data-testid="process-stop-chat" data-stop-chat="${htmlEscape(proc.session_id)}">Stop</button>`;
  }
  return "";
}

function renderStopAgentButton(proc) {
  const goal = proc.goal_id
    ? ` data-stop-agent-goal="${htmlEscape(proc.goal_id)}"`
    : "";
  return `<button class="danger" data-testid="process-stop-agent" data-stop-agent="${htmlEscape(proc.id)}"${goal}>Stop</button>`;
}

function workflowToggleDisabled() { return false; }

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

function bindProcessDetailCells() {
  updateProcessDetailAffordances();
  $$(".process-details-cell").forEach((cell) => {
    bindOnce(cell, "click", () => openProcessDetailsIfOverflowing(cell));
    bindOnce(cell, "keydown", (ev) => {
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
      renderInto(block, `<span class="muted">Status unavailable: ${htmlEscape(e.message)}</span>`);
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
  const block = document.getElementById("target-app-status-block");
  if (block) {
    renderInto(block, `
      <div style="display:flex;align-items:center;gap:10px">
        <span class="target-app-dot" data-status-dot></span>
        <strong>${htmlEscape(stateLabel)}</strong>
        ${snap.has_status_checks ? `<span class="muted small">status checks configured</span>` : `<span class="muted small">No status checks configured</span>`}
      </div>
      <p class="muted small" style="margin:8px 0 0">${htmlEscape(healthBits)}</p>
      ${healthDetail}
      ${op}
      ${snap.last_error ? `<p class="muted small" style="margin-top:6px;color:var(--error)">Last error: ${htmlEscape(snap.last_error)}</p>` : ""}
      ${snap.legacy_config_present ? `<p class="muted small" style="margin-top:6px;color:var(--warn)">Legacy target-app settings detected.</p>` : ""}
    `);
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

function bindSettingsProcessesTab() {
  bindProcessDetailCells();
  $$("[data-toggle-workflow]").forEach((b) => {
    bindOnce(b, "click", async () => {
      const action = workflowPauseActionModel({
        workflow_paused: b.dataset.workflowPaused === "true",
      });
      const ok = action.shouldPause
        ? await modalConfirm(
            action.confirmation,
            { title: "Pause Workflow", okLabel: "Pause Workflow", danger: true },
          )
        : true;
      if (!ok) return;
      await withButtonBusy(b, action.busyLabel, async () => {
        try {
          await api("POST", "/api/workflow/pause", action.payload);
          await refreshProcessesSettingsTab({ force: true });
          if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
          if (!action.shouldPause) scheduleProcessesTabRefreshes();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$('[data-background-worker-action]').forEach((b) => {
    bindOnce(b, "click", async () => {
      const action = b.dataset.backgroundWorkerAction;
      const workerKind = b.dataset.workerKind;
      await withButtonBusy(b, action === "start" ? "Starting…" : "Stopping…", async () => {
        try {
          await api(
            "POST",
            `/api/processes/background-workers/${encodeURIComponent(workerKind)}/${action}`,
            {},
          );
          await refreshProcessesSettingsTab({ force: true });
          scheduleProcessesTabRefreshes();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$('[data-stop-process]').forEach((b) => {
    bindOnce(b, "click", async () => {
      await withButtonBusy(b, "Stopping…", async () => {
        try {
          await api("POST", `/api/processes/${encodeURIComponent(b.dataset.stopProcess)}/stop`, {
            signal: "kill",
          });
          await refreshProcessesSettingsTab({ force: true });
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$('[data-update-refine]').forEach((b) => {
    bindOnce(b, "click", async () => {
      await withButtonBusy(b, "Updating…", async () => {
        try {
          const result = await api("POST", "/api/system/source/promote", {});
          toast(result.operation?.message || "Refine update queued; reconnecting after restart", "info");
          scheduleProcessesTabRefreshes();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$('[data-stop-daemon]').forEach((b) => {
    bindOnce(b, "click", async () => {
      await withButtonBusy(b, "Stopping…", async () => {
        try {
          await api("POST", "/api/system/stop", {});
          toast("Stopping all Refine processes for this runtime", "info");
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-stop-agent]").forEach((b) => {
    bindOnce(b, "click", async () => {
      const processId = b.dataset.stopAgent;
      await withButtonBusy(b, "Stopping…", async () => {
        try {
          const stopped = await api("POST", `/api/processes/${encodeURIComponent(processId)}/stop`, {
            signal: "kill",
          });
          if (typeof removeToolbarTabsForStoppedProcess === "function") {
            removeToolbarTabsForStoppedProcess(
              processId,
              stopped?.process?.session_id || "",
            );
          }
          if (stopped?.worktrees_retained) {
            const goalOutcome = stopped?.goal?.status === "cancelled"
              ? "Explicit Goal cancellation remains terminal."
              : "The Goal is now failed; start a fresh follow-up Round to retry.";
            toast(
              `Agent stopped. ${goalOutcome} Its workflow worktree and branch were retained for inspection or explicit cleanup.`,
              "info",
            );
          }
          await refreshProcessesSettingsTab();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  bindCommand("#s-target-run-start", "target_app.start");
  bindCommand("#s-target-run-stop", "target_app.stop");
  bindCommand("#s-target-run-build", "target_app.build");
  bindCommand("#s-target-health-now", "target_app.health");
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
