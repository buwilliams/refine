// The selected launch narrows runtime slots; authored template references stay authoritative.
let resourceMapType = 'workflow';
let resourceMapVariant = '';
let resourceMapOpen = false;
const resourceMapTypes = {workflow:'Workflow agent', skill:'Custom Skill agent', planning:'Planning agent', general:'General agent', goal:'Goal agent', system:'System task agent'};
function resourceMapVariants(data) {
  if (resourceMapType === 'workflow') return [['workflow','Goal workflow']];
  if (resourceMapType === 'skill') return [['manual-skill','Manual run'],['supervised-skill','Automatic run']];
  if (resourceMapType === 'system') return (data.items || []).filter(row => row.usage?.group === 'tasks' && row.usage?.kind === 'template').map(row => [row.item.id,row.name]);
  return [['terminal-session','Terminal — toolbar / CLI'], ...(resourceMapType === 'general' ? [] : [['chat-session','Managed chat — API']])];
}
function resourceMapLaunchDescription(data) {
  if (resourceMapVariant === 'chat-session') return "Started through Refine’s chat API by an integration or API client. Current toolbar actions use Terminal, not Managed chat.";
  if (resourceMapVariant === 'terminal-session') {
    const action = {planning:'Planning Agent', general:'Agent', goal:'Goal Agent'}[resourceMapType];
    return `Toolbar → ${action} opens this terminal path. Agent terminals launched through the CLI also use Terminal Session. Refine supplies the initial prompt, then you interact with the agent’s CLI.`;
  }
  if (resourceMapType === 'workflow') return 'Used when Refine runs an assigned Skill at a Goal step or hook. These agents are started by the workflow.';
  if (resourceMapVariant === 'manual-skill') return 'Used when you choose Run Skill in Workflow → Custom actions or select a Skill from the New Goal menu.';
  if (resourceMapVariant === 'supervised-skill') return 'Used when a system event, such as Node startup, runs an assigned Skill automatically.';
  return (data.items || []).find(row => row.item.id === resourceMapVariant)?.usage?.description || 'Started by the corresponding Refine system operation.';
}
function renderResourceMap(data) {
  const variants = resourceMapVariants(data);
  if (!variants.some(([id]) => id === resourceMapVariant)) resourceMapVariant = variants[0]?.[0] || '';
  const supervised = resourceMapType === 'workflow' || (resourceMapType === 'skill' && resourceMapVariant === 'supervised-skill');
  const root = supervised ? 'goal-agents-session' : resourceMapVariant;
  const {rows} = templateComposition(data);
  const assignments = data.workflowData?.events.items || [];
  const skills = (data.skills || []).filter(skill => assignments.some(event => event.bindings?.some(binding => binding.skill_id === skill.id) && (resourceMapType === 'workflow' ? event.source?.startsWith('workflow.') : resourceMapVariant === 'manual-skill' ? !event.source : event.source && !event.source.startsWith('workflow.'))));
  skills.forEach(skill => rows.set(`skill:${skill.id}`, {name:skill.name,item:{id:`skill:${skill.id}`,prompt:skill.prompt},usage:{kind:'skill'}}));
  const instructions = {planning:'planning-agent',general:'agent',goal:'goal-agent'}[resourceMapType];
  const slots = structuredClone(templateCompositionSlots);
  slots['goal-agents-session'].goal_prompt = [resourceMapVariant];
  if (instructions) {
    slots['terminal-session'].instructions = [instructions];
    slots['terminal-session'].workflow_context = resourceMapType === 'planning' ? ['terminal-profiles-toolbar-agent-workflow'] : resourceMapType === 'goal' ? [] : slots['terminal-session'].workflow_context;
    slots['terminal-session'].active_refine = resourceMapType === 'general' ? ['terminal-profiles-active-refine'] : [];
    slots['chat-session'].instructions = [instructions];
    slots['terminal-session'].profile_context = resourceMapType === 'planning' ? ['terminal-profiles-plan'] : resourceMapType === 'goal' ? ['terminal-profiles-goal-diagnostic'] : [];
  }
  const children = id => {
    const prompt = rows.get(id)?.item.prompt || '';
    const targets = new Map();
    for (const match of prompt.matchAll(/(?<!\\){{\s*([\w.-]+)\s*}}/g)) {
      const name = match[1];
      if (name.startsWith('templates.')) targets.set(name.slice(10), false);
      else if (name === 'skill') targets.set('$skill', false);
      else for (const target of slots[id]?.[name] || []) {
        const conditional = !['instructions','goal_prompt','completion_contract'].includes(name);
        if (!targets.has(target)) targets.set(target, conditional);
      }
    }
    return [...targets].map(([id,conditional]) => ({id,conditional}));
  };
  let remaining = 150;
  const node = (id, conditional = false, path = []) => {
    if (--remaining < 0) return '';

    if (id === '$skill') return `<li><div class="resource-map-node"><button type="button" class="secondary" data-map-skills>Assigned Skill</button><span class="muted small">Selected for the trigger.</span></div>${skills.length ? `<details class="resource-map-includes"><summary>Show ${skills.length} assigned Skills and their includes</summary><ul>${skills.map(skill => node(`skill:${skill.id}`,true,[...path,id])).join('')}</ul></details>` : ''}</li>`;
    const row = rows.get(id);
    if (!row) return `<li><div class="resource-map-node muted">${htmlEscape(id)} · unavailable</div></li>`;
    const repeated = path.includes(id);
    const nested = repeated ? [] : children(id);
    return `<li><div class="resource-map-node"><button type="button" class="secondary" ${id.startsWith('skill:') ? 'data-map-skill' : 'data-map-template'}="${htmlEscape(id.replace(/^skill:/,''))}">${htmlEscape(row.name)}</button><span class="muted small">${row.usage?.kind === 'skill' ? 'Skill' : row.usage?.kind === 'partial' ? 'Partial' : 'Template'}${conditional ? ' · When applicable' : ''}${repeated ? ' · Circular reference' : ''}</span></div>${nested.length ? `<details class="resource-map-includes" ${path.length === 0 ? 'open' : ''}><summary>Includes ${nested.length} ${nested.length === 1 ? 'piece' : 'pieces'}</summary><ul>${nested.filter(child => !child.conditional).map(child => node(child.id,false,[...path,id])).join('')}${nested.some(child => child.conditional) ? `<li><details class="resource-map-includes"><summary>Context when applicable (${nested.filter(child => child.conditional).length})</summary><ul>${nested.filter(child => child.conditional).map(child => node(child.id,true,[...path,id])).join('')}</ul></details></li>` : ''}</ul></details>` : ''}</li>`;
  };
  return `<details class="resource-map-overview" ${resourceMapOpen ? 'open' : ''}><summary>How agent prompts are built</summary><div class="resource-map-body">
    <div class="actions resource-map-controls"><label>Agent type<select data-resource-map-type>${Object.entries(resourceMapTypes).map(([id,label]) => `<option value="${id}" ${id === resourceMapType ? 'selected' : ''}>${label}</option>`).join('')}</select></label>${variants.length > 1 ? `<label>${resourceMapType === 'system' ? 'Task' : 'Launch path'}<select data-resource-map-variant>${variants.map(([id,label]) => `<option value="${htmlEscape(id)}" ${id === resourceMapVariant ? 'selected' : ''}>${htmlEscape(label)}</option>`).join('')}</select></label>` : ''}</div>
    <p class="resource-map-launch" data-resource-map-launch><strong>Where this is used</strong><br>${htmlEscape(resourceMapLaunchDescription(data))}</p>
    <p class="muted small">Viewing this map does not start an agent or change its configuration. ${resourceMapType === 'workflow' ? 'Goal steps and hooks use Workflow inside the agent session.' : resourceMapType === 'skill' ? 'Manual runs open a Skill terminal; automatic runs use a supervised agent session.' : resourceMapType === 'system' ? 'Choose a task to see the prompt Refine uses for that operation.' : 'The session template combines the selected agent instructions with its context.'} Expand includes to follow saved references. Click a name to edit it. “When applicable” pieces depend on the launch.</p>
    ${root && rows.has(root) ? `<div class="resource-map-flow"><ul class="resource-map-tree" aria-label="${htmlEscape(resourceMapTypes[resourceMapType])} prompt composition">${node(root)}${remaining < 0 ? '<li class="muted small">More references are available in the template editors.</li>' : ''}</ul><div class="resource-map-result"><span aria-hidden="true">→</span><div><strong>Prompt sent to agent</strong><p class="muted small">Included pieces and runtime values are filled in before launch.</p></div></div></div>` : '<p class="muted">No templates for this agent type are available.</p>'}
    </div></details>`;
}
function bindResourceMap(data) {
  const host = document.querySelector('[data-resource-map-host]');
  if (!host) return;
  host.querySelector('.resource-map-overview').ontoggle = event => { resourceMapOpen = event.target.open; };
  const redraw = selector => {
    // These selectors commit local navigation immediately; they are not unsaved form drafts.
    host.querySelectorAll('select').forEach(select => {
      const value = select.value;
      [...select.options].forEach(option => { option.defaultSelected = option.value === value; });
    });
    resourceMapOpen = true;
    renderInto(host, renderResourceMap(data), () => bindResourceMap(data));
    host.querySelector(selector)?.focus({preventScroll:true});
  };
  host.querySelector('[data-resource-map-type]').onchange = event => { resourceMapType = event.target.value; resourceMapVariant = ''; redraw('[data-resource-map-type]'); };
  const variant = host.querySelector('[data-resource-map-variant]');
  if (variant) variant.onchange = event => { resourceMapVariant = event.target.value; redraw('[data-resource-map-variant]'); };
  host.querySelectorAll('[data-map-skill]').forEach(button => { button.onclick = () => openSkillEditor((data.skills || []).find(skill => skill.id === button.dataset.mapSkill)); });
  host.querySelectorAll('[data-map-template]').forEach(button => { button.onclick = () => openTemplateEditor(button.dataset.mapTemplate); });
  host.querySelectorAll('[data-map-skills]').forEach(button => { button.onclick = () => {
    const section = host.closest('[data-testid="settings-templates"]');
    const filter = section.querySelector('[data-resource-type]');
    section.querySelector('[data-template-catalog-search]').value = '';
    filter.value = 'Skill'; filter.dispatchEvent(new Event('change')); filter.focus();
  }; });
}
