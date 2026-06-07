function renderImportDraftActionBar({
  start,
  end,
  visibleCount,
  totalCount,
  filtered,
  unresolvedCount,
  selectedCount,
  duplicateCount,
  totalPages,
  page,
  pageAllSelected,
  pageSomeSelected,
  allFilteredSelected,
  updateField,
}) {
  const pageInfo = renderImportDraftRange(start, end, visibleCount, totalCount, filtered);
  return `
    <details class="filter-shell import-review-shell" data-testid="import-review-shell" open>
      <summary>
        <span class="filter-shell-title">Filters &amp; bulk actions</span>
        <span class="filter-pill" data-testid="import-selected-count">${selectedCount} selected</span>
        ${filtered ? `<span class="filter-pill">Needs resolution</span>` : ""}
      </summary>
      <div class="filter-shell-body">
        <div class="filter-bar">
          <div class="filter-row filter-row-primary">
            <label class="import-resolution-filter small">
              <input type="checkbox" data-import-unresolved-filter data-testid="import-unresolved-filter" ${filtered ? "checked" : ""}>
              Needs resolution (${unresolvedCount})
            </label>
            <span class="muted small" data-testid="import-review-range">${htmlEscape(pageInfo)}</span>
            <span class="muted small">${selectedCount} selected</span>
            ${duplicateCount ? `<span class="muted small" data-testid="import-duplicate-count">${duplicateCount} duplicate${duplicateCount === 1 ? "" : "s"}</span>` : ""}
          </div>
          <div class="filter-row filter-row-bulk">
            <span class="muted small">Bulk update selected:</span>
            <button type="button" class="secondary small" data-import-toggle-page data-testid="import-select-page" ${visibleCount ? "" : "disabled"}>
              ${pageAllSelected ? "Deselect page" : "Select page"}
            </button>
            <button type="button" class="secondary small" data-import-toggle-all data-testid="import-select-all" ${visibleCount ? "" : "disabled"}>
              ${allFilteredSelected ? "Deselect all" : "Select all"}
            </button>
            <button type="button" class="secondary small" data-import-select-duplicates data-testid="import-select-duplicates" ${duplicateCount ? "" : "disabled"}>Select duplicates</button>
            <button type="button" class="secondary small" data-import-dismiss-duplicates data-testid="import-dismiss-duplicates" ${duplicateCount ? "" : "disabled"}>Dismiss duplicates</button>
            <button type="button" class="secondary small" data-import-originals data-testid="import-import-selected">Import selected</button>
            <button type="button" class="secondary small" data-import-backlog-originals data-testid="import-move-originals">Move originals to backlog</button>
            <select data-import-update-field data-testid="import-update-field" aria-label="Original Gap field">
              ${["actual", "target", "reporter", "priority"].map((field) => `
                <option value="${field}" ${field === updateField ? "selected" : ""}>${field}</option>`).join("")}
            </select>
            <button type="button" class="secondary small" data-import-update-originals data-testid="import-update-originals">Update originals</button>
          </div>
        </div>
        ${totalPages > 1 ? `<span class="muted small" data-testid="import-review-page-label">Page ${page} of ${totalPages}</span>` : ""}
      </div>
    </details>`;
}

function renderImportDraftRange(start, end, visibleCount, totalCount, filtered) {
  if (!visibleCount) return filtered ? `Showing 0 of ${totalCount}` : "Showing 0";
  const base = `Showing ${start + 1}-${end} of ${visibleCount}`;
  return filtered ? `${base} needing resolution (${totalCount} total)` : `${base} of ${totalCount}`;
}

function importDraftPayload(draft) {
  return {
    name: draft.name.trim(),
    actual: draft.actual.trim(),
    target: draft.target.trim(),
    reporter: draft.reporter.trim(),
    priority: draft.priority,
    node_id: (draft.node_id || "").trim(),
    duplicate_decision: draft.duplicateDecision || "",
  };
}

function renderImportDraftPager(page, totalPages) {
  if (totalPages <= 1) return "";
  return `
    <div class="pagination import-draft-pagination">
      <button type="button" class="secondary small" data-import-page="prev" data-testid="import-page-prev" ${page <= 1 ? "disabled" : ""}>Previous</button>
      <span class="muted small" data-testid="import-page-label">Page ${page} of ${totalPages}</span>
      <button type="button" class="secondary small" data-import-page="next" data-testid="import-page-next" ${page >= totalPages ? "disabled" : ""}>Next</button>
    </div>`;
}

function renderImportDraftTable(pageDrafts, { pageAllSelected, pageSomeSelected, draftCount }) {
  return `
    <table class="table import-drafts-table" data-testid="import-drafts-table">
      <colgroup>
        <col class="import-col-select">
        <col class="import-col-order">
        <col class="import-col-name">
        <col class="import-col-reporter">
        <col class="import-col-priority">
        <col class="import-col-node">
        <col class="import-col-actual">
        <col class="import-col-target">
      </colgroup>
      <thead>
        <tr>
          <th class="gap-select-col">
            <input type="checkbox" data-import-toggle-page-checkbox
                   aria-label="Select page"
                   ${pageAllSelected ? "checked" : ""}
                   data-indeterminate="${pageSomeSelected ? "1" : "0"}">
          </th>
          <th>Order</th>
          <th>Name</th>
          <th>Reporter</th>
          <th>Priority</th>
          <th>Node</th>
          <th>Actual</th>
          <th>Target</th>
        </tr>
      </thead>
      <tbody>
        ${pageDrafts.map(({ draft, index }) => renderImportDraftRow(draft, index, draftCount)).join("")}
      </tbody>
    </table>`;
}

function renderImportDraftRow(d, index, draftCount) {
  return `
    <tr class="draft ${importDraftNeedsResolution(d) ? "needs-resolution" : ""}"
        data-testid="import-draft-row"
        data-idx="${index}" data-duplicate-decision="${htmlEscape(d.duplicateDecision || "")}">
      <td class="gap-select-col">
        <input type="checkbox" data-import-draft-select data-testid="import-draft-select" ${d.selected ? "checked" : ""}
               aria-label="Select draft ${index + 1}">
      </td>
      <td>
        <div class="actions compact-actions">
          <button type="button" class="secondary small" data-import-draft-move="up" data-testid="import-draft-move-up" data-idx="${index}" ${index === 0 ? "disabled" : ""}>Up</button>
          <button type="button" class="secondary small" data-import-draft-move="down" data-testid="import-draft-move-down" data-idx="${index}" ${index >= draftCount - 1 ? "disabled" : ""}>Down</button>
        </div>
      </td>
      <td>
        <input type="text" class="d-name" data-testid="import-draft-name" value="${htmlEscape(d.name)}" placeholder="Name">
        ${d.error ? `<p class="small draft-error" data-testid="import-draft-error">${htmlEscape(d.error)}</p>` : ""}
        ${d.duplicate ? `<p class="muted small import-decision-label" data-testid="import-duplicate-decision">${htmlEscape(importDuplicateDecisionLabel(d.duplicateDecision))}</p>` : ""}
      </td>
      <td><input type="text" class="d-reporter" data-testid="import-draft-reporter" value="${htmlEscape(d.reporter)}" placeholder="Reporter"></td>
      <td>
        <select class="d-priority" data-testid="import-draft-priority">
          ${["low", "medium", "high"].map((priority) => `
            <option value="${priority}" ${d.priority === priority ? "selected" : ""}>${priority}</option>`).join("")}
        </select>
      </td>
      <td><input type="text" class="d-node" data-testid="import-draft-node" value="${htmlEscape(d.node_id || "")}" placeholder="current"></td>
      <td>
        <textarea class="d-actual" data-testid="import-draft-actual" rows="3">${htmlEscape(d.actual)}</textarea>
        ${d.duplicate ? renderImportDuplicateActual(d.duplicate) : ""}
      </td>
      <td>
        <textarea class="d-target" data-testid="import-draft-target" rows="3">${htmlEscape(d.target)}</textarea>
        ${d.duplicate ? renderImportDuplicateTarget(d.duplicate) : ""}
      </td>
    </tr>`;
}

function importDuplicateDecisionLabel(decision) {
  if (decision === "duplicate") return "Duplicate dismissed";
  if (decision === "original") return "Will import as original";
  if (decision === "move_original_to_backlog") return "Will move original to backlog";
  if (decision?.startsWith("update_original_")) {
    return `Will update original ${decision.replace("update_original_", "")}`;
  }
  return "Needs duplicate resolution";
}

function renderImportDuplicateActual(match) {
  return `
    <div class="import-duplicate">
      <div class="small" style="font-weight:600">Possible duplicate</div>
      <p class="muted small" style="margin:4px 0">
        ${htmlEscape(match.name || match.id)} · ${htmlEscape(match.node_display_name || match.node_id || "Default")}
        · ${htmlEscape(match.status || "")}
      </p>
      <div class="small muted">Matched actual</div>
      <p>${htmlEscape(match.actual || "")}</p>
    </div>`;
}

function renderImportDuplicateTarget(match) {
  return `
    <div class="import-duplicate">
      <div class="small muted">Matched target</div>
      <p>${htmlEscape(match.target || "")}</p>
    </div>`;
}

function bindImportDraftPage(root, draftState, saveSession = null, options = {}) {
  const saveReview = () => {
    if (!saveSession) return;
    const destination = typeof options.featureDestination === "function"
      ? options.featureDestination()
      : options.featureDestination;
    saveSession({
      phase: typeof options.phase === "function" ? options.phase() : "review",
      drafts: draftState,
      ...(destination ? { featureDestination: destination } : {}),
    });
  };
  const pageToggle = root.querySelector("[data-import-toggle-page-checkbox]");
  if (pageToggle) {
    pageToggle.indeterminate = pageToggle.dataset.indeterminate === "1";
    pageToggle.addEventListener("change", () => {
      root.querySelector("[data-import-toggle-page]")?.click();
    });
  }
  $$(".draft", root).forEach((row) => {
    const draft = draftState[Number(row.dataset.idx)];
    if (!draft) return;
    row.dataset.duplicateDecision = draft.duplicateDecision || "";
    row.querySelector("[data-import-draft-select]")?.addEventListener("change", (e) => {
      draft.selected = e.target.checked;
      saveReview();
      if (typeof options.onSelectionChange === "function") options.onSelectionChange();
    });
    $$(".import-duplicate-actions button", row).forEach((btn) => {
      btn.classList.toggle(
        "selected",
        btn.dataset.duplicateDecision === row.dataset.duplicateDecision,
      );
      btn.addEventListener("click", () => {
        row.dataset.duplicateDecision = btn.dataset.duplicateDecision;
        draft.duplicateDecision = btn.dataset.duplicateDecision;
        saveReview();
        $$(".import-duplicate-actions button", row).forEach((candidate) => {
          candidate.classList.toggle("selected", candidate === btn);
        });
      });
    });
    $$(".d-name, .d-reporter, .d-priority, .d-actual, .d-target", row).forEach((field) => {
      const syncAndClearError = () => {
        syncImportDraftRow(row, draftState);
        draft.error = "";
        saveReview();
        row.querySelector(".draft-error")?.remove();
      };
      field.addEventListener("input", syncAndClearError);
      field.addEventListener("change", syncAndClearError);
    });
    $$(".d-actual, .d-target", row).forEach((field) => {
      field.addEventListener("input", () => {
        if (!row.querySelector(".import-duplicate")) return;
        row.dataset.duplicateDecision = "";
        draft.duplicateDecision = "";
        draft.duplicate = null;
        draft.error = "";
        saveReview();
        row.querySelector(".import-duplicate")?.remove();
        row.querySelectorAll(".import-duplicate").forEach((el) => el.remove());
        row.querySelector(".draft-error")?.remove();
        row.querySelector(".import-decision-label")?.remove();
      });
    });
  });
}

function syncImportDraftPage(root, draftState) {
  $$(".draft", root).forEach((row) => syncImportDraftRow(row, draftState));
}

function syncImportDraftRow(row, draftState) {
  const draft = draftState[Number(row.dataset.idx)];
  if (!draft) return;
  draft.selected = !!row.querySelector("[data-import-draft-select]")?.checked;
  draft.name = row.querySelector(".d-name")?.value || "";
  draft.actual = row.querySelector(".d-actual")?.value || "";
  draft.target = row.querySelector(".d-target")?.value || "";
  draft.reporter = row.querySelector(".d-reporter")?.value || "";
  draft.priority = row.querySelector(".d-priority")?.value || "low";
  draft.node_id = row.querySelector(".d-node")?.value || "";
  draft.duplicateDecision = row.dataset.duplicateDecision || "";
}
