// Events and Skills share one revision-fenced configuration capability.
let automationEditor = null;
let automationEditorOpening = false;
let customEventsGeneration = 0;

async function loadAutomationSettings(tab) {
  const definition = await api("GET", tab === "skills" ? "/api/skills" : "/api/event-definitions");
  return { ...definition, tab };
}

function automationScopeLabel(scope) {
  return scope?.node_id ? `Node: ${scope.node_id}` : "Project";
}

function renderAutomationSettings(tab, data = {}) {
  const isSkill = tab === "skills";
  const items = (data.items || []).filter(item => isSkill || item.kind === "custom");
  return `<section class="settings-section" data-testid="settings-${tab}">
    <div class="actions"><h3>${isSkill ? "Skills" : "Events"}</h3><span class="spacer"></span>
      <label for="automation-list-scope">Scope</label><select id="automation-list-scope" data-automation-scope><option value="all">Project and nodes</option><option value="project">Project</option><option value="node">This node</option></select>
      <button data-automation-new>New ${isSkill ? "Skill" : "Event"}</button>
      ${isSkill ? "" : '<button class="secondary" data-event-history>Execution history</button>'}</div>
    <p class="muted">${isSkill ? "Reusable agent instructions. Open a Skill to edit its instructions and assign the system or custom Events that trigger it." : "Custom Events that users can run from Controls, the command palette, or the CLI. Assign Events to Skills in the Skills tab."}</p>
    <table class="table" data-testid="automation-table"><thead><tr><th>Name</th>${isSkill ? "<th>Result role</th>" : ""}<th>Scope</th><th>Status</th></tr></thead><tbody>
    ${items.map(item => `<tr data-automation-row data-automation-edit="${htmlEscape(item.id)}" data-scope="${htmlEscape(item.scope?.node_id || "project")}" tabindex="0" aria-label="Edit ${htmlEscape(item.name)}">
      <td>${htmlEscape(item.name)}</td>${isSkill ? `<td>${htmlEscape(item.role)}</td>` : ""}<td>${htmlEscape(automationScopeLabel(item.scope))}</td>
      <td>${item.enabled ? "Enabled" : "Disabled"}</td></tr>`).join("")}
    </tbody></table>${items.length ? "" : `<p class="muted">No ${isSkill ? "Skills" : "custom Events"} yet.</p>`}</section>`;
}

function bindAutomationSettings(tab, data) {
  const root = document.querySelector(`[data-testid="settings-${tab}"]`);
  if (!root) return;
  root.querySelector("[data-automation-new]").onclick = () => openAutomationEditor(tab);
  root.querySelectorAll("[data-automation-edit]").forEach(row => {
    const edit = () => openAutomationEditor(tab, data.items.find(item => item.id === row.dataset.automationEdit));
    row.onclick = event => { if (!event.target.closest("button, a, input, select")) edit(); };
    row.onkeydown = event => {
      if (event.target === row && ["Enter", " "].includes(event.key)) { event.preventDefault(); edit(); }
    };
  });
  root.querySelector("[data-event-history]")?.addEventListener("click", () => openEventHistory());
  root.querySelector("[data-automation-scope]").onchange = event => {
    const scope = event.target.value;
    root.querySelectorAll("[data-automation-row]").forEach(row => row.hidden = scope === "project" ? row.dataset.scope !== "project" : scope === "node" ? row.dataset.scope !== nodeContextActiveNodeId() : false);
  };
}

function automationModal(title, content) {
  const priorFocus = document.activeElement;
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `<div class="modal automation-modal" role="dialog" aria-modal="true" aria-labelledby="automation-dialog-title" data-testid="automation-modal">
    <div class="modal-title" id="automation-dialog-title" tabindex="-1">${htmlEscape(title)}</div>
    <div class="modal-body">${content}<p data-automation-error role="alert" class="form-error"></p></div>
    <div class="modal-actions"><button class="danger" data-delete hidden>Delete</button><span class="spacer"></span><button class="secondary" data-close>Cancel</button><button data-save>Save</button></div></div>`;
  const close = () => { root.remove(); document.removeEventListener("keydown", onKey, true); if (automationEditor === root) automationEditor = null; priorFocus?.focus?.(); };
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

function automationChoices(attribute, label, options, selected) {
  return `<div class="segmented-control" role="group" aria-label="${htmlEscape(label)}" data-choice-group ${attribute} data-value="${htmlEscape(String(selected))}">${options.map(([value, text]) => `<button type="button" data-choice="${htmlEscape(String(value))}" aria-pressed="${String(value) === String(selected)}">${htmlEscape(text)}</button>`).join("")}</div>`;
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

async function openAutomationEditor(tab, original = null) {
  if (automationEditor || automationEditorOpening) return;
  automationEditorOpening = true;
  const generation = captureNodeContextGeneration();
  const isSkill = tab === "skills";
  let catalog, events, revision, item;
  try {
    const [sources, eventList, skillList] = await Promise.all([
      api("GET", "/api/event-definitions/catalog"), api("GET", "/api/event-definitions"),
      isSkill ? api("GET", "/api/skills") : Promise.resolve(null),
    ]);
    if (!isNodeContextGenerationCurrent(generation)) return;
    if (isSkill && eventList.revision !== skillList.revision) throw new Error("Configuration changed while opening the editor. Open it again to use the latest definitions.");
    catalog = sources; events = eventList.items; revision = eventList.revision;
    item = original ? (isSkill ? skillList.items : events).find(value => value.id === original.id) : {
      id: `${isSkill ? "skill" : "event"}-${crypto.randomUUID()}`, name: "", prompt: "", role: "task", kind: "custom", source: null,
      enabled: true, scope: {node_id: null}, parameters: [], bindings: [], on_success: null,
    };
    if (!item) throw new Error("This definition was removed. Refresh the list before editing.");
    item = JSON.parse(JSON.stringify(item));
  } catch (error) { showActionError(error); return; }
  finally { automationEditorOpening = false; }
  const root = automationModal(`${original ? "Edit" : "New"} ${isSkill ? "Skill" : "Event"}`, `<form data-automation-form>
    <div class="form-row"><label for="automation-name">Name</label><input type="text" id="automation-name" required data-name value="${htmlEscape(item.name)}"></div>
    <div class="automation-field-pair">${automationScopeControl("data-scope", item.scope)}<div class="form-row"><label>Status</label>${automationChoices("data-enabled", "Status", [["true", "Enabled"], ["false", "Disabled"]], item.enabled)}</div></div>
    ${isSkill ? `<div class="form-row"><label>Result role</label>${automationChoices("data-role", "Result role", catalog.roles.map(role => [role, role[0].toUpperCase() + role.slice(1)]), item.role)}</div><div class="form-row"><label for="automation-prompt">Instructions</label><textarea id="automation-prompt" rows="9" required data-prompt>${htmlEscape(item.prompt)}</textarea></div>` : '<p class="muted small">This custom Event can be run from Controls or the command palette. Assign it to a Skill in the Skills tab.</p>'}
    <section class="automation-section"><h3>Parameters</h3><div class="automation-table-scroll"><table class="table automation-parameters" data-parameters><thead><tr><th>Name</th><th>Type</th><th>Required</th><th>Default</th><th></th></tr></thead>${parameterRows(item.parameters)}</table></div><button type="button" class="secondary" data-add-parameter>Add parameter</button></section>
    ${isSkill ? '<section class="automation-section"><h3>Events</h3><p class="muted small">Choose the Events that trigger this Skill. Order controls when it runs relative to other Skills on the same Event; lower numbers run first.</p><div data-bindings></div><button type="button" class="secondary" data-add-binding>Add event</button></section>' : `<div class="form-row"><label for="automation-success">After all blocking Skills succeed</label><select id="automation-success" data-success><option value="">No additional action</option>${["start", "accept", "retry", "reopen"].map(a => `<option value="${a}" ${a === item.on_success ? "selected" : ""}>${a[0].toUpperCase() + a.slice(1)}</option>`).join("")}</select></div>`}
    </form>`);
  automationEditor = root;
  root.dataset.nodeContextDirty = "false";
  const dirty = () => { root.dataset.nodeContextDirty = "true"; };
  root.addEventListener("input", dirty);
  root.addEventListener("change", dirty);
  const error = e => { root.querySelector("[data-automation-error]").textContent = e.message || String(e); };
  root.querySelector("[data-add-parameter]").onclick = () => {
    dirty(); root.querySelector("[data-parameters]").insertAdjacentHTML("beforeend", parameterRows([{}]));
    root.querySelector("[data-parameter]:last-child [data-name]").focus();
  };
  root.addEventListener("click", event => {
    const choice = event.target.closest("[data-choice]");
    if (choice) {
      dirty(); const group = choice.closest("[data-choice-group]"); group.dataset.value = choice.dataset.choice;
      group.querySelectorAll("[data-choice]").forEach(button => button.setAttribute("aria-pressed", String(button === choice)));
      if (group.matches("[data-scope-kind]")) group.closest(".automation-scope").querySelector("[data-scope-node-row]").hidden = group.dataset.value !== "node";
    }
    if (event.target.closest("[data-remove-parameter]")) {
      dirty(); event.target.closest("[data-parameter]").remove();
      if (isSkill) drawBindings(readBindings());
    }
    if (event.target.closest("[data-remove-binding]")) { dirty(); event.target.closest("[data-binding]").remove(); }
  });
  function updateParameterDefault(row) {
    const kind = row.querySelector("[data-kind]").value;
    const value = row.querySelector("[data-default]").value;
    const choices = row.querySelector("[data-choices]").value.split(",").map(v => v.trim()).filter(Boolean);
    row.querySelector("[data-choices-row]").hidden = kind !== "choice";
    row.querySelector("[data-default-cell]").innerHTML = parameterDefaultControl(kind, value, choices);
  }
  root.addEventListener("change", event => {
    if (event.target.matches("[data-kind], [data-choices]")) updateParameterDefault(event.target.closest("[data-parameter]"));
  });
  function readBindings() {
    return [...root.querySelectorAll("[data-binding]")].map(row => ({event_id: row.querySelector("[data-event]").value, binding: {
      id: row.dataset.binding, skill_id: item.id, enabled: row.querySelector("[data-binding-enabled]").dataset.value === "true",
      mode: row.querySelector("[data-mode]").dataset.value, order: Number(row.querySelector("[data-order]").value),
      scope: readAutomationScope(row.querySelector("[data-binding-scope]")), overrides: row.querySelector("[data-override]").value || null,
      inputs: Object.fromEntries([...row.querySelectorAll("[data-input-name]")].filter(input => input.value).map(input => [input.dataset.inputName, input.value.trim()])),
    }}));
  }
  function eventOptions(selected) {
    return [["System events", events.filter(e => e.kind === "system")], ["Custom events", events.filter(e => e.kind === "custom")]].map(([label, items]) => `<optgroup label="${label}">${items.map(e => `<option value="${htmlEscape(e.id)}" ${selected === e.id ? "selected" : ""}>${htmlEscape(e.name)}${e.scope?.node_id ? ` · ${htmlEscape(automationScopeLabel(e.scope))}` : ""}${e.enabled ? "" : " (disabled)"}</option>`).join("")}</optgroup>`).join("");
  }
  function drawBindings(assignments) {
    const parameters = [...root.querySelectorAll("[data-parameter] [data-name]")].map(input => input.value.trim()).filter(Boolean);
    root.querySelector("[data-bindings]").innerHTML = assignments.map(({event_id, binding: b}) => {
      const event = events.find(e => e.id === event_id);
      const projectBindings = [...(event?.bindings || []).filter(other => other.skill_id !== item.id), ...assignments.filter(a => a.event_id === event_id).map(a => a.binding)].filter(other => other.id !== b.id && !other.scope?.node_id);
      return `<fieldset class="automation-binding" data-binding="${htmlEscape(b.id)}"><legend>Event assignment</legend>
        <div class="form-row"><label>Event</label><select aria-label="Event" data-event required>${eventOptions(event_id)}</select></div>
        <div class="form-row"><label>Execution</label>${automationChoices("data-mode", "Execution", [["blocking", "Blocking"], ["background", "Background"], ["context", "Context"]], b.mode)}</div>
        <div class="automation-field-pair"><div class="form-row"><label>Order</label><input type="number" aria-label="Order" data-order required min="-2147483648" max="2147483647" step="1" value="${b.order || 0}"></div><div class="form-row"><label>Status</label>${automationChoices("data-binding-enabled", "Assignment status", [["true", "Enabled"], ["false", "Disabled"]], b.enabled)}</div></div>
        ${automationScopeControl("data-binding-scope", b.scope)}
        <div class="form-row"><label>Override project assignment</label><select aria-label="Override project assignment" data-override><option value="">Additional assignment</option>${projectBindings.map(other => `<option value="${htmlEscape(other.id)}" ${b.overrides === other.id ? "selected" : ""}>${htmlEscape(other.skill_id === item.id ? item.name || "This Skill" : other.skill_id)} · ${htmlEscape(other.id)}</option>`).join("")}</select></div>
        <div data-binding-inputs>${parameters.map(name => `<div class="form-row"><label>${htmlEscape(name)} context source</label><input type="text" aria-label="${htmlEscape(name)} context source" data-input-name="${htmlEscape(name)}" value="${htmlEscape(b.inputs?.[name] || "")}" placeholder="goal.id, system.node_id, or event.${htmlEscape(name)}" list="event-context-sources"></div>`).join("")}</div>
        <button type="button" class="secondary" data-remove-binding>Remove event</button></fieldset>`;
    }).join("") + '<datalist id="event-context-sources"><option value="goal.id"><option value="goal.name"><option value="system.node_id"><option value="system.project_root"><option value="system.candidate_commit"></datalist>';
    root.querySelectorAll("[data-event]").forEach(select => select.onchange = () => {
      const assignments = readBindings();
      const changed = assignments.find(a => a.binding.id === select.closest("[data-binding]").dataset.binding);
      changed.binding.overrides = null;
      drawBindings(assignments);
    });
  }
  if (isSkill) {
    drawBindings(events.flatMap(event => event.bindings.filter(binding => binding.skill_id === item.id).map(binding => ({event_id: event.id, binding}))));
    root.querySelector("[data-add-binding]").onclick = () => {
      if (!events.length) { error(new Error("Create an Event before assigning it to a Skill.")); return; }
      dirty();
      const event = events.find(e => e.source === `workflow.${root.querySelector("[data-role]").dataset.value}.enter`) || events[0];
      const scope = readAutomationScope(root.querySelector("[data-scope]"));
      drawBindings([...readBindings(), {event_id: event.id, binding: {id: `binding-${crypto.randomUUID()}`, skill_id: item.id, mode: "blocking", order: Math.max(-1, ...event.bindings.map(b => b.order)) + 1, enabled: true, scope, inputs: {}}}]);
    };
    root.addEventListener("change", event => { if (event.target.matches("[data-parameter] [data-name]")) drawBindings(readBindings()); });
  }
  let saving = false;
  async function save(remove = false) {
    if (saving) return;
    if (!isNodeContextGenerationCurrent(generation)) { error(new Error("The selected project or node changed. Close this editor and reopen it.")); return; }
    const button = root.querySelector("[data-save]"); saving = true; button.disabled = true;
    try {
      const path = `/api/${isSkill ? "skills" : "event-definitions"}/${encodeURIComponent(item.id)}`;
      if (remove) await api("DELETE", path, {revision});
      else {
        if (!root.querySelector("form").reportValidity()) return;
        const edited = {id: item.id, name: root.querySelector("#automation-name").value.trim(), enabled: root.querySelector("[data-enabled]").dataset.value === "true", scope: readAutomationScope(root.querySelector("[data-scope]")), parameters: readParameters(root)};
        if (isSkill) Object.assign(edited, {prompt: root.querySelector("[data-prompt]").value, role: root.querySelector("[data-role]").dataset.value, provenance: item.provenance || null});
        else Object.assign(edited, {kind: item.kind, source: item.source, bindings: item.bindings, on_success: root.querySelector("[data-success]").value || null});
        await api("PUT", path, {revision, item: edited, ...(isSkill ? {event_bindings: readBindings()} : {})});
      }
      root._close(); await refreshSettings({force: true}); await refreshCustomEvents();
    } catch (e) {
      error(e);
      if (e.status === 409) { await refreshSettings({force: true}); root.querySelector("[data-automation-error]").textContent = "Configuration changed. Your draft is retained here; close and reopen the refreshed definition before saving."; }
    } finally { saving = false; button.disabled = false; }
  }
  root.querySelector("[data-save]").onclick = () => save();
  root.querySelector("form").onsubmit = event => { event.preventDefault(); save(); };
  if (original) { root.querySelector("[data-delete]").hidden = false; root.querySelector("[data-delete]").onclick = () => save(true); }
}

async function refreshCustomEvents() {
  const generation = ++customEventsGeneration;
  const nodeGeneration = captureNodeContextGeneration();
  const root = document.getElementById("nav-custom-events");
  if (!root) return;
  const draw = events => {
    renderInto(root, `<div class="nav-menu-label nav-context-section-label">Events</div>${events.map(e => `<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-custom-event="${htmlEscape(e.id)}"><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="m8 5 11 7-11 7Z"></path></svg><span>${htmlEscape(e.name)}</span></button>`).join("")}<button class="nav-menu-item nav-control-item nav-management-item" type="button" data-add-event><svg class="nav-menu-icon" aria-hidden="true" viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"></path></svg><span>Add event...</span></button>`);
    root.querySelectorAll("[data-custom-event]").forEach(button => button.onclick = () => { root.closest("details")?.removeAttribute("open"); triggerCustomEvent(button.dataset.customEvent); });
    root.querySelector("[data-add-event]").onclick = () => { root.closest("details")?.removeAttribute("open"); openAutomationEditor("events"); };
  };
  try {
    const data = await api("GET", `/api/event-definitions?node_id=${encodeURIComponent(nodeContextActiveNodeId())}`, undefined, {recordError: false});
    if (generation !== customEventsGeneration || !isNodeContextGenerationCurrent(nodeGeneration)) return;
    const events = data.items.filter(e => e.kind === "custom" && e.enabled);
    for (const key of commandRegistry.keys()) if (key.startsWith("event.custom.")) commandRegistry.delete(key);
    draw(events);
    for (const event of events) registerCommand({id: `event.custom.${event.id}`, title: event.name, group: "Events", run: () => triggerCustomEvent(event.id)});
  } catch (_) {
    if (generation === customEventsGeneration && isNodeContextGenerationCurrent(nodeGeneration)) { draw([]); for (const key of commandRegistry.keys()) if (key.startsWith("event.custom.")) commandRegistry.delete(key); }
  }
}

async function triggerCustomEvent(id, selectedGoalId = null) {
  const generation = captureNodeContextGeneration();
  const goalId = selectedGoalId || state.currentGoal || null;
  try {
    const [definition, schema] = await Promise.all([api("GET", `/api/event-definitions/${encodeURIComponent(id)}`), api("GET", `/api/event-definitions/${encodeURIComponent(id)}/inputs${goalId ? `?goal_id=${encodeURIComponent(goalId)}` : ""}`)]);
    if (!isNodeContextGenerationCurrent(generation)) return;
    if (definition.item.on_success && !goalId) {
      const goalModal = automationModal(`Run ${definition.item.name}`, '<form><div class="form-row"><label for="event-goal-id">Goal ID</label><input type="text" id="event-goal-id" data-goal-id required placeholder="Select the Goal this Event will act on"></div></form>');
      goalModal.querySelector("[data-save]").textContent = "Continue";
      const choose = () => { if (!goalModal.querySelector("form").reportValidity()) return; const chosen = goalModal.querySelector("[data-goal-id]").value.trim(); goalModal._close(); triggerCustomEvent(id, chosen); };
      goalModal.querySelector("[data-save]").onclick = choose;
      goalModal.querySelector("form").onsubmit = e => { e.preventDefault(); choose(); };
      return;
    }
    const requestId = crypto.randomUUID();
    const launch = async parameters => {
      if (!isNodeContextGenerationCurrent(generation)) throw new Error("Project or node changed; launch the Event again from its new context.");
      const result = await api("POST", `/api/event-definitions/${encodeURIComponent(id)}/trigger`, {goal_id: goalId, node_id: nodeContextActiveNodeId(), parameters, request_id: requestId});
      toast(`Event queued: ${definition.item.name}`, "info"); return result;
    };
    if (!schema.parameters.length) { const result = await launch({}); await openEventHistory(result.id); return; }
    const root = automationModal(`Run ${definition.item.name}`, `<form data-event-inputs>${schema.parameters.map((p, index) => `<div class="form-row"><label for="event-parameter-${index}">${htmlEscape(p.name)}${p.required ? " *" : ""}</label>
      ${p.kind === "choice" || p.kind === "boolean" ? `<select id="event-parameter-${index}" data-parameter-index="${index}" ${p.required ? "required" : ""}><option value="">Choose…</option>${(p.kind === "boolean" ? ["true","false"] : p.choices).map(v => `<option value="${htmlEscape(v)}" ${String(p.default) === v ? "selected" : ""}>${htmlEscape(v)}</option>`).join("")}</select>` : `<input id="event-parameter-${index}" data-parameter-index="${index}" type="${p.kind === "number" ? "number" : "text"}" ${p.kind === "number" ? 'step="any"' : ""} ${p.required ? "required" : ""} value="${htmlEscape(p.default == null ? "" : String(p.default))}">`}</div>`).join("")}</form>`);
    root.querySelector("[data-save]").textContent = "Run Event";
    const submit = async () => {
      if (!root.querySelector("form").reportValidity()) return;
      const params = {};
      root.querySelectorAll("[data-parameter-index]").forEach(input => { const p = schema.parameters[Number(input.dataset.parameterIndex)]; if (input.value !== "") params[p.name] = p.kind === "number" ? Number(input.value) : p.kind === "boolean" ? input.value === "true" : input.value; });
      root.querySelector("[data-save]").disabled = true;
      try { const result = await launch(params); root._close(); await openEventHistory(result.id); } catch (e) { root.querySelector("[data-automation-error]").textContent = e.message; root.querySelector("[data-save]").disabled = false; }
    };
    root.querySelector("[data-save]").onclick = submit; root.querySelector("form").onsubmit = e => { e.preventDefault(); submit(); };
  } catch (e) { showActionError(e); }
}

async function openEventHistory(id = null, offset = 0, goalId = null) {
  const generation = captureNodeContextGeneration();
  const data = await api("GET", id ? `/api/event-invocations/${encodeURIComponent(id)}` : `/api/event-invocations?offset=${offset}&limit=30${goalId ? `&goal_id=${encodeURIComponent(goalId)}` : ""}`);
  if (!isNodeContextGenerationCurrent(generation)) return;
  const root = automationModal(id ? data.event.name : goalId ? `Event agents · ${goalId}` : "Event execution history", id ? `<p><strong>${htmlEscape(data.state)}</strong></p>${data.error ? `<p role="alert">${htmlEscape(data.error)}</p>` : ""}${Object.values(data.results).map(r => `<section><h3>${htmlEscape(r.binding_id)} · ${htmlEscape(r.outcome)}</h3><p>${htmlEscape(r.summary)}</p><pre style="white-space:pre-wrap">${htmlEscape((r.evidence || []).join("\n"))}</pre><details><summary>Result artifacts</summary><pre style="white-space:pre-wrap">${htmlEscape(JSON.stringify(r.artifacts, null, 2))}</pre></details></section>`).join("")}${(data.attempts || []).length ? `<details><summary>Execution attempts and process evidence</summary>${data.attempts.map(a => `<p>${htmlEscape(a.binding_id)} · Process ${htmlEscape(a.process_id || "unavailable")}</p><pre style="white-space:pre-wrap">${htmlEscape(a.diagnostic || "")}${htmlEscape(a.raw_output || "")}</pre>`).join("")}<a href="#/settings/processes">Open Processes</a></details>` : ""}` : `<table class="table"><thead><tr><th>Event</th><th>State</th><th>Created</th></tr></thead><tbody>${data.items.map(run => `<tr><td><button class="secondary" data-open-run="${run.id}">${htmlEscape(run.event.name)}</button></td><td>${htmlEscape(run.state)}</td><td>${htmlEscape(run.created_at)}</td></tr>`).join("")}</tbody></table><div class="actions"><button class="secondary" data-previous ${offset ? "" : "disabled"}>Previous</button><button class="secondary" data-next ${offset + data.items.length < data.total ? "" : "disabled"}>Next</button></div>`);
  root.querySelector("[data-close]").textContent = "Close";
  root.querySelector("[data-save]").textContent = "Refresh";
  root.querySelector("[data-save]").onclick = () => { root._close(); openEventHistory(id, offset, goalId); };
  root.querySelectorAll("[data-open-run]").forEach(button => button.onclick = () => { root._close(); openEventHistory(button.dataset.openRun); });
  root.querySelector("[data-previous]")?.addEventListener("click", () => { root._close(); openEventHistory(null, Math.max(0, offset - 30), goalId); });
  root.querySelector("[data-next]")?.addEventListener("click", () => { root._close(); openEventHistory(null, offset + 30, goalId); });
  if (id && ["pending", "running"].includes(data.state)) { const button = root.querySelector("[data-delete]"); button.hidden = false; button.textContent = "Cancel Event"; button.onclick = async () => { if (!isNodeContextGenerationCurrent(generation)) return; try { await api("POST", `/api/event-invocations/${id}/cancel`, {}); root._close(); await openEventHistory(id); } catch (e) { root.querySelector("[data-automation-error]").textContent = e.message; } }; }
}

document.getElementById("nav-context-menu")?.addEventListener("toggle", event => { if (event.target.open) refreshCustomEvents(); });
window.addEventListener("load", () => refreshCustomEvents());
