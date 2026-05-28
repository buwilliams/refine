// ---- System -----------------------------------------------------------------

let _targetAppDraftDirty = false;

async function renderSettings() {
  await renderSettingsSurface("settings");
}

async function renderInstanceSettings() {
  await renderSettingsSurface("instance");
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
  if (
    _targetAppDraftDirty &&
    !options.force &&
    state.currentRoute === "instance" &&
    document.querySelector('[data-tab-pane="application"].active')
  ) {
    return;
  }
  try {
    const data = await loadSettingsSurfaceData();
    drawSettingsSurface(surface, data);
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
    state.currentRoute === "instance" &&
    activeSlug === "application" &&
    _targetAppDraftDirty &&
    !options.force
  ) {
    return;
  }
  try {
    const data = await loadSettingsSurfaceData();
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
  const [
    s, diag, reps, project, gov, quality, dash, instances, guidance,
    performance, processes,
  ] = await Promise.all([
    api("GET", "/api/settings"),
    api("GET", "/api/diagnostics"),
    api("GET", "/api/reporters"),
    api("GET", "/api/project/status"),
    api("GET", "/api/governance"),
    api("GET", "/api/quality"),
    api("GET", "/api/dashboard"),
    api("GET", "/api/instances"),
    api("GET", "/api/guidance"),
    api("GET", typeof performanceApiPath === "function"
      ? performanceApiPath()
      : "/api/performance"),
    api("GET", "/api/processes"),
  ]);
  state.project = project;
  state.reporters = reps.reporters || [];
  updateActiveInstanceLabel();
  const settings = s.settings || {};
  const instanceList = instances.instances || state.project?.instances || [];
  const activeInstanceId = instances.active_instance_id || state.project?.active_instance_id || "";
  const activeInstance = instanceList.find((i) => i.id === activeInstanceId) || null;
  const activeInstanceLabel = activeInstance?.display_name || activeInstanceId || "Default";
  const projectApps = state.project?.apps || [];
  const currentProject = state.project?.client_repo || "";
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  return {
    s: settings,
    diag: diag || {},
    reps: state.reporters,
    project: project || {},
    gov: gov || {},
    quality: quality || {},
    dash: dash || {},
    instances: instanceList,
    instanceCounts: instances.counts || {},
    activeInstanceId,
    activeInstanceLabel,
    guidanceItems: guidance.guidance || [],
    performance: performance || {},
    performanceBackend: (performance || {}).backend || (diag || {}).backend || {},
    processes: processes || {},
    cli: (settings.agent_cli || "claude").toLowerCase(),
    projectApps,
    currentProject,
    projectRegistryEnabled: project.registry_enabled !== false,
    appOptions,
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

async function copySettingsFromInstance(section, {
  title = "Copy settings",
  refreshTab = readSettingsTab(),
  button = null,
} = {}) {
  const snap = await api("GET", "/api/instances");
  const active = snap.active_instance_id || state.project?.active_instance_id || "";
  const choices = (snap.instances || []).filter((inst) => inst.id && inst.id !== active);
  if (!choices.length) {
    toast("No other instances available.", "warn");
    return;
  }
  const opts = choices.map((inst) => `
    <option value="${htmlEscape(inst.id)}">
      ${htmlEscape(inst.display_name || inst.id)}${inst.archived ? " (archived)" : ""}
    </option>`).join("");
  const body = () => `
    <div class="modal-title">${htmlEscape(title)}</div>
    <div class="modal-body">
      <label>Source instance</label>
      <select class="modal-input" style="width:100%">
        ${opts}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>Cancel</button>
      <button data-ok>Copy</button>
    </div>`;
  const source = await _openModal(
    body, { cancel: null, ok: choices[0].id }, ".modal-input",
  );
  if (source === null) return;
  await withButtonBusy(button, "Copying...", async () => {
    try {
      const r = await api("POST", "/api/instances/copy-settings", {
        source_instance_id: source,
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

function settingsActiveInstanceLabel(project = state.project) {
  const instances = project?.instances || [];
  const activeId = project?.active_instance_id || "";
  const active = project?.active_instance
    || instances.find((i) => i.id === activeId)
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
    } else if (el.id === "s-quality-regressions-enabled") {
      el.textContent = enabled ? "Regressions enabled" : "Regressions disabled";
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
}) {
  const htmlId = htmlEscape(id);
  const describedById = `${htmlId}-description`;
  const trimmed = String(value || "").trim();
  const emptyPreview = `No ${htmlEscape(title.toLowerCase())} yet.`;
  return `
    <section class="settings-section settings-markdown-field" data-settings-markdown-field>
      <div class="settings-section-heading">
        <h3>${htmlEscape(title)}</h3>
        <button type="button"
                class="secondary settings-markdown-edit"
                title="Edit ${htmlEscape(title)}"
                aria-label="Edit ${htmlEscape(title)}"
                data-settings-markdown-title="${htmlEscape(title)}"
                data-settings-markdown-empty="${emptyPreview}"
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
                ${description ? `aria-describedby="${describedById}"` : ""}
                hidden>${htmlEscape(value)}</textarea>
    </section>`;
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
    title: "System",
    basePath: "#/system",
    storageKey: "refine_system_tab",
    tabs: [
      { slug: "processes", label: "Processes" },
      { slug: "performance", label: "Performance" },
    ],
  },
  instance: {
    title: "Instance",
    basePath: "#/instance",
    storageKey: "refine_instance_tab",
    tabs: [
      { slug: "instances", label: "Instances" },
      { slug: "reporters", label: "Reporters" },
      { slug: "application", label: "Application" },
      { slug: "runtime", label: "Runtime" },
    ],
  },
  project: {
    title: "Project",
    basePath: "#/project",
    storageKey: "refine_project_tab",
    tabs: [
      { slug: "application", label: "Application" },
      { slug: "quality", label: "Quality" },
      { slug: "governance", label: "Governance" },
      { slug: "guidance", label: "Guidance" },
    ],
  },
};
const SETTINGS_TABS = SETTINGS_SURFACES.settings.tabs;
const INSTANCE_SETTINGS_TABS = SETTINGS_SURFACES.instance.tabs;
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
  return `
    <nav class="settings-tabs" id="settings-tabs">
      ${surface.tabs.map((t) => `
        <a class="settings-tab ${t.slug === activeSlug ? "active" : ""}"
           href="${surface.basePath}/${t.slug}"
           data-tab-target="${t.slug}">${htmlEscape(t.label)}</a>`).join("")}
    </nav>`;
}

function renderSettingsPane(slug, body, activeSlug) {
  return `
    <section class="settings-pane ${slug === activeSlug ? "active" : ""}"
             data-tab-pane="${slug}">
      <div class="card settings-tab-card">${body}</div>
    </section>`;
}

function bindSettingsTabHandlers() {
  $$(".settings-tab", $("#settings-tabs")).forEach((btn) => {
    btn.addEventListener("click", () => {
      setSettingsTab(btn.dataset.tabTarget);
    });
  });
}

function renderSqliteCacheSection(error = null) {
  return `
    <section class="settings-section">
      <h3>SQLite cache</h3>
      ${error ? `<p class="muted small" style="color:var(--error);margin-top:0">${htmlEscape(error.message || String(error))}</p>` : ""}
      <p class="muted small" style="margin-top:0">
        Rebuilds <code>index.sqlite</code> from canonical <code>.refine</code> JSON.
      </p>
      <div class="actions">
        <button class="danger" id="s-rebuild-cache">Rebuild SQLite cache</button>
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
    total ? `Processing ${completed} of ${total} Gaps` : "Rebuilding SQLite cache"
  );
  const detail = total
    ? `${Math.min(completed, total)} / ${total} Gap${total === 1 ? "" : "s"} processed`
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
  if (surface === SETTINGS_SURFACES.settings) {
    if (slug === "processes") {
      return renderProcessesTab(data.processes, data.s, data.diag, data.dash);
    }
    if (slug === "performance") {
      return renderSettingsPerformanceTab(data.performance, data.performanceBackend);
    }
  }
  if (surface === SETTINGS_SURFACES.instance) {
    if (slug === "instances") {
      return renderSettingsInstancesTab({
        instances: data.instances,
        instanceCounts: data.instanceCounts,
        activeInstanceId: data.activeInstanceId,
      });
    }
    if (slug === "reporters") {
      return renderSettingsReportersTab(data.reps, data.activeInstanceLabel);
    }
    if (slug === "application") {
      return renderInstanceApplicationConfigSections({
        s: data.s,
        activeInstanceLabel: data.activeInstanceLabel,
      });
    }
    if (slug === "runtime") {
      return renderInstanceRuntimeConfigSections(data.s, data.activeInstanceLabel, data.cli);
    }
  }
  if (surface === SETTINGS_SURFACES.project) {
    if (slug === "application") {
      return renderSettingsApplicationTab({
        projectApps: data.projectApps,
        currentProject: data.currentProject,
        projectRegistryEnabled: data.projectRegistryEnabled,
        appOptions: data.appOptions,
      });
    }
    if (slug === "quality") return renderSettingsQualityTab(data.quality);
    if (slug === "governance") return renderSettingsGovernanceTab(data.gov);
    if (slug === "guidance") return renderSettingsGuidanceTab(data.guidanceItems);
  }
  return `<p class="muted">Unknown settings tab.</p>`;
}

function bindSettingsTabBody(surface, slug, data) {
  if (surface === SETTINGS_SURFACES.settings) {
    if (slug === "processes") bindSettingsProcessesTab(data.s);
    else if (slug === "performance") {
      bindSettingsPerformanceTab(
        data.s, data.diag, data.reps, null, data.gov,
        data.dash, { instances: data.instances, counts: data.instanceCounts },
        { guidance: data.guidanceItems }, data.performanceBackend,
      );
    }
  } else if (surface === SETTINGS_SURFACES.instance) {
    if (slug === "instances") bindSettingsInstancesTab();
    else if (slug === "reporters") bindSettingsReportersTab();
    else if (slug === "application") bindInstanceApplicationConfigControls();
    else if (slug === "runtime") bindInstanceRuntimeConfigControls();
  } else if (surface === SETTINGS_SURFACES.project) {
    if (slug === "application") bindSettingsApplicationTab(data.currentProject);
    else if (slug === "quality") bindSettingsQualityTab();
    else if (slug === "governance") bindSettingsGovernanceTab();
    else if (slug === "guidance") bindSettingsGuidanceTab(data.guidanceItems);
  }
}

function drawSettingsSurface(surface, data) {
  const root = document.getElementById("settings-content");
  if (!root) return;
  const activeSlug = readSettingsTab(surface);
  const tabStrip = renderSettingsTabStrip(activeSlug, surface);
  const pane = (tab) => renderSettingsPane(
    tab.slug,
    renderSettingsTabBody(surface, tab.slug, data),
    activeSlug,
  );
  $("#settings-content").innerHTML = `
    ${tabStrip}
    ${surface.tabs.map(pane).join("")}
  `;

  bindSettingsTabHandlers();
  surface.tabs.forEach((tab) => bindSettingsTabBody(surface, tab.slug, data));
}
