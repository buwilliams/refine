// ---- System -----------------------------------------------------------------

let _targetAppDraftDirty = false;

async function renderSettings() {
  renderBanners([]);
  // First-paint scaffold only; subsequent refreshes route through
  // `refreshSettings` so SSE / post-save reloads don't flash `Loading…`.
  if (!document.getElementById("settings-content")) {
    $("#main").innerHTML = `<h2>System</h2><div id="settings-content"><p class="muted">Loading…</p></div>`;
  }
  await refreshSettings();
}

async function refreshSettings(options = {}) {
  if (state.currentRoute !== "settings") return;
  if (
    _targetAppDraftDirty &&
    !options.force &&
    document.querySelector('[data-tab-pane="application"].active')
  ) {
    return;
  }
  try {
    const [
      s, diag, reps, feats, project, gov, quality, dash, instances, guidance,
      performance, processes,
    ] = await Promise.all([
      api("GET", "/api/settings"),
      api("GET", "/api/diagnostics"),
      api("GET", "/api/reporters"),
      api("GET", "/api/features"),
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
    // Keep the cached matrix fresh so gates elsewhere react too.
    state.features = feats;
    state.project = project;
    updateActiveInstanceLabel();
    drawSettings(
      s.settings || {}, diag, reps.reporters || [], feats,
      gov || {}, quality || {}, dash || {}, instances || {}, guidance || {},
      performance || {}, processes || {},
    );
  } catch (e) {
    const root = document.getElementById("settings-content");
    if (root) drawRuntimeRecovery(e);
  }
}

async function refreshActiveSettingsTab(options = {}) {
  if (state.currentRoute !== "settings") return;
  const slug = readSettingsTab();
  await refreshSettingsTab(slug, options);
}

async function refreshSettingsTab(slug, options = {}) {
  const activeSlug = normalizeSettingsTab(slug) || readSettingsTab();
  if (!document.querySelector(`[data-tab-pane="${activeSlug}"] .settings-tab-card`)) {
    await refreshSettings(options);
    return;
  }
  if (
    activeSlug === "application" &&
    _targetAppDraftDirty &&
    !options.force
  ) {
    return;
  }
  try {
    if (activeSlug === "processes") {
      const [s, diag, dash, processes] = await Promise.all([
        api("GET", "/api/settings"),
        api("GET", "/api/diagnostics"),
        api("GET", "/api/dashboard"),
        api("GET", "/api/processes"),
      ]);
      updateSettingsTabContent(
        "processes",
        renderProcessesTab(processes || {}, s.settings || {}, diag || {}, dash || {}),
        () => bindSettingsProcessesTab(s.settings || {}),
      );
    } else if (activeSlug === "instances") {
      const [project, instances] = await Promise.all([
        api("GET", "/api/project/status"),
        api("GET", "/api/instances"),
      ]);
      state.project = project;
      updateActiveInstanceLabel();
      const list = instances.instances || [];
      updateSettingsTabContent(
        "instances",
        renderSettingsInstancesTab(
          list,
          instances.counts || {},
          instances.active_instance_id || project.active_instance_id || "",
          list.filter((inst) => !inst.archived),
        ),
        bindSettingsInstancesTab,
      );
    } else if (activeSlug === "performance") {
      const [performance, diag] = await Promise.all([
        api("GET", typeof performanceApiPath === "function"
          ? performanceApiPath()
          : "/api/performance"),
        api("GET", "/api/diagnostics"),
      ]);
      const backend = (performance || {}).backend || (diag || {}).backend || {};
      updateSettingsTabContent(
        "performance",
        renderSettingsPerformanceTab(performance || {}, backend),
        () => bindSettingsPerformanceTab(null, diag || {}, [], null, {}, {}, {}, {}, backend),
      );
    } else if (activeSlug === "reporters") {
      const [project, reps] = await Promise.all([
        api("GET", "/api/project/status"),
        api("GET", "/api/reporters"),
      ]);
      state.project = project;
      state.reporters = reps.reporters || [];
      updateActiveInstanceLabel();
      updateSettingsTabContent(
        "reporters",
        renderSettingsReportersTab(state.reporters, settingsActiveInstanceLabel()),
        bindSettingsReportersTab,
      );
    } else if (activeSlug === "guidance") {
      const guidance = await api("GET", "/api/guidance");
      const items = guidance.guidance || [];
      updateSettingsTabContent(
        "guidance",
        renderSettingsGuidanceTab(items),
        () => bindSettingsGuidanceTab(items),
      );
    } else if (activeSlug === "governance") {
      const gov = await api("GET", "/api/governance");
      updateSettingsTabContent(
        "governance",
        renderSettingsGovernanceTab(gov || {}),
        bindSettingsGovernanceTab,
      );
    } else if (activeSlug === "quality") {
      const quality = await api("GET", "/api/quality");
      updateSettingsTabContent(
        "quality",
        renderSettingsQualityTab(quality || {}),
        bindSettingsQualityTab,
      );
    } else if (activeSlug === "application") {
      const [s, project] = await Promise.all([
        api("GET", "/api/settings"),
        api("GET", "/api/project/status"),
      ]);
      state.project = project;
      updateActiveInstanceLabel();
      const projectApps = project.apps || [];
      const currentProject = project.client_repo || "";
      const appOptions = projectApps.map((app) => `
        <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
          ${htmlEscape(app.name || app.path)}
        </option>`).join("");
      updateSettingsTabContent(
        "application",
        renderSettingsApplicationTab({
          s: s.settings || {},
          projectApps,
          currentProject,
          projectRegistryEnabled: project.registry_enabled !== false,
          appOptions,
          activeInstanceLabel: settingsActiveInstanceLabel(project),
        }),
        () => bindSettingsApplicationTab(currentProject),
      );
    } else if (activeSlug === "runtime") {
      const [s, feats, project] = await Promise.all([
        api("GET", "/api/settings"),
        api("GET", "/api/features"),
        api("GET", "/api/project/status"),
      ]);
      state.features = feats;
      state.project = project;
      updateActiveInstanceLabel();
      applyFeatureGates();
      const settings = s.settings || {};
      const cli = (settings.agent_cli || "claude").toLowerCase();
      updateSettingsTabContent(
        "runtime",
        renderSettingsRuntimeTab(settings, feats, settingsActiveInstanceLabel(project), cli),
        bindSettingsRuntimeTab,
      );
    } else {
      await refreshSettings(options);
    }
  } catch (e) {
    await showActionError(e);
  }
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

const SETTINGS_TAB_STORAGE_KEY = "refine_settings_tab";
const SETTINGS_TABS = [
  { slug: "processes",    label: "Processes" },
  { slug: "instances",    label: "Instances" },
  { slug: "performance",  label: "Performance" },
  { slug: "reporters",    label: "Reporters" },
  { slug: "guidance",     label: "Guidance" },
  { slug: "governance",   label: "Governance" },
  { slug: "quality",      label: "Quality" },
  { slug: "application",  label: "Application" },
  { slug: "runtime",      label: "Runtime" },
];

function normalizeSettingsTab(slug) {
  if (slug === "system" || slug === "project") return "application";
  if (slug === "agents") return "guidance";
  return SETTINGS_TABS.some((t) => t.slug === slug) ? slug : null;
}

function activeSettingsTabFromRoute() {
  const parsed = typeof parseHash === "function" ? parseHash() : {};
  return parsed.route === "settings" ? normalizeSettingsTab(parsed.tab) : null;
}

function readSettingsTab(tabs = SETTINGS_TABS) {
  const routed = activeSettingsTabFromRoute();
  if (routed) {
    localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, routed);
    return routed;
  }
  const parsed = typeof parseHash === "function" ? parseHash() : {};
  if (parsed.route === "settings" && !parsed.tab) {
    const first = tabs[0]?.slug;
    if (first) localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, first);
    return first;
  }
  const stored = localStorage.getItem(SETTINGS_TAB_STORAGE_KEY);
  const normalizedStored = normalizeSettingsTab(stored);
  if (normalizedStored && tabs.some((t) => t.slug === normalizedStored)) {
    if (normalizedStored !== stored) {
      localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, normalizedStored);
    }
    return normalizedStored;
  }
  return tabs[0]?.slug;
}

function setSettingsTab(slug) {
  const normalized = normalizeSettingsTab(slug);
  if (!normalized) return;
  localStorage.setItem(SETTINGS_TAB_STORAGE_KEY, normalized);
  // Toggle classes immediately; hashchange handles normal linked tab
  // navigation, and this keeps repeated clicks on the current hash responsive.
  $$("[data-tab-pane]").forEach((pane) => {
    pane.classList.toggle("active", pane.dataset.tabPane === normalized);
  });
  $$(".settings-tab").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.tabTarget === normalized);
  });
}


function renderSettingsTabStrip(activeSlug) {
  return `
    <nav class="settings-tabs" id="settings-tabs">
      ${SETTINGS_TABS.map((t) => `
        <a class="settings-tab ${t.slug === activeSlug ? "active" : ""}"
           href="#/system/${t.slug}"
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
  const activeSlug = "runtime";
  $("#settings-content").innerHTML = `
    ${renderSettingsTabStrip(activeSlug)}
    ${renderSettingsPane("runtime", renderSqliteCacheSection(error), activeSlug)}
  `;
  bindSettingsTabHandlers();
  bindRebuildCacheHandler();
}

function bindRebuildCacheHandler() {
  $("#s-rebuild-cache")?.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Rebuild the SQLite cache from canonical .refine JSON? If the existing database is corrupted, Refine will replace it and SQLite-only runtime history may be lost.",
      { title: "Rebuild SQLite cache", okLabel: "Rebuild" },
    );
    if (!ok) return;
    await withButtonBusy($("#s-rebuild-cache"), "Rebuilding…", async () => {
      try {
        let result = await api("POST", "/api/cache/rebuild", { background: true });
        if (result.job) {
          drawSqliteCacheProgress(result.job.progress || {});
          result = await waitForBackgroundJob(result.job, {
            onProgress: drawSqliteCacheProgress,
          });
          if (result.http_status && result.http_status >= 400) {
            const raw = result.error || {};
            const err = new Error(raw.message || "SQLite cache rebuild failed");
            err.details = raw.details;
            err.code = raw.code;
            throw err;
          }
        }
        const verb = result.mode === "recreated" ? "recreated" : "rebuilt";
        toast(`SQLite cache ${verb}; ${result.gaps || 0} Gap${result.gaps === 1 ? "" : "s"} indexed`, "info");
        await refreshSettings({ force: true });
      } catch (e) { await showActionError(e, "SQLite cache rebuild failed"); }
    });
  });
}


function drawSettings(
  s, diag, reps, feats, gov = {}, quality = {}, dash = {}, instanceData = {},
  guidanceData = {}, performanceData = {}, processData = {},
) {
  const cli = (s.agent_cli || "claude").toLowerCase();
  const projectApps = state.project?.apps || [];
  const currentProject = state.project?.client_repo || "";
  const projectRegistryEnabled = state.project?.registry_enabled !== false;
  const instances = instanceData.instances || state.project?.instances || [];
  const activeInstanceId = instanceData.active_instance_id || state.project?.active_instance_id || "";
  const activeInstance = instances.find((i) => i.id === activeInstanceId) || null;
  const activeInstanceLabel = activeInstance?.display_name || activeInstanceId || "Default";
  const transferTargetInstances = instances.filter((inst) => !inst.archived);
  const instanceCounts = instanceData.counts || {};
  const guidanceItems = guidanceData.guidance || [];
  const performance = performanceData || {};
  const performanceBackend = performance.backend || diag.backend || {};
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  // Tab definitions. Order here drives the tab strip; `slug` is the
  // localStorage key, route segment, and DOM hook.
  const tabs = SETTINGS_TABS;
  const activeSlug = readSettingsTab(tabs);
  const tabStrip = renderSettingsTabStrip(activeSlug);
  const pane = (slug, body) => renderSettingsPane(slug, body, activeSlug);
  $("#settings-content").innerHTML = `
    ${tabStrip}
    ${pane("application", renderSettingsApplicationTab({
      s, projectApps, currentProject, projectRegistryEnabled,
      appOptions, activeInstanceLabel,
    }))}
    ${pane("processes", renderProcessesTab(processData, s, diag, dash))}
    ${pane("guidance", renderSettingsGuidanceTab(guidanceItems))}
    ${pane("runtime", renderSettingsRuntimeTab(s, feats, activeInstanceLabel, cli))}
    ${pane("performance", renderSettingsPerformanceTab(performance, performanceBackend))}
    ${pane("governance", renderSettingsGovernanceTab(gov))}
    ${pane("quality", renderSettingsQualityTab(quality))}
    ${pane("instances", renderSettingsInstancesTab(
      instances, instanceCounts, activeInstanceId, transferTargetInstances,
    ))}
    ${pane("reporters", renderSettingsReportersTab(reps, activeInstanceLabel))}
  `;

  bindSettingsTabHandlers();
  bindSettingsProcessesTab(s);
  bindSettingsGuidanceTab(guidanceItems);
  bindSettingsRuntimeTab();
  bindSettingsPerformanceTab(s, diag, reps, feats, gov, dash, instanceData, guidanceData);
  bindSettingsGovernanceTab();
  bindSettingsQualityTab();
  bindSettingsApplicationTab(currentProject);
  bindSettingsInstancesTab();
  bindSettingsReportersTab();
}
