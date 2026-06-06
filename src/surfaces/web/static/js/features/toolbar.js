// ---- Toolbar ----------------------------------------------------------------

// chatState holds one tab per chat: the permanent "standalone" tab plus one
// per Gap that the user opened via Open Chat. Each tab carries its own
// session id, accumulated output, and closed-reason. Only the active tab is
// polled; output for other tabs accumulates server-side in the runner's
// per-session deque until the user switches to that tab.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const FILES_TAB_ID = "files";
const SYSTEM_TAB_ID = "system";
const STANDARD_TOOLBAR_TAB_ORDER = [SYSTEM_TAB_ID, FILES_TAB_ID, "standalone"];
const SYSTEM_OPERATION_LOG_LIMIT = 250;
const SYSTEM_LOG_FILTERS = [
  { status: "info", label: "Info" },
  { status: "start", label: "Started" },
  { status: "queued", label: "Queued" },
  { status: "complete", label: "Completed" },
  { status: "error", label: "Errors" },
];
const GAP_CHAT_ROUND_STATUSES = new Set([
  "backlog", "todo", "review", "done", "failed", "cancelled",
]);
const FILES_TREE_MAX_DEPTH = 3;
const FILES_TREE_MAX_ENTRIES = 200;
const FILES_SEARCH_MAX_RESULTS = 20;
const FILES_SEARCH_DEBOUNCE_MS = 250;
const FILE_TEXT_CHUNK_BYTES = 128_000;
const CHAT_ACTIVITY_PULSE_MS = 1800;
let filesSearchTimer = null;
let filesSearchRequestSeq = 0;
let filesSearchAbortController = null;
const chatState = {
  tabs: {},                // tabId → { gapId, label, sessionId, output, closedReason }
  activeTabId: "standalone",
  pollTimer: null,
  open: false,             // dock expanded?
  bodyHeight: null,        // user-resized body height in px; null → 20vh default
  fullscreen: false,       // when true, panel fills viewport below the topbar
};
const systemOperationState = {
  messages: [],
  filters: new Set(),
};
const filesState = {
  path: "",
  treeRootPath: "",
  pathInputValue: "",
  selectedPath: "",
  entriesByPath: {},
  treeMetaByPath: {},
  expanded: new Set([""]),
  file: null,
  fileChunkLoading: false,
  searchQuery: "",
  searchResults: null,
  searchSelectedIndex: -1,
  searchLoading: false,
  searchError: "",
  loading: false,
  error: "",
};

function ensureStandaloneTab() {
  if (!chatState.tabs.standalone) {
    chatState.tabs.standalone = {
      gapId: null, label: "Standalone", mode: "standalone",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, progress: "", showProgress: true,
    };
  }
  ensureFilesTab();
  ensureSystemTab();
  reorderStandardToolbarTabs();
}

function ensureFilesTab() {
  if (!chatState.tabs[FILES_TAB_ID]) {
    chatState.tabs[FILES_TAB_ID] = {
      gapId: null, label: "Files", mode: "files",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, progress: "", showProgress: true,
    };
  }
}

function ensureSystemTab() {
  if (!chatState.tabs[SYSTEM_TAB_ID]) {
    chatState.tabs[SYSTEM_TAB_ID] = {
      gapId: null, label: "System", mode: "system",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, progress: "", showProgress: true,
    };
  }
}

function reorderStandardToolbarTabs() {
  const existing = chatState.tabs || {};
  const ordered = {};
  for (const id of STANDARD_TOOLBAR_TAB_ORDER) {
    if (existing[id]) ordered[id] = existing[id];
  }
  for (const [id, tab] of Object.entries(existing)) {
    if (!STANDARD_TOOLBAR_TAB_ORDER.includes(id)) ordered[id] = tab;
  }
  chatState.tabs = ordered;
}

function loadChatStateFromStorage() {
  try {
    const raw = localStorage.getItem(CHAT_TABS_STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && parsed.tabs) {
      chatState.tabs = parsed.tabs;
      if (parsed.activeTabId && chatState.tabs[parsed.activeTabId]) {
        chatState.activeTabId = parsed.activeTabId;
      }
      if (typeof parsed.open === "boolean") chatState.open = parsed.open;
      if (typeof parsed.bodyHeight === "number" && parsed.bodyHeight > 0) {
        chatState.bodyHeight = parsed.bodyHeight;
      }
      if (typeof parsed.fullscreen === "boolean") {
        chatState.fullscreen = parsed.fullscreen;
      }
    }
  } catch {}
  ensureStandaloneTab();
}

function saveChatStateToStorage() {
  // We persist sessionIds too: the runner can keep them alive across page
  // navigations. On a stale id the next read returns alive=false and we
  // clear it.
  const tabs = {};
  for (const [id, t] of Object.entries(chatState.tabs)) {
      tabs[id] = {
        gapId: t.gapId, label: t.label,
        mode: t.mode || (t.gapId ? "gap" : id === "plan" ? "plan" : "standalone"),
        gapStatus: t.gapStatus || "",
        sessionId: t.sessionId,
        output: (t.output || "").slice(-50_000),
        progress: (t.progress || "").slice(-20_000),
        showProgress: t.showProgress !== false,
        closedReason: t.closedReason,
        agentResponded: !!t.agentResponded,
    };
  }
  try {
    localStorage.setItem(CHAT_TABS_STORAGE_KEY, JSON.stringify({
      tabs, activeTabId: chatState.activeTabId,
      open: chatState.open, bodyHeight: chatState.bodyHeight,
      fullscreen: chatState.fullscreen,
    }));
  } catch {}
}

function defaultToolbarBodyHeight() {
  return Math.max(120, Math.round(window.innerHeight * 0.20));
}

function defaultChatBodyHeight() { return defaultToolbarBodyHeight(); }

function clampToolbarBodyHeight(px) {
  const min = 120;
  const max = Math.max(min, Math.round(window.innerHeight * 0.85));
  return Math.max(min, Math.min(max, Math.round(px)));
}

function clampChatBodyHeight(px) { return clampToolbarBodyHeight(px); }

function initToolbar() {
  loadChatStateFromStorage();
  ensureStandaloneTab();
  if (typeof drainPendingSystemOperations === "function") drainPendingSystemOperations();
  drawToolbar();
  observeToolbarSize();
  observeTopbarHeight();
}

function initChatDock() { initToolbar(); }

function resetChatForProjectSwitch() {
  if (chatState.pollTimer) {
    clearInterval(chatState.pollTimer);
    chatState.pollTimer = null;
  }
  chatState.tabs = {};
  chatState.activeTabId = "standalone";
  chatState.open = false;
  chatState.fullscreen = false;
  ensureStandaloneTab();
  resetFilesState();
  saveChatStateToStorage();
  drawToolbar();
}

function resetFilesState() {
  filesState.path = "";
  filesState.selectedPath = "";
  filesState.treeRootPath = "";
  filesState.pathInputValue = "";
  filesState.entriesByPath = {};
  filesState.treeMetaByPath = {};
  filesState.expanded = new Set([""]);
  filesState.file = null;
  filesState.fileChunkLoading = false;
  filesState.searchQuery = "";
  filesState.searchResults = null;
  filesState.searchSelectedIndex = -1;
  filesState.searchLoading = false;
  filesState.searchError = "";
  filesState.loading = false;
  filesState.error = "";
}

// Publish the topbar's actual height as --topbar-height on <html> so the
// fullscreen Toolbar can anchor its top edge just below the main nav.
function observeTopbarHeight() {
  const topbar = document.querySelector(".topbar");
  if (!topbar) return;
  const apply = () => {
    document.documentElement.style.setProperty(
      "--topbar-height", `${topbar.offsetHeight}px`,
    );
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(topbar);
  } else {
    window.addEventListener("resize", apply);
  }
}

// Keep --toolbar-dock-height in sync with whatever vertical space the dock
// actually occupies (collapsed bar, expanded panel, or mid-drag). `body`
// reads this variable as its bottom padding so page content never slides
// underneath the dock.
function observeToolbarSize() {
  const root = $("#toolbar-dock");
  if (!root) return;
  const apply = () => {
    document.documentElement.style.setProperty(
      "--toolbar-dock-height", `${root.offsetHeight}px`,
    );
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(root);
  } else {
    window.addEventListener("resize", apply);
  }
}

function observeChatDockSize() { observeToolbarSize(); }

// Opens the dock and (optionally) ensures a tab for a specific gap is active.
// Wired up by the "Open Chat" button on the gap detail page and by any
// surviving `#/chat?gap=...` deep links. For gap tabs with no live session,
// kicks off a chat session immediately so the runner can inject the Gap
// context into the provider session before the user types.
function openChatDock({ gapId = null, gapStatus = null } = {}) {
  ensureStandaloneTab();
  if (gapId) {
    if (!chatState.tabs[gapId]) {
      chatState.tabs[gapId] = {
        gapId,
        label: `Gap ${gapId.slice(0, 8)}…`,
        mode: "gap",
        gapStatus: gapStatus || "",
        sessionId: null, output: "", progress: "", showProgress: true,
        closedReason: null, agentResponded: false,
      };
    } else if (gapStatus) {
      chatState.tabs[gapId].gapStatus = gapStatus;
    }
    chatState.activeTabId = gapId;
  }
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  if (gapId) {
    const t = chatState.tabs[gapId];
    if (t && !t.sessionId) startGapChatSession(t);
  }
}

async function renderGapPlan() {
  await renderGapsList();
  openPlanChatDock();
}

function ensurePlanTab() {
  ensureStandaloneTab();
  if (!chatState.tabs.plan) {
    chatState.tabs.plan = {
      gapId: null,
      label: "Plan",
      mode: "plan",
      sessionId: null,
      output: "",
      progress: "",
      showProgress: true,
      closedReason: null,
      agentResponded: false,
    };
  }
}

async function openPlanChatDock(options = {}) {
  const initialPrompt = typeof options === "string"
    ? options
    : String(options.initialPrompt || "");
  ensurePlanTab();
  chatState.activeTabId = "plan";
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  const t = chatState.tabs.plan;
  let started = Promise.resolve();
  if (t && !t.sessionId) {
    started = startPlanChatSession(t);
  }
  if (initialPrompt.trim()) {
    await started;
    await sendChatText(initialPrompt);
  }
}

async function startPlanChatSession(tab) {
  try {
    const r = await api("POST", "/api/chat/start", { purpose: "plan" });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    tab.mode = "plan";
    tab.progress = "";
    tab.showProgress = true;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    drawToolbar();
    $("#chat-input")?.focus();
  } catch (e) {
    toast("Could not start plan: " + e.message, "error");
  }
}

async function startGapChatSession(tab) {
  try {
    const r = await api("POST", "/api/chat/start", { gap_id: tab.gapId });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    tab.progress = "";
    tab.showProgress = true;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    drawToolbar();
    $("#chat-input")?.focus();
  } catch (e) {
    toast("Could not start chat: " + e.message, "error");
  }
}

function toggleToolbar() {
  chatState.open = !chatState.open;
  // Collapsing the dock also exits fullscreen — leaving fullscreen on
  // while the body is hidden would orphan the topbar offset.
  if (!chatState.open) chatState.fullscreen = false;
  saveChatStateToStorage();
  drawToolbar();
}

function toggleChatDock() { toggleToolbar(); }

function minimizeToolbar() {
  if (!chatState.open && !chatState.fullscreen) return;
  chatState.open = false;
  chatState.fullscreen = false;
  saveChatStateToStorage();
  drawToolbar();
}

function toggleToolbarFullscreen() {
  chatState.fullscreen = !chatState.fullscreen;
  if (chatState.fullscreen) chatState.open = true;  // fullscreen implies open
  saveChatStateToStorage();
  drawToolbar();
}

function toggleChatFullscreen() { toggleToolbarFullscreen(); }

function drawToolbar() {
  const root = $("#toolbar-dock");
  if (!root) return;
  ensureStandaloneTab();
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const active = tabs[activeId] || tabs.standalone;
  const filesActive = active.mode === "files";
  const systemActive = active.mode === "system";
  const hasSession = !!active.sessionId;

  const startLabel = active.gapId
    ? `Start attached to Gap ${active.gapId.slice(0, 10)}…`
    : active.mode === "plan"
      ? "Start plan"
    : "Start standalone";
  const toggleLabel = hasSession
    ? (active.gapId ? "Stop session" : active.mode === "plan" ? "Stop plan" : "Stop standalone")
    : startLabel;
  const toggleClass = hasSession ? "danger" : "";

  const statusLine = !active.sessionId
    ? "No active session."
    : (active.closedReason
        ? `Session ${active.sessionId} ended — ${active.closedReason}.`
        : `Session ${active.sessionId} active.`);

  root.classList.toggle("open", !!chatState.open);
  root.classList.toggle("fullscreen", !!chatState.fullscreen);
  if (chatState.open && !chatState.bodyHeight) {
    chatState.bodyHeight = defaultToolbarBodyHeight();
  }
  root.innerHTML = `
    <div class="toolbar-dock-resize" id="toolbar-dock-resize"
         role="separator" aria-orientation="horizontal"
         aria-label="Resize Toolbar"
         title="Drag to resize"></div>
    <div class="toolbar-dock-bar" id="toolbar-dock-bar"
         title="${chatState.open ? "Click to collapse" : "Click a tab to expand Toolbar"}">
      <span class="toolbar-dock-label">TOOLBAR</span>
      <div class="toolbar-tabs">
        ${Object.entries(tabs).map(([id, t]) => `
          <button class="toolbar-tab ${id === activeId ? "active" : ""}"
                  data-tab-id="${htmlEscape(id)}"
                  title="${htmlEscape(toolbarTabTitle(t))}">
            ${htmlEscape(t.label)}${t.sessionId ? ` <span class="toolbar-tab-dot" title="active session"></span>` : ""}
            ${id === "standalone" || id === FILES_TAB_ID || id === SYSTEM_TAB_ID ? "" : `<span class="toolbar-tab-close" data-close-tab="${htmlEscape(id)}" title="Close tab">×</span>`}
          </button>`).join("")}
      </div>
      <button class="toolbar-dock-toggle toolbar-dock-fullscreen-btn${chatState.fullscreen ? " active" : ""}"
              id="btn-dock-fullscreen"
              aria-label="${chatState.fullscreen ? "Exit fullscreen Toolbar" : "Fullscreen Toolbar"}"
              aria-pressed="${chatState.fullscreen ? "true" : "false"}"
              title="${chatState.fullscreen ? "Exit fullscreen" : "Fullscreen"}">⛶</button>
      <button class="toolbar-dock-toggle toolbar-dock-collapse" id="btn-dock-toggle"
              aria-label="${chatState.open ? "Collapse Toolbar" : "Expand Toolbar"}"
              title="${chatState.open ? "Collapse Toolbar" : "Expand Toolbar"}">▾</button>
    </div>
    <div class="toolbar-dock-body"
         style="${chatState.bodyHeight ? `height:${chatState.bodyHeight}px` : ""}">
      ${filesActive
        ? renderFilesPanel()
        : systemActive
          ? renderSystemPanel()
          : renderChatPanel(active, {
              toggleClass,
              toggleLabel,
              statusLine,
              hasSession,
            })}
    </div>
  `;
  if (!filesActive && !systemActive) applyPendingIndicator(active);
  if (filesActive) bindFilesPanel(root);
  if (systemActive) bindSystemPanel(root);

  if (chatState.open && !filesActive && !systemActive) {
    const out = $("#chat-output");
    if (out) out.scrollTop = out.scrollHeight;
    if (active.gapId && !active.gapStatus) refreshGapChatStatus(active.gapId);
  }

  $$(".toolbar-tab", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      if (e.target.matches("[data-close-tab]")) return;
      const id = el.dataset.tabId;
      if (!id) return;
      if (id === chatState.activeTabId) {
        // Clicking the active tab toggles the dock open/closed.
        toggleToolbar();
      } else {
        switchChatTab(id);
        if (!chatState.open) {
          chatState.open = true;
          saveChatStateToStorage();
          drawToolbar();
        }
      }
    });
  });
  $$("[data-close-tab]", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      closeChatTab(el.dataset.closeTab);
    });
  });
  $("#btn-dock-toggle")?.addEventListener("click", toggleToolbar);
  $("#btn-dock-fullscreen")?.addEventListener("click", toggleToolbarFullscreen);
  if (!filesActive && !systemActive) {
    $("#btn-chat-toggle")?.addEventListener("click", toggleActiveChat);
    $("#btn-plan-draft")?.addEventListener("click", draftGapsFromPlan);
    $("#btn-gap-round-extract")?.addEventListener("click", extractRoundFromGapChat);
    $("#btn-chat-clear")?.addEventListener("click", clearActiveChat);
    $("#chat-activity-toggle")?.addEventListener("click", toggleChatProgress);
    $("#chat-input")?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        sendChatLine();
      }
    });
    $("#chat-input")?.addEventListener("input", (e) => {
      resizeChatInput(e.currentTarget);
    });
    resizeChatInput($("#chat-input"));
  }

  wireToolbarResize(root);
  restartPollForActiveTab();
  if (filesActive && !filesState.entriesByPath[""] && !filesState.loading) {
    loadFilesDirectory("", { expand: true, redraw: true });
  }
}

function drawChatDock() { drawToolbar(); }

function toolbarTabTitle(tab) {
  if (tab.mode === "files") return "File browser";
  if (tab.mode === "system") return "System operations";
  return tab.gapId || "Standalone chat";
}

function renderChatPanel(active, { toggleClass, toggleLabel, statusLine, hasSession }) {
  const progressText = active.progress || "";
  const showProgress = active.showProgress !== false;
  const hasActivityToggle = hasSession || progressText;
  const showActivityPanel = showProgress && hasActivityToggle;
  const activityLabel = chatActivityLabel(active);
  const showInputDots = chatActivityIsPulsing(active);
  const progressToggleLabel = showProgress ? "Collapse activity" : "Expand activity";
  const inputPlaceholder = chatInputPlaceholder(active);
  return `
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-chat-toggle" class="${toggleClass}">${htmlEscape(toggleLabel)}</button>
        ${active.mode === "plan" ? `
          <button id="btn-plan-draft" class="secondary"
                  ${planHasAgentResponse(active) ? "" : "disabled"}>
            Draft Feature
          </button>` : ""}
        ${active.gapId ? `
          <button id="btn-gap-round-extract" class="secondary"
                  ${gapChatCanExtractRound(active) ? "" : "disabled"}>
            Draft Round
          </button>` : ""}
        <button id="btn-chat-clear" class="secondary"
                ${(active.output || active.progress || active.sessionId) ? "" : "disabled"}>
          Clear history
        </button>
        ${active.gapId ? `
          <a id="chat-gap-link" class="chat-gap-link"
             href="#/gaps/${encodeURIComponent(active.gapId)}"
             title="Open Gap ${htmlEscape(active.gapId)}">
            Gap ${htmlEscape(active.gapId.slice(0, 10))}…
          </a>` : ""}
        <span class="spacer"></span>
        <span id="chat-status" class="muted small">${htmlEscape(statusLine)}</span>
      </div>
      <div class="chat-output-box">
        <div id="chat-output" class="chat-output">${mdToHtml(active.output || "")}</div>
        <button type="button"
                id="chat-activity-toggle"
                class="chat-activity-toggle"
                aria-expanded="${showProgress ? "true" : "false"}"
                title="${htmlEscape(progressToggleLabel)}"
                ${hasActivityToggle ? "" : "hidden"}>
          <span id="chat-activity-label">${htmlEscape(activityLabel)}</span>
          <span class="chat-activity-chevron" aria-hidden="true">
            ${toolbarIcon(showProgress ? "collapse" : "expand")}
          </span>
        </button>
        <div id="chat-progress-panel" class="chat-progress-panel" ${showActivityPanel ? "" : "hidden"}>
          <div id="chat-progress" class="chat-progress">${renderChatProgress(progressText)}</div>
        </div>
      </div>
      <div class="actions" style="margin-top:8px">
        <div class="chat-input-wrap">
          <span id="chat-input-pending-dots"
                class="chat-pending-dots chat-input-pending-dots"
                ${showInputDots ? "" : "hidden"}>
            <span></span><span></span><span></span>
          </span>
          <textarea id="chat-input"
                    class="${showInputDots ? "chat-input-waiting" : ""}"
                    rows="2"
                    placeholder="${htmlEscape(inputPlaceholder)}"
                    ${hasSession && !active.pending ? "" : "disabled"}></textarea>
        </div>
      </div>
    `;
}

function renderChatProgress(text) {
  const lines = String(text || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .slice(-80);
  if (!lines.length) {
    return `<div class="chat-progress-empty">Waiting for activity.</div>`;
  }
  return lines.map((line) => `
    <div class="chat-progress-line">${htmlEscape(line)}</div>
  `).join("");
}

function resizeChatInput(input) {
  if (!input) return;
  input.style.height = "auto";
  const max = 120;
  const next = Math.min(max, Math.max(42, input.scrollHeight || 42));
  input.style.height = `${next}px`;
}

function recordSystemOperation(payload) {
  const item = {
    message: String(payload?.message || "").trim(),
    status: normalizeSystemLogStatus(payload?.status),
    category: String(payload?.category || "system"),
    timestamp: String(payload?.timestamp || new Date().toISOString()),
  };
  if (!item.message) return;
  if (isDuplicateSystemOperation(item)) return;
  systemOperationState.messages.push(item);
  if (systemOperationState.messages.length > SYSTEM_OPERATION_LOG_LIMIT) {
    systemOperationState.messages = systemOperationState.messages.slice(-SYSTEM_OPERATION_LOG_LIMIT);
  }
  if (chatState.tabs[SYSTEM_TAB_ID] && chatState.open && chatState.activeTabId === SYSTEM_TAB_ID) {
    drawToolbar();
  }
}

function isDuplicateSystemOperation(item) {
  const cutoff = Date.parse(item.timestamp || "") - 5000;
  return systemOperationState.messages.slice(-20).some((existing) => {
    if (existing.message !== item.message || existing.status !== item.status) return false;
    const existingTime = Date.parse(existing.timestamp || "");
    if (Number.isNaN(existingTime) || Number.isNaN(cutoff)) return true;
    return existingTime >= cutoff;
  });
}

function renderSystemPanel() {
  const messages = systemOperationState.messages.slice(-SYSTEM_OPERATION_LOG_LIMIT);
  const activeFilters = activeSystemLogFilters(messages);
  const visibleMessages = !activeFilters.size
    ? messages
    : messages.filter((item) => activeFilters.has(item.status));
  const countLabel = !activeFilters.size
    ? `${messages.length} / ${SYSTEM_OPERATION_LOG_LIMIT}`
    : `${visibleMessages.length} of ${messages.length}`;
  return `
    <div class="system-panel">
      <div class="system-panel-header">
        <span>System operations</span>
        ${renderSystemLogFilters(messages, activeFilters)}
        <span class="muted small">${countLabel}</span>
      </div>
      <div class="system-log" role="log" aria-live="polite" aria-label="Recent system operations">
        ${visibleMessages.length
          ? visibleMessages.map(renderSystemLogLine).join("")
          : `<div class="system-log-empty">${messages.length ? "No system activity matches this filter." : "Waiting for system activity."}</div>`}
      </div>
    </div>`;
}

function renderSystemLogFilters(messages, activeFilters) {
  const options = systemLogFilterOptions(messages);
  return `
    <div class="system-log-filters" aria-label="Filter system operations">
      <label class="system-log-filter${!activeFilters.size ? " active" : ""}">
        <input type="checkbox"
               data-system-log-filter="all"
               ${!activeFilters.size ? "checked" : ""}
               aria-label="Show all system operations">
        <span>All</span>
      </label>
      ${options.map((option) => `
        <label class="system-log-filter system-log-filter-${option.status}${activeFilters.has(option.status) ? " active" : ""}">
          <input type="checkbox"
                 data-system-log-filter="${htmlEscape(option.status)}"
                 ${activeFilters.has(option.status) ? "checked" : ""}
                 aria-label="Show ${htmlEscape(option.label.toLowerCase())} system operations">
          <span>${htmlEscape(option.label)}</span>
        </label>`).join("")}
    </div>`;
}

function systemLogFilterOptions(messages) {
  const options = [...SYSTEM_LOG_FILTERS];
  const knownStatuses = new Set(options.map((option) => option.status));
  for (const item of messages) {
    if (knownStatuses.has(item.status)) continue;
    knownStatuses.add(item.status);
    options.push({ status: item.status, label: systemLogStatusLabel(item.status) });
  }
  return options;
}

function activeSystemLogFilters(messages) {
  const knownStatuses = new Set(systemLogFilterOptions(messages).map((option) => option.status));
  return new Set([...systemOperationState.filters].filter((status) => knownStatuses.has(status)));
}

function bindSystemPanel(root) {
  $$("[data-system-log-filter]", root).forEach((el) => {
    el.addEventListener("change", () => {
      const filter = el.dataset.systemLogFilter || "all";
      if (filter === "all") {
        systemOperationState.filters.clear();
      } else if (systemOperationState.filters.has(filter)) {
        systemOperationState.filters.delete(filter);
      } else {
        systemOperationState.filters.add(filter);
      }
      drawToolbar();
    });
  });
}

function systemLogStatusLabel(status) {
  return String(status || "info")
    .split(/[-_]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ") || "Info";
}

function renderSystemLogLine(item) {
  const time = formatSystemLogTime(item.timestamp);
  return `
    <div class="system-log-line system-log-${item.status}">
      <span class="system-log-time">${htmlEscape(time)}</span>
      <span class="system-log-message">${htmlEscape(item.message)}</span>
    </div>`;
}

function normalizeSystemLogStatus(status) {
  return String(status || "info").toLowerCase().replace(/[^a-z0-9_-]+/g, "-") || "info";
}

function formatSystemLogTime(raw) {
  const date = new Date(raw || "");
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function toolbarIcon(name) {
  const icons = {
    clear: '<path d="M18 6 6 18"></path><path d="m6 6 12 12"></path>',
    collapse: '<path d="m18 15-6-6-6 6"></path>',
    copy: '<rect x="9" y="9" width="10" height="10" rx="2"></rect><path d="M5 15V7a2 2 0 0 1 2-2h8"></path>',
    expand: '<path d="m6 9 6 6 6-6"></path>',
    go: '<path d="M5 12h14"></path><path d="m13 6 6 6-6 6"></path>',
    refresh: '<path d="M20 11a8 8 0 0 0-13.7-4.6L4 8"></path><path d="M4 4v4h4"></path><path d="M4 13a8 8 0 0 0 13.7 4.6L20 16"></path><path d="M20 20v-4h-4"></path>',
    search: '<path d="m21 21-4.3-4.3"></path><circle cx="11" cy="11" r="7"></circle>',
  };
  return `<svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">${icons[name] || ""}</svg>`;
}

function renderFilesPanel() {
  const inputPath = filesState.pathInputValue || "";
  const status = filesState.loading
    ? "Loading..."
    : filesState.error
      ? filesState.error
      : filesState.file?.path
        ? filesState.file.path
        : "Select a file.";
  return `
    <div class="files-panel">
      <div class="files-pathbar">
        <label for="files-path-input" class="files-path-label">Path</label>
        <input type="text" id="files-path-input"
               autocomplete="off" spellcheck="false"
               placeholder="Repo-relative path"
               value="${htmlEscape(inputPath)}">
        <button type="button" class="secondary files-icon-btn"
                data-files-copy title="Copy path" aria-label="Copy path">
          ${toolbarIcon("copy")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-clear title="Clear path" aria-label="Clear path">
          ${toolbarIcon("clear")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-go title="Go to path" aria-label="Go to path">
          ${toolbarIcon("go")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-refresh title="Refresh" aria-label="Refresh">
          ${toolbarIcon("refresh")}
        </button>
      </div>
      <div class="files-browser">
        <div class="files-tree-panel">
          <div class="files-tree-header">
            <span>Files</span>
            <div class="files-tree-actions">
              <button type="button" class="secondary files-icon-btn"
                      data-files-expand-all title="Expand all" aria-label="Expand all">
                ${toolbarIcon("expand")}
              </button>
              <button type="button" class="secondary files-icon-btn"
                      data-files-clear-tree title="Clear tree" aria-label="Clear tree">
                ${toolbarIcon("clear")}
              </button>
              <button type="button" class="secondary files-icon-btn"
                      data-files-collapse-all title="Collapse all" aria-label="Collapse all">
                ${toolbarIcon("collapse")}
              </button>
            </div>
          </div>
          <div class="files-tree-search">
            <span class="files-tree-search-icon">${toolbarIcon("search")}</span>
            <input type="search" id="files-search-input"
                   autocomplete="off" spellcheck="false"
                   placeholder="Search files"
                   value="${htmlEscape(filesState.searchQuery || "")}">
          </div>
          <div class="files-tree" role="tree" aria-label="Directories and files">
            ${renderFilesTreePanel()}
          </div>
        </div>
        <div class="files-content">
          <div class="files-content-header">
            <span class="muted small">${htmlEscape(status)}</span>
            ${filesState.file?.previewable ? `
              <button type="button" class="secondary files-icon-btn"
                      data-files-copy-content
                      title="Copy file contents"
                      aria-label="Copy file contents">
                ${toolbarIcon("copy")}
              </button>` : ""}
          </div>
          ${renderFilesContent()}
        </div>
      </div>
    </div>`;
}

function renderFilesTreePanel() {
  const query = (filesState.searchQuery || "").trim();
  if (query) return renderFilesSearchResults();
  return renderFilesTree(filesState.treeRootPath || "");
}

function renderFilesSearchResults() {
  if (filesState.searchLoading && !filesState.searchResults) {
    return `<p class="muted small files-empty">Searching...</p>`;
  }
  if (filesState.searchError) {
    return `<p class="muted small files-empty">${htmlEscape(filesState.searchError)}</p>`;
  }
  const results = filesState.searchResults;
  if (!results) {
    return `<p class="muted small files-empty">Type to search the target repo.</p>`;
  }
  const entries = results.entries || [];
  if (!entries.length) {
    return `<p class="muted small files-empty">No matches for "${htmlEscape(results.query || filesState.searchQuery)}".</p>`;
  }
  const selectedIndex = normalizedFilesSearchSelectedIndex(results);
  const rows = entries.map((entry, idx) => {
    const selected = idx === selectedIndex;
    return `
    <div class="files-tree-item files-search-result ${selected ? "selected" : ""}"
         role="treeitem"
         aria-selected="${selected ? "true" : "false"}"
         style="--depth:0"
         data-files-path="${htmlEscape(entry.path)}"
         data-files-type="${htmlEscape(entry.type)}"
         data-files-search-index="${idx}"
         data-files-search-result="1">
      <span class="files-tree-caret" aria-hidden="true">${entry.type === "directory" ? "▸" : ""}</span>
      <span class="files-tree-name">
        <span>${htmlEscape(entry.name || entry.path || ".")}</span>
        <small>${htmlEscape(entry.path || entry.name || "")}</small>
      </span>
      ${selected && entry.type === "file" ? `<span class="files-search-action">Enter to open</span>` : ""}
    </div>`;
  }).join("");
  const limit = results.truncated
    ? `<p class="muted small files-empty">Showing first ${FILES_SEARCH_MAX_RESULTS} matches.</p>`
    : "";
  const loading = filesState.searchLoading
    ? `<p class="muted small files-empty">Searching...</p>`
    : "";
  return loading + rows + limit;
}

function renderFilesTree(path = "", depth = 0) {
  const entries = filesState.entriesByPath[path];
  const meta = filesState.treeMetaByPath[path] || {};
  if (!entries) {
    return depth === 0
      ? `<p class="muted small files-empty">Loading repository...</p>`
      : "";
  }
  if (!entries.length && depth === 0) {
    return `<p class="muted small files-empty">No files.</p>`;
  }
  const rows = entries.map((entry) => {
    const isDir = entry.type === "directory";
    const expandable = isDir && depth < FILES_TREE_MAX_DEPTH;
    const expanded = expandable && filesState.expanded.has(entry.path);
    const selected = entry.path === filesState.selectedPath;
    return `
      <div class="files-tree-item ${selected ? "selected" : ""}"
           role="treeitem"
           aria-expanded="${expandable ? expanded ? "true" : "false" : ""}"
           style="--depth:${depth}"
           data-files-path="${htmlEscape(entry.path)}"
           data-files-type="${htmlEscape(entry.type)}"
           data-files-depth="${depth}">
        <span class="files-tree-caret" aria-hidden="true">${expandable ? expanded ? "▾" : "▸" : ""}</span>
        <span class="files-tree-name">${htmlEscape(entry.name || entry.path || ".")}</span>
      </div>
      ${expandable && expanded ? renderFilesTree(entry.path, depth + 1) : ""}`;
  }).join("");
  const limit = meta.truncated
    ? `<p class="muted small files-empty">Showing first ${FILES_TREE_MAX_ENTRIES} entries.</p>`
    : "";
  const depthLimit = depth === FILES_TREE_MAX_DEPTH && entries.some((entry) => entry.type === "directory")
    ? `<p class="muted small files-empty">Tree depth limit reached.</p>`
    : "";
  return rows + limit + depthLimit;
}

function renderFilesContent() {
  const file = filesState.file;
  if (filesState.loading && !file) {
    return `<div class="files-message">Loading...</div>`;
  }
  if (filesState.error && !file) {
    return `<div class="files-message">${htmlEscape(filesState.error)}</div>`;
  }
  if (!file) {
    return `<div class="files-message">Choose a file from the tree or enter a path.</div>`;
  }
  if (!file.previewable) {
    return `<div class="files-message">${htmlEscape(file.reason || "Preview is not available.")}</div>`;
  }
  if (file.kind === "image") {
    return `
      <div class="files-image-preview">
        <img src="${htmlEscape(file.data_url || "")}" alt="${htmlEscape(file.name || file.path || "Image preview")}">
      </div>`;
  }
  return `
    <div class="files-source" data-language="${htmlEscape(languageForPath(file.path))}">
      ${renderSourceLines(file.content || "", file.path, file.start_line || 1)}
      ${file.has_more ? `
        <div class="files-load-more" data-files-load-more>
          ${filesState.fileChunkLoading ? "Loading..." : "Scroll to load more"}
        </div>` : ""}
    </div>`;
}

function renderSourceLines(content, path, startLine = 1) {
  const lang = languageForPath(path);
  const lines = String(content ?? "").replace(/\r\n/g, "\n").split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  const shown = lines.length ? lines : [""];
  return shown.map((line, idx) => `
    <div class="files-source-line">
      <span class="files-line-number">${startLine + idx}</span>
      <code class="files-line-code">${highlightFileLine(line, lang)}</code>
    </div>`).join("");
}

function languageForPath(path) {
  const ext = String(path || "").toLowerCase().split(".").pop() || "";
  return {
    js: "js", jsx: "js", ts: "js", tsx: "js",
    css: "css",
    html: "html", htm: "html",
    sh: "sh", bash: "sh", zsh: "sh",
    cs: "cs",
    py: "py",
    rs: "rs",
    md: "md", markdown: "md",
    json: "json",
    toml: "toml",
    yaml: "yaml", yml: "yaml",
    sql: "sql",
  }[ext] || "text";
}

function highlightFileLine(line, lang) {
  let s = htmlEscape(line);
  if (lang === "md") {
    s = s.replace(/^(#{1,6})(\s.*)?$/, '<span class="tok-keyword">$1</span><span class="tok-string">$2</span>');
    s = s.replace(/(`[^`]+`)/g, '<span class="tok-string">$1</span>');
    return s;
  }
  if (lang === "html") {
    s = s.replace(/(&lt;\/?[\w:-]+)/g, '<span class="tok-keyword">$1</span>');
    s = s.replace(/([\w:-]+)=(&quot;.*?&quot;|&#39;.*?&#39;)/g, '<span class="tok-attr">$1</span>=<span class="tok-string">$2</span>');
    return s;
  }
  if (lang === "css") {
    s = s.replace(/(\/\*.*?\*\/)/g, '<span class="tok-comment">$1</span>');
    s = s.replace(/([\w-]+)(\s*:)/g, '<span class="tok-attr">$1</span>$2');
    s = s.replace(/(#[-\w]+|\.[-\w]+|:[-\w]+)/g, '<span class="tok-keyword">$1</span>');
    return s;
  }
  if (["js", "cs", "py", "rs", "sh"].includes(lang)) {
    s = s.replace(/(&quot;.*?&quot;|&#39;.*?&#39;|`.*?`)/g, '<span class="tok-string">$1</span>');
    s = s.replace(/(\b\d+(?:\.\d+)?\b)/g, '<span class="tok-number">$1</span>');
    const keywords = {
      js: "async|await|break|case|catch|class|const|continue|default|else|export|for|from|function|if|import|let|new|return|switch|throw|try|var|while",
      cs: "class|namespace|using|public|private|protected|static|void|string|int|bool|var|new|return|if|else|for|foreach|while|async|await",
      py: "and|as|class|def|elif|else|except|False|finally|for|from|if|import|in|is|lambda|None|not|or|pass|return|True|try|while|with",
      rs: "async|await|break|const|continue|crate|else|enum|fn|for|if|impl|let|loop|match|mod|mut|pub|return|self|struct|trait|use|where|while",
      sh: "case|do|done|elif|else|esac|fi|for|function|if|in|then|while",
    }[lang];
    s = s.replace(new RegExp(`\\b(${keywords})\\b`, "g"), '<span class="tok-keyword">$1</span>');
    s = s.replace(/(#.*$|\/\/.*$)/g, '<span class="tok-comment">$1</span>');
    return s;
  }
  if (["json", "toml", "yaml", "sql"].includes(lang)) {
    s = s.replace(/(&quot;[^&]*?&quot;)(\s*:)?/g, '<span class="tok-string">$1</span>$2');
    s = s.replace(/(\b\d+(?:\.\d+)?\b|true|false|null)/gi, '<span class="tok-number">$1</span>');
    s = s.replace(/(#.*$|--.*$)/g, '<span class="tok-comment">$1</span>');
  }
  return s;
}

function bindFilesPanel(root) {
  root.querySelector("#files-path-input")?.addEventListener("input", (e) => {
    filesState.pathInputValue = e.target.value || "";
  });
  root.querySelector("#files-path-input")?.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    navigateFilesPath(e.target.value);
  });
  root.querySelector("#files-search-input")?.addEventListener("input", (e) => {
    filesState.searchSelectedIndex = -1;
    scheduleFilesSearch(e.target.value);
  });
  root.querySelector("#files-search-input")?.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      moveFilesSearchSelection(1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      moveFilesSearchSelection(-1);
      return;
    }
    if (e.key !== "Enter") return;
    e.preventDefault();
    const currentQuery = String(e.target.value || "").trim();
    if (!filesState.searchLoading && filesState.searchResults?.query === currentQuery && openSelectedFilesSearchResult()) return;
    runFilesSearch(e.target.value, { refocus: true, openSelectedFile: true });
  });
  root.querySelector("[data-files-go]")?.addEventListener("click", () => {
    navigateFilesPath(root.querySelector("#files-path-input")?.value || "");
  });
  root.querySelector("[data-files-clear]")?.addEventListener("click", () => clearFilesPathInput());
  root.querySelector("[data-files-refresh]")?.addEventListener("click", () => refreshFilesPanel());
  root.querySelector("[data-files-expand-all]")?.addEventListener("click", () => expandAllFilesTree());
  root.querySelector("[data-files-clear-tree]")?.addEventListener("click", () => clearFilesTreeView());
  root.querySelector("[data-files-collapse-all]")?.addEventListener("click", () => collapseAllFilesTree());
  root.querySelector("[data-files-copy]")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(root.querySelector("#files-path-input")?.value || "");
      toast("Path copied", "info");
    } catch {
      toast("Clipboard copy is unavailable.", "error");
    }
  });
  root.querySelector("[data-files-copy-content]")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(filesState.file?.content || "");
      toast("File contents copied", "info");
    } catch {
      toast("Clipboard copy is unavailable.", "error");
    }
  });
  const source = root.querySelector(".files-source");
  source?.addEventListener("scroll", () => {
    if (!filesState.file?.has_more || filesState.fileChunkLoading) return;
    const remaining = source.scrollHeight - source.scrollTop - source.clientHeight;
    if (remaining < 240) loadNextFileChunk();
  });
  $$(".files-tree-item", root).forEach((row) => {
    row.addEventListener("click", () => {
      const path = row.dataset.filesPath || "";
      const type = row.dataset.filesType || "";
      const depth = Number.parseInt(row.dataset.filesDepth || "0", 10);
      if (row.dataset.filesSearchResult === "1") {
        filesState.searchSelectedIndex = Number.parseInt(row.dataset.filesSearchIndex || "-1", 10);
      }
      if (type === "directory") {
        if (row.dataset.filesSearchResult === "1") {
          filesState.searchQuery = "";
          filesState.searchResults = null;
          filesState.searchSelectedIndex = -1;
          filesState.searchError = "";
          loadFilesDirectory(path, { expand: true, redraw: true });
          return;
        }
        if (depth >= FILES_TREE_MAX_DEPTH) {
          filesState.path = path;
          filesState.selectedPath = path;
          drawToolbar();
          return;
        }
        if (filesState.expanded.has(path)) {
          filesState.expanded.delete(path);
          filesState.path = path;
          filesState.selectedPath = path;
          drawToolbar();
        } else {
          loadFilesDirectory(path, { expand: true, redraw: true });
        }
      } else if (type === "file") {
        loadFile(path);
      } else {
        filesState.selectedPath = path;
        filesState.error = "This entry cannot be previewed.";
        drawToolbar();
      }
    });
  });
}

function scheduleFilesSearch(query) {
  filesState.searchQuery = String(query || "");
  filesState.searchError = "";
  cancelFilesSearchRequest();
  if (!filesState.searchQuery.trim()) {
    filesState.searchLoading = false;
    filesState.searchResults = null;
    filesState.searchSelectedIndex = -1;
    drawToolbar();
    focusFilesSearchInput();
    return;
  }
  filesSearchTimer = setTimeout(() => {
    runFilesSearch(filesState.searchQuery, { refocus: true });
  }, FILES_SEARCH_DEBOUNCE_MS);
}

function cancelFilesSearchRequest({ invalidate = true } = {}) {
  if (filesSearchTimer) {
    clearTimeout(filesSearchTimer);
    filesSearchTimer = null;
  }
  if (filesSearchAbortController) {
    filesSearchAbortController.abort();
    filesSearchAbortController = null;
  }
  if (invalidate) filesSearchRequestSeq += 1;
}

function topFilesSearchFile(results) {
  return (results?.entries || []).find((entry) => entry.type === "file") || null;
}

function filesSearchFileIndexes(results = filesState.searchResults) {
  return (results?.entries || [])
    .map((entry, idx) => entry.type === "file" ? idx : -1)
    .filter((idx) => idx >= 0);
}

function normalizedFilesSearchSelectedIndex(results = filesState.searchResults) {
  const fileIndexes = filesSearchFileIndexes(results);
  if (!fileIndexes.length) return -1;
  if (fileIndexes.includes(filesState.searchSelectedIndex)) {
    return filesState.searchSelectedIndex;
  }
  filesState.searchSelectedIndex = fileIndexes[0];
  return filesState.searchSelectedIndex;
}

function selectedFilesSearchEntry() {
  const selectedIndex = normalizedFilesSearchSelectedIndex();
  if (selectedIndex < 0) return null;
  return filesState.searchResults?.entries?.[selectedIndex] || null;
}

function moveFilesSearchSelection(delta) {
  const fileIndexes = filesSearchFileIndexes();
  if (!fileIndexes.length) return;
  const selectedIndex = normalizedFilesSearchSelectedIndex();
  const current = Math.max(0, fileIndexes.indexOf(selectedIndex));
  const next = Math.min(fileIndexes.length - 1, Math.max(0, current + delta));
  filesState.searchSelectedIndex = fileIndexes[next];
  drawToolbar();
  focusFilesSearchInput();
  scrollSelectedFilesSearchResultIntoView();
}

function openSelectedFilesSearchResult() {
  const entry = selectedFilesSearchEntry();
  if (!entry || entry.type !== "file") return false;
  loadFile(entry.path);
  return true;
}

function scrollSelectedFilesSearchResultIntoView() {
  requestAnimationFrame(() => {
    const row = document.querySelector(".files-search-result.selected");
    row?.scrollIntoView({ block: "nearest" });
  });
}

async function runFilesSearch(query, { refocus = false, openSelectedFile = false } = {}) {
  cancelFilesSearchRequest({ invalidate: false });
  query = String(query || "").trim();
  filesSearchRequestSeq += 1;
  const requestSeq = filesSearchRequestSeq;
  filesState.searchQuery = query;
  filesState.searchError = "";
  if (!query) {
    filesState.searchLoading = false;
    filesState.searchResults = null;
    filesState.searchSelectedIndex = -1;
    drawToolbar();
    if (refocus) focusFilesSearchInput();
    return;
  }
  filesState.searchLoading = true;
  drawToolbar();
  if (refocus) focusFilesSearchInput();
  const controller = new AbortController();
  filesSearchAbortController = controller;
  try {
    const result = await api(
      "GET",
      `/api/files/search?q=${encodeURIComponent(query)}&max_entries=${FILES_SEARCH_MAX_RESULTS}`,
      undefined,
      { signal: controller.signal },
    );
    if (requestSeq !== filesSearchRequestSeq) return;
    filesState.searchResults = result;
    normalizedFilesSearchSelectedIndex(result);
    filesState.searchLoading = false;
    drawToolbar();
    if (refocus) focusFilesSearchInput();
    scrollSelectedFilesSearchResultIntoView();
    if (openSelectedFile) {
      const entry = selectedFilesSearchEntry() || topFilesSearchFile(result);
      if (entry) await loadFile(entry.path);
    }
  } catch (e) {
    if (e?.name === "AbortError") return;
    if (requestSeq !== filesSearchRequestSeq) return;
    filesState.searchLoading = false;
    filesState.searchResults = null;
    filesState.searchSelectedIndex = -1;
    filesState.searchError = e.message || String(e);
    drawToolbar();
    if (refocus) focusFilesSearchInput();
  } finally {
    if (filesSearchAbortController === controller) {
      filesSearchAbortController = null;
    }
  }
}

function focusFilesSearchInput() {
  const input = $("#files-search-input");
  if (!input) return;
  input.focus();
  const end = input.value.length;
  try { input.setSelectionRange(end, end); } catch {}
}

async function expandAllFilesTree() {
  const treeRoot = filesState.treeRootPath || "";
  filesState.loading = true;
  filesState.error = "";
  drawToolbar();
  try {
    const query = [
      `path=${encodeURIComponent(treeRoot)}`,
      "recursive=1",
      `max_depth=${FILES_TREE_MAX_DEPTH}`,
      `max_entries=${FILES_TREE_MAX_ENTRIES}`,
    ].join("&");
    const result = await api("GET", `/api/files/tree?${query}`);
    mergeFilesTreeResult(result);
    filesState.expanded = new Set(Object.keys(result.entries_by_path || { "": [] }));
    filesState.expanded.add(result.path || "");
    filesState.path = result.path || "";
    filesState.treeRootPath = result.path || "";
    filesState.selectedPath = filesState.selectedPath || result.path || "";
    filesState.loading = false;
    drawToolbar();
    if (result.truncated) {
      toast(`File tree limited to ${FILES_TREE_MAX_ENTRIES} entries.`, "warn");
    }
  } catch (e) {
    filesState.loading = false;
    filesState.error = e.message || String(e);
    drawToolbar();
  }
}

function collapseAllFilesTree() {
  filesState.expanded = new Set([filesState.treeRootPath || ""]);
  drawToolbar();
}

async function clearFilesTreeView() {
  cancelFilesSearchRequest();
  const treeRoot = filesState.treeRootPath || "";
  filesState.searchQuery = "";
  filesState.searchResults = null;
  filesState.searchSelectedIndex = -1;
  filesState.searchLoading = false;
  filesState.searchError = "";
  filesState.path = treeRoot;
  filesState.selectedPath = treeRoot;
  filesState.file = null;
  filesState.fileChunkLoading = false;
  filesState.error = "";
  filesState.expanded = new Set([treeRoot]);
  delete filesState.entriesByPath[treeRoot];
  await loadFilesDirectory(treeRoot, { expand: true, redraw: true });
}

async function openFilesToolbar(options = {}) {
  ensureFilesTab();
  chatState.activeTabId = FILES_TAB_ID;
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  const opts = typeof options === "string" ? { path: options } : options;
  const path = String(opts.path || "");
  const search = String(opts.search || "").trim();
  const focusSearch = !!opts.focusSearch || !!search;
  if (search) {
    filesState.searchQuery = search;
    filesState.searchResults = null;
    filesState.searchSelectedIndex = -1;
    filesState.searchError = "";
  }
  if (path.trim()) {
    await navigateFilesPath(path);
  } else if (!filesState.entriesByPath[""]) {
    await loadFilesDirectory("", { expand: true, redraw: true });
  }
  if (search) {
    await runFilesSearch(search, { refocus: focusSearch });
  } else if (focusSearch) {
    focusFilesSearchInput();
  }
}

async function navigateFilesPath(rawPath) {
  filesState.pathInputValue = String(rawPath || "");
  const path = normalizeFilesPath(rawPath);
  filesState.selectedPath = path;
  filesState.path = path;
  filesState.error = "";
  try {
    const result = await loadFilesDirectory(path, { expand: true, redraw: false });
    filesState.treeRootPath = result.path || "";
    drawToolbar();
  } catch (e) {
    await loadFile(path);
  }
}

async function clearFilesPathInput() {
  filesState.pathInputValue = "";
  filesState.path = "";
  filesState.treeRootPath = "";
  filesState.selectedPath = "";
  filesState.error = "";
  filesState.expanded = new Set([""]);
  delete filesState.entriesByPath[""];
  await loadFilesDirectory("", { expand: true, redraw: true });
}

async function refreshFilesPanel() {
  const dir = filesState.treeRootPath || "";
  delete filesState.entriesByPath[dir];
  await loadFilesDirectory(dir, { expand: true, redraw: false }).catch(() => {});
  if (filesState.selectedPath) {
    await loadFile(filesState.selectedPath, { redraw: false }).catch(() => {});
  }
  drawToolbar();
}

async function loadFilesDirectory(path, { expand = false, redraw = true } = {}) {
  path = normalizeFilesPath(path);
  filesState.loading = true;
  filesState.error = "";
  if (redraw) drawToolbar();
  try {
    const result = await api("GET", `/api/files/tree?path=${encodeURIComponent(path)}`);
    mergeFilesTreeResult(result);
    filesState.path = result.path || "";
    filesState.selectedPath = result.path || "";
    if (expand) filesState.expanded.add(result.path || "");
    filesState.loading = false;
    if (redraw) drawToolbar();
    return result;
  } catch (e) {
    filesState.loading = false;
    filesState.error = e.message || String(e);
    if (redraw) drawToolbar();
    throw e;
  }
}

function mergeFilesTreeResult(result) {
  if (!result) return;
  const entriesByPath = result.entries_by_path || {
    [result.path || ""]: result.entries || [],
  };
  for (const [path, entries] of Object.entries(entriesByPath)) {
    filesState.entriesByPath[path] = entries || [];
  }
  const metaByPath = result.meta_by_path || {
    [result.path || ""]: {
      truncated: !!result.truncated,
      depth: 0,
    },
  };
  for (const [path, meta] of Object.entries(metaByPath)) {
    filesState.treeMetaByPath[path] = meta || {};
  }
}

async function loadFile(path, { redraw = true } = {}) {
  path = normalizeFilesPath(path);
  filesState.loading = true;
  filesState.error = "";
  if (redraw) drawToolbar();
  try {
    const file = await api(
      "GET",
      `/api/files/read?path=${encodeURIComponent(path)}&offset=0&limit=${FILE_TEXT_CHUNK_BYTES}`,
    );
    filesState.file = file;
    filesState.fileChunkLoading = false;
    filesState.selectedPath = file.path || path;
    filesState.path = parentPath(file.path || path);
    filesState.loading = false;
    if (redraw) drawToolbar();
    return file;
  } catch (e) {
    filesState.file = null;
    filesState.fileChunkLoading = false;
    filesState.loading = false;
    filesState.error = e.message || String(e);
    if (redraw) drawToolbar();
    throw e;
  }
}

async function loadNextFileChunk() {
  const file = filesState.file;
  if (!file || !file.has_more || file.next_offset == null) return;
  const scrollTop = $(".files-source")?.scrollTop || 0;
  filesState.fileChunkLoading = true;
  drawToolbar();
  restoreFilesSourceScroll(scrollTop);
  try {
    const next = await api(
      "GET",
      `/api/files/read?path=${encodeURIComponent(file.path)}&offset=${encodeURIComponent(file.next_offset)}&limit=${FILE_TEXT_CHUNK_BYTES}`,
    );
    if (!filesState.file || filesState.file.path !== file.path) return;
    filesState.file.content = `${filesState.file.content || ""}${next.content || ""}`;
    filesState.file.has_more = !!next.has_more;
    filesState.file.next_offset = next.next_offset;
    filesState.file.large = !!next.large;
    filesState.fileChunkLoading = false;
    drawToolbar();
    restoreFilesSourceScroll(scrollTop);
  } catch (e) {
    filesState.fileChunkLoading = false;
    filesState.error = e.message || String(e);
    drawToolbar();
    restoreFilesSourceScroll(scrollTop);
  }
}

function restoreFilesSourceScroll(scrollTop) {
  requestAnimationFrame(() => {
    const source = $(".files-source");
    if (source) source.scrollTop = scrollTop;
  });
}

function normalizeFilesPath(path) {
  const normalized = String(path || "")
    .replace(/\\/g, "/")
    .replace(/^\/+/, "")
    .replace(/\/+/g, "/")
    .replace(/\/$/, "");
  return normalized
    .split("/")
    .filter((part) => part && part !== ".")
    .join("/");
}

function parentPath(path) {
  const parts = normalizeFilesPath(path).split("/").filter(Boolean);
  parts.pop();
  return parts.join("/");
}

function wireToolbarResize(root) {
  const handle = root.querySelector("#toolbar-dock-resize");
  const body = root.querySelector(".toolbar-dock-body");
  if (!handle || !body) return;
  handle.addEventListener("pointerdown", (e) => {
    if (!chatState.open) return;
    e.preventDefault();
    const startY = e.clientY;
    const startH = body.getBoundingClientRect().height;
    handle.setPointerCapture(e.pointerId);
    root.classList.add("resizing");
    function onMove(ev) {
      // Drag up grows the panel; drag down shrinks it.
      const next = clampToolbarBodyHeight(startH + (startY - ev.clientY));
      body.style.height = next + "px";
      chatState.bodyHeight = next;
    }
    function onUp(ev) {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      try { handle.releasePointerCapture(ev.pointerId); } catch {}
      root.classList.remove("resizing");
      saveChatStateToStorage();
    }
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  });
}

function wireChatDockResize(root) { wireToolbarResize(root); }

// Back-compat alias used by helpers below; thin wrapper.
function drawChat() { drawToolbar(); }

function refreshProcessesTabForChatChange() {
  if (state.currentRoute !== "node") return;
  if (typeof readSettingsTab === "function" && readSettingsTab() !== "processes") return;
  if (typeof refreshSettings !== "function") return;
  refreshSettings().catch(() => {});
}

function applyPendingIndicator(tab) {
  const toggle = $("#chat-activity-toggle");
  const dots = $("#chat-input-pending-dots");
  const label = $("#chat-activity-label");
  const input = $("#chat-input");
  if (toggle) {
    toggle.hidden = !tab || !(tab.sessionId || tab.progress);
    toggle.setAttribute("aria-expanded", tab?.showProgress === false ? "false" : "true");
    toggle.title = tab?.showProgress === false ? "Expand activity" : "Collapse activity";
  }
  if (dots) dots.hidden = !chatActivityIsPulsing(tab);
  if (label) label.textContent = chatActivityLabel(tab);
  if (input) {
    input.disabled = !tab || !tab.sessionId || tab.pending;
    input.placeholder = chatInputPlaceholder(tab);
    input.classList.toggle("chat-input-waiting", chatActivityIsPulsing(tab));
  }
  syncChatActionButtons(tab);
}

function markChatActivityPulse(tab) {
  if (!tab) return;
  tab.activityPulseUntil = Date.now() + CHAT_ACTIVITY_PULSE_MS;
}

function chatActivityIsPulsing(tab) {
  return !!tab?.pending;
}

function chatActivityLabel(tab) {
  return "Activity panel";
}

function chatInputPlaceholder(tab) {
  if (!tab?.sessionId) {
    return "Click Start to begin session before sending messages is enabled.";
  }
  if (tab.pending) return "Waiting on agent...";
  return "Type and press enter.";
}

function syncChatActionButtons(tab) {
  syncPlanDraftButton(tab);
  syncGapRoundExtractButton(tab);
}

function syncPlanDraftButton(tab) {
  const btn = $("#btn-plan-draft");
  if (!btn || !tab || tab.mode !== "plan") return;
  btn.disabled = !planHasAgentResponse(tab);
}

function syncGapRoundExtractButton(tab) {
  const btn = $("#btn-gap-round-extract");
  if (!btn || !tab || !tab.gapId) return;
  btn.disabled = !gapChatCanExtractRound(tab);
}

function restartPollForActiveTab() {
  if (chatState.pollTimer) {
    clearInterval(chatState.pollTimer);
    chatState.pollTimer = null;
  }
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || t.mode === "files" || t.mode === "system" || !t.sessionId) return;
  chatState.pollTimer = setInterval(pollChat, 800);
  // Fire an immediate poll so the user doesn't wait 800ms for the first read.
  pollChat();
}

function switchChatTab(tabId) {
  if (!chatState.tabs[tabId]) return;
  chatState.activeTabId = tabId;
  saveChatStateToStorage();
  drawChat();
}

async function closeChatTab(tabId) {
  if (tabId === "standalone" || tabId === FILES_TAB_ID || tabId === SYSTEM_TAB_ID) return;
  const t = chatState.tabs[tabId];
  if (!t) return;
  if (t.sessionId) {
    try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
    refreshProcessesTabForChatChange();
  }
  delete chatState.tabs[tabId];
  if (chatState.activeTabId === tabId) chatState.activeTabId = "standalone";
  saveChatStateToStorage();
  drawChat();
}

async function clearActiveChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t) return;
  if (!t.output && !t.progress && !t.sessionId) return;     // nothing to clear
  const btn = $("#btn-chat-clear");
  const ok = await modalConfirm(
    "Clear this chat's history? Any active session will be stopped and " +
    "the transcript wiped. Starting again gives the agent a fresh conversation.",
    { title: "Clear chat history", okLabel: "Clear", danger: true,
      cancelLabel: "Keep history" },
  );
  if (!ok) return;
  await withButtonBusy(btn, "Clearing…", async () => {
    if (t.sessionId) {
      try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
      refreshProcessesTabForChatChange();
    }
    t.sessionId = null;
    t.output = "";
    t.progress = "";
    t.showProgress = true;
    t.closedReason = null;
    t.pending = false;
    t.agentResponded = false;
    saveChatStateToStorage();
    drawChat();
  });
}

async function toggleActiveChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t) return;
  const btn = $("#btn-chat-toggle");
  if (t.sessionId) {
    await withButtonBusy(btn, "Stopping…", async () => {
      try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
      t.sessionId = null;
      t.closedReason = "stopped by user";
      saveChatStateToStorage();
      refreshProcessesTabForChatChange();
      drawChat();
    });
    return;
  }
  await withButtonBusy(btn, "Starting…", async () => {
    try {
      const r = await api("POST", "/api/chat/start",
                          t.gapId ? { gap_id: t.gapId } : t.mode === "plan" ? { purpose: "plan" } : {});
      t.sessionId = r.session_id;
      t.closedReason = null;
      t.output = "";
      t.progress = "";
      t.showProgress = true;
      t.agentResponded = false;
      saveChatStateToStorage();
      refreshProcessesTabForChatChange();
      drawChat();
      $("#chat-input")?.focus();
    } catch (e) {
      toast("Could not start chat: " + e.message, "error");
    }
  });
}

function toggleChatProgress() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t) return;
  t.showProgress = t.showProgress === false;
  saveChatStateToStorage();
  drawChat();
}

function planTranscriptText(tab) {
  return (tab?.output || "")
    .split(/\r?\n/)
    .filter((line) => !line.startsWith("[refine]"))
    .join("\n")
    .trim();
}

function chatLinesIncludeAgentResponse(lines) {
  return (lines || []).some((line) => {
    const text = String(line || "").trim();
    return text && !text.startsWith("[refine]");
  });
}

function planHasAgentResponse(tab) {
  if (!tab) return false;
  if (tab.agentResponded) return true;
  return (tab.output || "")
    .split(/\r?\n/)
    .some((line) => {
      const text = line.trim();
      return text && !text.startsWith("[refine]") && !text.startsWith(">");
    });
}

function gapChatCanExtractRound(tab) {
  return !!(
    tab
    && tab.gapId
    && GAP_CHAT_ROUND_STATUSES.has(tab.gapStatus || "")
    && !tab.pending
    && gapChatTranscriptText(tab)
  );
}

function gapChatTranscriptText(tab) {
  const lines = String(tab?.output || "")
    .split(/\r?\n/)
    .filter((line) => !line.startsWith("[refine]"));
  if (!chatLinesIncludeAgentResponse(lines)) return "";
  return lines.join("\n").trim();
}

async function draftGapsFromPlan() {
  const t = chatState.tabs.plan;
  if (!t) return;
  const transcript = planTranscriptText(t);
  if (!planHasAgentResponse(t) || !transcript) {
    toast("Wait for the agent to respond before drafting Gaps.", "error");
    return;
  }
  if (typeof openPlanDraftModalFromText !== "function") {
    toast("Plan drafting is unavailable.", "error");
    return;
  }
  openPlanDraftModalFromText(transcript);
  minimizeToolbar();
}

async function extractRoundFromGapChat() {
  const tab = chatState.tabs[chatState.activeTabId];
  if (!tab || !tab.gapId) return;
  if (!tab.gapStatus) {
    await refreshGapChatStatus(tab.gapId, { redraw: false });
  }
  if (!GAP_CHAT_ROUND_STATUSES.has(tab.gapStatus || "")) {
    toast(
      `Cannot draft a round while the Gap is ${tab.gapStatus || "unknown"}.`,
      "error",
    );
    syncGapRoundExtractButton(tab);
    return;
  }
  const transcript = gapChatTranscriptText(tab);
  if (!transcript) {
    toast("Wait for the agent to respond before extracting a round.", "error");
    return;
  }
  if (typeof extractImportDrafts !== "function") {
    toast("Round extraction is unavailable.", "error");
    return;
  }
  if (!state.lastReporter) {
    toast("Pick a reporter in the top-right selector", "error");
    return;
  }
  openGapRoundExtractModal(tab.gapId, transcript);
  minimizeToolbar();
}

function openGapRoundExtractModal(gapId, transcript) {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         aria-labelledby="gap-round-extract-title">
      <div class="modal-title" id="gap-round-extract-title">Extract round</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review the extracted round before adding it to this Gap.
        </div>
        <div id="gap-round-extract-body"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button id="btn-add-extracted-round" disabled>Add round</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);
  let closed = false;
  const abort = new AbortController();
  function close() {
    if (closed) return;
    closed = true;
    abort.abort();
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  root.querySelector("[data-cancel]").addEventListener("click", close);
  const bodyRoot = root.querySelector("#gap-round-extract-body");
  const addButton = root.querySelector("#btn-add-extracted-round");
  loadExtractedRoundDraft({
    gapId,
    transcript,
    root,
    bodyRoot,
    addButton,
    close,
    signal: abort.signal,
  }).catch(async (e) => {
    if (e.name === "AbortError") return;
    if (bodyRoot) {
      bodyRoot.innerHTML = `<p class="muted" style="color:var(--error)">${htmlEscape(e.message || "Round extraction failed")}</p>`;
    }
  });
}

async function loadExtractedRoundDraft({ gapId, transcript, root, bodyRoot, addButton, close, signal }) {
  const drafts = await extractImportDrafts(transcript, bodyRoot, signal);
  if (signal.aborted) return;
  const draft = (drafts || []).find((item) => {
    return String(item?.actual || item?.target || "").trim();
  });
  if (!draft) {
    bodyRoot.innerHTML = `<p class="muted">No round draft extracted.</p>`;
    return;
  }
  const reporter = state.lastReporter || "";
  bodyRoot.innerHTML = `
    ${(drafts || []).length > 1
      ? `<p class="muted small">Using the first extracted draft from ${(drafts || []).length} candidates.</p>`
      : ""}
    <p class="muted small">Submitting as <strong>${htmlEscape(reporter)}</strong>. Change the Reporter in the top-right selector.</p>
    <form id="gap-round-extract-form" class="round-form">
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual">${htmlEscape(draft.actual || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target">${htmlEscape(draft.target || "")}</textarea>
      </div>
    </form>
  `;
  addButton.disabled = false;
  addButton.addEventListener("click", async () => {
    const form = root.querySelector("#gap-round-extract-form");
    if (!form) return;
    const fd = new FormData(form);
    const nextReporter = String(state.lastReporter || "").trim();
    const actual = String(fd.get("actual") || "").trim();
    const target = String(fd.get("target") || "").trim();
    if (!nextReporter) return toast("Pick a reporter in the top-right selector", "error");
    if (!actual && !target) return toast("Provide actual or target", "error");
    await withButtonBusy(addButton, "Adding…", async () => {
      try {
        await api("POST", `/api/gaps/${gapId}/rounds`, {
          reporter: nextReporter,
          actual,
          target,
        });
        const tab = chatState.tabs[gapId];
        if (tab) {
          tab.gapStatus = "todo";
          tab.output = `${tab.output || ""}\n[refine] Extracted this chat into a new Gap round.\n`;
          saveChatStateToStorage();
          drawChat();
        }
        toast("New round submitted", "info");
        close();
        if (state.currentGap === gapId && typeof loadGapDetail === "function") {
          await loadGapDetail(gapId);
        }
      } catch (err) {
        await showActionError(err, "Could not add extracted round");
      }
    });
  });
}

async function refreshGapChatStatus(gapId, { redraw = true } = {}) {
  const tab = chatState.tabs[gapId];
  if (!tab || tab.gapStatusLoading) return;
  tab.gapStatusLoading = true;
  try {
    const { gap } = await api("GET", "/api/gaps/" + encodeURIComponent(gapId));
    if (gap?.status) tab.gapStatus = gap.status;
  } catch {
    if (!tab.gapStatus) tab.gapStatus = "unknown";
  } finally {
    tab.gapStatusLoading = false;
    saveChatStateToStorage();
    if (redraw && chatState.activeTabId === gapId) drawToolbar();
  }
}

async function pollChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
  const sid = t.sessionId;
  try {
    const r = await api("GET", `/api/chat/${sid}/read`);
    if (!chatState.tabs[chatState.activeTabId]
        || chatState.tabs[chatState.activeTabId] !== t
        || t.sessionId !== sid) {
      return;
    }
    if (r.lines && r.lines.length) {
      markChatActivityPulse(t);
      if (chatLinesIncludeAgentResponse(r.lines)) {
        t.agentResponded = true;
      }
      t.output = (t.output || "") + r.lines.join("\n") + "\n";
      // Only update the DOM if this tab is still active.
      if (chatState.activeTabId in chatState.tabs &&
          chatState.tabs[chatState.activeTabId].sessionId === sid) {
        const out = $("#chat-output");
        if (out) {
          const atBottom = out.scrollHeight - out.scrollTop - out.clientHeight < 50;
          // Re-render the full transcript as markdown — incremental
          // append won't work since block elements (code fences, lists)
          // can span multiple chunks.
          out.innerHTML = mdToHtml(t.output || "");
          if (atBottom) out.scrollTop = out.scrollHeight;
        }
      }
      saveChatStateToStorage();
    }
    if (r.progress_lines && r.progress_lines.length) {
      markChatActivityPulse(t);
      t.progress = (t.progress || "") + r.progress_lines.join("\n") + "\n";
      if (chatState.activeTabId in chatState.tabs &&
          chatState.tabs[chatState.activeTabId].sessionId === sid) {
        const progress = $("#chat-progress");
        if (progress) {
          const atBottom = progress.scrollHeight - progress.scrollTop - progress.clientHeight < 50;
          progress.innerHTML = renderChatProgress(t.progress || "");
          if (atBottom) progress.scrollTop = progress.scrollHeight;
        }
      }
      saveChatStateToStorage();
    }
    // Pending state is authoritative from the runner: `in_flight` is true
    // while an agent CLI subprocess is running for this session.
    const wasPending = !!t.pending;
    t.pending = !!r.in_flight;
    applyPendingIndicator(t);
    syncChatActionButtons(t);
    if (wasPending !== t.pending) refreshProcessesTabForChatChange();
    if (r.alive === false) {
      t.closedReason = r.closed_reason || "session ended";
      t.sessionId = null;
      t.pending = false;
      saveChatStateToStorage();
      refreshProcessesTabForChatChange();
      drawChat();
    }
  } catch {
    // Tolerate transient errors; SSE/poller reconnects on its own.
  }
}

async function sendChatLine() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId || t.pending) return;
  const input = $("#chat-input");
  const text = input.value;
  if (!text.trim()) return;
  input.value = "";
  resizeChatInput(input);
  await sendChatText(text);
}

async function sendChatText(text) {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId || t.pending) return;
  text = String(text || "");
  if (!text.trim()) return;
  const quotedText = text.split(/\r?\n/).map((line) => `> ${line}`).join("\n");
  const echo = `\n${quotedText}\n`;
  t.output = (t.output || "") + echo;
  const out = $("#chat-output");
  if (out) {
    out.innerHTML = mdToHtml(t.output || "");
    out.scrollTop = out.scrollHeight;
  }
  // Optimistically flip into pending so the indicator appears immediately
  // (the next poll will confirm via `in_flight`).
  t.pending = true;
  applyPendingIndicator(t);
  saveChatStateToStorage();
  refreshProcessesTabForChatChange();
  try {
    await api("POST", `/api/chat/${t.sessionId}/input`, { text });
    // Trigger a poll right away so we pick up `in_flight` and (likely soon)
    // the response without waiting the full 800ms tick.
    pollChat();
  } catch (e) {
    t.pending = false;
    applyPendingIndicator(t);
    toast("Could not send: " + e.message, "error");
  }
}
