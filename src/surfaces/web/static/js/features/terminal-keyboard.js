// Shared terminal keyboard policy. Renderer lifecycle and input transport live
// in toolbar.js; clipboard operations live in terminal-clipboard.js.
function handleTerminalCustomKeyEvent(e, terminal, tab, term) {
  return terminal.term === term && !handleTerminalClipboardKeydown(e, terminal)
    && !handleTerminalNavigationKeydown(e, terminal)
    && !handleTerminalNewlineKeydown(e, terminal)
    && !handleAgentTerminalSuspendKeydown(e, tab, terminal);
}

function handleTerminalNavigationKeydown(e, terminal = terminalStateFor()) {
  if (e.type !== "keydown" || e.isComposing || !e.altKey
      || e.ctrlKey || e.shiftKey || e.metaKey) return false;
  if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return false;
  // Use the shared clipboard focus scope, including active tab/state identity.
  if (!terminalClipboardHasFocus(terminal)) return false;
  e.preventDefault();
  // Vendored xterm maps Alt+vertical arrows to Ctrl on non-macOS platforms.
  // Preserve the modifier here and suppress xterm's second encoding. Retained
  // output consumes the gesture too, without sending to an ended session.
  const sessionId = terminal.sessionId;
  if (sessionId && !terminal.exited) {
    queueTerminalInput(e.key === "ArrowUp" ? "\x1b[1;3A" : "\x1b[1;3B", terminal, sessionId);
  }
  return true;
}

function handleTerminalKeydown(e, terminal = terminalStateFor()) {
  if (handleTerminalClipboardKeydown(e, terminal)) return;
  if (!terminal?.sessionId || terminal.exited) return;
  const data = terminalKeyData(e);
  if (data == null) return;
  e.preventDefault();
  queueTerminalInput(data, terminal);
}

function handleTerminalNewlineKeydown(e, terminal = terminalStateFor()) {
  if (!terminal?.sessionId || terminal.exited) return false;
  if (e.type && e.type !== "keydown") return false;
  if (
    e.key !== "Enter"
    || !e.ctrlKey
    || e.altKey
    || e.metaKey
  ) return false;
  e.preventDefault();
  // Browser key events can distinguish Ctrl+Enter even when xterm's legacy
  // keyboard encoding cannot. Both supported agent TUIs treat Ctrl+J (LF) as
  // an editor newline, so forward that semantic input instead of plain Enter.
  queueTerminalInput("\n", terminal);
  return true;
}

function handleAgentTerminalSuspendKeydown(
  e,
  tab = currentToolbarTab(),
  terminal = terminalStateFor(),
) {
  if (!terminal?.sessionId || terminal.exited || tab?.mode === "terminal") return false;
  if (!toolbarTabUsesTerminal(tab) || (e.type && e.type !== "keydown")) return false;
  if (
    String(e.key || "").toLowerCase() !== "z"
    || !e.ctrlKey
    || e.altKey
    || e.metaKey
  ) return false;
  // Agent sessions have no useful foreground shell to resume a stopped TUI.
  // Consume the VSUSP keystroke before xterm can emit SUB (0x1a) to the PTY;
  // ordinary Terminal tabs keep standard shell job-control behavior.
  e.preventDefault();
  return true;
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
