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
  root.addEventListener("click", (e) => {
    if (e.target === root) dismiss();
  });
  root.querySelector(".modal-close").addEventListener("click", dismiss);

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
  try {
    const { goal } = await api("GET", "/api/goals/" + goalId);
    if (state.currentGoal !== goalId || goal?.id !== goalId) return;
    drawGoalDetail(goal);
  } catch (e) {
    if (state.currentGoal !== goalId) return;
    const container = goalDetailContainer();
    if (container) {
      container.innerHTML = `<p class="muted">Could not load Goal: ${htmlEscape(e.message)}</p>`;
    }
  }
}

// User-driven workflow transitions for a Goal. Each state declares its
// `back` and `forward` neighbors. System-owned states have no user buttons —
// `in-progress` (Workflow Engine owns), `qa` (Quality owns), `ready-merge`
// (candidate preparation owns), and `build` (target-app build owns) have no user buttons
// because they're system-driven phases the agent passes through
// automatically (todo → in-progress → ready-merge → build → qa → review).
// Forward from `review` goes through the dedicated /approve endpoint for
// approval. No user action moves a Goal into `review`; successful build does.
//
// failed / cancelled only expose a back arrow — there's no obvious
// forward target for them (they're terminal-ish in opposite directions
// from done). Failed Goals normally go back to todo and rerun; candidate-stage
// failures use the latest workflow transition to requeue the isolated branch.
const GOAL_WORKFLOW = {
  backlog:      { forward: { label: "Todo →",     next: "todo"   } },
  todo:         { back:    { label: "← Backlog",  next: "backlog" } },
  // in-progress: no user buttons — Workflow Engine owns.
  // qa: no user buttons — Quality owns.
  // ready-merge: no user buttons — candidate preparation owns.
  // build: no user buttons — target-app build owns.
  review:       { back:    { label: "← Todo",     next: "todo"   },
                  forward: { label: "Approve →",  next: "done", approve: true } },
  done:         { back:    { label: "← Review",   next: "review" } },
  failed:       { back:    { label: "← Todo",     next: "todo"   } },
  cancelled:    { back:    { label: "← Todo",     next: "todo"   } },
};

function workflowForGoal(goal, latest) {
  if (goal.status === "failed" && isQualityRetryGoal(latest)) {
    return { back: { label: "← QA", next: "qa", retryQuality: true } };
  }
  if (goal.status === "failed" && isMergeRetryGoal(latest)) {
    return { back: { label: "← Candidate", next: "ready-merge", retryMerge: true } };
  }
  return GOAL_WORKFLOW[goal.status] || {};
}

function isQualityRetryGoal(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("qa") &&
         message.includes("failed");
}

function isMergeRetryGoal(latest) {
  const message = latest?.latest_workflow_log?.message || "";
  return message.includes("Workflow status changed:") &&
         message.includes("ready-merge") &&
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
  // The Goal Agent is the workflow-owned native CLI session. Open Agent
  // attaches to that process while implementation is active; it never starts
  // a second conversational agent for the same Goal.
  const canOpenAgent = goal.status === "in-progress";

  // Dynamic workflow buttons: each state shows the previous/next state
  // it can move to as back / forward buttons. The user-driven workflow
  // skips system-owned statuses. Forward from review goes through the
  // `approve` endpoint; everything else is a bookkeeping status
  // update via PATCH /api/goals/<id>.
  const workflow = workflowForGoal(goal, latest);
  const backBtn = workflow.back ? `
    <button id="btn-state-back" data-testid="goal-state-back">${htmlEscape(workflow.back.label)}</button>
  ` : "";
  const forwardBtn = workflow.forward ? `
    <button id="btn-state-forward" data-testid="goal-state-forward">${htmlEscape(workflow.forward.label)}</button>
  ` : "";

  const container = goalDetailContainer();
  if (!container) return;
  container.innerHTML = `
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
                  title="${canOpenAgent ? "Attach to the running Goal Agent" : "The Goal Agent is available while implementation is active"}">Open Agent</button>
          <details class="nav-menu goal-action-menu" id="goal-action-menu">
            <summary class="btn goal-action-more" aria-label="More Goal actions" data-testid="goal-action-menu-toggle"></summary>
            <div class="nav-menu-panel goal-action-panel">
              <button class="nav-menu-item" type="button" id="btn-watch-logs" data-testid="goal-action-watch-logs">Watch Logs</button>
              <button class="nav-menu-item" type="button" id="btn-reporter" data-testid="goal-action-reporter">Reporter</button>
              <button class="nav-menu-item" type="button" id="btn-assignee" data-testid="goal-action-assignee">Assignee</button>
              <button class="nav-menu-item" type="button" id="btn-rename" data-testid="goal-action-rename">Rename</button>
              <button class="nav-menu-item" type="button" id="btn-priority" data-testid="goal-action-priority">Change Priority</button>
              <button class="nav-menu-item" type="button" id="btn-goal-feature-assign" data-testid="goal-action-assign-feature">Move to Feature</button>
              <button class="nav-menu-item" type="button" id="btn-goal-feature-remove" data-testid="goal-action-remove-feature" ${goal.feature_id ? "" : "disabled"}>Remove from Feature</button>
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

      ${latest ? renderGovernanceSummary(latest) : ""}
      ${latest ? renderQualitySummary(latest) : ""}

      <h3>Rounds (${rounds.length})</h3>
      ${rounds.length === 0 ? `<p class="muted">No rounds yet.</p>` :
        rounds.map((rnd, idx) => renderRound(
          rnd, idx,
          idx === rounds.length - 1,
          prevRoundOpen,
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
          <details class="note-composer" data-testid="goal-note-composer" style="margin-top:10px">
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
  `;
  recordFeatureBlockingNotice(goal, featureBlockingNotice);

  $("#btn-open-agent")?.addEventListener("click", () => {
    openAgentDock({ goalId: goal.id, goalStatus: goal.status });
  });
  $("#btn-watch-logs")?.addEventListener("click", () => {
    closeGoalActionMenu();
    openGoalLogTail({ goalId: goal.id, goalName: goal.name });
  });
  // Workflow back / forward buttons. Forward from `review` calls the
  // dedicated /approve endpoint; every other arrow is a plain
  // status PATCH.
  const wireWorkflow = (btnId, target) => {
    if (!target) return;
    $(btnId)?.addEventListener("click", async () => {
      const btn = $(btnId);
      const busyLabel = target.approve
        ? "Approving…"
        : target.retryMerge
          ? "Queueing candidate…"
          : `Moving to ${target.next}…`;
      await withButtonBusy(btn, busyLabel, async () => {
        try {
          if (target.approve) {
            const r = await api("POST", `/api/goals/${goal.id}/approve`);
            if (r.ok) toast(r.message || "Approved", "info");
            else toast(r.message || "Approval did not complete", "error");
          } else if (target.retryQuality) {
            const r = await api("POST", `/api/goals/${goal.id}/retry-quality`);
            if (r.ok) toast(r.message || "Queued for QA", "info");
            else toast(r.message || "QA retry did not queue", "error");
          } else if (target.retryMerge) {
            const r = await api("POST", `/api/goals/${goal.id}/retry-merge`);
            if (r.ok) toast(r.message || "Queued candidate", "info");
            else toast(r.message || "Candidate retry did not queue", "error");
          } else {
            await api("PATCH", `/api/goals/${goal.id}`, { status: target.next });
            toast(`Moved to ${target.next}`, "info");
          }
          await loadGoalDetail(goal.id);
        } catch (e) { await showActionError(e); }
      });
    });
  };
  wireWorkflow("#btn-state-back", workflow.back);
  wireWorkflow("#btn-state-forward", workflow.forward);
  $("#btn-reporter")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    await openGoalReporterModal(goal);
  });
  $("#btn-assignee")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    await openGoalAssigneeModal(goal);
  });
  $("#btn-rename")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    const name = await modalPrompt("New name", goal.name,
                                   { title: "Rename Goal" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/goals/" + goal.id, { name: name.trim() });
      await loadGoalDetail(goal.id);
    } catch (e) { await showActionError(e); }
  });
  $(".note-composer")?.addEventListener("toggle", (e) => {
    if (e.target.open) $("#new-note-body")?.focus();
  });
  $("#btn-add-note")?.addEventListener("click", async () => {
    const btn = $("#btn-add-note");
    const ta = $("#new-note-body");
    if (!ta) return;
    const body = (ta.value || "").trim();
    if (!body) return toast("Note can't be empty", "error");
    const author = state.lastReporter || "";
    await withButtonBusy(btn, "Saving…", async () => {
      try {
        await api("POST", `/api/goals/${goal.id}/notes`, { author, body });
        toast("Note added", "info");
        await loadGoalDetail(goal.id);
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-note-edit]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteEdit;
    const existing = (goal.notes || []).find((n) => n.id === id);
    if (!existing) return;
    const body = await modalPrompt(
      "Edit note", existing.body,
      { title: "Edit note", okLabel: "Save" },
    );
    if (body === null) return;
    const trimmed = (body || "").trim();
    if (!trimmed) return toast("Note can't be empty", "error");
    const nextNotes = (goal.notes || []).map(
      (n) => n.id === id ? { ...n, body: trimmed } : n,
    );
    try {
      await api("PATCH", "/api/goals/" + goal.id, { notes: nextNotes });
      toast("Note updated", "info");
      await loadGoalDetail(goal.id);
    } catch (err) { await showActionError(err); }
  }));
  $$("[data-note-delete]").forEach((el) => el.addEventListener("click", async (e) => {
    e.preventDefault();
    const id = el.dataset.noteDelete;
    const ok = await modalConfirm(
      "Delete this note?",
      { title: "Delete note", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    const nextNotes = (goal.notes || []).filter((n) => n.id !== id);
    try {
      await api("PATCH", "/api/goals/" + goal.id, { notes: nextNotes });
      toast("Note deleted", "info");
      await loadGoalDetail(goal.id);
    } catch (err) { await showActionError(err); }
  }));
  $("#btn-priority")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    const current = goal.priority || "low";
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
      await api("PATCH", "/api/goals/" + goal.id, { priority: next });
      toast(`Priority set to ${next}`, "info");
      await loadGoalDetail(goal.id);
    } catch (err) {
      await showActionError(err);
    }
  });
  $("#btn-goal-feature-assign")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    await openGoalFeatureAssignModal(goal);
    await loadGoalDetail(goal.id);
  });
  $("#btn-goal-feature-remove")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    if (!goal.feature_id) return;
    const ok = await modalConfirm(
      "Remove this Goal from its Feature? The Goal will not be deleted.",
      { title: "Remove from Feature", okLabel: "Remove", cancelLabel: "Keep it" },
    );
    if (!ok) return;
    try {
      await api("DELETE", `/api/features/${encodeURIComponent(goal.feature_id)}/goals/${encodeURIComponent(goal.id)}`);
      toast("Goal removed from Feature", "info");
      await loadGoalDetail(goal.id);
      if (state.currentRoute === "goals") await refreshGoalsTable();
    } catch (e) {
      showActionError(e, "Remove from Feature failed");
    }
  });
  $("#btn-cancel")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    const btn = $("#btn-cancel");
    if (btn.disabled) return;
    const ok = await modalConfirm(
      "Cancel this Goal? Any running subprocess will be stopped and the worktree + branch cleaned up.",
      { title: "Cancel Goal", okLabel: "Cancel Goal", danger: true,
        cancelLabel: "Keep working" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Cancelling…", async () => {
      try {
        await api("POST", `/api/goals/${goal.id}/cancel`);
        toast("Cancelled", "info");
        await loadGoalDetail(goal.id);
      } catch (e) { await showActionError(e); }
    });
  });
  $("#btn-delete")?.addEventListener("click", async () => {
    closeGoalActionMenu();
    const ok = await modalConfirm(
      `Delete Goal "${goal.name}"? This cannot be undone.`,
      { title: "Delete Goal", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    try {
      await api("DELETE", "/api/goals/" + goal.id);
      location.hash = "#/goals";
    } catch (e) { await showActionError(e); }
  });

  bindFailureBannerActions(goal);
  bindRoundFormSubmit(goal);
  restoreRoundFormDraftFocus(goal.id);
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

function renderRound(rnd, idx, isLatest, prevRoundOpen = {}) {
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
          ? `<span class="status-pill ${reviewStateClass(rnd.quality_state, "qa")}">quality: ${htmlEscape(rnd.quality_state)}</span>`
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
      ${round.governance_details ? `<details data-testid="goal-governance-details"><summary>Details</summary><pre>${htmlEscape(round.governance_details)}</pre></details>` : ""}
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
        <span class="status-pill ${reviewStateClass(round.quality_state, "qa")}" data-testid="goal-quality-state">quality: ${htmlEscape(normalizeReviewState(round.quality_state))}</span>
        ${round.quality_checked_at ? `<span class="muted small" data-testid="goal-quality-checked-at">${fmtTime(round.quality_checked_at)}</span>` : ""}
      </div>
      ${round.quality_message ? `<p style="margin-bottom:6px" data-testid="goal-quality-message">${htmlEscape(round.quality_message)}</p>` : ""}
      ${round.quality_details ? `<details data-testid="goal-quality-details"><summary>Details</summary><pre>${htmlEscape(round.quality_details)}</pre></details>` : ""}
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

function bindRoundFormSubmit(goal) {
  const form = $("#round-form");
  if (!form) return;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const reporter = state.lastReporter || "";
    if (!reporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const prompt = (fd.get("prompt") || "").toString().trim();
    if (!prompt) return toast("Provide a prompt", "error");
    const kind = form.dataset.kind;
    try {
      const assignee = goal.assignee || reporter;
      if (kind === "submit") {
        await api("POST", `/api/goals/${goal.id}/rounds`, { reporter, assignee, prompt });
        toast("New round submitted", "info");
      } else {
        await api("PATCH", `/api/goals/${goal.id}/rounds/latest`, { reporter, assignee, prompt });
        toast("Round updated", "info");
      }
      _goalRoundFormDraft = null;
      await loadGoalDetail(goal.id);
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
