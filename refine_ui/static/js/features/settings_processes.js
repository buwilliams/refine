// ---- System / Processes -----------------------------------------------------

function renderProcessesTab(processData, settings, diag, dash) {
  const paused = typeof processData.paused === "boolean"
    ? processData.paused
    : settings.paused === "1";
  const backend = processData.backend || diag.backend || dash.backend || {};
  const processes = processData.processes || [];
  const runnerWork = processData.runner_work || [];
  const runnerReachable = typeof processData.runner_reachable === "boolean"
    ? processData.runner_reachable
    : !!diag.reachable;
  const anchorMs = Date.now();
  const rows = buildManagedProcessRows(
    processes, paused, backend, runnerReachable, diag,
  ).map((proc) => renderManagedProcessRow(proc)).join("");
  const agentRows = (processes || [])
    .filter((proc) => proc.kind === "agent" || proc.kind === "chat")
    .map((proc) => renderAgentProcessRow(proc, anchorMs)).join("");
  const workRows = runnerWork.map((work) => renderRunnerWorkRow(work, anchorMs)).join("");
  return `
    <section class="settings-section">
      <h3>Managed processes</h3>
      ${rows ? `
        <table class="table process-table managed-process-table">
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
      <h3>Agents</h3>
      ${agentRows ? `
        <table class="table process-table agents-process-table">
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
      <h3>Runner workers</h3>
      <table class="table process-table runner-workers-table">
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

function buildManagedProcessRows(processes, paused, backend, runnerReachable, diag) {
  const rows = (processes || [])
    .filter((proc) => proc.kind !== "agent" && proc.kind !== "chat")
    .map((proc) => {
      if (proc.kind !== "runner") return proc;
      return {
        ...proc,
        details: runnerProcessDetails(backend, runnerReachable, diag),
      };
    });
  const scheduler = {
    id: "agent-scheduler",
    kind: "agent_scheduler",
    label: "Agent scheduler",
    status: paused ? "paused" : "active",
    pid: null,
    details: paused
      ? "New agent subprocesses wait; running subprocesses continue."
      : "New agent subprocesses launch on demand.",
    actions: [paused ? "resume" : "pause"],
    cpu_priority: { label: "-" },
    max_memory: { label: "-" },
  };
  rows.push(scheduler);
  return rows;
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
  const label = proc.session_id
    ? `<code>${htmlEscape(proc.session_id)}</code>`
    : htmlEscape(proc.label || processKindLabel(kind));
  const details = managedProcessDetails(proc);
  const detailsAttrs = details
    ? ` class="process-details-cell" data-full-details="${htmlEscape(details)}" data-detail-title="Process details" title="${htmlEscape(details)}"`
    : "";
  return `
    <tr data-process-id="${htmlEscape(proc.id || "")}" data-process-kind="${htmlEscape(kind)}">
      <td>${label}</td>
      <td data-process-status>${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td>${pid}</td>
      <td>${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td>${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td data-process-details${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
    </tr>`;
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
    ? `${htmlEscape(proc.mode === "gap" ? "Gap chat" : "Standalone chat")}<br><code>${htmlEscape(proc.session_id || "")}</code>`
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
      <td>${label}</td>
      <td>${htmlEscape(processStatusLabel(proc.status || ""))}</td>
      <td>${pid}</td>
      <td>${context ? context : `<span class="muted small">-</span>`}</td>
      <td>${htmlEscape(processResourceLabel(proc.cpu_priority))}</td>
      <td>${htmlEscape(processResourceLabel(proc.max_memory))}</td>
      <td>${elapsed}</td>
      <td>${idle}</td>
      <td class="process-actions"><div class="actions">${renderProcessActions(proc)}</div></td>
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
  if (proc.kind === "agent_scheduler") {
    const paused = proc.status === "paused";
    return `<button id="btn-pause" class="${paused ? "" : "secondary"}">${paused ? "Resume" : "Pause"} agents</button>`;
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
      <button class="secondary" id="s-target-run-rebuild" ${inFlight || !snap.has_rebuild_command ? "disabled" : ""}>Rebuild</button>
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
      <td>${htmlEscape(runnerWorkKindLabel(work.kind))}</td>
      <td>${htmlEscape(processStatusLabel(work.status || ""))}</td>
      <td>${gap}</td>
      <td>${elapsed}</td>
      <td>${queued ? fmtCount(queued) : `<span class="muted small">-</span>`}</td>
      <td${detailsAttrs}>${details ? htmlEscape(details) : `<span class="muted small">-</span>`}</td>
      <td class="process-actions"><div class="actions">${renderRunnerWorkActions(work)}</div></td>
    </tr>`;
}

function renderRunnerWorkActions(work) {
  if (work.kind === "target_app_rebuilder") {
    const busy = ["running", "queued", "unknown"].includes(work.status);
    return `<button class="secondary" data-runner-target-app-rebuild ${busy ? "disabled" : ""}>Rebuild</button>`;
  }
  if (work.kind === "target_app_config_generator") {
    const busy = ["running", "queued", "unknown"].includes(work.status);
    return `<button class="secondary" data-runner-target-app-generate ${busy ? "disabled" : ""}>Generate</button>`;
  }
  if (work.kind === "sqlite_cache_rebuild") {
    const busy = ["running", "queued", "unknown"].includes(work.status);
    return `<button class="danger" data-runner-cache-rebuild ${busy ? "disabled" : ""}>Rebuild</button>`;
  }
  if (work.kind === "activity_log_cleanup") {
    return `
      <select data-runner-log-cleanup-days aria-label="Activity log retention">
        ${[0, 7, 30, 60, 90, 365].map((n) =>
          `<option value="${n}" ${n === 7 ? "selected" : ""}>${n === 0 ? "0 days" : `${n} days`}</option>`).join("")}
      </select>
      <button class="danger" data-runner-log-cleanup>Clean up</button>`;
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
    rebuildBtn.disabled = inFlight || !snap.has_rebuild_command;
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
    if (!snap.has_rebuild_command) {
      rebuildBtn.title = "Configure a rebuild command above first.";
    } else if (inFlight) {
      rebuildBtn.title = "Application state is changing.";
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
  $("#btn-pause")?.addEventListener("click", async () => {
    const paused = s.paused === "1";
    await withButtonBusy($("#btn-pause"), paused ? "Resuming…" : "Pausing…", async () => {
      try {
        await api("PATCH", "/api/settings", { paused: paused ? "0" : "1" });
        await refreshProcessesSettingsTab({ force: true });
        if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
        if (paused) scheduleProcessesTabRefreshes();
      } catch (e) { await showActionError(e); }
    });
  });
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
      const ok = await modalConfirm(
        "Ask the agent to analyse the codebase and draft target-app configuration? This can take a minute or two and overwrites the fields on the Application tab.",
        { title: "Generate target-app config", okLabel: "Generate" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Generating…", async () => {
        try {
          const r = await api("POST", "/api/target-app/generate-instructions",
                              { kind: "all" });
          if (r.ok && r.config) {
            setSettingsTab("application");
            applyGeneratedTargetAppConfig(r.config);
            toast("Generated — review and click Save application to persist", "info");
          } else {
            toast("Generation produced no configuration", "error");
          }
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $$("[data-runner-cache-rebuild]").forEach((b) => {
    b.addEventListener("click", async () => {
      const ok = await modalConfirm(
        "Rebuild the SQLite cache from canonical .refine JSON? If the existing database is corrupted, Refine will replace it and SQLite-only runtime history may be lost.",
        { title: "Rebuild SQLite cache", okLabel: "Rebuild" },
      );
      if (!ok) return;
      await withButtonBusy(b, "Rebuilding…", async () => {
        try {
          let result = await api("POST", "/api/cache/rebuild", { background: true });
          if (result.job) {
            drawSqliteCacheProgress(result.job.progress || {});
            result = await waitForBackgroundJob(result.job, {
              onProgress: drawSqliteCacheProgress,
            });
            if (result.http_status && result.http_status >= 400) {
              const raw = result.error || {};
              const err = new Error(raw.message || "SQLite cache rebuild failed");
              err.details = raw.details;
              err.code = raw.code;
              throw err;
            }
          }
          toast("SQLite cache rebuilt", "info");
          await refreshProcessesSettingsTab({ force: true });
        } catch (e) { await showActionError(e, "SQLite cache rebuild failed"); }
      });
    });
  });
  $$("[data-runner-log-cleanup]").forEach((b) => {
    b.addEventListener("click", async () => {
      const select = b.parentElement?.querySelector("[data-runner-log-cleanup-days]");
      const days = parseInt(select?.value || "7", 10);
      const human = days === 0
        ? "Delete ALL activity log entries? This cannot be undone."
        : `Delete activity log entries older than ${days} day${days === 1 ? "" : "s"}? This cannot be undone.`;
      const ok = await modalConfirm(human, {
        title: "Clean up old logs",
        okLabel: days === 0 ? "Delete all" : "Delete",
        danger: true,
      });
      if (!ok) return;
      await withButtonBusy(b, "Cleaning…", async () => {
        try {
          const r = await api("POST", "/api/activity/cleanup", { days });
          toast(`Deleted ${r.deleted} log entr${r.deleted === 1 ? "y" : "ies"}.`, "info");
          await refreshProcessesSettingsTab({ force: true });
        } catch (e) { await showActionError(e); }
      });
    });
  });
  $("#s-target-run-start")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-start");
    await withButtonBusy(btn, "Starting…", async () => {
      await runTargetAppAction("start");
      await refreshProcessesSettingsTab({ force: true });
    });
  });
  $("#s-target-run-stop")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-stop");
    await withButtonBusy(btn, "Stopping…", async () => {
      await runTargetAppAction("stop");
      await refreshProcessesSettingsTab({ force: true });
    });
  });
  $("#s-target-run-rebuild")?.addEventListener("click", async () => {
    const btn = $("#s-target-run-rebuild");
    await withButtonBusy(btn, "Rebuilding…", async () => {
      await runTargetAppAction("rebuild");
      await refreshProcessesSettingsTab({ force: true });
    });
  });
  $("#s-target-sync-now")?.addEventListener("click", async () => {
    const btn = $("#s-target-sync-now");
    await withButtonBusy(btn, "Syncing…", async () => {
      await syncProjectUpdates();
      await refreshReporters({ selectFallback: true });
      await refreshProcessesSettingsTab({ force: true });
    });
  });
  $("#s-target-health-now")?.addEventListener("click", async () => {
    const btn = $("#s-target-health-now");
    await withButtonBusy(btn, "Probing…", async () => {
      try {
        const r = await api("POST", "/api/target-app/health");
        const ok = "last_check_ok" in r ? r.last_check_ok : r.last_health_ok;
        toast(ok ? "Status check OK" : (r.probe_message || "Unhealthy"),
              ok ? "info" : "error");
        drawTargetAppStatusBlock(r);
        await refreshProcessesSettingsTab({ force: true });
      } catch (e) { await showActionError(e); }
    });
  });
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
