// ---- Chat -------------------------------------------------------------------

// chatState holds one tab per chat: the permanent "standalone" tab plus one
// per Gap that the user opened via Open Chat. Each tab carries its own
// session id, accumulated output, and closed-reason. Only the active tab is
// polled; output for other tabs accumulates server-side in the runner's
// per-session deque until the user switches to that tab.
const CHAT_TABS_STORAGE_KEY = "refine_chat_tabs";
const chatState = {
  tabs: {},                // tabId → { gapId, label, sessionId, output, closedReason }
  activeTabId: "standalone",
  pollTimer: null,
  open: false,             // dock expanded?
  bodyHeight: null,        // user-resized body height in px; null → 20vh default
  fullscreen: false,       // when true, panel fills viewport below the topbar
};

function ensureStandaloneTab() {
  if (!chatState.tabs.standalone) {
    chatState.tabs.standalone = {
      gapId: null, label: "Standalone",
      sessionId: null, output: "", closedReason: null,
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
      sessionId: t.sessionId,
      output: (t.output || "").slice(-50_000),
      closedReason: t.closedReason,
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

function defaultChatBodyHeight() {
  return Math.max(120, Math.round(window.innerHeight * 0.20));
}

function clampChatBodyHeight(px) {
  const min = 120;
  const max = Math.max(min, Math.round(window.innerHeight * 0.85));
  return Math.max(min, Math.min(max, Math.round(px)));
}

function initChatDock() {
  loadChatStateFromStorage();
  ensureStandaloneTab();
  drawChatDock();
  observeChatDockSize();
  observeTopbarHeight();
}

// Publish the topbar's actual height as --topbar-height on <html> so the
// fullscreen chat dock can anchor its top edge just below the main nav.
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

// Keep --chat-dock-height in sync with whatever vertical space the dock
// actually occupies (collapsed bar, expanded panel, or mid-drag). `body`
// reads this variable as its bottom padding so page content never slides
// underneath the dock.
function observeChatDockSize() {
  const root = $("#chat-dock");
  if (!root) return;
  const apply = () => {
    document.documentElement.style.setProperty(
      "--chat-dock-height", `${root.offsetHeight}px`,
    );
  };
  apply();
  if (typeof ResizeObserver === "function") {
    new ResizeObserver(apply).observe(root);
  } else {
    window.addEventListener("resize", apply);
  }
}

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
        sessionId: null, output: "", closedReason: null,
      };
    }
    chatState.activeTabId = gapId;
  }
  chatState.open = true;
  saveChatStateToStorage();
  drawChatDock();
  if (gapId) {
    const t = chatState.tabs[gapId];
    if (t && !t.sessionId) startGapChatSession(t);
  }
}

async function startGapChatSession(tab) {
  try {
    const r = await api("POST", "/api/chat/start", { gap_id: tab.gapId });
    tab.sessionId = r.session_id;
    tab.closedReason = null;
    saveChatStateToStorage();
    drawChatDock();
    $("#chat-input")?.focus();
  } catch (e) {
    toast("Could not start chat: " + e.message, "error");
  }
}

function toggleChatDock() {
  chatState.open = !chatState.open;
  // Collapsing the dock also exits fullscreen — leaving fullscreen on
  // while the body is hidden would orphan the topbar offset.
  if (!chatState.open) chatState.fullscreen = false;
  saveChatStateToStorage();
  drawChatDock();
}

function toggleChatFullscreen() {
  chatState.fullscreen = !chatState.fullscreen;
  if (chatState.fullscreen) chatState.open = true;  // fullscreen implies open
  saveChatStateToStorage();
  drawChatDock();
}

function drawChatDock() {
  const root = $("#chat-dock");
  if (!root) return;
  // Provider-scoped feature gate. When chat is disabled for the
  // current CLI we still keep the dock visible (it's part of the
  // layout) and the tab strip clickable, but the body shows a
  // single explanatory notice instead of the session UI so users
  // can't try to start a chat that the server will reject.
  if (!featureEnabled("chat")) {
    root.classList.toggle("open", !!chatState.open);
    const providerName = state.features?.current_provider || "the current provider";
    root.innerHTML = `
      <div class="chat-dock-bar" id="chat-dock-bar"
           title="Chat is disabled for this provider">
        <span class="chat-dock-label">Chat</span>
        <span class="muted small" style="margin-left:8px">disabled</span>
        <span class="spacer" style="flex:1"></span>
        <button class="chat-dock-toggle chat-dock-collapse" id="btn-dock-toggle"
                aria-label="${chatState.open ? "Collapse chat" : "Expand chat"}"
                title="${chatState.open ? "Collapse" : "Expand"}">▾</button>
      </div>
      <div class="chat-dock-body" style="padding:14px">
        <p class="muted">
          Chat is disabled for the <code>${htmlEscape(providerName)}</code>
          AI provider. It depends on provider session-resume support.
          Switch the provider on
          <a href="#/settings">Settings → Runtime</a>, or enable the
          override on the Runtime tab's <strong>Feature flags</strong> section
          (experimental).
        </p>
      </div>
    `;
    $("#chat-dock-bar")?.addEventListener("click", (e) => {
      if (!e.target.closest("#btn-dock-toggle") && !e.target.closest(".chat-tab")) return;
      toggleChatDock();
    });
    $("#btn-dock-toggle")?.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleChatDock();
    });
    return;
  }
  const tabs = chatState.tabs;
  const activeId = chatState.activeTabId;
  const active = tabs[activeId] || tabs.standalone;
  const hasSession = !!active.sessionId;

  const startLabel = active.gapId
    ? `Start attached to Gap ${active.gapId.slice(0, 10)}…`
    : "Start standalone";
  const toggleLabel = hasSession
    ? (active.gapId ? "Stop session" : "Stop standalone")
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
    chatState.bodyHeight = defaultChatBodyHeight();
  }
  root.innerHTML = `
    <div class="chat-dock-resize" id="chat-dock-resize"
         role="separator" aria-orientation="horizontal"
         aria-label="Resize chat panel"
         title="Drag to resize"></div>
    <div class="chat-dock-bar" id="chat-dock-bar"
         title="${chatState.open ? "Click to collapse" : "Click a tab to expand chat"}">
      <span class="chat-dock-label">Chat</span>
      <div class="chat-tabs">
        ${Object.entries(tabs).map(([id, t]) => `
          <button class="chat-tab ${id === activeId ? "active" : ""}"
                  data-tab-id="${htmlEscape(id)}"
                  title="${htmlEscape(t.gapId || "Standalone chat")}">
            ${htmlEscape(t.label)}${t.sessionId ? ` <span class="chat-tab-dot" title="active session"></span>` : ""}
            ${id === "standalone" ? "" : `<span class="chat-tab-close" data-close-tab="${htmlEscape(id)}" title="Close tab">×</span>`}
          </button>`).join("")}
      </div>
      <button class="chat-dock-toggle chat-dock-fullscreen-btn${chatState.fullscreen ? " active" : ""}"
              id="btn-dock-fullscreen"
              aria-label="${chatState.fullscreen ? "Exit fullscreen chat" : "Fullscreen chat"}"
              aria-pressed="${chatState.fullscreen ? "true" : "false"}"
              title="${chatState.fullscreen ? "Exit fullscreen" : "Fullscreen"}">⛶</button>
      <button class="chat-dock-toggle chat-dock-collapse" id="btn-dock-toggle"
              aria-label="${chatState.open ? "Collapse chat" : "Expand chat"}"
              title="${chatState.open ? "Collapse chat" : "Expand chat"}">▾</button>
    </div>
    <div class="chat-dock-body"
         style="${chatState.bodyHeight ? `height:${chatState.bodyHeight}px` : ""}">
      <div class="actions" style="margin-bottom:10px">
        <button id="btn-chat-toggle" class="${toggleClass}">${htmlEscape(toggleLabel)}</button>
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
    </div>
  `;
  applyPendingIndicator(active);

  if (chatState.open) {
    const out = $("#chat-output");
    if (out) out.scrollTop = out.scrollHeight;
  }

  $$(".chat-tab", root).forEach((el) => {
    el.addEventListener("click", (e) => {
      if (e.target.matches("[data-close-tab]")) return;
      const id = el.dataset.tabId;
      if (!id) return;
      if (id === chatState.activeTabId) {
        // Clicking the active tab toggles the dock open/closed.
        toggleChatDock();
      } else {
        switchChatTab(id);
        if (!chatState.open) {
          chatState.open = true;
          saveChatStateToStorage();
          drawChatDock();
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
  $("#btn-dock-toggle")?.addEventListener("click", toggleChatDock);
  $("#btn-dock-fullscreen")?.addEventListener("click", toggleChatFullscreen);
  $("#btn-chat-toggle")?.addEventListener("click", toggleActiveChat);
  $("#btn-chat-clear")?.addEventListener("click", clearActiveChat);
  $("#chat-input")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      sendChatLine();
    }
  });

  wireChatDockResize(root);
  restartPollForActiveTab();
}

function wireChatDockResize(root) {
  const handle = root.querySelector("#chat-dock-resize");
  const body = root.querySelector(".chat-dock-body");
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
      const next = clampChatBodyHeight(startH + (startY - ev.clientY));
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

// Back-compat alias used by helpers below; thin wrapper.
function drawChat() { drawChatDock(); }

function applyPendingIndicator(tab) {
  const ind = $("#chat-pending");
  const input = $("#chat-input");
  if (ind) ind.hidden = !tab || !tab.pending;
  if (input) input.disabled = !tab || !tab.sessionId || tab.pending;
}

function restartPollForActiveTab() {
  if (chatState.pollTimer) {
    clearInterval(chatState.pollTimer);
    chatState.pollTimer = null;
  }
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
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
  if (tabId === "standalone") return;            // never closeable
  const t = chatState.tabs[tabId];
  if (!t) return;
  if (t.sessionId) {
    try { await api("POST", `/api/chat/${t.sessionId}/stop`); } catch {}
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
    }
    t.sessionId = null;
    t.output = "";
    t.closedReason = null;
    t.pending = false;
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
      drawChat();
    });
    return;
  }
  await withButtonBusy(btn, "Starting…", async () => {
    try {
      const r = await api("POST", "/api/chat/start",
                          t.gapId ? { gap_id: t.gapId } : {});
      t.sessionId = r.session_id;
      t.closedReason = null;
      t.output = "";
      saveChatStateToStorage();
      drawChat();
      $("#chat-input")?.focus();
    } catch (e) {
      toast("Could not start chat: " + e.message, "error");
    }
  });
}

async function pollChat() {
  const t = chatState.tabs[chatState.activeTabId];
  if (!t || !t.sessionId) return;
  const sid = t.sessionId;
  try {
    const r = await api("GET", `/api/chat/${sid}/read`);
    if (r.lines && r.lines.length) {
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
    if (r.alive === false) {
      t.closedReason = r.closed_reason || "session ended";
      t.sessionId = null;
      t.pending = false;
      saveChatStateToStorage();
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
