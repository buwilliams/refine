// ---- Target application status (topbar indicator + System controls) --------
//
// The topbar dot is a *read-only* status indicator (deliberately not a
// one-click toggle, so typical users can't take the app down by
// accident). Green/running links to the configured App URL when present;
// every other state links to System, where the Start / Stop controls live.
// The visible label names the active project.

let _targetAppSnapshot = null;
let _agentStatusRefreshTimer = null;

function targetAppProjectLabel() {
  const project = state.project || {};
  const current = project.client_repo || "";
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
  // Backup poll so the dot isn't stale if SSE drops.
  setInterval(refreshTargetAppToggle, 30000);
  setInterval(refreshAgentStatusIndicator, 30000);
}

async function refreshTargetAppToggle() {
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  try {
    const snap = await api("GET", "/api/target-app/status");
    applyTargetAppSnapshot(snap);
  } catch {
    // Leave whatever state the dot was showing; we'll retry on the next tick.
  }
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
  try {
    const snap = await api("GET", "/api/processes");
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
  const agentCount = processes.filter((proc) => proc.kind === "agent").length;
  const status = !snap.runner_reachable
    ? "down"
    : snap.paused
      ? "paused"
      : "running";
  indicator.dataset.state = status;
  indicator.href = "#/system/processes";
  indicator.removeAttribute("target");
  indicator.removeAttribute("rel");
  const label = `Agents (${agentCount})`;
  const compactLabel = String(agentCount);
  const statusLabel = {
    running: "running",
    paused: "paused",
    down: "process down",
  }[status];
  const lbl = indicator.querySelector(".agent-status-label");
  if (lbl) lbl.textContent = compactLabel;
  indicator.setAttribute("aria-label", `${label}: ${statusLabel}; click to view processes`);
  indicator.title = `${label}: ${statusLabel}`;
}

function applyTargetAppSnapshot(snap) {
  _targetAppSnapshot = snap;
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  const appState = snap.state || "unknown";
  indicator.dataset.state = appState;
  const contextMenu = document.getElementById("nav-context-menu");
  if (contextMenu) contextMenu.dataset.state = appState;
  const projectLabel = targetAppProjectLabel();
  if (typeof updateNavAppContextLabel === "function") updateNavAppContextLabel(projectLabel);
  const stateLabel = {
    running: "running",
    degraded: "degraded",
    stopped: "stopped",
    starting: "starting…",
    rebuilding: "rebuilding…",
    stopping: "stopping…",
    failed: "failed",
    unknown: "unknown",
  }[appState] || "unknown";
  const checkAt = snap.last_check_at || snap.last_health_at || "";
  const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
  const appUrl = (snap.app_url || "").trim();
  const opensApp = appState === "running" && appUrl;
  indicator.href = opensApp ? appUrl : "#/system/processes";
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
    + (opensApp ? " — open target application" : " — click to manage in System");
  const lbl = indicator.querySelector(".target-app-label");
  if (lbl) lbl.textContent = projectLabel;
  // Repaint the System status block (and the start/stop button)
  // whenever the System screen is visible.
  if (state.currentRoute === "settings") {
    drawTargetAppStatusBlock(snap);
  }
}

async function runTargetAppAction(action) {
  // action is "start", "stop", or "rebuild". Called from the buttons on System.
  const snap = _targetAppSnapshot || {};
  const hasPrompt = action === "start"
    ? snap.has_start_command
    : action === "stop"
      ? snap.has_stop_command
      : snap.has_rebuild_command;
  const isStop = action === "stop";
  const isRebuild = action === "rebuild";
  const noCommand = !hasPrompt;
  const ok = await modalConfirm(
    isStop
      ? (noCommand
          ? "No stop command is configured. Continue with a no-op?"
          : "Stop the target application now?")
      : isRebuild
        ? (noCommand
            ? "No rebuild command is configured. Continue and mark awaiting-rebuild Gaps ready for review?"
            : "Rebuild the target application now? Refine will run the saved rebuild command on the host.")
        : (noCommand
            ? "No start command is configured. Continue with a no-op?"
            : "Start the target application now? Refine will run the saved start command on the host."),
    { title: isStop ? "Stop application" : (isRebuild ? "Rebuild application" : "Start application"),
      okLabel: isStop ? "Stop" : (isRebuild ? "Rebuild" : "Start"),
      danger: isStop },
  );
  if (!ok) return;
  // Optimistic UI flip so the dot transitions immediately.
  applyTargetAppSnapshot({
    ..._targetAppSnapshot,
    state: isStop ? "stopping" : (isRebuild ? "rebuilding" : "starting"),
  });
  try {
    const r = await api("POST", `/api/target-app/${action}`);
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
