// refine — vanilla JS single-page app. No build step, no framework.

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

const state = {
  reporters: [],
  lastReporter: localStorage.getItem("refine_last_reporter") || "",
  project: null,
  dashboard: null,
  needsAttentionBanners: [],
  currentRoute: null,
  currentGap: null,
  // The hash that's "underneath" the Gap detail modal. Updated whenever
  // navigate() runs for a route other than gaps_detail. Used to restore
  // the URL when the modal is dismissed so the page the user came from
  // is what they land back on.
  underlayHash: "#/",
};

const WORKFLOW_STATUSES = [
  "backlog",
  "todo",
  "in-progress",
  "qa",
  "ready-merge",
  "awaiting-rebuild",
  "review",
  "done",
  "failed",
  "cancelled",
];
const POST_REBUILD_WORKFLOW_STATUSES = [
  "backlog",
  "todo",
  "in-progress",
  "ready-merge",
  "awaiting-rebuild",
  "qa",
  "review",
  "done",
  "failed",
  "cancelled",
];
const STATUS_FILTER_OPTIONS = ["", ...WORKFLOW_STATUSES];
const WORKFLOW_STATUS_LABELS = {
  "backlog": "Backlog",
  "todo": "To do",
  "in-progress": "In progress",
  "qa": "QA",
  "ready-merge": "Ready to merge",
  "awaiting-rebuild": "Awaiting rebuild",
  "review": "Review",
  "done": "Done",
  "failed": "Failed",
  "cancelled": "Cancelled",
};

function workflowStatusLabel(status) {
  return WORKFLOW_STATUS_LABELS[status] || status || "";
}

function workflowStatuses() {
  return state.dashboard?.quality_timing === "post_rebuild"
    ? POST_REBUILD_WORKFLOW_STATUSES
    : WORKFLOW_STATUSES;
}

function updateActiveNodeLabel() {
  const el = document.getElementById("active-node-label");
  if (!el) return;
  const project = state.project || {};
  const active = project.active_node || null;
  const activeId = project.active_node_id || "";
  const label = active?.display_name || active?.name || activeId || "none";
  el.textContent = project.attached === false ? "none" : label;
  el.title = el.textContent;
}

function updateNavReporterContext() {
  const el = document.getElementById("nav-context-reporter-summary");
  if (!el) return;
  el.textContent = state.lastReporter || "No reporter";
  el.title = el.textContent;
}

function updateNavAppContextLabel(label) {
  const el = document.getElementById("nav-context-app-summary");
  if (!el) return;
  el.textContent = label || "Application";
  el.title = el.textContent;
}

function hasAttachedProject() {
  return state.project?.attached === true;
}

function clearProjectScopedUiState() {
  state.reporters = [];
  state.dashboard = null;
  state.currentGap = null;
  if (typeof gapsExcludedIds !== "undefined") gapsExcludedIds.clear();
  if (typeof gapsIncludedIds !== "undefined") gapsIncludedIds.clear();
  setLastReporter("");
  populateAllReporterDropdowns();
  updateActiveNodeLabel();
  updateNavAppContextLabel("No app");
  if (typeof applyNoTargetAppSnapshot === "function") applyNoTargetAppSnapshot();
  if (typeof resetChatForProjectSwitch === "function") resetChatForProjectSwitch();
}

function renderNoProjectEmptyState(title = "Refine") {
  renderBanners([]);
  $("#main").innerHTML = `
    <h2>${htmlEscape(title)}</h2>
    <div class="empty-state">
      <div class="empty-state-title">No app configured.</div>
      <p class="muted">Open the Guide to configure Refine and attach an app.</p>
      <button type="button" class="secondary" id="empty-open-guide">Open Guide</button>
    </div>`;
  $("#empty-open-guide")?.addEventListener("click", () => {
    if (typeof openGuide === "function") {
      openGuide({
        context: "no-app",
        categoryId: "get-started",
        itemId: "quickstart-add-app",
        openTarget: false,
      });
    }
  });
}

function renderNoProjectIfDetached(title) {
  if (hasAttachedProject()) return false;
  renderNoProjectEmptyState(title);
  return true;
}

function renderNoProjectIfApiDetached(data, title) {
  if (data?.attached !== false) return false;
  enterNoProjectMode({ ...(state.project || {}), attached: false });
  renderNoProjectEmptyState(title);
  return true;
}

function enterNoProjectMode(project = null, { openGuidePanel = false } = {}) {
  if (project) state.project = project;
  clearProjectScopedUiState();
  if (sseSource) {
    sseSource.close();
    sseSource = null;
  }
  if (openGuidePanel && typeof openGuide === "function") {
    openGuide({
      context: "no-app",
      categoryId: "get-started",
      itemId: "quickstart-add-app",
      openTarget: false,
    });
  }
}

function refreshCurrentSettingsSurface(options = {}) {
  if (!["settings", "node", "project"].includes(state.currentRoute || "")) return undefined;
  if (typeof refreshActiveSettingsTab === "function") {
    return refreshActiveSettingsTab(options);
  }
  if (typeof refreshSettings === "function") {
    return refreshSettings(options);
  }
}

// ---- API helpers ------------------------------------------------------------

async function api(method, path, body, options = {}) {
  const opts = { method, headers: {} };
  if (options.signal) opts.signal = options.signal;
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  let data = null;
  try { data = await res.json(); } catch {}
  if (!res.ok) {
    let msg = data?.error?.message || res.statusText || "Request failed";
    const details = data?.error?.details;
    const code = data?.error?.code;
    if (code === "background_job_active" && details) {
      msg = `${msg} Active operation: ${details}.`;
    }
    const err = new Error(msg);
    err.status = res.status;
    err.details = details;
    err.code = code;
    err.error = data?.error || null;
    err.__uiLogged = true;
    state.lastApiErrorLog = { message: msg, at: Date.now() };
    recordUiError(msg, {
      source: "api",
      path,
      status: res.status,
      code,
      details,
    });
    throw err;
  }
  return data;
}

async function waitForBackgroundJob(jobOrId, {
  intervalMs = 750,
  timeoutMs = 10 * 60 * 1000,
  onProgress = null,
} = {}) {
  const jobId = typeof jobOrId === "string" ? jobOrId : jobOrId?.id;
  if (!jobId) throw new Error("Background job id missing");
  const started = Date.now();
  let lastProgress = "";
  while (true) {
    const snap = await api("GET", `/api/jobs/${jobId}`);
    const job = snap.job || {};
    const progressKey = JSON.stringify(job.progress || {});
    if (onProgress && progressKey !== lastProgress) {
      lastProgress = progressKey;
      onProgress(job.progress || {}, job);
    }
    if (job.status === "complete") return job.result || {};
    if (job.status === "failed") {
      const err = new Error(job.error?.message || "Background job failed");
      err.details = job.error?.details;
      err.code = job.error?.code;
      throw err;
    }
    if (job.status === "cancelled") {
      const err = new Error("Background job cancelled");
      err.code = "job_cancelled";
      throw err;
    }
    if (Date.now() - started > timeoutMs) {
      throw new Error("Background job timed out");
    }
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
}

async function resolveBackgroundJobResponse(response, message = "") {
  if (!response?.job) return response;
  if (message) toast(message, "info");
  const result = await waitForBackgroundJob(response.job);
  if (result.http_status && result.http_status >= 400) {
    const raw = result.error || {};
    const err = new Error(raw.message || "Background job failed");
    err.details = raw.details;
    err.code = raw.code;
    throw err;
  }
  return result;
}

function toast(message, kind = "info") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 4000);
  if (kind === "error" && !isDuplicateApiErrorToast(message)) {
    recordUiError(message, { source: "toast" });
  }
}

function isDuplicateApiErrorToast(message) {
  const last = state.lastApiErrorLog;
  if (!last || Date.now() - last.at > 5000) return false;
  const current = String(message || "");
  return current === last.message
    || current.includes(last.message)
    || last.message.includes(current);
}

function recordUiError(message, details = {}) {
  if (!message) return;
  const payload = {
    message: String(message),
    route: location.hash || location.pathname,
    ...details,
  };
  fetch("/api/activity/ui-error", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  }).catch(() => {});
}

// ---- Project attach/setup ---------------------------------------------------

function looksLikeGitRemoteInput(value) {
  const text = String(value || "").trim();
  if (text.startsWith("git@") || text.startsWith("ssh://") || text.startsWith("git://")) return true;
  if (text.startsWith("file://")) return true;
  try {
    const parsed = new URL(text);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch (_) {
    return false;
  }
}

function projectSetupFolderIcon() {
  return `
    <svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">
      <path d="M3 6.5A2.5 2.5 0 0 1 5.5 4h4.2l2 2H18.5A2.5 2.5 0 0 1 21 8.5v8A2.5 2.5 0 0 1 18.5 19h-13A2.5 2.5 0 0 1 3 16.5z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/>
    </svg>`;
}

async function resolveProjectSetupPath(path, { kind = "app", remote = "" } = {}) {
  const query = new URLSearchParams({ kind, path: path || "" });
  if (remote) query.set("remote", remote);
  return api("GET", `/api/project/path?${query.toString()}`);
}

function updateProjectSetupPreview(el, text) {
  if (!el) return;
  el.textContent = text;
  el.title = text;
}

async function openProjectDirectoryPicker({
  title = "Choose folder",
  value = "",
  kind = "app",
  remote = "",
} = {}) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop project-directory-backdrop";
    root.innerHTML = `
      <div class="modal project-directory-modal" role="dialog" aria-modal="true" aria-labelledby="project-directory-title">
        <div class="modal-title" id="project-directory-title">${htmlEscape(title)}</div>
        <div class="modal-body">
          <div class="project-directory-path-row">
            <input id="project-directory-path" type="text" class="modal-input"
                   autocomplete="off" value="${htmlEscape(value)}">
            <button type="button" class="secondary project-setup-icon-btn" id="project-directory-load" title="Open path" aria-label="Open path">
              ${projectSetupFolderIcon()}
            </button>
          </div>
          <div class="project-setup-path-preview" id="project-directory-preview"></div>
          <div class="project-directory-browser" id="project-directory-browser"></div>
          <div class="form-error" id="project-directory-error" style="display:none"></div>
        </div>
        <div class="modal-actions">
          <button class="secondary" type="button" id="project-directory-cancel">Cancel</button>
          <button class="secondary" type="button" id="project-directory-up">Up</button>
          <button type="button" id="project-directory-choose">Choose folder</button>
        </div>
      </div>`;
    document.body.appendChild(root);

    const input = root.querySelector("#project-directory-path");
    const preview = root.querySelector("#project-directory-preview");
    const browser = root.querySelector("#project-directory-browser");
    const error = root.querySelector("#project-directory-error");
    const upButton = root.querySelector("#project-directory-up");
    const chooseButton = root.querySelector("#project-directory-choose");
    let currentPath = "";
    let selectedPath = "";

    const close = (result) => {
      root.remove();
      resolve(result);
    };

    const load = async (pathValue = input.value.trim()) => {
      const query = new URLSearchParams({
        kind,
        path: pathValue || "",
        max_entries: "200",
      });
      if (remote) query.set("remote", remote);
      error.style.display = "none";
      browser.innerHTML = `<div class="project-directory-empty">Loading...</div>`;
      try {
        const result = await api("GET", `/api/project/directories?${query.toString()}`);
        currentPath = result.path || "";
        selectedPath = result.selected_path || currentPath;
        input.value = selectedPath;
        updateProjectSetupPreview(preview, `Selected path: ${selectedPath}`);
        upButton.disabled = !result.parent;
        upButton.dataset.parent = result.parent || "";
        const entries = Array.isArray(result.entries) ? result.entries : [];
        if (!entries.length) {
          browser.innerHTML = `<div class="project-directory-empty">No folders</div>`;
          return;
        }
        browser.innerHTML = entries.map((entry) => `
          <button type="button" class="project-directory-entry" data-path="${htmlEscape(entry.path)}">
            ${projectSetupFolderIcon()}
            <span>${htmlEscape(entry.name)}</span>
          </button>
        `).join("");
        if (result.truncated) {
          browser.insertAdjacentHTML("beforeend", `<div class="project-directory-empty">Folder list truncated</div>`);
        }
        browser.querySelectorAll(".project-directory-entry").forEach((btn) => {
          btn.addEventListener("click", () => load(btn.dataset.path || ""));
        });
      } catch (err) {
        browser.innerHTML = "";
        error.textContent = err.details || err.message || "Could not open folder";
        error.style.display = "";
      }
    };

    root.querySelector("#project-directory-cancel").addEventListener("click", () => close(null));
    root.querySelector("#project-directory-load").addEventListener("click", () => load());
    upButton.addEventListener("click", () => {
      if (upButton.dataset.parent) load(upButton.dataset.parent);
    });
    chooseButton.addEventListener("click", async () => {
      const typed = input.value.trim();
      try {
        const resolved = await resolveProjectSetupPath(typed, { kind, remote });
        close(resolved.path || typed || selectedPath || currentPath);
      } catch (_) {
        close(typed || selectedPath || currentPath);
      }
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        load();
      }
    });
    input.addEventListener("input", () => {
      const typed = input.value.trim();
      updateProjectSetupPreview(preview, `Selected path: ${typed || selectedPath || currentPath}`);
    });
    load(value);
    input.focus();
    input.select();
  });
}

function isManualProjectMigration(schema) {
  return !!(schema && schema.migration_required && schema.safe_auto === false);
}

function manualMigrationText(source) {
  return source?.operator_instructions
    || source?.details
    || "Stop all old Refine nodes for this app, run `refine migrate run [port]` from one upgraded checkout, push the migrated .refine state, then restart upgraded nodes.";
}

function isManualMigrationError(err) {
  const text = `${err?.details || ""}\n${err?.message || ""}`;
  return /refine migrate run|manual cluster migration/i.test(text);
}

async function ensureProjectAttached() {
  const snap = await refreshProjectStatus();
  if (!snap) return false;
  if (snap.attached) {
    const schema = snap.schema || {};
    if (schema.compatible !== false) {
      await syncProjectUpdates({ silent: true });
      return true;
    }
    if (schema.migration_required) {
      if (isManualProjectMigration(schema)) {
        $("#main").innerHTML = `
          <h2>Project migration required</h2>
          <p class="muted">${htmlEscape(manualMigrationText(schema))}</p>`;
        return false;
      }
      const ok = await modalConfirm(
        "This app uses an older Refine schema. Migrate .refine state and open it?",
        { title: "Migrate app", okLabel: "Migrate and open" },
      );
      if (!ok) {
        $("#main").innerHTML = `
          <h2>Project migration required</h2>
          <p class="muted">This app was not loaded because its .refine state needs migration.</p>`;
        return false;
      }
      try {
        const closeMigration = showProjectMigrationDialog();
        let result;
        try {
          result = await api("POST", "/api/project/attach", {
            path: snap.client_repo,
            migrate: true,
          });
        } finally {
          closeMigration();
        }
        await applyProjectAttachResult(result);
        return true;
      } catch (e) {
        toast(e.details || e.message || "Migration failed", "error");
        return false;
      }
    }
    $("#main").innerHTML = `
      <h2>Unsupported project schema</h2>
      <p class="muted">This app was not loaded because its .refine state was written by a newer Refine version.</p>`;
    return false;
  }
  enterNoProjectMode(snap);
  return false;
}

async function refreshProjectStatus() {
  let snap = null;
  try {
    snap = await api("GET", "/api/project/status");
  } catch (e) {
    toast(e.message || "Could not check project status", "error");
    return null;
  }
  state.project = snap;
  updateActiveNodeLabel();
  if (snap.attached === false) {
    clearProjectScopedUiState();
  }
  return snap;
}

async function syncProjectUpdates({ silent = false } = {}) {
  try {
    const result = await api("POST", "/api/project/sync", {});
    await refreshProjectStatus();
    if (!silent) toast(result.message || "Project updates synced", "info");
    return result;
  } catch (e) {
    if (!silent) {
      await showActionError(e, "Could not sync latest project updates");
    } else {
      const message = e.details || e.message || "Could not sync latest project updates";
      toast(message, "warn");
    }
    if (!silent) throw e;
    return null;
  }
}

function openProjectAttachModal({
  message = "",
  title = "Choose project",
  okLabel = "Attach project",
  defaultPath = "",
  reloadOnSuccess = true,
  openGuideOnSuccess = false,
} = {}) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop project-setup-backdrop";
    root.innerHTML = `
      <div class="modal project-setup-modal" role="dialog" aria-modal="true" aria-labelledby="project-setup-title">
        <form id="project-setup-form">
          <div class="modal-title" id="project-setup-title">${htmlEscape(title)}</div>
          <div class="modal-body">
            <p class="muted">${htmlEscape(message)}</p>
            <div class="project-setup-field">
              <label for="project-setup-path">Project path or Git remote</label>
              <div class="project-setup-input-row">
                <input id="project-setup-path" name="path" type="text" class="modal-input"
                       placeholder="/path/to/app or git@github.com:org/app.git" autocomplete="off" required
                       value="${htmlEscape(defaultPath)}">
                <button type="button" class="secondary project-setup-icon-btn" id="project-setup-path-browse"
                        title="Browse local folders" aria-label="Browse local folders">
                  ${projectSetupFolderIcon()}
                </button>
              </div>
              <div class="project-setup-path-preview" id="project-setup-path-preview"></div>
            </div>
            <div class="project-setup-field">
              <label for="project-setup-clone-path">Local destination</label>
              <div class="project-setup-input-row">
                <input id="project-setup-clone-path" name="clone_path" type="text" class="modal-input"
                       placeholder="Default: next to the Refine checkout" autocomplete="off" disabled>
                <button type="button" class="secondary project-setup-icon-btn" id="project-setup-clone-browse"
                        title="Browse clone destination" aria-label="Browse clone destination" disabled>
                  ${projectSetupFolderIcon()}
                </button>
              </div>
              <div class="project-setup-path-preview" id="project-setup-clone-preview"></div>
            </div>
            <p class="muted small">
              If the directory does not exist, refine will create it and run git init.
              If you paste a Git remote, refine will clone it first; private repos
              require working host credentials.
            </p>
            <div class="form-error" id="project-setup-error" style="display:none"></div>
          </div>
          <div class="modal-actions">
            <button class="secondary" type="button" id="project-setup-cancel">Cancel</button>
            <button type="submit" id="project-setup-submit">${htmlEscape(okLabel)}</button>
          </div>
        </form>
      </div>`;
    document.body.appendChild(root);

    const form = root.querySelector("#project-setup-form");
    const input = root.querySelector("#project-setup-path");
    const cloneInput = root.querySelector("#project-setup-clone-path");
    const pathPreview = root.querySelector("#project-setup-path-preview");
    const clonePreview = root.querySelector("#project-setup-clone-preview");
    const pathBrowse = root.querySelector("#project-setup-path-browse");
    const cloneBrowse = root.querySelector("#project-setup-clone-browse");
    const error = root.querySelector("#project-setup-error");
    const button = root.querySelector("#project-setup-submit");
    let previewTimer = 0;
    const refreshPathPreviews = async () => {
      const path = input.value.trim();
      const remote = looksLikeGitRemoteInput(path);
      if (!path) {
        updateProjectSetupPreview(pathPreview, "Final path: enter a local path or Git remote");
      } else if (remote) {
        updateProjectSetupPreview(pathPreview, `Git remote: ${path}`);
      } else {
        try {
          const resolved = await resolveProjectSetupPath(path, { kind: "app" });
          updateProjectSetupPreview(pathPreview, `Final path: ${resolved.path}`);
        } catch (_) {
          updateProjectSetupPreview(pathPreview, `Final path: ${path}`);
        }
      }
      if (!remote) {
        updateProjectSetupPreview(clonePreview, "Available when project path is a Git remote.");
        return;
      }
      const clonePath = cloneInput.value.trim();
      try {
        const resolved = await resolveProjectSetupPath(clonePath, { kind: "clone", remote: path });
        updateProjectSetupPreview(clonePreview, `Final path: ${resolved.path}`);
      } catch (_) {
        updateProjectSetupPreview(clonePreview, clonePath ? `Final path: ${clonePath}` : "Final path: default clone location from remote name");
      }
    };
    const schedulePathPreviews = () => {
      clearTimeout(previewTimer);
      previewTimer = setTimeout(refreshPathPreviews, 120);
    };
    const updateCloneInput = () => {
      const remote = looksLikeGitRemoteInput(input.value);
      cloneInput.disabled = !remote;
      cloneBrowse.disabled = !remote;
      if (!remote) cloneInput.value = "";
      schedulePathPreviews();
    };
    root.querySelector("#project-setup-cancel").addEventListener("click", () => {
      clearTimeout(previewTimer);
      root.remove();
      resolve(null);
    });
    input.addEventListener("input", updateCloneInput);
    cloneInput.addEventListener("input", schedulePathPreviews);
    pathBrowse.addEventListener("click", async () => {
      const selected = await openProjectDirectoryPicker({
        title: "Choose app folder",
        value: looksLikeGitRemoteInput(input.value) ? "" : input.value.trim(),
        kind: "app",
      });
      if (selected) {
        input.value = selected;
        updateCloneInput();
      }
    });
    cloneBrowse.addEventListener("click", async () => {
      if (cloneInput.disabled) return;
      const selected = await openProjectDirectoryPicker({
        title: "Choose clone destination",
        value: cloneInput.value.trim(),
        kind: "clone",
        remote: input.value.trim(),
      });
      if (selected) {
        cloneInput.value = selected;
        schedulePathPreviews();
      }
    });
    updateCloneInput();
    input.focus();
    input.select();

    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      const path = input.value.trim();
      if (!path) return;
      const attachBody = { path };
      if (looksLikeGitRemoteInput(path)) {
        const clonePath = cloneInput.value.trim();
        if (clonePath) attachBody.clone_path = clonePath;
      }
      error.style.display = "none";
      button.disabled = true;
      button.textContent = "Attaching...";
      try {
        const result = await api("POST", "/api/project/attach", attachBody);
        if (reloadOnSuccess) {
          if (typeof loadGuideStateForProject === "function") loadGuideStateForProject(result, { redraw: false });
          state.project = result;
          showProjectAttachToast(result);
          window.location.reload();
        } else {
          await applyProjectAttachResult(result, { openGuide: openGuideOnSuccess });
          root.remove();
          await maybeOpenProjectTemplateModal(result);
        }
        resolve(result);
      } catch (err) {
        if (err.status === 409 && /migration required/i.test(err.message || "")) {
          if (isManualMigrationError(err)) {
            error.textContent = manualMigrationText(err);
            error.style.display = "";
            button.disabled = false;
            button.textContent = okLabel;
            return;
          }
          const migrate = await modalConfirm(
            "This app uses an older Refine schema. Migrate .refine state and open it?",
            { title: "Migrate app", okLabel: "Migrate and open" },
          );
          if (migrate) {
            try {
              const closeMigration = showProjectMigrationDialog();
              let result;
              try {
                result = await api("POST", "/api/project/attach", { ...attachBody, migrate: true });
              } finally {
                closeMigration();
              }
              if (reloadOnSuccess) {
                if (typeof loadGuideStateForProject === "function") loadGuideStateForProject(result, { redraw: false });
                state.project = result;
                showProjectAttachToast(result);
                window.location.reload();
              } else {
                await applyProjectAttachResult(result, { openGuide: openGuideOnSuccess });
                root.remove();
                await maybeOpenProjectTemplateModal(result);
              }
              resolve(result);
              return;
            } catch (migrateErr) {
              err = migrateErr;
            }
          }
        }
        error.textContent = err.details || err.message || "Could not attach project";
        error.style.display = "";
        button.disabled = false;
        button.textContent = okLabel;
      }
    });
  });
}

function openAddAppModal(options = {}) {
  return openProjectAttachModal({
    message: "Add an existing app path, paste a Git remote, or choose a new directory to create and initialize.",
    title: "Add app",
    okLabel: "Add and switch",
    reloadOnSuccess: false,
    openGuideOnSuccess: true,
    ...options,
  });
}

function showProjectAttachToast(result) {
  if (result.runner && result.runner.started === false && result.runner.message) {
    toast(result.runner.message, "warn");
  } else {
    toast("Project attached", "success");
  }
}

function showProjectMigrationDialog(message = "Migrating selected application...") {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true">
      <div class="modal-title">Migrating application</div>
      <div class="modal-body">
        <div class="loading-row" style="padding:0">
          <span class="loading-spinner"></span>
          <span>${htmlEscape(message)}</span>
        </div>
      </div>
    </div>`;
  document.body.appendChild(root);
  return () => root.remove();
}

async function applyProjectAttachResult(result, options = {}) {
  state.project = result;
  if (typeof loadGuideStateForCurrentApp === "function") {
    loadGuideStateForCurrentApp({ redraw: true });
  } else if (typeof resetGuideState === "function") {
    resetGuideState({ redraw: true });
  }
  updateActiveNodeLabel();
  state.dashboard = null;
  state.currentGap = null;
  state.underlayHash = "#/project/application";
  if (typeof gapsExcludedIds !== "undefined") gapsExcludedIds.clear();
  if (options.toast !== false) showProjectAttachToast(result);
  resetChatForProjectSwitch();
  initSSE();
  await syncProjectUpdates({ silent: true });
  await refreshNodeScopedState({ selectReporterFallback: true });
  await refreshTargetAppToggle();
  if (location.hash !== "#/project/application") {
    location.hash = "#/project/application";
  } else if (["settings", "node", "project"].includes(state.currentRoute || "")) {
    await refreshSettings();
  } else {
    navigate();
  }
  if (options.openGuide && typeof openGuide === "function") {
    openGuide({
      context: result.config_created ? "app-created" : "app-existing",
      categoryId: "project",
      itemId: "project-application",
    });
  }
}

async function maybeOpenProjectTemplateModal(project) {
  if (!project || project.scaffold_required !== true) return null;
  let templates = Array.isArray(project.scaffold_templates) ? project.scaffold_templates : [];
  if (!templates.length) {
    templates = await loadProjectTemplates();
  }
  if (!templates.length) return null;
  return openProjectTemplateModal(templates);
}

async function openProjectTemplateSelector() {
  const templates = await loadProjectTemplates();
  if (!templates.length) {
    toast("No app templates are available", "warn");
    return null;
  }
  return openProjectTemplateModal(templates);
}

async function loadProjectTemplates() {
  try {
    const result = await api("GET", "/api/project/templates");
    return Array.isArray(result.templates) ? result.templates : [];
  } catch (e) {
    toast(e.message || "Could not load app templates", "error");
    return [];
  }
}

function openProjectTemplateModal(templates) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop project-template-backdrop";
    root.innerHTML = `
      <div class="modal project-template-modal" role="dialog" aria-modal="true" aria-labelledby="project-template-title">
        <div class="modal-title" id="project-template-title">Select app template</div>
        <div class="modal-body">
          <p class="muted small">
            This app does not have application code yet. Refine will create a high-priority Gap for the selected scaffold.
          </p>
          <div class="project-template-options">
            ${templates.map((template) => `
              <button type="button" class="project-template-option" data-template-id="${htmlEscape(template.id)}">
                <span class="project-template-name">${htmlEscape(template.name || template.id)}</span>
                <span class="project-template-summary">${htmlEscape(template.summary || "")}</span>
              </button>
            `).join("")}
          </div>
          <div class="form-error" id="project-template-error" style="display:none"></div>
        </div>
        <div class="modal-actions">
          <button class="secondary" type="button" id="project-template-cancel">Skip</button>
        </div>
      </div>`;
    document.body.appendChild(root);

    const error = root.querySelector("#project-template-error");
    let closed = false;
    function close(value) {
      if (closed) return;
      closed = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }
    function onKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(null);
      }
    }
    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (e) => {
      if (e.target === root) close(null);
    });
    root.querySelector("#project-template-cancel").addEventListener("click", () => close(null));
    $$(".project-template-option", root).forEach((button) => {
      button.addEventListener("click", async () => {
        const templateId = button.dataset.templateId || "";
        error.style.display = "none";
        $$(".project-template-option", root).forEach((b) => { b.disabled = true; });
        button.classList.add("is-selected");
        try {
          const result = await api("POST", "/api/project/scaffold", { template: templateId });
          const gap = result.gap || {};
          toast("Scaffold Gap created", "success");
          close(result);
          if (gap.id) location.hash = "#/gaps/" + encodeURIComponent(gap.id);
        } catch (e) {
          error.textContent = e.details || e.message || "Could not create scaffold Gap";
          error.style.display = "";
          button.classList.remove("is-selected");
          $$(".project-template-option", root).forEach((b) => { b.disabled = false; });
        }
      });
    });
    const first = root.querySelector(".project-template-option");
    if (first) first.focus();
  });
}

// ---- Modals (replace native prompt / confirm) -------------------------------
//
// Both return a Promise. modalPrompt resolves to the entered string or null
// if cancelled. modalConfirm resolves to a boolean.
//
// Keyboard: Enter submits, Escape cancels. Clicking the backdrop cancels.

function _openModal(buildBody, onResolveDefault, focusSel) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    const body = buildBody();
    root.innerHTML = `<div class="modal" role="dialog" aria-modal="true">${body}</div>`;
    document.body.appendChild(root);

    let resolved = false;
    function close(value) {
      if (resolved) return;
      resolved = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }
    function onKey(e) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(onResolveDefault.cancel);
      } else if (e.key === "Enter") {
        // Allow Enter inside a textarea to insert newlines (none today, but safe).
        if (e.target && e.target.tagName === "TEXTAREA") return;
        e.preventDefault();
        const okBtn = root.querySelector("[data-ok]");
        if (okBtn) okBtn.click();
      }
    }
    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (e) => {
      if (e.target === root) close(onResolveDefault.cancel);
    });

    root.querySelector("[data-cancel]")?.addEventListener("click", () =>
      close(onResolveDefault.cancel));
    root.querySelector("[data-ok]").addEventListener("click", () => {
      const input = root.querySelector(".modal-input");
      close(input ? input.value : onResolveDefault.ok);
    });

    const focus = root.querySelector(focusSel);
    if (focus) {
      focus.focus();
      if (focus.tagName === "INPUT") focus.select();
    }
  });
}

function modalPrompt(label, defaultValue = "", {
  title = null, okLabel = "OK", cancelLabel = "Cancel",
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body">
      <label>${htmlEscape(label)}</label>
      <input type="text" class="modal-input" value="${htmlEscape(defaultValue)}">
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: "" }, ".modal-input");
}

function modalConfirm(message, {
  title = null, okLabel = "OK", cancelLabel = "Cancel", danger = false,
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body modal-message-body">${htmlEscape(message)}</div>
    <div class="modal-actions">
      <button class="secondary" data-cancel>${htmlEscape(cancelLabel)}</button>
      <button ${danger ? 'class="danger"' : ""} data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: false, ok: true }, "[data-ok]");
}

function modalAlert(message, {
  title = "Action not allowed", okLabel = "OK",
} = {}) {
  const body = () => `
    ${title ? `<div class="modal-title">${htmlEscape(title)}</div>` : ""}
    <div class="modal-body modal-message-body">${htmlEscape(message)}</div>
    <div class="modal-actions">
      <button data-ok>${htmlEscape(okLabel)}</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: true }, "[data-ok]");
}

function isNodeOwnershipError(err) {
  return err?.code === "node_ownership"
    || (err?.status === 409 && /owned by another node/i.test(err?.message || ""));
}

function isBackgroundJobActiveError(err) {
  return err?.code === "background_job_active";
}

function backgroundJobActiveMessage(err) {
  const base = err?.message || "Refine is already applying changes.";
  const hasDetails = err?.details && base.includes(err.details);
  const details = err?.details && !hasDetails ? `\n\nActive operation: ${err.details}` : "";
  return `${base}${details}\n\nWait for the current operation to finish, then try again.`;
}

async function showActionError(err, fallbackPrefix = "") {
  if (isNodeOwnershipError(err)) {
    await modalAlert(err.message || "This action is not allowed because the Gap is owned by another node.");
    return;
  }
  if (isBackgroundJobActiveError(err)) {
    await modalAlert(backgroundJobActiveMessage(err), {
      title: "Refine is busy",
      okLabel: "OK",
    });
    return;
  }
  const message = err?.message || "Request failed";
  toast(fallbackPrefix ? `${fallbackPrefix}: ${message}` : message, "error");
}

function fmtTime(iso) {
  if (!iso) return "";
  const d = new Date(iso);
  if (isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function fmtElapsed(seconds) {
  seconds = Math.max(0, Math.floor(seconds));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  return `${(seconds / 3600).toFixed(1)}h`;
}

// Abbreviate counter values so dashboard cards stay legible at scale:
// 999 → "999", 1300 → "1.3K", 1_300_000 → "1.3M", 1_300_000_000 → "1.3B".
// Drops a trailing ".0" (so 1000 reads as "1K", not "1.0K"). Promotes
// to the next tier when 1-decimal rounding would push the value over
// 999 at the current tier (e.g. 999_999 reads as "1M", not "1000K").
function fmtCount(n) {
  n = Number(n) || 0;
  if (n < 1000) return String(n);
  const tiers = [
    { div: 1_000_000_000, suffix: "B" },
    { div: 1_000_000,     suffix: "M" },
    { div: 1_000,         suffix: "K" },
  ];
  for (let i = 0; i < tiers.length; i++) {
    const t = tiers[i];
    if (n < t.div) continue;
    const rounded = Math.round((n / t.div) * 10) / 10;
    if (rounded >= 1000 && i > 0) {
      const up = tiers[i - 1];
      return (n / up.div).toFixed(1).replace(/\.0$/, "") + up.suffix;
    }
    return rounded.toFixed(1).replace(/\.0$/, "") + t.suffix;
  }
  return String(n);
}

function renderPaginationControls(idPrefix, pageMeta = {}, itemCount = 0,
                                  noun = "entry", opts = {}) {
  const limit = Math.max(1, parseInt(pageMeta.limit || itemCount || 1, 10));
  const offset = Math.max(0, parseInt(pageMeta.offset || 0, 10));
  const page = Math.floor(offset / limit) + 1;
  const hasPrev = offset > 0;
  const hasNext = !!pageMeta.has_more;
  const total = Number.isFinite(pageMeta.total) ? pageMeta.total : null;
  const lastPage = total === null ? null : Math.max(1, Math.ceil(total / limit));
  const showBoundaries = !!opts.boundaries && lastPage !== null;
  if (!hasPrev && !hasNext) return "";
  const start = itemCount ? offset + 1 : offset;
  const end = offset + itemCount;
  const label = itemCount
    ? `${start}-${end} ${noun}${itemCount === 1 ? "" : "s"}`
    : `Page ${page}`;
  return `
    <div class="pagination" id="${htmlEscape(idPrefix)}-pagination">
      <span class="muted small">${htmlEscape(label)}</span>
      <span class="spacer"></span>
      ${showBoundaries ? `<button class="secondary small" data-page="1" ${hasPrev ? "" : "disabled"}>First</button>` : ""}
      <button class="secondary small" data-page="${page - 1}" ${hasPrev ? "" : "disabled"}>Previous</button>
      <span class="muted small">Page ${page}</span>
      <button class="secondary small" data-page="${page + 1}" ${hasNext ? "" : "disabled"}>Next</button>
      ${showBoundaries ? `<button class="secondary small" data-page="${lastPage}" ${page < lastPage ? "" : "disabled"}>Last</button>` : ""}
    </div>`;
}

function bindPaginationControls(root, idPrefix, onPage) {
  $$(`#${idPrefix}-pagination [data-page]`, root).forEach((btn) => {
    btn.addEventListener("click", () => onPage(parseInt(btn.dataset.page, 10)));
  });
}

function htmlEscape(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

// ---- Minimal Markdown → HTML ------------------------------------------------
//
// Used to render chat transcripts. Inputs come from the selected agent CLI's
// stream-json `assistant.content[].text` blocks (text only — never raw HTML),
// plus the user-echoed `> message` lines we synthesize locally. We html-escape
// every text fragment before substitution and only emit a small whitelist of
// inline tags, so even if agent text contained literal HTML we'd render it
// as literal text.
//
// Block-level: code fences (```), headings (#…######), unordered (- / *) and
// ordered (1.) lists, blockquotes (>), paragraphs separated by blank lines.
// Inline: **bold**, *italic*, `code`, [text](http(s)://...).
function mdToHtml(text) {
  if (!text) return "";
  const lines = String(text).replace(/\r\n/g, "\n").split("\n");
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    const fence = line.match(/^```\s*([^\s`]*)\s*$/);
    if (fence) {
      const lang = fence[1] || "";
      const code = [];
      i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i])) {
        code.push(lines[i]);
        i++;
      }
      if (i < lines.length) i++;
      const cls = lang ? ` class="lang-${htmlEscape(lang)}"` : "";
      out.push(`<pre><code${cls}>${htmlEscape(code.join("\n"))}</code></pre>`);
      continue;
    }
    if (!line.trim()) { i++; continue; }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      const lvl = heading[1].length;
      out.push(`<h${lvl}>${mdInline(heading[2])}</h${lvl}>`);
      i++;
      continue;
    }
    if (/^\s*>\s?/.test(line)) {
      const quoted = [];
      while (i < lines.length && /^\s*>\s?/.test(lines[i])) {
        quoted.push(lines[i].replace(/^\s*>\s?/, ""));
        i++;
      }
      // Recurse so nested formatting inside the quote works.
      out.push(`<blockquote>${mdToHtml(quoted.join("\n"))}</blockquote>`);
      continue;
    }
    if (/^\s*[-*]\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*[-*]\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*[-*]\s+/, ""));
        i++;
      }
      out.push(`<ul>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ul>`);
      continue;
    }
    if (/^\s*\d+\.\s+/.test(line)) {
      const items = [];
      while (i < lines.length && /^\s*\d+\.\s+/.test(lines[i])) {
        items.push(lines[i].replace(/^\s*\d+\.\s+/, ""));
        i++;
      }
      out.push(`<ol>${items.map(it => `<li>${mdInline(it)}</li>`).join("")}</ol>`);
      continue;
    }
    // Paragraph: gather until a blank line or a recognized block opener.
    const para = [];
    while (i < lines.length) {
      const ln = lines[i];
      if (!ln.trim()) break;
      if (/^```/.test(ln) || /^(#{1,6})\s+/.test(ln) ||
          /^\s*[-*]\s+/.test(ln) || /^\s*\d+\.\s+/.test(ln) ||
          /^\s*>\s?/.test(ln)) break;
      para.push(ln);
      i++;
    }
    if (para.length) {
      out.push(`<p>${mdInline(para.join("\n"))}</p>`);
    }
  }
  return out.join("\n");
}

function mdInline(text) {
  let s = htmlEscape(text);
  // Inline code first so its contents aren't mangled by later passes.
  s = s.replace(/`([^`\n]+)`/g, (_, c) => `<code>${c}</code>`);
  // Bold before italic so ** isn't greedily matched as italic.
  s = s.replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/__([^_\n]+)__/g, "<strong>$1</strong>");
  s = s.replace(/\*([^*\n]+)\*/g, "<em>$1</em>");
  s = s.replace(/(^|[^\w])_([^_\n]+)_(?![\w])/g, "$1<em>$2</em>");
  // Links: only http(s)/mailto pass through; anything else stays literal.
  s = s.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (m, txt, url) => {
    if (!/^(https?:|mailto:)/i.test(url)) return m;
    return `<a href="${url}" target="_blank" rel="noopener noreferrer">${txt}</a>`;
  });
  // Convert remaining hard newlines inside a paragraph to <br>.
  s = s.replace(/\n/g, "<br>");
  return s;
}

// ---- reporter dropdown ------------------------------------------------------

async function refreshReporters({ selectFallback = false } = {}) {
  if (!hasAttachedProject()) {
    state.reporters = [];
    setLastReporter("");
    populateAllReporterDropdowns();
    return;
  }
  const data = await api("GET", "/api/reporters");
  state.reporters = data.reporters || [];
  reconcileLastReporter({ selectFallback });
  populateAllReporterDropdowns();
}

async function refreshNodeScopedState({ selectReporterFallback = false } = {}) {
  if (typeof resetChatForProjectSwitch === "function") resetChatForProjectSwitch();
  state.reporters = [];
  state.dashboard = null;
  state.currentGap = null;
  setLastReporter("");
  populateAllReporterDropdowns();
  await refreshReporters({ selectFallback: selectReporterFallback });
}

function reconcileLastReporter({ selectFallback = false } = {}) {
  const names = state.reporters.map((r) => r.name).filter(Boolean);
  if (state.lastReporter && names.includes(state.lastReporter)) return;
  const next = selectFallback ? (names[0] || "") : "";
  setLastReporter(next);
}

function populateAllReporterDropdowns() {
  const all = $$("[data-reporter-select]");
  // include the global reporter selector in the topbar
  const globalSel = $("#global-reporter");
  if (globalSel) all.push(globalSel);
  for (const sel of all) {
    const current = sel.value || sel.dataset.value || state.lastReporter;
    sel.innerHTML = "";
    const optBlank = document.createElement("option");
    optBlank.value = "";
    optBlank.textContent = hasAttachedProject() ? "— pick reporter —" : "No reporter";
    sel.appendChild(optBlank);
    for (const r of state.reporters) {
      const opt = document.createElement("option");
      opt.value = r.name;
      opt.textContent = r.name;
      sel.appendChild(opt);
    }
    if (hasAttachedProject()) {
      const optAdd = document.createElement("option");
      optAdd.value = "__add__";
      optAdd.textContent = "+ Add new reporter…";
      sel.appendChild(optAdd);
    }
    // Restore selection if still valid
    const stillValid = state.reporters.some((r) => r.name === current);
    sel.value = stillValid ? current : "";
  }
  updateNavReporterContext();
}

async function handleReporterAdd(sel) {
  const name = await modalPrompt("Name for the new reporter:",
                                  "", { title: "Add reporter" });
  if (!name || !name.trim()) {
    sel.value = state.lastReporter || "";
    return null;
  }
  try {
    const { reporter } = await api("POST", "/api/reporters", { name });
    await refreshReporters();
    sel.value = reporter.name;
    setLastReporter(reporter.name);
    return reporter.name;
  } catch (e) {
    toast(`Could not add reporter: ${e.message}`, "error");
    sel.value = state.lastReporter || "";
    return null;
  }
}

function setLastReporter(name) {
  const wasEmpty = !state.lastReporter;
  state.lastReporter = name;
  if (name) localStorage.setItem("refine_last_reporter", name);
  else localStorage.removeItem("refine_last_reporter");
  const g = $("#global-reporter");
  if (g) g.value = name;
  updateNavReporterContext();
  // Keep any in-page "Submitting as X" indicator in sync without re-rendering
  // the form (which would lose the user's typed-but-unsubmitted text).
  for (const el of $$(".js-reporter-name")) el.textContent = name;
  // If the user just picked their first reporter, re-render views gated on a
  // selected reporter so the form replaces the "pick a reporter" notice.
  if (wasEmpty && name) {
    const r = state.currentRoute;
    if (r === "gaps_new" || r === "gaps_import" || r === "gaps_detail") {
      navigate();
    }
  }
  // Dashboard's "Awaiting your review" section is reporter-scoped — refresh
  // it whenever the selection changes so the list re-targets immediately.
  if (state.currentRoute === "dashboard") refreshDashboard();
}

// react to "+ Add new reporter" selection on any dropdown
document.addEventListener("change", async (e) => {
  if (e.target.matches("[data-reporter-select], #global-reporter")) {
    if (!hasAttachedProject()) {
      e.target.value = "";
      setLastReporter("");
      return;
    }
    if (e.target.value === "__add__") {
      const newName = await handleReporterAdd(e.target);
      if (newName) e.target.dispatchEvent(new Event("change-after-add"));
    } else if (e.target.value) {
      setLastReporter(e.target.value);
    }
  }
});

function closeTopbarMenus(target = null) {
  for (const menu of $$(".topbar-actions details[open]")) {
    if (!target || !menu.contains(target)) menu.open = false;
  }
}

// Topbar create/support actions open in place rather than navigating away.
// The hrefs are kept for deep-linking / accessibility; click handlers
// intercept so the user's current view stays underneath.
document.addEventListener("click", (e) => {
  const menuSummary = e.target.closest(".nav-menu > summary");
  if (menuSummary) {
    closeTopbarMenus(menuSummary);
  }
  if (e.target.closest("#btn-new-gap")) {
    e.preventDefault();
    closeTopbarMenus();
    runCommand("gap.new");
  } else if (e.target.closest("#btn-plan")) {
    e.preventDefault();
    closeTopbarMenus();
    runCommand("plan.open");
  } else if (e.target.closest("#btn-import")) {
    e.preventDefault();
    closeTopbarMenus();
    runCommand("gap.import");
  } else if (e.target.closest("#btn-refine-issue, #btn-refine-issue-menu")) {
    e.preventDefault();
    closeTopbarMenus();
    runCommand("refine.issue.request");
  } else if (e.target.closest("#target-app-indicator")) {
    closeTopbarMenus();
  } else if (e.target.closest(".nav-context-panel .nav-menu-item")) {
    closeTopbarMenus();
  } else if (!e.target.closest(".nav-menu")) {
    closeTopbarMenus();
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeTopbarMenus();
});

// ---- Banners ----------------------------------------------------------------

function renderBanners(items) {
  const root = $("#banners");
  root.innerHTML = "";
  for (const item of items) {
    const tpl = $("#t-banner").content.cloneNode(true);
    const div = tpl.querySelector(".banner");
    div.classList.add(item.severity || "error");
    tpl.querySelector(".banner-msg").textContent = item.message;
    if (item.action) {
      const btn = document.createElement("button");
      btn.textContent = item.action.label;
      btn.onclick = item.action.onClick;
      tpl.querySelector(".banner-actions").appendChild(btn);
    }
    root.appendChild(tpl);
  }
}

// ---- SSE --------------------------------------------------------------------

let sseSource = null;

function initSSE() {
  if (sseSource) sseSource.close();
  sseSource = new EventSource("/api/sse");
  sseSource.addEventListener("activity_added", (e) => {
    try {
      const entry = JSON.parse(e.data || "{}");
      if (typeof recordSystemOperation === "function") {
        recordSystemOperation({
          message: entry.message,
          status: entry.severity || "info",
          category: entry.category || "activity",
          timestamp: entry.datetime,
        });
      }
    } catch {}
    // Refresh dashboard activity if visible; refresh current gap if relevant.
    // Route through the silent `refresh*` paths — not `render*` — so the
    // screen doesn't blink back to `Loading…` on every event.
    if (typeof scheduleAgentStatusRefresh === "function") scheduleAgentStatusRefresh();
    if (state.currentRoute === "dashboard") refreshDashboard();
    if (state.currentRoute === "logs") loadLogs();
    if (state.currentRoute === "changes") loadChanges();
  });
  sseSource.addEventListener("status_change", () => {
    if (typeof scheduleAgentStatusRefresh === "function") scheduleAgentStatusRefresh();
    if (state.currentRoute === "dashboard") refreshDashboard();
    // Refresh only the table on background updates so an in-progress
    // keystroke in the search box isn't interrupted by a full re-render.
    if (state.currentRoute === "gaps") refreshGapsTable();
    if (state.currentRoute === "logs") loadLogs();
    if (["settings", "node", "project"].includes(state.currentRoute || "")) {
      refreshCurrentSettingsSurface();
    }
    // Changes screen: the Merge agent can land a new merge commit;
    // a cancellation flips an existing row's Undo button state.
    if (state.currentRoute === "changes") loadChanges();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      loadGapDetail(state.currentGap);
    }
  });
  sseSource.addEventListener("target_app_state", () => {
    refreshTargetAppToggle();
  });
  sseSource.addEventListener("target_app_health", () => {
    refreshTargetAppToggle();
  });
  sseSource.addEventListener("reporters_changed", async () => {
    await refreshReporters();
    if (state.currentRoute === "node"
        && document.querySelector('[data-tab-pane="reporters"].active')) {
      refreshCurrentSettingsSurface();
    }
  });
  sseSource.addEventListener("project_updated", async () => {
    await refreshProjectStatus();
    await refreshReporters();
    if (typeof refreshAgentStatusIndicator === "function") refreshAgentStatusIndicator();
    if (state.currentRoute === "dashboard") refreshDashboard();
    if (state.currentRoute === "gaps") refreshGapsTable();
    if (state.currentRoute === "logs") loadLogs();
    if (["settings", "node", "project"].includes(state.currentRoute || "")) refreshCurrentSettingsSurface();
    if (state.currentRoute === "changes") loadChanges();
    if (state.currentRoute === "gaps_detail" && state.currentGap) {
      loadGapDetail(state.currentGap);
    }
  });
  sseSource.addEventListener("round_log_added", () => {
    if (state.currentRoute === "logs") loadLogs();
  });
  sseSource.addEventListener("system_operation", (e) => {
    try {
      const payload = JSON.parse(e.data || "{}");
      if (typeof recordSystemOperation === "function") {
        recordSystemOperation(payload);
      }
    } catch {}
  });
  sseSource.onerror = () => {
    // Browser will auto-reconnect.
  };
}
