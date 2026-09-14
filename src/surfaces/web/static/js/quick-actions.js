// Quick Actions uses the same snapshots and commands as the Processes screen.
const quickActionBusy = {target: false, workflow: false};
let quickWorkflowSnapshot = null;

function quickActionButton(action, label, icon, disabled = false) {
  const shapes = {
    start: '<path d="m7 4 14 8-14 8V4Z"></path>',
    pause: '<path d="M8 5v14M16 5v14"></path>',
    stop: '<rect x="5" y="5" width="14" height="14" rx="1"></rect>',
    resume: '<path d="m5 4 12 8-12 8V4ZM20 5v14"></path>',
  };
  return `<button type="button" data-quick-action="${action}" aria-label="${htmlEscape(label)}" title="${htmlEscape(label)}"${disabled ? " disabled" : ""}><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24" focusable="false">${shapes[icon]}</svg></button>`;
}

function quickConfigureLink(destination, label) {
  return `<a class="quick-configure" href="${destination}" aria-label="${label}" title="${label}"><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24" focusable="false"><path d="M4 7h16M4 17h16"></path><circle cx="9" cy="7" r="3"></circle><circle cx="15" cy="17" r="3"></circle></svg></a>`;
}

function renderQuickTargetActions(snap) {
  const root = document.getElementById("quick-target-actions");
  if (!root) return;
  const running = ["running", "degraded"].includes(snap.state);
  const action = running ? "stop" : "start";
  const configured = action === "stop"
    ? (snap.has_stop_action ?? snap.has_stop_instructions ?? snap.has_stop_command)
    : (snap.has_start_action ?? snap.has_start_instructions ?? snap.has_start_command);
  const label = `${running ? "Stop" : "Start"} target application${configured ? "" : " — configure instructions in Settings first"}`;
  const disabled = quickActionBusy.target || !hasAttachedProject() || !configured || ["starting", "stopping", "building"].includes(snap.state);
  const generation = captureNodeContextGeneration();
  renderInto(root, quickActionButton(action, label, action, disabled) + quickConfigureLink("#/settings/target-app", "Configure target application"), () => {
    root.querySelector("button").onclick = async () => {
      if (disabled || quickActionBusy.target || !isNodeContextGenerationCurrent(generation)) return;
      quickActionBusy.target = true;
      renderQuickTargetActions(snap);
      try { await runTargetAppAction(action); }
      finally {
        quickActionBusy.target = false;
        await refreshTargetAppToggle();
      }
    };
  });
}

function renderQuickWorkflowActions(snap) {
  quickWorkflowSnapshot = snap;
  const root = document.getElementById("quick-workflow-actions");
  if (!root) return;
  const worker = (snap.background_workers || []).find(item => item.worker_kind === "workflow");
  const available = worker?.management_actions || [];
  const canStop = available.includes("stop_background_worker");
  const canStart = available.includes("start_background_worker");
  const paused = workflowPausedFor(snap);
  const disabled = quickActionBusy.workflow || !hasAttachedProject() || !!snap.error;
  const knownPause = typeof snap.paused === "boolean" || typeof snap.workflow_paused === "boolean";
  const html = quickActionButton(paused ? "unpause" : "pause", paused ? "Unpause workflow" : "Pause workflow", paused ? "resume" : "pause", disabled || !knownPause)
    + quickActionButton(canStop ? "stop" : "start", canStop ? "Stop workflow worker" : "Start workflow worker", canStop ? "stop" : "start", disabled || !(canStop || canStart))
    + quickConfigureLink("#/settings/workflow", "Configure workflow");
  const generation = captureNodeContextGeneration();
  renderInto(root, html, () => {
    root.querySelectorAll("button").forEach(button => { button.onclick = () => runQuickWorkflowAction(button, generation); });
  });
}

async function runQuickWorkflowAction(button, generation) {
  if (button.disabled || quickActionBusy.workflow || !isNodeContextGenerationCurrent(generation)) return;
  const action = button.dataset.quickAction;
  quickActionBusy.workflow = true;
  renderQuickWorkflowActions(quickWorkflowSnapshot);
  try {
    if (action === "pause") {
      const model = workflowPauseActionModel({workflow_paused: false});
      if (!await modalConfirm(model.confirmation, {title: "Pause Workflow", okLabel: "Pause Workflow", danger: true})) return;
    }
    if (!isNodeContextGenerationCurrent(generation)) return;
    if (action === "pause" || action === "unpause") {
      await api("POST", "/api/workflow/pause", {paused: action === "pause"});
    } else {
      await api("POST", `/api/processes/background-workers/workflow/${action}`, {});
    }
  } catch (error) { await showActionError(error); }
  finally {
    quickActionBusy.workflow = false;
    await refreshAgentStatusIndicator();
    if (isNodeContextGenerationCurrent(generation) && isSettingsRoute() && readSettingsTab() === "processes") await refreshProcessesSettingsTab({force: true});
  }
}
