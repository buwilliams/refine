// ---- Gaps: bulk-update modal ------------------------------------------------
//
// Each bulk action prompts for a new value and confirms the change against
// the current filter-scoped selection. Exactly one field is changed per call
// so the confirmation reads cleanly.

const BULK_PRIORITY_OPTIONS = ["low", "medium", "high"];
const BULK_STATUS_OPTIONS = [
  { value: "__last_workflow_state", label: "(Last workflow state)" },
  { value: "backlog", label: "backlog" },
  { value: "todo", label: "todo" },
  { value: "awaiting-rebuild", label: "awaiting-rebuild" },
  { value: "review", label: "review" },
  { value: "done", label: "done" },
  { value: "failed", label: "failed" },
  { value: "cancelled", label: "cancelled" },
];

function gapsBulkFilterFromHash() {
  const f = gapsFilterFromHash();
  const filter = {};
  for (const key of ["status", "q", "reporter", "feature", "node"]) {
    if (f[key]) filter[key] = f[key];
  }
  if (f.rounds_gte !== "") filter.rounds_gte = parseInt(f.rounds_gte, 10);
  if (f.rounds_lte !== "") filter.rounds_lte = parseInt(f.rounds_lte, 10);
  return filter;
}

async function openBulkModal(field) {
  // Snapshot the current filter for display context; the mutation itself
  // targets all matching Gaps unless the user has switched to an explicit
  // picked-ID selection by clearing the header checkbox.
  const filter = gapsBulkFilterFromHash();
  const filterDesc = describeGapsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGapSelection()) {
    toast("No Gaps selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected");
  const label = { priority: "Priority", status: "Status", reporter: "Reporter" }[field];

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
        attempts back to ready-merge, other failed or reviewable Gaps back to todo, and leaves active
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
        Rewrites the latest round's <strong>reporter</strong> on each Gap.
        Earlier rounds keep their original reporter.
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
    let r = await api("POST", "/api/gaps/bulk", {
      filter, ...selectionFields, update: { [field]: next },
    });
    r = await resolveBackgroundJobResponse(
      r,
      `Bulk ${label.toLowerCase()} update is running in the background`,
    );
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    await refreshGapsListIfCurrent();
  } catch (e) {
    await showActionError(e, "Bulk update failed");
  }
}

function _selectionRequestFields() {
  if (gapsSelectAllMatching) {
    return { exclude_ids: Array.from(gapsExcludedIds) };
  }
  return { selected_ids: Array.from(gapsIncludedIds) };
}

function _hasAnyGapSelection() {
  return gapsSelectAllMatching || gapsIncludedIds.size > 0;
}

function _selectionCountText(noun = "selected") {
  if (gapsSelectAllMatching) {
    if (gapsExcludedIds.size) {
      return `all matching Gaps except ${gapsExcludedIds.size} excluded`;
    }
    return "all matching Gaps selected";
  }
  const selectedCount = gapsIncludedIds.size;
  const visibleIds = (_lastGapsRender?.gaps || []).map((g) => g.id);
  const currentPageOnly = visibleIds.length > 0
    && visibleIds.length === selectedCount
    && visibleIds.every((id) => gapsIncludedIds.has(id));
  if (currentPageOnly) {
    return `${selectedCount} Gaps on this page ${noun}`;
  }
  return `${selectedCount} explicitly ${noun}`;
}

// Highlight each non-default Gaps filter control with the accent
// border + show the "Filtered" pill next to the count when any filter
// is active. Called after every table refresh.
function applyGapsFilterIndicator(f) {
  const active = {
    "search": !!f.q,
    "filter-status": !!f.status,
    "filter-reporter": !!f.reporter,
    "filter-feature": !!f.feature,
    "filter-rounds-gte": !!f.rounds_gte,
    "filter-rounds-lte": !!f.rounds_lte,
    "filter-node": !!f.node && f.node !== "all",
    "gaps-severity": !!f.severity,
    "gaps-category": !!f.category,
    "gaps-actor": !!f.actor,
    "gaps-limit": f.limit !== GAPS_DEFAULT_LIMIT,
  };
  let anyActive = false;
  for (const [id, on] of Object.entries(active)) {
    const el = document.getElementById(id);
    if (!el) continue;
    el.classList.toggle("filter-active", on);
    if (on) anyActive = true;
  }
  const pill = $("#gaps-filtered");
  if (pill) pill.hidden = !anyActive;
  const tbl = $("#gaps-table");
  if (tbl) tbl.classList.toggle("results-filtered", anyActive);
}

async function openBulkTransferNodeModal() {
  const filter = gapsBulkFilterFromHash();
  const filterDesc = describeGapsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGapSelection()) {
    toast("No Gaps selected.", "warn");
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
        In-progress, qa, ready-merge, and awaiting-rebuild Gaps are skipped.
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
    const r = await api("POST", "/api/nodes/transfer-gaps", {
      filter, ...selectionFields, target_node_id: target,
    });
    toast(`Transferred ${r.updated}; skipped ${r.skipped}.`, "info");
    await refreshGapsListIfCurrent();
  } catch (e) {
    toast(`Transfer failed: ${e.message}`, "error");
  }
}

async function openBulkAssignFeatureModal({ button = null } = {}) {
  const filter = gapsBulkFilterFromHash();
  const filterDesc = describeGapsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGapSelection()) {
    toast("No Gaps selected.", "warn");
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
      · ${feature.done_count || 0}/${feature.gap_count || 0} done
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
        Selected Gaps already in this Feature or owned by another node are skipped.
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
    const r = await api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/bulk`, {
      filter, ...selectionFields,
    });
    toast(`Assigned ${r.updated}; skipped ${r.skipped}.`, "info");
    await refreshGapsListIfCurrent();
  });
}

async function confirmBulkDelete() {
  const filter = gapsBulkFilterFromHash();
  const filterDesc = describeGapsFilter(filter);
  const selectionFields = _selectionRequestFields();
  if (!_hasAnyGapSelection()) {
    toast("No Gaps selected.", "warn");
    return;
  }
  const countText = _selectionCountText("selected gaps");
  const ok = await modalConfirm(
    `Permanently delete ${countText} (${filterDesc})? This cancels any ` +
    "running subprocesses, removes worktrees and branches for non-done " +
    "Gaps, and erases their gap.json files. This cannot be undone.",
    {
      title: "Delete Gaps",
      okLabel: `Delete ${countText}`,
      cancelLabel: "Keep them",
      danger: true,
    },
  );
  if (!ok) return;
  try {
    const r = await api("POST", "/api/gaps/bulk/delete", {
      filter, ...selectionFields,
    });
    const failedN = (r.failures || []).length;
    if (failedN) {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}, ` +
            `${failedN} failed.`, "warn");
    } else {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}.`, "info");
    }
    await refreshGapsListIfCurrent();
  } catch (e) {
    await showActionError(e, "Bulk delete failed");
  }
}

async function refreshGapsListIfCurrent() {
  if (state.currentRoute === "gaps") await renderGapsList();
}

function describeGapsFilter(filter) {
  const parts = [];
  if (filter.status)   parts.push(`status=${filter.status}`);
  if (filter.reporter) parts.push(`reporter=${filter.reporter}`);
  if (filter.feature)  parts.push(`feature=${filter.feature}`);
  if (filter.rounds_gte) parts.push(`rounds≥${filter.rounds_gte}`);
  if (filter.rounds_lte) parts.push(`rounds≤${filter.rounds_lte}`);
  if (filter.node && filter.node !== "all") parts.push(`node=${filter.node}`);
  if (filter.q)        parts.push(`q="${filter.q}"`);
  if (filter.severity) parts.push(`severity=${filter.severity}`);
  if (filter.category) parts.push(`category=${filter.category}`);
  if (filter.actor)    parts.push(`actor=${filter.actor}`);
  return parts.length ? parts.join(", ") : "all gaps";
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
