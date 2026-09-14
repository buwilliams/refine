function resourceMapTemplateIds(data, type) {
  const ids = new Set();
  for (const [variant] of resourceMapVariants(data, type)) {
    for (const defaults of [false, true]) {
      const graph = resourceMapGraph(data, type, variant, defaults), visited = new Set();
      const visit = id => {
        if (visited.has(id)) return;
        visited.add(id);
        if (id === '$skill') { graph.skills.forEach(skill => visit(`skill:${skill.id}`)); return; }
        if (!graph.rows.has(id)) return;
        if (!id.startsWith('skill:')) ids.add(id);
        graph.children(id).forEach(child => visit(child.id));
      };
      visit(graph.root);
    }
  }
  return ids;
}
async function openResourceReset({id, type}) {
  if (automationEditor || automationEditorOpening) return;
  automationEditorOpening = true;
  const generation = captureNodeContextGeneration();
  let data;
  try {
    const workflow = await loadWorkflowSettings();
    data = {...workflow.templates, workflowData:workflow};
    if (!isNodeContextGenerationCurrent(generation)) return;
  } catch (error) { showActionError(error); return; }
  finally { automationEditorOpening = false; }
  const ids = type ? resourceMapTemplateIds(data,type) : new Set([id]);
  const rows = data.items.filter(row => ids.has(row.item.id)).sort((a,b)=>a.name.localeCompare(b.name));
  const changed = rows.filter(row => row.item.prompt !== row.default_prompt);
  const otherTypes = Object.entries(resourceMapTypes).filter(([key]) => key !== type).map(([key,label])=>({label,ids:resourceMapTemplateIds(data,key)}));
  const root = automationModal(type ? `Reset ${resourceMapTypes[type]} templates` : `Reset ${rows[0]?.name || id}`, `
    <p>Restore the built-in defaults for ${type ? `all launch paths of the ${htmlEscape(resourceMapTypes[type])}` : 'this template'}. Skill instructions are not reset.</p>
    <p class="muted">Shared templates also affect the other agent types listed below. Templates outside this list stay unchanged.</p>
    <div class="automation-table-scroll"><table class="table"><thead><tr><th>Template</th><th>Change</th><th>Also used by</th></tr></thead><tbody>${rows.map(row=>`<tr><td>${htmlEscape(row.name)}</td><td>${row.item.prompt === row.default_prompt ? 'Already default' : 'Restore default'}</td><td>${htmlEscape(otherTypes.filter(other=>other.ids.has(row.item.id)).map(other=>other.label).join(', ') || '—')}</td></tr>`).join('')}</tbody></table></div>
    <p>${changed.length ? `${changed.length} ${changed.length === 1 ? 'template will' : 'templates will'} be reset. This takes effect when saved.` : 'All selected templates already match their defaults.'}</p>`);
  automationEditor = root;
  root.querySelector('[data-delete]').remove();
  const save = root.querySelector('[data-save]');
  save.textContent = 'Reset to default'; save.disabled = !changed.length;
  let saving = false;
  save.onclick = async () => {
    if (saving) return;
    saving = true; save.disabled = true;
    try {
      if (!isNodeContextGenerationCurrent(generation)) throw new Error('The selected project changed. Reopen the reset preview.');
      await api('POST','/api/templates/reset',{revisions:Object.fromEntries(rows.map(row=>[row.item.id,row.item.revision]))});
      root._close(); await refreshSettings({force:true});
    } catch (error) {
      root.querySelector('[data-automation-error]').textContent = error.status === 409 ? 'Templates changed since this preview. Cancel and reopen it to review the latest changes.' : error.message;
    } finally { saving = false; save.disabled = !changed.length; }
  };
}
