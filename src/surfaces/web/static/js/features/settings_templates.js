// System-owned template identities, with project-owned editable content.
function renderTemplatesSettings(data = {}) {
  const primary = ["workflow", "planning-agent", "agent", "goal-agent"];
  const rows = [...(data.items || [])].sort((a, b) => {
    const rank = id => primary.includes(id) ? primary.indexOf(id) : primary.length;
    return rank(a.item.id) - rank(b.item.id) || a.name.localeCompare(b.name);
  });
  return `<section class="settings-section" data-testid="settings-templates">
    <h3>Templates</h3><p class="muted">Control the context and instructions Refine sends to agents. Variables insert Skills and task information.</p>
    <table class="table"><thead><tr><th>Name</th><th>Content</th></tr></thead><tbody>${rows.map(row => `<tr tabindex="0" data-template-id="${htmlEscape(row.item.id)}" aria-label="Edit ${htmlEscape(row.name)}"><td>${htmlEscape(row.name)}</td><td>${row.customized ? "Customized" : "Default"}</td></tr>`).join("")}</tbody></table></section>`;
}

function bindTemplatesSettings() {
  document.querySelectorAll("[data-template-id]").forEach(row => {
    row.onclick = () => openTemplateEditor(row.dataset.templateId);
    row.onkeydown = event => { if (["Enter", " "].includes(event.key)) { event.preventDefault(); openTemplateEditor(row.dataset.templateId); } };
  });
}

async function openTemplateEditor(id) {
  if (automationEditor || automationEditorOpening) return;
  automationEditorOpening = true;
  const generation = captureNodeContextGeneration();
  let current, catalog;
  try {
    [current, catalog] = await Promise.all([api("GET", `/api/templates/${encodeURIComponent(id)}`), api("GET", "/api/templates")]);
    if (!isNodeContextGenerationCurrent(generation)) return;
  } catch (error) { showActionError(error); return; }
  finally { automationEditorOpening = false; }
  const values = Object.fromEntries(current.variables.filter(v => v.name !== "refine_executable").map(v => [v.name, ""]));
  values.skill = {template: "Implement {{current_round_goal}}. Use {{refine_executable}} when needed."};
  values.current_round_goal = "The current Round's request";
  values.workflow_step = "implement";
  const root = automationModal(`${current.name} — Edit Template`, `
    ${renderSettingsMarkdownField({id: "template-prompt", title: "Template", value: current.item.prompt, rows: 16})}
    <details><summary>Variables and templates</summary><p class="muted">Skills and referenced templates expand their variables. Messages, Goal text, and other task data remain literal. Use a backslash before {{ to show a literal variable.</p>
    <div class="form-row"><label for="template-insert">Insert variable or template</label><select id="template-insert"><option value="">Choose…</option><optgroup label="Variables">${current.variables.map(v => `<option value="${htmlEscape(v.name)}">${htmlEscape(v.name)} — ${htmlEscape(v.description)}</option>`).join("")}</optgroup><optgroup label="Templates">${catalog.items.map(v => `<option value="templates.${htmlEscape(v.item.id)}">${htmlEscape(v.name)}</option>`).join("")}</optgroup></select></div></details>
    <details><summary>Preview with sample values</summary><label for="template-values">Sample values (JSON)</label><textarea id="template-values" rows="8" style="width:100%">${htmlEscape(JSON.stringify(values, null, 2))}</textarea><button type="button" class="secondary" data-template-preview>Render preview</button><pre data-template-result style="white-space:pre-wrap;overflow-wrap:anywhere" aria-live="polite"></pre></details>
    <details><summary>Built-in default</summary><pre style="white-space:pre-wrap">${htmlEscape(current.default_prompt)}</pre></details>`);
  automationEditor = root;
  root.querySelector("[data-delete]").remove();
  root.querySelector("[data-settings-markdown-edit]").remove();
  root.querySelector("[data-settings-markdown-preview]").hidden = true;
  const editor = root.querySelector("#template-prompt");
  editor.hidden = false;
  root.dataset.nodeContextDirty = "false";
  root.addEventListener("input", () => { root.dataset.nodeContextDirty = "true"; });
  const error = e => { root.querySelector("[data-automation-error]").textContent = e.message || String(e); };
  root.querySelector("#template-insert").onchange = event => {
    if (!event.target.value) return;
    editor.setRangeText(`{{${event.target.value}}}`, editor.selectionStart, editor.selectionEnd, "end");
    editor.dispatchEvent(new Event("input", {bubbles: true})); editor.focus(); event.target.value = "";
  };
  let busy = false;
  async function action(button, preview) {
    if (busy) return;
    if (!isNodeContextGenerationCurrent(generation)) { error(new Error("The selected project changed. Reopen this editor before saving.")); return; }
    busy = true;
    button.disabled = true;
    try {
      const body = preview ? {prompt: editor.value, values: JSON.parse(root.querySelector("#template-values").value)} : {revision: current.item.revision, prompt: editor.value};
      const result = await api(preview ? "POST" : "PUT", `/api/templates/${encodeURIComponent(id)}${preview ? "/preview" : ""}`, body);
      if (!isNodeContextGenerationCurrent(generation) || !root.isConnected) return;
      if (preview) root.querySelector("[data-template-result]").textContent = result.prompt;
      else { root._close(); await refreshSettings({force: true}); }
    } catch (e) { error(e.status === 409 ? new Error("This Template changed. Your draft is retained; reopen the Template before saving.") : e); }
    finally { busy = false; button.disabled = false; }
  }
  if (state.project?.attached === false) {
    root.querySelector("[data-save]").disabled = true;
    root.querySelector("[data-save]").title = "Attach a project to edit Templates";
  }
  root.querySelector("[data-save]").onclick = event => action(event.currentTarget, false);
  root.querySelector("[data-template-preview]").onclick = event => action(event.currentTarget, true);
}
