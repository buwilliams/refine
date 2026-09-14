// Shared custom dropdowns project the existing Node/Reporter selection controls.
function syncTopbarPickers() {
  document.querySelectorAll("[data-topbar-picker]").forEach(syncTopbarPicker);
}

function syncTopbarPicker(root) {
  const select = root.querySelector("select"), summary = root.querySelector("summary");
  const label = select.getAttribute("aria-label");
  const chosen = select.selectedOptions[0]?.textContent || `No ${label.toLowerCase()}`;
  root.querySelector("[data-picker-value]").textContent = chosen;
  summary.setAttribute("aria-label", `${label}: ${chosen}`);
  summary.setAttribute("aria-disabled", String(select.disabled || !select.options.length));
  summary.setAttribute("aria-expanded", String(root.open));
  summary.title = `${label}: ${chosen}`;
  if (select.disabled || !select.options.length) root.open = false;
  const panel = root.querySelector("[role=listbox]");
  renderInto(panel, [...select.options].map((option, index) => `<button type="button" role="option" data-picker-index="${index}" aria-selected="${option.selected}" tabindex="${option.selected ? 0 : -1}"${option.disabled ? " disabled" : ""}>${htmlEscape(option.textContent)}</button>`).join(""), () => {
    panel.querySelectorAll("button").forEach(button => {
      button.onclick = () => {
        if (select.disabled || button.disabled) return;
        select.selectedIndex = Number(button.dataset.pickerIndex);
        root.open = false;
        summary.focus();
        select.dispatchEvent(new Event("change", {bubbles: true}));
        syncTopbarPicker(root);
      };
      button.onkeydown = event => {
        const options = [...panel.querySelectorAll("button:not(:disabled)")];
        const index = options.indexOf(button);
        const next = event.key === "ArrowDown" ? (index + 1) % options.length : event.key === "ArrowUp" ? (index + options.length - 1) % options.length : event.key === "Home" ? 0 : event.key === "End" ? options.length - 1 : null;
        if (next === null) return;
        event.preventDefault(); options[next].focus();
      };
    });
  });
}

function initTopbarPickers() {
  document.querySelectorAll("[data-topbar-picker]").forEach(root => {
    if (root.dataset.bound) return;
    root.dataset.bound = "true";
    const select = root.querySelector("select"), summary = root.querySelector("summary");
    summary.onclick = event => { if (select.disabled || !select.options.length) event.preventDefault(); };
    summary.onkeydown = event => {
      if (!["ArrowDown", "ArrowUp"].includes(event.key) || select.disabled || !select.options.length) return;
      event.preventDefault();
      closeTopbarMenus(summary);
      root.open = true;
      (root.querySelector('[role="option"][aria-selected="true"]') || root.querySelector('[role="option"]'))?.focus();
    };
    root.addEventListener("keydown", event => {
      if (event.key === "Escape") { root.open = false; summary.focus(); }
    });
    root.addEventListener("toggle", () => summary.setAttribute("aria-expanded", String(root.open)));
    select.addEventListener("change", () => syncTopbarPicker(root));
    new MutationObserver(() => syncTopbarPicker(root)).observe(select, {childList: true, subtree: true, attributes: true, characterData: true});
    syncTopbarPicker(root);
  });
}
