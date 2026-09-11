// Events and Skills share one revision-fenced configuration capability.
let automationEditor = null;
let automationEditorOpening = false;
let automationHistory = null;
let automationHistoryOpening = false;
let automationStatusSaving = false;
let manualSkillsGeneration = 0;
let manualSkillOpening = false;

async function loadAutomationSettings(tab) {
  const definition = await api("GET", "/api/skills");
  return { ...definition, tab };
}

function automationScopeLabel(scope) {
  return scope?.node_id ? `Node: ${scope.node_id}` : "Project";
}

function renderAutomationSettings(tab, data = {}) {
  const items = data.items || [];
  return `<section class="settings-section" data-testid="settings-${tab}">
    <div class="actions"><h3>Skills</h3><span class="spacer"></span>
      <label for="automation-list-scope">Scope</label><select id="automation-list-scope" data-automation-scope><option value="all">Project and nodes</option><option value="project">Project</option><option value="node">This node</option></select>
      <button class="secondary" data-event-history>History</button>
      <button data-automation-new>New Skill</button></div>
    <table class="table" data-testid="automation-table"><thead><tr><th>Name</th><th>Trigger</th><th>Scope</th><th>Status</th></tr></thead><tbody>
    ${items.map(item => `<tr data-automation-row data-automation-edit="${htmlEscape(item.id)}" data-scope="${htmlEscape(item.scope?.node_id || "project")}" tabindex="0" aria-label="Edit ${htmlEscape(item.name)}">
      <td>${htmlEscape(item.name)}</td><td>${htmlEscape(skillTriggerLabel(item.trigger_source))}</td><td>${htmlEscape(automationScopeLabel(item.scope))}</td>
      <td>${automationChoices(`data-automation-status="${htmlEscape(item.id)}"`, `${item.name} status`, [["true", "Enabled"], ["false", "Disabled"]], item.enabled, automationStatusSaving)}</td></tr>`).join("")}
    </tbody></table>${items.length ? "" : `<p class="muted">No Skills yet.</p>`}</section>`;
}

function bindAutomationSettings(tab, data) {
  const root = document.querySelector(`[data-testid="settings-${tab}"]`);
  if (!root) return;
  root.querySelector("[data-automation-new]").onclick = () => openSkillEditor();
  root.querySelectorAll("[data-automation-edit]").forEach(row => {
    const edit = () => openSkillEditor(data.items.find(item => item.id === row.dataset.automationEdit));
    row.onclick = event => { if (!event.target.closest("button, a, input, select")) edit(); };
    row.onkeydown = event => {
      if (event.target === row && ["Enter", " "].includes(event.key)) { event.preventDefault(); edit(); }
    };
  });
  const generation = captureNodeContextGeneration();
  root.querySelectorAll("[data-automation-status] button").forEach(button => {
    button.onclick = async event => {
      event.stopPropagation();
      const group = button.closest("[data-automation-status]");
      const item = data.items.find(item => item.id === group.dataset.automationStatus);
      const enabled = button.dataset.choice === "true";
      if (automationStatusSaving || !isNodeContextGenerationCurrent(generation) || !item || item.enabled === enabled) return;
      automationStatusSaving = true;
      setAutomationStatusBusy();
      let failure = null;
      try {
        // Omitting triggers preserves the Skill's assignments.
        const result = await api("PUT", `/api/skills/${encodeURIComponent(item.id)}`, {revision: data.revision, item: {...item, enabled}});
        data.revision = result.revision;
        Object.assign(item, result.item);
      } catch (error) { failure = error; }
      finally {
        automationStatusSaving = false;
        setAutomationStatusBusy();
      }
      if (!isNodeContextGenerationCurrent(generation)) return;
      await refreshSettings({force: true});
      await refreshManualSkills();
      if (failure) showActionError(failure);
    };
  });
  root.querySelector("[data-event-history]").onclick = () => openEventHistory();
  root.querySelector("[data-automation-scope]").onchange = event => {
    const scope = event.target.value;
    root.querySelectorAll("[data-automation-row]").forEach(row => row.hidden = scope === "project" ? row.dataset.scope !== "project" : scope === "node" ? row.dataset.scope !== nodeContextActiveNodeId() : false);
  };
}

function setAutomationStatusBusy() {
  document.querySelectorAll("[data-automation-status]").forEach(group => {
    group.setAttribute("aria-busy", String(automationStatusSaving));
    group.querySelectorAll("button").forEach(button => { button.disabled = automationStatusSaving; });
  });
}

function automationModal(title, content) {
  const priorFocus = document.activeElement;
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `<div class="modal automation-modal" role="dialog" aria-modal="true" aria-labelledby="automation-dialog-title" data-testid="automation-modal">
    <div class="modal-title" id="automation-dialog-title" tabindex="-1">${htmlEscape(title)}</div>
    <div class="modal-body">${content}<p data-automation-error role="alert" class="form-error"></p></div>
    <div class="modal-actions"><button class="danger" data-delete hidden>Delete</button><span class="spacer"></span><button class="secondary" data-close>Cancel</button><button data-save>Save</button></div></div>`;
  const close = () => { root.remove(); document.removeEventListener("keydown", onKey, true); if (automationEditor === root) automationEditor = null; if (automationHistory === root) automationHistory = null; priorFocus?.focus?.(); };
  function onKey(event) {
    if (!root.contains(event.target)) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
    if (event.key === "Tab") {
      const controls = [...root.querySelectorAll("button:not([disabled]):not([hidden]), input:not([disabled]), textarea, select, [tabindex='0']")].filter(e => e.getClientRects().length);
      const first = controls[0], last = controls.at(-1);
      if (event.shiftKey && (document.activeElement === first || !controls.includes(document.activeElement))) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  }
  document.body.appendChild(root); document.addEventListener("keydown", onKey, true);
  root.querySelector("[data-close]").onclick = close;
  root.querySelector(".modal-title").focus();
  root._close = close;
  return root;
}

function automationChoices(attribute, label, options, selected, disabled = false) {
  return `<div class="segmented-control" role="group" aria-label="${htmlEscape(label)}" data-choice-group ${attribute} data-value="${htmlEscape(String(selected))}">${options.map(([value, text]) => `<button type="button" data-choice="${htmlEscape(String(value))}" aria-pressed="${String(value) === String(selected)}" ${disabled ? "disabled" : ""}>${htmlEscape(text)}</button>`).join("")}</div>`;
}

function automationScopeControl(attribute, scope = {}) {
  const nodes = state.project?.nodes || [];
  const selected = scope.node_id || nodeContextActiveNodeId();
  const ids = [...new Set([...nodes.map(n => n.id), selected].filter(Boolean))];
  return `<div class="automation-scope" ${attribute}>
    <div class="form-row"><label>Scope</label>${automationChoices("data-scope-kind", "Scope", [["project", "Project"], ["node", "Node"]], scope.node_id ? "node" : "project")}</div>
    <div class="form-row" data-scope-node-row ${scope.node_id ? "" : "hidden"}><label>Node</label><select aria-label="Scope node" data-scope-node>${ids.map(id => `<option value="${htmlEscape(id)}" ${id === selected ? "selected" : ""}>${htmlEscape(nodes.find(n => n.id === id)?.display_name || id)}</option>`).join("")}</select></div>
  </div>`;
}

function readAutomationScope(root) {
  return {node_id: root.querySelector("[data-scope-kind]").dataset.value === "node" ? root.querySelector("[data-scope-node]").value : null};
}

function parameterRows(parameters = []) {
  return parameters.map(p => {
    const kind = p.kind || "text";
    return `<tbody data-parameter data-description="${htmlEscape(p.description || "")}"><tr>
      <td><input type="text" aria-label="Parameter name" data-name required value="${htmlEscape(p.name || "")}"></td>
      <td><select aria-label="Parameter type" data-kind>${["text", "number", "boolean", "choice"].map(k => `<option value="${k}" ${k === kind ? "selected" : ""}>${k[0].toUpperCase() + k.slice(1)}</option>`).join("")}</select></td>
      <td>${automationChoices("data-required", "Required", [["true", "Yes"], ["false", "No"]], !!p.required)}</td>
      <td data-default-cell>${parameterDefaultControl(kind, p.default, p.choices)}</td>
      <td><button type="button" class="secondary" data-remove-parameter aria-label="Remove parameter">Remove</button></td></tr>
      <tr data-choices-row ${kind === "choice" ? "" : "hidden"}><td colspan="5"><div class="form-row"><label>Choices</label><input type="text" aria-label="Choices separated by commas" data-choices placeholder="One, Two, Three" value="${htmlEscape((p.choices || []).join(", "))}"><span class="muted small">Separate choices with commas.</span></div></td></tr></tbody>`;
  }).join("");
}

function parameterDefaultControl(kind, value, choices = []) {
  const raw = value == null ? "" : String(value);
  if (kind === "boolean" || kind === "choice") {
    const options = kind === "boolean" ? ["true", "false"] : choices;
    return `<select aria-label="Default value" data-default><option value="">No default</option>${options.map(v => `<option value="${htmlEscape(v)}" ${v === raw ? "selected" : ""}>${htmlEscape(v)}</option>`).join("")}</select>`;
  }
  return `<input type="${kind === "number" ? "number" : "text"}" ${kind === "number" ? 'step="any"' : ""} aria-label="Default value" data-default value="${htmlEscape(raw)}">`;
}

function readParameters(root) {
  return [...root.querySelectorAll("[data-parameter]")].map(row => {
    const kind = row.querySelector("[data-kind]").value, raw = row.querySelector("[data-default]").value;
    let value = raw === "" ? null : raw;
    if (raw !== "" && kind === "number") { value = Number(raw); if (!Number.isFinite(value)) throw new Error("A number parameter requires a numeric default."); }
    if (raw !== "" && kind === "boolean") value = raw === "true";
    return { name: row.querySelector("[data-name]").value.trim(), kind, required: row.querySelector("[data-required]").dataset.value === "true", default: value, choices: kind === "choice" ? row.querySelector("[data-choices]").value.split(",").map(v => v.trim()).filter(Boolean) : [], description: row.dataset.description || "" };
  });
}

function skillTriggerLabel(source) {
  if (source === "custom") return "Custom";
  if (source === "node.startup.ready") return "Node starts";
  const match = source?.match(/^workflow\.(.+)\.(enter|exit|error|success)$/);
  return match ? `${match[1][0].toUpperCase() + match[1].slice(1)} ${({enter:"starts",exit:"exits",error:"error",success:"success"})[match[2]]}` : "Unconfigured";
}

let skillIdSequence = 0;
function newSkillId() {
  const browserCrypto = globalThis.crypto;
  if (typeof browserCrypto?.randomUUID === "function") return `skill-${browserCrypto.randomUUID()}`;
  // getRandomValues also works on remote HTTP pages without the secure-context UUID API.
  if (typeof browserCrypto?.getRandomValues === "function") {
    const bytes = browserCrypto.getRandomValues(new Uint8Array(16));
    return `skill-${Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("")}`;
  }
  // These are record identifiers, not credentials. Keep older browsers usable too.
  return `skill-${Date.now().toString(36)}-${(++skillIdSequence).toString(36)}-${Math.random().toString(36).slice(2)}`;
}

async function openSkillEditor(original = null, clone = false) {
  if (automationEditor || automationEditorOpening) return;
  automationEditorOpening = true;
  const generation = captureNodeContextGeneration();
  let sources, revision, item, trigger;
  const editing = !!original && !clone;
  try {
    const [catalog, current] = await Promise.all([
      api("GET", "/api/skills/catalog"),
      api("GET", original ? `/api/skills/${encodeURIComponent(original.id)}` : "/api/skills"),
    ]);
    if (!isNodeContextGenerationCurrent(generation)) return;
    sources = catalog.sources; revision = current.revision;
    item = original ? structuredClone(current.item) : {id: newSkillId(), name: "", prompt: "", enabled: true, scope: {node_id: null}, parameters: []};
    trigger = current.trigger || {source: "custom", mode: "blocking", order: 0, inputs: {}};
    if (clone) { item.id = newSkillId(); item.name += " copy"; trigger = {...trigger, id: undefined}; }
  } catch (error) { showActionError(error); return; }
  finally { automationEditorOpening = false; }
  const root = automationModal(editing ? `${item.name} — Edit Skill` : "New Skill", `<form data-automation-form novalidate>
    <div class="automation-instructions">${renderSettingsMarkdownField({id: "automation-prompt", title: "Instructions", value: item.prompt, rows: 16})}</div>
    <details class="automation-skill-settings" data-skill-settings><summary>Skill settings</summary>
    <div class="form-row"><label for="automation-name">Name</label><input type="text" id="automation-name" required value="${htmlEscape(item.name)}"></div>
    <div class="automation-fields"><div class="form-row"><label for="automation-trigger">Trigger</label><select id="automation-trigger" data-trigger-source required>${[["Manual", sources.filter(source => source === "custom")], ["Automatic", sources.filter(source => source !== "custom")]].map(([label, values]) => `<optgroup label="${label}">${values.map(source => `<option value="${htmlEscape(source)}" ${source === trigger.source ? "selected" : ""}>${htmlEscape(skillTriggerLabel(source))}</option>`).join("")}</optgroup>`).join("")}</select></div>
    ${automationScopeControl("data-scope", item.scope)}<div class="form-row"><label>Status</label>${automationChoices("data-enabled", "Status", [["true", "Enabled"], ["false", "Disabled"]], item.enabled)}</div></div>
    <p class="muted small automation-trigger-help" data-trigger-help></p>
    <section class="automation-section"><h3>Parameters <span class="muted small">— optional inputs</span></h3><div class="automation-table-scroll"><table class="table automation-parameters" data-parameters><thead><tr><th>Name</th><th>Type</th><th>Required</th><th>Default</th><th></th></tr></thead>${parameterRows(item.parameters)}</table></div><button type="button" class="secondary" data-add-parameter>Add parameter</button></section>
    <section class="automation-section" data-automatic-options><h3>Workflow options</h3><div class="form-row"><label>How it runs</label>${automationChoices("data-mode", "How it runs", [["blocking", "Required"], ["background", "Background"], ["context", "Context only"]], trigger.mode)}<span class="muted small">Required work must succeed before the workflow moves on. Context only adds instructions to other agents.</span></div><div class="form-row"><label for="automation-order">Order</label><input id="automation-order" type="number" data-order required min="-2147483648" max="2147483647" step="1" value="${trigger.order || 0}"><span class="muted small">Lower numbers run first when several Skills use this trigger.</span></div></section>
    <section class="automation-section" data-context-options><h3>Parameter context <span class="muted small">— optional</span></h3><p class="muted small">Fill an input from Goal or system context. Leave it blank to use its default or ask when run manually.</p><div data-context-inputs></div><datalist id="skill-context-sources"><option value="goal.id"><option value="goal.name"><option value="system.node_id"><option value="system.project_root"></datalist></section>
    </details>
  </form>`);
  automationEditor = root;
  const instructionField = root.querySelector("[data-settings-markdown-field]");
  const instructionEditor = root.querySelector("#automation-prompt");
  instructionEditor.dataset.prompt = "";
  instructionEditor.required = true;
  instructionEditor.setAttribute("aria-label", "Instructions in Markdown");
  bindSettingsMarkdownFields(root);
  if (!item.prompt.trim()) editSettingsMarkdownField(instructionField);
  root.querySelector("#automation-name").addEventListener("input", event => {
    root.querySelector(".modal-title").textContent = event.target.value.trim()
      ? `${event.target.value.trim()} — ${editing ? "Edit" : "New"} Skill`
      : `${editing ? "Edit" : "New"} Skill`;
  });
  root.dataset.nodeContextDirty = "false";
  const dirty = () => { root.dataset.nodeContextDirty = "true"; };
  root.addEventListener("input", dirty); root.addEventListener("change", dirty);
  const error = e => { root.querySelector("[data-automation-error]").textContent = e.message || String(e); };
  const readInputs = () => Object.fromEntries([...root.querySelectorAll("[data-input-name]")].filter(input => input.value.trim()).map(input => [input.dataset.inputName, input.value.trim()]));
  function drawContext(inputs = readInputs()) {
    const names = [...root.querySelectorAll("[data-parameter] [data-name]")].map(input => input.value.trim()).filter(Boolean);
    root.querySelector("[data-context-options]").hidden = !names.length;
    root.querySelector("[data-context-inputs]").innerHTML = names.map(name => `<div class="form-row"><label>${htmlEscape(name)}</label><input type="text" aria-label="${htmlEscape(name)} context source" data-input-name="${htmlEscape(name)}" value="${htmlEscape(inputs[name] || "")}" list="skill-context-sources" placeholder="Use default or ask when run"></div>`).join("");
  }
  function updateTrigger() {
    const source = root.querySelector("[data-trigger-source]").value;
    root.querySelector("[data-automatic-options]").hidden = source === "custom";
    root.querySelector("[data-trigger-help]").textContent = source === "custom" ? "Run from Controls → Skills or the CLI. Web runs open in an agent tab." : "Runs automatically at this point. Refine supplies the context and expected result.";
  }
  root.querySelector("[data-add-parameter]").onclick = () => {
    dirty(); root.querySelector("[data-parameters]").insertAdjacentHTML("beforeend", parameterRows([{}]));
    root.querySelector("[data-parameter]:last-child [data-name]").focus();
  };
  root.addEventListener("click", event => {
    const choice = event.target.closest("[data-choice]");
    if (choice) {
      event.preventDefault(); dirty();
      const group = choice.closest("[data-choice-group]"); group.dataset.value = choice.dataset.choice;
      group.querySelectorAll("[data-choice]").forEach(button => button.setAttribute("aria-pressed", String(button === choice)));
      if (group.matches("[data-scope-kind]")) group.closest(".automation-scope").querySelector("[data-scope-node-row]").hidden = group.dataset.value !== "node";
    }
    if (event.target.closest("[data-remove-parameter]")) { dirty(); event.target.closest("[data-parameter]").remove(); drawContext(); }
  });
  root.addEventListener("change", event => {
    if (event.target.matches("[data-kind], [data-choices]")) {
      const row = event.target.closest("[data-parameter]");
      const kind = row.querySelector("[data-kind]").value;
      const value = row.querySelector("[data-default]").value;
      const choices = row.querySelector("[data-choices]").value.split(",").map(v => v.trim()).filter(Boolean);
      row.querySelector("[data-choices-row]").hidden = kind !== "choice";
      row.querySelector("[data-default-cell]").innerHTML = parameterDefaultControl(kind, value, choices);
    }
    if (event.target.matches("[data-parameter] [data-name]")) drawContext();
    if (event.target.matches("[data-trigger-source]")) updateTrigger();
  });
  drawContext(trigger.inputs); updateTrigger();
  let saving = false;
  async function save(remove = false) {
    if (saving) return;
    if (!isNodeContextGenerationCurrent(generation)) { error(new Error("The selected project or node changed. Close this editor and reopen it.")); return; }
    const button = root.querySelector("[data-save]"); saving = true; button.disabled = true;
    try {
      const path = `/api/skills/${encodeURIComponent(item.id)}`;
      if (remove) await api("DELETE", path, {revision});
      else {
        const form = root.querySelector("form");
        root.querySelector("#automation-name").value = root.querySelector("#automation-name").value.trim();
        const invalid = form.querySelector(":invalid");
        if (invalid) {
          if (invalid === instructionEditor) editSettingsMarkdownField(instructionField);
          else root.querySelector("[data-skill-settings]").open = true;
          invalid.focus(); form.reportValidity(); return;
        }
        const scope = readAutomationScope(root.querySelector("[data-scope]"));
        const source = root.querySelector("[data-trigger-source]").value;
        const edited = {id: item.id, name: root.querySelector("#automation-name").value.trim(), prompt: root.querySelector("[data-prompt]").value, enabled: root.querySelector("[data-enabled]").dataset.value === "true", scope, parameters: readParameters(root), provenance: item.provenance || null};
        await api("PUT", path, {revision, item: edited, trigger: {id: trigger.id, source, mode: source === "custom" ? "blocking" : root.querySelector("[data-mode]").dataset.value, order: Number(root.querySelector("[data-order]").value), inputs: readInputs()}});
      }
      root._close(); await refreshSettings({force: true}); await refreshManualSkills();
    } catch (e) {
      error(e);
      if (e.status === 409) { await refreshSettings({force: true}); root.querySelector("[data-automation-error]").textContent = "Configuration changed. Your draft is retained here; close and reopen the Skill before saving."; }
    } finally { saving = false; button.disabled = false; }
  }
  root.querySelector("[data-save]").onclick = () => save();
  root.querySelector("form").onsubmit = event => { event.preventDefault(); save(); };
  if (editing) {
    root.querySelector("[data-delete]").hidden = false;
    root.querySelector("[data-delete]").onclick = () => save(true);
    const copy = document.createElement("button"); copy.type = "button"; copy.className = "secondary"; copy.dataset.cloneSkill = ""; copy.textContent = "Clone Skill";
    copy.onclick = () => { root._close(); openSkillEditor(original, true); };
    root.querySelector("[data-delete]").before(copy);
  }
}

async function refreshManualSkills() {
  const generation = ++manualSkillsGeneration;
  const nodeGeneration = captureNodeContextGeneration();
  const root = document.getElementById("nav-manual-skills");
  if (!root) return;
  const draw = skills => {
    renderInto(root, `<div class="nav-menu-label nav-context-section-label">Skills</div>${skills.map(e => `<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-manual-skill="${htmlEscape(e.id)}"><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="m8 5 11 7-11 7Z"></path></svg><span>${htmlEscape(e.name)}</span></button>`).join("")}<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-add-skill><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"></path></svg><span>Add skill...</span></button>`);
    root.querySelectorAll("[data-manual-skill]").forEach(button => button.onclick = () => { root.closest("details")?.removeAttribute("open"); triggerManualSkill(button.dataset.manualSkill); });
    root.querySelector("[data-add-skill]").onclick = () => { root.closest("details")?.removeAttribute("open"); openSkillEditor(); };
  };
  try {
    const data = await api("GET", `/api/skills?node_id=${encodeURIComponent(nodeContextActiveNodeId())}`, undefined, {recordError: false});
    if (generation !== manualSkillsGeneration || !isNodeContextGenerationCurrent(nodeGeneration)) return;
    const skills = data.items.filter(skill => skill.enabled && (data.manual_skill_ids || []).includes(skill.id));
    for (const key of commandRegistry.keys()) if (key.startsWith("skill.manual.")) commandRegistry.delete(key);
    draw(skills);
    for (const event of skills) registerCommand({id: `skill.manual.${event.id}`, title: event.name, group: "Skills", run: () => triggerManualSkill(event.id)});
  } catch (_) {
    if (generation === manualSkillsGeneration && isNodeContextGenerationCurrent(nodeGeneration)) { draw([]); for (const key of commandRegistry.keys()) if (key.startsWith("skill.manual.")) commandRegistry.delete(key); }
  }
}

async function triggerManualSkill(id) {
  if (manualSkillOpening || document.querySelector(".automation-modal")) return;
  manualSkillOpening = true;
  const generation = captureNodeContextGeneration();
  try {
    const [definition, schema] = await Promise.all([api("GET", `/api/skills/${encodeURIComponent(id)}`), api("GET", `/api/skills/${encodeURIComponent(id)}/inputs`)]);
    if (!isNodeContextGenerationCurrent(generation)) return;
    const launch = async parameters => {
      if (!isNodeContextGenerationCurrent(generation)) throw new Error("Project or node changed; launch the Skill again from its new context.");
      return createToolbarTab("skill", {label: definition.item.name, skillLaunch: {id, parameters}});
    };
    if (!schema.parameters.length) { await launch({}); return; }
    const root = automationModal(`Run ${definition.item.name}`, `<form data-event-inputs>${schema.parameters.map((p, index) => `<div class="form-row"><label for="event-parameter-${index}">${htmlEscape(p.name)}${p.required ? " *" : ""}</label>
      ${p.kind === "choice" || p.kind === "boolean" ? `<select id="event-parameter-${index}" data-parameter-index="${index}" ${p.required ? "required" : ""}><option value="">Choose…</option>${(p.kind === "boolean" ? ["true","false"] : p.choices).map(v => `<option value="${htmlEscape(v)}" ${String(p.default) === v ? "selected" : ""}>${htmlEscape(v)}</option>`).join("")}</select>` : `<input id="event-parameter-${index}" data-parameter-index="${index}" type="${p.kind === "number" ? "number" : "text"}" ${p.kind === "number" ? 'step="any"' : ""} ${p.required ? "required" : ""} value="${htmlEscape(p.default == null ? "" : String(p.default))}">`}</div>`).join("")}</form>`);
    root.querySelector("[data-save]").textContent = "Run Skill";
    const submit = async () => {
      if (!root.querySelector("form").reportValidity()) return;
      const params = {};
      root.querySelectorAll("[data-parameter-index]").forEach(input => { const p = schema.parameters[Number(input.dataset.parameterIndex)]; if (input.value !== "") params[p.name] = p.kind === "number" ? Number(input.value) : p.kind === "boolean" ? input.value === "true" : input.value; });
      root.querySelector("[data-save]").disabled = true;
      try { root._close(); await launch(params); } catch (e) { if (root.isConnected) { root.querySelector("[data-automation-error]").textContent = e.message; root.querySelector("[data-save]").disabled = false; } else showActionError(e); }
    };
    root.querySelector("[data-save]").onclick = submit; root.querySelector("form").onsubmit = e => { e.preventDefault(); submit(); };
  } catch (e) { showActionError(e); }
  finally { manualSkillOpening = false; }
}

function skillRunStatus(run) {
  const waiting = { capacity: "Waiting for agent capacity", workspace_busy: "Waiting for checkout", paused: "Automation paused" };
  return waiting[run.waiting?.reason] || run.execution_state || run.state;
}

async function openEventHistory(id = null, offset = 0, goalId = null) {
  if (automationHistoryOpening) return;
  if (automationHistory?.isConnected) { automationHistory.querySelector(".modal-title").focus(); return; }
  automationHistoryOpening = true;
  try {
    const generation = captureNodeContextGeneration();
    const data = await api("GET", id ? `/api/event-invocations/${encodeURIComponent(id)}` : `/api/event-invocations?offset=${offset}&limit=30${goalId ? `&goal_id=${encodeURIComponent(goalId)}` : ""}`);
    if (!isNodeContextGenerationCurrent(generation)) return;
    const root = automationModal(id ? data.event.name : goalId ? `Skill runs · ${goalId}` : "Skill history", id ? `<p><strong>${htmlEscape(skillRunStatus(data))}</strong></p>${data.context?.goal_id && data.gate ? `<p>Workflow gate: ${htmlEscape(data.gate)}</p>` : ""}${data.execution_state && data.execution_state !== data.state ? `<p>Originally recorded as ${htmlEscape(data.state)}; status above includes every executed Skill.</p>` : ""}${data.error ? `<p role="alert">${htmlEscape(data.error)}</p>` : ""}${Object.values(data.results).map(r => `<section><h3>${htmlEscape(r.binding_id)} · ${htmlEscape(r.outcome)}</h3><p>${htmlEscape(r.summary)}</p><pre style="white-space:pre-wrap">${htmlEscape((r.evidence || []).join("\n"))}</pre><details><summary>Result artifacts</summary><pre style="white-space:pre-wrap">${htmlEscape(JSON.stringify(r.artifacts, null, 2))}</pre></details></section>`).join("")}${(data.attempts || []).length ? `<details><summary>Execution attempts and process evidence</summary>${data.attempts.map(a => `<p>${htmlEscape(a.binding_id)} · ${a.purpose === "completion_repair" ? "Report repair" : "Work"}${a.started_at && a.received_at ? ` · ${Math.max(0, Math.round((Date.parse(a.received_at) - Date.parse(a.started_at)) / 1000))}s` : ""} · Process ${htmlEscape(a.process_id || "unavailable")}</p><pre style="white-space:pre-wrap">${htmlEscape(a.diagnostic || "")}${htmlEscape(a.raw_output || "")}</pre>`).join("")}<a href="#/settings/processes">Open Processes</a></details>` : ""}` : `<table class="table"><thead><tr><th>Run</th><th>State</th><th>Created</th></tr></thead><tbody>${data.items.map(run => `<tr><td><button class="secondary" data-open-run="${run.id}">${htmlEscape(run.event.name)}</button></td><td>${htmlEscape(skillRunStatus(run))}</td><td>${htmlEscape(run.created_at)}</td></tr>`).join("")}</tbody></table><div class="automation-pagination"><button class="secondary" data-previous ${offset ? "" : "disabled"}>Previous</button><button class="secondary" data-next ${offset + data.items.length < data.total ? "" : "disabled"}>Next</button></div>`);
    automationHistory = root;
    root.querySelector("[data-close]").textContent = "Close";
    root.querySelector("[data-save]").textContent = "Refresh";
    root.querySelector("[data-save]").onclick = () => { root._close(); openEventHistory(id, offset, goalId); };
    root.querySelectorAll("[data-open-run]").forEach(button => button.onclick = () => { root._close(); openEventHistory(button.dataset.openRun); });
    root.querySelector("[data-previous]")?.addEventListener("click", () => { root._close(); openEventHistory(null, Math.max(0, offset - 30), goalId); });
    root.querySelector("[data-next]")?.addEventListener("click", () => { root._close(); openEventHistory(null, offset + 30, goalId); });
    if (id && ["pending", "running"].includes(data.state)) { const button = root.querySelector("[data-delete]"); button.hidden = false; button.textContent = "Cancel run"; button.onclick = async () => { if (!isNodeContextGenerationCurrent(generation)) return; try { await api("POST", `/api/event-invocations/${id}/cancel`, {}); root._close(); await openEventHistory(id); } catch (e) { root.querySelector("[data-automation-error]").textContent = e.message; } }; }
  } catch (error) { showActionError(error); }
  finally { automationHistoryOpening = false; }
}

document.getElementById("nav-context-menu")?.addEventListener("toggle", event => { if (event.target.open) refreshManualSkills(); });
window.addEventListener("load", () => refreshManualSkills());
