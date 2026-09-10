// Clipboard interactions belong to the originating renderer, including retained
// output. Only paste depends on a live managed session.
function terminalSelection(terminal) {
  if (!terminal?.term?.hasSelection?.()) return "";
  return terminal.term.getSelection?.() || "";
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
  if (!terminalClipboardHasFocus(terminal) || e.altKey) return false;
  if (e.type && e.type !== "keydown") return false;
  if (!e.ctrlKey && !e.metaKey) return false;
  const key = String(e.key || "").toLowerCase();
  if (key === "c") {
    // Prevent the native shortcut as well as xterm input, even if all copy
    // methods fail. The captured text remains available for manual recovery.
    if (!copyTerminalSelection(terminal)) return false;
    e.preventDefault();
    return true;
  }
  if (key !== "v" || !terminal.sessionId || terminal.exited) return false;
  if (readTerminalClipboard(terminal)) e.preventDefault();
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
  term.onSelectionChange?.(() => {
    if (terminal.term === term) updateTerminalClipboardControls(terminal);
  });
}

function renderTerminalCopyControl(terminal) {
  const gesture = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || "")
    ? "Option-drag" : "Shift-drag";
  return `<span class="muted small" data-testid="terminal-selection-hint"
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
  if (!pasteTerminalText(text, terminal)) return false;
  e.preventDefault();
  // This listener runs during capture above xterm's textarea. Once the shared
  // terminal path accepts the paste, keep xterm from processing the same event
  // a second time after Terminal.paste has emitted its terminal-native input.
  e.stopPropagation();
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
  terminal.term.paste(text);
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
  const term = terminal.term;
  const isCurrent = () => terminalStates.get(terminal.tabId) === terminal
    && terminal.sessionId === sessionId && terminal.term === term && !terminal.exited;
  try {
    Promise.resolve(readText.call(navigator.clipboard))
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
