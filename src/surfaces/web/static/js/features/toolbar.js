// ---- Toolbar ----------------------------------------------------------------

// chatState holds one tab per chat: the permanent "standalone" tab plus one
// per Gap that the user opened via Open Chat. Each tab carries its own
// session id, accumulated output, and closed-reason. Only the active tab is
// polled; output for other tabs accumulates server-side in the runner's
// per-session deque until the user switches to that tab.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const FILES_TAB_ID = "files";
const SYSTEM_TAB_ID = "system";
const TERMINAL_TAB_ID = "terminal";
const STANDARD_TOOLBAR_TAB_ORDER = [SYSTEM_TAB_ID, FILES_TAB_ID, TERMINAL_TAB_ID, "standalone"];
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
const TERMINAL_OUTPUT_MAX_CHARS = 50_000;
const CHAT_ACTIVITY_PULSE_MS = 1800;
let filesSearchTimer = null;
let filesSearchRequestSeq = 0;
let filesSearchAbortController = null;
const chatState = {
  tabs: {},                // tabId → { gapId, label, sessionId, output, closedReason }
  activeTabId: "standalone",
  open: false,             // dock expanded?
  bodyHeight: null,        // user-resized body height in px; null → 20vh default
  fullscreen: false,       // when true, panel fills viewport below the topbar
  focusInputUntil: 0,
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
  searchSelectedPath: "",
  searchUserSelectedPath: "",
  searchLoading: false,
  searchError: "",
  loading: false,
  error: "",
};
const terminalState = {
  sessionId: "",
  cwd: "",
  display: "",
  term: null,
  cursor: 0,
  inputBuffer: "",
  inputFlushTimer: null,
  inputSendPromise: Promise.resolve(),
  lastSeq: 0,
  loading: false,
  connected: false,
  exited: false,
  error: "",
};
let terminalEventSource = null;

function ensureStandaloneTab() {
  if (!chatState.tabs.standalone) {
    chatState.tabs.standalone = {
      gapId: null, label: "Standalone", mode: "standalone",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, sentUserInput: false, progress: "", showProgress: true,
    };
  }
  ensureChatTabQueueState(chatState.tabs.standalone);
  ensureFilesTab();
  ensureSystemTab();
  ensureTerminalTab();
  reorderStandardToolbarTabs();
}

function ensureFilesTab() {
  if (!chatState.tabs[FILES_TAB_ID]) {
    chatState.tabs[FILES_TAB_ID] = {
      gapId: null, label: "Files", mode: "files",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, sentUserInput: false, progress: "", showProgress: true,
    };
  }
  ensureChatTabQueueState(chatState.tabs[FILES_TAB_ID]);
}

function ensureSystemTab() {
  if (!chatState.tabs[SYSTEM_TAB_ID]) {
    chatState.tabs[SYSTEM_TAB_ID] = {
      gapId: null, label: "System", mode: "system",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, sentUserInput: false, progress: "", showProgress: true,
    };
  }
  ensureChatTabQueueState(chatState.tabs[SYSTEM_TAB_ID]);
}

function ensureTerminalTab() {
  if (!chatState.tabs[TERMINAL_TAB_ID]) {
    chatState.tabs[TERMINAL_TAB_ID] = {
      gapId: null, label: "Terminal", mode: "terminal",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false, sentUserInput: false, progress: "", showProgress: true,
    };
  }
  ensureChatTabQueueState(chatState.tabs[TERMINAL_TAB_ID]);
}

function reorderStandardToolbarTabs() {
  const existing = chatState.tabs || {};
  for (const tab of Object.values(existing)) ensureChatTabQueueState(tab);
  const ordered = {};
  for (const id of STANDARD_TOOLBAR_TAB_ORDER) {
    if (existing[id]) ordered[id] = existing[id];
  }
  for (const [id, tab] of Object.entries(existing)) {
    if (!STANDARD_TOOLBAR_TAB_ORDER.includes(id)) ordered[id] = tab;
  }
  chatState.tabs = ordered;
}

function currentToolbarTab() {
  ensureStandaloneTab();
  let tab = chatState.tabs[chatState.activeTabId];
  if (!tab) {
    chatState.activeTabId = "standalone";
    tab = chatState.tabs.standalone;
  }
  return ensureChatTabQueueState(tab);
}

function currentChatTab() {
  const tab = currentToolbarTab();
  if (!tab || tab.mode === "files" || tab.mode === "system" || tab.mode === "terminal") return null;
  return tab;
}

function ensureChatTabQueueState(tab) {
  if (!tab) return tab;
  tab.queuedMessages = normalizeQueuedMessages(tab.queuedMessages);
  tab.localQueuedMessages = normalizeQueuedMessages(tab.localQueuedMessages);
  tab.starting = !!tab.starting;
  tab.sending = !!tab.sending;
  tab.sentUserInput = !!tab.sentUserInput;
  return tab;
}

function normalizeQueuedMessages(messages) {
  if (!Array.isArray(messages)) return [];
  return messages
    .map((message) => ({
      id: String(message?.id || newLocalQueuedMessageId()),
      text: String(message?.text || ""),
      created_at: String(message?.created_at || new Date().toISOString()),
      updated_at: String(message?.updated_at || message?.created_at || new Date().toISOString()),
      local: !!message?.local,
    }))
    .filter((message) => message.text.trim());
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
      if (Array.isArray(parsed.systemFilters)) {
        systemOperationState.filters = new Set(
          parsed.systemFilters.map((item) => String(item || "").trim()).filter(Boolean),
        );
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
        sentUserInput: !!t.sentUserInput,
        queuedMessages: normalizeQueuedMessages(t.queuedMessages),
        localQueuedMessages: normalizeQueuedMessages(t.localQueuedMessages),
        starting: !!t.starting,
    };
  }
  try {
    localStorage.setItem(CHAT_TABS_STORAGE_KEY, JSON.stringify({
      tabs, activeTabId: chatState.activeTabId,
      open: chatState.open, bodyHeight: chatState.bodyHeight,
      fullscreen: chatState.fullscreen,
      systemFilters: [...systemOperationState.filters],
    }));
  } catch {}
}

function defaultToolbarBodyHeight() {
  return Math.max(320, Math.round(window.innerHeight * 0.32));
}

function defaultChatBodyHeight() { return defaultToolbarBodyHeight(); }

function clampToolbarBodyHeight(px) {
  const min = 320;
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
  chatState.tabs = {};
  chatState.activeTabId = "standalone";
  chatState.open = false;
  chatState.bodyHeight = null;
  chatState.fullscreen = false;
  systemOperationState.filters.clear();
  ensureStandaloneTab();
  resetFilesState();
  resetTerminalState();
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
  filesState.searchSelectedPath = "";
  filesState.searchUserSelectedPath = "";
  filesState.searchLoading = false;
  filesState.searchError = "";
  filesState.loading = false;
  filesState.error = "";
}

function resetTerminalState() {
  if (terminalEventSource) {
    terminalEventSource.close();
    terminalEventSource = null;
  }
  terminalState.sessionId = "";
  terminalState.cwd = "";
  terminalState.display = "";
  if (terminalState.term) {
    terminalState.term.dispose();
    terminalState.term = null;
  }
  terminalState.cursor = 0;
  terminalState.inputBuffer = "";
  if (terminalState.inputFlushTimer) {
    clearTimeout(terminalState.inputFlushTimer);
    terminalState.inputFlushTimer = null;
  }
  terminalState.inputSendPromise = Promise.resolve();
  terminalState.lastSeq = 0;
  terminalState.loading = false;
  terminalState.connected = false;
  terminalState.exited = false;
  terminalState.error = "";
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
        closedReason: null, agentResponded: false, sentUserInput: false,
        queuedMessages: [], localQueuedMessages: [], starting: false,
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
      sentUserInput: false,
      queuedMessages: [],
      localQueuedMessages: [],
      starting: false,
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
  if (t && !t.sessionId) {
    startPlanChatSession(t);
  }
  if (initialPrompt.trim()) {
    queueChatTextForTab(t, initialPrompt);
    saveChatStateToStorage();
    drawToolbar();
    if (t?.sessionId) flushLocalQueuedMessages(t);
  }
}

async function startPlanChatSession(tab) {
  if (!tab || tab.starting || tab.sessionId) return;
  tab.starting = true;
  saveChatStateToStorage();
  applyPendingIndicator(tab);
  try {
    const r = await api("POST", "/api/chat/start", { purpose: "plan" });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    tab.mode = "plan";
    tab.progress = "";
    tab.showProgress = true;
    tab.starting = false;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    await flushLocalQueuedMessages(tab);
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  } catch (e) {
    tab.starting = false;
    saveChatStateToStorage();
    applyPendingIndicator(tab);
    toast("Could not start plan: " + e.message, "error");
  }
}

async function startGapChatSession(tab) {
  if (!tab || tab.starting || tab.sessionId) return;
  tab.starting = true;
  saveChatStateToStorage();
  applyPendingIndicator(tab);
  try {
    const r = await api("POST", "/api/chat/start", { gap_id: tab.gapId });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    tab.progress = "";
    tab.showProgress = true;
    tab.starting = false;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    await flushLocalQueuedMessages(tab);
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  } catch (e) {
    tab.starting = false;
    saveChatStateToStorage();
    applyPendingIndicator(tab);
    toast("Could not start chat: " + e.message, "error");
  }
}

async function startStandaloneChatSession(tab) {
  if (!tab || tab.starting || tab.sessionId) return;
  tab.starting = true;
  saveChatStateToStorage();
  applyPendingIndicator(tab);
  try {
    const r = await api("POST", "/api/chat/start", {});
    tab.sessionId = r.session_id;
    tab.worktree = r.worktree || null;
    tab.closedReason = null;
    tab.progress = "";
    tab.showProgress = true;
    tab.starting = false;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    await flushLocalQueuedMessages(tab);
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  } catch (e) {
    tab.starting = false;
    saveChatStateToStorage();
    applyPendingIndicator(tab);
    toast("Could not start chat: " + e.message, "error");
  }
}

async function ensureChatSession(tab) {
  if (!tab || tab.sessionId || tab.starting) return;
  if (tab.mode === "plan") await startPlanChatSession(tab);
  else if (tab.gapId) await startGapChatSession(tab);
  else await startStandaloneChatSession(tab);
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
  const active = currentToolbarTab();
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const filesActive = active.mode === "files";
  const systemActive = active.mode === "system";
  const terminalActive = active.mode === "terminal";
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

  const statusLine = chatStatusLine(active);

  root.classList.toggle("open", !!chatState.open);
  root.classList.toggle("fullscreen", !!chatState.fullscreen);
  if (chatState.open && !chatState.bodyHeight) {
    chatState.bodyHeight = defaultToolbarBodyHeight();
  }
  root.innerHTML = `
    <div class="toolbar-dock-resize" id="toolbar-dock-resize"
         role="separator" aria-orientation="horizontal"
         aria-label="Resize Toolbar"
         data-testid="toolbar-resize"
         title="Drag to resize"></div>
    <div class="toolbar-dock-bar" id="toolbar-dock-bar"
         data-testid="toolbar-bar"
         title="${chatState.open ? "Click to collapse" : "Click a tab to expand Toolbar"}">
      <span class="toolbar-dock-label">TOOLBAR</span>
      <div class="toolbar-tabs">
        ${Object.entries(tabs).map(([id, t]) => `
          <button class="toolbar-tab ${id === activeId ? "active" : ""}"
                  data-tab-id="${htmlEscape(id)}"
                  data-testid="toolbar-tab-${htmlEscape(id)}"
                  title="${htmlEscape(toolbarTabTitle(t))}">
            ${htmlEscape(t.label)}${t.sessionId ? ` <span class="toolbar-tab-dot" data-testid="toolbar-tab-dot" title="active session"></span>` : ""}
            ${id === "standalone" || id === FILES_TAB_ID || id === SYSTEM_TAB_ID || id === TERMINAL_TAB_ID ? "" : `<span class="toolbar-tab-close" data-close-tab="${htmlEscape(id)}" data-testid="toolbar-tab-close" title="Close tab">×</span>`}
          </button>`).join("")}
      </div>
      <button class="toolbar-dock-toggle toolbar-dock-fullscreen-btn${chatState.fullscreen ? " active" : ""}"
              id="btn-dock-fullscreen"
              data-testid="toolbar-fullscreen"
              aria-label="${chatState.fullscreen ? "Exit fullscreen Toolbar" : "Fullscreen Toolbar"}"
              aria-pressed="${chatState.fullscreen ? "true" : "false"}"
              title="${chatState.fullscreen ? "Exit fullscreen" : "Fullscreen"}">⛶</button>
      <button class="toolbar-dock-toggle toolbar-dock-collapse" id="btn-dock-toggle"
              data-testid="toolbar-collapse"
              aria-label="${chatState.open ? "Collapse Toolbar" : "Expand Toolbar"}"
              title="${chatState.open ? "Collapse Toolbar" : "Expand Toolbar"}">▾</button>
    </div>
    <div class="toolbar-dock-body${terminalActive ? " terminal-toolbar-body" : ""}"
         data-testid="toolbar-body"
         style="${chatState.bodyHeight ? `height:${chatState.bodyHeight}px` : ""}">
      ${filesActive
        ? renderFilesPanel()
        : systemActive
          ? renderSystemPanel()
          : terminalActive
            ? renderTerminalPanel()
            : renderChatPanel(active, {
                toggleClass,
                toggleLabel,
                statusLine,
                hasSession,
              })}
    </div>
  `;
  if (!filesActive && !systemActive && !terminalActive) applyPendingIndicator(active);
  if (filesActive) bindFilesPanel(root);
  if (systemActive) bindSystemPanel(root);
  if (terminalActive) bindTerminalPanel(root);

  if (chatState.open && !filesActive && !systemActive && !terminalActive) {
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
  if (!filesActive && !systemActive && !terminalActive) {
    $("#btn-chat-toggle")?.addEventListener("click", toggleActiveChat);
    $("#btn-plan-draft")?.addEventListener("click", draftGapsFromPlan);
    $("#btn-standalone-draft-gap")?.addEventListener("click", draftGapFromStandaloneChat);
    $("#btn-standalone-submit-merge")?.addEventListener("click", submitStandaloneChatForMerge);
    $("#btn-gap-round-extract")?.addEventListener("click", extractRoundFromGapChat);
    $("#btn-chat-clear")?.addEventListener("click", clearActiveChat);
    $("#chat-activity-toggle")?.addEventListener("click", toggleChatProgress);
    $("#chat-input")?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        sendChatLine();
      }
    });
    $("#btn-chat-send")?.addEventListener("click", sendChatLine);
    $("#chat-input")?.addEventListener("input", (e) => {
      resizeChatInput(e.currentTarget);
    });
    $$("[data-queued-message-save]", root).forEach((button) => {
      button.addEventListener("click", () => saveQueuedChatMessage(button.dataset.queuedMessageSave || ""));
    });
    $$("[data-queued-message-remove]", root).forEach((button) => {
      button.addEventListener("click", () => removeQueuedChatMessage(button.dataset.queuedMessageRemove || ""));
    });
    resizeChatInput($("#chat-input"));
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  }

  wireToolbarResize(root);
  if (filesActive && !filesState.entriesByPath[""] && !filesState.loading) {
    loadFilesDirectory("", { expand: true, redraw: true });
  }
  if (terminalActive && !terminalState.loading && !terminalState.sessionId && !terminalState.error) {
    startTerminalSession();
  }
  if (terminalActive) {
    focusTerminalSoon();
  }
}

function drawChatDock() { drawToolbar(); }

function toolbarTabTitle(tab) {
  if (tab.mode === "files") return "File browser";
  if (tab.mode === "system") return "System operations";
  if (tab.mode === "terminal") return "Terminal";
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
  const queuedMessages = allQueuedMessages(active);
  const standaloneWorktreePath = active.mode === "standalone" && active.worktree?.path
    ? String(active.worktree.path)
    : "";
  return `
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-chat-toggle" class="${toggleClass}" data-testid="chat-toggle">${htmlEscape(toggleLabel)}</button>
        ${active.mode === "plan" ? `
          <button id="btn-plan-draft" class="secondary" data-testid="plan-draft"
                  ${planHasAgentResponse(active) ? "" : "disabled"}>
            Draft Feature
          </button>` : ""}
        ${active.mode === "standalone" ? `
          <button id="btn-standalone-draft-gap" class="secondary" data-testid="standalone-draft-gap"
                  ${standaloneChatCanDraftGap(active) ? "" : "disabled"}>
            Draft Gap
          </button>
          <button id="btn-standalone-submit-merge" class="secondary" data-testid="standalone-submit-merge"
                  ${standaloneChatCanSubmitReadyMerge(active) ? "" : "disabled"}>
            Submit Gap
          </button>` : ""}
        ${active.gapId ? `
          <button id="btn-gap-round-extract" class="secondary" data-testid="gap-draft-round"
                  ${gapChatCanExtractRound(active) ? "" : "disabled"}>
            Draft Round
          </button>` : ""}
        <button id="btn-chat-clear" class="secondary" data-testid="chat-clear"
                ${(active.output || active.progress || active.sessionId || queuedMessages.length) ? "" : "disabled"}>
          Clear history
        </button>
        ${active.gapId ? `
          <a id="chat-gap-link" class="chat-gap-link"
             data-testid="chat-gap-link"
             href="#/gaps/${encodeURIComponent(active.gapId)}"
             title="Open Gap ${htmlEscape(active.gapId)}">
            Gap ${htmlEscape(active.gapId.slice(0, 10))}…
          </a>` : ""}
        <span class="spacer"></span>
        <span id="chat-status" class="muted small${chatActivityIsPulsing(active) ? " chat-status-working" : ""}" data-testid="chat-status">
          ${chatActivityIsPulsing(active) ? `
            <span class="chat-pending-dots chat-status-pending-dots" aria-hidden="true">
              <span></span><span></span><span></span>
            </span>` : ""}
          <span>${htmlEscape(statusLine)}</span>
        </span>
      </div>
      ${standaloneWorktreePath ? `
        <div class="muted small" style="margin:-4px 0 8px" data-testid="standalone-worktree-path">
          Worktree: <code>${htmlEscape(standaloneWorktreePath)}</code>
        </div>` : ""}
      <div class="chat-output-box">
        <div id="chat-output" class="chat-output" data-testid="chat-output">${mdToHtml(active.output || "")}</div>
        <button type="button"
                id="chat-activity-toggle"
                class="chat-activity-toggle"
                data-testid="chat-activity-toggle"
                aria-expanded="${showProgress ? "true" : "false"}"
                title="${htmlEscape(progressToggleLabel)}"
                ${hasActivityToggle ? "" : "hidden"}>
          <span id="chat-activity-label" data-testid="chat-activity-label">${htmlEscape(activityLabel)}</span>
          <span class="chat-activity-chevron" aria-hidden="true">
            ${toolbarIcon(showProgress ? "collapse" : "expand")}
          </span>
        </button>
        <div id="chat-progress-panel" class="chat-progress-panel" data-testid="chat-progress-panel" ${showActivityPanel ? "" : "hidden"}>
          <div id="chat-progress" class="chat-progress" data-testid="chat-progress">${renderChatProgress(progressText)}</div>
        </div>
      </div>
      ${renderQueuedChatMessages(queuedMessages)}
      <div class="actions" style="margin-top:8px">
        <div class="chat-input-wrap">
          <span id="chat-input-pending-dots"
                class="chat-pending-dots chat-input-pending-dots"
                ${showInputDots ? "" : "hidden"}>
            <span></span><span></span><span></span>
          </span>
          <textarea id="chat-input"
                    data-testid="chat-input"
                    class="${showInputDots ? "chat-input-waiting" : ""}"
                    rows="2"
                    placeholder="${htmlEscape(inputPlaceholder)}"></textarea>
        </div>
        <button id="btn-chat-send" class="primary" data-testid="chat-send" ${active.sending ? "disabled" : ""}>Send</button>
      </div>
    `;
}

function allQueuedMessages(tab) {
  return [
    ...normalizeQueuedMessages(tab?.localQueuedMessages).map((message) => ({ ...message, local: true })),
    ...normalizeQueuedMessages(tab?.queuedMessages).map((message) => ({ ...message, local: false })),
  ];
}

function renderQueuedChatMessages(messages) {
  if (!messages.length) return "";
  return `
    <div class="chat-queue" id="chat-queue" data-testid="chat-queue">
      <div class="chat-queue-header" data-testid="chat-queue-header">
        <span>Queued messages</span>
        <span class="muted small" data-testid="chat-queue-count">${messages.length}</span>
      </div>
      ${messages.map((message) => `
        <div class="chat-queue-item"
             data-testid="chat-queue-item"
             data-queued-message-id="${htmlEscape(message.id)}"
             data-queued-message-local="${message.local ? "1" : "0"}">
          <textarea class="chat-queue-edit" rows="2" data-queued-message-text data-testid="chat-queue-text">${htmlEscape(message.text)}</textarea>
          <div class="chat-queue-actions">
            <button class="secondary small" data-queued-message-save="${htmlEscape(message.id)}" data-testid="chat-queue-save">Save</button>
            <button class="danger small" data-queued-message-remove="${htmlEscape(message.id)}" data-testid="chat-queue-remove">Remove</button>
          </div>
        </div>`).join("")}
    </div>`;
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
    <div class="system-panel" data-testid="toolbar-system-panel">
      <div class="system-panel-header" data-testid="toolbar-system-header">
        <span>System operations</span>
        ${renderSystemLogFilters(messages, activeFilters)}
        <span class="muted small" data-testid="system-log-count">${countLabel}</span>
      </div>
      <div class="system-log" role="log" aria-live="polite" aria-label="Recent system operations"
           data-testid="system-log">
        ${visibleMessages.length
          ? visibleMessages.map(renderSystemLogLine).join("")
          : `<div class="system-log-empty" data-testid="system-log-empty">${activeFilters.size || messages.length ? "No system activity matches this filter." : "Waiting for system activity."}</div>`}
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
               data-testid="system-log-filter-all"
               ${!activeFilters.size ? "checked" : ""}
               aria-label="Show all system operations">
        <span>All</span>
      </label>
      ${options.map((option) => `
        <label class="system-log-filter system-log-filter-${option.status}${activeFilters.has(option.status) ? " active" : ""}">
          <input type="checkbox"
                 data-system-log-filter="${htmlEscape(option.status)}"
                 data-testid="system-log-filter-${htmlEscape(option.status)}"
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
      saveChatStateToStorage();
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
    <div class="system-log-line system-log-${item.status}" data-testid="system-log-line">
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

function renderTerminalPanel() {
  const status = terminalState.loading
    ? "Starting shell..."
    : terminalState.error
      ? terminalState.error
      : terminalState.exited
        ? "Shell exited."
        : terminalState.connected
          ? terminalState.cwd || "Shell active."
          : "Connecting...";
  return `
    <div class="terminal-panel" data-testid="toolbar-terminal-panel">
      <div class="terminal-titlebar">
        <span class="muted small" data-testid="terminal-status">${htmlEscape(status)}</span>
      </div>
      <div class="terminal-output"
           data-testid="terminal-output"
           tabindex="0"
           role="textbox"
           aria-label="Terminal"
           spellcheck="false"></div>
    </div>`;
}

function bindTerminalPanel(root) {
  const output = root.querySelector(".terminal-output");
  output?.addEventListener("focus", () => output.classList.add("focused"));
  output?.addEventListener("blur", () => output.classList.remove("focused"));
  ensureTerminalRenderer(output);
}

async function startTerminalSession() {
  terminalState.loading = true;
  terminalState.error = "";
  drawToolbar();
  try {
    const size = terminalSize();
    const result = await api("POST", "/api/terminal/session", size);
    terminalState.sessionId = result.id || "";
    terminalState.cwd = result.cwd || "";
    terminalState.connected = !!terminalState.sessionId;
    terminalState.exited = false;
    terminalState.lastSeq = 0;
    connectTerminalEvents();
    terminalState.loading = false;
    drawToolbar();
    focusTerminalSoon();
  } catch (e) {
    terminalState.loading = false;
    terminalState.error = e.message || String(e);
    drawToolbar();
  }
}

function connectTerminalEvents() {
  if (terminalEventSource) terminalEventSource.close();
  if (!terminalState.sessionId) return;
  terminalEventSource = new EventSource(`/api/terminal/${encodeURIComponent(terminalState.sessionId)}/events`);
  terminalEventSource.addEventListener("terminal_output", handleTerminalEvent);
  terminalEventSource.addEventListener("terminal_error", (event) => {
    handleTerminalEvent(event);
    terminalState.error = "Terminal stream error.";
    drawToolbar();
  });
  terminalEventSource.addEventListener("terminal_exit", (event) => {
    handleTerminalEvent(event);
    terminalState.exited = true;
    terminalState.connected = false;
    if (terminalEventSource) {
      terminalEventSource.close();
      terminalEventSource = null;
    }
    drawToolbar();
  });
  terminalEventSource.onerror = () => {
    if (!terminalState.exited) {
      terminalState.error = "Terminal connection lost.";
      drawToolbar();
    }
  };
}

function handleTerminalEvent(event) {
  try {
    const payload = JSON.parse(event.data || "{}");
    const seq = Number(payload.seq || 0);
    if (seq && seq <= terminalState.lastSeq) return;
    if (seq) terminalState.lastSeq = seq;
    terminalReceiveOutput(payload.data || "");
  } catch {
    terminalReceiveOutput(event.data || "");
  }
}

function handleTerminalKeydown(e) {
  if (!terminalState.sessionId || terminalState.exited) return;
  const data = terminalKeyData(e);
  if (data == null) return;
  e.preventDefault();
  queueTerminalInput(data);
}

function handleTerminalPaste(e) {
  if (!terminalState.sessionId || terminalState.exited) return;
  const text = e.clipboardData?.getData("text/plain") || "";
  if (!text) return;
  e.preventDefault();
  queueTerminalInput(text.replace(/\r?\n/g, "\r"));
}

function terminalKeyData(e) {
  if (e.ctrlKey && e.key && e.key.length === 1) {
    const code = e.key.toUpperCase().charCodeAt(0);
    if (code >= 64 && code <= 95) return String.fromCharCode(code - 64);
  }
  if (e.altKey || e.metaKey) return null;
  const special = {
    Enter: "\r",
    Backspace: "\x7f",
    Tab: "\t",
    Escape: "\x1b",
    ArrowUp: "\x1b[A",
    ArrowDown: "\x1b[B",
    ArrowRight: "\x1b[C",
    ArrowLeft: "\x1b[D",
    Home: "\x1b[H",
    End: "\x1b[F",
    Delete: "\x1b[3~",
    PageUp: "\x1b[5~",
    PageDown: "\x1b[6~",
  };
  if (special[e.key]) return special[e.key];
  if (e.key && e.key.length === 1) return e.key;
  return null;
}

function queueTerminalInput(data) {
  terminalState.inputBuffer += data;
  if (terminalState.inputFlushTimer) return;
  terminalState.inputFlushTimer = setTimeout(flushTerminalInput, 12);
}

function flushTerminalInput() {
  const data = terminalState.inputBuffer;
  terminalState.inputBuffer = "";
  terminalState.inputFlushTimer = null;
  if (!data || !terminalState.sessionId) return;
  const sessionId = terminalState.sessionId;
  terminalState.inputSendPromise = terminalState.inputSendPromise
    .catch(() => undefined)
    .then(async () => {
      try {
        await api("POST", `/api/terminal/${encodeURIComponent(sessionId)}/input`, { data });
      } catch (e) {
        terminalState.error = e.message || String(e);
        drawToolbar();
      }
    });
}

function terminalReceiveOutput(text) {
  if (terminalState.term) {
    terminalState.term.write(text || "");
  } else {
    terminalState.display = `${terminalState.display || ""}${text || ""}`;
  }
  scrollTerminalOutputToEnd();
}

function ensureTerminalRenderer(output) {
  if (!output || !window.Terminal) return;
  if (terminalState.term?.element && output.contains(terminalState.term.element)) return;
  if (terminalState.term) {
    terminalState.term.dispose();
    terminalState.term = null;
  }
  const size = terminalSize(output);
  const term = new window.Terminal({
    cols: size.cols,
    rows: size.rows,
    cursorBlink: true,
    convertEol: true,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: 12.5,
    lineHeight: 1.35,
    scrollback: 2000,
    theme: {
      background: "#fbfbf7",
      foreground: "#111827",
      cursor: "#111827",
      selectionBackground: "#dbeafe",
      black: "#111827",
      red: "#b91c1c",
      green: "#047857",
      yellow: "#a16207",
      blue: "#1d4ed8",
      magenta: "#7e22ce",
      cyan: "#0e7490",
      white: "#f8fafc",
      brightBlack: "#64748b",
      brightRed: "#dc2626",
      brightGreen: "#059669",
      brightYellow: "#ca8a04",
      brightBlue: "#2563eb",
      brightMagenta: "#9333ea",
      brightCyan: "#0891b2",
      brightWhite: "#ffffff",
    },
  });
  term.open(output);
  if (terminalState.display) term.write(terminalState.display);
  term.onData((data) => queueTerminalInput(data));
  terminalState.term = term;
}

function terminalApplyOutput(text) {
  for (let index = 0; index < text.length; index += 1) {
    const ch = text[index];
    if (ch === "\x1b") {
      const consumed = terminalApplyEscape(text.slice(index));
      if (consumed > 0) {
        index += consumed - 1;
        continue;
      }
    }
    if (ch === "\r") {
      terminalState.cursor = terminalLineStart();
    } else if (ch === "\n") {
      terminalInsert("\n");
    } else if (ch === "\b" || ch === "\x7f") {
      terminalBackspace();
    } else if (ch >= " " || ch === "\t") {
      terminalInsert(ch);
    }
  }
  if (terminalState.display.length > TERMINAL_OUTPUT_MAX_CHARS) {
    const excess = terminalState.display.length - TERMINAL_OUTPUT_MAX_CHARS;
    terminalState.display = terminalState.display.slice(excess);
    terminalState.cursor = Math.max(0, terminalState.cursor - excess);
  }
}

function terminalApplyEscape(text) {
  const match = /^\x1b\[(\??[0-9;]*)([A-Za-z~])/.exec(text);
  if (!match) return 0;
  const params = match[1] || "";
  const final = match[2];
  const first = parseInt(params.replace(/^\?/, "").split(";")[0] || "1", 10) || 1;
  if (final === "K") {
    const end = terminalLineEnd();
    terminalState.display = terminalState.display.slice(0, terminalState.cursor) + terminalState.display.slice(end);
  } else if (final === "G") {
    terminalState.cursor = Math.min(terminalLineStart() + Math.max(0, first - 1), terminalLineEnd());
  } else if (final === "C") {
    terminalState.cursor = Math.min(terminalState.cursor + first, terminalLineEnd());
  } else if (final === "D") {
    terminalState.cursor = Math.max(terminalState.cursor - first, terminalLineStart());
  }
  return match[0].length;
}

function terminalInsert(ch) {
  const before = terminalState.display.slice(0, terminalState.cursor);
  const after = terminalState.display.slice(terminalState.cursor);
  terminalState.display = before + ch + after;
  terminalState.cursor += ch.length;
}

function terminalBackspace() {
  const start = terminalLineStart();
  if (terminalState.cursor <= start) return;
  terminalState.display =
    terminalState.display.slice(0, terminalState.cursor - 1) +
    terminalState.display.slice(terminalState.cursor);
  terminalState.cursor -= 1;
}

function terminalLineStart() {
  return terminalState.display.lastIndexOf("\n", Math.max(0, terminalState.cursor - 1)) + 1;
}

function terminalLineEnd() {
  const next = terminalState.display.indexOf("\n", terminalState.cursor);
  return next === -1 ? terminalState.display.length : next;
}

function terminalSize(output = document.querySelector(".terminal-output")) {
  if (!output) return { cols: 100, rows: 30 };
  const styles = window.getComputedStyle(output);
  const fontSize = parseFloat(styles.fontSize) || 13;
  const lineHeight = parseFloat(styles.lineHeight) || fontSize * 1.4;
  return {
    cols: Math.max(20, Math.floor(output.clientWidth / (fontSize * 0.62))),
    rows: Math.max(8, Math.floor(output.clientHeight / lineHeight)),
  };
}

function focusTerminalSoon() {
  requestAnimationFrame(() => {
    const output = document.querySelector(".terminal-output");
    if (terminalState.term) {
      terminalState.term.focus();
    } else if (output && document.activeElement !== output) {
      output.focus({ preventScroll: true });
    }
  });
}

function scrollTerminalOutputToEnd() {
  if (terminalState.term && typeof terminalState.term.scrollToBottom === "function") {
    terminalState.term.scrollToBottom();
    return;
  }
  requestAnimationFrame(() => {
    const output = document.querySelector(".terminal-output");
    if (output) output.scrollTop = output.scrollHeight;
  });
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
    <div class="files-panel" data-testid="toolbar-files-panel">
      <div class="files-pathbar" data-testid="files-pathbar">
        <label for="files-path-input" class="files-path-label">Path</label>
        <input type="text" id="files-path-input"
               data-testid="files-path-input"
               autocomplete="off" spellcheck="false"
               placeholder="Repo-relative path"
               value="${htmlEscape(inputPath)}">
        <button type="button" class="secondary files-icon-btn"
                data-files-copy data-testid="files-copy-path" title="Copy path" aria-label="Copy path">
          ${toolbarIcon("copy")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-clear data-testid="files-clear-path" title="Clear path" aria-label="Clear path">
          ${toolbarIcon("clear")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-go data-testid="files-go-path" title="Go to path" aria-label="Go to path">
          ${toolbarIcon("go")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-refresh data-testid="files-refresh" title="Refresh" aria-label="Refresh">
          ${toolbarIcon("refresh")}
        </button>
      </div>
      <div class="files-browser" data-testid="files-browser">
        <div class="files-tree-panel" data-testid="files-tree-panel">
          <div class="files-tree-header">
            <span>Files</span>
            <div class="files-tree-actions">
              <button type="button" class="secondary files-icon-btn"
                      data-files-expand-all data-testid="files-expand-all" title="Expand all" aria-label="Expand all">
                ${toolbarIcon("expand")}
              </button>
              <button type="button" class="secondary files-icon-btn"
                      data-files-clear-tree data-testid="files-clear-tree" title="Clear tree" aria-label="Clear tree">
                ${toolbarIcon("clear")}
              </button>
              <button type="button" class="secondary files-icon-btn"
                      data-files-collapse-all data-testid="files-collapse-all" title="Collapse all" aria-label="Collapse all">
                ${toolbarIcon("collapse")}
              </button>
            </div>
          </div>
          <div class="files-tree-search" data-testid="files-search">
            <span class="files-tree-search-icon">${toolbarIcon("search")}</span>
            <input type="search" id="files-search-input"
                   data-testid="files-search-input"
                   data-files-selected-path="${htmlEscape(filesState.searchUserSelectedPath || filesState.searchSelectedPath || "")}"
                   autocomplete="off" spellcheck="false"
                   placeholder="Search files"
                   value="${htmlEscape(filesState.searchQuery || "")}">
          </div>
          <div class="files-tree" role="tree" aria-label="Directories and files"
               data-testid="files-tree">
            ${renderFilesTreePanel()}
          </div>
        </div>
        <div class="files-content" data-testid="files-content">
          <div class="files-content-header" data-testid="files-content-header">
            <span class="muted small" data-testid="files-status">${htmlEscape(status)}</span>
            ${filesState.file?.previewable ? `
              <button type="button" class="secondary files-icon-btn"
                      data-files-copy-content
                      data-testid="files-copy-content"
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
         data-testid="files-search-result"
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
           data-testid="files-tree-row"
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
    return `<div class="files-message" data-testid="files-message">Loading...</div>`;
  }
  if (filesState.error && !file) {
    return `<div class="files-message" data-testid="files-message">${htmlEscape(filesState.error)}</div>`;
  }
  if (!file) {
    return `<div class="files-message" data-testid="files-message">Choose a file from the tree or enter a path.</div>`;
  }
  if (!file.previewable) {
    return `<div class="files-message" data-testid="files-message">${htmlEscape(file.reason || "Preview is not available.")}</div>`;
  }
  if (file.kind === "image") {
    return `
      <div class="files-image-preview" data-testid="files-image-preview">
        <img src="${htmlEscape(file.data_url || "")}" alt="${htmlEscape(file.name || file.path || "Image preview")}">
      </div>`;
  }
  return `
    <div class="files-source" data-testid="files-source" data-language="${htmlEscape(languageForPath(file.path))}">
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
    <div class="files-source-line" data-testid="files-source-line">
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
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
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
    const selectedPath = e.currentTarget?.dataset?.filesSelectedPath || "";
    if (selectedPath && openFilesSearchPath(selectedPath)) return;
    if (filesState.searchResults && openSelectedFilesSearchResult()) return;
    if (filesState.searchLoading) return;
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
        filesState.searchSelectedPath = path;
        filesState.searchUserSelectedPath = path;
      }
      if (type === "directory") {
        if (row.dataset.filesSearchResult === "1") {
          filesState.searchQuery = "";
          filesState.searchResults = null;
          filesState.searchSelectedIndex = -1;
          filesState.searchSelectedPath = "";
          filesState.searchUserSelectedPath = "";
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
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
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
  const entries = results?.entries || [];
  const fileIndexes = filesSearchFileIndexes(results);
  if (!fileIndexes.length) {
    filesState.searchSelectedIndex = -1;
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
    return -1;
  }
  if (filesState.searchSelectedPath) {
    const selectedPathIndex = entries.findIndex((entry) =>
      entry.type === "file" && entry.path === filesState.searchSelectedPath
    );
    if (selectedPathIndex >= 0) {
      filesState.searchSelectedIndex = selectedPathIndex;
      return selectedPathIndex;
    }
  }
  if (fileIndexes.includes(filesState.searchSelectedIndex)) {
    filesState.searchSelectedPath = entries[filesState.searchSelectedIndex]?.path || "";
    return filesState.searchSelectedIndex;
  }
  filesState.searchSelectedIndex = fileIndexes[0];
  filesState.searchSelectedPath = entries[filesState.searchSelectedIndex]?.path || "";
  return filesState.searchSelectedIndex;
}

function selectedFilesSearchEntry() {
  if (filesState.searchUserSelectedPath) {
    const userSelected = filesState.searchResults?.entries?.find((entry) =>
      entry.type === "file" && entry.path === filesState.searchUserSelectedPath
    );
    if (userSelected) return userSelected;
  }
  const selectedRow = document.querySelector('.files-search-result[aria-selected="true"]');
  const selectedPath = selectedRow?.dataset.filesPath || "";
  if (selectedPath) {
    const selectedType = selectedRow?.dataset.filesType || "";
    filesState.searchSelectedPath = selectedPath;
    return filesState.searchResults?.entries?.find((entry) => entry.path === selectedPath) || {
      path: selectedPath,
      type: selectedType || "file",
    };
  }
  if (filesState.searchSelectedPath) {
    const selectedByPath = filesState.searchResults?.entries?.find((entry) =>
      entry.type === "file" && entry.path === filesState.searchSelectedPath
    );
    if (selectedByPath) return selectedByPath;
  }
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
  filesState.searchSelectedPath = filesState.searchResults?.entries?.[filesState.searchSelectedIndex]?.path || "";
  filesState.searchUserSelectedPath = filesState.searchSelectedPath;
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

function openFilesSearchPath(path) {
  const entry = filesState.searchResults?.entries?.find((candidate) =>
    candidate.type === "file" && candidate.path === path
  );
  if (!entry) return false;
  filesState.searchSelectedPath = entry.path;
  filesState.searchUserSelectedPath = entry.path;
  loadFile(entry.path);
  return true;
}

function scrollSelectedFilesSearchResultIntoView() {
  requestAnimationFrame(() => {
    const row = document.querySelector('.files-search-result[aria-selected="true"]');
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
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
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
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
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
  input.dataset.filesSelectedPath = filesState.searchUserSelectedPath || filesState.searchSelectedPath || "";
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
  filesState.searchSelectedPath = "";
  filesState.searchUserSelectedPath = "";
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
    filesState.searchSelectedPath = "";
    filesState.searchUserSelectedPath = "";
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
  const send = $("#btn-chat-send");
  const status = $("#chat-status");
  if (toggle) {
    toggle.hidden = !tab || !(tab.sessionId || tab.progress);
    toggle.setAttribute("aria-expanded", tab?.showProgress === false ? "false" : "true");
    toggle.title = tab?.showProgress === false ? "Expand activity" : "Collapse activity";
  }
  if (dots) dots.hidden = !chatActivityIsPulsing(tab);
  if (label) label.textContent = chatActivityLabel(tab);
  if (input) {
    input.disabled = !tab;
    input.placeholder = chatInputPlaceholder(tab);
    input.classList.toggle("chat-input-waiting", chatActivityIsPulsing(tab));
  }
  if (send) send.disabled = !tab || !!tab.sending;
  if (status && tab) {
    status.classList.toggle("chat-status-working", chatActivityIsPulsing(tab));
    status.innerHTML = `${chatActivityIsPulsing(tab) ? `
      <span class="chat-pending-dots chat-status-pending-dots" aria-hidden="true">
        <span></span><span></span><span></span>
      </span>` : ""}<span>${htmlEscape(chatStatusLine(tab))}</span>`;
  }
  syncChatActionButtons(tab);
}

function markChatActivityPulse(tab) {
  if (!tab) return;
  tab.activityPulseUntil = Date.now() + CHAT_ACTIVITY_PULSE_MS;
}

function chatActivityIsPulsing(tab) {
  return !!(tab?.pending || tab?.starting);
}

function chatActivityLabel(tab) {
  if (tab?.starting) return "Starting session";
  if (tab?.pending) return "Agent working";
  return "Activity panel";
}

function chatStatusLine(tab) {
  if (!tab?.sessionId) {
    if (tab?.starting) return "Starting session.";
    return "No active session.";
  }
  if (tab.closedReason) return `Session ${tab.sessionId} ended — ${tab.closedReason}.`;
  if (tab.pending) return `Agent working in session ${tab.sessionId}.`;
  if (tab.sending) return `Sending message to session ${tab.sessionId}.`;
  return `Session ${tab.sessionId} active.`;
}

function chatInputPlaceholder(tab) {
  if (!tab?.sessionId && tab?.starting) return "Starting session. Type to queue messages.";
  if (!tab?.sessionId) return "Type to queue a message and start the session.";
  if (tab.pending) return "Agent is busy. Press Enter to queue another message.";
  return "Type and press Enter.";
}

function syncChatActionButtons(tab) {
  syncPlanDraftButton(tab);
  syncStandaloneDraftGapButton(tab);
  syncStandaloneSubmitMergeButton(tab);
  syncGapRoundExtractButton(tab);
}

function syncPlanDraftButton(tab) {
  const btn = $("#btn-plan-draft");
  if (!btn || !tab || tab.mode !== "plan") return;
  btn.disabled = !planHasAgentResponse(tab);
}

function syncStandaloneDraftGapButton(tab) {
  const btn = $("#btn-standalone-draft-gap");
  if (!btn || !tab || tab.mode !== "standalone") return;
  btn.disabled = !standaloneChatCanDraftGap(tab);
}

function syncStandaloneSubmitMergeButton(tab) {
  const btn = $("#btn-standalone-submit-merge");
  if (!btn || !tab || tab.mode !== "standalone") return;
  btn.disabled = !standaloneChatCanSubmitReadyMerge(tab);
}

function syncGapRoundExtractButton(tab) {
  const btn = $("#btn-gap-round-extract");
  if (!btn || !tab || !tab.gapId) return;
  btn.disabled = !gapChatCanExtractRound(tab);
}

function handleChatSseEvent(payload) {
  const sessionId = String(payload?.session_id || "");
  if (!sessionId) return;
  const tab = Object.values(chatState.tabs)
    .find((candidate) => candidate?.sessionId === sessionId);
  if (!tab) return;
  const event = payload?.event && typeof payload.event === "object" ? payload.event : {};
  const eventId = String(event.id || "");
  if (eventId) {
    tab.seenSseEventIds = Array.isArray(tab.seenSseEventIds) ? tab.seenSseEventIds : [];
    if (tab.seenSseEventIds.includes(eventId)) return;
    tab.seenSseEventIds.push(eventId);
    tab.seenSseEventIds = tab.seenSseEventIds.slice(-200);
  }

  const wasPending = !!tab.pending;
  const payloadInFlight = payload.in_flight === true;
  if (payloadInFlight) {
    tab.pending = true;
  } else if (chatSseEventCanClearPending(event)) {
    tab.pending = false;
  }
  if (payload.closed === true) {
    tab.closedReason = event.text || "session ended";
    tab.sessionId = null;
    tab.pending = false;
  }

  let changed = false;
  if (event.progress === true) {
    changed = appendChatProgressLines(tab, [event.text]) || changed;
  } else {
    const line = chatLineFromSseEvent(event);
    if (line) {
      if (event.role === "assistant") tab.agentResponded = true;
      if (event.role === "user") {
        tab.sentUserInput = true;
        tab.queuedMessages = [];
        tab.localQueuedMessages = [];
      }
      changed = appendChatOutputLines(tab, [line]) || changed;
    }
  }

  if (changed) markChatActivityPulse(tab);
  saveChatStateToStorage();
  if (wasPending !== tab.pending) refreshProcessesTabForChatChange();
  if (chatState.tabs[chatState.activeTabId] === tab) {
    if (payload.closed === true || event.role === "user") {
      drawToolbar();
    } else {
      renderActiveChatTranscript(tab);
      applyPendingIndicator(tab);
      syncChatActionButtons(tab);
    }
  }
}

function chatSseEventCanClearPending(event) {
  const role = String(event?.role || "");
  return role === "assistant" || role === "system";
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
  const t = currentChatTab();
  if (!t) return;
  if (!t.output && !t.progress && !t.sessionId && !allQueuedMessages(t).length) return;
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
    t.worktree = null;
    t.output = "";
    t.progress = "";
    t.showProgress = true;
    t.closedReason = null;
    t.pending = false;
    t.queuedMessages = [];
    t.localQueuedMessages = [];
    t.starting = false;
    t.agentResponded = false;
    t.sentUserInput = false;
    saveChatStateToStorage();
    drawChat();
  });
}

async function toggleActiveChat() {
  const t = currentChatTab();
  if (!t) return;
  const btn = $("#btn-chat-toggle");
  if (t.sessionId) {
    await withButtonBusy(btn, "Stopping…", async () => {
      try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
      t.sessionId = null;
      t.worktree = null;
      t.closedReason = "stopped by user";
      saveChatStateToStorage();
      refreshProcessesTabForChatChange();
      drawChat();
    });
    return;
  }
  await withButtonBusy(btn, "Starting…", async () => {
    await ensureChatSession(t);
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
    .filter((line) => !line.startsWith("[refine]") && !line.trim().startsWith(">"))
    .join("\n")
    .trim();
}

function standaloneChatTranscriptText(tab) {
  const lines = String(tab?.output || "")
    .split(/\r?\n/)
    .filter((line) => !line.startsWith("[refine]"));
  if (!chatLinesIncludeAgentResponse(lines)) return "";
  return lines.join("\n").trim();
}

function chatLinesIncludeAgentResponse(lines) {
  return (lines || []).some((line) => {
    const text = String(line || "").trim();
    return text && !text.startsWith("[refine]");
  });
}

function planHasAgentResponse(tab) {
  if (!tab) return false;
  if (tab.mode === "plan" && !tab.sentUserInput) return false;
  if (tab.agentResponded) return true;
  return (tab.output || "")
    .split(/\r?\n/)
    .some((line) => {
      const text = line.trim();
      return text && !text.startsWith("[refine]") && !text.startsWith(">");
    });
}

function standaloneChatCanDraftGap(tab) {
  return !!(
    tab
    && tab.mode === "standalone"
    && !tab.pending
    && standaloneChatTranscriptText(tab)
  );
}

function standaloneChatCanSubmitReadyMerge(tab) {
  return !!(
    tab
    && tab.mode === "standalone"
    && tab.sessionId
    && tab.worktree?.path
    && !tab.pending
  );
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
    toast("Wait for the agent to respond before drafting a Feature.", "error");
    return;
  }
  if (
    typeof extractPlanDraftsInBackground !== "function" ||
    typeof openPlanDraftModalFromResult !== "function"
  ) {
    toast("Plan drafting is unavailable.", "error");
    return;
  }
  toast("Extracting Plan Feature and Gaps in the background.", "info");
  recordUiNotice("Plan Draft extraction started", {
    kind: "info",
    source: "background-operation",
  });
  minimizeToolbar();
  try {
    const result = await extractPlanDraftsInBackground(transcript);
    await openPlanDraftModalFromResult(transcript, result);
  } catch (error) {
    await showActionError(error, "Plan Draft extraction failed");
  }
}

async function draftGapFromStandaloneChat() {
  const t = chatState.tabs.standalone;
  if (!t) return;
  const transcript = standaloneChatTranscriptText(t);
  if (!transcript) {
    toast("Wait for the agent to respond before drafting a Gap.", "error");
    return;
  }
  if (!state.lastReporter) {
    toast("Pick a reporter in the top-right selector", "error");
    return;
  }
  if (typeof extractImportDrafts !== "function") {
    toast("Gap drafting is unavailable.", "error");
    return;
  }
  openStandaloneGapDraftModalFromText(transcript);
  minimizeToolbar();
}

async function submitStandaloneChatForMerge() {
  const t = chatState.tabs.standalone;
  if (!standaloneChatCanSubmitReadyMerge(t)) {
    toast("Start a standalone session and wait for active work to finish.", "error");
    return;
  }
  if (!state.lastReporter) {
    toast("Pick a reporter in the top-right selector", "error");
    return;
  }
  openStandaloneReadyMergeModal(t);
  minimizeToolbar();
}

function openStandaloneGapDraftModalFromText(transcript) {
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         data-testid="standalone-gap-draft-modal"
         aria-labelledby="standalone-gap-draft-title">
      <div class="modal-title" id="standalone-gap-draft-title">Draft Gap</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review the drafted Gap before saving it as standalone work.
        </div>
        <div id="standalone-gap-draft-body" data-testid="standalone-gap-draft-body"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="standalone-gap-draft-cancel">Cancel</button>
        <button id="btn-save-standalone-gap-draft" data-testid="standalone-gap-draft-submit" disabled>Create Gap</button>
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
  const bodyRoot = root.querySelector("#standalone-gap-draft-body");
  const saveButton = root.querySelector("#btn-save-standalone-gap-draft");
  loadStandaloneGapDraft({
    transcript,
    root,
    bodyRoot,
    saveButton,
    close,
    signal: abort.signal,
  }).catch((e) => {
    if (e.name === "AbortError") return;
    if (bodyRoot) {
      bodyRoot.innerHTML = `<p class="muted" style="color:var(--error)">${htmlEscape(e.message || "Gap drafting failed")}</p>`;
    }
  });
}

async function loadStandaloneGapDraft({ transcript, root, bodyRoot, saveButton, close, signal }) {
  const drafts = await extractImportDrafts(transcript, bodyRoot, signal, { purpose: "standalone_gap" });
  if (signal.aborted) return;
  const draft = (drafts || []).find((item) => {
    return String(item?.actual || item?.target || item?.name || "").trim();
  });
  if (!draft) {
    bodyRoot.innerHTML = `<p class="muted">No Gap draft extracted.</p>`;
    return;
  }
  const reporter = state.lastReporter || draft.reporter || "";
  bodyRoot.innerHTML = `
    ${(drafts || []).length > 1
      ? `<p class="muted small">Using the first extracted draft from ${(drafts || []).length} candidates.</p>`
      : ""}
    <p class="muted small">Submitting as <strong>${htmlEscape(reporter)}</strong>. Change the Reporter in the top-right selector.</p>
    <form id="standalone-gap-draft-form" class="round-form">
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual" data-testid="standalone-gap-draft-actual">${htmlEscape(draft.actual || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" data-testid="standalone-gap-draft-target">${htmlEscape(draft.target || draft.name || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Priority</label>
        <select name="priority" data-testid="standalone-gap-draft-priority">
          ${["low", "medium", "high"].map((priority) => `
            <option value="${priority}" ${(draft.priority || "low") === priority ? "selected" : ""}>${priority[0].toUpperCase()}${priority.slice(1)}</option>
          `).join("")}
        </select>
      </div>
    </form>
  `;
  saveButton.disabled = false;
  saveButton.addEventListener("click", async () => {
    const form = root.querySelector("#standalone-gap-draft-form");
    if (!form) return;
    const fd = new FormData(form);
    const nextReporter = String(state.lastReporter || reporter || "").trim();
    const actual = String(fd.get("actual") || "").trim();
    const target = String(fd.get("target") || "").trim();
    const priority = String(fd.get("priority") || "low").trim() || "low";
    if (!nextReporter) return toast("Pick a reporter in the top-right selector", "error");
    if (!actual && !target) return toast("Provide actual or target", "error");
    await withButtonBusy(saveButton, "Creating…", async () => {
      try {
        const r = await api("POST", "/api/gaps", {
          reporter: nextReporter,
          actual,
          target,
          priority,
        });
        const gapId = r?.gap?.id || "";
        const tab = chatState.tabs.standalone;
        if (tab) {
          tab.output = `${tab.output || ""}\n[refine] Drafted this standalone chat into Gap ${gapId || "new"}.\n`;
          saveChatStateToStorage();
          drawChat();
        }
        toast("Gap created", "info");
        close();
        if (gapId) location.hash = "#/gaps/" + encodeURIComponent(gapId);
      } catch (err) {
        await showActionError(err, "Could not create drafted Gap");
      }
    });
  });
}

function openStandaloneReadyMergeModal(tab) {
  const transcript = standaloneChatTranscriptText(tab);
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal import-modal" role="dialog" aria-modal="true"
         data-testid="standalone-ready-merge-modal"
         aria-labelledby="standalone-ready-merge-title">
      <div class="modal-title" id="standalone-ready-merge-title">Submit Gap</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review the Gap details before handing the standalone worktree to the merge workflow.
        </div>
        <div class="muted small" style="margin-bottom:8px">
          Worktree: <code data-testid="standalone-ready-merge-worktree">${htmlEscape(tab.worktree?.path || "")}</code>
        </div>
        <div id="standalone-ready-merge-body" data-testid="standalone-ready-merge-body"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="standalone-ready-merge-cancel">Cancel</button>
        <button id="btn-submit-standalone-ready-merge" data-testid="standalone-ready-merge-submit" disabled>Submit</button>
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
  const bodyRoot = root.querySelector("#standalone-ready-merge-body");
  const submitButton = root.querySelector("#btn-submit-standalone-ready-merge");
  loadStandaloneReadyMergeDraft({
    tab,
    transcript,
    root,
    bodyRoot,
    submitButton,
    close,
    signal: abort.signal,
  }).catch((e) => {
    if (e.name === "AbortError") return;
    renderStandaloneReadyMergeForm({ draft: {}, tab, root, bodyRoot, submitButton, close });
  });
}

async function loadStandaloneReadyMergeDraft({ tab, transcript, root, bodyRoot, submitButton, close, signal }) {
  let draft = {};
  if (transcript && typeof extractImportDrafts === "function") {
    const drafts = await extractImportDrafts(transcript, bodyRoot, signal, { purpose: "standalone_gap" });
    if (signal.aborted) return;
    draft = (drafts || []).find((item) => {
      return String(item?.actual || item?.target || item?.name || "").trim();
    }) || {};
  }
  renderStandaloneReadyMergeForm({ draft, tab, root, bodyRoot, submitButton, close });
}

function renderStandaloneReadyMergeForm({ draft, tab, root, bodyRoot, submitButton, close }) {
  const reporter = state.lastReporter || draft.reporter || "";
  bodyRoot.innerHTML = `
    <p class="muted small">Submitting as <strong>${htmlEscape(reporter)}</strong>. Change the Reporter in the top-right selector.</p>
    <form id="standalone-ready-merge-form" class="round-form">
      <div class="form-row">
        <label>Actual (current behavior)</label>
        <textarea name="actual" data-testid="standalone-ready-merge-actual">${htmlEscape(draft.actual || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" data-testid="standalone-ready-merge-target">${htmlEscape(draft.target || draft.name || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Priority</label>
        <select name="priority" data-testid="standalone-ready-merge-priority">
          ${["low", "medium", "high"].map((priority) => `
            <option value="${priority}" ${(draft.priority || "low") === priority ? "selected" : ""}>${priority[0].toUpperCase()}${priority.slice(1)}</option>
          `).join("")}
        </select>
      </div>
    </form>
  `;
  submitButton.disabled = false;
  submitButton.addEventListener("click", async () => {
    const form = root.querySelector("#standalone-ready-merge-form");
    if (!form) return;
    const fd = new FormData(form);
    const nextReporter = String(state.lastReporter || reporter || "").trim();
    const actual = String(fd.get("actual") || "").trim();
    const target = String(fd.get("target") || "").trim();
    const priority = String(fd.get("priority") || "low").trim() || "low";
    if (!nextReporter) return toast("Pick a reporter in the top-right selector", "error");
    if (!actual || !target) return toast("Provide actual and target", "error");
    await withButtonBusy(submitButton, "Submitting…", async () => {
      try {
        const r = await api("POST", `/api/chat/${tab.sessionId}/submit-ready-merge`, {
          reporter: nextReporter,
          actual,
          target,
          priority,
        });
        const gapId = r?.gap?.id || "";
        tab.sessionId = null;
        tab.pending = false;
        tab.closedReason = "submitted for ready-merge";
        tab.worktree = r?.worktree || tab.worktree || null;
        if (tab.worktree && gapId) tab.worktree.submitted_gap_id = gapId;
        tab.output = `${tab.output || ""}\n[refine] Submitted standalone worktree as ready-merge Gap ${gapId || "new"}.\n`;
        saveChatStateToStorage();
        refreshProcessesTabForChatChange();
        drawChat();
        toast("Gap submitted for merge", "info");
        close();
        if (gapId) location.hash = "#/gaps/" + encodeURIComponent(gapId);
      } catch (err) {
        await showActionError(err, "Could not submit standalone worktree");
      }
    });
  });
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
         data-testid="gap-round-extract-modal"
         aria-labelledby="gap-round-extract-title">
      <div class="modal-title" id="gap-round-extract-title">Extract round</div>
      <div class="modal-body" style="max-height:72vh;overflow:auto">
        <div class="muted small" style="margin-bottom:8px">
          Review the extracted round before adding it to this Gap.
        </div>
        <div id="gap-round-extract-body" data-testid="gap-round-extract-body"></div>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="gap-round-extract-cancel">Cancel</button>
        <button id="btn-add-extracted-round" data-testid="gap-round-extract-submit" disabled>Add round</button>
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
  const drafts = await extractImportDrafts(transcript, bodyRoot, signal, { purpose: "round" });
  if (signal.aborted) return;
  const draft = (drafts || []).find((item) => {
    return String(item?.actual || "").trim() && String(item?.target || "").trim();
  }) || (drafts || []).find((item) => {
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
        <textarea name="actual" data-testid="gap-round-extract-actual">${htmlEscape(draft.actual || "")}</textarea>
      </div>
      <div class="form-row">
        <label>Target (desired behavior)</label>
        <textarea name="target" data-testid="gap-round-extract-target">${htmlEscape(draft.target || "")}</textarea>
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

function appendChatOutputLines(tab, lines) {
  if (!tab || !Array.isArray(lines) || !lines.length) return false;
  const now = Date.now();
  const recent = Array.isArray(tab.recentOutputLines)
    ? tab.recentOutputLines.filter((entry) => now - Number(entry?.at || 0) < 5000)
    : [];
  const existingOutputLines = new Set(
    String(tab.output || "")
      .split(/\n/)
      .filter((line) => line.trim())
      .slice(-100),
  );
  const next = [];
  for (const line of lines) {
    const text = String(line ?? "");
    if (text.trim() && existingOutputLines.has(text)) continue;
    if (recent.some((entry) => entry?.text === text)) continue;
    next.push(text);
    recent.push({ text, at: now });
    if (text.trim()) existingOutputLines.add(text);
  }
  tab.recentOutputLines = recent.slice(-100);
  if (!next.length) return false;
  tab.output = (tab.output || "") + next.join("\n") + "\n";
  return true;
}

function appendChatProgressLines(tab, lines) {
  if (!tab || !Array.isArray(lines) || !lines.length) return false;
  const existingProgressLines = new Set(
    String(tab.progress || "")
      .split(/\n/)
      .filter((line) => line.trim())
      .slice(-100),
  );
  const next = [];
  for (const line of lines) {
    const text = String(line ?? "");
    if (!text.trim()) continue;
    if (existingProgressLines.has(text)) continue;
    next.push(text);
    existingProgressLines.add(text);
  }
  if (!next.length) return false;
  tab.progress = (tab.progress || "") + next.join("\n") + "\n";
  return true;
}

function chatLineFromSseEvent(event) {
  const text = String(event?.text || "");
  if (!text.trim()) return "";
  const role = String(event?.role || "");
  if (role === "user") return `> ${text}`;
  if (role === "assistant" || role === "system") return text;
  return text;
}

function renderActiveChatTranscript(tab) {
  if (!tab || chatState.tabs[chatState.activeTabId] !== tab) return;
  const out = $("#chat-output");
  if (out) {
    const atBottom = out.scrollHeight - out.scrollTop - out.clientHeight < 50;
    out.innerHTML = mdToHtml(tab.output || "");
    if (atBottom) out.scrollTop = out.scrollHeight;
  }
  const progress = $("#chat-progress");
  if (progress) {
    const atBottom = progress.scrollHeight - progress.scrollTop - progress.clientHeight < 50;
    progress.innerHTML = renderChatProgress(tab.progress || "");
    if (atBottom) progress.scrollTop = progress.scrollHeight;
  }
}

async function sendChatLine() {
  const t = currentChatTab();
  if (!t) return;
  if (t.sending) return;
  const input = $("#chat-input");
  const text = input.value;
  if (!text.trim()) return;
  input.value = "";
  resizeChatInput(input);
  requestChatInputFocus();
  await sendChatText(text, t);
  requestChatInputFocus();
}

async function saveQueuedChatMessage(messageId) {
  const tab = currentChatTab();
  if (!tab || !messageId) return;
  const row = document.querySelector(`[data-queued-message-id="${cssEscape(messageId)}"]`);
  const text = row?.querySelector("[data-queued-message-text]")?.value || "";
  if (!text.trim()) return toast("Queued message cannot be empty.", "error");
  const isLocal = row?.dataset.queuedMessageLocal === "1";
  if (isLocal || !tab.sessionId) {
    const message = normalizeQueuedMessages(tab.localQueuedMessages)
      .find((item) => item.id === messageId);
    if (!message) return;
    message.text = text.trim();
    message.updated_at = new Date().toISOString();
    message.local = true;
    tab.localQueuedMessages = normalizeQueuedMessages(tab.localQueuedMessages)
      .map((item) => item.id === messageId ? message : item);
    saveChatStateToStorage();
    drawToolbar();
    return;
  }
  try {
    const r = await api("PATCH", `/api/chat/${tab.sessionId}/queue/${encodeURIComponent(messageId)}`, {
      text: text.trim(),
    });
    tab.queuedMessages = normalizeQueuedMessages(r.queued_messages);
    saveChatStateToStorage();
    drawToolbar();
  } catch (e) {
    toast("Could not update queued message: " + e.message, "error");
  }
}

async function removeQueuedChatMessage(messageId) {
  const tab = currentChatTab();
  if (!tab || !messageId) return;
  const row = document.querySelector(`[data-queued-message-id="${cssEscape(messageId)}"]`);
  const isLocal = row?.dataset.queuedMessageLocal === "1";
  if (isLocal || !tab.sessionId) {
    tab.localQueuedMessages = normalizeQueuedMessages(tab.localQueuedMessages)
      .filter((item) => item.id !== messageId);
    saveChatStateToStorage();
    drawToolbar();
    return;
  }
  try {
    const r = await api("DELETE", `/api/chat/${tab.sessionId}/queue/${encodeURIComponent(messageId)}`);
    tab.queuedMessages = normalizeQueuedMessages(r.queued_messages);
    saveChatStateToStorage();
    drawToolbar();
  } catch (e) {
    toast("Could not remove queued message: " + e.message, "error");
  }
}

function cssEscape(value) {
  if (window.CSS?.escape) return CSS.escape(value);
  return String(value).replace(/["\\]/g, "\\$&");
}

function requestChatInputFocus() {
  chatState.focusInputUntil = Date.now() + 3000;
  focusChatInputSoon();
}

function shouldKeepChatInputFocused() {
  return Date.now() < (chatState.focusInputUntil || 0);
}

function focusChatInputSoon() {
  setTimeout(() => {
    const input = $("#chat-input");
    if (!input || input.disabled) return;
    input.focus();
    const end = input.value.length;
    if (typeof input.setSelectionRange === "function") {
      input.setSelectionRange(end, end);
    }
  }, 0);
}

async function sendChatText(text, tab = currentChatTab()) {
  const t = tab;
  if (!t) return;
  text = String(text || "");
  if (!text.trim()) return;
  if (t.sending) return;
  t.sending = true;
  saveChatStateToStorage();
  drawToolbar();
  try {
    await sendChatTextUnlocked(t, text);
  } finally {
    t.sending = false;
    saveChatStateToStorage();
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  }
}

async function sendChatTextUnlocked(t, text) {
  if (!t.sessionId) {
    queueChatTextForTab(t, text);
    await ensureChatSession(t);
    saveChatStateToStorage();
    drawToolbar();
    requestChatInputFocus();
    return;
  }
  if (normalizeQueuedMessages(t.localQueuedMessages).length) {
    queueChatTextForTab(t, text);
    await flushLocalQueuedMessages(t);
    return;
  }
  await queueChatTextOnServer(t, text);
}

function queueChatTextForTab(tab, text) {
  if (!tab) return;
  const trimmed = String(text || "").trim();
  if (!trimmed) return;
  ensureChatTabQueueState(tab);
  tab.localQueuedMessages.push({
    id: newLocalQueuedMessageId(),
    text: trimmed,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
    local: true,
  });
}

function newLocalQueuedMessageId() {
  return `local-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

async function flushLocalQueuedMessages(tab) {
  if (!tab?.sessionId) return;
  ensureChatTabQueueState(tab);
  if (!tab.localQueuedMessages.length) return;
  const messages = tab.localQueuedMessages.slice();
  const text = messages.length === 1
    ? messages[0].text
    : messages.map((message, idx) => `Message ${idx + 1}:\n${message.text}`).join("\n\n");
  tab.localQueuedMessages = [];
  saveChatStateToStorage();
  drawToolbar();
  if (shouldKeepChatInputFocused()) focusChatInputSoon();
  await queueChatTextOnServer(tab, text);
}

async function queueChatTextOnServer(tab, text) {
  if (!tab?.sessionId) return;
  try {
    tab.pending = true;
    saveChatStateToStorage();
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
    const r = await api("POST", `/api/chat/${tab.sessionId}/input`, { text });
    tab.queuedMessages = normalizeQueuedMessages(r.queued_messages);
    tab.pending = r.in_flight === undefined ? true : !!r.in_flight;
    tab.closedReason = null;
    tab.sentUserInput = true;
    saveChatStateToStorage();
    refreshProcessesTabForChatChange();
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
  } catch (e) {
    queueChatTextForTab(tab, text);
    tab.pending = false;
    saveChatStateToStorage();
    drawToolbar();
    if (shouldKeepChatInputFocused()) focusChatInputSoon();
    toast("Could not send: " + e.message, "error");
  }
}
