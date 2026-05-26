// ---- Toolbar ----------------------------------------------------------------

// chatState holds one tab per chat: the permanent "standalone" tab plus one
// per Gap that the user opened via Open Chat. Each tab carries its own
// session id, accumulated output, and closed-reason. Only the active tab is
// polled; output for other tabs accumulates server-side in the runner's
// per-session deque until the user switches to that tab.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const FILES_TAB_ID = "files";
const chatState = {
  tabs: {},                // tabId → { gapId, label, sessionId, output, closedReason }
  activeTabId: "standalone",
  pollTimer: null,
  open: false,             // dock expanded?
  bodyHeight: null,        // user-resized body height in px; null → 20vh default
  fullscreen: false,       // when true, panel fills viewport below the topbar
};
const filesState = {
  path: "",
  selectedPath: "",
  entriesByPath: {},
  expanded: new Set([""]),
  file: null,
  loading: false,
  error: "",
};

function ensureStandaloneTab() {
  if (!chatState.tabs.standalone) {
    chatState.tabs.standalone = {
      gapId: null, label: "Standalone", mode: "standalone",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false,
    };
  }
  ensureFilesTab();
}

function ensureFilesTab() {
  if (!chatState.tabs[FILES_TAB_ID]) {
    chatState.tabs[FILES_TAB_ID] = {
      gapId: null, label: "Files", mode: "files",
      sessionId: null, output: "", closedReason: null,
      agentResponded: false,
    };
  }
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
        sessionId: t.sessionId,
        output: (t.output || "").slice(-50_000),
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
  filesState.entriesByPath = {};
  filesState.expanded = new Set([""]);
  filesState.file = null;
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
function openChatDock({ gapId = null } = {}) {
  ensureStandaloneTab();
  if (gapId) {
    if (!chatState.tabs[gapId]) {
      chatState.tabs[gapId] = {
        gapId,
        label: `Gap ${gapId.slice(0, 8)}…`,
        mode: "gap",
        sessionId: null, output: "", closedReason: null, agentResponded: false,
      };
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
  ensureFilesTab();
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const active = tabs[activeId] || tabs.standalone;
  const filesActive = active.mode === "files";
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
                  title="${htmlEscape(t.mode === "files" ? "File browser" : t.gapId || "Standalone chat")}">
            ${htmlEscape(t.label)}${t.sessionId ? ` <span class="toolbar-tab-dot" title="active session"></span>` : ""}
            ${id === "standalone" || id === FILES_TAB_ID ? "" : `<span class="toolbar-tab-close" data-close-tab="${htmlEscape(id)}" title="Close tab">×</span>`}
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
      ${filesActive ? renderFilesPanel() : renderChatPanel(active, {
        toggleClass,
        toggleLabel,
        statusLine,
        hasSession,
      })}
    </div>
  `;
  if (!filesActive) applyPendingIndicator(active);
  if (filesActive) bindFilesPanel(root);

  if (chatState.open && !filesActive) {
    const out = $("#chat-output");
    if (out) out.scrollTop = out.scrollHeight;
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
  if (!filesActive) {
    $("#btn-chat-toggle")?.addEventListener("click", toggleActiveChat);
    $("#btn-plan-draft")?.addEventListener("click", draftGapsFromPlan);
    $("#btn-chat-clear")?.addEventListener("click", clearActiveChat);
    $("#chat-input")?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        sendChatLine();
      }
    });
  }

  wireToolbarResize(root);
  restartPollForActiveTab();
  if (filesActive && !filesState.entriesByPath[""] && !filesState.loading) {
    loadFilesDirectory("", { expand: true, redraw: true });
  }
}

function drawChatDock() { drawToolbar(); }

function renderChatPanel(active, { toggleClass, toggleLabel, statusLine, hasSession }) {
  return `
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-chat-toggle" class="${toggleClass}">${htmlEscape(toggleLabel)}</button>
        ${active.mode === "plan" ? `
          <button id="btn-plan-draft" class="secondary"
                  ${planHasAgentResponse(active) ? "" : "disabled"}>
            Draft Gaps
          </button>` : ""}
        <button id="btn-chat-clear" class="secondary"
                ${(active.output || active.sessionId) ? "" : "disabled"}>
          Clear history
        </button>
        <span class="spacer"></span>
        <span id="chat-status" class="muted small">${htmlEscape(statusLine)}</span>
      </div>
      <div class="chat-output-box">
        <div id="chat-output" class="chat-output">${mdToHtml(active.output || "")}</div>
        <div id="chat-pending" class="chat-pending" hidden>
          <span class="chat-pending-dots"><span></span><span></span><span></span></span>
          Agent is thinking…
        </div>
      </div>
      <div class="actions" style="margin-top:8px">
        <input type="text" id="chat-input"
               placeholder="${hasSession
                 ? "Type and press Enter…"
                 : "Click Start to begin session before sending messages is enabled."}"
               ${hasSession && !active.pending ? "" : "disabled"}>
      </div>
    `;
}

function toolbarIcon(name) {
  const icons = {
    copy: '<rect x="9" y="9" width="10" height="10" rx="2"></rect><path d="M5 15V7a2 2 0 0 1 2-2h8"></path>',
    paste: '<path d="M8 4h8v4H8z"></path><path d="M16 6h2a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h2"></path>',
    go: '<path d="M5 12h14"></path><path d="m13 6 6 6-6 6"></path>',
    refresh: '<path d="M21 12a9 9 0 0 1-15.5 6.2"></path><path d="M3 12A9 9 0 0 1 18.5 5.8"></path><path d="M3 18v-6h6"></path><path d="M21 6v6h-6"></path>',
  };
  return `<svg aria-hidden="true" viewBox="0 0 24 24" focusable="false">${icons[name] || ""}</svg>`;
}

function renderFilesPanel() {
  const inputPath = filesState.selectedPath || filesState.path || "";
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
        <input type="text" id="files-path-input"
               autocomplete="off" spellcheck="false"
               placeholder="Repo-relative path"
               value="${htmlEscape(inputPath)}">
        <button type="button" class="secondary files-icon-btn"
                data-files-copy title="Copy path" aria-label="Copy path">
          ${toolbarIcon("copy")}
        </button>
        <button type="button" class="secondary files-icon-btn"
                data-files-paste title="Paste path" aria-label="Paste path">
          ${toolbarIcon("paste")}
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
        <div class="files-tree" role="tree" aria-label="Directories and files">
          ${renderFilesTree()}
        </div>
        <div class="files-content">
          <div class="files-content-header">
            <span class="muted small">${htmlEscape(status)}</span>
          </div>
          ${renderFilesContent()}
        </div>
      </div>
    </div>`;
}

function renderFilesTree(path = "", depth = 0) {
  const entries = filesState.entriesByPath[path];
  if (!entries) {
    return depth === 0
      ? `<p class="muted small files-empty">Loading repository...</p>`
      : "";
  }
  if (!entries.length && depth === 0) {
    return `<p class="muted small files-empty">No files.</p>`;
  }
  return entries.map((entry) => {
    const isDir = entry.type === "directory";
    const expanded = isDir && filesState.expanded.has(entry.path);
    const selected = entry.path === filesState.selectedPath;
    return `
      <div class="files-tree-item ${selected ? "selected" : ""}"
           role="treeitem"
           aria-expanded="${isDir ? expanded ? "true" : "false" : ""}"
           style="--depth:${depth}"
           data-files-path="${htmlEscape(entry.path)}"
           data-files-type="${htmlEscape(entry.type)}">
        <span class="files-tree-caret" aria-hidden="true">${isDir ? expanded ? "▾" : "▸" : ""}</span>
        <span class="files-tree-name">${htmlEscape(entry.name || entry.path || ".")}</span>
      </div>
      ${isDir && expanded ? renderFilesTree(entry.path, depth + 1) : ""}`;
  }).join("");
}

function renderFilesContent() {
  const file = filesState.file;
  if (filesState.error && !file) {
    return `<div class="files-message">${htmlEscape(filesState.error)}</div>`;
  }
  if (!file) {
    return `<div class="files-message">Choose a file from the tree or enter a path.</div>`;
  }
  if (!file.previewable) {
    return `<div class="files-message">${htmlEscape(file.reason || "Preview is not available.")}</div>`;
  }
  return `
    <div class="files-source" data-language="${htmlEscape(languageForPath(file.path))}">
      ${renderSourceLines(file.content || "", file.path)}
    </div>`;
}

function renderSourceLines(content, path) {
  const lang = languageForPath(path);
  const lines = String(content ?? "").replace(/\r\n/g, "\n").split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  const shown = lines.length ? lines : [""];
  return shown.map((line, idx) => `
    <div class="files-source-line">
      <span class="files-line-number">${idx + 1}</span>
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
  root.querySelector("#files-path-input")?.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    navigateFilesPath(e.target.value);
  });
  root.querySelector("[data-files-go]")?.addEventListener("click", () => {
    navigateFilesPath(root.querySelector("#files-path-input")?.value || "");
  });
  root.querySelector("[data-files-refresh]")?.addEventListener("click", () => refreshFilesPanel());
  root.querySelector("[data-files-copy]")?.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(root.querySelector("#files-path-input")?.value || "");
      toast("Path copied", "info");
    } catch {
      toast("Clipboard copy is unavailable.", "error");
    }
  });
  root.querySelector("[data-files-paste]")?.addEventListener("click", async () => {
    try {
      const text = await navigator.clipboard.readText();
      const input = root.querySelector("#files-path-input");
      if (input) {
        input.value = text;
        input.focus();
      }
    } catch {
      toast("Clipboard paste is unavailable.", "error");
    }
  });
  $$(".files-tree-item", root).forEach((row) => {
    row.addEventListener("click", () => {
      const path = row.dataset.filesPath || "";
      const type = row.dataset.filesType || "";
      if (type === "directory") {
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

async function openFilesToolbar(options = {}) {
  ensureFilesTab();
  chatState.activeTabId = FILES_TAB_ID;
  chatState.open = true;
  saveChatStateToStorage();
  drawToolbar();
  const path = typeof options === "string"
    ? options
    : String(options.path || "");
  if (path.trim()) {
    await navigateFilesPath(path);
  } else if (!filesState.entriesByPath[""]) {
    await loadFilesDirectory("", { expand: true, redraw: true });
  }
}

async function navigateFilesPath(rawPath) {
  const path = normalizeFilesPath(rawPath);
  filesState.selectedPath = path;
  filesState.path = path;
  filesState.error = "";
  try {
    await loadFilesDirectory(path, { expand: true, redraw: false });
    drawToolbar();
  } catch (e) {
    await loadFile(path);
  }
}

async function refreshFilesPanel() {
  const dir = filesState.path || "";
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
    filesState.entriesByPath[result.path || ""] = result.entries || [];
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

async function loadFile(path, { redraw = true } = {}) {
  path = normalizeFilesPath(path);
  filesState.loading = true;
  filesState.error = "";
  if (redraw) drawToolbar();
  try {
    const file = await api("GET", `/api/files/read?path=${encodeURIComponent(path)}`);
    filesState.file = file;
    filesState.selectedPath = file.path || path;
    filesState.path = parentPath(file.path || path);
    filesState.loading = false;
    if (redraw) drawToolbar();
    return file;
  } catch (e) {
    filesState.file = null;
    filesState.loading = false;
    filesState.error = e.message || String(e);
    if (redraw) drawToolbar();
    throw e;
  }
}

function normalizeFilesPath(path) {
  return String(path || "")
    .replace(/\\/g, "/")
    .replace(/^\/+/, "")
    .replace(/\/+/g, "/")
    .replace(/\/$/, "");
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
  if (state.currentRoute !== "settings") return;
  if (typeof refreshSettings !== "function") return;
  refreshSettings().catch(() => {});
}

function applyPendingIndicator(tab) {
  const ind = $("#chat-pending");
  const input = $("#chat-input");
  if (ind) ind.hidden = !tab || !tab.pending;
  if (input) input.disabled = !tab || !tab.sessionId || tab.pending;
  syncPlanDraftButton(tab);
}

function syncPlanDraftButton(tab) {
  const btn = $("#btn-plan-draft");
  if (!btn || !tab || tab.mode !== "plan") return;
  btn.disabled = !planHasAgentResponse(tab);
}

function restartPollForActiveTab() {
  if (chatState.pollTimer) {
    clearInterval(chatState.pollTimer);
    chatState.pollTimer = null;
  }
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || t.mode === "files" || !t.sessionId) return;
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
  if (tabId === "standalone" || tabId === FILES_TAB_ID) return;
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
  if (!t.output && !t.sessionId) return;     // nothing to clear
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
}

async function pollChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
  const sid = t.sessionId;
  try {
    const r = await api("GET", `/api/chat/${sid}/read`);
    if (r.lines && r.lines.length) {
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
    // Pending state is authoritative from the runner: `in_flight` is true
    // while an agent CLI subprocess is running for this session.
    const wasPending = !!t.pending;
    t.pending = !!r.in_flight;
    if (wasPending !== t.pending) applyPendingIndicator(t);
    syncPlanDraftButton(t);
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
  await sendChatText(text);
}

async function sendChatText(text) {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId || t.pending) return;
  text = String(text || "");
  if (!text.trim()) return;
  const echo = `\n> ${text}\n`;
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
