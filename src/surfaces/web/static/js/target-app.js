// ---- Target application status (topbar indicator + Node controls) ----------
//
// The topbar dot is a *read-only* status indicator (deliberately not a
// one-click toggle, so typical users can't take the app down by
// accident). Green/running links to the configured App URL when present;
// every other state links to Node, where the Start / Stop controls live.
// The visible label names the active project.

let _targetAppSnapshot = null;
let _agentStatusRefreshTimer = null;

function targetAppProjectLabel() {
  const project = state.project || {};
  if (project.attached === false) return "No app";
  const current = project.target_root || "";
  const apps = Array.isArray(project.apps) ? project.apps : [];
  const app = apps.find((candidate) => candidate.path === current);
  if (app?.name) return app.name;
  if (current) {
    return current.split(/[\\/]+/).filter(Boolean).pop() || current;
  }
  return "Project";
}

function initTargetAppToggle() {
  const indicator = document.getElementById("target-app-indicator");
  if (indicator) refreshTargetAppToggle();
  refreshAgentStatusIndicator();
}

async function refreshTargetAppToggle() {
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  if (!hasAttachedProject()) {
    applyNoTargetAppSnapshot();
    return;
  }
  try {
    const snap = await api("GET", "/api/target-app/status");
    applyTargetAppSnapshot(snap);
  } catch {
    // Leave whatever state the dot was showing; we'll retry on the next tick.
  }
}

function applyNoTargetAppSnapshot() {
  applyTargetAppSnapshot({
    state: "unknown",
    app_url: "",
    last_check_at: "",
    last_health_at: "",
    last_error: "",
  });
}

function scheduleAgentStatusRefresh() {
  if (_agentStatusRefreshTimer) return;
  _agentStatusRefreshTimer = setTimeout(() => {
    _agentStatusRefreshTimer = null;
    refreshAgentStatusIndicator();
  }, 250);
}

async function refreshAgentStatusIndicator() {
  const indicator = document.getElementById("agent-status-indicator");
  if (!indicator) return;
  if (!hasAttachedProject()) {
    applyAgentStatusSnapshot({
      runner_reachable: false,
      paused: false,
      processes: [],
      error: "No app configured",
    });
    return;
  }
  try {
    const snap = await api("GET", "/api/processes?summary=1");
    applyAgentStatusSnapshot(snap);
  } catch {
    applyAgentStatusSnapshot({
      runner_reachable: false,
      paused: false,
      processes: [],
      error: "Process status unavailable",
    });
  }
}

function applyAgentStatusSnapshot(snap) {
  const indicator = document.getElementById("agent-status-indicator");
  if (!indicator) return;
  const processes = Array.isArray(snap.processes) ? snap.processes : [];
  const agentCount = Number.isFinite(snap.agent_count)
    ? snap.agent_count
    : processes.filter((proc) => proc.kind === "agent").length;
  const status = !snap.runner_reachable
    ? "down"
    : snap.paused
      ? "paused"
      : "running";
  indicator.dataset.state = status;
  indicator.href = "#/node/processes";
  indicator.removeAttribute("target");
  indicator.removeAttribute("rel");
  const label = `Agents (${agentCount})`;
  const statusLabel = {
    running: "running",
    paused: "paused",
    down: "process down",
  }[status];
  const lbl = indicator.querySelector(".agent-status-label");
  if (lbl) lbl.textContent = `${statusLabel} · ${agentCount}`;
  indicator.setAttribute("aria-label", `${label}: ${statusLabel}; click to view processes`);
  indicator.title = `${label}: ${statusLabel}`;
}

function applyTargetAppSnapshot(snap) {
  _targetAppSnapshot = snap;
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  const appState = snap.state === "running" && snap.has_status_checks === false
    ? "unknown"
    : snap.state || "unknown";
  indicator.dataset.state = appState;
  const projectLabel = targetAppProjectLabel();
  const stateLabel = {
    running: "running",
    degraded: "degraded",
    stopped: "stopped",
    starting: "starting…",
    building: "building…",
    stopping: "stopping…",
    failed: "failed",
    unknown: "unknown",
  }[appState] || "unknown";
  const stateEl = indicator.querySelector(".target-app-state");
  if (stateEl) stateEl.textContent = stateLabel;
  const checkAt = snap.last_check_at || snap.last_health_at || "";
  const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
  const appUrl = (snap.app_url || "").trim();
  const opensApp = appState === "running" && appUrl;
  indicator.href = opensApp ? appUrl : "#/node/processes";
  if (opensApp) {
    indicator.target = "_blank";
    indicator.rel = "noopener noreferrer";
    indicator.setAttribute("aria-label", "Open target application");
  } else {
    indicator.removeAttribute("target");
    indicator.removeAttribute("rel");
    indicator.setAttribute("aria-label", "Target application status (click to manage)");
  }
  indicator.title = `${projectLabel}: ${stateLabel}`
    + (checkAt
        ? ` · last check ${checkOk ? "OK" : "FAIL"} at ${fmtTime(checkAt)}`
        : "")
    + (snap.last_error ? ` · ${snap.last_error}` : "")
    + (opensApp ? " — open target application" : " — click to manage in Node");
  const lbl = indicator.querySelector(".target-app-label");
  if (lbl) lbl.textContent = projectLabel;
  // Repaint the Node process block (and the start/stop button) whenever it is visible.
  if (state.currentRoute === "node" && typeof readSettingsTab === "function" && readSettingsTab() === "processes") {
    drawTargetAppStatusBlock(snap);
  }
}

async function runTargetAppAction(action) {
  // action is "start", "stop", or "build". Called from the buttons on System.
  const snap = _targetAppSnapshot || {};
  const hasPrompt = action === "start"
    ? (snap.has_start_action ?? snap.has_start_instructions ?? snap.has_start_command)
    : action === "stop"
      ? (snap.has_stop_action ?? snap.has_stop_instructions ?? snap.has_stop_command)
      : (snap.has_build_action ?? snap.has_build_instructions ?? snap.has_build_command);
  const isStop = action === "stop";
  const isBuild = action === "build";
  const noCommand = !hasPrompt;
  const ok = await modalConfirm(
    isStop
      ? (noCommand
          ? "No stop instructions are configured. Continue with a no-op?"
          : "Stop the target application now?")
      : isBuild
        ? (noCommand
            ? "No build instructions are configured. Queue the build anyway?"
            : "Build the target application now? Refine will ask the configured agent to handle the build on the host.")
        : (noCommand
            ? "No start instructions are configured. Continue with a no-op?"
            : "Start the target application now? Refine will ask the configured agent to start it on the host."),
    { title: isStop ? "Stop application" : (isBuild ? "Build application" : "Start application"),
      okLabel: isStop ? "Stop" : (isBuild ? "Build" : "Start"),
      danger: isStop },
  );
  if (!ok) return;
  // Optimistic UI flip so the dot transitions immediately.
  applyTargetAppSnapshot({
    ..._targetAppSnapshot,
    state: isStop ? "stopping" : (isBuild ? "building" : "starting"),
  });
  try {
    const queued = await api("POST", `/api/target-app/${action}`);
    const r = await resolveBackgroundOperationResponse(
      queued,
      `Target application ${action} is running in the background`,
    );
    if (isBuild && r.queued !== undefined) {
      toast(r.queued ? "Target application build queued" : "Target application build was not queued", r.queued ? "info" : "warn");
      await refreshTargetAppToggle();
      return;
    }
    toast(r.message || `${action} completed`, r.ok ? "info" : "error");
    applyTargetAppSnapshot({
      ..._targetAppSnapshot,
      state: r.state || (isStop ? "stopped" : "running"),
      last_error: r.ok ? "" : (r.message || ""),
    });
  } catch (e) {
    await showActionError(e, "Target app action failed");
    // Reset to whatever the server thinks.
    refreshTargetAppToggle();
  }
}
