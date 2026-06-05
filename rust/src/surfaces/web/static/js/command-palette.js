// ---- Command palette --------------------------------------------------------

let commandPaletteOpen = false;
let commandPaletteSelected = 0;

function initCommandPalette() {
  const trigger = document.getElementById("btn-command-palette");
  if (trigger) {
    trigger.title = `Command palette (${commandShortcutLabel()})`;
    const hint = trigger.querySelector("[data-command-shortcut]");
    if (hint) hint.textContent = commandShortcutLabel();
    trigger.addEventListener("click", (e) => {
      e.preventDefault();
      openCommandPalette();
    });
  }
  document.addEventListener("keydown", (e) => {
    const isShortcut = (e.ctrlKey || e.metaKey)
      && !e.altKey
      && !e.shiftKey
      && String(e.key || "").toLowerCase() === "k";
    if (!isShortcut) return;
    if (document.querySelector(".modal-backdrop:not(.command-palette-backdrop)")) {
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    openCommandPalette();
  }, true);
}

function openCommandPalette() {
  if (commandPaletteOpen) return;
  commandPaletteOpen = true;
  commandPaletteSelected = 0;
  const root = document.createElement("div");
  root.className = "modal-backdrop command-palette-backdrop";
  root.innerHTML = `
    <div class="modal command-palette" role="dialog" aria-modal="true"
         aria-labelledby="command-palette-title">
      <div class="command-palette-head">
        <div class="modal-title" id="command-palette-title">Command palette</div>
        <kbd>${htmlEscape(commandShortcutLabel())}</kbd>
      </div>
      <input type="text" id="command-palette-input"
             class="command-palette-input"
             autocomplete="off" spellcheck="false"
             placeholder="Type a command or parameters...">
      <div class="command-palette-results" id="command-palette-results"
           role="listbox" aria-label="Commands"></div>
    </div>`;
  document.body.appendChild(root);

  const input = root.querySelector("#command-palette-input");
  const results = root.querySelector("#command-palette-results");
  let currentResults = [];

  function close() {
    commandPaletteOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  }
  function draw() {
    currentResults = searchCommands(input.value);
    if (commandPaletteSelected >= currentResults.length) commandPaletteSelected = 0;
    const visible = currentResults.slice(0, 12);
    results.innerHTML = visible.length ? visible.map((item, idx) => {
      const command = item.command;
      const disabled = !item.enabled || item.params.__parseError;
      return `
        <button type="button"
                class="command-palette-row ${idx === commandPaletteSelected ? "selected" : ""}"
                data-command-index="${idx}"
                role="option"
                aria-selected="${idx === commandPaletteSelected ? "true" : "false"}"
                ${disabled ? "disabled" : ""}>
          <span class="command-palette-row-main">
            <span class="command-palette-row-title">${htmlEscape(command.title)}</span>
            ${command.description ? `<span class="command-palette-row-desc">${htmlEscape(command.description)}</span>` : ""}
          </span>
          <span class="command-palette-row-group">${htmlEscape(command.group || "")}</span>
        </button>`;
    }).join("") : `<div class="command-palette-empty">No commands found.</div>`;
    results.querySelectorAll("[data-command-index]").forEach((row) => {
      row.addEventListener("click", () => {
        const idx = Number(row.dataset.commandIndex) || 0;
        executeItem(currentResults[idx]);
      });
    });
  }
  async function executeItem(item) {
    if (!item || !item.enabled || item.params.__parseError) return;
    const inputValue = input.value;
    close();
    await runCommand(item.command.id, {
      input: inputValue,
      params: item.params,
    });
  }
  async function executeSelected() {
    await executeItem(currentResults[commandPaletteSelected]);
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      commandPaletteSelected = Math.min(currentResults.length - 1, commandPaletteSelected + 1);
      draw();
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      commandPaletteSelected = Math.max(0, commandPaletteSelected - 1);
      draw();
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      executeSelected();
    }
  }

  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  input.addEventListener("input", () => {
    commandPaletteSelected = 0;
    draw();
  });
  document.addEventListener("keydown", onKey, true);
  draw();
  input.focus();
}
