// ---- System / Application ---------------------------------------------------

function renderProjectApplicationsSection({
  projectApps, currentProject, projectRegistryEnabled, appOptions,
}) {
  return `
    <section class="settings-section">
      <h3>Applications</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small">
        Current app: <code>${htmlEscape(state.project?.target_root || "Not attached")}</code>
      </p>
      ${projectRegistryEnabled ? "" : `
        <p class="muted small" style="color:var(--warn)">
          App switching requires the host-native Refine supervisor. Start from
          the refine source checkout with <code>./r start</code>
          before a project is attached.
        </p>`}
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Known apps",
        "project-known-apps",
        "add an existing repo or a new directory, then switch between apps here.",
      )}</label>
        <select id="s-project-select" data-testid="project-app-select" ${projectApps.length ? "" : "disabled"}>
          ${appOptions || `<option value="">No apps yet</option>`}
        </select></div>
      <div class="actions settings-section-actions">
        <button class="secondary" id="s-project-add" data-testid="project-add-app" ${projectRegistryEnabled ? "" : "disabled"}>Add app</button>
        <button class="secondary" id="s-project-template" data-testid="project-template" ${currentProject ? "" : "disabled"}>Select app template</button>
        <button id="s-project-switch" data-testid="project-switch-app" ${projectApps.length && projectRegistryEnabled ? "" : "disabled"}>Switch to selected</button>
        <button class="danger" id="s-project-remove" data-testid="project-remove-app" ${projectApps.length && projectRegistryEnabled ? "" : "disabled"}>Remove selected</button>
      </div>
    </section>`;
}

function renderSettingsApplicationTab({
  projectApps, currentProject, projectRegistryEnabled, appOptions,
}) {
  return renderProjectApplicationsSection({
    projectApps,
    currentProject,
    projectRegistryEnabled,
    appOptions,
  });
}

function renderNodeApplicationConfigSections({ s, activeNodeLabel }) {
  const rawAutoBuildMode = String(s.target_app_auto_build || "on_worktree_merge");
  const autoBuildMode = rawAutoBuildMode === "nightly" ? "daily" : rawAutoBuildMode;
  return `
    <section class="settings-section">
      <h3>Application</h3>
      <p class="scope-label muted small">Node: ${htmlEscape(activeNodeLabel)}</p>
      <div class="actions">
        <button class="secondary" id="s-application-copy-node" data-testid="target-app-copy-node">Copy from node</button>
        <button class="secondary" id="s-target-generate-ai" data-testid="target-app-generate-ai">Generate with AI</button>
      </div>
    </section>

    <section class="settings-section">
      <h3>Scope</h3>
      <p class="muted small">
        Where refine's agent work lands inside the target root. The base
        target location still owns all git plumbing — worktree create, fetch,
        merge, push.
      </p>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Agent subpath",
        "application-agent-subpath",
        "optional sub-project (relative to the repo root) used as the cwd for agent + chat subprocesses. Leave blank to use the repo root.",
      )}</label>
        <input type="text" id="s-subpath"
               placeholder="e.g. apps/web"
               value="${htmlEscape(s.agent_subpath || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Merge target branch",
        "application-merge-target",
        "branch all Gap worktrees are based on and all Merge agent work lands on. Leave blank to follow the host's currently-checked-out branch.",
      )}</label>
        <input type="text" id="s-merge-target"
               placeholder="e.g. main"
               value="${htmlEscape(s.merge_target_branch || "")}"></div>
    </section>

    <section class="settings-section">
      <h3>Target application</h3>
      <p class="muted small" style="margin-top:0">
        <strong>Generate with AI</strong> analyses the codebase, writes a
        <code>.refine/manage-app.sh</code> wrapper with timestamped logging,
        and points the commands below at it
        (<code>./.refine/manage-app.sh start|stop|build|status</code>).
        Refine runs the saved commands directly on the host. You can override
        any field — including swapping in your own commands.
      </p>
      ${(s.target_app_start_instructions || s.target_app_stop_instructions || s.target_app_health_url) ? `
        <p class="muted small" style="color:var(--warn)">
          Legacy prose target-app settings are present. Use Processes → Runner
          workers → target-app config generator to convert them into structured
          commands.
        </p>` : ""}
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "App URL",
        "application-url",
        "opened from the status indicator when the app is running.",
      )}</label>
        <input type="url" id="s-target-app-url"
               data-testid="target-app-url"
               placeholder="http://localhost:3000"
               value="${htmlEscape(s.target_app_url || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Start command",
        "application-start",
        "one-line shell command that starts the app and returns promptly.",
      )}</label>
        <input type="text" id="s-target-start-command"
               data-testid="target-app-start-command"
               placeholder="./.refine/manage-app.sh start"
               value="${htmlEscape(s.target_app_start_command || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Stop command",
        "application-stop",
        "one-line shell command that stops the app; should be idempotent when practical.",
      )}</label>
        <input type="text" id="s-target-stop-command"
               data-testid="target-app-stop-command"
               placeholder="./.refine/manage-app.sh stop"
               value="${htmlEscape(s.target_app_stop_command || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Build command",
        "application-build",
        "one-line shell command that prepares generated artifacts for review.",
      )}</label>
        <input type="text" id="s-target-build-command"
               data-testid="target-app-build-command"
               placeholder="./.refine/manage-app.sh build"
               value="${htmlEscape(s.target_app_build_command || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Automatic application build",
        "application-auto-build",
        "controls when Refine builds merged work before it becomes ready for review.",
      )}</label>
        <select id="s-target-auto-build">
          ${[
            ["never", "Never"],
            ["on_worktree_merge", "On worktree merge"],
            ["hourly", "Hourly"],
            ["daily", "Daily (time)"],
          ].map(([v, lbl]) => `<option value="${v}" ${autoBuildMode === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Daily build time",
        "application-auto-build-time",
        "UTC whole-hour time used when automatic build is Daily.",
      )}</label>
        <select id="s-target-auto-build-hour-utc"
                ${autoBuildMode === "daily" ? "" : "disabled"}>
          ${Array.from({ length: 24 }, (_, hour) => {
            const value = String(hour);
            const label = `${String(hour).padStart(2, "0")}:00 UTC`;
            return `<option value="${value}" ${String(s.target_app_auto_build_hour_utc || "0") === value ? "selected" : ""}>${label}</option>`;
          }).join("")}
        </select></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Status command",
        "application-status",
        "exit 0 only when the app is healthy or running.",
      )}</label>
        <input type="text" id="s-target-status-command"
               data-testid="target-app-status-command"
               placeholder="./.refine/manage-app.sh status"
               value="${htmlEscape(s.target_app_status_command || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Working directory",
        "application-working-directory",
        "repo-relative path, or blank for repo root.",
      )}</label>
        <input type="text" id="s-target-cwd"
               data-testid="target-app-cwd"
               placeholder="."
               value="${htmlEscape(s.target_app_cwd || "")}"></div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Environment overrides",
        "application-environment",
        "JSON object merged into the host environment.",
      )}</label>
        <textarea id="s-target-env" data-testid="target-app-env" rows="3" placeholder='{"PORT":"3000"}'>${htmlEscape(s.target_app_env_json || "{}")}</textarea></div>
      <div class="form-grid two">
        <div class="form-row"><label>${renderSettingsGuideLabel("Start timeout (s)", "application-start-timeout")}</label>
          <input type="number" id="s-target-start-timeout" data-testid="target-app-start-timeout" value="${htmlEscape(s.target_app_start_timeout_seconds || "120")}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Stop timeout (s)", "application-stop-timeout")}</label>
          <input type="number" id="s-target-stop-timeout" data-testid="target-app-stop-timeout" value="${htmlEscape(s.target_app_stop_timeout_seconds || "60")}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Build timeout (s)", "application-build-timeout")}</label>
          <input type="number" id="s-target-build-timeout" data-testid="target-app-build-timeout" value="${htmlEscape(s.target_app_build_timeout_seconds || "300")}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Status timeout (s)", "application-status-timeout")}</label>
          <input type="number" id="s-target-status-timeout" data-testid="target-app-status-timeout" value="${htmlEscape(s.target_app_status_timeout_seconds || "10")}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("Log path", "application-log-path")}</label>
          <input type="text" id="s-target-log-path" data-testid="target-app-log-path" value="${htmlEscape(s.target_app_log_path || "")}"></div>
      </div>
      <h4 style="margin:16px 0 8px">Optional checks</h4>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "HTTP check URL",
        "application-http-check-url",
        "optional; 2xx means healthy. Runs on the host.",
      )}</label>
        <input type="text" id="s-target-http-url"
               data-testid="target-app-http-url"
               placeholder="http://localhost:3000/health"
               value="${htmlEscape(s.target_app_http_check_url || s.target_app_health_url || "")}"></div>
      <div class="form-grid two">
        <div class="form-row"><label>${renderSettingsGuideLabel("TCP host", "application-tcp-host")}</label>
          <input type="text" id="s-target-tcp-host" data-testid="target-app-tcp-host" value="${htmlEscape(s.target_app_tcp_check_host || "")}"></div>
        <div class="form-row"><label>${renderSettingsGuideLabel("TCP port", "application-tcp-port")}</label>
          <input type="number" id="s-target-tcp-port" data-testid="target-app-tcp-port" value="${htmlEscape(s.target_app_tcp_check_port || "")}"></div>
      </div>
      <div class="form-row"><label>${renderSettingsGuideLabel(
        "Process check command",
        "application-process-check-command",
        "optional one-line command; exit 0 when the expected process exists.",
      )}</label>
        <input type="text" id="s-target-process-command"
               data-testid="target-app-process-command"
               value="${htmlEscape(s.target_app_process_check_command || "")}"></div>
      <div class="form-row" id="s-target-notes-row" style="display:none"><label>Generated notes</label>
        <p class="muted small" id="s-target-notes" data-testid="target-app-notes"></p></div>
    </section>`;
}

function collectSettingsApplicationPayload() {
  return {
    agent_subpath: $("#s-subpath").value,
    merge_target_branch: $("#s-merge-target").value,
    target_app_url: $("#s-target-app-url").value,
    target_app_start_command: $("#s-target-start-command").value,
    target_app_stop_command: $("#s-target-stop-command").value,
    target_app_build_command: $("#s-target-build-command").value,
    target_app_auto_build: $("#s-target-auto-build").value,
    target_app_auto_build_hour_utc: $("#s-target-auto-build-hour-utc").value,
    target_app_status_command: $("#s-target-status-command").value,
    target_app_cwd: $("#s-target-cwd").value,
    target_app_env_json: $("#s-target-env").value,
    target_app_start_timeout_seconds: $("#s-target-start-timeout").value,
    target_app_stop_timeout_seconds: $("#s-target-stop-timeout").value,
    target_app_build_timeout_seconds: $("#s-target-build-timeout").value,
    target_app_status_timeout_seconds: $("#s-target-status-timeout").value,
    target_app_log_path: $("#s-target-log-path").value,
    target_app_http_check_url: $("#s-target-http-url").value,
    target_app_tcp_check_host: $("#s-target-tcp-host").value,
    target_app_tcp_check_port: $("#s-target-tcp-port").value,
    target_app_process_check_command: $("#s-target-process-command").value,
  };
}

async function autosaveSettingsApplication(options = {}) {
  await api("PATCH", "/api/settings", collectSettingsApplicationPayload());
  _targetAppDraftDirty = false;
  refreshTargetAppStatus();
  if (options.refresh) {
    await refreshSettingsTab("target-app", { force: true });
  }
}

function applyGeneratedTargetAppConfig(cfg) {
  _targetAppDraftDirty = true;
  const set = (id, value) => {
    const el = $(id);
    if (el) el.value = value == null ? "" : String(value);
  };
  set("#s-target-start-command", cfg.start_command || "");
  set("#s-target-stop-command", cfg.stop_command || "");
  set("#s-target-build-command", cfg.build_command || "");
  set("#s-target-status-command", cfg.status_command || "");
  set("#s-target-cwd", cfg.cwd || "");
  set("#s-target-env", JSON.stringify(cfg.env || {}, null, 2));
  set("#s-target-start-timeout", cfg.start_timeout_seconds || 120);
  set("#s-target-stop-timeout", cfg.stop_timeout_seconds || 60);
  set("#s-target-build-timeout", cfg.build_timeout_seconds || 300);
  set("#s-target-status-timeout", cfg.status_timeout_seconds || 10);
  set("#s-target-log-path", cfg.log_path || "");
  set("#s-target-http-url", cfg.http_check_url || "");
  set("#s-target-tcp-host", cfg.tcp_check_host || "");
  set("#s-target-tcp-port", cfg.tcp_check_port || "");
  set("#s-target-process-command", cfg.process_check_command || "");
  const notesRow = $("#s-target-notes-row");
  const notes = $("#s-target-notes");
  if (notesRow && notes) {
    notes.textContent = cfg.notes || "";
    notesRow.style.display = cfg.notes ? "" : "none";
  }
  autosaveSettingsApplication({ refresh: true })
    .then(() => toast("Generated target-app config saved", "info"))
    .catch(async (e) => {
      _targetAppDraftDirty = false;
      await modalAlert(
        `Target-app config autosave failed: ${e?.message || "Request failed"}\n\nThe fields were restored to the last saved values.`,
        { title: "Save failed" },
      );
      await refreshSettingsTab("target-app", { force: true });
    });
}

function bindProjectApplicationsControls(currentProject, refreshTab = "runtime") {
  $("#s-project-add")?.addEventListener("click", async () => {
    await openAddAppModal();
  });
  $("#s-project-template")?.addEventListener("click", async () => {
    await openProjectTemplateSelector();
  });
  $("#s-project-switch")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const path = ($("#s-project-select")?.value || "").trim();
    if (!path || path === currentProject) return;
    const ok = await modalConfirm(
      "Switch refine to the selected app? Running agents will be stopped and the current app must be clean.",
      { title: "Switch app", okLabel: "Switch" },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Switching…", async () => {
      try {
        const result = await api("POST", "/api/project/attach", { path });
        await applyProjectAttachResult(result);
      } catch (e) {
        if (e.status === 409 && /migration required/i.test(e.message || "")) {
          if (typeof isManualMigrationError === "function" && isManualMigrationError(e)) {
            const text = typeof manualMigrationText === "function" ? manualMigrationText(e) : (e.details || e.message);
            await modalAlert(text, {
              title: "Project migration required",
              okLabel: "OK",
            });
            return;
          }
          const migrate = await modalConfirm(
            "This app uses an older Refine schema. Migrate .refine state and open it?",
            { title: "Migrate app", okLabel: "Migrate and open" },
          );
          if (!migrate) return;
          const closeMigration = showProjectMigrationDialog();
          let result;
          try {
            result = await api("POST", "/api/project/attach", { path, migrate: true });
          } finally {
            closeMigration();
          }
          await applyProjectAttachResult(result);
          return;
        }
        toast(e.details || e.message, "error");
      }
    });
  });
  $("#s-project-remove")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const path = ($("#s-project-select")?.value || "").trim();
    if (!path) return;
    const ok = await modalConfirm(
      "Remove this app from the known-apps list? This does not delete files.",
      { title: "Remove app", okLabel: "Remove", danger: true },
    );
    if (!ok) return;
    await withButtonBusy(btn, "Removing…", async () => {
      try {
        const result = await api("DELETE", "/api/apps", { path });
        if (result.auto_attached) {
          await applyProjectAttachResult(result, { toast: false });
          toast("App removed; loaded next app", "info");
          await maybeOpenProjectTemplateModal(result);
          return;
        }
        state.project = result.attached === false
          ? { ...result, apps: result.apps || [] }
          : { ...(state.project || {}), ...result, apps: result.apps || [] };
        if (typeof resetGuideState === "function") resetGuideState({ redraw: false });
        updateActiveNodeLabel();
        toast("App removed", "info");
        if (result.attached === false) {
          enterNoProjectMode(result, { openGuidePanel: true });
          refreshProjectApplicationsSectionOnly(result);
          if (["dashboard", "gaps", "changes", "logs"].includes(state.currentRoute || "")) {
            navigate();
          }
          return;
        }
        await refreshSettingsTab(refreshTab, { force: true });
      } catch (e) { toast(e.details || e.message, "error"); }
    });
  });
}

function refreshProjectApplicationsSectionOnly(project) {
  const projectApps = project?.apps || [];
  const currentProject = project?.target_root || "";
  const appOptions = projectApps.map((app) => `
    <option value="${htmlEscape(app.path)}" ${app.path === currentProject ? "selected" : ""}>
      ${htmlEscape(app.name || app.path)}
    </option>`).join("");
  updateSettingsTabContent(
    "application",
    renderSettingsApplicationTab({
      projectApps,
      currentProject,
      projectRegistryEnabled: project?.registry_enabled !== false,
      appOptions,
    }),
    () => bindSettingsApplicationTab(currentProject),
  );
}

function bindNodeApplicationConfigControls() {
  bindCommand("#s-application-copy-node", "settings.application.copy_node");
  bindCommand("#s-target-generate-ai", "target_app.generate");
  if (typeof syncTargetAppGenerateButtonState === "function") {
    syncTargetAppGenerateButtonState();
  }
  const root = document.querySelector('[data-tab-pane="target-app"]');
  const autoBuild = $("#s-target-auto-build");
  const autoBuildHour = $("#s-target-auto-build-hour-utc");
  if (autoBuild && autoBuildHour) {
    autoBuild.addEventListener("change", () => {
      autoBuildHour.disabled = autoBuild.value !== "daily";
    });
  }
  bindSettingsAutosave(
    root,
    "#s-subpath, #s-merge-target, #s-target-app-url, #s-target-start-command, #s-target-stop-command, #s-target-build-command, #s-target-auto-build, #s-target-auto-build-hour-utc, #s-target-status-command, #s-target-cwd, #s-target-env, #s-target-start-timeout, #s-target-stop-timeout, #s-target-build-timeout, #s-target-status-timeout, #s-target-log-path, #s-target-http-url, #s-target-tcp-host, #s-target-tcp-port, #s-target-process-command",
    autosaveSettingsApplication,
  );
}

function bindSettingsApplicationTab(currentProject) {
  bindProjectApplicationsControls(currentProject, "application");
}
