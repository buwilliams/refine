// ---- Gaps: bulk-update modal ------------------------------------------------
//
// Each bulk action prompts for a new value and confirms the change against
// the *current* filter (read from the URL hash at click time). The server
// re-runs the filter query, so what the user sees in the table is what gets
// updated — no client-side ID list to drift out of sync. Exactly one field
// is changed per call so the confirmation reads cleanly.

const BULK_PRIORITY_OPTIONS = ["low", "medium", "high"];
const BULK_STATUS_OPTIONS = [
  "backlog", "todo", "awaiting-rebuild", "review",
  "done", "failed", "cancelled",
];

async function openBulkModal(field) {
  // Snapshot the current filter so the modal + the server-side bulk
  // operation see exactly what the user sees in the table.
  const f = gapsFilterFromHash();
  const filter = {
    status: f.status, q: f.q, reporter: f.reporter,
    instance: f.instance,
    severity: f.severity, category: f.category, actor: f.actor,
  };
  const filterDesc = describeGapsFilter(filter);
  // When the filter shell is open, the user may have unchecked some of
  // the rows. Translate that into an explicit exclude list and a
  // selected-count for the modal text.
  const excludeIds = _selectionSnapshot();
  const matchingCount = _lastGapsRender?.gaps?.length || 0;
  const selectedCount = matchingCount - excludeIds.filter(
    (id) => (_lastGapsRender?.gaps || []).some((g) => g.id === id),
  ).length;
  const countText = excludeIds.length && _lastGapsRender
    ? `${selectedCount} of ${matchingCount} selected`
    : ($("#gaps-count")?.textContent || "").trim();
  const label = { priority: "Priority", status: "Status", reporter: "Reporter" }[field];

  let valueControlHtml = "";
  if (field === "priority") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-priority" style="width:100%">
        ${BULK_PRIORITY_OPTIONS.map((p) => `<option value="${p}">${p}</option>`).join("")}
      </select>`;
  } else if (field === "status") {
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-status" style="width:100%">
        ${BULK_STATUS_OPTIONS.map((s) => `<option value="${s}">${s}</option>`).join("")}
      </select>
      <p class="muted small" style="margin-top:6px">
        Bulk status updates skip in-progress and ready-merge Gaps.
        Use per-Gap workflow actions for automated states.
      </p>`;
  } else if (field === "reporter") {
    const opts = (state.reporters || [])
      .map((r) => `<option value="${htmlEscape(r.name)}">${htmlEscape(r.name)}</option>`)
      .join("");
    valueControlHtml = `
      <select class="modal-input" id="bulk-value-reporter" style="width:100%">
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
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Apply</button>
    </div>`;
  const next = await _openModal(
    body, { cancel: null, ok: "" }, ".modal-input",
  );
  if (next === null) return;
  if (!next) return;          // user opened the picker but didn't choose
  try {
    let r = await api("POST", "/api/gaps/bulk", {
      filter, exclude_ids: excludeIds, update: { [field]: next },
    });
    r = await resolveBackgroundJobResponse(
      r,
      `Bulk ${label.toLowerCase()} update is running in the background`,
    );
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    // Preserve the user's unchecked rows across the refresh — they
    // explicitly opted those out of the operation that just ran and
    // will likely want them excluded from follow-up actions too.
    // Stale IDs (rows that no longer match the filter) are harmless;
    // they're just ignored at the next selection-state pass.
    await renderGapsList();
  } catch (e) {
    await showActionError(e, "Bulk update failed");
  }
}

// Frozen-at-call-time copy of the user's deselected IDs (so a slow
// network request doesn't see live edits).
function _selectionSnapshot() {
  return Array.from(gapsExcludedIds);
}

// Highlight each non-default Gaps filter control with the accent
// border + show the "Filtered" pill next to the count when any filter
// is active. Called after every table refresh.
function applyGapsFilterIndicator(f) {
  const active = {
    "search": !!f.q,
    "filter-status": !!f.status,
    "filter-reporter": !!f.reporter,
    "filter-instance": !!f.instance,
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

async function openBulkTransferInstanceModal() {
  const f = gapsFilterFromHash();
  const filter = {
    status: f.status, q: f.q, reporter: f.reporter,
    instance: f.instance,
    severity: f.severity, category: f.category, actor: f.actor,
  };
  const filterDesc = describeGapsFilter(filter);
  const excludeIds = _selectionSnapshot();
  const matchingCount = _lastGapsRender?.gaps?.length || 0;
  const selectedCount = matchingCount - excludeIds.filter(
    (id) => (_lastGapsRender?.gaps || []).some((g) => g.id === id),
  ).length;
  const countText = excludeIds.length && _lastGapsRender
    ? `${selectedCount} of ${matchingCount} selected`
    : ($("#gaps-count")?.textContent || "").trim();

  let instances = state.project?.instances || [];
  try {
    const snap = await api("GET", "/api/instances");
    instances = snap.instances || [];
    state.project = {
      ...(state.project || {}),
      instances,
      active_instance_id: snap.active_instance_id || state.project?.active_instance_id || "",
    };
  } catch {
    // Keep the project-status snapshot. The submit call will surface
    // any real schema or registry error.
  }
  const choices = instances.filter((inst) => !inst.archived);
  if (!choices.length) {
    toast("No active instances available.", "warn");
    return;
  }
  const opts = choices.map((inst) => `
    <option value="${htmlEscape(inst.id)}">
      ${htmlEscape(inst.display_name || inst.id)}
    </option>`).join("");
  const body = () => `
    <div class="modal-title">Transfer to instance</div>
    <div class="modal-body">
      <div class="muted small" style="margin-bottom:8px">
        Applies to ${htmlEscape(countText || "all matching")} —
        ${htmlEscape(filterDesc)}.
      </div>
      <label for="bulk-transfer-instance-value">Target instance</label>
      <select class="modal-input" id="bulk-transfer-instance-value" style="width:100%">
        ${opts}
      </select>
      <p class="muted small" style="margin-top:6px">
        In-progress, ready-merge, and awaiting-rebuild Gaps are skipped.
      </p>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Transfer</button>
    </div>`;
  const target = await _openModal(
    body, { cancel: null, ok: choices[0].id }, ".modal-input",
  );
  if (target === null) return;
  try {
    const r = await api("POST", "/api/instances/transfer-gaps", {
      filter, exclude_ids: excludeIds, target_instance_id: target,
    });
    toast(`Transferred ${r.updated}; skipped ${r.skipped}.`, "info");
    await renderGapsList();
  } catch (e) {
    toast(`Transfer failed: ${e.message}`, "error");
  }
}

async function confirmBulkDelete() {
  const f = gapsFilterFromHash();
  const filter = {
    status: f.status, q: f.q, reporter: f.reporter,
    instance: f.instance,
    severity: f.severity, category: f.category, actor: f.actor,
  };
  const filterDesc = describeGapsFilter(filter);
  const excludeIds = _selectionSnapshot();
  const matchingCount = _lastGapsRender?.gaps?.length || 0;
  const selectedCount = matchingCount - excludeIds.filter(
    (id) => (_lastGapsRender?.gaps || []).some((g) => g.id === id),
  ).length;
  const countText = excludeIds.length && _lastGapsRender
    ? `${selectedCount} of ${matchingCount} selected gaps`
    : (($("#gaps-count")?.textContent || "matching gaps").trim());
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
      filter, exclude_ids: excludeIds,
    });
    const failedN = (r.failures || []).length;
    if (failedN) {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}, ` +
            `${failedN} failed.`, "warn");
    } else {
      toast(`Deleted ${r.deleted} gap${r.deleted === 1 ? "" : "s"}.`, "info");
    }
    // Preserve the user's unchecked rows so follow-up bulk actions
    // continue to skip them. IDs of deleted gaps drop out of the next
    // fetch naturally — they remain in the set but are inert.
    await renderGapsList();
  } catch (e) {
    await showActionError(e, "Bulk delete failed");
  }
}

function describeGapsFilter(filter) {
  const parts = [];
  if (filter.status)   parts.push(`status=${filter.status}`);
  if (filter.reporter) parts.push(`reporter=${filter.reporter}`);
  if (filter.instance) parts.push(`instance=${filter.instance}`);
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
  btn.disabled = true;
  btn.textContent = busyLabel;
  try {
    return await fn();
  } finally {
    // The button may have been re-rendered by the awaited work (e.g., a
    // reload of the view); setting properties on a detached node is a no-op.
    btn.disabled = wasDisabled;
    btn.textContent = orig;
  }
}
