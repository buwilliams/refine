// ---- Goals: detail -----------------------------------------------------------

// Goal detail is rendered as a modal layered over whatever screen the user
// was on (Dashboard, Goals list, etc.) so that opening a Goal doesn't blow
// away context. `navigate()` handles the `#/goals/<id>` route by calling
// `openGoalDetailModal` directly — so `renderGoalDetail` is no longer wired
// into the routes table. Kept as a thin wrapper in case any callers find
// it useful later.
async function renderGoalDetail(r) {
  openGoalDetailModal(r.id);
}

let _goalModalRoot = null;
let _goalRoundFormDraft = null;
// The goal and workflow most recently drawn. The detail controls are bound once
// and outlive the render that bound them, so they resolve the goal and its
// available transitions through here instead of closing over one render's values
// — the status, name, and workflow arrows all change as the goal moves.
let _goalDetailView = { goal: null, workflow: null };
const _loggedFeatureBlockingNoticeKeys = new Set();

function goalDetailContainer() {
  return _goalModalRoot?.querySelector(".goal-detail-modal-body") || null;
}

function openGoalDetailModal(goalId) {
  // Make sure something is underneath. On a cold-load deep link (e.g. user
  // pastes `#/goals/abc123` into a new tab), `#main` is empty — paint the
  // dashboard underneath so dismissing the modal leaves the user on a
  // sensible page.
  ensureGoalModalUnderlay();

  if (_goalModalRoot) {
    // Modal is already open — swap the body to the new goal.
    const body = _goalModalRoot.querySelector(".goal-detail-modal-body");
    if (body) body.innerHTML = `<p class="muted">Loading…</p>`;
    loadGoalDetail(goalId);
    return;
  }

  const root = document.createElement("div");
  root.className = "modal-backdrop goal-detail-backdrop";
  root.innerHTML = `
    <div class="modal goal-detail-modal" role="dialog" aria-modal="true"
         aria-label="Goal detail">
      <button class="modal-close" type="button" aria-label="Close">×</button>
      <div class="goal-detail-modal-body"><p class="muted">Loading…</p></div>
    </div>
  `;
  document.body.appendChild(root);
  _goalModalRoot = root;

  function onKey(e) {
    if (e.key === "Escape") { e.preventDefault(); dismiss(); }
  }
  function dismiss() {
    closeGoalDetailModal({ navigateAway: true });
  }
  document.addEventListener("keydown", onKey, true);
  root._cleanup = () => document.removeEventListener("keydown", onKey, true);
  bindOnce(root, "click", (e) => {
    if (e.target === root) dismiss();
  });
  bindOnce(root.querySelector(".modal-close"), "click", dismiss);

  loadGoalDetail(goalId);
}

function closeGoalDetailModal({ navigateAway = false } = {}) {
  if (!_goalModalRoot) return;
  _goalModalRoot._cleanup?.();
  _goalModalRoot.remove();
  _goalModalRoot = null;
  _goalRoundFormDraft = null;
  state.currentGoal = null;
  state.currentGoalData = null;
  if (navigateAway) {
    // Restore the URL to whatever was underneath. If we're already there
    // somehow (shouldn't happen), no-op so we don't trigger a redundant
    // re-render.
    const target = state.underlayHash || "#/";
    if (location.hash !== target) location.hash = target;
    else state.currentRoute = parseHash().route;
  }
}

function ensureGoalModalUnderlay() {
  const main = $("#main");
  if (main && main.innerHTML.trim()) return;
  // Paint the dashboard underneath. We don't change `state.currentRoute`
  // here — the caller will set it to "goals_detail" — but the dashboard's
  // render is keyed off DOM state, not route, so this works.
  renderDashboard();
}

async function loadGoalDetail(goalId) {
  const nodeGeneration = captureNodeContextGeneration();
  try {
    const { goal } = await api("GET", "/api/goals/" + goalId);
    if (!isNodeContextGenerationCurrent(nodeGeneration)
        || state.currentGoal !== goalId || goal?.id !== goalId) return;
    drawGoalDetail(goal);
  } catch (e) {
    if (!isNodeContextGenerationCurrent(nodeGeneration) || state.currentGoal !== goalId) return;
    const container = goalDetailContainer();
    if (container) {
      container.innerHTML = `<p class="muted">Could not load Goal: ${htmlEscape(e.message)}</p>`;
    }
  }
}

// User-driven workflow transitions for a Goal. Each state declares its
// `back` and `forward` neighbors. System-owned states have no user buttons —
// Plan, Implement, Quality, and Governance are system-owned and have no
// user-driven status buttons.
// Forward from `review` goes through the dedicated /approve endpoint for
// approval. Declining review requires the new-round form so the next attempt
// has a distinct identity and starts from the current integrated target.
//
// failed / cancelled only expose a back arrow — there's no obvious
// forward target for them (they're terminal-ish in opposite directions
// from done). Failed Goals normally go back to todo and rerun; candidate-stage
// failures use the latest workflow transition to requeue the isolated branch.
const GOAL_WORKFLOW = {
  backlog:      { forward: { label: "Todo →",     next: "todo"   } },
  todo:         { back:    { label: "← Backlog",  next: "backlog" } },
  // plan / implement / quality / governance: automation owns these states.
  review:       { forward: { label: "Approve →",  next: "done", approve: true } },
  done:         { back:    { label: "← Review",   next: "review" } },
  failed:       { back:    { label: "← Todo",     next: "todo"   } },
  cancelled:    { back:    { label: "← Todo",     next: "todo"   } },
};

function workflowForGoal(goal, latest) {
  if (goal.status === "failed" && isQualityRetryGoal(latest)) {
    return { back: { label: "← Quality", next: "quality", retryQuality: true } };
  }
  if (goal.status === "failed" && isMergeRetryGoal(latest)) {
    return { back: { label: "← Governance", next: "governance", retryGovernance: true } };
  }
  return GOAL_WORKFLOW[goal.status] || {};
}

function isQualityRetryGoal(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("quality") &&
         message.includes("failed");
}

function isMergeRetryGoal(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("governance") &&
         message.includes("failed");
}

function currentRoundLog(log, workflowLog) {
  if (!log) return null;
  const logDatetime = String(log.datetime || "");
  const workflowDatetime = String(workflowLog?.datetime || "");
  if (logDatetime && workflowDatetime && logDatetime < workflowDatetime) {
    return null;
  }
  return log;
}

function latestStateBoundary(latest) {
  const stateLog = latest?.latest_state_log;
  const workflowLog = latest?.latest_workflow_log;
  if (!stateLog) return workflowLog || null;
  if (!workflowLog) return stateLog;
  const stateDatetime = String(stateLog.datetime || "");
  const workflowDatetime = String(workflowLog.datetime || "");
  return !stateDatetime || !workflowDatetime || stateDatetime >= workflowDatetime
    ? stateLog
    : workflowLog;
}

function renderGoalFeatureAssociation(goal) {
  const feature = goal.feature_id
    ? `<a href="#/features/${encodeURIComponent(goal.feature_id)}">${htmlEscape(goal.feature_id)}</a>${goal.feature_order ? ` · order ${goal.feature_order}` : ""}`
    : `<span class="muted">Standalone</span>`;
  return `
    <div class="goal-feature-row muted small" style="margin-bottom:14px" data-testid="goal-feature-association">
      Feature ${feature}
    </div>`;
}

function goalTransferToActiveNodeTarget(goal) {
  const activeNodeId = String(nodeContextActiveNodeId() || "").trim();
  const ownerNodeId = String(goal?.node_id || "default").trim();
  if (!activeNodeId || activeNodeId.toLowerCase() === ownerNodeId.toLowerCase()) {
    return "";
  }
  return activeNodeId;
}

function renderGoalTransferToActiveNodeAction(goal) {
  if (!goalTransferToActiveNodeTarget(goal)) return "";
  return `<button class="nav-menu-item" type="button" id="btn-transfer-to-active-node" data-testid="goal-action-transfer-to-my-node">Transfer to my node</button>`;
}

async function transferGoalToActiveNode(goal) {
  const targetNodeId = goalTransferToActiveNodeTarget(goal);
  if (!targetNodeId || !goal?.id) return;
  try {
    await api("POST", "/api/nodes/transfer-goals", {
      item_id: goal.id,
      target_node_id: targetNodeId,
    });
    toast("Transferred to my node", "info");
    await loadGoalDetail(goal.id);
  } catch (error) {
    await showActionError(error, "Transfer failed");
  }
}

async function openGoalFeatureAssignModal(goal) {
  const data = await api("GET", "/api/features?limit=100&node=current");
  const features = (data.features || []).map((entry) => {
    if (typeof normalizeFeatureEntry === "function") {
      return normalizeFeatureEntry(entry);
    }
    const feature = { ...(entry?.feature || entry || {}) };
    const rollup = entry?.rollup || feature.rollup || {};
    feature.status = feature.status || rollup.status || "backlog";
    feature.goal_count = feature.goal_count ?? rollup.goal_count ?? (entry?.goal_ids || feature.goal_ids || []).length;
    feature.done_count = feature.done_count ?? rollup.done_count ?? 0;
    return feature;
  });
  if (!features.length) {
    await modalAlert("Create a Feature before assigning this Goal.", {
      title: "Assign Feature",
    });
    return;
  }
  const body = () => `
    <div class="modal-title">${goal.feature_id ? "Move to Feature" : "Assign to Feature"}</div>
    <div class="modal-body">
      <label>Feature</label>
      <select class="modal-input" data-testid="goal-feature-select">
        ${features.map((feature) => `
          <option value="${htmlEscape(feature.id)}" ${feature.id === goal.feature_id ? "selected" : ""}>
            ${htmlEscape(feature.name || feature.id)} · ${htmlEscape(feature.status || "backlog")} · ${feature.done_count || 0}/${feature.goal_count || 0} done
          </option>`).join("")}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="modal-cancel">Cancel</button>
      <button data-ok data-testid="modal-ok">${goal.feature_id ? "Move" : "Assign"}</button>
    </div>`;
  const featureId = await _openModal(body, { cancel: null, ok: "" }, ".modal-input");
  if (!featureId || featureId === goal.feature_id) return;
  try {
    await api("POST", `/api/features/${encodeURIComponent(featureId)}/goals/${encodeURIComponent(goal.id)}`);
    toast(goal.feature_id ? "Goal moved to Feature" : "Goal assigned to Feature", "info");
  } catch (e) {
    showActionError(e, "Assign Feature failed");
  }
}

function drawGoalDetail(goal) {
  if (!goal) return;
  if (_goalRoundFormDraft && _goalRoundFormDraft.goalId !== goal.id) {
    _goalRoundFormDraft = null;
  }
  const hadEditRoundForm = !!document.querySelector('#round-form[data-kind="edit"]');
  const roundFormDraft = captureRoundFormDraft(goal.id);
  if (roundFormDraft) {
    _goalRoundFormDraft = roundFormDraft;
  } else if (hadEditRoundForm && _goalRoundFormDraft?.goalId === goal.id) {
    _goalRoundFormDraft = null;
  }
  state.currentGoalData = goal;
  renderBanners([]);
  // Preserve the notes-card open state across re-renders of the same goal so
  // saving a note (or an SSE-driven refresh) doesn't snap it shut.
  const notesOpen = document.querySelector(
    `.notes-card[data-goal-id="${goal.id}"]`,
  )?.open ?? false;
  // Same idea for the per-round wrapper. Metadata refreshes still redraw the
  // modal; preserve expanded sections so status or project updates do not
  // collapse the user's working context.
  const prevRoundOpen = {};
  document.querySelectorAll("details.round[data-round-idx]").forEach((el) => {
    prevRoundOpen[el.dataset.roundIdx] = el.open;
  });
  const prevPlanHistoryOpen = {};
  document.querySelectorAll("details[data-plan-history][data-round-idx]").forEach((el) => {
    prevPlanHistoryOpen[`${el.dataset.roundIdx}:${el.dataset.planHistory}`] = el.open;
  });
  const rounds = goal.rounds || [];
  const latest = rounds[rounds.length - 1] || null;
  const failureBanner = computeFailureBanner(goal, latest);
  const governanceBanner = computeGovernanceBanner(goal, latest);
  const featureBlockingNotice = computeFeatureBlockingNotice(goal);
  const nodeDisplayName = goal.node_display_name || goal.node_id || "Unknown";
  const nodeOwnerTitle = goal.node_id
    ? `Node owner: ${nodeDisplayName} (${goal.node_id})`
    : `Node owner: ${nodeDisplayName}`;

  const isLatestEditable = (goal.status === "backlog" ||
                            goal.status === "todo");
  const canSubmitNewRound = (goal.status === "review" ||
                             goal.status === "failed");
  const hasPreservedDraft = hasPreservedRoundFormDraft(goal.id);
  const cancelEnabled = !["done", "cancelled"].includes(goal.status);
  // During implementation this attaches to the workflow-owned Goal Agent.
  // Outside implementation it opens a separate diagnostic session whose stop
  // lifecycle cannot requeue, cancel, or otherwise mutate the Goal.
  const canOpenAgent = !!goal.id;
  const openAgentTitle = ["plan", "implement", "quality"].includes(goal.status)
    ? "Attach to the running Goal Agent"
    : "Open a diagnostic Agent with this Goal's recorded context";

  // Dynamic workflow buttons: each state shows the previous/next state
  // it can move to as back / forward buttons. The user-driven workflow
  // skips system-owned statuses. Forward from review goes through the
  // `approve` endpoint. Review rejection uses the new-round form rather than
  // an unversioned status update.
  const workflow = workflowForGoal(goal, latest);
  const backBtn = workflow.back ? `
    <button id="btn-state-back" data-testid="goal-state-back">${htmlEscape(workflow.back.label)}</button>
  ` : "";
  const forwardBtn = workflow.forward ? `
    <button id="btn-state-forward" data-testid="goal-state-forward">${htmlEscape(workflow.forward.label)}</button>
  ` : "";

  const container = goalDetailContainer();
  if (!container) return;
  // `<details>` open state is local UI state the server knows nothing about, so
  // it has to be rendered back or the next refresh closes it under the user.
  const actionMenuOpen = !!container.querySelector("#goal-action-menu")?.open;
  const noteComposerOpen = !!container.querySelector(".note-composer")?.open;
  _goalDetailView = { goal, workflow };
  recordFeatureBlockingNotice(goal, featureBlockingNotice);
  renderInto(container, `
    <div class="goal-detail" data-testid="goal-detail">
      <div class="row" style="align-items:center;margin-bottom:8px">
        <h2 style="margin:0" data-testid="goal-title">${htmlEscape(goal.name)}</h2>
        <span class="status-pill ${goal.status}" data-testid="goal-status-pill">${workflowStatusLabel(goal.status)}</span>
        <span class="priority-pill priority-${goal.priority || "low"}" data-testid="goal-priority-pill">priority: ${goal.priority || "low"}</span>
      </div>
      <div class="actions" style="margin-bottom:10px" data-testid="goal-workflow-actions">
        ${backBtn}
        ${forwardBtn}
        <div class="goal-action-group">
          <button class="goal-action-primary" id="btn-open-agent" data-testid="goal-open-agent"
                  ${canOpenAgent ? "" : "disabled"}
                  title="${htmlEscape(openAgentTitle)}">Open Agent</button>
          <details class="nav-menu goal-action-menu" id="goal-action-menu"${actionMenuOpen ? " open" : ""}>
            <summary class="btn goal-action-more" aria-label="More Goal actions" data-testid="goal-action-menu-toggle"></summary>
            <div class="nav-menu-panel goal-action-panel">
              <button class="nav-menu-item" type="button" id="btn-watch-logs" data-testid="goal-action-watch-logs">Watch Logs</button>
              <button class="nav-menu-item" type="button" id="btn-reporter" data-testid="goal-action-reporter">Reporter</button>
              <button class="nav-menu-item" type="button" id="btn-assignee" data-testid="goal-action-assignee">Assignee</button>
              <button class="nav-menu-item" type="button" id="btn-rename" data-testid="goal-action-rename">Rename</button>
              <button class="nav-menu-item" type="button" id="btn-priority" data-testid="goal-action-priority">Change Priority</button>
              <button class="nav-menu-item" type="button" id="btn-goal-feature-assign" data-testid="goal-action-assign-feature">Move to Feature</button>
              <button class="nav-menu-item" type="button" id="btn-goal-feature-remove" data-testid="goal-action-remove-feature" ${goal.feature_id ? "" : "disabled"}>Remove from Feature</button>
              ${renderGoalTransferToActiveNodeAction(goal)}
              <button class="nav-menu-item" type="button" id="btn-cancel" data-testid="goal-action-cancel" ${cancelEnabled ? "" : "disabled"}>Cancel</button>
              <button class="nav-menu-item danger" type="button" id="btn-delete" data-testid="goal-delete">Delete</button>
            </div>
          </details>
        </div>
      </div>
      <div class="muted small" style="margin-bottom:14px" data-testid="goal-metadata">
        ID <code>${goal.id}</code> · created ${fmtTime(goal.created)} · updated ${fmtTime(goal.updated)} · node <span title="${htmlEscape(nodeOwnerTitle)}">${htmlEscape(nodeDisplayName)}</span>
        · reporter <strong>${htmlEscape(goal.reporter || "unreported")}</strong>
        · assignee <strong>${htmlEscape(goal.assignee || "unassigned")}</strong>
        ${goal.branch_name ? ` · branch <code>${goal.branch_name}</code>` : ""}
      </div>
      ${renderGoalFeatureAssociation(goal)}

      ${failureBanner ? `
        <div class="banner ${failureBanner.severity}" data-testid="goal-failure-banner">
          <span class="banner-msg" data-testid="goal-failure-banner-message">${htmlEscape(failureBanner.message)}</span>
          <span class="banner-actions">${failureBanner.actionsHtml}</span>
        </div>` : ""}
      ${governanceBanner ? `
        <div class="banner ${governanceBanner.severity}" data-testid="goal-governance-banner">
          <span class="banner-msg" data-testid="goal-governance-banner-message">${htmlEscape(governanceBanner.message)}</span>
        </div>` : ""}
      ${featureBlockingNotice ? `
        <div class="banner ${featureBlockingNotice.severity}" data-testid="goal-feature-blocking-banner">
          <span class="banner-msg" data-testid="goal-feature-blocking-banner-message">${htmlEscape(featureBlockingNotice.message)}</span>
        </div>` : ""}

      ${latest ? renderFailureSummary(goal, latest) : ""}
      ${latest ? renderGovernanceSummary(latest) : ""}
      ${latest ? renderQualitySummary(latest) : ""}

      <h3>Rounds (${rounds.length})</h3>
      ${rounds.length === 0 ? `<p class="muted">No rounds yet.</p>` :
        rounds.map((rnd, idx) => renderRound(
          rnd, idx,
          idx === rounds.length - 1,
          prevRoundOpen,
          prevPlanHistoryOpen,
        )).join("")}

      ${(isLatestEditable || hasPreservedDraft) ? `
        <div class="card" style="margin-top:14px">
          <h3>Edit latest round</h3>
          ${renderRoundForm("edit", latest, {
            draft: _goalRoundFormDraft,
            disabled: !isLatestEditable,
            formId: isLatestEditable ? "round-form" : "round-form-draft",
          })}
        </div>` : ""}

      ${canSubmitNewRound ? `
        <div class="card" style="margin-top:14px">
          <h3>${goal.status === "failed" ? "Submit recovery round" : "Submit follow-up round"}</h3>
          ${renderRoundForm("submit", null)}
        </div>` : ""}

      <details class="card notes-card" data-goal-id="${goal.id}" data-testid="goal-notes" style="margin-top:14px" ${notesOpen ? "open" : ""}>
        <summary class="notes-card-summary" data-testid="goal-notes-toggle">
          <span><strong>Notes (${(goal.notes || []).length})</strong></span>
          <span class="muted small">Saved to goal.json and included in Goal Agent context.</span>
          <span class="spacer"></span>
          <span id="goal-notes-status" class="muted small"></span>
        </summary>
        <div style="margin-top:10px">
          <div id="notes-list">
            ${(goal.notes || []).length === 0
              ? `<p class="muted small">No notes yet.</p>`
              : (goal.notes || []).map(renderNote).join("")}
          </div>
          <details class="note-composer" data-testid="goal-note-composer" style="margin-top:10px"${noteComposerOpen ? " open" : ""}>
            <summary data-testid="goal-note-composer-toggle">+ Add a note</summary>
            <div class="form-row" style="margin-top:8px">
              <textarea id="new-note-body" data-testid="goal-note-body" rows="3"
                        placeholder="Anything the agent or team should know — links to specs, prior decisions, constraints, related code paths."></textarea>
            </div>
            <div class="actions">
              <button id="btn-add-note" data-testid="goal-note-submit">Save note</button>
            </div>
          </details>
        </div>
      </details>
    </div>
  `, bindGoalDetailControls);
}

function bindGoalDetailControls() {
  const liveGoal = () => _goalDetailView.goal || {};
  const liveWorkflow = () => _goalDetailView.workflow || {};

  bindOnce($("#btn-open-agent"), "click", () => {
    openAgentDock({ goalId: liveGoal().id, goalStatus: liveGoal().status });
  });
  bindOnce($("#btn-watch-logs"), "click", () => {
    closeGoalActionMenu();
    openGoalLogTail({ goalId: liveGoal().id, goalName: liveGoal().name });
  });
  // Workflow back / forward buttons. Forward from `review` calls the
  // dedicated /approve endpoint; every other arrow is a plain
  // status PATCH.
  const wireWorkflow = (btnId, direction) => {
    if (!liveWorkflow()[direction]) return;
    bindOnce($(btnId), "click", async () => {
      const btn = $(btnId);
      const target = liveWorkflow()[direction];
      if (!target) return;
      const busyLabel = target.approve
        ? "Approving…"
        : target.retryGovernance
          ? "Queueing candidate…"
          : `Moving to ${target.next}…`;
      await withButtonBusy(btn, busyLabel, async () => {
        try {
          if (target.approve) {
            const r = await api("POST", `/api/goals/${liveGoal().id}/approve`);
            if (r.ok) toast(r.message || "Approved", "info");
            else toast(r.message || "Approval did not complete", "error");
          } else if (target.retryQuality) {
            const r = await api("POST", `/api/goals/${liveGoal().id}/retry-quality`);
            if (r.ok) toast(r.message || "Queued for Quality", "info");
            else toast(r.message || "Quality retry did not queue", "error");
          } else if (target.retryGovernance) {
            const r = await api("POST", `/api/goals/${liveGoal().id}/retry-governance`);
            if (r.ok) toast(r.message || "Queued for Governance", "info");
            else toast(r.message || "Governance retry did not queue", "error");
          } else {
            await api("PATCH", `/api/goals/${liveGoal().id}`, { status: target.next });
            toast(`Moved to ${target.next}`, "info");
          }
          await loadGoalDetail(liveGoal().id);
        } catch (e) { await showActionError(e); }
      });
    });
  };
  wireWorkflow("#btn-state-back", "back");
  wireWorkflow("#btn-state-forward", "forward");
  bindOnce($("#btn-reporter"), "click", async () => {
    closeGoalActionMenu();
    await openGoalReporterModal(liveGoal());
  });
  bindOnce($("#btn-assignee"), "click", async () => {
    closeGoalActionMenu();
    await openGoalAssigneeModal(liveGoal());
  });
  bindOnce($("#btn-rename"), "click", async () => {
    closeGoalActionMenu();
    const name = await modalPrompt("New name", liveGoal().name,
                                   { title: "Rename Goal" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/goals/" + liveGoal().id, { name: name.trim() });
      await loadGoalDetail(liveGoal().id);
    } catch (e) { await showActionError(e); }
  });
  bindOnce($(".note-composer"), "toggle", (e) => {
    if (e.target.open) $("#new-note-body")?.focus();
  });
  bindOnce($("#btn-add-note"), "click", async () => {
    const btn = $("#btn-add-note");
    const ta = $("#new-note-body");
    if (!ta) return;
    const body = (ta.value || "").trim();
    if (!body) return toast("Note can't be empty", "error");
    const author = state.lastReporter || "";
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        await api("POST", `/api/goals/${liveGoal().id}/notes`, { author, body });
        toast("Note added", "info");
        await loadGoalDetail(liveGoal().id);
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-note-edit]").forEach((el) => bindOnce(el, "click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteEdit;
    const existing = (liveGoal().notes || []).find((n) => n.id === id);
    if (!existing) return;
    const body = await modalPrompt(
      "Edit note", existing.body,
      { title: "Edit note", okLabel: "Save" },
    );
    if (body === null) return;
    const trimmed = (body || "").trim();
    if (!trimmed) return toast("Note can't be empty", "error");
    const nextNotes = (liveGoal().notes || []).map(
      (n) => n.id === id ? { ...n, body: trimmed } : n,
    );
    try {
      await api("PATCH", "/api/goals/" + liveGoal().id, { notes: nextNotes });
      toast("Note updated", "info");
      await loadGoalDetail(liveGoal().id);
    } catch (err) { await showActionError(err); }
  }));
  $$("[data-note-delete]").forEach((el) => bindOnce(el, "click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteDelete;
    const ok = await modalConfirm(
      "Delete this note?",
      { title: "Delete note", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    const nextNotes = (liveGoal().notes || []).filter((n) => n.id !== id);
    try {
      await api("PATCH", "/api/goals/" + liveGoal().id, { notes: nextNotes });
      toast("Note deleted", "info");
      await loadGoalDetail(liveGoal().id);
    } catch (err) { await showActionError(err); }
  }));
  bindOnce($("#btn-priority"), "click", async () => {
    closeGoalActionMenu();
    const current = liveGoal().priority || "low";
    const body = () => `
      <div class="modal-title">Change priority</div>
      <div class="modal-body">
        <label for="modal-priority-select">Priority</label>
        <select class="modal-input" id="modal-priority-select" data-testid="goal-priority-select" style="width:100%">
          ${["low", "medium", "high"].map((p) =>
            `<option value="${p}" ${p === current ? "selected" : ""}>${p}</option>`,
          ).join("")}
        </select>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="modal-cancel">Cancel</button>
        <button data-ok data-testid="modal-ok">Save</button>
      </div>`;
    const next = await _openModal(
      body, { cancel: null, ok: current }, ".modal-input",
    );
    if (next === null || next === current) return;
    try {
      await api("PATCH", "/api/goals/" + liveGoal().id, { priority: next });
      toast(`Priority set to ${next}`, "info");
      await loadGoalDetail(liveGoal().id);
    } catch (err) {
      await showActionError(err);
    }
  });
  bindOnce($("#btn-goal-feature-assign"), "click", async () => {
    closeGoalActionMenu();
    await openGoalFeatureAssignModal(liveGoal());
    await loadGoalDetail(liveGoal().id);
  });
  bindOnce($("#btn-goal-feature-remove"), "click", async () => {
    closeGoalActionMenu();
    if (!liveGoal().feature_id) return;
    const ok = await modalConfirm(
      "Remove this Goal from its Feature? The Goal will not be deleted.",
      { title: "Remove from Feature", okLabel: "Remove", cancelLabel: "Keep it" },
    );
    if (!ok) return;
    try {
      await api("DELETE", `/api/features/${encodeURIComponent(liveGoal().feature_id)}/goals/${encodeURIComponent(liveGoal().id)}`);
      toast("Goal removed from Feature", "info");
      await loadGoalDetail(liveGoal().id);
      if (state.currentRoute === "goals") await refreshGoalsTable();
    } catch (e) {
      showActionError(e, "Remove from Feature failed");
    }
  });
  bindOnce($("#btn-transfer-to-active-node"), "click", async () => {
    closeGoalActionMenu();
    const btn = $("#btn-transfer-to-active-node");
    await withButtonBusy(btn, "Transferring…", async () => {
      await transferGoalToActiveNode(liveGoal());
    });
  });
  bindOnce($("#btn-cancel"), "click", async () => {
    closeGoalActionMenu();
    const btn = $("#btn-cancel");
    if (btn.disabled) return;
    const ok = await modalConfirm(
      "Cancel this Goal? Any running subprocess will be stopped. Workflow worktrees and branches will be retained for inspection or explicit cleanup.",
      { title: "Cancel Goal", okLabel: "Cancel Goal", danger: true,
        cancelLabel: "Keep working" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Cancelling…", async () => {
      try {
        await api("POST", `/api/goals/${liveGoal().id}/cancel`);
        toast("Cancelled", "info");
        await loadGoalDetail(liveGoal().id);
      } catch (e) { await showActionError(e); }
    });
  });
  bindOnce($("#btn-delete"), "click", async () => {
    closeGoalActionMenu();
    const ok = await modalConfirm(
      `Delete Goal "${liveGoal().name}"? This cannot be undone.`,
      { title: "Delete Goal", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    try {
      await api("DELETE", "/api/goals/" + liveGoal().id);
      location.hash = "#/goals";
    } catch (e) { await showActionError(e); }
  });

  bindFailureBannerActions(liveGoal());
  bindRoundFormSubmit();
  restoreRoundFormDraftFocus(liveGoal().id);
}

function closeGoalActionMenu() {
  const menu = $("#goal-action-menu");
  if (menu) menu.open = false;
}

async function openGoalReporterModal(goal) {
  if (typeof refreshReporters === "function") {
    try {
      await refreshReporters();
    } catch {}
  }
  const current = goal.reporter || "";
  const reporters = state.reporters || [];
  const options = reporters
    .map((r) => `<option value="${htmlEscape(r.name)}" ${r.name === current ? "selected" : ""}>${htmlEscape(r.name)}</option>`)
    .join("");
  const missingCurrent = current && !reporters.some((r) => r.name === current)
    ? `<option value="${htmlEscape(current)}" selected>${htmlEscape(current)}</option>`
    : "";
  const body = () => `
    <div class="modal-title">Change reporter</div>
    <div class="modal-body">
      <label for="modal-reporter-select">Reporter</label>
      <select class="modal-input" id="modal-reporter-select" data-testid="goal-reporter-select" style="width:100%">
        <option value="">— pick reporter —</option>
        ${missingCurrent}
        ${options}
        <option value="__add__">+ Add new reporter…</option>
      </select>
      <p class="muted small" style="margin-top:6px">
        Updates who first reported this Goal. Round history keeps its original reporters.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="modal-cancel">Cancel</button>
      <button data-ok data-testid="modal-ok">Save</button>
    </div>`;
  let next = await _openModal(body, { cancel: null, ok: current }, ".modal-input");
  if (next === null) return;
  if (next === "__add__") {
    const name = await modalPrompt("Name for the new reporter:", "", { title: "Add reporter" });
    next = (name || "").trim();
    if (!next) return;
    try {
      await api("POST", "/api/reporters", { name: next });
      await refreshReporters();
    } catch (e) {
      await showActionError(e, "Could not add reporter");
      return;
    }
  }
  next = (next || "").trim();
  if (!next || next === current) return;
  try {
    await api("PATCH", "/api/goals/" + goal.id, { reporter: next });
    toast(`Reporter set to ${next}`, "info");
    await loadGoalDetail(goal.id);
  } catch (e) {
    await showActionError(e, "Reporter update failed");
  }
}

async function openGoalAssigneeModal(goal) {
  if (typeof refreshReporters === "function") {
    try {
      await refreshReporters();
    } catch {}
  }
  const current = goal.assignee || "";
  const reporters = state.reporters || [];
  const options = reporters
    .map((r) => `<option value="${htmlEscape(r.name)}" ${r.name === current ? "selected" : ""}>${htmlEscape(r.name)}</option>`)
    .join("");
  const missingCurrent = current && !reporters.some((r) => r.name === current)
    ? `<option value="${htmlEscape(current)}" selected>${htmlEscape(current)}</option>`
    : "";
  const body = () => `
    <div class="modal-title">Change assignee</div>
    <div class="modal-body">
      <label for="modal-assignee-select">Assignee</label>
      <select class="modal-input" id="modal-assignee-select" data-testid="goal-assignee-select" style="width:100%">
        <option value="">— pick assignee —</option>
        ${missingCurrent}
        ${options}
      </select>
      <p class="muted small" style="margin-top:6px">
        Updates the latest round's assignee, which is this Goal's current owner.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="modal-cancel">Cancel</button>
      <button data-ok data-testid="modal-ok">Save</button>
    </div>`;
  const next = await _openModal(body, { cancel: null, ok: current }, ".modal-input");
  if (next === null || !next || next === current) return;
  try {
    await api("PATCH", "/api/goals/" + goal.id, { assignee: next });
    toast(`Assignee set to ${next}`, "info");
    await loadGoalDetail(goal.id);
  } catch (e) {
    await showActionError(e, "Assignee update failed");
  }
}

function captureRoundFormDraft(goalId) {
  const form = document.querySelector('#round-form[data-kind="edit"]');
  if (!form || state.currentGoalData?.id !== goalId) return null;
  const promptEl = form.elements.prompt;
  if (!promptEl) return null;
  const rounds = state.currentGoalData.rounds || [];
  const latest = rounds[rounds.length - 1] || {};
  const prompt = promptEl.value || "";
  const dirty = prompt !== (latest.prompt || "");
  if (!dirty) return null;
  const activeEl = form.contains(document.activeElement) ? document.activeElement : null;
  const activeName = activeEl?.name || "";
  return {
    goalId,
    prompt,
    activeName,
    selectionStart: typeof activeEl?.selectionStart === "number" ? activeEl.selectionStart : null,
    selectionEnd: typeof activeEl?.selectionEnd === "number" ? activeEl.selectionEnd : null,
  };
}

function hasPreservedRoundFormDraft(goalId) {
  return !!(_goalRoundFormDraft && _goalRoundFormDraft.goalId === goalId);
}

function restoreRoundFormDraftFocus(goalId) {
  const draft = _goalRoundFormDraft;
  if (!draft || draft.goalId !== goalId || !draft.activeName) return;
  const form = document.querySelector('#round-form[data-kind="edit"]');
  const el = form?.elements?.[draft.activeName];
  if (!el || el.readOnly || el.disabled) return;
  el.focus();
  if (typeof el.setSelectionRange === "function" &&
      draft.selectionStart !== null && draft.selectionEnd !== null) {
    el.setSelectionRange(draft.selectionStart, draft.selectionEnd);
  }
}

function renderImplementationChecklist(items, implementation) {
  const results = new Map((implementation?.execution?.checklist || []).map((item) => [item.id, item]));
  if (!items?.length) return `<p class="muted small">No checklist has been recorded.</p>`;
  return `<ol class="implementation-plan-checklist" data-testid="goal-implementation-plan-checklist">
    ${items.map((item) => {
      const result = results.get(item.id);
      return `<li data-checklist-id="${htmlEscape(item.id)}">
        <div><code>${htmlEscape(item.id)}</code> ${htmlEscape(item.description || "")}
          ${result ? `<span class="status-pill ${result.outcome === "completed" ? "done" : "failed"}" data-testid="goal-implementation-checklist-outcome">${htmlEscape(result.outcome)}</span>` : ""}
        </div>
        ${item.affected_behavior?.length ? `<div class="muted small">Affected: ${item.affected_behavior.map(htmlEscape).join(", ")}</div>` : ""}
        ${item.governance_rationale ? `<div class="muted small">Governance: ${htmlEscape(item.governance_rationale)}</div>` : ""}
        ${item.verification?.length ? `<div class="muted small">Intended evidence: ${item.verification.map(htmlEscape).join("; ")}</div>` : ""}
        ${result?.evidence ? `<div class="small" data-testid="goal-implementation-checklist-evidence">${htmlEscape(result.evidence)}</div>` : ""}
      </li>`;
    }).join("")}
  </ol>`;
}

function renderImplementationPlan(rnd, idx, prevPlanHistoryOpen = {}) {
  const plan = rnd?.implementation_plan;
  if (!plan) return "";
  const finalPlan = plan.final_plan?.result;
  const proposal = plan.proposal?.result;
  const criticism = plan.criticism?.result;
  const historyOpen = (kind) => prevPlanHistoryOpen[`${idx}:${kind}`] ? " open" : "";
  const phase = plan.phase || "unknown";
  const planState = plan.state || "in_progress";
  const phaseLabel = planState === "in_progress" ? phase : planState;
  return `<section class="card implementation-plan" data-testid="goal-implementation-plan" aria-labelledby="implementation-plan-title-${idx}" style="margin-top:12px">
    <div class="row" style="align-items:center;gap:8px">
      <h4 id="implementation-plan-title-${idx}" style="margin:0">Implementation Plan</h4>
      <span class="status-pill ${planState === "completed" ? "done" : planState === "failed" ? "failed" : "plan"}" data-testid="goal-implementation-plan-phase">${htmlEscape(phaseLabel)}</span>
      <span class="muted small">phase ${htmlEscape(phase)}</span>
      <span class="spacer"></span>
      <span class="muted small">phase started ${fmtTime(plan.phase_started_at || plan.started_at)} · updated ${fmtTime(plan.updated_at)}</span>
    </div>
    ${plan.active_process ? `<div class="muted small" data-testid="goal-implementation-plan-active-process">Active operation ${htmlEscape(plan.active_process.operation_id || "unknown")}${plan.active_process.process_id ? ` · process ${htmlEscape(plan.active_process.process_id)}` : " · awaiting process registration"}</div>` : ""}
    ${finalPlan ? `<p data-testid="goal-implementation-plan-summary">${htmlEscape(finalPlan.summary || "")}</p>${renderImplementationChecklist(finalPlan.checklist, plan.implementation)}` : `<p class="muted">The final plan is not available yet.</p>`}
    ${plan.implementation?.execution?.verification?.length ? `<div class="small" data-testid="goal-implementation-verification"><strong>Verification:</strong> ${plan.implementation.execution.verification.map(htmlEscape).join("; ")}</div>` : ""}
    ${plan.failure ? `<div class="banner error" role="alert" data-testid="goal-implementation-plan-failure"><span class="banner-msg">${htmlEscape(plan.failure.phase)} failed ${plan.failure.failed_at ? `at ${fmtTime(plan.failure.failed_at)}` : ""}: ${htmlEscape(plan.failure.message)}</span></div>` : ""}
    ${proposal ? `<details data-plan-history="proposal" data-round-idx="${idx}" data-testid="goal-implementation-plan-proposal"${historyOpen("proposal")}><summary>Original proposal · ${fmtTime(plan.proposal.completed_at)}</summary><p>${htmlEscape(proposal.summary || "")}</p>${renderImplementationChecklist(proposal.checklist)}</details>` : ""}
    ${criticism ? `<details data-plan-history="criticism" data-round-idx="${idx}" data-testid="goal-implementation-plan-criticism"${historyOpen("criticism")}><summary>Independent criticism · ${fmtTime(plan.criticism.completed_at)}</summary><p>${htmlEscape(criticism.summary || "")}</p><ul>${(criticism.findings || []).map((finding) => `<li><code>${htmlEscape(finding.id)}</code> ${htmlEscape(finding.description || "")}<div class="muted small">${htmlEscape(finding.recommendation || "")}</div></li>`).join("")}</ul></details>` : ""}
  </section>`;
}

function renderRound(rnd, idx, isLatest, prevRoundOpen = {}, prevPlanHistoryOpen = {}) {
  // Preserve the user's open/closed choice across re-renders. New rounds
  // (no prior entry in the snapshot) default to "open on latest" — the
  // historical behavior.
  const key = String(idx);
  const roundOpen = key in prevRoundOpen ? prevRoundOpen[key] : isLatest;
  return `
    <details class="round" data-round-idx="${idx}" data-testid="goal-round" ${roundOpen ? "open" : ""}>
      <summary class="round-head" data-testid="goal-round-summary">
        <strong>Round ${idx + 1}</strong>
        ${isLatest ? `<span class="status-pill review">latest</span>` : ""}
        ${isLatest && rnd.rule_state && rnd.rule_state !== "unclassified"
          ? `<span class="status-pill ${reviewStateClass(rnd.rule_state)}">governance: ${htmlEscape(rnd.rule_state)}</span>`
          : ""}
        ${isLatest && rnd.quality_state && rnd.quality_state !== "unclassified"
          ? `<span class="status-pill ${reviewStateClass(rnd.quality_state, "quality")}">quality: ${htmlEscape(rnd.quality_state)}</span>`
          : ""}
        <span class="spacer"></span>
        <span class="muted small">
          reporter ${htmlEscape(rnd.reporter || "(none)")}
          · assignee ${htmlEscape(rnd.assignee || "(none)")}
          · ${fmtTime(rnd.created)}
        </span>
      </summary>
      <div class="round-body">
        <dl class="pair">
          <dt>prompt</dt><dd data-testid="goal-round-detail-prompt">${htmlEscape(rnd.prompt || "").replace(/\n/g, "<br>")}</dd>
        </dl>
        ${renderImplementationPlan(rnd, idx, prevPlanHistoryOpen)}
        ${rnd.implementation_report ? `
          <div class="card implementation-report" data-testid="goal-implementation-report" style="margin-top:12px">
            <div class="row" style="align-items:center;gap:8px">
              <h4 style="margin:0">Implementation report</h4>
              <span class="muted small">what changed, why, and verification</span>
              <span class="spacer"></span>
              ${rnd.implementation_reported_at
                ? `<span class="muted small" data-testid="goal-implementation-reported-at">${fmtTime(rnd.implementation_reported_at)}</span>`
                : ""}
            </div>
            <div data-testid="goal-implementation-report-body" style="margin-top:8px;white-space:pre-wrap">${htmlEscape(rnd.implementation_report)}</div>
          </div>` : ""}
      </div>
    </details>
  `;
}

// A Goal can fail after every gate it reached passed — an integration that
// found the branch tip moved, for one — so the reason gets its own card above
// the gate summaries rather than being inferred from them.
function goalFailureEvidence(goal, round) {
  if (!round) return null;
  const stateBoundary = latestStateBoundary(round);
  // Failure transitions are normally logged immediately after their cause, so
  // a strict post-transition boundary would discard the error users need.
  // The latest error is already scoped to this round by the backend.
  const errorLog = goal?.status === "failed"
    ? round.latest_error_log || null
    : currentRoundLog(round.latest_error_log, stateBoundary);
  const workflowLog = currentRoundLog(round.latest_workflow_log, stateBoundary);
  const fallbackLog = currentRoundLog(
    round.latest_log?.severity && round.latest_log.severity !== "info"
      ? round.latest_log
      : null,
    stateBoundary,
  );
  const log = errorLog || workflowLog || fallbackLog;
  const message = round.failure_message || log?.message || "";
  if (!message || (goal?.status !== "failed" && !round.failure_message)) return null;
  return {
    category: round.failure_category || log?.category || "workflow",
    message,
    at: round.failure_at || log?.datetime || "",
    log_details: log?.details || null,
  };
}

function renderFailureSummary(goal, round) {
  const failure = goalFailureEvidence(goal, round);
  if (!failure) {
    return "";
  }
  const details = diagnosticDetailsText({
    category: failure.category,
    message: failure.message,
    occurred_at: failure.at || null,
    evidence: failure.log_details,
  });
  return `
    <div class="card" style="margin:0 0 14px" data-testid="goal-failure-summary">
      <h3>Failure</h3>
      <div class="row" style="gap:8px;flex-wrap:wrap">
        ${failure.category ? `<span class="status-pill failed" data-testid="goal-failure-category">${htmlEscape(failure.category)}</span>` : ""}
        ${failure.at ? `<span class="muted small" data-testid="goal-failure-at">${fmtTime(failure.at)}</span>` : ""}
      </div>
      <p style="margin-bottom:6px" data-testid="goal-failure-message">${htmlEscape(failure.message)}</p>
      <details data-testid="goal-failure-details"><summary>Details</summary><pre>${htmlEscape(details)}</pre></details>
    </div>`;
}

function renderGovernanceSummary(round) {
  const governance = governanceReviewStatus(round);
  if (!governance.visible) {
    return "";
  }
  const actions = round.governance_rule_actions || [];
  const states = governance.states;
  return `
    <div class="card" style="margin:0 0 14px" data-testid="goal-governance-summary">
      <h3>Governance</h3>
      <div class="row" style="gap:8px;flex-wrap:wrap">
        <span class="status-pill ${reviewStateClass(states.rules)}" data-testid="goal-governance-rules">rules: ${htmlEscape(states.rules)}</span>
        <span class="status-pill ${reviewStateClass(states.product)}" data-testid="goal-governance-product">product: ${htmlEscape(states.product)}</span>
        <span class="status-pill ${reviewStateClass(states.constitution)}" data-testid="goal-governance-constitution">constitution: ${htmlEscape(states.constitution)}</span>
        <span class="status-pill ${reviewStateClass(states.meta)}" data-testid="goal-governance-meta">meta: ${htmlEscape(states.meta)}</span>
      </div>
      ${round.governance_message ? `<p style="margin-bottom:6px" data-testid="goal-governance-message">${htmlEscape(round.governance_message)}</p>` : ""}
      ${round.governance_details ? `<details data-testid="goal-governance-details"><summary>Details</summary><pre>${htmlEscape(diagnosticDetailsText(round.governance_details))}</pre></details>` : ""}
      ${actions.length ? `
        <details style="margin-top:8px" data-testid="goal-governance-actions">
          <summary>Rule actions (${actions.length})</summary>
          ${actions.map((a) => `
            <div class="log-entry info" data-testid="goal-governance-action">
              <div>${htmlEscape(a.action || "")}${a.text ? `: ${htmlEscape(a.text)}` : ""}</div>
              ${a.reason ? `<div class="meta">${htmlEscape(a.reason)}</div>` : ""}
            </div>`).join("")}
        </details>` : ""}
    </div>`;
}

function renderQualitySummary(round) {
  if (!round || !round.quality_state || round.quality_state === "unclassified") {
    return "";
  }
  return `
    <div class="card" style="margin:0 0 14px" data-testid="goal-quality-summary">
      <h3>Quality</h3>
      <div class="row" style="gap:8px;flex-wrap:wrap">
        <span class="status-pill ${reviewStateClass(round.quality_state, "quality")}" data-testid="goal-quality-state">quality: ${htmlEscape(normalizeReviewState(round.quality_state))}</span>
        ${round.quality_checked_at ? `<span class="muted small" data-testid="goal-quality-checked-at">${fmtTime(round.quality_checked_at)}</span>` : ""}
      </div>
      ${round.quality_message ? `<p style="margin-bottom:6px" data-testid="goal-quality-message">${htmlEscape(round.quality_message)}</p>` : ""}
      ${round.quality_details ? `<details data-testid="goal-quality-details"><summary>Details</summary><pre>${htmlEscape(diagnosticDetailsText(round.quality_details))}</pre></details>` : ""}
    </div>`;
}

function renderNote(n) {
  const firstLine = (n.body || "").split("\n", 1)[0];
  const preview = firstLine.length > 80
    ? firstLine.slice(0, 77) + "…"
    : firstLine;
  const meta = [n.author, n.created ? fmtTime(n.created) : ""].filter(Boolean).join(" · ");
  return `
    <details class="note" data-testid="goal-note">
      <summary data-testid="goal-note-summary">
        <span class="note-preview" data-testid="goal-note-preview">${htmlEscape(preview || "(empty)")}</span>
        ${meta ? `<span class="muted small note-meta">${htmlEscape(meta)}</span>` : ""}
      </summary>
      <div class="note-body" data-testid="goal-note-detail">${htmlEscape(n.body || "").replace(/\n/g, "<br>")}</div>
      <div class="actions" style="margin-top:6px">
        <button class="secondary" data-note-edit="${htmlEscape(n.id)}" data-testid="goal-note-edit">Edit</button>
        <button class="danger" data-note-delete="${htmlEscape(n.id)}" data-testid="goal-note-delete">Delete</button>
      </div>
    </details>`;
}

function renderRoundForm(
  kind,
  prefill,
  { draft = null, disabled = false, formId = "round-form" } = {},
) {
  const prompt = draft?.prompt ?? prefill?.prompt ?? "";
  const reporter = state.lastReporter || "";
  if (!reporter) return renderPickReporterNotice();
  const submitLabel = kind === "submit" ? "Submit new round" : "Save changes";
  const readonly = disabled ? "readonly" : "";
  const buttonDisabled = disabled ? "disabled" : "";
  return `
    <form id="${htmlEscape(formId)}" data-kind="${kind}" data-testid="goal-round-form">
      <div class="muted small" style="margin-bottom:8px">
        Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
        — change in the top-right reporter selector.
      </div>
      ${disabled ? `
        <p class="muted small">
          This Goal is no longer editable. Unsaved text is preserved here so you can copy it.
        </p>` : ""}
      <div class="form-row">
        <label>Prompt</label>
        <textarea name="prompt" data-testid="goal-round-prompt" placeholder="Describe what the agent should accomplish." ${readonly}>${htmlEscape(prompt)}</textarea>
      </div>
      <div class="actions">
        <button type="submit" data-testid="goal-round-submit" ${buttonDisabled}>${submitLabel}</button>
      </div>
    </form>
  `;
}

function renderPickReporterNotice() {
  return `
    <p class="muted">
      Pick a reporter in the top-right selector to enable this form.
    </p>
  `;
}

// The submit handler is bound once and outlives the render that bound it, so it
// reads the current goal rather than the one passed in when it was first bound.
function bindRoundFormSubmit() {
  const liveGoal = () => _goalDetailView.goal || {};
  const form = $("#round-form");
  if (!form) return;
  bindOnce(form, "submit", async (e) => {
    e.preventDefault();
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const prompt = (fd.get("prompt") || "").toString().trim();
    if (!prompt) return toast("Provide a prompt", "error");
    const kind = form.dataset.kind;
    try {
      const assignee = liveGoal().assignee || reporter;
      if (kind === "submit") {
        await api("POST", `/api/goals/${liveGoal().id}/rounds`, { reporter, assignee, prompt });
        toast("New round submitted", "info");
      } else {
        await api("PATCH", `/api/goals/${liveGoal().id}/rounds/latest`, { reporter, assignee, prompt });
        toast("Round updated", "info");
      }
      _goalRoundFormDraft = null;
      await loadGoalDetail(liveGoal().id);
    } catch (err) {
      await showActionError(err);
    }
  });
}

function computeFailureBanner(goal, latest) {
  const stateBoundary = latestStateBoundary(latest);
  const workflowLog = currentRoundLog(latest?.latest_workflow_log, stateBoundary);
  if (goal.status === "failed") {
    const lastLog = latest?.latest_log;
    const errLog = currentRoundLog(latest?.latest_error_log, stateBoundary);
    const fallbackLog = currentRoundLog(
      lastLog?.severity && lastLog.severity !== "info" ? lastLog : null,
      stateBoundary,
    );
    return {
      severity: "error",
      message: errLog?.message || workflowLog?.message || fallbackLog?.message || "Goal failed",
      actionsHtml: "",
    };
  }
  if (goal.status === "review") {
    // Was there a recent error? Then we treat it as a stuck-review state.
    const errLog = currentRoundLog(latest?.latest_error_log, stateBoundary);
    if (errLog) {
      return {
        severity: "error",
        message: errLog.message || "Review needs attention",
        actionsHtml: "",
      };
    }
  }
  return null;
}

function computeGovernanceBanner(goal, latest) {
  const governance = governanceReviewStatus(latest);
  if (!governance.visible) {
    return null;
  }
  if (governance.passed) return null;
  return {
    severity: goal.status === "backlog" ? "warn" : "error",
    message: latest.governance_message || "Governance review requires changes before implementation.",
  };
}

function computeFeatureBlockingNotice(goal) {
  const notice = goal?.feature_blocking_notice;
  if (!notice || !notice.message) return null;
  return {
    severity: "warn",
    message: notice.message,
    details: {
      goal_id: goal.id,
      feature_id: notice.feature_id || goal.feature_id || null,
      blocked_count: notice.blocked_count || 0,
      blocked_goal_ids: notice.blocked_goal_ids || [],
      next_blocked_goal_id: notice.next_blocked_goal_id || null,
    },
  };
}

function recordFeatureBlockingNotice(goal, notice) {
  if (!notice || typeof recordUiNotice !== "function") return;
  const details = notice.details || {};
  const key = [
    goal?.id || "",
    goal?.updated || "",
    details.feature_id || "",
    details.blocked_count || 0,
    details.next_blocked_goal_id || "",
  ].join(":");
  if (_loggedFeatureBlockingNoticeKeys.has(key)) return;
  _loggedFeatureBlockingNoticeKeys.add(key);
  if (_loggedFeatureBlockingNoticeKeys.size > 100) {
    _loggedFeatureBlockingNoticeKeys.delete(_loggedFeatureBlockingNoticeKeys.values().next().value);
  }
  recordUiNotice(notice.message, {
    kind: "warn",
    source: "workflow",
    details,
  });
}

function bindFailureBannerActions(_goal) {
  // No banner-level actions: Approve / Open Agent / Reopen / Rename / Cancel /
  // Delete all live in the unified action menu at the top of the page.
}
