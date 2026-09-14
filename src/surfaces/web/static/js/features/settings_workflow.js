// Workflow is a view over the existing revision-fenced Skills, events, and Templates.
let workflowSettingsView = "goals";
let workflowSettingsStep = "plan";
let workflowSettingsHook = "enter";
let workflowSettingsSystemSource = "";
let workflowSettingsRoute = "";
let workflowSettingsData = null;
const workflowHookLabels = {enter: "On entry", success: "On success", error: "On error", exit: "On exit"};
const workflowViewLabels = {goals: "Goal steps", system: "System events", custom: "Custom actions", resources: "Shared resources"};

async function loadWorkflowSettings(detached = false) {
  if (detached) return {detached: true, skills: {items: []}, events: {items: []}, catalog: {sources: []}, templates: await api("GET", "/api/templates")};
  for (let attempt = 0; attempt < 2; attempt++) {
    const [skills, events, catalog, templates] = await Promise.all([
      api("GET", "/api/skills"), api("GET", "/api/event-definitions"),
      api("GET", "/api/event-definitions/catalog"), api("GET", "/api/templates"),
    ]);
    if (skills.revision == null || events.revision == null || skills.revision === events.revision) return {skills, events, catalog, templates: {...templates, skills: skills.items}};
  }
  throw new Error("Workflow configuration changed while loading. Refresh to see the latest assignments.");
}

function workflowSources(data) {
  return [...new Set([...(data.catalog.sources || []), ...(data.events.items || []).map(event => event.source).filter(Boolean), ...(data.skills.items || []).map(skill => skill.trigger_source).filter(Boolean)])];
}
function workflowStepNames(data) {
  return [...new Set(workflowSources(data).filter(source => source.startsWith("workflow.")).map(source => source.split(".")[1]))];
}
function workflowSourceLabel(source) {
  if (source === "custom") return "Manual and custom actions";
  if (source === "node.startup.ready") return "Node ready after startup";
  if (source?.startsWith("workflow.")) return skillTriggerLabel(source);
  return (source || "").split(/[._-]/).map(word => word[0]?.toUpperCase() + word.slice(1)).join(" ");
}
function workflowAssignments(data, source) {
  const skills = new Map(data.skills.items.map(skill => [skill.id, skill]));
  const assignments = [];
  for (const event of data.events.items || []) {
    if ((event.source || "custom") !== source) continue;
    for (const binding of event.bindings || []) {
      const skill = skills.get(binding.skill_id);
      if (skill) assignments.push({skill, binding, event});
    }
  }
  // Older servers may list Skills without event projections. Keep their editor reachable.
  for (const skill of skills.values()) {
    if (skill.trigger_source === source && !assignments.some(row => row.skill.id === skill.id)) assignments.push({skill, binding: {order: 0, mode: "blocking"}, event: {enabled: true}});
  }
  return assignments.sort((a, b) => (a.binding.order || 0) - (b.binding.order || 0) || a.skill.id.localeCompare(b.skill.id));
}
function workflowCurrentSource(data) {
  if (workflowSettingsView === "custom") return "custom";
  if (workflowSettingsView === "system") {
    const sources = workflowSources(data).filter(source => source !== "custom" && !source.startsWith("workflow."));
    if (!sources.includes(workflowSettingsSystemSource)) workflowSettingsSystemSource = sources[0] || "";
    return workflowSettingsSystemSource;
  }
  const steps = workflowStepNames(data);
  if (!steps.includes(workflowSettingsStep)) workflowSettingsStep = steps[0] || "plan";
  const hooks = workflowSources(data).filter(source => source.startsWith(`workflow.${workflowSettingsStep}.`)).map(source => source.split(".")[2]);
  if (!hooks.includes(workflowSettingsHook)) workflowSettingsHook = hooks[0] || "enter";
  return `workflow.${workflowSettingsStep}.${workflowSettingsHook}`;
}
function renderWorkflowAssignments(data, source) {
  const assignments = workflowAssignments(data, source);
  return `<section class="workflow-trigger-details" aria-label="${htmlEscape(workflowSourceLabel(source))}">
    <div class="actions"><h4>${source.startsWith("workflow.") ? "Assigned Skills" : htmlEscape(workflowSourceLabel(source))}</h4><span class="spacer"></span><button type="button" class="secondary" data-workflow-existing="${htmlEscape(source)}">Add existing Skill</button><button type="button" data-workflow-add="${htmlEscape(source)}">Create Skill</button></div>
    <p class="muted">${source === "custom" ? "Skills launched manually or by a custom event. Manual terminals and automatic runs have separate prompt templates." : "Choose a Skill to edit its instructions and settings. Skills run in the order shown; context-only Skills add instructions to the other agents."}</p>
    ${assignments.length ? `<div class="workflow-assignment-list">${assignments.map(({skill, binding, event}, index) => {
      const disabled = !skill.enabled || event.enabled === false || binding.enabled === false;
      const scope = skill.scope?.node_id || binding.scope?.node_id || event.scope?.node_id;
      return `<article class="workflow-assignment workflow-skill-card" data-workflow-assignment="${htmlEscape(skill.id)}">
        <strong>${htmlEscape(skill.name)}</strong>
          <span class="muted small">${source === "custom" ? "Custom action" : `Order ${binding.order || 0}`} · ${htmlEscape({blocking:"Required",background:"Background",context:"Context only"}[binding.mode] || binding.mode || "Required")} · ${scope ? `Node: ${htmlEscape(scope)}` : "Project"}${disabled ? " · Disabled" : ""}</span>
          ${source === "custom" && event.name && event.id !== "custom" ? `<span class="muted small">${htmlEscape(event.name)}</span>` : ""}
        <div class="workflow-assignment-actions"><button type="button" class="secondary" data-workflow-skill="${htmlEscape(skill.id)}" aria-label="Edit ${htmlEscape(skill.name)} Skill">Edit Skill</button><button type="button" class="secondary" data-workflow-preview="${index}">${binding.mode === "context" ? "Preview context" : "Preview prompt"}</button><button type="button" class="secondary" data-workflow-assignment-edit="${index}">Assignment settings</button></div>
      </article>`;
    }).join("")}</div>` : '<p class="workflow-empty muted">No Skills assigned. Add a Skill to run work at this trigger.</p>'}
  </section>`;
}
function renderWorkflowSettings(data) {
  workflowSettingsData = data;
  const route = location.hash;
  if (workflowSettingsRoute !== route) {
    workflowSettingsRoute = route;
    const legacy = parseHash().tab;
    workflowSettingsView = ["skills", "templates"].includes(legacy) ? "resources" : "goals";
  }
  return `<div id="workflow-settings-surface">${workflowSettingsContent(data)}</div>`;
}
function workflowSettingsContent(data) {
  let content = "";
  if (data.detached) {
    content = `<p class="muted">Attach a project to configure steps, event triggers, and Skills. Built-in Templates remain available to explore.</p>${renderTemplatesSettings(data.templates)}`;
  } else if (workflowSettingsView === "resources") {
    content = renderTemplatesSettings({...data.templates, workflowData: data});
  } else {
    const source = workflowCurrentSource(data);
    if (workflowSettingsView === "goals") {
      const steps = workflowStepNames(data);
      content += `<div class="workflow-step-picker" role="group" aria-label="Goal workflow steps">${steps.map(step => {
        const count = workflowSources(data).filter(source => source.startsWith(`workflow.${step}.`)).reduce((sum, source) => sum + workflowAssignments(data, source).length, 0);
        return `<button type="button" class="secondary" data-workflow-step="${htmlEscape(step)}" aria-pressed="${step === workflowSettingsStep}"><strong>${htmlEscape(step[0].toUpperCase() + step.slice(1))}</strong><span class="muted small">${count} ${count === 1 ? "Skill" : "Skills"}</span></button>`;
      }).join("")}</div><div class="flat-tabs workflow-hook-tabs" role="tablist" aria-label="${htmlEscape(workflowSettingsStep)} triggers">${Object.entries(workflowHookLabels).filter(([key]) => workflowSources(data).includes(`workflow.${workflowSettingsStep}.${key}`)).map(([key,label]) => `<button type="button" role="tab" id="workflow-hook-${key}" aria-controls="workflow-hook-panel" data-workflow-hook="${key}" aria-selected="${key === workflowSettingsHook}" tabindex="${key === workflowSettingsHook ? 0 : -1}">${label} (${workflowAssignments(data, `workflow.${workflowSettingsStep}.${key}`).length})</button>`).join("")}</div>`;
    } else if (workflowSettingsView === "system") {
      const sources = workflowSources(data).filter(source => source !== "custom" && !source.startsWith("workflow."));
      content += `<div class="workflow-step-picker workflow-system-picker" role="group" aria-label="System events">${sources.map(item => `<button type="button" class="secondary" data-workflow-system="${htmlEscape(item)}" aria-pressed="${item === source}"><strong>${htmlEscape(workflowSourceLabel(item))}</strong><span class="muted small">${workflowAssignments(data, item).length} Skills</span></button>`).join("")}</div>`;
    }
    content += source ? (workflowSettingsView === "goals" ? `<div id="workflow-hook-panel" role="tabpanel" aria-labelledby="workflow-hook-${workflowSettingsHook}">${renderWorkflowAssignments(data, source)}</div>` : renderWorkflowAssignments(data, source)) : '<p class="muted">No system event triggers are available.</p>';
  }
  return `<section class="settings-section" data-testid="settings-workflow">
    <div class="actions"><h3>Workflow</h3><span class="spacer"></span>${data.detached ? "" : '<button type="button" class="secondary" data-workflow-history>Run history</button>'}</div>
    <p class="muted">Choose when work happens, then manage its Skills and the context sent to agents.</p>
    ${data.detached ? "" : `<div class="flat-tabs workflow-view-tabs" role="tablist" aria-label="Workflow configuration">${Object.entries(workflowViewLabels).map(([key,label]) => `<button type="button" role="tab" id="workflow-view-${key}" aria-controls="workflow-view-panel" data-workflow-view="${key}" aria-selected="${key === workflowSettingsView}" tabindex="${key === workflowSettingsView ? 0 : -1}">${label}</button>`).join("")}</div>`}
    <div class="workflow-view-content" id="workflow-view-panel"${data.detached ? "" : ` role="tabpanel" aria-labelledby="workflow-view-${workflowSettingsView}"`}>${content}</div></section>`;
}
function redrawWorkflowSettings() {
  renderInto(document.getElementById("workflow-settings-surface"), workflowSettingsContent(workflowSettingsData), () => bindWorkflowSettings(workflowSettingsData));
}
function bindWorkflowTabGroup(root, selector, keyName, select) {
  root.querySelectorAll(selector).forEach(button => {
    button.onclick = () => { select(button.dataset[keyName]); redrawWorkflowSettings(); document.querySelector(`${selector}[data-${keyName.replace(/[A-Z]/g, c => "-" + c.toLowerCase())}="${button.dataset[keyName]}"]`)?.focus({preventScroll:true}); };
    button.onkeydown = event => {
      const buttons = [...root.querySelectorAll(selector)], index = buttons.indexOf(button);
      const next = event.key === "ArrowRight" ? (index + 1) % buttons.length : event.key === "ArrowLeft" ? (index + buttons.length - 1) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
      if (next === null) return;
      event.preventDefault(); buttons[next].click();
    };
  });
}
function bindWorkflowSettings(data) {
  const root = document.querySelector('[data-testid="settings-workflow"]');
  if (!root) return;
  if (data.detached) { bindTemplatesSettings(); return; }
  bindWorkflowTabGroup(root, "[data-workflow-view]", "workflowView", value => { workflowSettingsView = value; });
  bindWorkflowTabGroup(root, "[data-workflow-hook]", "workflowHook", value => { workflowSettingsHook = value; });
  root.querySelectorAll("[data-workflow-step]").forEach(button => { button.onclick = () => { workflowSettingsStep = button.dataset.workflowStep; redrawWorkflowSettings(); root.querySelector(`[data-workflow-step="${workflowSettingsStep}"]`)?.focus({preventScroll:true}); }; });
  root.querySelectorAll("[data-workflow-system]").forEach(button => { button.onclick = () => { workflowSettingsSystemSource = button.dataset.workflowSystem; redrawWorkflowSettings(); document.querySelector(`[data-workflow-system="${CSS.escape(workflowSettingsSystemSource)}"]`)?.focus({preventScroll:true}); }; });
  root.querySelector("[data-workflow-history]").onclick = () => openEventHistory();
  root.querySelectorAll("[data-workflow-add]").forEach(button => { button.onclick = () => openSkillEditor(null, false, {source: button.dataset.workflowAdd}); });
  root.querySelectorAll("[data-workflow-skill]").forEach(button => { button.onclick = () => openSkillEditor(data.skills.items.find(skill => skill.id === button.dataset.workflowSkill)); });
  root.querySelectorAll("[data-workflow-existing]").forEach(button => { button.onclick = () => openWorkflowAssignment(data, button.dataset.workflowExisting); });
  root.querySelectorAll("[data-workflow-assignment-edit]").forEach(button => { button.onclick = () => openWorkflowAssignment(data, workflowCurrentSource(data), workflowAssignments(data, workflowCurrentSource(data))[Number(button.dataset.workflowAssignmentEdit)]); });
  root.querySelectorAll("[data-workflow-preview]").forEach(button => { button.onclick = () => previewWorkflowAssignment(data, workflowCurrentSource(data), Number(button.dataset.workflowPreview), button); });
  if (workflowSettingsView === "resources") {
    bindTemplatesSettings();
  }
}

async function previewWorkflowAssignment(data, source, index, button) {
  const generation = captureNodeContextGeneration(), rows = workflowAssignments(data, source), row = rows[index];
  if (!row) return;
  button.disabled = true;
  try {
    // Read the same revision used to display the assignment before composing a sample.
    const current = await api("GET", `/api/skills/${encodeURIComponent(row.skill.id)}`);
    if (current.revision !== data.skills.revision) throw new Error("This Workflow changed. Refresh before previewing its assigned Skills.");
    const parameters = Object.fromEntries((current.item.parameters || []).filter(p => p.default != null).map(p => [p.name, p.default]));
    const role = /^workflow\.(plan|implement|quality|governance)\.enter$/.exec(source)?.[1] || "task";
    const values = {skill: {template: current.item.prompt}, skill_name: current.item.name, parameters: JSON.stringify(parameters), workflow_step: source.startsWith("workflow.") ? source.split(".")[1] : "", current_round_goal: "Sample Goal request", context: "{}", execution: JSON.stringify({binding_id: row.binding.id, role}), completion_contract: JSON.stringify(data.catalog.completion_contract || {}), continuation: "", observational: "", attached_skills: ""};
    const node = row.skill.scope?.node_id || nodeContextActiveNodeId();
    const contexts = rows.filter(item => item.binding.mode === "context" && item.skill.enabled && item.binding.enabled !== false && item.event.enabled !== false && [item.skill.scope, item.binding.scope, item.event.scope].every(scope => !scope?.node_id || scope.node_id === node));
    const rendered = await Promise.all(contexts.map(context => api("POST", "/api/templates/context-skill/preview", {values: {...values, skill: {template: context.skill.prompt}, skill_name: context.skill.name, parameters: JSON.stringify(Object.fromEntries((context.skill.parameters || []).filter(p => p.default != null).map(p => [p.name, p.default])))}})));
    values.attached_skills = rendered.map(result => result.prompt).join("\n\n");
    if (!isNodeContextGenerationCurrent(generation)) return;
    const templateId = row.binding.mode === "context" ? "context-skill" : source.startsWith("workflow.") ? "workflow" : "supervised-skill";
    let previewTemplate = templateId;
    if (row.binding.mode !== "context") {
      const prompt = await api("POST", `/api/templates/${templateId}/preview`, {values});
      values.goal_prompt = prompt.prompt;
      values.signal_path = "/runtime/sample-completion.json";
      values.completion_contract = {template: "{{templates.goal-completion}}"};
      previewTemplate = "goal-agents-session";
    }
    if (!isNodeContextGenerationCurrent(generation)) return;
    await openTemplateEditor(previewTemplate, {previewValues: values, initialTab: "preview", previewOnly: true, description: `${current.item.name} at ${workflowSourceLabel(source)}. Sample data only; this does not run the Skill.`});
  } catch (error) { showActionError(error); }
  finally { if (button.isConnected) button.disabled = false; }
}

async function openWorkflowAssignment(data, source, row = null) {
  if (automationEditor || automationEditorOpening) return;
  const generation = captureNodeContextGeneration();
  const assigned = new Set(workflowAssignments(data, source).map(row => row.skill.id));
  const choices = row ? [row.skill] : data.skills.items.filter(skill => !assigned.has(skill.id));
  const root = automationModal(row ? `${row.skill.name} — Assignment` : `Add existing Skill — ${workflowSourceLabel(source)}`, `
    <p class="muted">${row ? "These settings apply only to this assignment. Edit Skill changes its instructions everywhere it is used." : "Reuse a Skill here without changing its other assignments."}</p>
    <div class="form-row"><label for="assignment-skill">Skill</label><select id="assignment-skill" ${row ? "disabled" : ""}>${choices.map(skill => `<option value="${htmlEscape(skill.id)}">${htmlEscape(skill.name)}</option>`).join("")}</select></div>
    ${choices.length ? "" : '<p class="muted">No unassigned Skills are available for this trigger. Create a Skill instead.</p>'}
    <div class="form-row"><label for="assignment-mode">How it runs</label><select id="assignment-mode">${Object.entries({blocking:"Required",background:"Background",context:"Context only"}).map(([value,label]) => `<option value="${value}" ${value === (row?.binding.mode || "blocking") ? "selected" : ""}>${label}</option>`).join("")}</select></div>
    <div class="form-row"><label for="assignment-order">Order</label><input id="assignment-order" type="number" min="-2147483648" max="2147483647" step="1" required value="${row?.binding.order || 0}"></div>
    <div class="form-row"><label>Status</label>${automationChoices('id="assignment-enabled"', "Assignment status", [["true", "Enabled"], ["false", "Disabled"]], row?.binding.enabled !== false)}</div>
    <div data-assignment-inputs></div>`);
  automationEditor = root;
  const status = root.querySelector('#assignment-enabled');
  status.querySelectorAll('[data-choice]').forEach(button => { button.onclick = () => {
    status.dataset.value = button.dataset.choice;
    status.querySelectorAll('[data-choice]').forEach(choice => choice.setAttribute('aria-pressed', String(choice === button)));
  }; });
  const picker = root.querySelector('#assignment-skill');
  const drawInputs = () => {
    const skill = choices.find(skill => skill.id === picker.value);
    renderInto(root.querySelector('[data-assignment-inputs]'), (skill?.parameters || []).map(parameter => `<div class="form-row"><label>${htmlEscape(parameter.name)} context</label><input data-assignment-input="${htmlEscape(parameter.name)}" aria-label="${htmlEscape(parameter.name)} context" placeholder="Use default, or enter a context path" value="${htmlEscape(row?.binding.inputs?.[parameter.name] || '')}"></div>`).join(''));
  };
  picker.onchange = drawInputs; drawInputs();
  root.querySelector('[data-save]').disabled = !choices.length;
  const remove = root.querySelector('[data-delete]');
  remove.hidden = !row; remove.textContent = 'Remove assignment';
  let saving = false;
  const save = async deleting => {
    if (saving) return;
    saving = true;
    root.querySelectorAll('.modal-actions button').forEach(button => button.disabled = true);
    try {
      if (!isNodeContextGenerationCurrent(generation)) throw new Error('The selected node changed. Reopen this assignment.');
      const order = root.querySelector('#assignment-order');
      if (!deleting && !order.reportValidity()) return;
      const skill = choices.find(skill => skill.id === picker.value);
      const current = await api('GET', `/api/skills/${encodeURIComponent(skill.id)}`);
      if (current.revision !== data.skills.revision) throw new Error('Workflow configuration changed. Reopen this assignment before saving.');
      const assignments = data.events.items.flatMap(event => (event.bindings || []).filter(binding => binding.skill_id === skill.id && !(row && event.id === row.event.id && binding.id === row.binding.id)).map(binding => ({event_id:event.id, binding})));
      if (!deleting) {
        const event = row?.event || data.events.items.find(event => (event.source || 'custom') === source);
        if (!event) throw new Error('This event is no longer available. Refresh the Workflow.');
        const inputs = {...(row?.binding.inputs || {})};
        root.querySelectorAll('[data-assignment-input]').forEach(input => { if (input.value.trim()) inputs[input.dataset.assignmentInput] = input.value.trim(); else delete inputs[input.dataset.assignmentInput]; });
        assignments.push({event_id:event.id, binding:{...row?.binding, id:row?.binding.id || newSkillId(), skill_id:skill.id, enabled:status.dataset.value === 'true', mode:root.querySelector('#assignment-mode').value, order:Number(order.value), scope:row?.binding.scope || skill.scope || {node_id:null}, inputs}});
      }
      await api('PUT', `/api/skills/${encodeURIComponent(skill.id)}`, {revision:current.revision, item:current.item, event_bindings:assignments});
      root._close(); await refreshSettings({force:true}); await refreshManualSkills();
    } catch (error) { root.querySelector('[data-automation-error]').textContent = error.message; }
    finally { saving = false; root.querySelectorAll('.modal-actions button').forEach(button => button.disabled = false); }
  };
  root.querySelector('[data-save]').onclick = () => save(false);
  remove.onclick = () => save(true);
}
