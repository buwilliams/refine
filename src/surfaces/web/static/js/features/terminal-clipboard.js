// Clipboard interactions belong to the originating renderer, including retained
// output. Only paste depends on a live managed session.
function terminalSelection(terminal) {
  const snapshot = terminal?.selectionSnapshot;
  if (snapshot && (snapshot.term !== terminal.term || snapshot.sessionId !== terminal.sessionId)) {
    terminal.selectionSnapshot = null;
  }
  return terminal?.term?.getSelection?.() || terminal?.selectionSnapshot?.text || "";
}

// Snapshot identity is also its generation: a late copy may consume only the
// exact selection it captured. Clipboard attempts own their recovery text.
function invalidateTerminalSelection(terminal) {
  if (!terminal?.selectionSnapshot) return;
  terminal.selectionSnapshot = null;
  updateTerminalClipboardControls(terminal);
}

function resetTerminalClipboard(terminal) {
  if (!terminal) return;
  terminal.selectionSnapshot = null;
  terminal.clipboard = null;
}

function terminalPreservesCopy(terminal) {
  return !!(terminalSelection(terminal)
    || terminal?.clipboard?.pending || terminal?.clipboard?.recovery);
}

function terminalClipboardIsVisible(terminal) {
  return !!terminal && terminalStates.get(terminal.tabId) === terminal
    && !!chatState.tabs[terminal.tabId]
    && chatState.open && chatState.activeTabId === terminal.tabId;
}

function terminalClipboardHasFocus(terminal) {
  return terminalClipboardIsVisible(terminal)
    && !!terminal.term?.element?.contains(document.activeElement);
}

function handleTerminalClipboardKeydown(e, terminal = terminalStateFor()) {
  if (!terminalClipboardHasFocus(terminal)) return false;
  if (e.type && e.type !== "keydown") return false;
  const key = String(e.key || "").toLowerCase();
  const copy = !e.altKey && (((e.ctrlKey || e.metaKey) && key === "c")
    || (e.ctrlKey && !e.shiftKey && key === "insert"));
  const paste = !e.altKey && (((e.ctrlKey || e.metaKey) && key === "v")
    || (e.shiftKey && !e.ctrlKey && key === "insert"));
  if (!copy && !["shift", "control", "alt", "meta", "altgraph", "capslock", "numlock", "scrolllock"].includes(key)) {
    invalidateTerminalSelection(terminal);
  }
  if ((!copy && !paste) || (copy && !terminalSelection(terminal))) return false;
  if (paste && (!terminal.sessionId || terminal.exited)) {
    // Retained output remains copyable, but xterm must not interpret a paste
    // shortcut as control input after its managed session has ended.
    e.preventDefault();
    return true;
  }
  if (e.ctrlKey && e.shiftKey && !e.metaKey && (key === "c" || key === "v")) {
    // Terminal-specific shortcuts have no portable browser default. Cancel
    // them even on failure; failed copies retain their text for manual recovery.
    e.preventDefault();
    if (!e.repeat) {
      if (copy) copyTerminalSelection(terminal);
      else readTerminalClipboard(terminal);
    }
  }
  // Skip xterm interpretation while preserving the browser's native shortcut.
  // Its copy/paste event owns the operation without requiring async API access.
  return true;
}

function handleTerminalCopy(e, terminal = terminalStateFor()) {
  if (!terminalClipboardHasFocus(terminal)) return false;
  return copyTerminalSelection(terminal, e);
}

function bindTerminalClipboardEvents(terminal) {
  const term = terminal.term;
  term.element?.addEventListener("copy", (e) => {
    if (terminal.term === term) handleTerminalCopy(e, terminal);
  }, true);
  term.element?.addEventListener("paste", (e) => {
    if (terminal.term === term) handleTerminalPaste(e, terminal);
  }, true);
  term.element?.addEventListener("pointerdown", (e) => {
    if (terminal.term === term && e.button === 0) invalidateTerminalSelection(terminal);
  }, true);
  const captureSelection = () => {
    if (terminal.term !== term) return;
    const text = term.getSelection?.();
    if (text) terminal.selectionSnapshot = { text, term, sessionId: terminal.sessionId };
    updateTerminalClipboardControls(terminal);
  };
  term.onSelectionChange?.(captureSelection);
  // xterm can omit a nonempty selection-change event when a later gesture
  // selects the same coordinates. Capture before its mouse-up handler can
  // report input and clear that selection.
  term.element?.addEventListener("pointerup", captureSelection, true);
}

function renderTerminalCopyControl(terminal) {
  const gesture = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || "")
    ? "Option-drag" : "Shift-drag";
  return `<span class="muted small" id="terminal-selection-hint" data-testid="terminal-selection-hint"
               title="Select text even when the application captures mouse input">${gesture} to select text</span>
    <button type="button" class="secondary" data-terminal-copy data-testid="terminal-copy" data-copy-tab="${htmlEscape(terminal.tabId)}"
            ${terminalSelection(terminal) ? "" : "disabled"}>Copy selection</button>`;
}

function renderTerminalCopyFeedback(terminal) {
  const copy = terminal?.clipboard;
  return `<div class="terminal-copy-feedback" data-copy-tab="${htmlEscape(terminal.tabId)}">
    <span class="muted small" role="status" aria-live="polite" data-terminal-copy-status>${htmlEscape(copy?.message || "")}</span>
    <div class="terminal-copy-recovery" data-terminal-copy-recovery ${copy?.recovery ? "" : "hidden"}>
      <label for="terminal-copy-text">Selected terminal text</label>
      <textarea id="terminal-copy-text" data-terminal-copy-text readonly rows="3" spellcheck="false">${htmlEscape(copy?.recovery ? copy.text : "")}</textarea>
      <div>
        <button type="button" class="secondary" data-terminal-copy-select>Select text</button>
        <button type="button" class="secondary" data-terminal-copy-dismiss>Dismiss</button>
      </div>
    </div>
  </div>`;
}

function bindTerminalClipboardControls(root) {
  const button = root.querySelector("[data-terminal-copy]");
  // Preserve mouse selection and focus; keyboard activation remains native.
  bindOnce(button, "mousedown", (e) => e.preventDefault());
  bindOnce(button, "click", () => copyTerminalSelection(terminalStates.get(button.dataset.copyTab)));
  bindOnce(root.querySelector("[data-terminal-copy-select]"), "click", () => {
    const field = root.querySelector("[data-terminal-copy-text]");
    field?.focus({ preventScroll: true });
    field?.select();
  });
  bindOnce(root.querySelector("[data-terminal-copy-dismiss]"), "click", () => {
    const tabId = root.querySelector(".terminal-copy-feedback")?.dataset.copyTab;
    const terminal = terminalStates.get(tabId);
    if (!terminalClipboardIsVisible(terminal)) return;
    terminal.clipboard = null;
    updateTerminalClipboardControls(terminal);
    terminal.term?.focus();
  });
  updateTerminalClipboardControls(terminalStateFor());
}

function updateTerminalClipboardControls(terminal) {
  flushTerminalHistoryReplay(terminal);
  if (!terminalClipboardIsVisible(terminal)) return;
  const root = document.querySelector("#toolbar-dock");
  const button = root?.querySelector("[data-terminal-copy]");
  if (button) button.disabled = !terminalSelection(terminal);
  const copy = terminal.clipboard;
  const status = root?.querySelector("[data-terminal-copy-status]");
  if (status) status.textContent = copy?.message || "";
  const recovery = root?.querySelector("[data-terminal-copy-recovery]");
  if (recovery) recovery.hidden = !copy?.recovery;
  const field = root?.querySelector("[data-terminal-copy-text]");
  const text = copy?.recovery ? copy.text : "";
  if (field && field.value !== text) field.value = text;
  // No Toolbar redraw: keep the renderer, focus, and current selection intact.
  scheduleActiveTerminalFit();
}

function copyTerminalSelection(terminal, event = null) {
  const text = terminalSelection(terminal);
  if (!text || !terminalClipboardIsVisible(terminal)) return false;
  const copy = {
    text, tab: chatState.tabs[terminal.tabId], term: terminal.term, sessionId: terminal.sessionId,
    selectionSnapshot: terminal.selectionSnapshot,
    focus: document.activeElement, message: "Copying selection…", recovery: false, pending: true,
  };
  terminal.clipboard = copy;
  if (event) {
    event.preventDefault();
    event.stopPropagation();
    try {
      if (typeof event.clipboardData?.setData === "function") {
        event.clipboardData.setData("text/plain", text);
        finishTerminalCopy(terminal, copy);
        return true;
      }
    } catch (_) {
      // Continue through the write API and the browser's selection fallback.
    }
  }
  updateTerminalClipboardControls(terminal);
  try {
    const clipboard = typeof navigator !== "undefined" ? navigator.clipboard : null;
    if (typeof clipboard?.writeText !== "function") {
      recoverTerminalCopy(terminal, copy);
      return true;
    }
    Promise.resolve(clipboard.writeText(text)).then(
      () => finishTerminalCopy(terminal, copy),
      () => recoverTerminalCopy(terminal, copy),
    );
  } catch (_) {
    recoverTerminalCopy(terminal, copy);
  }
  return true;
}

function terminalCopyIsCurrent(terminal, copy) {
  return terminalStates.get(terminal.tabId) === terminal
    && chatState.tabs[terminal.tabId] === copy.tab
    && terminal.clipboard === copy && terminal.term === copy.term
    && terminal.sessionId === copy.sessionId;
}

function finishTerminalCopy(terminal, copy) {
  if (!terminalCopyIsCurrent(terminal, copy)) return;
  if (terminal.selectionSnapshot === copy.selectionSnapshot) terminal.selectionSnapshot = null;
  copy.message = "Selection copied.";
  copy.recovery = false;
  copy.pending = false;
  updateTerminalClipboardControls(terminal);
}

function recoverTerminalCopy(terminal, copy) {
  if (!terminalCopyIsCurrent(terminal, copy)) return;
  // A rejected async write must never focus a temporary field in another tab
  // or steal focus from a control the user moved to while awaiting permission.
  if (terminalClipboardIsVisible(terminal) && document.activeElement === copy.focus
      && copyTerminalTextWithSelection(copy.text)) {
    finishTerminalCopy(terminal, copy);
    return;
  }
  copy.recovery = true;
  copy.pending = false;
  copy.message = "Automatic copy was blocked. Select the text below, then use your browser’s Copy command (Ctrl+C or Cmd+C).";
  updateTerminalClipboardControls(terminal);
}

function copyTerminalTextWithSelection(text) {
  if (typeof document.execCommand !== "function") return false;
  const focus = document.activeElement;
  const caret = typeof focus?.selectionStart === "number" ? {
    start: focus.selectionStart, end: focus.selectionEnd, direction: focus.selectionDirection,
  } : null;
  const selection = window.getSelection?.();
  const ranges = [];
  for (let i = 0; i < (selection?.rangeCount || 0); i += 1) {
    ranges.push(selection.getRangeAt(i).cloneRange());
  }
  const field = document.createElement("textarea");
  field.value = text;
  field.readOnly = true;
  field.style.cssText = "position:fixed;left:-10000px;top:0;";
  document.body.appendChild(field);
  try {
    field.focus({ preventScroll: true });
    field.select();
    return document.execCommand("copy") === true;
  } catch (_) {
    return false;
  } finally {
    field.remove();
    focus?.focus({ preventScroll: true });
    if (selection) {
      selection.removeAllRanges();
      ranges.forEach((range) => selection.addRange(range));
    }
    // Text controls expose a collapsed DOM range. Restoring their caret can
    // replace a real DOM selection, so only do so when no text range was saved.
    if (caret && !ranges.some((range) => !range.collapsed)) {
      focus.setSelectionRange(caret.start, caret.end, caret.direction);
    }
  }
}

function handleTerminalPaste(e, terminal = terminalStateFor()) {
  if (!terminalClipboardHasFocus(terminal)) return false;
  // Own empty and rejected events too, so xterm cannot duplicate the operation
  // or bypass a failed clipboard read through its own paste listener.
  e.preventDefault();
  e.stopPropagation();
  if (!terminal.sessionId || terminal.exited) return true;
  try {
    const text = e.clipboardData?.getData("text/plain");
    if (typeof text !== "string") throw new Error("Browser paste data is unavailable.");
    pasteTerminalText(text, terminal);
  } catch (error) {
    showTerminalClipboardError("paste", error, terminal);
  }
  return true;
}

function pasteTerminalText(
  text,
  terminal = terminalStateFor(),
  sessionId = terminal?.sessionId,
) {
  if (
    typeof text !== "string"
    || !text
    || !terminal
    || terminalStates.get(terminal.tabId) !== terminal
    || terminal.exited
    || !sessionId
    || terminal.sessionId !== sessionId
    || typeof terminal.term?.paste !== "function"
  ) return false;
  // Let xterm normalize line endings and honor the PTY application's
  // bracketed-paste mode. Agent TUIs use that framing to preserve multiline
  // content as one editable prompt rather than submitting embedded lines.
  invalidateTerminalSelection(terminal);
  terminal.term.paste(text);
  return true;
}

function readTerminalClipboard(terminal) {
  const sessionId = terminal.sessionId;
  const term = terminal.term;
  const tab = chatState.tabs[terminal.tabId];
  const isCurrent = () => terminalClipboardHasFocus(terminal)
    && chatState.tabs[terminal.tabId] === tab
    && terminal.sessionId === sessionId && terminal.term === term && !terminal.exited;
  try {
    const clipboard = typeof navigator !== "undefined" ? navigator.clipboard : null;
    if (typeof clipboard?.readText !== "function") {
      throw new Error("Browser clipboard read access is unavailable.");
    }
    Promise.resolve(clipboard.readText())
      .then((text) => {
        if (
          typeof text === "string"
          && text
          && isCurrent()
        ) {
          pasteTerminalText(text, terminal, sessionId);
        }
      })
      .catch((error) => {
        if (isCurrent()) showTerminalClipboardError("paste", error, terminal);
      });
  } catch (error) {
    if (isCurrent()) showTerminalClipboardError("paste", error, terminal);
    return false;
  }
  return true;
}

function showTerminalClipboardError(action, error, terminal) {
  const detail = error?.message || String(error || "Clipboard access failed.");
  terminal.error = `Unable to ${action} terminal text: ${detail} Use the browser Copy/Paste menu or Ctrl/Cmd+C/V.`;
  if (chatState.activeTabId === terminal.tabId) drawToolbar();
}
