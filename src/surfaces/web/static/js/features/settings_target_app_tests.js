// ---- Shared target-app test command settings -------------------------------

function targetAppTestCommandsFromSettings(settings = {}) {
  return targetAppTestCommandsFromValue(settings.target_app_test_commands, settings.target_app_test_command);
}

function targetAppTestCommandsFromValue(value, fallbackCommand = "") {
  let items = [];
  const raw = String(value || "").trim();
  if (raw) {
    try {
      const parsed = JSON.parse(raw);
      if (Array.isArray(parsed)) items = parsed;
    } catch (_) {
      items = [{ command: raw, enabled: true }];
    }
  }
  if (!items.length && String(fallbackCommand || "").trim()) {
    items = [{ command: String(fallbackCommand).trim(), enabled: true }];
  }
  const seen = new Set();
  return items.flatMap((item) => {
    const command = String(item?.command || (typeof item === "string" ? item : "") || "").trim();
    if (!command || seen.has(command)) return [];
    seen.add(command);
    return [{ command, enabled: item?.enabled !== false }];
  });
}

function targetAppTestCommandsValue(commands) {
  const normalized = (commands || [])
    .map((item) => ({
      command: String(item.command || "").trim(),
      enabled: item.enabled !== false,
    }))
    .filter((item) => item.command);
  return normalized.length ? JSON.stringify(normalized) : "";
}

function targetAppTestCommandsSummary(commands) {
  const total = commands.length;
  const enabled = commands.filter((item) => item.enabled !== false).length;
  if (!total) return "No target-app test commands configured.";
  return `${enabled} enabled of ${total} target-app test command${total === 1 ? "" : "s"}`;
}

function renderTargetAppTestCommandsPreview(commands) {
  if (!commands.length) {
    return `<span class="settings-editable-none">No target-app test commands configured.</span>`;
  }
  return `
    <div class="target-test-command-preview-list">
      ${commands.map((item) => `
        <div class="target-test-command-preview-row ${item.enabled === false ? "is-disabled" : ""}">
          <span class="status-pill ${item.enabled === false ? "muted" : "qa"}">${item.enabled === false ? "disabled" : "enabled"}</span>
          <code>${htmlEscape(item.command)}</code>
        </div>`).join("")}
    </div>`;
}

function renderTargetAppTestCommandRows(commands) {
  if (!commands.length) {
    return `<p class="muted small" data-target-test-empty>No commands configured.</p>`;
  }
  return commands.map((item, index) => `
    <div class="target-test-command-row" data-target-test-command-row>
      <label class="target-test-command-enabled">
        <input type="checkbox"
               data-target-test-enabled
               ${item.enabled === false ? "" : "checked"}
               aria-label="Enable test command ${index + 1}">
        Enabled
      </label>
      <input type="text"
             data-target-test-command
             ${index === 0 ? "data-settings-editable-focus" : ""}
             data-testid="target-app-test-command-${index + 1}"
             placeholder="npm test"
             value="${htmlEscape(item.command || "")}">
      <button type="button"
              class="secondary"
              data-target-test-remove
              aria-label="Remove test command ${index + 1}">Remove</button>
    </div>`).join("");
}

function renderTargetAppTestCommandsField(settings = {}, options = {}) {
  const commands = targetAppTestCommandsFromSettings(settings);
  const value = targetAppTestCommandsValue(commands);
  const id = options.id || "s-target-test-commands";
  const title = options.label || "Target-app tests";
  const description = options.description || "CLI commands Refine runs for workflow QA.";
  const previewClass = commands.length === 1 ? " target-test-command-preview-single" : "";
  return `
    <div class="form-row settings-editable-field target-test-commands-field"
         data-settings-editable-field
         data-target-test-command-field
         data-settings-editable-title="${htmlEscape(title)}"
         data-settings-empty-label="No target-app test commands configured.">
      <div class="settings-editable-heading">
        <label for="${htmlEscape(id)}">${renderSettingsGuideLabel(title, options.guideItemId || "application-test", description)}</label>
        <button type="button"
                class="secondary settings-editable-toggle"
                title="Edit ${htmlEscape(title)}"
                aria-label="Edit ${htmlEscape(title)}"
                data-testid="${htmlEscape(id)}-edit"
                data-settings-editable-toggle>
          ${settingsMarkdownIcon("edit")}
        </button>
      </div>
      <div class="settings-editable-preview${previewClass}" data-settings-editable-preview>
        ${renderTargetAppTestCommandsPreview(commands)}
      </div>
      <div class="settings-editable-editor" data-settings-editable-editor hidden>
        <textarea id="${htmlEscape(id)}"
                  data-settings-editable-value
                  data-testid="target-app-test-commands"
                  hidden>${htmlEscape(value)}</textarea>
        <div class="target-test-command-list" data-target-test-command-list>
          ${renderTargetAppTestCommandRows(commands)}
        </div>
        <div class="actions">
          <button type="button" class="secondary" data-target-test-add>Add command</button>
        </div>
      </div>
    </div>`;
}

function bindTargetAppTestCommandList(root = document) {
  $$("[data-target-test-command-field]", root).forEach((field) => {
    const value = field.querySelector("[data-settings-editable-value]");
    const list = field.querySelector("[data-target-test-command-list]");
    const add = field.querySelector("[data-target-test-add]");
    if (!value || !list || !add) return;

    const commandsFromRows = () => $$("[data-target-test-command-row]", list).map((row) => ({
      command: row.querySelector("[data-target-test-command]")?.value || "",
      enabled: row.querySelector("[data-target-test-enabled]")?.checked !== false,
    })).filter((item) => String(item.command || "").trim());

    const syncValue = () => {
      const commands = commandsFromRows();
      value.value = targetAppTestCommandsValue(commands);
      value.dataset.settingsPreviewValue = targetAppTestCommandsSummary(commands);
      value.dataset.settingsPreviewHtml = renderTargetAppTestCommandsPreview(commands);
      updateSettingsEditablePreview(field);
      syncTargetAppTestCommandsPreviewSizing(field, commands);
    };

    const bindRows = () => {
      $$("[data-target-test-enabled], [data-target-test-command]", list).forEach((control) => {
        control.addEventListener("input", syncValue);
        control.addEventListener("change", syncValue);
      });
      $$("[data-target-test-remove]", list).forEach((button) => {
        button.addEventListener("click", () => {
          button.closest("[data-target-test-command-row]")?.remove();
          if (!list.querySelector("[data-target-test-command-row]")) {
            list.innerHTML = renderTargetAppTestCommandRows([]);
          }
          syncValue();
        });
      });
    };

    add.addEventListener("click", () => {
      const commands = commandsFromRows();
      commands.push({ command: "", enabled: true });
      list.innerHTML = renderTargetAppTestCommandRows(commands);
      bindRows();
      list.querySelector("[data-target-test-command-row]:last-child [data-target-test-command]")?.focus();
      syncValue();
    });

    const initial = targetAppTestCommandsFromValue(value.value);
    value.value = targetAppTestCommandsValue(initial);
    value.dataset.settingsPreviewValue = targetAppTestCommandsSummary(initial);
    value.dataset.settingsPreviewHtml = renderTargetAppTestCommandsPreview(initial);
    bindRows();
    updateSettingsEditablePreview(field);
    syncTargetAppTestCommandsPreviewSizing(field, initial);
  });
}

function syncTargetAppTestCommandsPreviewSizing(field, commands) {
  const preview = field?.querySelector("[data-settings-editable-preview]");
  if (!preview) return;
  preview.classList.toggle("target-test-command-preview-single", commands.length === 1);
}

async function autosaveSettingsTargetAppTests(root = document) {
  const commands = root.querySelector("#s-target-test-commands");
  if (!commands) return;
  await api("PATCH", "/api/settings", {
    target_app_test_commands: commands.value,
  });
}
