// ---- System -----------------------------------------------------------------

let _targetAppDraftDirty = false;

async function renderSettings() {
  await renderSettingsSurface("settings");
}

async function renderNodeSettings() {
  await renderSettingsSurface("node");
}

async function renderProjectSettings() {
  await renderSettingsSurface("project");
}

async function renderSettingsSurface(route) {
  renderBanners([]);
  const surface = settingsSurfaceForRoute(route);
  // First-paint scaffold only; subsequent refreshes route through
  // `refreshSettings` so SSE / post-save reloads don't flash `Loading…`.
  if (
    !document.getElementById("settings-content") ||
    document.querySelector("#main > h2")?.textContent !== surface.title
  ) {
    $("#main").innerHTML = `<h2>${htmlEscape(surface.title)}</h2><div id="settings-content"><p class="muted">Loading…</p></div>`;
  }
  await refreshSettings();
}

async function refreshSettings(options = {}) {
  if (!isSettingsRoute()) return;
  const surface = settingsSurfaceForRoute();
  const activeSlug = readSettingsTab(surface);
  if (
    _targetAppDraftDirty &&
    !options.force &&
    state.currentRoute === "node" &&
    document.querySelector('[data-tab-pane="target-app"].active')
  ) {
    return;
  }
  try {
    const data = await loadSettingsSurfaceData(surface, activeSlug);
    drawSettingsSurface(surface, data, activeSlug);
  } catch (e) {
    const root = document.getElementById("settings-content");
    if (root) drawRuntimeRecovery(e);
  }
}

async function refreshActiveSettingsTab(options = {}) {
  if (!isSettingsRoute()) return;
  const slug = readSettingsTab(settingsSurfaceForRoute());
  await refreshSettingsTab(slug, options);
}

async function refreshSettingsTab(slug, options = {}) {
  const surface = settingsSurfaceForRoute();
  const activeSlug = normalizeSettingsTab(slug, surface) || readSettingsTab(surface);
  if (!document.querySelector(`[data-tab-pane="${activeSlug}"] .settings-tab-card`)) {
    await refreshSettings(options);
    return;
  }
  if (
    state.currentRoute === "node" &&
    activeSlug === "target-app" &&
    _targetAppDraftDirty &&
    !options.force
  ) {
    return;
  }
  try {
    const data = await loadSettingsSurfaceData(surface, activeSlug);
    updateSettingsTabContent(
      activeSlug,
      renderSettingsTabBody(surface, activeSlug, data),
      () => bindSettingsTabBody(surface, activeSlug, data),
    );
  } catch (e) {
    await showActionError(e);
  }
}

async function loadSettingsSurfaceData() {
  const surface = arguments[0] || settingsSurfaceForRoute();
  const activeSlug = arguments[1] || readSettingsTab(surface);
  const project = await api("GET", "/api/project/status");
  state.project = project;
  updateActiveNodeLabel();
  if (project.attached === false) {
    enterNoProjectMode(project);
    return detachedSettingsSurfaceData(project);
  }
  const needs = settingsSurfaceDataNeeds(surface, activeSlug);
  const [
    s, diag, reps, gov, quality, dash, nodes, guidance, performance, processes, releases,
  ] = await Promise.all([
    needs.settings ? api("GET", "/api/settings") : Promise.resolve({}),
    needs.diagnostics ? api("GET", "/api/diagnostics") : Promise.resolve({}),
    needs.reporters ? api("GET", "/api/reporters") : Promise.resolve({}),
    needs.governance ? api("GET", "/api/governance") : Promise.resolve({}),
    needs.quality ? api("GET", "/api/quality") : Promise.resolve({}),
    needs.dashboard ? api("GET", "/api/dashboard") : Promise.resolve({}),
    needs.nodes ? api("GET", "/api/nodes") : Promise.resolve({}),
    needs.guidance ? api("GET", "/api/guidance") : Promise.resolve({}),
    needs.performance ? api("GET", typeof performanceApiPath === "function"
      ? performanceApiPath()
      : "/api/performance") : Promise.resolve({}),
    needs.processes ? api("GET", "/api/processes") : Promise.resolve({}),
    needs.releases ? api("GET", "/api/system/releases") : Promise.resolve({}),
  ]);
  state.project = project;
  state.reporters = reps.reporters || [];
  updateActiveNodeLabel();
  const settings = s.settings || {};
  const nodeList = nodes.nodes || state.project?.nodes || [];
  const activeNodeId = nodes.active_node_id || state.project?.active_node_id || "";
  const activeNode = nodeList.find((i) => i.id === activeNodeId) || null;
  const activeNodeLabel = activeNode?.display_name || activeNodeId || "Default";
  const projectApps = state.project?.apps || [];
  const currentProject = state.project?.target_root || "";
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  return {
    noProject: false,
    s: settings,
    diag: diag || {},
    reps: state.reporters,
    project: project || {},
    gov: gov || {},
    quality: quality || {},
    dash: dash || {},
    nodes: nodeList,
    nodeCounts: nodes.counts || {},
    activeNodeId,
    activeNodeLabel,
    guidanceItems: guidance.guidance || [],
    performance: performance || {},
    performanceBackend: (performance || {}).backend || (diag || {}).backend || {},
    processes: processes || {},
    releases: releases.releases || { operations: [] },
    cli: (settings.agent_cli || "claude").toLowerCase(),
    projectApps,
    currentProject,
    projectRegistryEnabled: project.registry_enabled !== false,
    appOptions,
  };
}

function settingsSurfaceDataNeeds(surface, slug) {
  const needs = {
    settings: false,
    diagnostics: false,
    reporters: false,
    governance: false,
    quality: false,
    dashboard: false,
    nodes: false,
    guidance: false,
    performance: false,
    processes: false,
    releases: false,
  };
  if (surface === SETTINGS_SURFACES.settings) {
    if (slug === "releases") {
      needs.releases = true;
    } else if (slug === "processes") {
      needs.settings = true;
      needs.diagnostics = true;
      needs.dashboard = true;
      needs.processes = true;
    } else if (slug === "performance") {
      needs.settings = true;
      needs.diagnostics = true;
      needs.reporters = true;
      needs.governance = true;
      needs.dashboard = true;
      needs.nodes = true;
      needs.guidance = true;
      needs.performance = true;
    }
  } else if (surface === SETTINGS_SURFACES.node) {
    if (slug === "releases") {
      needs.releases = true;
    } else if (slug === "application") {
      needs.nodes = true;
    } else if (slug === "reporters") {
      needs.reporters = true;
      needs.nodes = true;
    } else if (slug === "target-app" || slug === "runtime") {
      needs.settings = true;
      needs.nodes = true;
    } else if (slug === "processes") {
      needs.settings = true;
      needs.diagnostics = true;
      needs.dashboard = true;
      needs.processes = true;
    } else if (slug === "performance") {
      needs.settings = true;
      needs.diagnostics = true;
      needs.reporters = true;
      needs.governance = true;
      needs.dashboard = true;
      needs.nodes = true;
      needs.guidance = true;
      needs.performance = true;
    }
  } else if (surface === SETTINGS_SURFACES.project) {
    if (slug === "quality") {
      needs.quality = true;
      needs.settings = true;
    }
    else if (slug === "governance") needs.governance = true;
    else if (slug === "guidance") needs.guidance = true;
  }
  return needs;
}

function detachedSettingsSurfaceData(project = {}) {
  const projectApps = project?.apps || [];
  const currentProject = project?.target_root || "";
  const nodeList = project?.nodes || [];
  const activeNodeId = project?.active_node_id || "";
  const activeNode = nodeList.find((i) => i.id === activeNodeId) || null;
  const activeNodeLabel = activeNode?.display_name || project?.active_node || activeNodeId || "Default";
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  return {
    noProject: true,
    s: {},
    diag: { backend: {} },
    reps: [],
    project: project || {},
    gov: {},
    quality: {},
    dash: {},
    nodes: nodeList,
    nodeCounts: {},
    activeNodeId,
    activeNodeLabel,
    guidanceItems: [],
    performance: {},
    performanceBackend: {},
    processes: {
      runner_reachable: false,
      paused: false,
      processes: [],
      target_app: { state: "unknown" },
    },
    releases: { operations: [] },
    cli: "",
    projectApps,
    currentProject,
    projectRegistryEnabled: project?.registry_enabled !== false,
    appOptions,
    cli: "claude",
  };
}

function updateSettingsTabContent(slug, body, bind) {
  const card = document.querySelector(`[data-tab-pane="${slug}"] .settings-tab-card`);
  if (!card) return;
  const next = document.createElement("div");
  next.innerHTML = body;
  if (card.innerHTML === next.innerHTML) return;
  reconcileSettingsChildren(card, next);
  rearmSettingsControls(card);
  if (typeof bind === "function") bind();
}

async function copySettingsFromNode(section, {
  title = "Copy settings",
  refreshTab = readSettingsTab(),
  button = null,
} = {}) {
  const snap = await api("GET", "/api/nodes");
  const active = snap.active_node_id || state.project?.active_node_id || "";
  const choices = (snap.nodes || []).filter((inst) => inst.id && inst.id !== active);
  if (!choices.length) {
    toast("No other nodes available.", "warn");
    return;
  }
  const opts = choices.map((inst) => `
    <option value="${htmlEscape(inst.id)}">
      ${htmlEscape(inst.display_name || inst.id)}${inst.archived ? " (archived)" : ""}
    </option>`).join("");
  const body = () => `
    <div class="modal-title">${htmlEscape(title)}</div>
    <div class="modal-body">
      <label>${renderSettingsGuideLabel("Source node", "node-copy-settings-source")}</label>
      <select class="modal-input" data-testid="copy-settings-source-node" style="width:100%">
        ${opts}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="copy-settings-cancel">Cancel</button>
      <button data-ok data-testid="copy-settings-submit">Copy</button>
    </div>`;
  const source = await _openModal(
    body, { cancel: null, ok: choices[0].id }, ".modal-input",
  );
  if (source === null) return;
  await withButtonBusy(button, "Copying...", async () => {
    try {
      const r = await api("POST", "/api/nodes/copy-settings", {
        source_node_id: source,
        section,
      });
      toast(`Copied ${r.copied_count || 0} setting${r.copied_count === 1 ? "" : "s"}.`, "info");
      await refreshSettingsTab(refreshTab, { force: true });
    } catch (e) { await showActionError(e, "Copy failed"); }
  });
}

function reconcileSettingsChildren(currentParent, nextParent) {
  const currentChildren = Array.from(currentParent.childNodes);
  const nextChildren = Array.from(nextParent.childNodes);
  const max = Math.max(currentChildren.length, nextChildren.length);
  for (let i = 0; i < max; i += 1) {
    const current = currentChildren[i];
    const next = nextChildren[i];
    if (!current && next) {
      currentParent.appendChild(next.cloneNode(true));
    } else if (current && !next) {
      current.remove();
    } else {
      reconcileSettingsNode(current, next);
    }
  }
}

function reconcileSettingsNode(current, next) {
  if (current.nodeType !== next.nodeType || current.nodeName !== next.nodeName) {
    current.replaceWith(next.cloneNode(true));
    return;
  }
  if (current.nodeType === Node.TEXT_NODE) {
    if (current.nodeValue !== next.nodeValue) current.nodeValue = next.nodeValue;
    return;
  }
  if (current.nodeType !== Node.ELEMENT_NODE) return;
  reconcileSettingsAttributes(current, next);
  reconcileSettingsChildren(current, next);
  reconcileSettingsFormState(current, next);
}

function reconcileSettingsAttributes(current, next) {
  Array.from(current.attributes).forEach((attr) => {
    if (!next.hasAttribute(attr.name)) current.removeAttribute(attr.name);
  });
  Array.from(next.attributes).forEach((attr) => {
    if (current.getAttribute(attr.name) !== attr.value) {
      current.setAttribute(attr.name, attr.value);
    }
  });
}

function reconcileSettingsFormState(current, next) {
  if (current instanceof HTMLInputElement && next instanceof HTMLInputElement) {
    if (current.type === "checkbox" || current.type === "radio") {
      current.checked = next.checked;
    } else if (current.value !== next.value) {
      current.value = next.value;
    }
  } else if (
    (current instanceof HTMLTextAreaElement && next instanceof HTMLTextAreaElement) ||
    (current instanceof HTMLSelectElement && next instanceof HTMLSelectElement)
  ) {
    if (current.value !== next.value) current.value = next.value;
  }
}

function rearmSettingsControls(root) {
  $$("button, input, select, textarea, a, [tabindex], [data-full-details]", root).forEach((el) => {
    const clone = el.cloneNode(true);
    el.replaceWith(clone);
  });
}

function settingsActiveNodeLabel(project = state.project) {
  const nodes = project?.nodes || [];
  const activeId = project?.active_node_id || "";
  const active = project?.active_node
    || nodes.find((i) => i.id === activeId)
    || null;
  return active?.display_name || active?.name || activeId || "Default";
}

function createSettingsAutosave(save, options = {}) {
  let inFlight = false;
  let pending = false;
  return async function autosave() {
    if (inFlight) {
      pending = true;
      return;
    }
    inFlight = true;
    try {
      await save();
      rememberSettingsAutosaveValues(options.controls || []);
      if (typeof options.afterSave === "function") await options.afterSave();
    } catch (e) {
      revertSettingsAutosaveValues(options.controls || []);
      const message = e?.message || "Request failed";
      await modalAlert(
        `${options.errorPrefix || "Save failed"}: ${message}\n\nThe fields were restored to the last saved values.`,
        { title: "Save failed" },
      );
      await refreshActiveSettingsTab({ force: true });
    } finally {
      inFlight = false;
      if (pending) {
        pending = false;
        autosave();
      }
    }
  };
}

function settingsControlValue(el) {
  if (!el) return "";
  if (el.dataset && "enabled" in el.dataset) return el.dataset.enabled || "0";
  if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) {
    return el.checked ? "1" : "0";
  }
  return el.value == null ? "" : String(el.value);
}

function setSettingsControlValue(el, value) {
  if (!el) return;
  if (el.dataset && "enabled" in el.dataset) {
    const enabled = value === "1";
    el.dataset.enabled = enabled ? "1" : "0";
    el.setAttribute("aria-pressed", enabled ? "true" : "false");
    el.classList.toggle("warn", !enabled);
    if (el.id === "s-quality-enabled") {
      el.textContent = enabled ? "QA enabled" : "QA disabled";
    }
  } else if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) {
    el.checked = value === "1";
  } else {
    el.value = value == null ? "" : String(value);
  }
}

function rememberSettingsAutosaveValues(controls) {
  (controls || []).forEach((el) => {
    el.dataset.settingsSavedValue = settingsControlValue(el);
  });
}

function revertSettingsAutosaveValues(controls) {
  (controls || []).forEach((el) => {
    if ("settingsSavedValue" in el.dataset) {
      setSettingsControlValue(el, el.dataset.settingsSavedValue);
    }
  });
}

function bindSettingsAutosave(root, selector, save, options = {}) {
  const controls = root ? $$(selector, root) : [];
  rememberSettingsAutosaveValues(controls);
  const autosave = createSettingsAutosave(save, { ...options, controls });
  if (!root) return autosave;
  controls.forEach((el) => {
    el.addEventListener(options.event || "change", autosave);
  });
  return autosave;
}

function renderSettingsMarkdownField({
  id,
  title,
  value = "",
  scope = "",
  description = "",
  rows = 7,
  guideItemId = "",
}) {
  const htmlId = htmlEscape(id);
  const describedById = `${htmlId}-description`;
  const trimmed = String(value || "").trim();
  const emptyPreview = `No ${htmlEscape(title.toLowerCase())} yet.`;
  return `
    <section class="settings-section settings-markdown-field" data-settings-markdown-field>
      <div class="settings-section-heading">
        <h3>${renderSettingsGuideLabel(title, guideItemId)}</h3>
        <button type="button"
                class="secondary settings-markdown-edit"
                title="Edit ${htmlEscape(title)}"
                aria-label="Edit ${htmlEscape(title)}"
                data-settings-markdown-title="${htmlEscape(title)}"
                data-settings-markdown-empty="${emptyPreview}"
                data-testid="${htmlEscape(id)}-edit"
                data-settings-markdown-edit>
          ${settingsMarkdownIcon("edit")}
        </button>
      </div>
      ${scope ? `<p class="scope-label muted small">${htmlEscape(scope)}</p>` : ""}
      ${description ? `<p class="muted small" id="${describedById}" style="margin-top:0">${htmlEscape(description)}</p>` : ""}
      <div class="settings-markdown-preview" data-settings-markdown-preview>
        ${trimmed ? mdToHtml(value) : `<p class="muted small">${emptyPreview}</p>`}
      </div>
      <textarea id="${htmlId}" rows="${rows}" data-settings-markdown-editor
                data-testid="${htmlId}"
                ${description ? `aria-describedby="${describedById}"` : ""}
                hidden>${htmlEscape(value)}</textarea>
    </section>`;
}

function renderSettingsGuideLabel(title, itemId = "", description = "") {
  return `
    <span class="settings-label-text">${htmlEscape(title)}</span>
    ${renderSettingsGuideIcon(itemId, title)}
    ${description ? `<span class="muted small">— ${htmlEscape(description)}</span>` : ""}`;
}

function renderSettingsGuideIcon(itemId = "", title = "setting") {
  if (!itemId) return "";
  return `
    <button type="button"
            class="settings-guide-icon"
            data-guide-label-item="${htmlEscape(itemId)}"
            data-testid="settings-guide-${htmlEscape(itemId)}"
            tabindex="-1"
            title="Open Guide: ${htmlEscape(title)}"
            aria-label="Open Guide for ${htmlEscape(title)}">
      <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
        <circle cx="12" cy="12" r="9"></circle>
        <path d="M9.8 9.4a2.4 2.4 0 0 1 4.4 1.3c0 1.7-2.2 2.1-2.2 3.8"></path>
        <path d="M12 17.5h.01"></path>
      </svg>
    </button>`;
}

function renderSettingsEditableField({
  id,
  label,
  guideItemId = "",
  description = "",
  control,
  valueLabel = "",
  emptyLabel = "none",
}) {
  const htmlId = htmlEscape(id);
  const title = String(label || "Setting");
  const trimmed = String(valueLabel == null ? "" : valueLabel).trim();
  const preview = trimmed
    ? htmlEscape(trimmed)
    : `<span class="settings-editable-none">${htmlEscape(emptyLabel)}</span>`;
  return `
    <div class="form-row settings-editable-field"
         data-settings-editable-field
         data-settings-editable-title="${htmlEscape(title)}"
         data-settings-empty-label="${htmlEscape(emptyLabel)}">
      <div class="settings-editable-heading">
        <label for="${htmlId}">${renderSettingsGuideLabel(title, guideItemId, description)}</label>
        <button type="button"
                class="secondary settings-editable-toggle"
                title="Edit ${htmlEscape(title)}"
                aria-label="Edit ${htmlEscape(title)}"
                data-testid="${htmlId}-edit"
                data-settings-editable-toggle>
          ${settingsMarkdownIcon("edit")}
        </button>
      </div>
      <div class="settings-editable-preview" data-settings-editable-preview>${preview}</div>
      <div class="settings-editable-editor" data-settings-editable-editor hidden>
        ${control}
      </div>
    </div>`;
}

function settingsMarkdownIcon(name) {
  if (name === "save") {
    return `
      <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
        <path d="M15.2 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V8.8Z"></path>
        <path d="M17 21v-8H7v8"></path>
        <path d="M7 3v5h8"></path>
      </svg>`;
  }
  return `
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M12 20h9"></path>
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z"></path>
    </svg>`;
}

function settingsEditableControl(field) {
  return field?.querySelector("[data-settings-editable-value]") ||
    field?.querySelector("[data-settings-editable-editor] input, [data-settings-editable-editor] select, [data-settings-editable-editor] textarea") ||
    null;
}

function settingsEditablePreviewValue(control) {
  if (!control) return "";
  if (control.dataset.settingsPreviewValue != null) {
    return control.dataset.settingsPreviewValue;
  }
  if (control.tagName === "SELECT") {
    return control.selectedOptions?.[0]?.textContent || control.value || "";
  }
  return control.value == null ? "" : String(control.value);
}

function setSettingsEditableButtonState(btn, editing) {
  if (!btn) return;
  const title = btn.closest("[data-settings-editable-field]")?.dataset.settingsEditableTitle || "setting";
  const action = editing ? "Save" : "Edit";
  btn.dataset.settingsEditableEditing = editing ? "1" : "0";
  btn.title = `${action} ${title}`;
  btn.setAttribute("aria-label", `${action} ${title}`);
  btn.innerHTML = settingsMarkdownIcon(editing ? "save" : "edit");
}

function updateSettingsEditablePreview(field) {
  if (!field) return;
  const preview = field.querySelector("[data-settings-editable-preview]");
  const control = settingsEditableControl(field);
  if (!preview || !control) return;
  if (control.dataset.settingsPreviewHtml) {
    preview.innerHTML = control.dataset.settingsPreviewHtml;
    return;
  }
  const value = settingsEditablePreviewValue(control).trim();
  const empty = field.dataset.settingsEmptyLabel || "none";
  preview.innerHTML = value
    ? htmlEscape(value)
    : `<span class="settings-editable-none">${htmlEscape(empty)}</span>`;
}

function syncSettingsEditableDisabled(control) {
  const field = control?.closest("[data-settings-editable-field]");
  const btn = field?.querySelector("[data-settings-editable-toggle]");
  if (!field || !btn) return;
  btn.disabled = !!control.disabled;
  if (control.disabled) {
    const preview = field.querySelector("[data-settings-editable-preview]");
    const editor = field.querySelector("[data-settings-editable-editor]");
    editor.hidden = true;
    if (preview) preview.hidden = false;
    setSettingsEditableButtonState(btn, false);
  }
}

function editSettingsEditableField(field) {
  if (!field) return;
  const preview = field.querySelector("[data-settings-editable-preview]");
  const editor = field.querySelector("[data-settings-editable-editor]");
  const btn = field.querySelector("[data-settings-editable-toggle]");
  const control = settingsEditableControl(field);
  if (!editor || !control || control.disabled) return;
  preview?.setAttribute("hidden", "");
  editor.hidden = false;
  setSettingsEditableButtonState(btn, true);
  const focusControl = field.querySelector("[data-settings-editable-focus]") || control;
  focusControl.focus();
  if (focusControl instanceof HTMLInputElement && focusControl.type !== "number") {
    focusControl.select();
  }
}

function commitSettingsEditableField(field) {
  if (!field) return;
  const preview = field.querySelector("[data-settings-editable-preview]");
  const editor = field.querySelector("[data-settings-editable-editor]");
  const btn = field.querySelector("[data-settings-editable-toggle]");
  const control = settingsEditableControl(field);
  if (!preview || !editor || !control) return;
  updateSettingsEditablePreview(field);
  editor.hidden = true;
  preview.hidden = false;
  setSettingsEditableButtonState(btn, false);
  if (control.dataset.settingsCommittedValue !== settingsControlValue(control)) {
    control.dataset.settingsCommittedValue = settingsControlValue(control);
    control.dispatchEvent(new Event("settings-editable-commit", { bubbles: true }));
  }
}

function bindSettingsEditableFields(root) {
  if (!root) return;
  $$("[data-settings-editable-field]", root).forEach((field) => {
    const control = settingsEditableControl(field);
    const btn = field.querySelector("[data-settings-editable-toggle]");
    if (!control || !btn) return;
    control.dataset.settingsCommittedValue = settingsControlValue(control);
    updateSettingsEditablePreview(field);
    syncSettingsEditableDisabled(control);
    btn.addEventListener("mousedown", (e) => {
      if (btn.dataset.settingsEditableEditing === "1") e.preventDefault();
    });
    btn.addEventListener("click", () => {
      if (btn.dataset.settingsEditableEditing === "1") {
        commitSettingsEditableField(field);
      } else {
        editSettingsEditableField(field);
      }
    });
    control.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && control.tagName !== "TEXTAREA") {
        e.preventDefault();
        commitSettingsEditableField(field);
      } else if (e.key === "Escape") {
        if ("settingsCommittedValue" in control.dataset) {
          setSettingsControlValue(control, control.dataset.settingsCommittedValue);
        }
        commitSettingsEditableField(field);
      }
    });
    control.addEventListener("change", () => {
      updateSettingsEditablePreview(field);
    });
  });
}

function setSettingsMarkdownButtonState(btn, editing) {
  if (!btn) return;
  const title = btn.dataset.settingsMarkdownTitle || "field";
  const action = editing ? "Save" : "Edit";
  btn.dataset.settingsMarkdownEditing = editing ? "1" : "0";
  btn.title = `${action} ${title}`;
  btn.setAttribute("aria-label", `${action} ${title}`);
  btn.innerHTML = settingsMarkdownIcon(editing ? "save" : "edit");
}

function commitSettingsMarkdownField(field) {
  if (!field) return;
  const preview = field.querySelector("[data-settings-markdown-preview]");
  const editor = field.querySelector("[data-settings-markdown-editor]");
  const btn = field.querySelector("[data-settings-markdown-edit]");
  if (!preview || !editor) return;
  const value = editor.value || "";
  const trimmed = value.trim();
  const empty = btn?.dataset.settingsMarkdownEmpty || "No content yet.";
  preview.innerHTML = trimmed ? mdToHtml(value) : `<p class="muted small">${htmlEscape(empty)}</p>`;
  editor.hidden = true;
  preview.hidden = false;
  setSettingsMarkdownButtonState(btn, false);
  editor.dispatchEvent(new Event("change", { bubbles: true }));
}

function editSettingsMarkdownField(field) {
  if (!field) return;
  const preview = field.querySelector("[data-settings-markdown-preview]");
  const editor = field.querySelector("[data-settings-markdown-editor]");
  const btn = field.querySelector("[data-settings-markdown-edit]");
  if (!editor) return;
  preview?.setAttribute("hidden", "");
  editor.hidden = false;
  setSettingsMarkdownButtonState(btn, true);
  editor.focus();
}

function bindSettingsMarkdownFields(root) {
  if (!root) return;
  $$("[data-settings-markdown-edit]", root).forEach((btn) => {
    btn.addEventListener("mousedown", (e) => {
      const editor = btn.closest("[data-settings-markdown-field]")
        ?.querySelector("[data-settings-markdown-editor]");
      if (editor && !editor.hidden) e.preventDefault();
    });
    btn.addEventListener("click", () => {
      const field = btn.closest("[data-settings-markdown-field]");
      if (!field) return;
      const editor = field.querySelector("[data-settings-markdown-editor]");
      if (editor && !editor.hidden) {
        commitSettingsMarkdownField(field);
      } else {
        editSettingsMarkdownField(field);
      }
    });
  });
  $$("[data-settings-markdown-editor]", root).forEach((editor) => {
    editor.addEventListener("blur", () => {
      if (editor.hidden) return;
      commitSettingsMarkdownField(editor.closest("[data-settings-markdown-field]"));
    });
  });
}

const SETTINGS_SURFACES = {
  settings: {
    title: "Node",
    basePath: "#/node",
    storageKey: "refine_system_tab",
    tabs: [
      { slug: "processes", label: "Processes" },
      { slug: "performance", label: "Performance" },
      { slug: "releases", label: "Refine (dev)" },
    ],
  },
  node: {
    title: "Node",
    basePath: "#/node",
    storageKey: "refine_node_tab",
    tabs: [
      { slug: "application", label: "Application" },
      { slug: "reporters", label: "Reporters" },
      { slug: "processes", label: "Processes" },
      { slug: "performance", label: "Performance" },
      { slug: "target-app", label: "Target App Config" },
      { slug: "runtime", label: "Runtime Config" },
      { slug: "releases", label: "Refine (dev)" },
    ],
  },
  project: {
    title: "Governance",
    basePath: "#/project",
    storageKey: "refine_project_tab",
    tabs: [
      { slug: "governance", label: "Governance" },
      { slug: "quality", label: "Quality" },
      { slug: "guidance", label: "Guidance" },
    ],
  },
};
const SETTINGS_TABS = SETTINGS_SURFACES.settings.tabs;
const INSTANCE_SETTINGS_TABS = SETTINGS_SURFACES.node.tabs;
const PROJECT_SETTINGS_TABS = SETTINGS_SURFACES.project.tabs;

function settingsSurfaceForRoute(route = state.currentRoute) {
  return SETTINGS_SURFACES[route] || SETTINGS_SURFACES.settings;
}

function isSettingsRoute(route = state.currentRoute) {
  return !!SETTINGS_SURFACES[route];
}

function normalizeSettingsTab(slug, surface = settingsSurfaceForRoute()) {
  if (slug === "system") return "processes";
  if (slug === "agents") return "processes";
  if (surface === SETTINGS_SURFACES.node && (slug === "application-config" || slug === "target-app-config")) {
    return "target-app";
  }
  if (slug === "project") return surface.tabs[0]?.slug || null;
  return surface.tabs.some((t) => t.slug === slug) ? slug : null;
}

function activeSettingsTabFromRoute(surface = settingsSurfaceForRoute()) {
  const parsed = typeof parseHash === "function" ? parseHash() : {};
  return parsed.route === state.currentRoute ? normalizeSettingsTab(parsed.tab, surface) : null;
}

function readSettingsTab(tabsOrSurface = settingsSurfaceForRoute()) {
  const surface = Array.isArray(tabsOrSurface)
    ? { ...settingsSurfaceForRoute(), tabs: tabsOrSurface }
    : tabsOrSurface;
  const tabs = surface.tabs;
  const routed = activeSettingsTabFromRoute(surface);
  if (routed) {
    localStorage.setItem(surface.storageKey, routed);
    return routed;
  }
  const parsed = typeof parseHash === "function" ? parseHash() : {};
  if (parsed.route === state.currentRoute && !parsed.tab) {
    const first = tabs[0]?.slug;
    if (first) localStorage.setItem(surface.storageKey, first);
    return first;
  }
  const stored = localStorage.getItem(surface.storageKey);
  const normalizedStored = normalizeSettingsTab(stored, surface);
  if (normalizedStored && tabs.some((t) => t.slug === normalizedStored)) {
    if (normalizedStored !== stored) {
      localStorage.setItem(surface.storageKey, normalizedStored);
    }
    return normalizedStored;
  }
  return tabs[0]?.slug;
}

function setSettingsTab(slug) {
  const surface = settingsSurfaceForRoute();
  const normalized = normalizeSettingsTab(slug, surface);
  if (!normalized) return;
  localStorage.setItem(surface.storageKey, normalized);
  // Toggle classes immediately; hashchange handles normal linked tab
  // navigation, and this keeps repeated clicks on the current hash responsive.
  $$("[data-tab-pane]").forEach((pane) => {
    pane.classList.toggle("active", pane.dataset.tabPane === normalized);
  });
  $$(".settings-tab").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.tabTarget === normalized);
  });
}


function renderSettingsTabStrip(activeSlug, surface = settingsSurfaceForRoute()) {
  const releaseStatus = (surface === SETTINGS_SURFACES.settings || surface === SETTINGS_SURFACES.node)
    ? '<div id="runtime-upgrade-banner" class="settings-release-status" aria-live="polite"></div>'
    : "";
  return `
    <div class="settings-tabs-row">
      <nav class="settings-tabs" id="settings-tabs">
        ${surface.tabs.map((t) => `
          <a class="settings-tab ${t.slug === activeSlug ? "active" : ""}"
             href="${surface.basePath}/${t.slug}"
             data-testid="settings-tab-${htmlEscape(t.slug)}"
             data-tab-target="${t.slug}">${htmlEscape(t.label)}</a>`).join("")}
      </nav>
      ${releaseStatus}
    </div>`;
}

function renderSettingsPane(slug, body, activeSlug) {
  return `
    <section class="settings-pane ${slug === activeSlug ? "active" : ""}"
             data-tab-pane="${slug}"
             data-testid="settings-pane-${htmlEscape(slug)}">
      <div class="card settings-tab-card">${body}</div>
    </section>`;
}

function bindSettingsTabHandlers() {
  $$(".settings-tab", $("#settings-tabs")).forEach((btn) => {
    btn.addEventListener("click", () => {
      setSettingsTab(btn.dataset.tabTarget);
    });
  });
  bindRuntimeUpgradeBanner();
  refreshRuntimeUpgradeBanner();
}

function renderSqliteCacheSection(error = null) {
  return `
    <section class="settings-section">
      <h3>Projection cache</h3>
      ${error ? `<p class="muted small" style="color:var(--error);margin-top:0">${htmlEscape(error.message || String(error))}</p>` : ""}
      <p class="muted small" style="margin-top:0">
        Rebuilds the runtime projection from canonical <code>.refine</code> JSON.
      </p>
      <div class="actions">
        <button class="danger" id="s-rebuild-cache">Rebuild projection cache</button>
      </div>
      <div id="sqlite-cache-progress" style="display:none;margin-top:12px"></div>
    </section>`;
}

function drawSqliteCacheProgress(progress = {}) {
  const root = $("#sqlite-cache-progress");
  if (!root) return;
  const total = Number(progress.total || 0);
  const completed = Number(progress.completed || 0);
  const message = progress.message || (
    total ? `Processing ${completed} of ${total} Goals` : "Rebuilding projection cache"
  );
  const detail = total
    ? `${Math.min(completed, total)} / ${total} Goal${total === 1 ? "" : "s"} processed`
    : "Preparing rebuild";
  root.style.display = "";
  root.innerHTML = `
    <div class="loading-row" style="padding:0">
      <span class="loading-spinner"></span>
      <span>${htmlEscape(message)}</span>
    </div>
    <p class="muted small" style="margin:8px 0 0">${htmlEscape(detail)}</p>
  `;
}

function drawRuntimeRecovery(error) {
  const surface = settingsSurfaceForRoute();
  const activeSlug = surface.tabs.some((t) => t.slug === "runtime")
    ? "runtime"
    : surface.tabs[0]?.slug || "runtime";
  $("#settings-content").innerHTML = `
    ${renderSettingsTabStrip(activeSlug, surface)}
    ${renderSettingsPane(activeSlug, renderSqliteCacheSection(error), activeSlug)}
  `;
  bindSettingsTabHandlers();
  bindRebuildCacheHandler();
}

function bindRebuildCacheHandler() {
  bindCommand("#s-rebuild-cache", "system.cache.rebuild");
}


function renderSettingsTabBody(surface, slug, data) {
  if (data.noProject) {
    if (surface === SETTINGS_SURFACES.node && slug === "application") {
      return renderSettingsApplicationTab({
        projectApps: data.projectApps,
        currentProject: data.currentProject,
        projectRegistryEnabled: data.projectRegistryEnabled,
        appOptions: data.appOptions,
      });
    }
    if (surface === SETTINGS_SURFACES.node && slug === "target-app") {
      return renderDetachedNodeConfig(
        renderNodeApplicationConfigSections({
          s: data.s || {},
          activeNodeLabel: data.activeNodeLabel,
        }),
      );
    }
    if (surface === SETTINGS_SURFACES.node && slug === "runtime") {
      return renderDetachedNodeConfig(
        renderNodeRuntimeConfigSections(data.s || {}, data.activeNodeLabel, data.cli || "claude"),
      );
    }
    return renderSettingsNoProjectTab(surface.title);
  }
  if (surface === SETTINGS_SURFACES.settings) {
    if (slug === "releases") {
      return renderSettingsReleasesTab(data.releases);
    }
    if (slug === "processes") {
      return renderProcessesTab(data.processes, data.s, data.diag, data.dash);
    }
    if (slug === "performance") {
      return renderSettingsPerformanceTab(data.performance, data.performanceBackend);
    }
  }
  if (surface === SETTINGS_SURFACES.node) {
    if (slug === "releases") {
      return renderSettingsReleasesTab(data.releases);
    }
    if (slug === "processes") {
      return renderProcessesTab(data.processes, data.s, data.diag, data.dash);
    }
    if (slug === "performance") {
      return renderSettingsPerformanceTab(data.performance, data.performanceBackend);
    }
    if (slug === "reporters") {
      return renderSettingsReportersTab(data.reps, data.activeNodeLabel);
    }
    if (slug === "application") {
      return `
        ${renderSettingsApplicationTab({
          projectApps: data.projectApps,
          currentProject: data.currentProject,
          projectRegistryEnabled: data.projectRegistryEnabled,
          appOptions: data.appOptions,
        })}
        ${renderSettingsNodesTab({
          nodes: data.nodes,
          nodeCounts: data.nodeCounts,
          activeNodeId: data.activeNodeId,
        })}`;
    }
    if (slug === "target-app") {
      return renderNodeApplicationConfigSections({
        s: data.s,
        activeNodeLabel: data.activeNodeLabel,
      });
    }
    if (slug === "runtime") {
      return renderNodeRuntimeConfigSections(data.s, data.activeNodeLabel, data.cli);
    }
  }
  if (surface === SETTINGS_SURFACES.project) {
    if (slug === "quality") return renderSettingsQualityTab(data.quality, data.s);
    if (slug === "governance") return renderSettingsGovernanceTab(data.gov);
    if (slug === "guidance") return renderSettingsGuidanceTab(data.guidanceItems);
  }
  return `<p class="muted">Unknown settings tab.</p>`;
}

function renderSettingsNoProjectTab(title = "Settings") {
  return `
    <section class="settings-section" data-testid="settings-no-project">
      <h3>No app configured.</h3>
      <p class="muted">Open the Guide to configure Refine and attach an app before using ${htmlEscape(title)} settings.</p>
      <button type="button" class="secondary" data-settings-open-guide data-testid="settings-open-guide">Open Guide</button>
    </section>`;
}

function renderDetachedNodeConfig(body) {
  return `
    <section class="settings-section" data-testid="settings-detached-config">
      <h3>No app attached.</h3>
      <p class="muted">
        Node configuration is shown for reference. Attach an app before saving
        application or runtime settings.
      </p>
      <button type="button" class="secondary" data-settings-open-guide data-testid="settings-open-guide">Open Guide</button>
    </section>
    ${disableSettingsControls(body)}`;
}

function disableSettingsControls(markup) {
  return markup.replace(/<(input|select|textarea|button)\b(?![^>]*\bdisabled\b)/g, "<$1 disabled");
}

function bindSettingsNoProjectTab() {
  $$("[data-settings-open-guide]").forEach((button) => {
    button.addEventListener("click", () => {
      if (typeof openGuide === "function") {
        openGuide({
          context: "no-app",
          categoryId: "get-started",
          itemId: "quickstart-add-app",
          openTarget: true,
        });
      }
    });
  });
}

function bindSettingsTabBody(surface, slug, data) {
  if (data.noProject) {
    if (surface === SETTINGS_SURFACES.node && slug === "application") {
      bindSettingsApplicationTab(data.currentProject);
    } else {
      bindSettingsNoProjectTab();
    }
    return;
  }
  if (surface === SETTINGS_SURFACES.settings) {
    if (slug === "releases") bindSettingsReleasesTab(data.releases);
    else if (slug === "processes") bindSettingsProcessesTab(data.s);
    else if (slug === "performance") {
      bindSettingsPerformanceTab(
        data.s, data.diag, data.reps, null, data.gov,
        data.dash, { nodes: data.nodes, counts: data.nodeCounts },
        { guidance: data.guidanceItems }, data.performanceBackend,
      );
    }
  } else if (surface === SETTINGS_SURFACES.node) {
    if (slug === "releases") bindSettingsReleasesTab(data.releases);
    else if (slug === "processes") bindSettingsProcessesTab(data.s);
    else if (slug === "performance") {
      bindSettingsPerformanceTab(
        data.s, data.diag, data.reps, null, data.gov,
        data.dash, { nodes: data.nodes, counts: data.nodeCounts },
        { guidance: data.guidanceItems }, data.performanceBackend,
      );
    }
    else if (slug === "reporters") bindSettingsReportersTab();
    else if (slug === "application") {
      bindSettingsApplicationTab(data.currentProject);
      bindSettingsNodesTab();
    }
    else if (slug === "target-app") bindNodeApplicationConfigControls();
    else if (slug === "runtime") bindNodeRuntimeConfigControls();
  } else if (surface === SETTINGS_SURFACES.project) {
    if (slug === "quality") bindSettingsQualityTab();
    else if (slug === "governance") bindSettingsGovernanceTab();
    else if (slug === "guidance") bindSettingsGuidanceTab(data.guidanceItems);
  }
}

function drawSettingsSurface(surface, data, activeSlugOverride = null) {
  const root = document.getElementById("settings-content");
  if (!root) return;
  const activeSlug = activeSlugOverride || readSettingsTab(surface);
  const tabStrip = renderSettingsTabStrip(activeSlug, surface);
  const pane = (tab) => renderSettingsPane(
    tab.slug,
    tab.slug === activeSlug ? renderSettingsTabBody(surface, tab.slug, data) : "",
    activeSlug,
  );
  $("#settings-content").innerHTML = `
    ${tabStrip}
    ${surface.tabs.map(pane).join("")}
  `;

  bindSettingsTabHandlers();
  bindSettingsTabBody(surface, activeSlug, data);
}
