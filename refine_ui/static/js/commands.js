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
    "awaiting-rebuild": "awaiting-rebuild",
  }[compact] || compact;
}

function currentInstanceBulkFilter(status) {
  return { status, instance: "current" };
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
    `Move all current-instance Gaps with status ${source} to ${dest}?`,
    {
      title: "Bulk move Gaps",
      okLabel: "Move Gaps",
      danger: true,
    },
  );
  if (!ok) return;
  await withButtonBusy(button, "Moving...", async () => {
    let r = await api("POST", "/api/gaps/bulk", {
      filter: currentInstanceBulkFilter(source),
      update: { status: dest },
    });
    r = await resolveBackgroundJobResponse(
      r,
      "Bulk status update is running in the background",
    );
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    if (state.currentRoute === "gaps") await renderGapsList();
    if (state.currentRoute === "dashboard") await refreshDashboard();
  });
}

async function runFailedBackCommand({ button = null } = {}) {
  const ok = await modalConfirm(
    "Move all current-instance failed Gaps back to their last safe workflow state?",
    {
      title: "Bulk retry failed Gaps",
      okLabel: "Move failed Gaps",
      danger: true,
    },
  );
  if (!ok) return;
  await withButtonBusy(button, "Moving...", async () => {
    let r = await api("POST", "/api/gaps/bulk", {
      filter: currentInstanceBulkFilter("failed"),
      update: { status: "__last_workflow_state" },
    });
    r = await resolveBackgroundJobResponse(
      r,
      "Bulk retry is running in the background",
    );
    toast(`Updated ${r.updated} gap${r.updated === 1 ? "" : "s"}`, "info");
    if (state.currentRoute === "gaps") await renderGapsList();
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
         aria-labelledby="refine-issue-title">
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
                   placeholder="Short summary">
          </div>
          <div class="form-row">
            <label>Description</label>
            <textarea id="refine-issue-input-description"
                      placeholder="What should change? Include what happened, what you expected, and any relevant context."></textarea>
          </div>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Open GitHub</button>
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
registerNavigationCommand("nav.gaps", "Gaps", "#/gaps", ["issues", "work"]);
registerNavigationCommand("nav.changes", "Changes", "#/changes", ["merges"]);
registerNavigationCommand("nav.logs", "Logs", "#/logs", ["activity"]);
for (const tab of SETTINGS_TABS || []) {
  registerNavigationCommand(
    `nav.system.${tab.slug}`,
    `System: ${tab.label}`,
    `#/system/${tab.slug}`,
    ["settings", "system", tab.slug],
  );
}

registerCommand({
  id: "gap.new",
  title: "New Gap",
  group: "Create",
  aliases: ["new", "new-gap", "gap"],
  keywords: ["create", "submit"],
  run: () => {
    closeTopbarMenus();
    openNewGapModal();
  },
});

registerCommand({
  id: "gap.import",
  title: "Import gaps",
  group: "Create",
  aliases: ["import", "import-gaps"],
  keywords: ["csv", "ai import"],
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
  keywords: ["chat", "draft gaps"],
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
  title: "Draft Gaps from plan",
  group: "AI",
  aliases: ["draft-plan", "draft-gaps"],
  visible: () => !!chatState?.tabs?.plan,
  enabled: () => planHasAgentResponse(chatState.tabs.plan),
  disabledMessage: "Wait for the agent to respond before drafting Gaps.",
  run: () => draftGapsFromPlan(),
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
  id: "gaps.clear_filters",
  title: "Gaps: clear filters",
  group: "Gaps",
  aliases: ["clear-gaps"],
  run: () => {
    if (state.currentRoute !== "gaps") {
      location.hash = "#/gaps";
      return;
    }
    history.replaceState(null, "", "#/gaps");
    renderGapsList();
  },
});

registerCommand({
  id: "gaps.select_page",
  title: "Gaps: select page",
  group: "Gaps",
  aliases: ["select-page"],
  visible: () => state.currentRoute === "gaps",
  run: () => selectCurrentGapsPage(),
});

registerCommand({
  id: "gaps.bulk.status",
  title: "Bulk: set selected Gap status",
  group: "Gaps",
  aliases: ["bulk-status"],
  visible: () => state.currentRoute === "gaps",
  run: ({ button } = {}) => openBulkModal("status", { button }),
});

registerCommand({
  id: "gaps.bulk.priority",
  title: "Bulk: set selected Gap priority",
  group: "Gaps",
  aliases: ["bulk-priority"],
  visible: () => state.currentRoute === "gaps",
  run: ({ button } = {}) => openBulkModal("priority", { button }),
});

registerCommand({
  id: "gaps.bulk.reporter",
  title: "Bulk: set selected Gap reporter",
  group: "Gaps",
  aliases: ["bulk-reporter"],
  visible: () => state.currentRoute === "gaps",
  run: ({ button } = {}) => openBulkModal("reporter", { button }),
});

registerCommand({
  id: "gaps.bulk.transfer_instance",
  title: "Bulk: transfer selected Gaps to instance",
  group: "Gaps",
  aliases: ["bulk-instance"],
  visible: () => state.currentRoute === "gaps",
  run: () => openBulkTransferInstanceModal(),
});

registerCommand({
  id: "gaps.bulk.delete",
  title: "Bulk: delete selected Gaps",
  group: "Gaps",
  aliases: ["bulk-delete"],
  visible: () => state.currentRoute === "gaps",
  run: () => confirmBulkDelete(),
});

registerCommand({
  id: "gaps.bulk.move",
  title: "Bulk: move all Gaps by status",
  group: "Gaps",
  aliases: ["bulk_move", "bulk-move", "move-gaps"],
  keywords: ["bulk_move source dest backlog todo"],
  parse: (input) => {
    const tail = commandTailFor(input, ["bulk_move", "bulk-move", "move-gaps"]);
    const parts = tail.split(/\s+/).filter(Boolean);
    return {
      source: parts[0] || "",
      dest: parts[1] || "",
    };
  },
  run: runBulkStatusCommand,
});

registerCommand({
  id: "gaps.bulk.failed_back",
  title: "Bulk: move failed back one workflow step",
  group: "Gaps",
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
  title: "Start or stop background processes",
  group: "System",
  aliases: ["pause-agents", "resume-agents"],
  run: async ({ button, settings } = {}) => {
    const s = settings || (await api("GET", "/api/settings")).settings || {};
    const paused = s.paused === "1";
    await withButtonBusy(button, paused ? "Starting..." : "Stopping...", async () => {
      await api("POST", "/api/processes/background", { stopped: !paused });
      if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
      if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
      if (paused) scheduleProcessesTabRefreshes();
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
      if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

for (const action of ["start", "stop", "rebuild"]) {
  const busyLabel = {
    start: "Starting...",
    stop: "Stopping...",
    rebuild: "Rebuilding...",
  }[action];
  registerCommand({
    id: `target_app.${action}`,
    title: `Target app: ${action}`,
    group: "Application",
    aliases: [`app-${action}`, `target-${action}`],
    run: async ({ button } = {}) => {
      await withButtonBusy(button, busyLabel, async () => {
        await runTargetAppAction(action);
        if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
      });
    },
  });
}

registerCommand({
  id: "target_app.sync",
  title: "Target app: sync project",
  group: "Application",
  aliases: ["sync-project"],
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Syncing...", async () => {
      await syncProjectUpdates();
      await refreshReporters({ selectFallback: true });
      if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

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
      drawTargetAppStatusBlock(r);
      if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

registerCommand({
  id: "target_app.generate",
  title: "Generate target-app config with AI",
  group: "AI",
  aliases: ["generate-app-config", "target-generate"],
  confirm: () => modalConfirm(
    "Ask the agent to analyse the codebase and draft target-app configuration? This can take a minute or two and overwrites the saved target-app fields.",
    { title: "Generate target-app config", okLabel: "Generate" },
  ),
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Generating...", async () => {
      const r = await api("POST", "/api/target-app/generate-instructions", { kind: "all" });
      if (r.ok && r.config) {
        setSettingsTab("application");
        applyGeneratedTargetAppConfig(r.config);
        toast("Generated target-app config saved", "info");
      } else {
        toast("Generation produced no configuration", "error");
      }
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
      if (state.currentRoute === "settings") await refreshSettingsTab("runtime", { force: true });
    });
  },
});

registerCommand({
  id: "quality.regression.new",
  title: "Quality: create regression",
  group: "Quality",
  aliases: ["regression_new", "new-regression", "create-regression"],
  keywords: ["playwright screenshot qa"],
  parse: (input) => ({
    prompt: commandTailFor(input, ["regression_new", "new-regression", "create-regression"]),
  }),
  run: async ({ button, prompt } = {}) => {
    if (state.currentRoute !== "settings") {
      location.hash = "#/system/quality";
    } else {
      setSettingsTab("quality");
    }
    return openRegressionCreateModal(prompt || "", button);
  },
});

registerCommand({
  id: "quality.regression.run",
  title: "Quality: run regressions on current checkout",
  group: "Quality",
  aliases: ["run-regressions", "regression-run"],
  keywords: ["playwright screenshot qa"],
  run: ({ button } = {}) => runQualityRegressions(button),
});

registerCommand({
  id: "system.cache.rebuild",
  title: "Rebuild SQLite cache",
  group: "System",
  aliases: ["rebuild-cache", "sqlite-cache"],
  errorTitle: "SQLite cache rebuild failed",
  confirm: () => modalConfirm(
    "Rebuild the SQLite cache from canonical .refine JSON? If the existing database is corrupted, Refine will replace it and SQLite-only runtime history may be lost.",
    { title: "Rebuild SQLite cache", okLabel: "Rebuild" },
  ),
  run: async ({ button } = {}) => {
    await withButtonBusy(button, "Rebuilding...", async () => {
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
      if (state.currentRoute === "settings") await refreshSettings({ force: true });
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
      if (state.currentRoute === "settings") await refreshProcessesSettingsTab({ force: true });
    });
  },
});

registerCommand({
  id: "settings.application.copy_instance",
  title: "Application: copy settings from instance",
  group: "System",
  aliases: ["copy-application-settings"],
  run: ({ button } = {}) => copySettingsFromInstance("application", {
    title: "Copy application settings",
    refreshTab: "application",
    button,
  }),
});

registerCommand({
  id: "settings.runtime.copy_instance",
  title: "Runtime: copy settings from instance",
  group: "System",
  aliases: ["copy-runtime-settings"],
  run: ({ button } = {}) => copySettingsFromInstance("runtime", {
    title: "Copy runtime settings",
    refreshTab: "runtime",
    button,
  }),
});
