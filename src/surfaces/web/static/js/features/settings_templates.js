// The catalog map and editor share the same project-owned Template sources.
let activeTemplatesCatalog = {};
function renderTemplatesSettings(data = {}) {
  activeTemplatesCatalog = data;
  return `<div id="template-catalog-surface">${renderTemplatesCatalog(data)}</div>`;
}
function bindTemplatesSettings() { bindTemplatesCatalog(activeTemplatesCatalog); }

async function openTemplateEditor(id, options = {}) {
  if (automationEditor || automationEditorOpening) return;
  automationEditorOpening = true;
  const generation = captureNodeContextGeneration();
  let current, catalog;
  try {
    [current, catalog] = await Promise.all([api("GET", `/api/templates/${encodeURIComponent(id)}`), api("GET", "/api/templates")]);
    if (!isNodeContextGenerationCurrent(generation)) return;
  } catch (error) { showActionError(error); return; }
  finally { automationEditorOpening = false; }
  // Keep sample input focused on the variables this template actually includes.
  const samples = {
    skill: {template: "Implement {{current_round_goal}}. Use {{refine_executable}} when needed."},
    current_round_goal: "The current Round's request",
    workflow_step: "Implement",
  };
  const values = {}, visited = new Set();
  function collectValues(source) {
    for (const match of source.matchAll(/(?<!\\){{\s*([\w.-]+)\s*}}/g)) {
      const name = match[1];
      if (visited.has(name)) continue;
      visited.add(name);
      if (name.startsWith("templates.")) {
        collectValues(catalog.items.find(row => row.item.id === name.slice(10))?.item.prompt || "");
      } else if (name !== "refine_executable") {
        values[name] = samples[name] ?? "";
        if (values[name]?.template) collectValues(values[name].template);
      }
    }
  }
  collectValues(current.item.prompt);
  Object.assign(values, options.previewValues || {});
  const tabs = {edit: "Edit", preview: "Preview", variables: "Variables", default: "Default"};
  const entries = [
    ...current.variables.map(v => ({name: v.name, label: v.name, description: v.description, group: "Variables"})),
    ...catalog.items.map(v => ({name: `templates.${v.item.id}`, label: v.name, description: v.usage?.description || `Include the ${v.name} template.`, group: v.usage?.kind === "partial" ? "Partials" : "Templates"})),
  ];
  const panels = {
    edit: `<label class="sr-only" for="template-prompt">Template content</label>
      <p class="muted template-tab-intro">Write the instructions sent to the agent. Use Variables to insert Skills, task information, or another template.</p>
      <textarea id="template-prompt" class="template-source" spellcheck="false">${htmlEscape(current.item.prompt)}</textarea>`,
    preview: `<div class="template-preview-toolbar"><p class="muted">See the complete prompt with sample values.</p><button type="button" class="secondary" data-template-preview>Refresh preview</button></div>
      <details class="template-samples"><summary>Sample values</summary><label class="sr-only" for="template-values">Sample values (JSON)</label>
        <p class="muted small">These values are for preview only. Use {"template": "…"} for text that expands its own variables.</p>
        <textarea id="template-values" class="template-source" rows="7" spellcheck="false">${htmlEscape(JSON.stringify(values, null, 2))}</textarea></details>
      <p class="muted small" data-template-preview-status role="status"></p>
      <pre class="template-output" data-template-result></pre>`,
    variables: `<p class="muted template-tab-intro">Insert a variable at your cursor. Skills and templates expand their own variables; user messages and Goal text stay literal.</p>
      <label class="sr-only" for="template-variable-search">Find a variable or template</label>
      <input type="search" id="template-variable-search" placeholder="Find a variable or template…" autocomplete="off">
      <div class="template-variable-list">${entries.map(entry => `<div class="template-variable" data-variable-search="${htmlEscape(`${entry.label} ${entry.name} ${entry.description}`.toLowerCase())}">
        <div><span class="muted small">${entry.group}</span><div><code>{{${htmlEscape(entry.name)}}}</code></div><p class="muted small">${htmlEscape(entry.description)}</p></div>
        <button type="button" class="secondary" data-template-insert="${htmlEscape(entry.name)}" aria-label="Insert ${htmlEscape(entry.label)}">Insert</button></div>`).join("")}</div>
      <p class="muted" data-template-no-matches hidden>No matching variables or templates.</p>
      <p class="muted small">Use a backslash before {{ to keep a variable as literal text.</p>`,
    default: `<p class="muted template-tab-intro">The built-in starting point for this template.</p>
      <pre class="template-output">${htmlEscape(current.default_prompt)}</pre>
      <button type="button" class="secondary" data-template-use-default>Use default in editor</button>`,
  };
  const root = automationModal(current.name, `
    ${options.description || current.usage ? `<p class="muted template-editor-usage">${htmlEscape(options.description || current.usage.description)}</p>` : ""}
    <div class="flat-tabs template-tabs" role="tablist" aria-label="Template editor">
      ${Object.entries(tabs).map(([key, label]) => `<button type="button" role="tab" id="template-tab-${key}" aria-controls="template-panel-${key}" aria-selected="${key === "edit"}" tabindex="${key === "edit" ? 0 : -1}" data-template-tab="${key}">${label}</button>`).join("")}
    </div>
    ${Object.entries(panels).map(([key, content]) => `<div class="round-tab-panel template-panel" role="tabpanel" id="template-panel-${key}" aria-labelledby="template-tab-${key}" data-template-panel="${key}"${key === "edit" ? "" : " hidden"}>${content}</div>`).join("")}`);
  automationEditor = root;
  root.querySelector(".modal").classList.add("template-editor-modal");
  root.querySelector("[data-delete]").remove();
  const editor = root.querySelector("#template-prompt");
  const previewButton = root.querySelector("[data-template-preview]");
  const saveButton = root.querySelector("[data-save]");
  saveButton.textContent = "Save template";
  root.dataset.nodeContextDirty = "false";
  editor.addEventListener("input", () => { root.dataset.nodeContextDirty = "true"; });
  const error = e => { root.querySelector("[data-automation-error]").textContent = e?.message || (e ? String(e) : ""); };
  function selectTab(key, focus = false) {
    root.querySelectorAll("[data-template-tab]").forEach(tab => {
      const active = tab.dataset.templateTab === key;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
      if (active && focus) tab.focus();
    });
    root.querySelectorAll("[data-template-panel]").forEach(panel => { panel.hidden = panel.dataset.templatePanel !== key; });
    if (key === "preview") action(previewButton, true);
  }
  root.querySelectorAll("[data-template-tab]").forEach(tab => {
    tab.onclick = () => selectTab(tab.dataset.templateTab);
    tab.onkeydown = event => {
      const keys = options.previewOnly ? ["preview"] : Object.keys(tabs), index = keys.indexOf(tab.dataset.templateTab);
      const next = event.key === "ArrowRight" ? (index + 1) % keys.length
        : event.key === "ArrowLeft" ? (index + keys.length - 1) % keys.length
        : event.key === "Home" ? 0 : event.key === "End" ? keys.length - 1 : null;
      if (next === null) return;
      event.preventDefault(); selectTab(keys[next], true);
    };
  });
  root.querySelectorAll("[data-template-insert]").forEach(button => {
    button.onclick = () => {
      editor.setRangeText(`{{${button.dataset.templateInsert}}}`, editor.selectionStart, editor.selectionEnd, "end");
      editor.dispatchEvent(new Event("input", {bubbles: true}));
      selectTab("edit"); editor.focus();
    };
  });
  root.querySelector("#template-variable-search").oninput = event => {
    const query = event.target.value.trim().toLowerCase();
    let count = 0;
    root.querySelectorAll("[data-variable-search]").forEach(row => {
      row.hidden = !row.dataset.variableSearch.includes(query);
      if (!row.hidden) count++;
    });
    root.querySelector("[data-template-no-matches]").hidden = count > 0;
  };
  root.querySelector("[data-template-use-default]").onclick = async () => {
    if (editor.value !== current.item.prompt && editor.value !== current.default_prompt
      && !await modalConfirm("Replace your draft with the built-in default?")) return;
    if (!root.isConnected) return;
    editor.value = current.default_prompt;
    editor.dispatchEvent(new Event("input", {bubbles: true}));
    selectTab("edit"); editor.focus();
  };
  let busy = false;
  async function action(button, preview) {
    if (busy) return;
    if (!isNodeContextGenerationCurrent(generation)) { error(new Error("The selected project changed. Reopen this editor before saving.")); return; }
    busy = true;
    previewButton.disabled = true;
    saveButton.disabled = true;
    error(null);
    const status = root.querySelector("[data-template-preview-status]");
    if (preview) status.textContent = "Rendering preview…";
    try {
      const body = preview ? {prompt: editor.value, values: JSON.parse(root.querySelector("#template-values").value)} : {revision: current.item.revision, prompt: editor.value};
      const result = await api(preview ? "POST" : "PUT", `/api/templates/${encodeURIComponent(id)}${preview ? "/preview" : ""}`, body);
      if (!isNodeContextGenerationCurrent(generation) || !root.isConnected) return;
      if (preview) {
        root.querySelector("[data-template-result]").textContent = result.prompt;
        status.textContent = editor.value !== body.prompt ? "Draft changed. Refresh preview to see your latest edits."
          : result.prompt ? "Preview ready" : "This template produces an empty prompt.";
      } else { root._close(); await refreshSettings({force: true}); }
    } catch (e) {
      if (preview) { status.textContent = "Preview could not be rendered."; root.querySelector("[data-template-result]").textContent = ""; }
      error(e.status === 409 ? new Error("This Template changed. Your draft is retained; reopen the Template before saving.") : e);
    } finally { busy = false; previewButton.disabled = false; saveButton.disabled = options.previewOnly || state.project?.attached === false; }
  }
  if (options.previewOnly) {
    saveButton.hidden = true;
    root.querySelector("[data-close]").textContent = "Close";
    root.querySelectorAll("[data-template-tab]").forEach(tab => { tab.hidden = tab.dataset.templateTab !== "preview"; });
  }
  if (state.project?.attached === false) {
    saveButton.disabled = true;
    saveButton.title = "Attach a project to edit Templates";
  }
  saveButton.onclick = () => action(saveButton, false);
  previewButton.onclick = () => action(previewButton, true);
  if (options.initialTab) selectTab(options.initialTab);
}
