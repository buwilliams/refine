// ---- Target application status (topbar indicator + System controls) --------
//
// The topbar dot is a *read-only* status indicator (deliberately not a
// one-click toggle, so typical users can't take the app down by
// accident). It links to System, where the actual Start / Stop
// button lives. The dot's colour reflects the current state
// (green=running, red=stopped, amber=in-flight, grey=unknown) and is
// refreshed via SSE plus a 30s safety poll in case SSE is wedged.

let _targetAppSnapshot = null;

function initTargetAppToggle() {
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  refreshTargetAppToggle();
  // Backup poll so the dot isn't stale if SSE drops.
  setInterval(refreshTargetAppToggle, 30000);
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

function applyTargetAppSnapshot(snap) {
  _targetAppSnapshot = snap;
  const indicator = document.getElementById("target-app-indicator");
  if (!indicator) return;
  const appState = snap.state || "unknown";
  indicator.dataset.state = appState;
  const label = {
    running:  "App: running",
    degraded: "App: degraded",
    stopped:  "App: stopped",
    starting: "App: starting…",
    stopping: "App: stopping…",
    failed:   "App: failed",
    unknown:  "App: unknown",
  }[appState] || "App";
  const checkAt = snap.last_check_at || snap.last_health_at || "";
  const checkOk = "last_check_ok" in snap ? snap.last_check_ok : snap.last_health_ok;
  indicator.title = label
    + (checkAt
        ? ` · last check ${checkOk ? "OK" : "FAIL"} at ${fmtTime(checkAt)}`
        : "")
    + (snap.last_error ? ` · ${snap.last_error}` : "")
    + " — click to manage in System";
  const lbl = indicator.querySelector(".target-app-label");
  if (lbl) lbl.textContent = label.replace(/^App: /, "");
  // Repaint the System status block (and the start/stop button)
  // whenever the System screen is visible.
  if (state.currentRoute === "settings") {
    drawTargetAppStatusBlock(snap);
  }
}

async function runTargetAppAction(action) {
  // action is "start" or "stop". Called from the buttons on System.
  const snap = _targetAppSnapshot || {};
  const hasPrompt = action === "start"
    ? snap.has_start_command
    : snap.has_stop_command;
  if (!hasPrompt) {
    toast(
      `No ${action} command configured. Set it above, then click Save.`,
      "error",
    );
    return;
  }
  const isStop = action === "stop";
  const ok = await modalConfirm(
    isStop
      ? "Stop the target application now?"
      : "Start the target application now? Refine will run the saved start command on the host.",
    { title: isStop ? "Stop application" : "Start application",
      okLabel: isStop ? "Stop" : "Start",
      danger: isStop },
  );
  if (!ok) return;
  // Optimistic UI flip so the dot transitions immediately.
  applyTargetAppSnapshot({
    ..._targetAppSnapshot,
    state: isStop ? "stopping" : "starting",
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
    toast(e.message, "error");
    // Reset to whatever the server thinks.
    refreshTargetAppToggle();
  }
}
