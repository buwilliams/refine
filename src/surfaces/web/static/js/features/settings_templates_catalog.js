// Resource usage follows saved references and launch-supplied context.
const templateCompositionSlots = {
  "supervised-skill": {attached_skills: ["context-skill"], continuation: ["workflow-continuation"], observational: ["workflow-observational"]},
  "manual-skill": {},
  "fleet-manage": {message: ["fleet-distribute"]},
  "terminal-session": {
    instructions: ["planning-agent", "agent", "goal-agent", "chat-standalone"],
    workflow_context: ["terminal-profiles-toolbar-agent-workflow", "terminal-profiles-general-agent-workflow"],
    active_refine: ["terminal-profiles-active-refine"], goal_attachment: ["terminal-profiles-attached-goal"],
    feature_attachment: ["terminal-profiles-attached-feature"], profile_context: ["terminal-profiles-plan", "terminal-profiles-goal-diagnostic"],
    supplemental_attachment: ["terminal-profiles-supplemental-context"],
  },
  "chat-session": {instructions: ["planning-agent", "goal-agent", "chat-feature", "chat-standalone"], context: ["chat-context-unavailable"]},
  "goal-agents-session": {goal_prompt: ["workflow", "supervised-skill"], completion_contract: ["goal-completion"]},
  "conflict-resolution": {context: ["sync-resolve-state-conflict"], ancestry_context: ["conflict-ancestry"], feedback_context: ["conflict-feedback"]},
};

function templateComposition(data) {
  const rows = new Map((data.items || []).map(row => [row.item.id, row]));
  const edges = [];
  rows.forEach((row, id) => {
    const names = new Set([...row.item.prompt.matchAll(/(?<!\\){{\s*([\w.-]+)\s*}}/g)].map(match => match[1]));
    const targets = new Map();
    names.forEach(name => {
      if (name.startsWith("templates.")) targets.set(name.slice(10), false);
      else (templateCompositionSlots[id]?.[name] || []).forEach(target => { if (!targets.has(target)) targets.set(target, true); });
    });
    targets.forEach((conditional, to) => { if (rows.has(to)) edges.push({from: id, to, conditional}); });
    if (names.has("skill")) {
      edges.push({from: id, to: "$skill", conditional: true});
    }
  });
  const skillPartials = new Set((data.skills || []).flatMap(skill => [...(skill.prompt || "").matchAll(/(?<!\\){{\s*templates\.([\w-]+)\s*}}/g)].map(match => match[1])));
  skillPartials.forEach(id => { if (rows.has(id)) edges.push({from: "$skill", to: id, conditional: true}); });
  return {rows, edges};
}

function renderTemplatesCatalog(data = {}) {
  const {edges, rows} = templateComposition(data);
  const resources = (data.workflowData?.skills.items || []).map(skill => {
    const assignments = (data.workflowData.events.items || []).filter(event => event.bindings?.some(binding => binding.skill_id === skill.id));
    return {id: skill.id, name: skill.name, kind: "Skill", attribute: "data-resource-skill", description: assignments.length ? assignments.map(event => workflowSourceLabel(event.source || "custom")).join(" · ") : "Not assigned. Add this Skill from a Goal step, system event, or custom action."};
  });
  rows.forEach(row => {
    const parents = [...new Set(edges.filter(edge => edge.to === row.item.id).flatMap(edge => edge.from === "$skill" ? (data.skills || []).filter(skill => [...(skill.prompt || "").matchAll(/(?<!\\){{\s*templates\.([\w-]+)\s*}}/g)].some(match => match[1] === row.item.id)).map(skill => `Skill: ${skill.name}`) : [rows.get(edge.from).name]))];
    resources.push({id: row.item.id, name: row.name, kind: row.usage?.kind === "partial" ? "Partial" : "Template", attribute: "data-template-id", description: row.usage?.description || "Agent prompt content.", included: parents.length ? `Included by ${parents.join(", ")}` : "", customized: row.customized});
  });
  resources.sort((a,b) => a.name.localeCompare(b.name));
  return `<section class="settings-section" data-testid="settings-templates">
    <p class="muted">Skills describe the work. Templates build agent prompts. Partials provide reusable guidance inside Skills and Templates. Edits apply everywhere a resource is used.</p>
    <div class="actions resource-filters"><label class="template-catalog-search">Find a resource<input type="search" data-template-catalog-search placeholder="Search names and where they are used…"></label><label>Type<select data-resource-type><option value="">All resources</option><option>Skill</option><option>Template</option><option>Partial</option></select></label>${data.workflowData ? '<button type="button" data-resource-new>Create Skill</button>' : ""}</div>
    <div class="template-catalog-table"><table class="table"><thead><tr><th>Name</th><th>Type</th><th>Where it is used</th></tr></thead><tbody>
    ${resources.map(row => `<tr data-template-catalog-row data-resource-kind="${row.kind}" data-template-search="${htmlEscape(`${row.name} ${row.description} ${row.included || ""}`.toLowerCase())}"><td><button type="button" class="template-list-name" ${row.attribute}="${htmlEscape(row.id)}">${htmlEscape(row.name)}</button>${row.customized ? '<span class="muted small">Customized</span>' : ""}</td><td>${row.kind}</td><td>${htmlEscape(row.description)}${row.included ? `<p class="muted small">${htmlEscape(row.included)}</p>` : ""}</td></tr>`).join("")}
    </tbody></table></div><p class="muted" data-template-catalog-empty hidden>No matching resources.</p><div class="actions resource-pagination"><span class="muted small" data-resource-range></span><span class="spacer"></span><button type="button" class="secondary" data-resource-previous>Previous</button><button type="button" class="secondary" data-resource-next>Next</button></div></section>`;
}
function bindTemplatesCatalog(data) {
  const section = document.querySelector('[data-testid="settings-templates"]');
  if (!section) return;
  section.querySelectorAll("[data-template-id]").forEach(button => { button.onclick = () => openTemplateEditor(button.dataset.templateId); });
  section.querySelectorAll("[data-resource-skill]").forEach(button => { button.onclick = () => openSkillEditor(data.workflowData.skills.items.find(skill => skill.id === button.dataset.resourceSkill)); });
  section.querySelector("[data-resource-new]")?.addEventListener("click", () => openSkillEditor());
  let page = 0;
  const filter = () => {
    const query = section.querySelector("[data-template-catalog-search]").value.trim().toLowerCase();
    const kind = section.querySelector("[data-resource-type]").value;
    const rows = [...section.querySelectorAll("[data-template-catalog-row]")];
    const matching = rows.filter(row => row.dataset.templateSearch.includes(query) && (!kind || row.dataset.resourceKind === kind));
    const size = 12;
    page = Math.min(page, Math.max(0, Math.ceil(matching.length / size) - 1));
    rows.forEach(row => { row.hidden = true; });
    matching.slice(page * size, (page + 1) * size).forEach(row => { row.hidden = false; });
    section.querySelector("[data-template-catalog-empty]").hidden = matching.length > 0;
    section.querySelector("[data-resource-range]").textContent = matching.length ? `${page * size + 1}–${Math.min((page + 1) * size, matching.length)} of ${matching.length} resources` : "";
    section.querySelector("[data-resource-previous]").disabled = page === 0;
    section.querySelector("[data-resource-next]").disabled = (page + 1) * size >= matching.length;
  };
  section.querySelector("[data-template-catalog-search]").oninput = () => { page = 0; filter(); };
  section.querySelector("[data-resource-type]").onchange = () => { page = 0; filter(); };
  section.querySelector("[data-resource-previous]").onclick = () => { page--; filter(); };
  section.querySelector("[data-resource-next]").onclick = () => { page++; filter(); };
  filter();
}
