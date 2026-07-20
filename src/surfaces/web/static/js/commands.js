// ---- Application commands ---------------------------------------------------

function commandFirstToken(input) {
  return String(input || "").trim().split(/\s+/, 1)[0].toLowerCase();
}

function commandTailFor(input, names) {
  const q = String(input || "").trim();
  const lower = q.toLowerCase();
  for (const name of names) {
    const n = String(name).toLowerCase();
    if (lower === n) return "";
    if (lower.startsWith(n + " ")) return q.slice(String(name).length).trim();
  }
  return "";
}

function normalizeCommandStatus(value) {
  const raw = String(value || "").trim().toLowerCase();
  const compact = raw.replace(/[_\s]+/g, "-");
  return {
    back: "backlog",
    backlog: "backlog",
    todo: "todo",
    "to-do": "todo",
    review: "review",
    done: "done",
    failed: "failed",
    cancelled: "cancelled",
    canceled: "cancelled",
    "build": "build",
  }[compact] || compact;
}

function currentNodeBulkFilter(status) {
  return { status, node: "current" };
}

async function promptCommandValue(label, current = "") {
  const value = await modalPrompt(label, current, {
    title: "Command parameter",
    okLabel: "Continue",
  });
  return value == null ? null : String(value).trim();
}

async function runBulkStatusCommand({ source, dest, button = null }) {
  source = normalizeCommandStatus(source || "");
  dest = normalizeCommandStatus(dest || "");
  if (!source) source = normalizeCommandStatus(await promptCommandValue("Source status", "backlog"));
  if (!source) return;
  if (!dest) dest = normalizeCommandStatus(await promptCommandValue("Destination status", "todo"));
  if (!dest) return;
  const valid = (BULK_STATUS_OPTIONS || []).map((item) => item.value);
  if (!valid.includes(dest)) {
    toast(`Unknown destination status: ${dest}`, "error");
    return;
  }
  const ok = await modalConfirm(
    `Move all current-node Goals with status ${source} to ${dest}?`,
    {
      title: "Bulk move Goals",
      okLabel: "Move Goals",
      danger: true,
    },
  );
  if (!ok) return;
  await withButtonBusy(button, "Moving...", async () => {
    let r = await api("POST", "/api/goals/bulk", {
      filter: currentNodeBulkFilter(source),
      update: { status: dest },
    });
    r = await resolveBackgroundOperationResponse(
      r,
      "Bulk status update is running in the background",
    );
    toast(`Updated ${r.updated} goal${r.updated === 1 ? "" : "s"}`, "info");
    if (state.currentRoute === "goals") await renderGoalsList();
    if (state.currentRoute === "dashboard") await refreshDashboard();
  });
}

async function runFailedBackCommand({ button = null } = {}) {
  const ok = await modalConfirm(
    "Move all current-node failed Goals back to their last safe workflow state?",
    {
      title: "Bulk retry failed Goals",
      okLabel: "Move failed Goals",
      danger: true,
    },
  );
  if (!ok) return;
  await withButtonBusy(button, "Moving...", async () => {
    let r = await api("POST", "/api/goals/bulk", {
      filter: currentNodeBulkFilter("failed"),
      update: { status: "__last_workflow_state" },
    });
    r = await resolveBackgroundOperationResponse(
      r,
      "Bulk retry is running in the background",
    );
    toast(`Updated ${r.updated} goal${r.updated === 1 ? "" : "s"}`, "info");
    if (state.currentRoute === "goals") await renderGoalsList();
    if (state.currentRoute === "dashboard") await refreshDashboard();
  });
}

function githubIssueUrl({ title, description }) {
  const url = new URL("https://github.com/buwilliams/refine/issues/new");
  if (title) url.searchParams.set("title", title);
  if (description) {
    url.searchParams.set("body", description);
  }
  return url.toString();
}

function openRefineIssueRequestModal() {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal refine-issue-modal" role="dialog" aria-modal="true"
         aria-labelledby="refine-issue-title"
         data-testid="refine-issue-modal">
      <div class="modal-title" id="refine-issue-title">Request refine feature/bugfix</div>
      <div class="modal-body">
        <p class="muted small" style="margin-top:0">
          This opens GitHub in a new tab with your title and description pre-filled.
          You can review and submit the issue there.
        </p>
        <form id="refine-issue-form">
          <div class="form-row">
            <label>Title</label>
            <input type="text" id="refine-issue-input-title"
                   data-testid="refine-issue-title"
                   placeholder="Short summary">
          </div>
          <div class="form-row">
            <label>Description</label>
            <textarea id="refine-issue-input-description"
                      data-testid="refine-issue-description"
                      placeholder="What should change? Include what happened, what you expected, and any relevant context."></textarea>
          </div>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="refine-issue-cancel">Cancel</button>
        <button data-ok data-testid="refine-issue-submit">Open GitHub</button>
      </div>
    </div>`;
  document.body.appendChild(root);

  let closed = false;
  function close() {
    if (closed) return;
    closed = true;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  }
  function submit() {
    const title = root.querySelector("#refine-issue-input-title")?.value.trim() || "";
    const description = root.querySelector("#refine-issue-input-description")?.value.trim() || "";
    if (!title && !description) {
      toast("Provide a title or description first.", "error");
      root.querySelector("#refine-issue-input-title")?.focus();
      return;
    }
    const opened = window.open(
      githubIssueUrl({ title, description }),
      "_blank",
      "noopener,noreferrer",
    );
    if (!opened) {
      toast("GitHub did not open. Allow popups for this site and try again.", "error");
      return;
    }
    close();
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Enter") {
      if (e.target && e.target.tagName === "TEXTAREA") return;
      e.preventDefault();
      submit();
    }
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  root.querySelector("[data-cancel]")?.addEventListener("click", close);
  root.querySelector("[data-ok]")?.addEventListener("click", submit);
  root.querySelector("#refine-issue-form")?.addEventListener("submit", (e) => {
    e.preventDefault();
    submit();
  });
  root.querySelector("#refine-issue-input-title")?.focus();
}

function navigateCommand(hash) {
  location.hash = hash;
}

function registerNavigationCommand(id, title, hash, keywords = []) {
  registerCommand({
    id,
    title,
    group: "Navigate",
    aliases: [title.toLowerCase()],
    keywords,
    run: () => navigateCommand(hash),
  });
}

registerNavigationCommand("nav.dashboard", "Dashboard", "#/", ["home"]);
registerNavigationCommand("nav.features", "Features", "#/features", ["feature", "planning"]);
registerNavigationCommand("nav.goals", "Goals", "#/goals", ["issues", "work"]);
registerNavigationCommand("nav.changes", "Changes", "#/changes", ["merges"]);
registerNavigationCommand("nav.logs", "Logs", "#/logs", ["activity"]);
for (const [surfaceKey, surface] of Object.entries(SETTINGS_SURFACES || {})) {
  const label = surface.title || surfaceKey;
  for (const tab of surface.tabs || []) {
    registerNavigationCommand(
      `nav.${surfaceKey}.${tab.slug}`,
      `${label}: ${tab.label}`,
      `${surface.basePath}/${tab.slug}`,
      ["settings", label.toLowerCase(), tab.slug],
    );
  }
}

registerCommand({
  id: "goal.new",
  title: "New Goal",
  group: "Create",
  aliases: ["new", "new-goal", "goal"],
  keywords: ["create", "submit"],
  run: () => {
    closeTopbarMenus();
    openNewGoalModal();
  },
});

registerCommand({
  id: "goal.import",
  title: "Import",
  group: "Create",
  aliases: ["import", "import-goals", "import-feature"],
  keywords: ["csv", "ai import", "feature"],
  run: () => {
    closeTopbarMenus();
    openImportModal();
  },
});

registerCommand({
  id: "refine.issue.request",
  title: "Request refine feature/bugfix",
  group: "Support",
  aliases: ["issue", "bug", "bugfix", "feature-request", "request-feature"],
  keywords: ["github", "report", "support", "feedback"],
  run: () => openRefineIssueRequestModal(),
});

registerCommand({
  id: "plan.open",
  title: "Plan: make a plan",
  group: "AI",
  aliases: ["plan", "make-plan"],
  keywords: ["chat", "draft goals"],
  parse: (input) => {
    const prompt = commandTailFor(input, ["plan", "make-plan"]);
    return prompt ? { prompt } : {};
  },
  run: async ({ prompt } = {}) => {
    await openPlanChatDock({ initialPrompt: prompt || "" });
  },
});

registerCommand({
  id: "plan.draft",
  title: "Draft Feature from plan",
  group: "AI",
  aliases: ["draft-plan", "draft-goals"],
  visible: () => !!chatState?.tabs?.plan,
  enabled: () => planHasAgentResponse(chatState.tabs.plan),
  disabledMessage: "Wait for the agent to respond before drafting a Feature.",
  run: () => draftGoalsFromPlan(),
});

registerCommand({
  id: "toolbar.toggle",
  title: "Toggle Toolbar",
  group: "Toolbar",
  aliases: ["toolbar", "toggle-toolbar", "chat", "toggle-chat"],
  run: () => toggleToolbar(),
});

registerCommand({
  id: "toolbar.fullscreen",
  title: "Maximize Toolbar",
  group: "Toolbar",
  aliases: ["fullscreen-toolbar", "maximize-toolbar", "fullscreen-chat", "maximize-chat"],
  run: () => toggleToolbarFullscreen(),
});

registerCommand({
  id: "files.open",
  title: "Files: open file browser",
  group: "Toolbar",
  aliases: ["files", "open-files", "file-browser"],
  keywords: ["source tree file browser"],
  parse: (input) => {
    const path = commandTailFor(input, ["files", "open-files", "file-browser"]);
    return path ? { path } : {};
  },
  run: ({ path } = {}) => openFilesToolbar({ path: path || "" }),
});

registerCommand({
  id: "files.search",
  title: "Files: search for file",
  group: "Toolbar",
  aliases: ["search-files", "find-file", "file-search"],
  keywords: ["source tree file browser"],
  parse: (input) => {
    const search = commandTailFor(input, ["search-files", "find-file", "file-search"]);
    return search ? { search } : {};
  },
  run: ({ search } = {}) => openFilesToolbar({ search: search || "", focusSearch: true }),
});

registerCommand({
  id: "goals.clear_filters",
  title: "Goals: clear filters",
  group: "Goals",
  aliases: ["clear-goals"],
  run: () => {
    if (state.currentRoute !== "goals") {
      location.hash = "#/goals";
      return;
    }
    history.replaceState(null, "", "#/goals");
    renderGoalsList();
  },
});

registerCommand({
  id: "goals.select_page",
  title: "Goals: select page",
  group: "Goals",
  aliases: ["select-page"],
  visible: () => state.currentRoute === "goals",
  run: () => selectCurrentGoalsPage(),
});

registerCommand({
  id: "features.select_page",
  title: "Features: select page",
  group: "Features",
  aliases: ["features-select-page"],
  visible: () => state.currentRoute === "features",
  run: () => selectCurrentFeaturesPage(),
});

registerCommand({
  id: "features.bulk.reporter",
  title: "Bulk: set selected Feature reporter",
  group: "Features",
  aliases: ["features-bulk-reporter"],
  visible: () => state.currentRoute === "features",
  run: () => openFeatureBulkModal("reporter"),
});

registerCommand({
  id: "features.bulk.assignee",
  title: "Bulk: set selected Feature assignee",
  group: "Features",
  aliases: ["features-bulk-assignee"],
  visible: () => state.currentRoute === "features",
  run: () => openFeatureBulkModal("assignee"),
});

registerCommand({
  id: "features.bulk.transfer_node",
  title: "Bulk: transfer selected Features to node",
  group: "Features",
  aliases: ["features-bulk-node"],
  visible: () => state.currentRoute === "features",
  run: () => openFeatureBulkTransferNodeModal(),
});

registerCommand({
  id: "features.bulk.delete",
  title: "Bulk: delete selected Features",
  group: "Features",
  aliases: ["features-bulk-delete"],
  visible: () => state.currentRoute === "features",
  run: () => confirmFeatureBulkDelete(),
});

registerCommand({
  id: "goals.bulk.status",
  title: "Bulk: set selected Goal status",
  group: "Goals",
  aliases: ["bulk-status"],
  visible: () => state.currentRoute === "goals",
  run: ({ button } = {}) => openBulkModal("status", { button }),
});

registerCommand({
  id: "goals.bulk.priority",
  title: "Bulk: set selected Goal priority",
  group: "Goals",
  aliases: ["bulk-priority"],
  visible: () => state.currentRoute === "goals",
  run: ({ button } = {}) => openBulkModal("priority", { button }),
});

registerCommand({
  id: "goals.bulk.reporter",
  title: "Bulk: set selected Goal reporter",
  group: "Goals",
  aliases: ["bulk-reporter"],
  visible: () => state.currentRoute === "goals",
  run: ({ button } = {}) => openBulkModal("reporter", { button }),
});

registerCommand({
  id: "goals.bulk.assignee",
  title: "Bulk: set selected Goal assignee",
  group: "Goals",
  aliases: ["bulk-assignee"],
  visible: () => state.currentRoute === "goals",
  run: ({ button } = {}) => openBulkModal("assignee", { button }),
});

registerCommand({
  id: "goals.bulk.feature",
  title: "Bulk: assign selected Goals to Feature",
  group: "Goals",
  aliases: ["bulk-feature"],
  visible: () => state.currentRoute === "goals",
  run: ({ button } = {}) => openBulkAssignFeatureModal({ button }),
});

registerCommand({
  id: "goals.bulk.transfer_node",
  title: "Bulk: transfer selected Goals to node",
  group: "Goals",
  aliases: ["bulk-node"],
  visible: () => state.currentRoute === "goals",
  run: () => openBulkTransferNodeModal(),
});

registerCommand({
  id: "goals.bulk.delete",
  title: "Bulk: delete selected Goals",
  group: "Goals",
  aliases: ["bulk-delete"],
  visible: () => state.currentRoute === "goals",
  run: () => confirmBulkDelete(),
});

registerCommand({
  id: "goals.bulk.move",
  title: "Bulk: move all Goals by status",
  group: "Goals",
  aliases: ["bulk_move", "bulk-move", "move-goals"],
  keywords: ["bulk_move source dest backlog todo"],
  parse: (input) => {
    const tail = commandTailFor(input, ["bulk_move", "bulk-move", "move-goals"]);
    const parts = tail.split(/\s+/).filter(Boolean);
    return {
      source: parts[0] || "",
      dest: parts[1] || "",
    };
  },
  run: runBulkStatusCommand,
});

registerCommand({
  id: "goals.bulk.failed_back",
  title: "Bulk: move failed back one workflow step",
  group: "Goals",
  aliases: ["bulk_failed_back", "failed-back"],
  keywords: ["retry failed last workflow"],
  run: runFailedBackCommand,
});

registerCommand({
  id: "changes.clear_filters",
  title: "Changes: clear filters",
  group: "Changes",
  aliases: ["clear-changes"],
  run: () => {
    if (state.currentRoute !== "changes") {
      location.hash = "#/changes";
      return;
    }
    history.replaceState(null, "", "#/changes");
    renderChanges();
  },
});

registerCommand({
  id: "logs.clear_filters",
  title: "Logs: clear filters",
  group: "Logs",
  aliases: ["clear-logs"],
  run: () => {
    location.hash = "#/logs";
  },
});

registerCommand({
  id: "system.agents.pause_toggle",
  title: "Pause or unpause workflow",
  group: "System",
  aliases: ["pause-workflow", "unpause-workflow", "resume-workflow", "pause-agents", "unpause-agents", "resume-agents"],
  run: async ({ button, settings } = {}) => {
    const settingsPayload = settings || (await api("GET", "/api/settings"));
    const workflowPaused = !!settingsPayload.runtime?.agents_paused ||
      !!settingsPayload.runtime?.background_processes_stopped ||
      settingsPayload.settings?.paused === "1";
    await withButtonBusy(button, workflowPaused ? "Unpausing..." : "Pausing...", async () => {
      await api("POST", "/api/workflow/pause", { paused: !workflowPaused });
      if (state.currentRoute === "node") await refreshProcessesSettingsTab({ force: true });
      if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
      if (workflowPaused) scheduleProcessesTabRefreshes();
    });
  },
});

registerCommand({
  id: "system.worktree.hard_reset",
  title: "Hard reset target worktree",
  group: "System",
  aliases: ["hard-reset", "reset-worktree"],
  danger: true,
  confirm: () => modalConfirm(
    "Hard reset the target worktree to its upstream branch and delete untracked files? This discards local target-app changes and cannot be undone.",
    {
      title: "Hard reset worktree",
      okLabel: "Hard reset",
      danger: true,
      cancelLabel: "Cancel",
    },
  ),
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Resetting...", async () => {
      const r = await api("POST", "/api/runner-workers/merger/hard-reset-worktree");
      toast(r.message || "Target worktree reset", "info");
      if (state.currentRoute === "node") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

for (const action of ["start", "stop", "build"]) {
  const busyLabel = {
    start: "Starting...",
    stop: "Stopping...",
    build: "Queueing...",
  }[action];
  registerCommand({
    id: `target_app.${action}`,
    title: `Target app: ${action}`,
    group: "Application",
    aliases: [`app-${action}`, `target-${action}`],
    run: async ({ button } = {}) => {
      await withButtonBusy(button, busyLabel, async () => {
        await runTargetAppAction(action);
        if (state.currentRoute === "node") await refreshProcessesSettingsTab({ force: true });
      });
    },
  });
}

registerCommand({
  id: "target_app.health",
  title: "Target app: check status",
  group: "Application",
  aliases: ["health-check", "check-app"],
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Probing...", async () => {
      const r = await api("POST", "/api/target-app/health");
      const ok = "last_check_ok" in r ? r.last_check_ok : r.last_health_ok;
      toast(ok ? "Status check OK" : (r.probe_message || "Unhealthy"), ok ? "info" : "error");
      applyTargetAppSnapshot(r);
      drawTargetAppStatusBlock(r);
      if (state.currentRoute === "node") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

async function ensureTargetAppSettingsPane() {
  if (state.currentRoute !== "node") {
    location.hash = "#/node/target-app";
  }
  for (let i = 0; i < 40; i += 1) {
    if (
      state.currentRoute === "node" &&
      document.querySelector('[data-tab-pane="target-app"]')
    ) {
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  setSettingsTab("target-app");
  await refreshSettingsTab("target-app", { force: true });
}

const TARGET_APP_GENERATE_OPERATION_KEY = "refine_target_app_generate_operation";
let targetAppGeneratePollOperationId = "";
let targetAppGeneratePollPromise = null;

function readTargetAppGenerateOperation() {
  try {
    const raw = localStorage.getItem(TARGET_APP_GENERATE_OPERATION_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    const operationId = String(parsed?.operationId || "").trim();
    if (!operationId) return null;
    const startedAt = Number(parsed?.startedAt || 0);
    if (startedAt && Date.now() - startedAt > 12 * 60 * 60 * 1000) {
      localStorage.removeItem(TARGET_APP_GENERATE_OPERATION_KEY);
      return null;
    }
    return { operationId, startedAt };
  } catch {
    localStorage.removeItem(TARGET_APP_GENERATE_OPERATION_KEY);
    return null;
  }
}

function writeTargetAppGenerateOperation(operationId) {
  if (!operationId) {
    localStorage.removeItem(TARGET_APP_GENERATE_OPERATION_KEY);
    return;
  }
  localStorage.setItem(TARGET_APP_GENERATE_OPERATION_KEY, JSON.stringify({
    operationId,
    startedAt: Date.now(),
  }));
}

function setTargetAppGenerateButtonLoading(active) {
  const button = document.querySelector("#s-target-generate-ai");
  if (!button) return;
  button.disabled = !!active;
  button.textContent = active ? "Generating..." : "Generate with AI";
}

async function handleTargetAppGenerateResult(result) {
  if (result?.http_status && result.http_status >= 400) {
    const raw = result.error || {};
    const err = new Error(raw.message || "Target-app config generation failed");
    err.details = raw.details;
    err.code = raw.code;
    throw err;
  }
  if (result?.ok && result.config) {
    await ensureTargetAppSettingsPane();
    applyGeneratedTargetAppConfig(result.config);
    return result;
  }
  throw new Error("Generation produced no configuration");
}

async function waitForTargetAppGenerateOperation(operationId) {
  if (targetAppGeneratePollOperationId === operationId && targetAppGeneratePollPromise) {
    return await targetAppGeneratePollPromise;
  }
  targetAppGeneratePollOperationId = operationId;
  targetAppGeneratePollPromise = waitForBackgroundOperation(operationId, {
    onProgress: (progress) => {
      const message = String(progress?.message || "").trim();
      if (message) recordUiNotice(message, { kind: "info", source: "background-operation" });
    },
  });
  try {
    return await targetAppGeneratePollPromise;
  } finally {
    targetAppGeneratePollOperationId = "";
    targetAppGeneratePollPromise = null;
  }
}

async function resumeTargetAppGenerateOperation() {
  const active = readTargetAppGenerateOperation();
  if (!active) {
    setTargetAppGenerateButtonLoading(false);
    return null;
  }
  setTargetAppGenerateButtonLoading(true);
  try {
    const result = await waitForTargetAppGenerateOperation(active.operationId);
    writeTargetAppGenerateOperation("");
    setTargetAppGenerateButtonLoading(false);
    return await handleTargetAppGenerateResult(result);
  } catch (error) {
    writeTargetAppGenerateOperation("");
    setTargetAppGenerateButtonLoading(false);
    throw error;
  }
}

function syncTargetAppGenerateButtonState() {
  if (!readTargetAppGenerateOperation()) {
    setTargetAppGenerateButtonLoading(false);
    return;
  }
  resumeTargetAppGenerateOperation().catch((error) => {
    showActionError(error, "Target-app config generation failed");
  });
}

async function runTargetAppGenerateOperation() {
  const response = await api("POST", "/api/target-app/generate-instructions", {
    kind: "all",
    background: true,
  });
  if (!response?.operation?.id) {
    return await handleTargetAppGenerateResult(response);
  }
  writeTargetAppGenerateOperation(response.operation.id);
  setTargetAppGenerateButtonLoading(true);
  recordUiNotice("Target-app config generation queued", {
    kind: "queued",
    source: "background-operation",
  });
  const result = await waitForTargetAppGenerateOperation(response.operation.id);
  writeTargetAppGenerateOperation("");
  setTargetAppGenerateButtonLoading(false);
  return await handleTargetAppGenerateResult(result);
}

registerCommand({
  id: "target_app.generate",
  title: "Generate target-app config with AI",
  group: "AI",
  aliases: ["generate-app-config", "target-generate"],
  confirm: () => modalConfirm(
    "Ask the agent to analyse the codebase and generate start/stop/build instructions plus test/status checks? This can take a minute or two and overwrites the saved target-app fields.",
    { title: "Generate target-app config", okLabel: "Generate" },
  ),
  run: async ({ button } = {}) => {
    await ensureTargetAppSettingsPane();
    await withButtonBusy(button, "Generating...", async () => {
      await runTargetAppGenerateOperation();
    });
  },
});

registerCommand({
  id: "runtime.recheck_auth",
  title: "Runtime: re-check auth",
  group: "System",
  aliases: ["recheck-auth", "auth"],
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Checking...", async () => {
      const r = await api("POST", "/api/settings/recheck-auth");
      toast(r.ok ? "Auth OK" : `Auth failed: ${r.message || "(no message)"}`, r.ok ? "info" : "error");
      if (state.currentRoute === "node" && typeof readSettingsTab === "function" && readSettingsTab() === "processes") {
        await refreshSettingsTab("processes", { force: true });
      } else if (state.currentRoute === "node") {
        await refreshSettingsTab("runtime", { force: true });
      }
    });
  },
});

registerCommand({
  id: "system.cache.rebuild",
  title: "Rebuild projection cache",
  group: "System",
  aliases: ["rebuild-cache", "sqlite-cache"],
  errorTitle: "Projection cache rebuild failed",
  confirm: () => modalConfirm(
    "Rebuild the runtime projection cache from canonical .refine JSON?",
    { title: "Rebuild projection cache", okLabel: "Rebuild" },
  ),
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Rebuilding...", async () => {
      let result = await api("POST", "/api/cache/rebuild", { background: true });
      if (result.operation) {
        drawSqliteCacheProgress(result.operation.progress || {});
        result = await waitForBackgroundOperation(result.operation, {
          onProgress: drawSqliteCacheProgress,
        });
        if (result.http_status && result.http_status >= 400) {
          const raw = result.error || {};
          const err = new Error(raw.message || "Projection cache rebuild failed");
          err.details = raw.details;
          err.code = raw.code;
          throw err;
        }
      }
      const verb = result.mode === "recreated" ? "recreated" : "rebuilt";
      toast(`Projection cache ${verb}; ${result.goals || 0} Goal${result.goals === 1 ? "" : "s"} indexed`, "info");
      if (["settings", "node", "project"].includes(state.currentRoute || "")) await refreshSettings({ force: true });
    });
  },
});

registerCommand({
  id: "system.logs.cleanup",
  title: "Clean up old activity logs",
  group: "System",
  aliases: ["cleanup-logs", "log-cleanup"],
  parse: (input) => {
    const tail = commandTailFor(input, ["cleanup-logs", "log-cleanup"]);
    const days = parseInt(tail || "", 10);
    return Number.isFinite(days) ? { days } : {};
  },
  run: async ({ button, days } = {}) => {
    if (!Number.isFinite(Number(days))) {
      const raw = await promptCommandValue("Delete entries older than how many days?", "7");
      if (raw == null) return;
      days = parseInt(raw, 10);
    }
    days = Math.max(0, parseInt(days, 10) || 0);
    const human = days === 0
      ? "Delete ALL activity log entries? This cannot be undone."
      : `Delete activity log entries older than ${days} day${days === 1 ? "" : "s"}? This cannot be undone.`;
    const ok = await modalConfirm(human, {
      title: "Clean up old logs",
      okLabel: days === 0 ? "Delete all" : "Delete",
      danger: true,
    });
    if (!ok) return;
    await withButtonBusy(button, "Cleaning...", async () => {
      const r = await api("POST", "/api/activity/cleanup", { days });
      toast(`Deleted ${r.deleted} log entr${r.deleted === 1 ? "y" : "ies"}.`, "info");
      if (state.currentRoute === "node") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

registerCommand({
  id: "settings.application.copy_node",
  title: "Application: copy settings from node",
  group: "System",
  aliases: ["copy-application-settings"],
  run: ({ button } = {}) => copySettingsFromNode("application", {
    title: "Copy application settings",
    refreshTab: "target-app",
    button,
  }),
});

registerCommand({
  id: "settings.runtime.copy_node",
  title: "Runtime: copy settings from node",
  group: "System",
  aliases: ["copy-runtime-settings"],
  run: ({ button } = {}) => copySettingsFromNode("runtime", {
    title: "Copy runtime settings",
    refreshTab: "runtime",
    button,
  }),
});
