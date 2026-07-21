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

function targetAppAutoBuildLabel(value) {
  return ({
    never: "Never",
    on_worktree_merge: "On worktree merge",
    hourly: "Hourly",
    daily: "Daily (time)",
  })[value] || value || "none";
}

function renderNodeApplicationConfigSections({ s, activeNodeLabel }) {
  const rawAutoBuildMode = String(s.target_app_auto_build || "on_worktree_merge");
  const autoBuildMode = rawAutoBuildMode === "nightly" ? "daily" : rawAutoBuildMode;
  const autoBuildHour = String(s.target_app_auto_build_hour_utc || "0");
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
      ${renderSettingsEditableField({
        id: "s-subpath",
        label: "Agent subpath",
        guideItemId: "application-agent-subpath",
        description: "optional sub-project (relative to the repo root) used as the cwd for agent + chat subprocesses. Leave blank to use the repo root.",
        valueLabel: s.agent_subpath || "",
        control: `<input type="text" id="s-subpath"
                         placeholder="e.g. apps/web"
                         value="${htmlEscape(s.agent_subpath || "")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-git-remote",
        label: "Git remote",
        guideItemId: "application-git-remote",
        description: "remote used for Refine state synchronization and Goal branch publication.",
        valueLabel: s.git_remote || "origin",
        control: `<input type="text" id="s-git-remote"
                         placeholder="origin"
                         value="${htmlEscape(s.git_remote || "origin")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-merge-target",
        label: "Integration branch",
        guideItemId: "application-merge-target",
        description: "branch all Goal worktrees are based on and approved implementations land on. Leave blank to follow the attached project's current branch.",
        valueLabel: s.merge_target_branch || "",
        control: `<input type="text" id="s-merge-target"
                         placeholder="e.g. main"
                         value="${htmlEscape(s.merge_target_branch || "")}">`,
      })}
    </section>

    <section class="settings-section">
      <h3>Target application</h3>
      <p class="muted small" style="margin-top:0">
        <strong>Generate with AI</strong> analyses the codebase and writes
        agent instructions for starting, stopping, and building the target app.
        Refine asks the configured agent to perform those lifecycle actions so
        setup, dependency, and recovery work can be handled in context.
      </p>
      ${(s.target_app_start_command || s.target_app_stop_command || s.target_app_build_command || s.target_app_health_url) ? `
        <p class="muted small" style="color:var(--warn)">
          Legacy command target-app settings are present. Use Processes → Runner
          workers → target-app config generator to convert them into structured
          agent instructions.
        </p>` : ""}
      ${renderSettingsEditableField({
        id: "s-target-app-url",
        label: "App URL",
        guideItemId: "application-url",
        description: "opened from the status indicator when the app is running.",
        valueLabel: s.target_app_url || "",
        control: `<input type="url" id="s-target-app-url"
                         data-testid="target-app-url"
                         placeholder="http://localhost:3000"
                         value="${htmlEscape(s.target_app_url || "")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-start-instructions",
        label: "Start instructions",
        guideItemId: "application-start",
        description: "agent guidance for starting the app and verifying it is usable.",
        valueLabel: s.target_app_start_instructions || "",
        control: `<textarea id="s-target-start-instructions"
                            data-testid="target-app-start-instructions"
                            rows="3"
                            placeholder="Start the app, repair setup issues if needed, leave it running, and verify the configured URL.">${htmlEscape(s.target_app_start_instructions || "")}</textarea>`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-stop-instructions",
        label: "Stop instructions",
        guideItemId: "application-stop",
        description: "agent guidance for stopping the app and confirming it is down.",
        valueLabel: s.target_app_stop_instructions || "",
        control: `<textarea id="s-target-stop-instructions"
                            data-testid="target-app-stop-instructions"
                            rows="3"
                            placeholder="Stop any target-app processes for this repo and confirm the local URL or port is no longer reachable.">${htmlEscape(s.target_app_stop_instructions || "")}</textarea>`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-build-instructions",
        label: "Build instructions",
        guideItemId: "application-build",
        description: "agent guidance for rebuilding the app and resolving expected setup issues.",
        valueLabel: s.target_app_build_instructions || "",
        control: `<textarea id="s-target-build-instructions"
                            data-testid="target-app-build-instructions"
                            rows="4"
                            placeholder="Build the app, install or repair project-local dependencies when safe, rerun after fixes, and report blockers with evidence.">${htmlEscape(s.target_app_build_instructions || "")}</textarea>`,
      })}
      ${renderTargetAppTestCommandsField(s, {
        guideItemId: "application-test",
        description: "CLI commands Refine runs for workflow QA.",
      })}
      ${renderSettingsEditableField({
        id: "s-target-auto-build",
        label: "Automatic application build",
        guideItemId: "application-auto-build",
        description: "controls when Refine builds isolated candidate work before review.",
        valueLabel: targetAppAutoBuildLabel(autoBuildMode),
        control: `<select id="s-target-auto-build">
          ${[
            ["never", "Never"],
            ["on_worktree_merge", "When candidate is ready"],
            ["hourly", "Hourly"],
            ["daily", "Daily (time)"],
          ].map(([v, lbl]) => `<option value="${v}" ${autoBuildMode === v ? "selected" : ""}>${lbl}</option>`).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-auto-build-hour-utc",
        label: "Daily build time",
        guideItemId: "application-auto-build-time",
        description: "UTC whole-hour time used when automatic build is Daily.",
        valueLabel: `${String(autoBuildHour).padStart(2, "0")}:00 UTC`,
        control: `<select id="s-target-auto-build-hour-utc"
                          ${autoBuildMode === "daily" ? "" : "disabled"}>
          ${Array.from({ length: 24 }, (_, hour) => {
            const value = String(hour);
            const label = `${String(hour).padStart(2, "0")}:00 UTC`;
            return `<option value="${value}" ${autoBuildHour === value ? "selected" : ""}>${label}</option>`;
          }).join("")}
        </select>`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-status-command",
        label: "Status command",
        guideItemId: "application-status",
        description: "exit 0 only when the app is healthy or running.",
        valueLabel: s.target_app_status_command || "",
        control: `<input type="text" id="s-target-status-command"
                         data-testid="target-app-status-command"
                         placeholder="curl -fsS http://127.0.0.1:3000/ >/dev/null"
                         value="${htmlEscape(s.target_app_status_command || "")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-cwd",
        label: "Working directory",
        guideItemId: "application-working-directory",
        description: "repo-relative path, or blank for repo root.",
        valueLabel: s.target_app_cwd || "",
        control: `<input type="text" id="s-target-cwd"
                         data-testid="target-app-cwd"
                         placeholder="."
                         value="${htmlEscape(s.target_app_cwd || "")}">`,
      })}
      ${renderSettingsEditableField({
        id: "s-target-env",
        label: "Environment overrides",
        guideItemId: "application-environment",
        description: "JSON object merged into the host environment.",
        valueLabel: s.target_app_env_json || "{}",
        control: `<textarea id="s-target-env" data-testid="target-app-env" rows="3" placeholder='{"PORT":"3000"}'>${htmlEscape(s.target_app_env_json || "{}")}</textarea>`,
      })}
      <div class="form-grid two">
        ${renderSettingsEditableField({
          id: "s-target-start-timeout",
          label: "Start timeout (s)",
          guideItemId: "application-start-timeout",
          valueLabel: s.target_app_start_timeout_seconds || "120",
          control: `<input type="number" id="s-target-start-timeout" data-testid="target-app-start-timeout" value="${htmlEscape(s.target_app_start_timeout_seconds || "120")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-stop-timeout",
          label: "Stop timeout (s)",
          guideItemId: "application-stop-timeout",
          valueLabel: s.target_app_stop_timeout_seconds || "60",
          control: `<input type="number" id="s-target-stop-timeout" data-testid="target-app-stop-timeout" value="${htmlEscape(s.target_app_stop_timeout_seconds || "60")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-build-timeout",
          label: "Build timeout (s)",
          guideItemId: "application-build-timeout",
          valueLabel: s.target_app_build_timeout_seconds || "300",
          control: `<input type="number" id="s-target-build-timeout" data-testid="target-app-build-timeout" value="${htmlEscape(s.target_app_build_timeout_seconds || "300")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-test-timeout",
          label: "Test timeout (s)",
          guideItemId: "application-test-timeout",
          valueLabel: s.target_app_test_timeout_seconds || "600",
          control: `<input type="number" id="s-target-test-timeout" data-testid="target-app-test-timeout" value="${htmlEscape(s.target_app_test_timeout_seconds || "600")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-status-timeout",
          label: "Status timeout (s)",
          guideItemId: "application-status-timeout",
          valueLabel: s.target_app_status_timeout_seconds || "10",
          control: `<input type="number" id="s-target-status-timeout" data-testid="target-app-status-timeout" value="${htmlEscape(s.target_app_status_timeout_seconds || "10")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-log-path",
          label: "Log path",
          guideItemId: "application-log-path",
          valueLabel: s.target_app_log_path || "",
          control: `<input type="text" id="s-target-log-path" data-testid="target-app-log-path" value="${htmlEscape(s.target_app_log_path || "")}">`,
        })}
      </div>
      <h4 style="margin:16px 0 8px">Optional checks</h4>
      ${renderSettingsEditableField({
        id: "s-target-http-url",
        label: "HTTP check URL",
        guideItemId: "application-http-check-url",
        description: "optional; 2xx means healthy. Runs on the host.",
        valueLabel: s.target_app_http_check_url || s.target_app_health_url || "",
        control: `<input type="text" id="s-target-http-url"
                         data-testid="target-app-http-url"
                         placeholder="http://localhost:3000/health"
                         value="${htmlEscape(s.target_app_http_check_url || s.target_app_health_url || "")}">`,
      })}
      <div class="form-grid two">
        ${renderSettingsEditableField({
          id: "s-target-tcp-host",
          label: "TCP host",
          guideItemId: "application-tcp-host",
          valueLabel: s.target_app_tcp_check_host || "",
          control: `<input type="text" id="s-target-tcp-host" data-testid="target-app-tcp-host" value="${htmlEscape(s.target_app_tcp_check_host || "")}">`,
        })}
        ${renderSettingsEditableField({
          id: "s-target-tcp-port",
          label: "TCP port",
          guideItemId: "application-tcp-port",
          valueLabel: s.target_app_tcp_check_port || "",
          control: `<input type="number" id="s-target-tcp-port" data-testid="target-app-tcp-port" value="${htmlEscape(s.target_app_tcp_check_port || "")}">`,
        })}
      </div>
      ${renderSettingsEditableField({
        id: "s-target-process-command",
        label: "Process check command",
        guideItemId: "application-process-check-command",
        description: "optional one-line command; exit 0 when the expected process exists.",
        valueLabel: s.target_app_process_check_command || "",
        control: `<input type="text" id="s-target-process-command"
                         data-testid="target-app-process-command"
                         value="${htmlEscape(s.target_app_process_check_command || "")}">`,
      })}
      <div class="form-row" id="s-target-notes-row" style="display:none"><label>Generated notes</label>
        <p class="muted small" id="s-target-notes" data-testid="target-app-notes"></p></div>
    </section>`;
}

function collectSettingsApplicationPayload() {
  return {
    agent_subpath: $("#s-subpath").value,
    git_remote: $("#s-git-remote").value,
    merge_target_branch: $("#s-merge-target").value,
    target_app_url: $("#s-target-app-url").value,
    target_app_start_instructions: $("#s-target-start-instructions").value,
    target_app_stop_instructions: $("#s-target-stop-instructions").value,
    target_app_build_instructions: $("#s-target-build-instructions").value,
    target_app_start_command: "",
    target_app_stop_command: "",
    target_app_build_command: "",
    target_app_test_commands: $("#s-target-test-commands").value,
    target_app_auto_build: $("#s-target-auto-build").value,
    target_app_auto_build_hour_utc: $("#s-target-auto-build-hour-utc").value,
    target_app_status_command: $("#s-target-status-command").value,
    target_app_cwd: $("#s-target-cwd").value,
    target_app_env_json: $("#s-target-env").value,
    target_app_start_timeout_seconds: $("#s-target-start-timeout").value,
    target_app_stop_timeout_seconds: $("#s-target-stop-timeout").value,
    target_app_build_timeout_seconds: $("#s-target-build-timeout").value,
    target_app_test_timeout_seconds: $("#s-target-test-timeout").value,
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
  set("#s-target-start-instructions", cfg.start_instructions || cfg.start_command || "");
  set("#s-target-stop-instructions", cfg.stop_instructions || cfg.stop_command || "");
  set("#s-target-build-instructions", cfg.build_instructions || cfg.build_command || "");
  set("#s-target-test-commands", targetAppTestCommandsValue(
    cfg.test_command ? [{ command: cfg.test_command, enabled: true }] : [],
  ));
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
          if (["dashboard", "goals", "changes", "logs"].includes(state.currentRoute || "")) {
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
      syncSettingsEditableDisabled(autoBuildHour);
    });
  }
  bindTargetAppTestCommandList(root);
  bindSettingsAutosave(
    root,
    "#s-subpath, #s-merge-target, #s-target-app-url, #s-target-start-instructions, #s-target-stop-instructions, #s-target-build-instructions, #s-target-test-commands, #s-target-auto-build, #s-target-auto-build-hour-utc, #s-target-status-command, #s-target-cwd, #s-target-env, #s-target-start-timeout, #s-target-stop-timeout, #s-target-build-timeout, #s-target-test-timeout, #s-target-status-timeout, #s-target-log-path, #s-target-http-url, #s-target-tcp-host, #s-target-tcp-port, #s-target-process-command",
    autosaveSettingsApplication,
    { event: "settings-editable-commit" },
  );
  bindSettingsEditableFields(root);
}

function bindSettingsApplicationTab(currentProject) {
  bindProjectApplicationsControls(currentProject, "application");
}
