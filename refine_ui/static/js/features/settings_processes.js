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
  const processes = processData.processes || [];
  const runnerWork = processData.runner_work || [];
  const runnerReachable = typeof processData.runner_reachable === "boolean"
    ? processData.runner_reachable
    : !!diag.reachable;
  const anchorMs = Date.now();
  const rows = buildManagedProcessRows(
    processes, { backgroundStopped, agentsPaused }, backend, runnerReachable, diag,
  ).map((proc) => renderManagedProcessRow(proc)).join("");
  const agentRows = (processes || [])
    .filter((proc) => proc.kind === "agent" || proc.kind === "chat")
    .map((proc) => renderAgentProcessRow(proc, anchorMs)).join("");
  const workRows = runnerWork.map((work) => renderRunnerWorkRow(work, anchorMs)).join("");
  return `
    <section class="settings-section">
      <div id="runtime-upgrade-banner"></div>
    </section>

    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Process management", "process-management")}</h3>
      ${rows ? `
        <table class="table process-table managed-process-table mobile-card-table">
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
      <h3>${renderSettingsGuideLabel("Agent processes", "process-agent-processes")}</h3>
      ${agentRows ? `
        <table class="table process-table agents-process-table mobile-card-table">
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
        </table>` : `<p class="muted">No active agent subprocesses or chat sessions.</p>`}
    </section>

    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Runner processes", "process-runner-processes")}</h3>
      <table class="table process-table runner-workers-table mobile-card-table">
        <colgroup>
          <col class="worker-col">
          <col class="status-col">
          <col class="gap-col">
          <col class="elapsed-col">
          <col class="queue-col">
          <col class="details-col">
          <col class="worker-actions-col">
        </colgroup>
        <thead><tr>
          <th>Worker</th><th>Status</th><th>Gap</th>
          <th>Elapsed</th><th>Queue</th><th>Details</th><th></th>
        </tr></thead>
        <tbody>${workRows}</tbody>
      </table>
      <div id="sqlite-cache-progress" style="display:none;margin-top:12px"></div>
    </section>`;
}

function buildManagedProcessRows(processes, pauseState, backend, runnerReachable, diag) {
  const backgroundStopped = !!pauseState.backgroundStopped;
  const agentsPaused = !!pauseState.agentsPaused;
  const rows = (processes || [])
    .filter((proc) => proc.kind !== "agent" && proc.kind !== "chat")
    .map((proc) => {
      if (proc.kind !== "runner") return proc;
      return {
        ...proc,
        details: runnerProcessDetails(backend, runnerReachable, diag),
      };
    });
  const background = {
    id: "background-processes",
    kind: "background_processes",
    label: "Background processes",
    status: backgroundStopped ? "paused" : "active",
    runner_reachable: runnerReachable,
    pid: null,
    details: backgroundStopped
      ? "Automatic runner work, agent launches, chats, and UI background jobs are stopped."
      : "Automatic runner work, agent launches, chats, and UI background jobs can run.",
    background_processes_stopped: backgroundStopped,
    actions: [backgroundStopped ? "start_background_processes" : "stop_background_processes", "hard_reset_worktree"],
    cpu_priority: { label: "-" },
    max_memory: { label: "-" },
  };
  const agentScheduler = {
    id: "agent-scheduler",
    kind: "agent_scheduler",
    label: "Agent scheduler",
    status: backgroundStopped || agentsPaused ? "paused" : "active",
    runner_reachable: runnerReachable,
    pid: null,
    details: backgroundStopped
      ? "Background processes are stopped; agent launches wait."
      : agentsPaused
        ? "New agent subprocesses wait; running subprocesses are cancelled."
        : "New agent subprocesses launch on demand.",
    agents_paused: agentsPaused,
    background_processes_stopped: backgroundStopped,
    actions: [agentsPaused ? "unpause_agents" : "pause_agents"],
    cpu_priority: { label: "-" },
    max_memory: { label: "-" },
  };
  return orderManagedProcessRows(
    rows, agentScheduler, background, backend.process_model === "supervisor",
  );
}

function orderManagedProcessRows(rows, agentScheduler, background, supervised) {
  const targetApp = rows.find((proc) => proc.kind === "target_app");
  const targetAppId = targetApp ? targetApp.id : null;
  if (!supervised) {
    return [
      ...rows.filter((proc) => !targetApp || proc.id !== targetAppId),
      agentScheduler,
      background,
      ...(targetApp ? [targetApp] : []),
    ];
  }
  const supervisor = rows.find((proc) => proc.kind === "supervisor");
  if (!supervisor) {
    return [
      ...rows.filter((proc) => !targetApp || proc.id !== targetAppId),
      agentScheduler,
      background,
      ...(targetApp ? [targetApp] : []),
    ];
  }
  const childKinds = new Set(["ui", "runner"]);
  const children = [
    ...rows.filter((proc) => childKinds.has(proc.kind)),
    agentScheduler,
    background,
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
    <tr${rowClassAttr} data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}"${supervisorChildAttr}${hiddenAttr}>
      <td data-label="Process">${label}</td>
      <td data-label="Status" data-process-status>${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td data-label="PID">${pid}</td>
      <td data-label="CPU priority">${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td data-label="Max memory">${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td data-label="Details" data-process-details${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
}

function renderManagedProcessLabel(proc, rawLabel) {
  if (proc.supervisor_parent && Number(proc.supervisor_child_count || 0) > 0) {
    const expanded = !!proc.supervisor_expanded;
    return `
      <span class="process-tree-label">
        <button type="button" class="process-tree-toggle" data-supervisor-toggle
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
  const pid = proc.pid ? htmlEscape(String(proc.pid)) : `<span class="muted small">-</span>`;
  const elapsed = Number.isFinite(Number(proc.elapsed_seconds))
    ? `<span class="js-elapsed-tick" data-base="${Number(proc.elapsed_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(proc.elapsed_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const idle = Number.isFinite(Number(proc.idle_seconds))
    ? `<span class="js-idle-tick" data-base="${Number(proc.idle_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(proc.idle_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const label = kind === "chat"
    ? `${htmlEscape(proc.mode === "gap" ? "Gap chat" : proc.mode === "plan" ? "Plan chat" : "Standalone chat")}<br><code>${htmlEscape(proc.session_id || "")}</code>`
    : proc.gap_id
    ? `<a href="#/gaps/${htmlEscape(proc.gap_id)}">${htmlEscape(proc.gap_id.slice(0, 10))}...</a>`
    : htmlEscape(proc.label || "Agent");
  const context = kind === "chat"
    ? proc.gap_id
      ? `<a href="#/gaps/${htmlEscape(proc.gap_id)}">${htmlEscape(proc.gap_id.slice(0, 10))}...</a>`
      : "standalone"
    : proc.round_idx != null
    ? String(Number(proc.round_idx) + 1)
    : "";
  return `
    <tr data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}">
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
    supervisor: "supervisor",
    runner: "runner",
    target_app: "application",
    agent_scheduler: "agent scheduler",
    background_processes: "background processes",
    agent: "agent",
    chat: "chat",
  }[kind] || "process";
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
    rebuilding: "rebuilding",
    stopping: "stopping",
    stopped: "stopped",
    failed: "failed",
    unknown: "unknown",
    active: "active",
    paused: "paused",
    idle: "idle",
  }[status] || status || "unknown";
}

function renderProcessActions(proc) {
  if (proc.kind === "supervisor") {
    const stopped = !!proc.background_processes_stopped;
    const agentsPaused = !!proc.agents_paused;
    return `
      <button class="${agentsPaused ? "" : "secondary"}" data-toggle-agent-processes="${agentsPaused ? "unpause" : "pause"}">${agentsPaused ? "Unpause" : "Pause"} agents</button>
      <button class="${stopped ? "" : "danger"}" data-toggle-background-processes="${stopped ? "start" : "stop"}">${stopped ? "Start" : "Stop"} Background</button>`;
  }
  if (proc.kind === "agent_scheduler") {
    const agentsPaused = !!proc.agents_paused;
    return `
      <button class="${agentsPaused ? "" : "secondary"}" data-toggle-agent-processes="${agentsPaused ? "unpause" : "pause"}" ${proc.runner_reachable ? "" : "disabled"}>${agentsPaused ? "Unpause" : "Pause"} agents</button>`;
  }
  if (proc.kind === "background_processes") {
    const paused = proc.status === "paused";
    const stopped = !!proc.background_processes_stopped;
    return `
      <button class="${stopped ? "" : "danger"}" data-toggle-background-processes="${stopped ? "start" : "stop"}">${stopped ? "Start" : "Stop"} Background</button>
      <button class="danger" data-hard-reset-worktree ${proc.runner_reachable && !paused ? "" : "disabled"}>Hard reset worktree</button>`;
  }
  if (proc.kind === "agent" && proc.gap_id) {
    return `<button class="danger" data-cancel-agent="${htmlEscape(proc.gap_id)}">Cancel</button>`;
  }
  if (proc.kind === "chat" && proc.session_id) {
    return `<button class="danger" data-stop-chat="${htmlEscape(proc.session_id)}">Stop</button>`;
  }
  if (proc.kind === "target_app") {
    const snap = proc.target_app || {};
    const inFlight = ["starting", "stopping", "rebuilding"].includes(snap.state);
    const isRunning = snap.state === "running" || snap.state === "degraded";
    const isStopped = snap.state === "stopped" || snap.state === "unknown" || snap.state === "failed";
    const showStop = targetAppShowsStopAction(snap.state);
    return `
      <span class="target-app-action-slot">
        <button id="s-target-run-start" class="${showStop ? "target-app-action-hidden" : ""}" ${showStop || isRunning || inFlight || !snap.has_start_command ? "disabled" : ""} ${showStop ? `aria-hidden="true" tabindex="-1"` : ""}>Start</button>
        <button class="danger ${showStop ? "" : "target-app-action-hidden"}" id="s-target-run-stop" ${!showStop || isStopped || inFlight || !snap.has_stop_command ? "disabled" : ""} ${showStop ? "" : `aria-hidden="true" tabindex="-1"`}>Stop</button>
      </span>
      <button class="secondary" id="s-target-run-rebuild" ${inFlight ? "disabled" : ""}>Rebuild</button>
      <button class="secondary" id="s-target-sync-now">Sync</button>
      <button class="secondary" id="s-target-health-now">Check</button>`;
  }
  return `<span class="muted small">-</span>`;
}

function targetAppShowsStopAction(state) {
  return ["running", "degraded", "stopping", "rebuilding"].includes(state);
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
  const gap = work.gap_id
    ? `<a href="#/gaps/${htmlEscape(work.gap_id)}">${htmlEscape(work.gap_id.slice(0, 10))}...</a>`
    : `<span class="muted small">-</span>`;
  const elapsed = Number.isFinite(Number(work.elapsed_seconds))
    ? `<span class="js-elapsed-tick" data-base="${Number(work.elapsed_seconds) || 0}" data-anchor-ms="${anchorMs}">${fmtElapsed(work.elapsed_seconds || 0)}</span>`
    : `<span class="muted small">-</span>`;
  const queued = Number(work.queued || 0);
  const details = work.details || work.last_outcome || "";
  const detailsAttrs = details
    ? ` class="runner-work-details process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Runner worker details" title="${htmlEscape(details)}"`
    : ` class="runner-work-details"`;
  return `
    <tr>
      <td data-label="Worker">${htmlEscape(runnerWorkKindLabel(work.kind))}</td>
      <td data-label="Status">${htmlEscape(processStatusLabel(work.status || ""))}</td>
      <td data-label="Gap">${gap}</td>
      <td data-label="Elapsed">${elapsed}</td>
      <td data-label="Queue">${queued ? fmtCount(queued) : `<span class="muted small">-</span>`}</td>
      <td data-label="Details"${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td data-label="Actions" class="process-actions"><div class="actions">${renderRunnerWorkActions(work)}</div></td>
    </tr>`;
}

function renderRunnerWorkActions(work) {
  if (work.kind === "target_app_rebuilder") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="secondary" data-runner-target-app-rebuild ${busy ? "disabled" : ""}>Rebuild</button>`;
  }
  if (work.kind === "target_app_config_generator") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="secondary" data-runner-target-app-generate ${busy ? "disabled" : ""}>Generate</button>`;
  }
  if (work.kind === "sqlite_cache_rebuild") {
    const busy = ["running", "queued", "unknown", "paused"].includes(work.status);
    return `<button class="danger" data-runner-cache-rebuild ${busy ? "disabled" : ""}>Rebuild</button>`;
  }
  if (work.kind === "activity_log_cleanup") {
    const paused = work.status === "paused";
    return `
      <select data-runner-log-cleanup-days aria-label="Activity log retention" ${paused ? "disabled" : ""}>
        ${[0, 7, 30, 60, 90, 365].map((n) =>
          `<option value="${n}" ${n === 7 ? "selected" : ""}>${n === 0 ? "0 days" : `${n} days`}</option>`).join("")}
      </select>
      <button class="danger" data-runner-log-cleanup ${paused ? "disabled" : ""}>Clean up</button>`;
  }
  return `<span class="muted small">-</span>`;
}

function runnerWorkKindLabel(kind) {
  return {
    merger: "merger",
    governance: "governance",
    target_app_rebuilder: "target-app rebuilder",
    target_app_config_generator: "target-app config generator",
    sqlite_cache_rebuild: "SQLite cache rebuilder",
    activity_log_cleanup: "activity log cleanup",
    import_prepare: "import preparer",
    import_persist: "import persister",
    bulk_update_gaps: "bulk Gap updater",
    bulk_delete_gaps: "bulk Gap deleter",
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
  if (backend.process_model === "single_process") return "Single UI process";
  return "Unknown";
}

function backendTransportLabel(backend = {}) {
  if (backend.transport === "unix_socket") return "Unix socket";
  if (backend.transport === "direct_call") return "Direct in-process call";
  return "Unknown";
}

function shortPath(path) {
  const text = String(path || "");
  return text.split(/[\\/]/).pop() || text;
}


async function refreshTargetAppStatus() {
  const block = document.getElementById("target-app-status-block");
  const hasControls = document.getElementById("s-target-run-start")
    || document.getElementById("s-target-run-rebuild")
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
    rebuilding: "Rebuilding…",
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
  const autoRebuildLabel = {
    never: "Never",
    on_worktree_merge: "On worktree merge",
    hourly: "Hourly",
    nightly: "Nightly (midnight)",
  }[snap.auto_rebuild || "never"] || "Never";
  const autoRebuild = `<p class="muted small" style="margin-top:6px">Automatic rebuild: ${htmlEscape(autoRebuildLabel)}${
    snap.auto_rebuild_last_finished_at
      ? ` · last ${snap.auto_rebuild_last_ok ? "OK" : "failed"} at ${fmtTime(snap.auto_rebuild_last_finished_at)}`
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
      ${autoRebuild}
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
        rebuilding: "#d4a106",
        stopping: "#d4a106",
        failed:   "#c63838",
      }[snap.state]) || "#b8bcc6";
    }
  }
  // Keep the target-app action set visually stable. State changes only
  // enable/disable buttons so the action column does not flicker.
  const startBtn = document.getElementById("s-target-run-start");
  const rebuildBtn = document.getElementById("s-target-run-rebuild");
  const stopBtn  = document.getElementById("s-target-run-stop");
  if (startBtn && stopBtn && rebuildBtn) {
    const isRunning  = snap.state === "running" || snap.state === "degraded";
    const isStopped  = snap.state === "stopped" || snap.state === "unknown" || snap.state === "failed";
    const inFlight   = snap.state === "starting" || snap.state === "stopping" || snap.state === "rebuilding";
    const showStop = targetAppShowsStopAction(snap.state);
    setTargetAppActionVisible(startBtn, !showStop);
    setTargetAppActionVisible(stopBtn, showStop);
    startBtn.disabled = showStop || isRunning || inFlight || !snap.has_start_command;
    rebuildBtn.disabled = inFlight;
    stopBtn.disabled  = !showStop || isStopped || inFlight || !snap.has_stop_command;
    if (!snap.has_start_command) {
      startBtn.title = "Configure a start command above first.";
    } else if (isRunning) {
      startBtn.title = "Application is already running.";
    } else if (inFlight) {
      startBtn.title = "Application state is changing.";
    } else {
      startBtn.title = "";
    }
    if (!snap.has_stop_command) {
      stopBtn.title = "Configure a stop command above first.";
    } else if (isStopped) {
      stopBtn.title = "Application is already stopped.";
    } else if (inFlight) {
      stopBtn.title = "Application state is changing.";
    } else {
      stopBtn.title = "";
    }
    if (inFlight) {
      rebuildBtn.title = "Application state is changing.";
    } else if (!snap.has_rebuild_command) {
      rebuildBtn.title = "No rebuild command configured; rebuild will still run the stop/start sequence.";
    } else {
      rebuildBtn.title = "";
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
  bindRuntimeUpgradeBanner("[data-tab-pane=\"processes\"]");
  refreshRuntimeUpgradeBanner();
  $$("[data-toggle-background-processes]").forEach((b) => {
    b.addEventListener("click", async () => {
      const shouldStop = b.dataset.toggleBackgroundProcesses === "stop";
      const ok = shouldStop
        ? await modalConfirm(
            "Stop background processes? Refine will keep the UI and backend running, pause scheduling, stop chats and agents, clear queued rebuilds, and cancel active background jobs.",
            { title: "Stop background processes", okLabel: "Stop background processes", danger: true },
          )
        : true;
      if (!ok) return;
      await withButtonBusy(b, shouldStop ? "Stopping…" : "Starting…", async () => {
        try {
          await api("POST", "/api/processes/background", { stopped: shouldStop });
          await refreshProcessesSettingsTab({ force: true });
          if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
          if (!shouldStop) scheduleProcessesTabRefreshes();
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-toggle-agent-processes]").forEach((b) => {
    b.addEventListener("click", async () => {
      const shouldPause = b.dataset.toggleAgentProcesses === "pause";
      const ok = shouldPause
        ? await modalConfirm(
            "Pause agent scheduling? Refine will stop running Gap agents and leave other background processes alone.",
            { title: "Pause agents", okLabel: "Pause agents", danger: true },
          )
        : true;
      if (!ok) return;
      await withButtonBusy(b, shouldPause ? "Pausing…" : "Unpausing…", async () => {
        try {
          await api("POST", "/api/processes/agents", { paused: shouldPause });
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
        "Cancel this Gap's running subprocess?",
        { title: "Cancel run", okLabel: "Cancel run", danger: true,
          cancelLabel: "Keep running" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Cancelling…", async () => {
        try {
          await api("POST", `/api/gaps/${id}/cancel`);
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
  $$("[data-runner-target-app-rebuild]").forEach((b) => {
    b.addEventListener("click", async () => {
      await withButtonBusy(b, "Queueing…", async () => {
        try {
          await api("POST", "/api/runner-workers/target-app-rebuilder/rebuild");
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
  bindCommand("#s-target-run-rebuild", "target_app.rebuild");
  bindCommand("#s-target-sync-now", "target_app.sync");
  bindCommand("#s-target-health-now", "target_app.health");
  // Kick off the initial status load (and let SSE refresh later).
  refreshTargetAppStatus();
}

function scheduleProcessesTabRefreshes() {
  for (const delay of [750, 2000]) {
    setTimeout(() => {
      if (state.currentRoute !== "settings") return;
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
