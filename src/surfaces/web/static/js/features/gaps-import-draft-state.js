function normalizeImportDraft(draft) {
  return {
    name: draft.name || "",
    actual: draft.actual || "",
    target: draft.target || "",
    reporter: draft.reporter || state.lastReporter || "",
    priority: String(draft.priority || "low").toLowerCase(),
    dependency_names: Array.isArray(draft.dependency_names)
      ? draft.dependency_names
      : Array.isArray(draft.depends_on)
        ? draft.depends_on
        : [],
    duplicate: draft.duplicate || null,
    duplicateDecision: draft.duplicateDecision || "",
    node_id: draft.node_id || "",
    selected: !!draft.selected,
    error: draft.error || "",
  };
}

function importDraftNeedsResolution(draft) {
  if (importDraftHiddenFromReview(draft)) return false;
  return !!draft.error || (!!draft.duplicate && !draft.duplicateDecision);
}

function importDraftHiddenFromReview(draft) {
  return draft.duplicateDecision === "duplicate";
}

function importDraftCreatesGap(draft) {
  const decision = draft.duplicateDecision || "";
  return !(
    decision === "duplicate"
    || decision === "move_original_to_backlog"
    || decision.startsWith("update_original_")
  );
}

function importDraftCreateCount(drafts) {
  return drafts.filter(importDraftCreatesGap).length;
}

function updateImportPersistButton(root, draftState, featureDestination = null) {
  const btn = root.querySelector("#btn-persist");
  if (!btn) return;
  const count = importDraftCreateCount(draftState);
  const destination = normalizeImportFeatureDestination(featureDestination);
  const suffix = destination.mode === "new"
    ? " to new Feature"
    : destination.mode === "existing"
      ? " to Feature"
      : "";
  btn.textContent = `Save (${count}) gap${count === 1 ? "" : "s"}${suffix}`;
}

function normalizeImportFeatureDestination(raw = null) {
  const mode = ["standalone", "new", "existing"].includes(raw?.mode)
    ? raw.mode
    : "standalone";
  return {
    mode,
    newName: String(raw?.newName || ""),
    newDescription: String(raw?.newDescription || ""),
    existingId: String(raw?.existingId || ""),
  };
}

function renderImportFeatureDestination(destination) {
  const dest = normalizeImportFeatureDestination(destination);
  return `
    <div class="import-feature-destination" data-testid="import-feature-destination">
      <div class="small" style="font-weight:600">Save destination</div>
      <div class="filter-row">
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="standalone" data-testid="import-feature-mode-standalone" ${dest.mode === "standalone" ? "checked" : ""}>
          <span>Standalone Gaps</span>
        </label>
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="new" data-testid="import-feature-mode-new" ${dest.mode === "new" ? "checked" : ""}>
          <span>New Feature</span>
        </label>
        <label class="checkbox-row">
          <input type="radio" name="import-feature-mode" value="existing" data-testid="import-feature-mode-existing" ${dest.mode === "existing" ? "checked" : ""}>
          <span>Existing Feature</span>
        </label>
      </div>
      <div class="import-feature-destination-fields" data-import-feature-fields="new" ${dest.mode === "new" ? "" : "hidden"}>
        <div class="form-row">
          <label>Feature name</label>
          <input type="text" data-import-feature-new-name data-testid="import-feature-new-name" value="${htmlEscape(dest.newName)}" placeholder="Settings redesign">
        </div>
        <div class="form-row">
          <label>Feature description</label>
          <textarea data-import-feature-new-description data-testid="import-feature-new-description" rows="3">${htmlEscape(dest.newDescription)}</textarea>
        </div>
      </div>
      <div class="import-feature-destination-fields" data-import-feature-fields="existing" ${dest.mode === "existing" ? "" : "hidden"}>
        <div class="form-row">
          <label>Feature</label>
          <select data-import-feature-existing data-testid="import-feature-existing" class="modal-input" data-selected="${htmlEscape(dest.existingId)}">
            <option value="">Loading Features...</option>
          </select>
        </div>
      </div>
      <p class="muted small" data-import-feature-summary data-testid="import-feature-summary">${htmlEscape(importFeatureDestinationSummary(dest))}</p>
    </div>`;
}

function bindImportFeatureDestination(root, onChange) {
  const apply = () => {
    const dest = readImportFeatureDestination(root);
    root.querySelectorAll("[data-import-feature-fields]").forEach((el) => {
      el.hidden = el.dataset.importFeatureFields !== dest.mode;
    });
    const summary = root.querySelector("[data-import-feature-summary]");
    if (summary) summary.textContent = importFeatureDestinationSummary(dest);
    onChange(dest);
  };
  root.querySelectorAll("input[name='import-feature-mode']").forEach((input) => {
    input.addEventListener("change", apply);
  });
  root.querySelector("[data-import-feature-new-name]")?.addEventListener("input", debounce(apply, 150));
  root.querySelector("[data-import-feature-new-description]")?.addEventListener("input", debounce(apply, 150));
  const select = root.querySelector("[data-import-feature-existing]");
  if (select) {
    select.addEventListener("change", apply);
    populateImportFeatureSelect(select).then(apply).catch(() => {
      select.innerHTML = `<option value="">Could not load Features</option>`;
    });
  }
}

function readImportFeatureDestination(root) {
  return normalizeImportFeatureDestination({
    mode: root.querySelector("input[name='import-feature-mode']:checked")?.value || "standalone",
    newName: root.querySelector("[data-import-feature-new-name]")?.value || "",
    newDescription: root.querySelector("[data-import-feature-new-description]")?.value || "",
    existingId: root.querySelector("[data-import-feature-existing]")?.value || "",
  });
}

async function populateImportFeatureSelect(select) {
  const selected = select.dataset.selected || "";
  const data = await api("GET", "/api/features?limit=100&node=current");
  const features = (data.features || [])
    .map((item) => item.feature || item)
    .filter((feature) => feature?.id);
  select.innerHTML = features.length
    ? features.map((feature) => `
        <option value="${htmlEscape(feature.id)}" ${feature.id === selected ? "selected" : ""}>
          ${htmlEscape(feature.name || feature.id)} · ${htmlEscape(feature.status || "backlog")} · ${feature.done_count || 0}/${feature.gap_count || 0} done
        </option>`).join("")
    : `<option value="">No Features available</option>`;
}

function importFeatureDestinationSummary(dest) {
  if (dest.mode === "new") {
    return dest.newName
      ? `Creates Feature "${dest.newName}" and saves imported Gaps in reviewed order.`
      : "Creates a new Feature and saves imported Gaps in reviewed order.";
  }
  if (dest.mode === "existing") {
    return dest.existingId
      ? `Appends imported Gaps to Feature ${dest.existingId} in reviewed order.`
      : "Choose an existing Feature before saving.";
  }
  return "Saves imported Gaps as standalone Gaps.";
}

function importFeatureDestinationPayload(destination) {
  const dest = normalizeImportFeatureDestination(destination);
  if (dest.mode === "new") {
    if (!dest.newName.trim()) {
      throw new Error("Feature name is required");
    }
    return {
      new_feature_name: dest.newName.trim(),
      new_feature_description: dest.newDescription.trim(),
    };
  }
  if (dest.mode === "existing") {
    if (!dest.existingId.trim()) {
      throw new Error("Choose a Feature before saving");
    }
    return { feature_id: dest.existingId.trim() };
  }
  return {};
}
