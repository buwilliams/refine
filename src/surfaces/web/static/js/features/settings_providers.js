// Provider drafts live in a modal, outside the periodically refreshed Runtime pane.
let providerDefaultDraft = null;
function renderProviderCatalog(data = {}) {
  const catalog = data.catalog;
  if (!catalog) return "";
  if (providerDefaultDraft && !isNodeContextGenerationCurrent(providerDefaultDraft.generation)) providerDefaultDraft = null;
  const defaultId = providerDefaultDraft?.id || catalog.default_provider;
  return `<section class="settings-section" data-testid="provider-catalog">
    <h3>Shared AI providers</h3>
    <p class="muted">These definitions and the system default are shared across nodes. Each host supplies its own executable and authentication.</p>
    <label for="provider-system-default">System default</label>
    <select id="provider-system-default">${catalog.providers.map(p => `<option value="${htmlEscape(p.id)}" ${p.id === defaultId ? "selected" : ""}>${htmlEscape(p.name)}</option>`).join("")}</select>
    <button type="button" id="provider-save-default">Save default</button>
    <div>${catalog.providers.map(p => `<p><strong>${htmlEscape(p.name)}</strong> <code>${htmlEscape(p.executable)}</code> <button type="button" data-provider-edit="${htmlEscape(p.id)}">Edit</button></p>`).join("")}</div>
    <button type="button" id="provider-add">Add CLI provider</button>
    <p class="muted small">Example arguments: <code>["--prompt", "{{context}}"]</code>. Each JSON string is one argument; no shell is used.</p>
    <p role="alert" id="provider-catalog-error"></p>
  </section>`;
}

function bindProviderCatalog(data = {}) {
  if (!data.catalog) return;
  document.querySelectorAll("[data-provider-edit]").forEach(button => {
    button.onclick = () => openProviderEditor(data.catalog, button.dataset.providerEdit);
  });
  const add = document.querySelector("#provider-add");
  if (add) add.onclick = () => openProviderEditor(data.catalog);
  const select = document.querySelector("#provider-system-default");
  if (select) select.onchange = () => {
    providerDefaultDraft = {id: select.value, generation: captureNodeContextGeneration(), revision: data.catalog.revision};
  };
  const save = document.querySelector("#provider-save-default");
  if (save) save.onclick = async () => {
    const catalog = structuredClone(data.catalog);
    catalog.default_provider = document.querySelector("#provider-system-default").value;
    catalog.revision = providerDefaultDraft?.revision ?? catalog.revision;
    save.disabled = true;
    try {
      await api("PUT", "/api/providers", catalog);
      providerDefaultDraft = null;
      await refreshSettingsTab("runtime", {force: true});
    } catch (error) { document.querySelector("#provider-catalog-error").textContent = error.message; }
    finally { save.disabled = false; }
  };
}

function providerModeEditor(name, mode) {
  const text = (key, value) => `<label for="provider-${name}-${key}">${key.replaceAll("_", " ")}</label><textarea id="provider-${name}-${key}" rows="3" spellcheck="false">${htmlEscape(JSON.stringify(value, null, 2))}</textarea>`;
  return `<fieldset><legend>${name === "automated" ? "Automated launches" : "Interactive terminal launches"}</legend>
    ${text("args", mode.args || [])}${text("context_args", mode.context_args || [])}
    <p class="muted small">Arguments run in order; context arguments follow when context is present.</p>
    <details><summary>Transport and session settings</summary>
      <label for="provider-${name}-transport">Prompt transport</label>
      <select id="provider-${name}-transport"><option value="inline_or_file">Arguments with large-context file fallback</option>${name === "automated" ? `<option value="native_stdin" ${mode.transport === "native_stdin" ? "selected" : ""}>Native stdin</option>` : ""}</select>
      ${text("stdin", mode.stdin ?? null)}${text("cwd_on_resume", mode.cwd_on_resume ?? true)}${text("cwd_args", mode.cwd_args || [])}${text("resume_args", mode.resume_args ?? null)}${text("pin_args", mode.pin_args ?? null)}
      <p class="muted small">stdin is a JSON string or null. Native stdin uses "{{context}}" here. Resume arguments replace base arguments; pin arguments precede them. {{session_id}} is available in session templates. {{cwd}} is available for automated launches with an explicit working directory.</p>
    </details></fieldset>`;
}

function readProviderMode(root, name) {
  const read = key => JSON.parse(root.querySelector(`#provider-${name}-${key}`).value);
  return {cwd_on_resume: read("cwd_on_resume"), args: read("args"), context_args: read("context_args"), cwd_args: read("cwd_args"),
    resume_args: read("resume_args"), pin_args: read("pin_args"), stdin: read("stdin"),
    transport: root.querySelector(`#provider-${name}-transport`).value};
}

function openProviderEditor(catalog, id) {
  if (automationEditor) return;
  const generation = captureNodeContextGeneration();
  const draft = structuredClone(catalog);
  const provider = draft.providers.find(p => p.id === id) || {
    id: "", name: "", executable: "", output_format: "plain",
    automated: {context_args: ["{{context}}"]}, interactive: {context_args: ["{{context}}"]},
  };
  const input = (key, label) => `<label for="provider-${key}">${label}</label><input id="provider-${key}" value="${htmlEscape(provider[key])}" ${key === "id" && id ? "readonly" : ""}>`;
  const root = automationModal(id ? `Edit ${provider.name}` : "Add CLI provider", `
    <p>Shared provider definition. Changes apply to subsequent launches.</p>
    ${input("id", "Stable provider ID")}${input("name", "Display name")}${input("executable", "Executable path or PATH name")}
    ${providerModeEditor("automated", provider.automated)}${providerModeEditor("interactive", provider.interactive)}
    <label for="provider-output">Output format</label><select id="provider-output">${["plain", "claude_json", "codex_json", "copilot_json"].map(format => `<option ${format === provider.output_format ? "selected" : ""}>${format}</option>`).join("")}</select>`);
  automationEditor = root;
  root.addEventListener("input", () => { root.dataset.nodeContextDirty = "true"; });
  root.addEventListener("change", () => { root.dataset.nodeContextDirty = "true"; });
  const save = root.querySelector("[data-save]"), remove = root.querySelector("[data-delete]");
  remove.hidden = !id;
  async function submit(deleting) {
    if (!isNodeContextGenerationCurrent(generation)) {
      root.querySelector("[data-automation-error]").textContent = "The project or node changed. Reopen the editor before saving.";
      return;
    }
    save.disabled = remove.disabled = true;
    try {
      const updated = structuredClone(draft);
      if (deleting) updated.providers = updated.providers.filter(p => p.id !== id);
      else {
        const record = {id: root.querySelector("#provider-id").value, name: root.querySelector("#provider-name").value,
          executable: root.querySelector("#provider-executable").value, output_format: root.querySelector("#provider-output").value,
          automated: readProviderMode(root, "automated"), interactive: readProviderMode(root, "interactive")};
        const index = updated.providers.findIndex(p => p.id === id);
        if (index < 0) updated.providers.push(record); else updated.providers[index] = record;
      }
      await api("PUT", "/api/providers", updated);
      root._close();
      await refreshSettingsTab("runtime", {force: true});
    } catch (error) {
      root.querySelector("[data-automation-error]").textContent = error.status === 409
        ? "The catalog changed or this provider is still selected. Your draft is retained. Reload the catalog before retrying conflicting edits."
        : error.message;
    } finally { save.disabled = remove.disabled = false; }
  }
  save.onclick = () => submit(false);
  remove.onclick = () => submit(true);
}
