// ---- Goals: bulk-update modal ------------------------------------------------
//
// Each bulk action prompts for a new value and confirms the change against
// the current filter-scoped selection. Exactly one field is changed per call
// so the confirmation reads cleanly.

const BULK_PRIORITY_OPTIONS = ["low", "medium", "high"];
const BULK_STATUS_OPTIONS = [
  { value: "__last_workflow_state", label: "(Last workflow state)" },
  { value: "backlog", label: "backlog" },
  { value: "todo", label: "todo" },
  { value: "build", label: "build" },
  { value: "review", label: "review" },
  { value: "done", label: "done" },
  { value: "failed", label: "failed" },
  { value: "cancelled", label: "cancelled" },
];

function goalsBulkFilterFromHash() {
  const f = goalsFilterFromHash();
  const filter = {};
  for (const key of ["status", "q", "reporter", "assignee", "feature", "node"]) {
    if (f[key]) filter[key] = f[key];
  }
  if (f.rounds_gte !== "") filter.rounds_gte = parseInt(f.rounds_gte, 10);
  if (f.rounds_lte !== "") filter.rounds_lte = parseInt(f.rounds_lte, 10);
  return filter;
}

async function openBulkModal(field) {
  // Snapshot the current filter for display context; the mutation itself
  // targets all matching Goals unless the user has switched to an explicit
  // picked-ID selection by clearing the header checkbox.
  const filter = goalsBulkFilterFromHash();
  const filterDesc = describeGoalsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGoalSelection()) {
    toast("No Goals selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected");
  const label = { priority: "Priority", status: "Status", reporter: "Reporter", assignee: "Assignee" }[field];

  if (field === "reporter" || field === "assignee") {
    try {
      // The initial reporter refresh is intentionally deferred so the first
      // screen can render quickly. A user can therefore reach this modal
      // before that refresh finishes; load the node-scoped model explicitly
      // instead of presenting an empty picker.
      await refreshReporters();
    } catch (e) {
      await showActionError(e, `Could not load ${label.toLowerCase()}s`);
      return;
    }
  }

  let valueControlHtml = "";
  if (field === "priority") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-priority" data-testid="bulk-value-priority" style="width:100%">
        ${BULK_PRIORITY_OPTIONS.map((p) => `<option value="${p}">${p}</option>`).join("")}
      </select>`;
  } else if (field === "status") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-status" data-testid="bulk-value-status" style="width:100%">
        ${BULK_STATUS_OPTIONS.map((s) => `<option value="${s.value}">${htmlEscape(s.label)}</option>`).join("")}
      </select>
      <p class="muted small" style="margin-top:6px">
        Last workflow state sends failed QA attempts back to qa, failed merge
        attempts back to ready-merge, other failed or reviewable Goals back to todo, and leaves active
        automation alone.
      </p>`;
  } else if (field === "reporter") {
    const opts = (state.reporters || [])
      .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
      .join("");
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-reporter" data-testid="bulk-value-reporter" style="width:100%">
        <option value="">— pick reporter —</option>
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        Updates each Goal's original <strong>reporter</strong>.
        Round history keeps its original reporters.
      </p>`;
  } else if (field === "assignee") {
    const opts = (state.reporters || [])
      .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
      .join("");
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-assignee" data-testid="bulk-value-assignee" style="width:100%">
        <option value="">— pick assignee —</option>
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        Updates the latest round's <strong>assignee</strong>, which is each Goal's current owner.
      </p>`;
  }

  const body = () => `
    <div class="modal-title">Bulk set ${htmlEscape(label.toLowerCase())}</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="bulk-value-${field}">New ${htmlEscape(label.toLowerCase())}</label>
      ${valueControlHtml}
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="bulk-cancel">Cancel</button>
      <button data-ok data-testid="bulk-apply">Apply</button>
    </div>`;
  const next = await _openModal(
    body, { cancel: null, ok: "" }, ".modal-input",
  );
  if (next === null) return;
  if (!next) return;          // user opened the picker but didn't choose
  try {
    let r = await api("POST", "/api/goals/bulk", {
      filter, ...selectionFields, update: { [field]: next },
    });
    r = await resolveBackgroundOperationResponse(
      r,
      `Bulk ${label.toLowerCase()} update is running in the background`,
    );
    toast(`Updated ${r.updated} goal${r.updated === 1 ? "" : "s"}`, "info");
    await refreshGoalsListIfCurrent();
  } catch (e) {
    await showActionError(e, "Bulk update failed");
  }
}

function _selectionRequestFields() {
  if (goalsSelectAllMatching) {
    return { exclude_ids: Array.from(goalsExcludedIds) };
  }
  return { selected_ids: Array.from(goalsIncludedIds) };
}

function _hasAnyGoalSelection() {
  return goalsSelectAllMatching || goalsIncludedIds.size > 0;
}

function _selectionCountText(noun = "selected") {
  if (goalsSelectAllMatching) {
    if (goalsExcludedIds.size) {
      return `all matching Goals except ${goalsExcludedIds.size} excluded`;
    }
    return "all matching Goals selected";
  }
  const selectedCount = goalsIncludedIds.size;
  const visibleIds = (_lastGoalsRender?.goals || []).map((g) => g.id);
  const currentPageOnly = visibleIds.length > 0
    && visibleIds.length === selectedCount
    && visibleIds.every((id) => goalsIncludedIds.has(id));
  if (currentPageOnly) {
    return `${selectedCount} Goals on this page ${noun}`;
  }
  return `${selectedCount} explicitly ${noun}`;
}

const GOALS_JIRA_EXPORT_OPERATION_KEY = "refine_goals_jira_export_operation";
let goalsJiraExportPollOperationId = "";
let goalsJiraExportPollPromise = null;
let goalsJiraExportSnapshot = null;
let goalsJiraExportLogs = [];

function goalsJiraExportTerminal(status) {
  return ["complete", "failed", "cancelled", "interrupted"].includes(status);
}

function readGoalsJiraExportOperation() {
  try {
    const raw = localStorage.getItem(GOALS_JIRA_EXPORT_OPERATION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    const operationId = String(parsed?.operationId || "").trim();
    const startedAt = Number(parsed?.startedAt || 0);
    if (!operationId || (startedAt && Date.now() - startedAt > 12 * 60 * 60 * 1000)) {
      localStorage.removeItem(GOALS_JIRA_EXPORT_OPERATION_KEY);
      return null;
    }
    return { operationId, startedAt };
  } catch {
    localStorage.removeItem(GOALS_JIRA_EXPORT_OPERATION_KEY);
    return null;
  }
}

function writeGoalsJiraExportOperation(operationId) {
  if (!operationId) {
    localStorage.removeItem(GOALS_JIRA_EXPORT_OPERATION_KEY);
    return;
  }
  localStorage.setItem(GOALS_JIRA_EXPORT_OPERATION_KEY, JSON.stringify({
    operationId,
    startedAt: Date.now(),
  }));
}

function setGoalsJiraExportButtonLoading(active, message = "Exporting…") {
  const button = $("#bulk-export-jira");
  if (!button) return;
  button.disabled = !!active;
  button.textContent = active ? message : "Export for Jira";
}

function goalsJiraExportOperationHtml(operation, logs = []) {
  const status = String(operation?.status || "running");
  const progress = operation?.progress || {};
  const completed = Math.max(0, Number(progress.completed || 0));
  const total = Math.max(0, Number(progress.total || 0));
  const message = String(
    progress.message
      || operation?.error?.message
      || (status === "complete" ? "Jira CSV ready" : `Jira export ${status}`),
  );
  const terminal = goalsJiraExportTerminal(status);
  const statusLabel = {
    complete: "Complete",
    failed: "Failed",
    cancelled: "Cancelled",
    interrupted: "Interrupted",
    cancelling: "Cancelling",
    pending: "Pending",
    running: "Running",
  }[status] || status;
  const statusClass = status === "complete"
    ? "done"
    : ["failed", "interrupted"].includes(status)
      ? "failed"
      : status === "cancelled"
        ? "cancelled"
        : "in-progress";
  const progressHtml = total > 0 ? `
    <progress value="${Math.min(completed, total)}" max="${total}"
              aria-label="Jira export progress"></progress>
    <span class="muted small" data-testid="goals-jira-export-progress-count">${htmlEscape(`${completed} of ${total}`)}</span>
  ` : `<span class="muted small">Working in the background.</span>`;
  const logHtml = logs.length
    ? logs.slice(-8).map((entry) => {
      const severity = entry.severity === "warning" ? "warn" : (entry.severity || "info");
      return `
        <div class="log-entry ${htmlEscape(severity)}" data-testid="goals-jira-export-log-entry">
          <div>${htmlEscape(entry.message || "Operation update")}</div>
          ${entry.datetime ? `<div class="meta">${htmlEscape(fmtTime(entry.datetime))}</div>` : ""}
        </div>`;
    }).join("")
    : `<p class="muted small" data-testid="goals-jira-export-log-empty">No operation logs yet.</p>`;
  const errorHtml = operation?.error?.message
    ? `<p class="banner error" data-testid="goals-jira-export-error">${htmlEscape(operation.error.message)}</p>`
    : "";
  const operationId = htmlEscape(operation?.id || "");
  return `
    <div class="goals-jira-export-heading">
      <div>
        <strong>Jira export</strong>
        <span class="status-pill ${statusClass}" data-testid="goals-jira-export-status">${htmlEscape(statusLabel)}</span>
      </div>
      <code class="muted small" title="Operation ID">${operationId}</code>
    </div>
    <p data-testid="goals-jira-export-message">${htmlEscape(message)}</p>
    <div class="goals-jira-export-progress" data-testid="goals-jira-export-progress">${progressHtml}</div>
    ${errorHtml}
    <details class="goals-jira-export-logs" data-testid="goals-jira-export-logs"${["failed", "interrupted"].includes(status) ? " open" : ""}>
      <summary>Operation logs (${logs.length})</summary>
      <div>${logHtml}</div>
    </details>
    <div class="actions goals-jira-export-actions">
      ${!terminal ? `<button class="danger secondary" data-jira-export-cancel data-testid="goals-jira-export-cancel">Cancel</button>` : ""}
      ${status === "complete" ? `<button data-jira-export-download data-testid="goals-jira-export-download">Download CSV</button>` : ""}
      ${terminal ? `<button class="secondary" data-jira-export-hide data-testid="goals-jira-export-hide">${status === "complete" ? "Hide" : "Dismiss"}</button>` : ""}
    </div>`;
}

function renderGoalsJiraExportOperation(operation = goalsJiraExportSnapshot, logs = goalsJiraExportLogs) {
  const root = $("#goals-jira-export-operation");
  if (!root) return;
  if (!operation?.id) {
    root.hidden = true;
    root.innerHTML = "";
    return;
  }
  goalsJiraExportSnapshot = operation;
  goalsJiraExportLogs = logs.slice(-8);
  root.hidden = false;
  root.innerHTML = goalsJiraExportOperationHtml(operation, goalsJiraExportLogs);
  root.querySelector("[data-jira-export-cancel]")?.addEventListener("click", () => {
    cancelGoalsJiraExportOperation(operation.id);
  });
  root.querySelector("[data-jira-export-download]")?.addEventListener("click", () => {
    if (operation.status === "complete") downloadGoalsJiraExport(operation.result?.export);
  });
  root.querySelector("[data-jira-export-hide]")?.addEventListener("click", () => {
    hideGoalsJiraExportOperation(operation);
  });
}

async function loadGoalsJiraExportLogs(operationId) {
  try {
    const response = await api(
      "GET",
      `/api/operations/${encodeURIComponent(operationId)}/logs?limit=8`,
    );
    return response.logs || [];
  } catch {
    return goalsJiraExportLogs;
  }
}

async function updateGoalsJiraExportOperation(operation) {
  const logs = await loadGoalsJiraExportLogs(operation.id);
  renderGoalsJiraExportOperation(operation, logs);
  const active = !goalsJiraExportTerminal(operation.status);
  const message = String(operation.progress?.message || "Exporting…").trim();
  setGoalsJiraExportButtonLoading(active, message);
}

async function cancelGoalsJiraExportOperation(operationId) {
  if (!operationId) return;
  const button = $("[data-jira-export-cancel]");
  if (button) {
    button.disabled = true;
    button.textContent = "Cancelling…";
  }
  try {
    const response = await api(
      "POST",
      `/api/operations/${encodeURIComponent(operationId)}/cancel`,
      {},
    );
    await updateGoalsJiraExportOperation(response.operation || {
      ...goalsJiraExportSnapshot,
      id: operationId,
      status: "cancelled",
    });
  } catch (error) {
    await showActionError(error, "Could not cancel Jira export");
  }
}

function hideGoalsJiraExportOperation(operation = goalsJiraExportSnapshot) {
  if (!goalsJiraExportTerminal(operation?.status)) return false;
  writeGoalsJiraExportOperation("");
  goalsJiraExportSnapshot = null;
  goalsJiraExportLogs = [];
  renderGoalsJiraExportOperation();
  setGoalsJiraExportButtonLoading(false);
  return true;
}

function downloadGoalsJiraExport(payload) {
  if (!payload?.csv) throw new Error("Jira export did not return CSV content");
  const blob = new Blob(
    [payload.csv],
    { type: payload.content_type || "text/csv;charset=utf-8" },
  );
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = payload.filename || "refine-goals-jira.csv";
  document.body.appendChild(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
  toast(
    `Exported ${payload.goal_count} Goal${payload.goal_count === 1 ? "" : "s"} for Jira`,
    "success",
  );
}

async function waitForGoalsJiraExportOperation(operationId, allowRecovery = true) {
  if (goalsJiraExportPollOperationId === operationId && goalsJiraExportPollPromise) {
    return await goalsJiraExportPollPromise;
  }
  goalsJiraExportPollOperationId = operationId;
  goalsJiraExportPollPromise = waitForBackgroundOperation(operationId, {
    onStatus: updateGoalsJiraExportOperation,
    onProgress: (progress) => {
      const message = String(progress?.message || "Exporting…").trim();
      setGoalsJiraExportButtonLoading(true, message);
    },
  });
  try {
    return await goalsJiraExportPollPromise;
  } catch (error) {
    if (allowRecovery && error?.code === "operation_interrupted") {
      const recovered = await api(
        "POST",
        `/api/goals/export/jira/${encodeURIComponent(operationId)}/retry`,
        {},
      );
      const recoveredId = recovered?.operation?.id;
      if (!recoveredId) throw error;
      writeGoalsJiraExportOperation(recoveredId);
      goalsJiraExportPollOperationId = "";
      goalsJiraExportPollPromise = null;
      return await waitForGoalsJiraExportOperation(recoveredId, false);
    }
    throw error;
  } finally {
    if (goalsJiraExportPollOperationId === operationId) {
      goalsJiraExportPollOperationId = "";
      goalsJiraExportPollPromise = null;
    }
  }
}

async function resumeGoalsJiraExportOperation() {
  const active = readGoalsJiraExportOperation();
  if (!active) {
    setGoalsJiraExportButtonLoading(false);
    return;
  }
  setGoalsJiraExportButtonLoading(true);
  try {
    const result = await waitForGoalsJiraExportOperation(active.operationId);
    setGoalsJiraExportButtonLoading(false);
    return result;
  } catch (error) {
    setGoalsJiraExportButtonLoading(false);
    if (!goalsJiraExportTerminal(goalsJiraExportSnapshot?.status)) {
      await showActionError(error, "Jira export failed");
    }
  }
}

function syncGoalsJiraExportOperation() {
  if (!readGoalsJiraExportOperation()) {
    setGoalsJiraExportButtonLoading(false);
    renderGoalsJiraExportOperation();
    return;
  }
  if (goalsJiraExportPollOperationId) {
    setGoalsJiraExportButtonLoading(true);
    return;
  }
  resumeGoalsJiraExportOperation();
}

async function exportSelectedGoalsForJira({ button = null } = {}) {
  if (!_hasAnyGoalSelection()) {
    toast("No Goals selected.", "warn");
    return;
  }
  const exportButton = button || $("#bulk-export-jira");
  await withButtonBusy(exportButton, "Exporting…", async () => {
    try {
      const response = await api("POST", "/api/goals/export/jira", {
        filter: goalsBulkFilterFromHash(),
        ..._selectionRequestFields(),
      });
      const operationId = response?.operation?.id;
      if (!operationId) throw new Error("Jira export operation id missing");
      writeGoalsJiraExportOperation(operationId);
      await updateGoalsJiraExportOperation(response.operation);
      await waitForGoalsJiraExportOperation(operationId);
    } catch (error) {
      if (!goalsJiraExportTerminal(goalsJiraExportSnapshot?.status)) {
        await showActionError(error, "Jira export failed");
      }
    }
  });
}

// Highlight each non-default Goals filter control with the accent
// border + show the "Filtered" pill next to the count when any filter
// is active. Called after every table refresh.
function applyGoalsFilterIndicator(f) {
  const active = {
    "search": !!f.q,
    "filter-status": !!f.status,
    "filter-reporter": !!f.reporter,
    "filter-assignee": !!f.assignee,
    "filter-feature": !!f.feature,
    "filter-rounds-gte": !!f.rounds_gte,
    "filter-rounds-lte": !!f.rounds_lte,
    "filter-node": !!f.node && f.node !== "all",
    "goals-severity": !!f.severity,
    "goals-category": !!f.category,
    "goals-actor": !!f.actor,
    "goals-limit": f.limit !== GOALS_DEFAULT_LIMIT,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#goals-filtered");
  if (pill) pill.hidden = !anyActive;
  const tbl = $("#goals-table");
  if (tbl) tbl.classList.toggle("results-filtered", anyActive);
}

async function openBulkTransferNodeModal() {
  const filter = goalsBulkFilterFromHash();
  const filterDesc = describeGoalsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGoalSelection()) {
    toast("No Goals selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected");

  let nodes = state.project?.nodes || [];
  try {
    const snap = await api("GET", "/api/nodes");
    nodes = snap.nodes || [];
    state.project = {
      ...(state.project || {}),
      nodes,
      active_node_id: snap.active_node_id || state.project?.active_node_id || "",
    };
  } catch {
    // Keep the project-status snapshot. The submit call will surface
    // any real schema or registry error.
  }
  const choices = nodes.filter((inst) => !inst.archived);
  if (!choices.length) {
    toast("No active nodes available.", "warn");
    return;
  }
  const opts = choices.map((inst) => `
    <option value="${htmlEscape(inst.id)}">
      ${htmlEscape(inst.display_name || inst.id)}
    </option>`).join("");
  const body = () => `
    <div class="modal-title">Transfer to node</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="bulk-transfer-node-value">Target node</label>
      <select class="modal-input" id="bulk-transfer-node-value" data-testid="bulk-transfer-node-value" style="width:100%">
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        In-progress, qa, ready-merge, and build Goals are skipped.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="bulk-transfer-cancel">Cancel</button>
      <button data-ok data-testid="bulk-transfer-apply">Transfer</button>
    </div>`;
  const target = await _openModal(
    body, { cancel: null, ok: choices[0].id }, ".modal-input",
  );
  if (target === null) return;
  try {
    const r = await api("POST", "/api/nodes/transfer-goals", {
      filter, ...selectionFields, target_node_id: target,
    });
    toast(`Transferred ${r.updated}; skipped ${r.skipped}.`, "info");
    await refreshGoalsListIfCurrent();
  } catch (e) {
    toast(`Transfer failed: ${e.message}`, "error");
  }
}

async function openBulkAssignFeatureModal({ button = null } = {}) {
  const filter = goalsBulkFilterFromHash();
  const filterDesc = describeGoalsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGoalSelection()) {
    toast("No Goals selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected");

  let features = [];
  try {
    const params = new URLSearchParams({
      node: "current",
      limit: "500",
      sort: "updated",
      dir: "desc",
    });
    const data = await api("GET", "/api/features?" + params);
    features = data.features || [];
  } catch (e) {
    await showActionError(e, "Could not load Features");
    return;
  }
  if (!features.length) {
    toast("No current-node Features available.", "warn");
    return;
  }
  const featureChoices = features
    .map((entry) => normalizeFeatureEntry(entry))
    .filter((feature) => feature.id);
  if (!featureChoices.length) {
    toast("No current-node Features available.", "warn");
    return;
  }
  const opts = featureChoices.map((feature) => `
    <option value="${htmlEscape(feature.id)}">
      ${htmlEscape(feature.name || feature.id)}
      · ${htmlEscape(feature.status || "backlog")}
      · ${feature.done_count || 0}/${feature.goal_count || 0} done
    </option>`).join("");
  const body = () => `
    <div class="modal-title">Assign to Feature</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="bulk-assign-feature-value">Feature</label>
      <select class="modal-input" id="bulk-assign-feature-value" data-testid="bulk-assign-feature-value" style="width:100%">
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        Selected Goals already in this Feature or owned by another node are skipped.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="bulk-feature-cancel">Cancel</button>
      <button data-ok data-testid="bulk-feature-apply">Assign</button>
    </div>`;
  const featureId = await _openModal(
    body, { cancel: null, ok: featureChoices[0].id }, ".modal-input",
  );
  if (featureId === null) return;
  await withButtonBusy(button, "Assigning...", async () => {
    const r = await api("POST", `/api/features/${encodeURIComponent(featureId)}/goals/bulk`, {
      filter, ...selectionFields,
    });
    toast(`Assigned ${r.updated}; skipped ${r.skipped}.`, "info");
    await refreshGoalsListIfCurrent();
  });
}

async function confirmBulkDelete() {
  const filter = goalsBulkFilterFromHash();
  const filterDesc = describeGoalsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGoalSelection()) {
    toast("No Goals selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected goals");
  const ok = await modalConfirm(
    `Permanently delete ${countText} (${filterDesc})? This cancels any ` +
    "running subprocesses, removes worktrees and branches for non-done " +
    "Goals, and erases their goal.json files. This cannot be undone.",
    {
      title: "Delete Goals",
      okLabel: `Delete ${countText}`,
      cancelLabel: "Keep them",
      danger: true,
    },
  );
  if (!ok) return;
  try {
    const r = await api("POST", "/api/goals/bulk/delete", {
      filter, ...selectionFields,
    });
    const failedN = (r.failures || []).length;
    if (failedN) {
      toast(`Deleted ${r.deleted} goal${r.deleted === 1 ? "" : "s"}, ` +
            `${failedN} failed.`, "warn");
    } else {
      toast(`Deleted ${r.deleted} goal${r.deleted === 1 ? "" : "s"}.`, "info");
    }
    await refreshGoalsListIfCurrent();
  } catch (e) {
    await showActionError(e, "Bulk delete failed");
  }
}

async function refreshGoalsListIfCurrent() {
  if (state.currentRoute === "goals") await renderGoalsList();
}

function describeGoalsFilter(filter) {
  const parts = [];
  if (filter.status)   parts.push(`status=${filter.status}`);
  if (filter.reporter) parts.push(`reporter=${filter.reporter}`);
  if (filter.assignee) parts.push(`assignee=${filter.assignee}`);
  if (filter.feature)  parts.push(`feature=${filter.feature}`);
  if (filter.rounds_gte) parts.push(`rounds≥${filter.rounds_gte}`);
  if (filter.rounds_lte) parts.push(`rounds≤${filter.rounds_lte}`);
  if (filter.node && filter.node !== "all") parts.push(`node=${filter.node}`);
  if (filter.q)        parts.push(`q="${filter.q}"`);
  if (filter.severity) parts.push(`severity=${filter.severity}`);
  if (filter.category) parts.push(`category=${filter.category}`);
  if (filter.actor)    parts.push(`actor=${filter.actor}`);
  return parts.length ? parts.join(", ") : "all goals";
}

function debounce(fn, ms) {
  let t;
  return (...args) => { clearTimeout(t); t = setTimeout(() => fn(...args), ms); };
}

// Run `fn` while the button shows a busy label and is disabled. Used for
// operations that may take noticeable time (verify, auth recheck, etc.)
// so the user sees that something is happening and can't
// accidentally double-fire the request.
async function withButtonBusy(btn, busyLabel, fn) {
  if (!btn) return await fn();
  const wasDisabled = btn.disabled;
  const orig = btn.textContent;
  const operationLabel = String(orig || busyLabel || "Operation").trim() || "Operation";
  btn.disabled = true;
  btn.textContent = busyLabel;
  recordUiNotice(`${operationLabel} started`, {
    kind: "start",
    source: "ui-operation",
  });
  try {
    const result = await fn();
    recordUiNotice(`${operationLabel} completed`, {
      kind: "complete",
      source: "ui-operation",
    });
    return result;
  } catch (error) {
    recordUiError(`${operationLabel} failed`, {
      source: "ui-operation",
      details: error?.message || String(error || "Request failed"),
    });
    throw error;
  } finally {
    // The button may have been re-rendered by the awaited work (e.g., a
    // reload of the view); setting properties on a detached node is a no-op.
    btn.disabled = wasDisabled;
    btn.textContent = orig;
  }
}
