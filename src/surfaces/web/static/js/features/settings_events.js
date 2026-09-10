// Events and Skills share one revision-fenced configuration capability.
let automationEditor = null;
let customEventsGeneration = 0;

async function loadAutomationSettings(tab) {
  const definition = await api("GET", tab === "skills" ? "/api/skills" : "/api/event-definitions");
  return { ...definition, tab };
}

function automationScopeLabel(scope) {
  return scope?.node_id ? `Node: ${scope.node_id}` : "Project";
}

function renderAutomationSettings(tab, data = {}) {
  const items = data.items || [];
  const isSkill = tab === "skills";
  return `<section class="settings-section" data-testid="settings-${tab}">
    <div class="actions"><h3>${isSkill ? "Skills" : "Events"}</h3><span class="spacer"></span>
      <label>Scope <select data-automation-scope><option value="all">Project and nodes</option><option value="project">Project</option><option value="node">This node</option></select></label>
      <button data-automation-new>New ${isSkill ? "Skill" : "Event"}</button>
      ${isSkill ? "" : '<button class="secondary" data-event-history>Execution history</button>'}</div>
    <p class="muted">${isSkill ? "Reusable instructions and additional context for agents. Refine supplies the completion contract for the selected role." : "Choose when Skills run, their order, and whether their results block workflow progression. Node bindings can explicitly override project bindings."}</p>
    <table class="table" data-testid="automation-table"><thead><tr><th>Name</th><th>${isSkill ? "Role" : "Trigger"}</th><th>Scope</th><th>Status</th><th>Actions</th></tr></thead><tbody>
    ${items.map(item => `<tr data-automation-row data-scope="${htmlEscape(item.scope?.node_id || "project")}">
      <td><button class="secondary" data-automation-edit="${htmlEscape(item.id)}">${htmlEscape(item.name)}</button></td>
      <td>${htmlEscape(isSkill ? item.role : item.source || "Manual")}</td><td>${htmlEscape(automationScopeLabel(item.scope))}</td>
      <td>${item.enabled ? "Enabled" : "Disabled"}</td><td>${!isSkill && item.kind === "custom" ? `<button class="secondary" data-event-trigger="${htmlEscape(item.id)}" ${item.enabled ? "" : "disabled"}>Run</button>` : ""}</td></tr>`).join("")}
    </tbody></table>${items.length ? "" : '<p class="muted">No definitions yet.</p>'}</section>`;
}

function bindAutomationSettings(tab, data) {
  const root = document.querySelector(`[data-testid="settings-${tab}"]`);
  if (!root) return;
  root.querySelector("[data-automation-new]").onclick = () => openAutomationEditor(tab, null, data.revision);
  root.querySelectorAll("[data-automation-edit]").forEach(button => button.onclick = () => openAutomationEditor(tab, data.items.find(item => item.id === button.dataset.automationEdit), data.revision));
  root.querySelectorAll("[data-event-trigger]").forEach(button => button.onclick = () => triggerCustomEvent(button.dataset.eventTrigger));
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
  root.innerHTML = `<div class="modal" role="dialog" aria-modal="true" aria-labelledby="automation-dialog-title" style="max-width:900px" data-testid="automation-modal">
    <div class="modal-title" id="automation-dialog-title">${htmlEscape(title)}</div>
    <div class="modal-body" style="max-height:72vh;overflow:auto">${content}<p data-automation-error role="alert" class="muted"></p></div>
    <div class="modal-actions"><button class="danger" data-delete hidden>Delete</button><span class="spacer"></span><button class="secondary" data-close>Cancel</button><button data-save>Save</button></div></div>`;
  const close = () => { root.remove(); document.removeEventListener("keydown", onKey, true); if (automationEditor === root) automationEditor = null; priorFocus?.focus?.(); };
  function onKey(event) {
    if (!root.contains(event.target)) return;
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); close(); }
    if (event.key === "Tab") {
      const controls = [...root.querySelectorAll("button:not([disabled]):not([hidden]), input:not([disabled]), textarea, select, [tabindex='0']")].filter(e => e.getClientRects().length);
      const first = controls[0], last = controls.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  }
  document.body.appendChild(root); document.addEventListener("keydown", onKey, true);
  root.querySelector("[data-close]").onclick = close;
  root.querySelector("input, select, textarea, [data-close]")?.focus();
  root._close = close;
  return root;
}

function scopeOptions(value = "") {
  const nodes = state.project?.nodes || [];
  const node = nodeContextActiveNodeId();
  const ids = [...new Set([...nodes.map(n => n.id), node, value].filter(Boolean))];
  return `<option value="" ${!value ? "selected" : ""}>Project</option>${ids.map(id => `<option value="${htmlEscape(id)}" ${id === value ? "selected" : ""}>Node: ${htmlEscape(nodes.find(n => n.id === id)?.display_name || id)}</option>`).join("")}`;
}

function parameterRows(parameters = []) {
  return parameters.map(p => `<tr data-parameter><td><input aria-label="Parameter name" data-name value="${htmlEscape(p.name || "")}"></td>
    <td><select aria-label="Parameter type" data-kind>${["text", "number", "boolean", "choice"].map(kind => `<option ${kind === (p.kind || "text") ? "selected" : ""}>${kind}</option>`).join("")}</select></td>
    <td><input aria-label="Required" type="checkbox" data-required ${p.required ? "checked" : ""}></td>
    <td><input aria-label="Default value" data-default value="${htmlEscape(p.default == null ? "" : String(p.default))}"></td>
    <td><input aria-label="Choices separated by commas" data-choices value="${htmlEscape((p.choices || []).join(", "))}"><input aria-label="Parameter description" data-description placeholder="Description" value="${htmlEscape(p.description || "")}"></td>
    <td><button type="button" class="secondary" data-remove-parameter>Remove</button></td></tr>`).join("");
}

function readParameters(root) {
  return [...root.querySelectorAll("[data-parameter]")].map(row => {
    const kind = row.querySelector("[data-kind]").value, raw = row.querySelector("[data-default]").value;
    let value = raw === "" ? null : raw;
    if (raw !== "" && kind === "number") { value = Number(raw); if (!Number.isFinite(value)) throw new Error("A number parameter requires a numeric default."); }
    if (raw !== "" && kind === "boolean") { if (!["true", "false"].includes(raw)) throw new Error("A boolean default must be true or false."); value = raw === "true"; }
    return { name: row.querySelector("[data-name]").value.trim(), kind, required: row.querySelector("[data-required]").checked, default: value, choices: row.querySelector("[data-choices]").value.split(",").map(v => v.trim()).filter(Boolean), description: row.querySelector("[data-description]").value };
  });
}

async function openAutomationEditor(tab, original, revision) {
  if (automationEditor) return;
  const generation = captureNodeContextGeneration();
  const isSkill = tab === "skills";
  const [catalog, skillList] = await Promise.all([api("GET", "/api/event-definitions/catalog"), isSkill ? Promise.resolve({items: []}) : api("GET", "/api/skills")]);
  if (!isNodeContextGenerationCurrent(generation)) return;
  const item = original ? JSON.parse(JSON.stringify(original)) : { id: `${isSkill ? "skill" : "event"}-${crypto.randomUUID()}`, name: "", prompt: "", role: "task", kind: "custom", source: null, enabled: true, scope: {node_id: null}, parameters: [], bindings: [], on_success: null };
  const root = automationModal(`${original ? "Edit" : "New"} ${isSkill ? "Skill" : "Event"}`, `<form data-automation-form>
    <div class="form-row"><label>Name<input required data-name value="${htmlEscape(item.name)}"></label></div>
    <div class="form-row"><label>Scope<select data-scope>${scopeOptions(item.scope?.node_id)}</select></label><label><input type="checkbox" data-enabled ${item.enabled ? "checked" : ""}> Enabled</label></div>
    ${isSkill ? `<div class="form-row"><label>Result role<select data-role>${catalog.roles.map(role => `<option ${role === item.role ? "selected" : ""}>${role}</option>`).join("")}</select></label></div><div class="form-row"><label>Instructions<textarea rows="12" required data-prompt>${htmlEscape(item.prompt)}</textarea></label></div>` : `<div class="form-row"><label>Trigger<select data-source ${original ? "disabled" : ""}><option value="">Manual custom Event</option>${catalog.sources.map(source => `<option value="${source}" ${source === item.source ? "selected" : ""}>${source.split(".").join(" · ")}</option>`).join("")}</select></label></div>`}
    <h3>Parameters</h3><table class="table"><thead><tr><th>Name</th><th>Type</th><th>Required</th><th>Default</th><th>Choices / description</th><th></th></tr></thead><tbody data-parameters>${parameterRows(item.parameters)}</tbody></table><button type="button" class="secondary" data-add-parameter>Add parameter</button>
    ${isSkill ? "" : `<h3>Skills</h3><div data-bindings></div><button type="button" class="secondary" data-add-binding>Add Skill</button><div class="form-row"><label>After all blocking Skills succeed<select data-success><option value="">No additional action</option>${["start", "accept", "retry", "reopen"].map(a => `<option value="${a}" ${a === item.on_success ? "selected" : ""}>${a}</option>`).join("")}</select></label></div>`}
    </form>`);
  automationEditor = root;
  root.dataset.nodeContextDirty = "false";
  root.addEventListener("input", () => { root.dataset.nodeContextDirty = "true"; });
  root.addEventListener("change", () => { root.dataset.nodeContextDirty = "true"; });
  const error = e => { root.querySelector("[data-automation-error]").textContent = e.message || String(e); };
  root.querySelector("[data-add-parameter]").onclick = () => { root.dataset.nodeContextDirty = "true"; root.querySelector("[data-parameters]").insertAdjacentHTML("beforeend", parameterRows([{}])); };
  root.addEventListener("click", e => { if (e.target.closest("[data-remove-parameter]")) e.target.closest("[data-parameter]").remove(); });
  function readBindings() {
    return [...root.querySelectorAll("[data-binding]")].map((row, order) => ({ id: row.dataset.binding, skill_id: row.querySelector("[data-skill]").value, enabled: row.querySelector("[data-binding-enabled]").checked, mode: row.querySelector("[data-mode]").value, order, scope: {node_id: row.querySelector("[data-binding-scope]").value || null}, overrides: row.querySelector("[data-override]").value || null, inputs: Object.fromEntries([...row.querySelectorAll("[data-input-name]")].filter(input => input.value).map(input => [input.dataset.inputName, input.value])) }));
  }
  function drawBindings(bindings) {
    root.querySelector("[data-bindings]").innerHTML = bindings.map(b => {
      const skill = skillList.items.find(s => s.id === b.skill_id);
      return `<fieldset data-binding="${htmlEscape(b.id)}" style="margin:12px 0"><legend>${htmlEscape(skill?.name || "Skill binding")}</legend><div class="form-row">
        <label>Skill<select data-skill>${skillList.items.map(s => `<option value="${htmlEscape(s.id)}" ${s.id === b.skill_id ? "selected" : ""}>${htmlEscape(s.name)} (${s.role})</option>`).join("")}</select></label>
        <label>Execution<select data-mode>${[["blocking","Wait for result"],["background","Background"],["context","Attach context"]].map(([v,label]) => `<option value="${v}" ${b.mode === v ? "selected" : ""}>${label}</option>`).join("")}</select></label>
        <label>Scope<select data-binding-scope>${scopeOptions(b.scope?.node_id)}</select></label><label><input type="checkbox" data-binding-enabled ${b.enabled ? "checked" : ""}> Enabled</label>
        <label>Override project binding<select data-override><option value="">Additional binding</option>${bindings.filter(other => other.id !== b.id && !other.scope?.node_id).map(other => `<option value="${htmlEscape(other.id)}" ${b.overrides === other.id ? "selected" : ""}>${htmlEscape(skillList.items.find(s => s.id === other.skill_id)?.name || other.id)}</option>`).join("")}</select></label></div>
        ${(skill?.parameters || []).map(p => `<label>${htmlEscape(p.name)} context source<input data-input-name="${htmlEscape(p.name)}" value="${htmlEscape(b.inputs?.[p.name] || "")}" placeholder="goal.id, system.node_id, or event.${htmlEscape(p.name)}" list="event-context-sources"></label>`).join("")}
        <div class="actions"><button type="button" class="secondary" data-binding-up>Move up</button><button type="button" class="secondary" data-remove-binding>Remove binding</button></div></fieldset>`;
    }).join("") + '<datalist id="event-context-sources"><option value="goal.id"><option value="goal.name"><option value="system.node_id"><option value="system.project_root"><option value="system.candidate_commit"></datalist>';
    root.querySelectorAll("[data-skill]").forEach(select => select.onchange = () => drawBindings(readBindings()));
    root.querySelectorAll("[data-remove-binding]").forEach(button => button.onclick = () => { button.closest("[data-binding]").remove(); });
    root.querySelectorAll("[data-binding-up]").forEach(button => button.onclick = () => { const row = button.closest("[data-binding]"); if (row.previousElementSibling) row.parentNode.insertBefore(row, row.previousElementSibling); });
  }
  if (!isSkill) {
    drawBindings(item.bindings);
    root.querySelector("[data-add-binding]").onclick = () => { if (!skillList.items.length) { error(new Error("Create a Skill before adding a binding.")); return; } drawBindings([...readBindings(), {id: `binding-${crypto.randomUUID()}`, skill_id: skillList.items[0].id, mode: "blocking", enabled: true, scope: {node_id: null}, inputs: {}}]); };
  }
  async function save(remove = false) {
    if (!isNodeContextGenerationCurrent(generation)) { error(new Error("The selected project or node changed. Close this editor and reopen it.")); return; }
    const button = root.querySelector("[data-save]"); button.disabled = true;
    try {
      const path = `/api/${isSkill ? "skills" : "event-definitions"}/${encodeURIComponent(item.id)}`;
      if (remove) await api("DELETE", path, {revision});
      else {
        if (!root.querySelector("form").reportValidity()) return;
        const edited = {id: item.id, name: root.querySelector("form > .form-row [data-name]").value.trim(), enabled: root.querySelector("[data-enabled]").checked, scope: {node_id: root.querySelector("[data-scope]").value || null}, parameters: readParameters(root)};
        if (isSkill) Object.assign(edited, {prompt: root.querySelector("[data-prompt]").value, role: root.querySelector("[data-role]").value, provenance: item.provenance || null});
        else { const source = root.querySelector("[data-source]").value || null; Object.assign(edited, {kind: source ? "system" : "custom", source, bindings: readBindings(), on_success: root.querySelector("[data-success]").value || null}); }
        await api("PUT", path, {revision, item: edited});
      }
      root._close(); await refreshSettings({force: true}); await refreshCustomEvents();
    } catch (e) {
      error(e);
      if (e.status === 409) { await refreshSettings({force: true}); root.querySelector("[data-automation-error]").textContent = "Configuration changed. Your draft is retained here; close and reopen the refreshed definition before saving."; }
    } finally { button.disabled = false; }
  }
  root.querySelector("[data-save]").onclick = () => save();
  root.querySelector("form").onsubmit = e => { e.preventDefault(); save(); };
  if (original) { root.querySelector("[data-delete]").hidden = false; root.querySelector("[data-delete]").onclick = () => save(true); }
}

async function refreshCustomEvents() {
  const generation = ++customEventsGeneration;
  const nodeGeneration = captureNodeContextGeneration();
  const root = document.getElementById("nav-custom-events");
  if (!root) return;
  try {
    const data = await api("GET", `/api/event-definitions?node_id=${encodeURIComponent(nodeContextActiveNodeId())}`, undefined, {recordError: false});
    if (generation !== customEventsGeneration || !isNodeContextGenerationCurrent(nodeGeneration)) return;
    const events = data.items.filter(e => e.kind === "custom" && e.enabled);
    for (const key of commandRegistry.keys()) if (key.startsWith("event.custom.")) commandRegistry.delete(key);
    renderInto(root, `<div class="nav-menu-section-label">Events</div>${events.map(e => `<button class="nav-menu-item" type="button" data-custom-event="${htmlEscape(e.id)}">${htmlEscape(e.name)}</button>`).join("") || '<span class="muted small">No custom Events</span>'}`);
    root.querySelectorAll("[data-custom-event]").forEach(button => button.onclick = () => { root.closest("details")?.removeAttribute("open"); triggerCustomEvent(button.dataset.customEvent); });
    for (const event of events) registerCommand({id: `event.custom.${event.id}`, title: event.name, group: "Events", run: () => triggerCustomEvent(event.id)});
  } catch (_) {
    if (generation === customEventsGeneration) { renderInto(root, ""); for (const key of commandRegistry.keys()) if (key.startsWith("event.custom.")) commandRegistry.delete(key); }
  }
}

async function triggerCustomEvent(id, selectedGoalId = null) {
  const generation = captureNodeContextGeneration();
  const goalId = selectedGoalId || state.currentGoal || null;
  try {
    const [definition, schema] = await Promise.all([api("GET", `/api/event-definitions/${encodeURIComponent(id)}`), api("GET", `/api/event-definitions/${encodeURIComponent(id)}/inputs${goalId ? `?goal_id=${encodeURIComponent(goalId)}` : ""}`)]);
    if (!isNodeContextGenerationCurrent(generation)) return;
    if (definition.item.on_success && !goalId) {
      const goalModal = automationModal(`Run ${definition.item.name}`, '<form><label>Goal ID<input data-goal-id required placeholder="Select the Goal this Event will act on"></label></form>');
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
    const root = automationModal(`Run ${definition.item.name}`, `<form data-event-inputs>${schema.parameters.map((p, index) => `<div class="form-row"><label>${htmlEscape(p.name)}${p.required ? " *" : ""}
      ${p.kind === "choice" || p.kind === "boolean" ? `<select data-parameter-index="${index}" ${p.required ? "required" : ""}><option value="">Choose…</option>${(p.kind === "boolean" ? ["true","false"] : p.choices).map(v => `<option value="${htmlEscape(v)}" ${String(p.default) === v ? "selected" : ""}>${htmlEscape(v)}</option>`).join("")}</select>` : `<input data-parameter-index="${index}" type="${p.kind === "number" ? "number" : "text"}" ${p.kind === "number" ? 'step="any"' : ""} ${p.required ? "required" : ""} value="${htmlEscape(p.default == null ? "" : String(p.default))}">`}</label><p class="muted small">${htmlEscape(p.description || "")}</p></div>`).join("")}</form>`);
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
