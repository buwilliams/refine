// ---- Toolbar ----------------------------------------------------------------

// chatState is the persisted toolbar layout retained for storage compatibility.
// Agent-facing tabs are managed terminal launch profiles; each has an independent
// PTY session while Files, System, and Goal Logs keep their specialized panels.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const CHAT_TABS_STORAGE_VERSION = 2;
const INTERACTIVE_TERMINAL_MODES = new Set(["terminal", "agent", "plan", "goal", "standalone"]);
const SYSTEM_OPERATION_LOG_LIMIT = 250;
const GOAL_LOG_TAIL_LIMIT = 200;
const GOAL_LOG_DEFAULT_ORDER = "tail";
const SYSTEM_LOG_FILTERS = [
  { status: "info", label: "Info" },
  { status: "start", label: "Started" },
  { status: "queued", label: "Queued" },
  { status: "complete", label: "Completed" },
  { status: "error", label: "Errors" },
];
const FILES_TREE_MAX_DEPTH = 3;
const FILES_TREE_MAX_ENTRIES = 200;
const FILES_SEARCH_MAX_RESULTS = 20;
const FILES_SEARCH_DEBOUNCE_MS = 250;
const FILE_TEXT_CHUNK_BYTES = 128_000;
const TERMINAL_OUTPUT_MAX_CHARS = 50_000;
const GOAL_TERMINAL_INITIAL_TAIL_BYTES = 16_000;
const TERMINAL_FONT_SIZE = 15;
const TERMINAL_LINE_HEIGHT = 1.35;
let filesSearchTimer = null;
let filesSearchRequestSeq = 0;
let filesSearchAbortController = null;
const chatState = {
  tabs: {},                // tabId → { goalId, label, sessionId, output, closedReason }
  activeTabId: null,
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
  searchSelectedPath: "",
  searchUserSelectedPath: "",
  searchLoading: false,
  searchError: "",
  loading: false,
  error: "",
};
const terminalStates = new Map();

function toolbarStateStorage() {
  // Toolbar layout is useful across a page refresh, but must not turn into a
  // permanent set of default tabs when the app is opened in a new session.
  return typeof sessionStorage === "undefined" ? localStorage : sessionStorage;
}

function toolbarTabUsesTerminal(tab) {
  return !!tab && INTERACTIVE_TERMINAL_MODES.has(tab.mode);
}

function normalizeInteractiveTerminalTab(tab) {
  if (!toolbarTabUsesTerminal(tab)) return tab;
  // Session ids persisted by the retired custom chat backend are not PTY session ids.
  if (tab.sessionId && !tab.processId) tab.sessionId = null;
  tab.processId = tab.processId || null;
  tab.provider = tab.provider || null;
  tab.cwd = tab.cwd || "";
  tab.attentionState = tab.attentionState || "";
  tab.attentionMessage = tab.attentionMessage || "";
  tab.exited = !!tab.exited;
  tab.initialPrompt = String(tab.initialPrompt || "");
  return tab;
}

function terminalStateFor(tabId = chatState.activeTabId) {
  const tab = normalizeInteractiveTerminalTab(chatState.tabs[tabId]);
  if (!tab) return null;
  let terminal = terminalStates.get(tabId);
  if (!terminal) {
    terminal = {
      tabId,
      mode: tab.mode,
      sessionId: tab.sessionId || "",
      processId: tab.processId || "",
      cwd: tab.cwd || "",
      attentionState: tab.attentionState || "",
      attentionMessage: tab.attentionMessage || "",
      display: "",
      term: null,
      inputBuffer: "",
      inputSessionId: "",
      inputFlushTimer: null,
      resizeTimer: null,
      inputSendPromise: Promise.resolve(),
      lastSeq: 0,
      lastCols: 0,
      lastRows: 0,
      loading: false,
      connected: false,
      exited: tab.sessionId ? false : !!tab.exited,
      statusChecked: !tab.sessionId,
      reattaching: !!tab.sessionId,
      attachmentPromise: null,
      historyInitialized: false,
      historyLoading: false,
      historyLoaded: false,
      historyStart: 0,
      historyEnd: 0,
      error: "",
      stopping: false,
      eventSource: null,
      outputResizeObserver: null,
      observedOutput: null,
    };
    terminalStates.set(tabId, terminal);
  }
  return terminal;
}

function ensureStandaloneTab() {
  for (const tab of Object.values(chatState.tabs || {})) {
    normalizeInteractiveTerminalTab(tab);
  }
}

function currentToolbarTab() {
  ensureStandaloneTab();
  if (chatState.activeTabId && chatState.tabs[chatState.activeTabId]) {
    return chatState.tabs[chatState.activeTabId];
  }
  const first = Object.keys(chatState.tabs)[0] || null;
  chatState.activeTabId = first;
  return first ? chatState.tabs[first] : null;
}

function loadChatStateFromStorage() {
  try {
    const raw = toolbarStateStorage().getItem(CHAT_TABS_STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && parsed.tabs) {
      chatState.tabs = Object.fromEntries(
        Object.entries(parsed.tabs).filter(([, tab]) => {
          if (tab?.mode === "supervisor") return false;
          if (parsed.version === CHAT_TABS_STORAGE_VERSION) return true;
          const legacyDefault = ["standalone", "system", "files", "terminal"].includes(tab?.mode);
          return !legacyDefault || !!tab?.sessionId;
        }),
      );
      for (const tab of Object.values(chatState.tabs)) {
        if (tab.mode === "standalone") tab.label = "Agent in Worktree";
        if (tab.mode === "plan") tab.label = "Planing Agent";
      }
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
  // Persist live PTY ids so a page reload can reattach to the daemon session.
  const tabs = {};
  for (const [id, t] of Object.entries(chatState.tabs)) {
      tabs[id] = {
        goalId: t.goalId, label: t.label,
        mode: t.mode || (t.goalId ? "goal" : id === "plan" ? "plan" : "standalone"),
        goalStatus: t.goalStatus || "",
        sessionId: t.sessionId,
        processId: t.processId || null,
        provider: t.provider || null,
        cwd: t.cwd || "",
        attentionState: t.attentionState || "",
        attentionMessage: t.attentionMessage || "",
        worktree: t.worktree || null,
        exited: !!t.exited,
        initialPrompt: String(t.initialPrompt || "").slice(-50_000),
        logEntries: t.mode === "goal_logs"
          ? normalizeGoalLogEntries(t.logEntries).slice(-GOAL_LOG_TAIL_LIMIT)
          : undefined,
        logQuery: t.mode === "goal_logs" ? String(t.logQuery || "") : undefined,
        logOrder: t.mode === "goal_logs" ? normalizeGoalLogOrder(t.logOrder) : undefined,
    };
  }
  try {
    toolbarStateStorage().setItem(CHAT_TABS_STORAGE_KEY, JSON.stringify({
      tabs, activeTabId: chatState.activeTabId,
      version: CHAT_TABS_STORAGE_VERSION,
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
  if (typeof drainPendingSystemOperations === "function") drainPendingSystemOperations();
  drawToolbar();
  observeToolbarSize();
  observeTopbarHeight();
}

function initChatDock() { initToolbar(); }

function resetChatForProjectSwitch() {
  chatState.tabs = {};
  chatState.activeTabId = null;
  chatState.open = false;
  chatState.bodyHeight = null;
  chatState.fullscreen = false;
  systemOperationState.filters.clear();
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
  const stops = [];
  for (const terminal of terminalStates.values()) {
    if (terminal.mode !== "goal" && terminal.sessionId && terminal.connected) {
      stops.push(
        api("POST", `/api/terminal/${encodeURIComponent(terminal.sessionId)}/stop`)
          .catch(() => undefined),
      );
    }
    terminal.eventSource?.close();
    terminal.outputResizeObserver?.disconnect();
    terminal.term?.dispose();
    if (terminal.inputFlushTimer) clearTimeout(terminal.inputFlushTimer);
    if (terminal.resizeTimer) clearTimeout(terminal.resizeTimer);
  }
  terminalStates.clear();
  if (stops.length) Promise.allSettled(stops).then(refreshProcessesTabForChatChange);
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
    scheduleActiveTerminalFit();
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(root);
  } else {
    window.addEventListener("resize", apply);
  }
}

function observeChatDockSize() { observeToolbarSize(); }

// Opens the dock and (optionally) ensures a terminal launch profile for a Goal.
// Activating an agent tab starts its configured terminal session on demand.
function openAgentDock({ goalId = null, goalStatus = null } = {}) {
  ensureStandaloneTab();
  if (!goalId) return createToolbarTab("agent");
  let tabId = goalId;
  if (goalId) {
    if (!chatState.tabs[goalId]) {
      chatState.tabs[goalId] = {
        goalId,
        label: `Goal ${goalId.slice(0, 8)}…`,
        mode: "goal",
        goalStatus: goalStatus || "",
        sessionId: null,
      };
    } else if (goalStatus) {
      chatState.tabs[goalId].goalStatus = goalStatus;
    }
    normalizeInteractiveTerminalTab(chatState.tabs[goalId]);
  }
  return activateToolbarTab(tabId);
}

function nextToolbarTabId(mode) {
  const suffix = typeof globalThis.crypto?.randomUUID === "function"
    ? globalThis.crypto.randomUUID()
    : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  return `${mode}:${suffix}`;
}

function nextToolbarLabel(mode) {
  const base = {
    agent: "Agent",
    standalone: "Agent in Worktree",
    system: "System",
    files: "Files",
    terminal: "Terminal",
    plan: "Planing Agent",
  }[mode] || "Tool";
  const count = Object.values(chatState.tabs).filter((tab) => tab.mode === mode).length + 1;
  return count === 1 ? base : `${base} ${count}`;
}

async function createToolbarTab(mode) {
  if (!["agent", "standalone", "system", "files", "terminal", "plan"].includes(mode)) return;
  const tabId = nextToolbarTabId(mode);
  chatState.tabs[tabId] = normalizeInteractiveTerminalTab({
    goalId: null,
    label: nextToolbarLabel(mode),
    mode,
    sessionId: null,
  });
  chatState.activeTabId = tabId;
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  if (toolbarTabUsesTerminal(chatState.tabs[tabId])) {
    await startTerminalSession(chatState.tabs[tabId]);
  }
  return tabId;
}

function openToolbarTab(tabId) {
  return activateToolbarTab(tabId);
}

function goalLogTabId(goalId) {
  return `goal-logs:${goalId}`;
}

function openGoalLogTail({ goalId, goalName = "" } = {}) {
  if (!goalId) return;
  ensureStandaloneTab();
  const tabId = goalLogTabId(goalId);
  if (!chatState.tabs[tabId]) {
    chatState.tabs[tabId] = {
      goalId,
      label: `Logs ${goalId.slice(0, 8)}…`,
      mode: "goal_logs",
      goalName,
      sessionId: null,
      logEntries: [],
      logQuery: "",
      logOrder: GOAL_LOG_DEFAULT_ORDER,
      logsLoaded: false,
      logsLoading: false,
      logsError: "",
    };
  } else if (goalName) {
    chatState.tabs[tabId].goalName = goalName;
  }
  chatState.activeTabId = tabId;
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  loadGoalLogTail(chatState.tabs[tabId]);
}

async function renderGoalPlan() {
  await renderGoalsList();
  openPlanChatDock();
}

function ensurePlanTab() {
  ensureStandaloneTab();
  if (!chatState.tabs.plan) {
    chatState.tabs.plan = {
      goalId: null,
      label: "Planing Agent",
      mode: "plan",
      sessionId: null,
    };
  }
  normalizeInteractiveTerminalTab(chatState.tabs.plan);
}

async function openPlanChatDock(options = {}) {
  const initialPrompt = typeof options === "string"
    ? options
    : String(options.initialPrompt || "");
  ensurePlanTab();
  const t = chatState.tabs.plan;
  if (initialPrompt.trim()) {
    t.initialPrompt = initialPrompt.trim();
  }
  await activateToolbarTab("plan");
}

async function activateToolbarTab(tabId, { toggleIfActive = false } = {}) {
  ensureStandaloneTab();
  const tab = chatState.tabs[tabId];
  if (!tab) return;
  const wasActive = chatState.activeTabId === tabId;
  const terminal = toolbarTabUsesTerminal(tab) ? terminalStateFor(tabId) : null;
  let shouldStart = !!terminal && !terminal.loading && !terminal.stopping &&
    (!terminal.sessionId || (terminal.statusChecked && terminal.exited));

  if (toggleIfActive && wasActive && chatState.open && !shouldStart && terminal?.statusChecked !== false) {
    toggleToolbar();
    return;
  }

  chatState.activeTabId = tabId;
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  if (terminal?.sessionId && !terminal.statusChecked) {
    await reattachTerminalSession(tab, terminal);
    shouldStart = !terminal.connected && !terminal.loading && terminal.statusChecked && terminal.exited;
  }
  if (shouldStart) await startTerminalSession(tab);
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
  for (const terminal of terminalStates.values()) {
    terminal.outputResizeObserver?.disconnect();
    terminal.outputResizeObserver = null;
    terminal.observedOutput = null;
  }
  ensureStandaloneTab();
  const active = currentToolbarTab();
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const filesActive = active?.mode === "files";
  const systemActive = active?.mode === "system";
  const terminalActive = toolbarTabUsesTerminal(active);
  const goalLogsActive = active?.mode === "goal_logs";

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
      <details class="toolbar-add-menu" data-testid="toolbar-add-menu">
        <summary class="toolbar-add-button" data-testid="toolbar-add" aria-label="Add Toolbar tab" title="Add tab">
          <svg class="toolbar-action-icon" data-testid="toolbar-add-icon" aria-hidden="true" viewBox="0 0 20 20" focusable="false">
            <path d="M10 4v12M4 10h12"></path>
          </svg>
        </summary>
        <div class="toolbar-add-options" role="menu">
          ${[
            ["agent", "Agent"],
            ["standalone", "Agent in Worktree"],
            ["system", "System"],
            ["files", "Files"],
            ["terminal", "Terminal"],
            ["plan", "Planing Agent"],
          ].map(([mode, label]) => `<button type="button" role="menuitem" data-add-toolbar-tab="${mode}">${label}</button>`).join("")}
        </div>
      </details>
      <div class="toolbar-tabs">
        ${Object.entries(tabs).map(([id, t]) => `
          <button class="toolbar-tab ${id === activeId ? "active" : ""} ${toolbarTabActivityClass(t)}"
                  data-tab-id="${htmlEscape(id)}"
                  data-testid="toolbar-tab-${htmlEscape(id)}"
                  title="${htmlEscape(toolbarTabTitle(t))}">
            ${htmlEscape(t.label)}${toolbarTabSessionDot(t)}
            <span class="toolbar-tab-close" data-close-tab="${htmlEscape(id)}" data-testid="toolbar-tab-close" title="Close tab">
              <svg class="toolbar-action-icon" data-testid="toolbar-tab-close-icon" aria-hidden="true" viewBox="0 0 20 20" focusable="false">
                <path d="M5 5l10 10M15 5L5 15"></path>
              </svg>
            </span>
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
            ? renderTerminalPanel(active)
            : goalLogsActive
              ? renderGoalLogPanel(active)
              : `<div class="toolbar-empty muted" data-testid="toolbar-empty">Use the Add button to open a tool or agent.</div>`}
    </div>
  `;
  if (filesActive) bindFilesPanel(root);
  if (systemActive) bindSystemPanel(root);
  if (terminalActive) bindTerminalPanel(root, active);
  if (goalLogsActive) bindGoalLogPanel(root, active);

  if (chatState.open && goalLogsActive) {
    const out = root.querySelector("#goal-log-tail");
    if (out) scrollGoalLogEdge(active, out);
  }

  $$(".toolbar-tab", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      if (e.target.matches("[data-close-tab]")) return;
      const id = el.dataset.tabId;
      if (!id) return;
      void activateToolbarTab(id, { toggleIfActive: true });
    });
  });
  $$("[data-close-tab]", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      closeChatTab(el.dataset.closeTab);
    });
  });
  $$("[data-add-toolbar-tab]", root).forEach((el) => {
    el.addEventListener("click", () => {
      root.querySelector(".toolbar-add-menu")?.removeAttribute("open");
      void createToolbarTab(el.dataset.addToolbarTab);
    });
  });
  $("#btn-dock-toggle")?.addEventListener("click", toggleToolbar);
  $("#btn-dock-fullscreen")?.addEventListener("click", toggleToolbarFullscreen);

  wireToolbarResize(root);
  if (filesActive && !filesState.entriesByPath[""] && !filesState.loading) {
    loadFilesDirectory("", { expand: true, redraw: true });
  }
  if (terminalActive) {
    const terminal = terminalStateFor(chatState.activeTabId);
    if (terminal?.sessionId && !terminal.statusChecked) {
      void reattachTerminalSession(active, terminal);
    } else {
      connectTerminalEvents(active);
    }
    focusTerminalSoon(active);
  }
  if (goalLogsActive && !active.logsLoaded && !active.logsLoading) {
    loadGoalLogTail(active);
  }
}

function drawChatDock() { drawToolbar(); }

function toolbarTabTitle(tab) {
  if (tab.mode === "agent") return "General-purpose agent terminal";
  if (tab.mode === "files") return "File browser";
  if (tab.mode === "system") return "System operations";
  if (toolbarTabUsesTerminal(tab)) return `${tab.label} terminal`;
  if (tab.mode === "goal_logs") return `Live logs for Goal ${tab.goalId}`;
  return tab.goalId || tab.label || "Toolbar";
}

function toolbarTabHasSessionIndicator(tab) {
  return toolbarTabUsesTerminal(tab) && !!(tab.sessionId && !tab.exited);
}

function toolbarTabActivityClass(tab) {
  return toolbarTabHasSessionIndicator(tab) ? "toolbar-tab-ready" : "";
}

function toolbarTabSessionDot(tab) {
  if (!toolbarTabHasSessionIndicator(tab)) return "";
  return ` <span class="toolbar-tab-dot" data-testid="toolbar-tab-dot" title="active session"></span>`;
}

function normalizeGoalLogEntries(entries) {
  if (!Array.isArray(entries)) return [];
  return entries.filter((entry) => entry && typeof entry === "object" && entry.message);
}

function normalizeGoalLogOrder(order) {
  return order === "head" ? "head" : GOAL_LOG_DEFAULT_ORDER;
}

function goalLogDetailsText(details) {
  if (details == null) return "";
  if (typeof details === "string") return details;
  try {
    return JSON.stringify(details, null, 2);
  } catch (_) {
    return String(details);
  }
}

function goalLogSearchText(entry) {
  const actions = Array.isArray(entry?.actions)
    ? entry.actions.flatMap((action) => [action?.label, action?.href, action?.command])
    : [];
  return [
    entry?.datetime,
    entry?.severity,
    entry?.category,
    entry?.actor,
    entry?.message,
    goalLogDetailsText(entry?.details),
    ...actions,
  ].filter(Boolean).join("\n").toLocaleLowerCase();
}

function visibleGoalLogEntries(tab) {
  const query = String(tab?.logQuery || "").trim().toLocaleLowerCase();
  const entries = normalizeGoalLogEntries(tab?.logEntries)
    .filter((entry) => !query || goalLogSearchText(entry).includes(query));
  return normalizeGoalLogOrder(tab?.logOrder) === "head" ? entries.reverse() : entries;
}

function goalLogEntryKey(entry) {
  return String(entry?.id || [
    entry?.datetime || "",
    entry?.severity || "",
    entry?.category || "",
    entry?.actor || "",
    entry?.message || "",
  ].join("\u0000"));
}

function mergeGoalLogEntries(...groups) {
  const merged = new Map();
  for (const entry of groups.flatMap(normalizeGoalLogEntries)) {
    merged.set(goalLogEntryKey(entry), entry);
  }
  return [...merged.values()]
    .sort((left, right) => {
      const byTime = String(left.datetime || "").localeCompare(String(right.datetime || ""));
      return byTime || goalLogEntryKey(left).localeCompare(goalLogEntryKey(right));
    })
    .slice(-GOAL_LOG_TAIL_LIMIT);
}

async function loadGoalLogTail(tab, { redraw = true } = {}) {
  if (!tab?.goalId || tab.logsLoading) return;
  tab.logsLoading = true;
  tab.logsError = "";
  if (redraw && chatState.tabs[chatState.activeTabId] === tab) drawToolbar();
  const params = new URLSearchParams({
    goal_id: tab.goalId,
    limit: String(GOAL_LOG_TAIL_LIMIT),
    offset: "0",
    sort: "datetime",
    // Fetch the newest page, then mergeGoalLogEntries presents it oldest-first.
    dir: "desc",
  });
  try {
    const data = await api("GET", `/api/activity?${params}`);
    // Preserve SSE entries that may arrive while the historical request is in flight.
    tab.logEntries = mergeGoalLogEntries(data.activity, tab.logEntries);
    tab.logsLoaded = true;
  } catch (error) {
    tab.logsError = error?.message || "Could not load Goal logs.";
  } finally {
    tab.logsLoading = false;
    saveChatStateToStorage();
    if (chatState.tabs[chatState.activeTabId] === tab) drawToolbar();
  }
}

function handleGoalLogSseEvent(entry) {
  const goalId = String(entry?.goal_id || "");
  if (!goalId) return;
  const tab = chatState.tabs[goalLogTabId(goalId)];
  if (!tab || tab.mode !== "goal_logs") return;
  const entries = normalizeGoalLogEntries(tab.logEntries);
  const key = goalLogEntryKey(entry);
  if (entries.some((candidate) => goalLogEntryKey(candidate) === key)) return;
  tab.logEntries = mergeGoalLogEntries(entries, [entry]);
  tab.logsLoaded = true;
  saveChatStateToStorage();
  if (chatState.open && chatState.activeTabId === goalLogTabId(goalId)) {
    const root = $("#toolbar-dock");
    if (!updateGoalLogPanel(root, tab)) drawToolbar();
  }
}

function renderGoalLogPanel(tab) {
  const order = normalizeGoalLogOrder(tab.logOrder);
  const query = String(tab.logQuery || "");
  return `
    <div class="goal-log-panel" data-testid="toolbar-goal-log-panel">
      <div class="goal-log-header">
        <span class="goal-log-live" aria-label="Following live Goal logs"><span aria-hidden="true"></span>Following</span>
        <a class="chat-goal-link"
           href="#/goals/${encodeURIComponent(tab.goalId)}"
           data-testid="goal-log-goal-link">
          Goal ${htmlEscape(tab.goalId.slice(0, 10))}…
        </a>
        <a class="chat-goal-link"
           href="#/logs?goal_id=${encodeURIComponent(tab.goalId)}"
           data-testid="goal-log-full-link">Open full logs</a>
        <span class="spacer"></span>
        <span class="muted small" data-testid="goal-log-status">${htmlEscape(goalLogStatus(tab))}</span>
        <button class="secondary small" type="button" id="btn-goal-log-refresh"
                data-testid="goal-log-refresh" ${tab.logsLoading ? "disabled" : ""}>Refresh</button>
      </div>
      <div class="goal-log-controls">
        <label class="goal-log-search" for="goal-log-search">
          <span aria-hidden="true">${toolbarIcon("search")}</span>
          <input type="search" id="goal-log-search" value="${htmlEscape(query)}"
                 autocomplete="off" placeholder="Search this trail"
                 data-testid="goal-log-search" aria-label="Search Goal logs">
        </label>
        <button class="secondary small goal-log-clear" type="button" id="btn-goal-log-clear"
                data-testid="goal-log-search-clear" ${query ? "" : "disabled"}>Clear</button>
        <div class="goal-log-order" role="group" aria-label="Log stream order">
          <button class="secondary small ${order === "head" ? "active" : ""}" type="button"
                  data-goal-log-order="head" data-testid="goal-log-order-head"
                  aria-pressed="${order === "head"}" title="Newest entries first">Head</button>
          <button class="secondary small ${order === "tail" ? "active" : ""}" type="button"
                  data-goal-log-order="tail" data-testid="goal-log-order-tail"
                  aria-pressed="${order === "tail"}" title="Newest entries last">Tail</button>
        </div>
      </div>
      <div class="goal-log-tail" id="goal-log-tail" role="log" aria-live="polite"
           aria-label="Live logs for Goal ${htmlEscape(tab.goalId)}"
           data-testid="goal-log-tail">
        <div id="goal-log-lines">${renderGoalLogLines(tab)}</div>
      </div>
    </div>`;
}

function goalLogStatus(tab) {
  if (tab.logsLoading) return "Loading…";
  if (tab.logsError) return tab.logsError;
  const total = normalizeGoalLogEntries(tab.logEntries).length;
  const visible = visibleGoalLogEntries(tab).length;
  return String(tab.logQuery || "").trim()
    ? `${visible} of ${total} matching ${total === 1 ? "entry" : "entries"}`
    : `${total} recent ${total === 1 ? "entry" : "entries"}`;
}

function renderGoalLogLines(tab) {
  const entries = visibleGoalLogEntries(tab);
  if (entries.length) return entries.map(renderGoalLogLine).join("");
  if (tab.logsError) {
    return `<div class="goal-log-empty" data-testid="goal-log-empty">${htmlEscape(tab.logsError)}</div>`;
  }
  const query = String(tab.logQuery || "").trim();
  return `<div class="goal-log-empty" data-testid="goal-log-empty">${query
    ? `No recent logs match “${htmlEscape(query)}”.`
    : "Waiting for Goal activity."}</div>`;
}

function renderGoalLogLine(entry) {
  const severity = normalizeSystemLogStatus(entry.severity);
  const details = goalLogDetailsText(entry.details);
  const actor = String(entry.actor || "").trim();
  return `
    <div class="goal-log-line goal-log-${htmlEscape(severity)}" data-testid="goal-log-line">
      <span class="goal-log-time">${htmlEscape(formatSystemLogTime(entry.datetime))}</span>
      <span class="goal-log-severity">[${htmlEscape(entry.severity || "info")}]</span>
      <span class="goal-log-category">[${htmlEscape(entry.category || "activity")}]</span>
      ${actor ? `<span class="goal-log-actor">[${htmlEscape(actor)}]</span>` : ""}
      <span class="goal-log-message">
        ${mdInline(String(entry.message || ""))}${renderGoalLogActions(entry.actions)}
        ${details ? `<details><summary>Details</summary><pre>${htmlEscape(details)}</pre></details>` : ""}
      </span>
    </div>`;
}

function renderGoalLogActions(actions) {
  if (!Array.isArray(actions)) return "";
  const links = actions.flatMap((action) => {
    if (action?.type !== "link") return [];
    const href = String(action.href || "").trim();
    if (!/^(https?:|mailto:|#\/|\/(?!\/))/i.test(href)) return [];
    const external = /^(https?:|mailto:)/i.test(href);
    return [`<a class="goal-log-action" href="${htmlEscape(href)}"${external
      ? ' target="_blank" rel="noopener noreferrer"'
      : ""}>${htmlEscape(action.label || href)}</a>`];
  });
  return links.length ? ` <span class="goal-log-actions">${links.join(" ")}</span>` : "";
}

function scrollGoalLogEdge(tab, element) {
  if (!element) return;
  element.scrollTop = normalizeGoalLogOrder(tab?.logOrder) === "head" ? 0 : element.scrollHeight;
}

function updateGoalLogPanel(root, tab) {
  if (!root) return false;
  const lines = root.querySelector("#goal-log-lines");
  const status = root.querySelector('[data-testid="goal-log-status"]');
  if (!lines || !status) return false;
  lines.innerHTML = renderGoalLogLines(tab);
  status.textContent = goalLogStatus(tab);
  scrollGoalLogEdge(tab, root.querySelector("#goal-log-tail"));
  return true;
}

function bindGoalLogPanel(root, tab) {
  root.querySelector("#btn-goal-log-refresh")?.addEventListener("click", () => {
    tab.logsLoaded = false;
    loadGoalLogTail(tab);
  });
  const search = root.querySelector("#goal-log-search");
  const clear = root.querySelector("#btn-goal-log-clear");
  search?.addEventListener("input", (event) => {
    tab.logQuery = event.currentTarget.value;
    if (clear) clear.disabled = !tab.logQuery;
    updateGoalLogPanel(root, tab);
    saveChatStateToStorage();
  });
  clear?.addEventListener("click", () => {
    tab.logQuery = "";
    if (search) {
      search.value = "";
      search.focus();
    }
    clear.disabled = true;
    updateGoalLogPanel(root, tab);
    saveChatStateToStorage();
  });
  root.querySelectorAll("[data-goal-log-order]").forEach((button) => {
    button.addEventListener("click", () => {
      tab.logOrder = normalizeGoalLogOrder(button.dataset.goalLogOrder);
      root.querySelectorAll("[data-goal-log-order]").forEach((candidate) => {
        const active = candidate.dataset.goalLogOrder === tab.logOrder;
        candidate.classList.toggle("active", active);
        candidate.setAttribute("aria-pressed", String(active));
      });
      updateGoalLogPanel(root, tab);
      saveChatStateToStorage();
    });
  });
}

function recordSystemOperation(payload, redraw = true) {
  const item = {
    message: String(payload?.message || "").trim(),
    status: normalizeSystemLogStatus(payload?.status),
    category: String(payload?.category || "system"),
    timestamp: String(payload?.timestamp || new Date().toISOString()),
    details: payload?.details ?? null,
  };
  if (!item.message) return;
  if (isDuplicateSystemOperation(item)) return;
  systemOperationState.messages.push(item);
  if (systemOperationState.messages.length > SYSTEM_OPERATION_LOG_LIMIT) {
    systemOperationState.messages = systemOperationState.messages.slice(-SYSTEM_OPERATION_LOG_LIMIT);
  }
  if (redraw && chatState.open && currentToolbarTab()?.mode === "system") {
    drawToolbar();
  }
}

function isDuplicateSystemOperation(item) {
  const cutoff = Date.parse(item.timestamp || "") - 5000;
  const itemDetails = formatSystemOperationDetails(item.details);
  return systemOperationState.messages.some((existing) => {
    if (
      existing.message !== item.message
      || existing.status !== item.status
      || existing.category !== item.category
      || formatSystemOperationDetails(existing.details) !== itemDetails
    ) return false;
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
  const details = systemOperationDetailEntries(item.details);
  return `
    <div class="system-log-line system-log-${item.status}"
         data-testid="system-log-line"
         data-system-log-status="${htmlEscape(item.status)}"
         data-system-log-category="${htmlEscape(item.category)}">
      <time class="system-log-time" datetime="${htmlEscape(item.timestamp)}" title="${htmlEscape(item.timestamp)}">${htmlEscape(time)}</time>
      <div class="system-log-body">
        <div class="system-log-headline">
          <span class="system-log-status" data-testid="system-log-status">${htmlEscape(systemLogStatusLabel(item.status))}</span>
          <span class="system-log-category" data-testid="system-log-category">${htmlEscape(item.category)}</span>
          <span class="system-log-message" data-testid="system-log-message">${htmlEscape(item.message)}</span>
        </div>
        ${details.length ? `
          <dl class="system-log-details" data-testid="system-log-details">
            ${details.map(([key, value]) => `
              <div class="system-log-detail" data-testid="system-log-detail">
                <dt>${htmlEscape(key)}</dt>
                <dd>${htmlEscape(value)}</dd>
              </div>`).join("")}
          </dl>` : ""}
      </div>
    </div>`;
}

function systemOperationDetailEntries(details) {
  if (details === null || details === undefined || details === "") return [];
  if (typeof details !== "object" || Array.isArray(details)) {
    return [["details", formatSystemOperationDetailValue(details)]];
  }
  const entries = Object.entries(details)
    .filter(([, value]) => value !== null && value !== undefined && value !== "")
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([key, value]) => [key, formatSystemOperationDetailValue(value)]);
  return entries.length ? entries : [["details", formatSystemOperationDetailValue(details)]];
}

function formatSystemOperationDetails(details) {
  return systemOperationDetailEntries(details)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function formatSystemOperationDetailValue(value) {
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  try {
    const encoded = JSON.stringify(value);
    return encoded === undefined ? String(value) : encoded;
  } catch (_) {
    return String(value);
  }
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

function renderTerminalPanel(tab) {
  const terminal = terminalStateFor(chatState.activeTabId);
  const role = tab.mode === "terminal" ? "shell" : `${tab.label} agent`;
  const status = terminal?.loading
    ? `Starting ${role}...`
    : terminal?.stopping
      ? `Stopping ${role}...`
      : terminal?.reattaching
        ? `Reattaching to ${role}...`
        : terminal?.error
          ? terminal.error
          : terminal?.exited
            ? `${role} exited.`
            : terminal?.connected
              ? terminal.attentionState === "needs_input"
                ? terminal.attentionMessage || `${role} needs your input.`
                : terminal.cwd || `${role} active.`
              : `${role} stopped.`;
  const pendingActionLabel = terminal?.stopping
    ? "Stopping…"
    : terminal?.reattaching
      ? "Reattaching…"
      : "Starting…";
  const action = terminal?.loading || terminal?.stopping || terminal?.reattaching
    ? `<button type="button" class="secondary" data-terminal-action disabled>${pendingActionLabel}</button>`
    : terminal?.connected
      ? `<button type="button" class="danger" data-terminal-action="stop" data-testid="terminal-stop">Stop</button>`
      : terminal?.sessionId && !terminal?.statusChecked
        ? `<button type="button" class="secondary" data-terminal-action="reattach">Reconnect</button>`
      : tab.mode === "goal" && terminal?.exited
        ? `<button type="button" class="secondary" disabled>Session ended</button>`
        : `<button type="button" class="primary" data-terminal-action="start" data-testid="terminal-start">${tab.mode === "goal" ? "Open" : terminal?.exited ? "Restart" : "Start"}</button>`;
  const provider = tab.provider ? ` · ${htmlEscape(tab.provider)}` : "";
  const worktree = tab.worktree?.path
    ? `<span class="muted small" data-testid="terminal-worktree"> · Worktree: <code>${htmlEscape(tab.worktree.path)}</code></span>`
    : "";
  return `
    <div class="terminal-panel" data-testid="toolbar-terminal-panel">
      <div class="terminal-titlebar">
        <span class="muted small" data-testid="terminal-status">${htmlEscape(status)}</span>
        <span class="muted small" data-testid="terminal-profile">${htmlEscape(tab.label)}${provider}</span>
        ${worktree}
        <span class="spacer"></span>
        ${action}
      </div>
      <div class="terminal-output"
           data-testid="terminal-output"
           tabindex="0"
           role="textbox"
           aria-label="Terminal"
           spellcheck="false"></div>
    </div>`;
}

function bindTerminalPanel(root, tab) {
  const output = root.querySelector(".terminal-output");
  output?.addEventListener("focus", () => output.classList.add("focused"));
  output?.addEventListener("blur", () => output.classList.remove("focused"));
  ensureTerminalRenderer(output, tab);
  observeTerminalOutputSize(output, tab);
  root.querySelector('[data-terminal-action="start"]')?.addEventListener("click", () => startTerminalSession(tab));
  root.querySelector('[data-terminal-action="reattach"]')?.addEventListener("click", () => reattachTerminalSession(tab));
  root.querySelector('[data-terminal-action="stop"]')?.addEventListener("click", () => stopTerminalSession(tab));
}

async function startTerminalSession(tab = currentToolbarTab()) {
  if (!toolbarTabUsesTerminal(tab)) return;
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId;
  const terminal = terminalStateFor(tabId);
  if (!terminal || terminal.loading || terminal.stopping || terminal.connected) return;
  terminal.loading = true;
  terminal.error = "";
  drawToolbar();
  try {
    const size = terminalSize();
    const result = await api("POST", "/api/terminal/session", {
      ...size,
      profile: tab.mode,
      goal_id: tab.goalId || undefined,
      feature_id: tab.featureId || undefined,
      initial_prompt: tab.initialPrompt || undefined,
      worktree: tab.mode === "standalone" ? tab.worktree || undefined : undefined,
    });
    terminal.eventSource?.close();
    terminal.term?.dispose();
    terminal.term = null;
    terminal.display = "";
    terminal.sessionId = result.id || "";
    terminal.processId = result.process_id || "";
    terminal.cwd = result.cwd || "";
    terminal.attentionState = result.attention_state || "";
    terminal.attentionMessage = result.attention_message || "";
    terminal.connected = !!terminal.sessionId;
    terminal.exited = false;
    terminal.statusChecked = true;
    terminal.reattaching = false;
    terminal.lastSeq = 0;
    terminal.historyInitialized = false;
    terminal.historyLoading = false;
    terminal.historyLoaded = false;
    terminal.historyStart = 0;
    terminal.historyEnd = 0;
    terminal.lastCols = size.cols;
    terminal.lastRows = size.rows;
    terminal.loading = false;
    tab.sessionId = terminal.sessionId;
    tab.processId = terminal.processId;
    tab.cwd = terminal.cwd;
    tab.attentionState = terminal.attentionState;
    tab.attentionMessage = terminal.attentionMessage;
    tab.provider = result.provider || null;
    tab.worktree = result.worktree || null;
    tab.exited = false;
    saveChatStateToStorage();
    prepareGoalTerminalHistory(tab, terminal, result.transcript_bytes);
    connectTerminalEvents(tab);
    refreshProcessesTabForChatChange();
    drawToolbar();
    focusTerminalSoon(tab);
  } catch (e) {
    terminal.loading = false;
    terminal.error = e.message || String(e);
    drawToolbar();
  }
}

async function stopTerminalSession(tab = currentToolbarTab()) {
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId;
  const terminal = terminalStateFor(tabId);
  if (!terminal?.sessionId || terminal.loading || terminal.stopping) return;
  const sessionId = terminal.sessionId;
  terminal.stopping = true;
  terminal.error = "";
  drawToolbar();
  try {
    const stopped = await api("POST", `/api/terminal/${encodeURIComponent(sessionId)}/stop`);
    if (stopped?.termination?.confirmed_exit === false) {
      throw new Error("Process termination was not confirmed.");
    }
    if (terminal.sessionId !== sessionId) return;
    finishTerminalExit(tab, terminal);
    refreshProcessesTabForChatChange();
    drawToolbar();
  } catch (e) {
    if (terminal.sessionId !== sessionId) return;
    terminal.stopping = false;
    terminal.error = e.message || String(e);
    drawToolbar();
  }
}

async function reattachTerminalSession(tab = currentToolbarTab(), existingTerminal = null) {
  if (!toolbarTabUsesTerminal(tab)) return false;
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId;
  const terminal = existingTerminal || terminalStateFor(tabId);
  if (!terminal?.sessionId) return false;
  if (terminal.stopping) return false;
  if (terminal.statusChecked) return terminal.connected && !terminal.exited;
  if (terminal.attachmentPromise) return terminal.attachmentPromise;

  terminal.reattaching = true;
  terminal.error = "";
  const sessionId = terminal.sessionId;
  terminal.attachmentPromise = (async () => {
    try {
      const status = await api(
        "GET",
        `/api/terminal/${encodeURIComponent(sessionId)}/status`,
        undefined,
        { cache: false },
      );
      if (terminal.sessionId !== sessionId || terminal.stopping) return false;
      terminal.statusChecked = true;
      terminal.reattaching = false;
      terminal.connected = !!status.alive;
      terminal.exited = !status.alive;
      terminal.processId = status.process_id || terminal.processId;
      terminal.cwd = status.cwd || terminal.cwd;
      terminal.attentionState = status.attention_state || "";
      terminal.attentionMessage = status.attention_message || "";
      tab.processId = terminal.processId;
      tab.cwd = terminal.cwd;
      tab.attentionState = terminal.attentionState;
      tab.attentionMessage = terminal.attentionMessage;
      tab.provider = status.provider || tab.provider || null;
      tab.worktree = status.worktree || tab.worktree || null;
      tab.exited = terminal.exited;
      saveChatStateToStorage();
      if (terminal.connected) {
        prepareGoalTerminalHistory(tab, terminal, status.transcript_bytes);
        connectTerminalEvents(tab);
      } else {
        terminal.eventSource?.close();
        terminal.eventSource = null;
        refreshProcessesTabForChatChange();
      }
      if (chatState.activeTabId === tabId) drawToolbar();
      return terminal.connected;
    } catch (error) {
      if (terminal.sessionId !== sessionId || terminal.stopping) return false;
      terminal.reattaching = false;
      terminal.connected = false;
      if (error?.status === 404) {
        terminal.statusChecked = true;
        terminal.exited = true;
        terminal.eventSource?.close();
        terminal.eventSource = null;
        tab.exited = true;
        saveChatStateToStorage();
      } else {
        terminal.statusChecked = false;
        terminal.error = `Unable to reattach terminal: ${error?.message || String(error)}`;
      }
      if (chatState.activeTabId === tabId) drawToolbar();
      return false;
    } finally {
      terminal.attachmentPromise = null;
    }
  })();
  return terminal.attachmentPromise;
}

function connectTerminalEvents(tab = currentToolbarTab()) {
  if (!toolbarTabUsesTerminal(tab)) return;
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId;
  const terminal = terminalStateFor(tabId);
  if (!terminal?.sessionId || terminal.exited || terminal.eventSource) return;
  const after = Math.max(0, Number(terminal.lastSeq || 0));
  const query = after ? `?after=${encodeURIComponent(after)}` : "";
  const source = new EventSource(
    `/api/terminal/${encodeURIComponent(terminal.sessionId)}/events${query}`,
  );
  terminal.eventSource = source;
  loadGoalTerminalHistory(tab, terminal);
  source.addEventListener("terminal_output", (event) => handleTerminalEvent(event, terminal));
  source.addEventListener("terminal_status", (event) => {
    try {
      const status = JSON.parse(event.data || "{}");
      terminal.attentionState = status.attention_state || "";
      terminal.attentionMessage = status.attention_message || "";
      tab.attentionState = terminal.attentionState;
      tab.attentionMessage = terminal.attentionMessage;
      saveChatStateToStorage();
      if (chatState.activeTabId === tabId) drawToolbar();
    } catch {}
  });
  source.addEventListener("terminal_error", (event) => {
    handleTerminalEvent(event, terminal);
    terminal.error = "Terminal stream error.";
    if (chatState.activeTabId === tabId) drawToolbar();
  });
  source.addEventListener("terminal_exit", (event) => {
    handleTerminalEvent(event, terminal);
    finishTerminalExit(tab, terminal);
    refreshProcessesTabForChatChange();
    if (chatState.activeTabId === tabId) drawToolbar();
  });
  source.onerror = () => {
    if (terminal.exited || terminal.eventSource !== source) return;
    // An explicit Stop can close the PTY stream before its workflow-aware
    // cancellation request finishes. Do not race that request by reattaching.
    if (terminal.stopping) return;
    terminal.error = "Terminal stream interrupted. Reconnecting…";
    terminal.connected = false;
    terminal.statusChecked = false;
    terminal.reattaching = true;
    void reattachTerminalSession(tab, terminal);
    if (chatState.activeTabId === tabId) drawToolbar();
  };
}

function finishTerminalExit(tab, terminal) {
  terminal.exited = true;
  terminal.connected = false;
  terminal.statusChecked = true;
  terminal.reattaching = false;
  terminal.loading = false;
  terminal.stopping = false;
  if (terminal.resizeTimer) clearTimeout(terminal.resizeTimer);
  terminal.resizeTimer = null;
  terminal.eventSource?.close();
  terminal.eventSource = null;
  tab.exited = true;
  saveChatStateToStorage();
}

function prepareGoalTerminalHistory(tab, terminal, transcriptBytes) {
  if (tab?.mode !== "goal" || !terminal || terminal.historyInitialized) return;
  const transcriptSize = Math.max(0, Number(transcriptBytes || 0));
  const retainedStart = Math.max(0, transcriptSize - TERMINAL_OUTPUT_MAX_CHARS);
  const tailStart = Math.max(
    retainedStart,
    transcriptSize - GOAL_TERMINAL_INITIAL_TAIL_BYTES,
  );
  terminal.historyInitialized = true;
  terminal.historyStart = retainedStart;
  terminal.historyEnd = tailStart;
  terminal.historyLoaded = retainedStart >= tailStart;
  terminal.lastSeq = Math.max(terminal.lastSeq, tailStart);
}

async function loadGoalTerminalHistory(tab, terminal) {
  if (
    tab?.mode !== "goal"
    || !terminal?.sessionId
    || !terminal.historyInitialized
    || terminal.historyLoaded
    || terminal.historyLoading
  ) {
    return;
  }
  const sessionId = terminal.sessionId;
  const historyStart = terminal.historyStart;
  const historyEnd = terminal.historyEnd;
  terminal.historyLoading = true;
  try {
    const params = new URLSearchParams({
      snapshot: "1",
      after: String(historyStart),
      before: String(historyEnd),
    });
    const snapshot = await api(
      "GET",
      `/api/terminal/${encodeURIComponent(sessionId)}/events?${params}`,
      undefined,
      { cache: false },
    );
    if (terminal.sessionId !== sessionId) return;
    const history = Array.isArray(snapshot?.events)
      ? snapshot.events.map((event) => String(event?.data || "")).join("")
      : "";
    if (history) terminalPrependOutput(history, terminal);
    terminal.historyLoaded = true;
  } catch (error) {
    if (terminal.sessionId === sessionId) {
      terminal.error = `Unable to load earlier terminal context: ${error?.message || String(error)}`;
      if (chatState.activeTabId === terminal.tabId) drawToolbar();
    }
  } finally {
    terminal.historyLoading = false;
  }
}

function handleTerminalEvent(event, terminal = terminalStateFor()) {
  if (!terminal) return;
  try {
    const payload = JSON.parse(event.data || "{}");
    const seq = Number(payload.seq || 0);
    if (seq && seq <= terminal.lastSeq) return;
    if (seq) terminal.lastSeq = seq;
    terminalReceiveOutput(payload.data || "", terminal);
  } catch {
    terminalReceiveOutput(event.data || "", terminal);
  }
}

function handleTerminalKeydown(e, terminal = terminalStateFor()) {
  if (!terminal?.sessionId || terminal.exited) return;
  if (handleTerminalClipboardKeydown(e, terminal)) return;
  const data = terminalKeyData(e);
  if (data == null) return;
  e.preventDefault();
  queueTerminalInput(data, terminal);
}

function handleTerminalClipboardKeydown(e, terminal = terminalStateFor()) {
  if (!terminal?.sessionId || terminal.exited || e.altKey) return false;
  if (e.type && e.type !== "keydown") return false;
  if (!e.ctrlKey && !e.metaKey) return false;
  const key = String(e.key || "").toLowerCase();
  if (key === "c") {
    const selection = terminalSelection(terminal);
    if (!selection) return false;
    if (writeTerminalClipboard(selection, terminal)) e.preventDefault();
    return true;
  }
  if (key !== "v") return false;
  if (readTerminalClipboard(terminal)) e.preventDefault();
  return true;
}

function terminalSelection(terminal) {
  if (!terminal?.term?.hasSelection?.()) return "";
  return terminal.term.getSelection?.() || "";
}

function handleTerminalCopy(e, terminal = terminalStateFor()) {
  if (!terminal?.sessionId || terminal.exited) return false;
  const selection = terminalSelection(terminal);
  if (!selection) return false;
  const setData = e.clipboardData?.setData;
  if (typeof setData !== "function") {
    showTerminalClipboardError(
      "copy",
      new Error("Browser copy data is unavailable."),
      terminal,
    );
    return false;
  }
  try {
    setData.call(e.clipboardData, "text/plain", selection);
  } catch (error) {
    showTerminalClipboardError("copy", error, terminal);
    return false;
  }
  e.preventDefault();
  return true;
}

function handleTerminalPaste(e, terminal = terminalStateFor()) {
  if (!terminal?.sessionId || terminal.exited) return false;
  let text;
  try {
    text = e.clipboardData?.getData("text/plain");
  } catch (error) {
    showTerminalClipboardError("paste", error, terminal);
    return false;
  }
  if (typeof text !== "string") {
    showTerminalClipboardError(
      "paste",
      new Error("Browser paste data is unavailable."),
      terminal,
    );
    return false;
  }
  if (!text) return false;
  e.preventDefault();
  queueTerminalInput(text, terminal);
  return true;
}

function writeTerminalClipboard(text, terminal) {
  const writeText = typeof navigator !== "undefined"
    ? navigator.clipboard?.writeText
    : null;
  if (typeof writeText !== "function") {
    showTerminalClipboardError(
      "copy",
      new Error("Browser clipboard write access is unavailable."),
      terminal,
    );
    return false;
  }
  try {
    Promise.resolve(writeText.call(navigator.clipboard, text))
      .catch((error) => showTerminalClipboardError("copy", error, terminal));
  } catch (error) {
    showTerminalClipboardError("copy", error, terminal);
    return false;
  }
  return true;
}

function readTerminalClipboard(terminal) {
  const readText = typeof navigator !== "undefined"
    ? navigator.clipboard?.readText
    : null;
  if (typeof readText !== "function") {
    showTerminalClipboardError(
      "paste",
      new Error("Browser clipboard read access is unavailable."),
      terminal,
    );
    return false;
  }
  const sessionId = terminal.sessionId;
  try {
    Promise.resolve(readText.call(navigator.clipboard))
      .then((text) => {
        if (
          typeof text === "string"
          && text
          && terminal.sessionId === sessionId
          && !terminal.exited
        ) {
          queueTerminalInput(text, terminal, sessionId);
        }
      })
      .catch((error) => showTerminalClipboardError("paste", error, terminal));
  } catch (error) {
    showTerminalClipboardError("paste", error, terminal);
    return false;
  }
  return true;
}

function showTerminalClipboardError(action, error, terminal) {
  const detail = error?.message || String(error || "Clipboard access failed.");
  terminal.error = `Unable to ${action} terminal text: ${detail}`;
  if (chatState.activeTabId === terminal.tabId) drawToolbar();
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

function queueTerminalInput(
  data,
  terminal = terminalStateFor(),
  sessionId = terminal?.sessionId,
) {
  if (!terminal || !data || !sessionId || terminal.sessionId !== sessionId) return;
  if (terminal.inputBuffer && terminal.inputSessionId !== sessionId) {
    if (terminal.inputFlushTimer) clearTimeout(terminal.inputFlushTimer);
    terminal.inputBuffer = "";
    terminal.inputFlushTimer = null;
  }
  terminal.inputSessionId = sessionId;
  terminal.inputBuffer += data;
  if (terminal.inputFlushTimer) return;
  terminal.inputFlushTimer = setTimeout(() => flushTerminalInput(terminal, sessionId), 12);
}

function flushTerminalInput(
  terminal = terminalStateFor(),
  sessionId = terminal?.inputSessionId || terminal?.sessionId,
) {
  if (!terminal || terminal.inputSessionId !== sessionId) return;
  const data = terminal.inputBuffer;
  terminal.inputBuffer = "";
  terminal.inputSessionId = "";
  terminal.inputFlushTimer = null;
  if (!data || terminal.sessionId !== sessionId) return;
  terminal.inputSendPromise = terminal.inputSendPromise
    .catch(() => undefined)
    .then(async () => {
      try {
        await api("POST", `/api/terminal/${encodeURIComponent(sessionId)}/input`, { data });
      } catch (e) {
        terminal.error = e.message || String(e);
        drawToolbar();
      }
    });
}

function terminalReceiveOutput(text, terminal = terminalStateFor()) {
  if (!terminal) return;
  if (text) {
    terminal.display = `${terminal.display || ""}${text}`;
    if (terminal.display.length > TERMINAL_OUTPUT_MAX_CHARS) {
      terminal.display = terminal.display.slice(-TERMINAL_OUTPUT_MAX_CHARS);
    }
  }
  if (terminal.term) {
    terminal.term.write(text || "");
  }
  // xterm keeps following output while its viewport is at the bottom and
  // preserves the viewport once the user scrolls into history. Do not force
  // the viewport back down here or incoming agent output becomes unreadable.
}

function terminalPrependOutput(text, terminal = terminalStateFor()) {
  if (!terminal || !text) return;
  terminal.display = `${text}${terminal.display || ""}`;
  if (terminal.display.length > TERMINAL_OUTPUT_MAX_CHARS) {
    terminal.display = terminal.display.slice(-TERMINAL_OUTPUT_MAX_CHARS);
  }
  if (typeof terminal.term?.reset !== "function") return;
  const replay = terminal.display;
  terminal.term.reset();
  terminal.term.write(replay, () => terminal.term?.scrollToBottom?.());
}

function ensureTerminalRenderer(output, tab = currentToolbarTab()) {
  if (!output || !window.Terminal) return;
  const terminal = terminalStateFor(
    Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId,
  );
  if (!terminal) return;
  if (terminal.term?.element) {
    if (!output.contains(terminal.term.element)) {
      output.replaceChildren(terminal.term.element);
    }
    resizeTerminalRenderer(output, terminal);
    return;
  }
  if (terminal.term) {
    terminal.term.dispose();
    terminal.term = null;
  }
  const size = terminalSize(output);
  const term = new window.Terminal({
    cols: size.cols,
    rows: size.rows,
    cursorBlink: true,
    convertEol: true,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace',
    fontSize: TERMINAL_FONT_SIZE,
    lineHeight: TERMINAL_LINE_HEIGHT,
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
  if (terminal.display) term.write(terminal.display);
  term.onData((data) => queueTerminalInput(data, terminal));
  term.attachCustomKeyEventHandler?.(
    (event) => !handleTerminalClipboardKeydown(event, terminal),
  );
  term.element?.addEventListener?.(
    "copy",
    (event) => handleTerminalCopy(event, terminal),
    true,
  );
  term.element?.addEventListener?.(
    "paste",
    (event) => handleTerminalPaste(event, terminal),
    true,
  );
  terminal.term = term;
  resizeTerminalRenderer(output, terminal);
}

function resizeTerminalRenderer(
  output = document.querySelector(".terminal-output"),
  terminal = terminalStateFor(),
) {
  if (!terminal?.term || typeof terminal.term.resize !== "function") return;
  const size = terminalSize(output, terminal);
  try {
    terminal.term.resize(size.cols, size.rows);
  } catch {}
  scheduleTerminalResize(terminal, size);
}

function scheduleTerminalResize(terminal, size) {
  if (!terminal?.connected || !terminal.sessionId) return;
  if (terminal.lastCols === size.cols && terminal.lastRows === size.rows) return;
  terminal.lastCols = size.cols;
  terminal.lastRows = size.rows;
  if (terminal.resizeTimer) clearTimeout(terminal.resizeTimer);
  terminal.resizeTimer = setTimeout(async () => {
    terminal.resizeTimer = null;
    try {
      await api("POST", `/api/terminal/${encodeURIComponent(terminal.sessionId)}/resize`, {
        cols: terminal.lastCols,
        rows: terminal.lastRows,
      });
    } catch (error) {
      terminal.error = error.message || String(error);
      if (chatState.activeTabId === terminal.tabId) drawToolbar();
    }
  }, 80);
}

function terminalSize(
  output = document.querySelector(".terminal-output"),
  terminal = terminalStateFor(),
) {
  if (!output) return { cols: 100, rows: 30 };
  const styles = window.getComputedStyle(output);
  const fontSize = parseFloat(styles.fontSize) || TERMINAL_FONT_SIZE;
  const horizontalPadding = (parseFloat(styles.paddingLeft) || 0)
    + (parseFloat(styles.paddingRight) || 0);
  const verticalPadding = (parseFloat(styles.paddingTop) || 0)
    + (parseFloat(styles.paddingBottom) || 0);
  const contentWidth = Math.max(0, output.clientWidth - horizontalPadding);
  const contentHeight = Math.max(0, output.clientHeight - verticalPadding);
  const cellWidth = terminal?.term?._core?._renderService?.dimensions?.css?.cell?.width
    || measuredTerminalCellWidth(styles, fontSize)
    || fontSize * 0.61;
  const cellHeight = terminal?.term?._core?._renderService?.dimensions?.css?.cell?.height
    || fontSize * TERMINAL_LINE_HEIGHT;
  const scrollbarWidth = terminal?.term?._core?._viewport?._scrollBarWidth || 16;
  return {
    cols: Math.max(20, Math.floor((contentWidth - scrollbarWidth) / cellWidth)),
    rows: Math.max(8, Math.floor(contentHeight / cellHeight)),
  };
}

function measuredTerminalCellWidth(styles, fontSize) {
  const canvas = document.createElement?.("canvas");
  const context = canvas?.getContext?.("2d");
  if (!context || typeof context.measureText !== "function") return 0;
  context.font = `${fontSize}px ${styles.fontFamily || "monospace"}`;
  return context.measureText("MMMMMMMMMM").width / 10;
}

function observeTerminalOutputSize(output, tab = currentToolbarTab()) {
  if (!output) return;
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab)
    || chatState.activeTabId;
  const terminal = terminalStateFor(tabId);
  if (!terminal) return;
  terminal.outputResizeObserver?.disconnect();
  terminal.observedOutput = output;
  if (typeof ResizeObserver !== "function") return;
  terminal.outputResizeObserver = new ResizeObserver(() => {
    const schedule = globalThis.requestAnimationFrame || ((callback) => callback());
    schedule(() => {
      if (terminal.observedOutput === output) resizeTerminalRenderer(output, terminal);
    });
  });
  terminal.outputResizeObserver.observe(output);
}

function scheduleActiveTerminalFit() {
  const tab = chatState.tabs[chatState.activeTabId];
  if (!toolbarTabUsesTerminal(tab)) return;
  const terminal = terminalStateFor(chatState.activeTabId);
  const output = terminal?.observedOutput || document.querySelector(".terminal-output");
  if (!terminal?.term || !output) return;
  const schedule = globalThis.requestAnimationFrame || ((callback) => callback());
  schedule(() => resizeTerminalRenderer(output, terminal));
}

function focusTerminalSoon(tab = currentToolbarTab()) {
  const tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id] === tab) || chatState.activeTabId;
  const terminal = terminalStateFor(tabId);
  const schedule = globalThis.requestAnimationFrame || ((callback) => callback());
  schedule(() => {
    const output = document.querySelector(".terminal-output");
    if (terminal?.term) {
      terminal.term.focus();
    } else if (output && document.activeElement !== output) {
      output.focus({ preventScroll: true });
    }
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
  let tabId = Object.keys(chatState.tabs).find((id) => chatState.tabs[id]?.mode === "files");
  if (!tabId) {
    tabId = nextToolbarTabId("files");
    chatState.tabs[tabId] = {
      goalId: null,
      label: nextToolbarLabel("files"),
      mode: "files",
      sessionId: null,
    };
  }
  chatState.activeTabId = tabId;
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
      if (toolbarTabUsesTerminal(currentToolbarTab())) resizeTerminalRenderer();
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

function switchChatTab(tabId) { return activateToolbarTab(tabId); }

async function closeChatTab(tabId) {
  const t = chatState.tabs[tabId];
  if (!t) return;
  const terminal = toolbarTabUsesTerminal(t) ? terminalStates.get(tabId) : null;
  const alreadyStopped = !!t.exited || !!(terminal?.statusChecked && terminal.exited);
  if (toolbarTabUsesTerminal(t) && t.sessionId && !alreadyStopped) {
    try {
      const stopped = await api("POST", `/api/terminal/${encodeURIComponent(t.sessionId)}/stop`);
      if (stopped?.termination?.confirmed_exit === false) {
        throw new Error("Process termination was not confirmed.");
      }
    } catch (error) {
      const sessionMissing = error?.code === "not_found" || error?.status === 404;
      if (!sessionMissing) {
        const currentTerminal = terminalStates.get(tabId) || terminalStateFor(tabId);
        if (currentTerminal) currentTerminal.error = error.message || String(error);
        await showActionError(error);
        drawToolbar();
        return;
      }
    }
    refreshProcessesTabForChatChange();
  }
  const currentTerminal = terminalStates.get(tabId);
  currentTerminal?.eventSource?.close();
  currentTerminal?.term?.dispose();
  terminalStates.delete(tabId);
  delete chatState.tabs[tabId];
  if (chatState.activeTabId === tabId) {
    chatState.activeTabId = Object.keys(chatState.tabs)[0] || null;
  }
  saveChatStateToStorage();
  drawChat();
}
