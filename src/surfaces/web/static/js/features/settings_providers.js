// Provider drafts are scoped to the selected project and node, separate from server refreshes.
function providerDraftKey(kind = 'editor') {
  return `refine.providers.${kind}:${JSON.stringify([nodeContextTargetRoot || state.project?.target_root || '', nodeContextActiveNodeId()])}`;
}
function providerStored(key, value) {
  try {
    if (value === undefined) return JSON.parse(localStorage.getItem(key) || 'null');
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, JSON.stringify(value));
  } catch { /* Storage can be unavailable; the open form still retains edits. */ }
  return null;
}
function providerOptions(catalog, selected) {
  const missing = selected && !catalog.providers.some(p => p.id === selected)
    ? `<option value="${htmlEscape(selected)}" selected>Unavailable provider (${htmlEscape(selected)})</option>` : '';
  return missing + catalog.providers.map(p => `<option value="${htmlEscape(p.id)}" ${p.id === selected ? 'selected' : ''}>${htmlEscape(p.name)}</option>`).join('');
}
function renderProviderCatalog(data = {}) {
  const catalog = data.catalog;
  if (!catalog) return '';
  const pending = providerStored(providerDraftKey('default'));
  const effective = catalog.providers.find(p => p.id === data.effective_provider);
  return `<section class="settings-section" data-testid="provider-catalog">
    <div class="provider-heading"><div><h3>Shared AI providers</h3><p class="muted small">Definitions and the system default are shared across nodes.</p></div><button type="button" id="provider-add">Add provider</button></div>
    <div class="form-row"><label for="provider-system-default">System default</label><div class="provider-selection"><select id="provider-system-default">${providerOptions(catalog, pending?.id ?? catalog.default_provider)}</select><button type="button" class="secondary" id="provider-save-default">Save default</button></div></div>
    <p class="muted small">Effective on this node: <strong>${htmlEscape(effective?.name || data.effective_provider || catalog.default_provider)}</strong> · ${data.selection_source === 'node' ? 'Node override' : 'Inherited system default'}. Change this node’s selection above.</p>
    <div class="provider-list">${catalog.providers.map(p => `<div class="provider-row"><div><strong>${htmlEscape(p.name)}</strong>${p.id === catalog.default_provider ? ' <span class="muted small">System default</span>' : ''}<div class="muted small"><code>${htmlEscape(p.executable)}</code></div></div><button type="button" class="secondary" data-provider-edit="${htmlEscape(p.id)}" aria-label="Edit ${htmlEscape(p.name)}">Edit</button></div>`).join('')}</div>
    <button type="button" class="secondary" id="provider-restore" ${providerStored(providerDraftKey()) ? '' : 'hidden'}>Resume unsaved provider draft</button>
    <button type="button" class="secondary" id="provider-discard" ${providerStored(providerDraftKey()) ? '' : 'hidden'}>Discard unsaved draft</button>
    <p role="status" id="provider-catalog-status" class="muted small"></p><p role="alert" class="form-error" id="provider-catalog-error"></p>
    <button type="button" class="secondary" id="provider-default-rebase" hidden>Review latest default</button>
  </section>`;
}
function bindProviderCatalog(data = {}) {
  if (!data.catalog) return;
  const generation = captureNodeContextGeneration(), key = providerDraftKey('default');
  const current = () => isNodeContextGenerationCurrent(generation);
  document.querySelectorAll('[data-provider-edit]').forEach(b => b.onclick = () => openProviderEditor(data.catalog, b.dataset.providerEdit));
  document.querySelector('#provider-add').onclick = () => openProviderEditor(data.catalog);
  document.querySelector('#provider-restore').onclick = () => {
    const saved = providerStored(providerDraftKey());
    if (saved) openProviderEditor(saved.base, saved.id, saved.record);
  };
  document.querySelector('#provider-discard').onclick = () => { providerStored(providerDraftKey(), null); document.querySelector('#provider-restore').hidden = true; document.querySelector('#provider-discard').hidden = true; };
  const select = document.querySelector('#provider-system-default'), save = document.querySelector('#provider-save-default');
  select.onchange = () => providerStored(key, {id: select.value, base: providerStored(key)?.base || data.catalog});
  save.onclick = async () => {
    if (!current()) return;
    const pending = providerStored(key) || {id: select.value, base: data.catalog};
    const catalog = structuredClone(pending.base);
    catalog.default_provider = pending.id;
    save.disabled = select.disabled = true; save.textContent = 'Saving…';
    try {
      await api('PUT', '/api/providers', catalog);
      if (!current()) return;
      providerStored(key, null);
      await refreshSettingsTab('runtime', {force: true});
      if (current()) document.querySelector('#provider-catalog-status').textContent = 'System default saved.';
    } catch (error) {
      if (!current()) return;
      document.querySelector('#provider-catalog-error').textContent = error.message;
      if (error.status === 409) document.querySelector('#provider-default-rebase').hidden = false;
    } finally { if (current()) { save.disabled = select.disabled = false; save.textContent = 'Save default'; } }
  };
  document.querySelector('#provider-default-rebase').onclick = async () => {
    try {
      const latest = (await api('GET', '/api/providers')).catalog;
      if (!current()) return;
      const pending = providerStored(key);
      if (!pending) return;
      // Explicit review is required even if only an unrelated definition changed.
      const name = latest.providers.find(p => p.id === latest.default_provider)?.name || latest.default_provider;
      providerStored(key, {...pending, base: latest});
      document.querySelector('#provider-catalog-error').textContent = `Latest default: ${name}. Your selection is retained. Choose Save default to apply it, or select the latest default.`;
      document.querySelector('#provider-default-rebase').hidden = true;
    } catch (error) { if (current()) document.querySelector('#provider-catalog-error').textContent = error.message; }
  };
}
function providerField(id, label, control, help = '') {
  return `<div class="form-row"><label for="${id}">${label}</label>${control}${help ? `<span class="muted small">${help}</span>` : ''}</div>`;
}
function providerArgs(name, label, values = []) {
  return `<div class="form-row provider-arguments" id="provider-${name}" data-args="${name}"><label>${label}</label><div data-argument-rows>${values.map((v, i) => providerArgRow(name, v, i)).join('')}</div><button type="button" class="secondary" data-add-argument="${name}">Add argument</button></div>`;
}
function providerArgRow(name, value, index) {
  return `<div class="provider-argument"><label class="muted small" for="provider-${name}-${index}">${index + 1}</label><textarea id="provider-${name}-${index}" aria-label="${name.replaceAll('-', ' ')} argument ${index + 1}" rows="${Math.max(1, Math.min(5, value.split(/\r\n|\r|\n/).length))}" spellcheck="false" data-original="${htmlEscape(JSON.stringify(value))}">
${htmlEscape(value)}</textarea><div class="provider-argument-actions"><button type="button" class="secondary" data-move="-1" aria-label="Move argument up">↑</button><button type="button" class="secondary" data-move="1" aria-label="Move argument down">↓</button><button type="button" class="secondary" data-remove-argument aria-label="Remove argument">Remove</button></div></div>`;
}
function providerModeEditor(name, mode) {
  const field = (key, label, control, help) => providerField(`provider-${name}-${key}`, label, control, help);
  return `${providerArgs(`${name}-args`, 'Arguments', mode.args)}
    ${providerArgs(`${name}-context_args`, 'Context arguments', mode.context_args)}
    <p class="muted small">One row is one argument, including spaces or line breaks. Context arguments follow when context is present. Use {{context}} for the request; no shell parsing occurs.</p>
    ${name === 'automated' ? field('transport', 'Context delivery', `<select id="provider-${name}-transport"><option value="inline_or_file">Arguments (large requests use a private file)</option><option value="native_stdin" ${mode.transport === 'native_stdin' ? 'selected' : ''}>Standard input</option></select>`) : ''}
    <div data-stdin-row ${mode.transport === 'native_stdin' ? '' : 'hidden'}>${field('stdin', 'Standard input template', `<textarea id="provider-${name}-stdin" rows="2" spellcheck="false" data-original="${htmlEscape(JSON.stringify(mode.stdin ?? '{{context}}'))}">
${htmlEscape(mode.stdin ?? '{{context}}')}</textarea>`)}</div>
    <details class="automation-optional"><summary>Working directory and sessions</summary>
      ${providerArgs(`${name}-cwd_args`, 'Working directory arguments', mode.cwd_args)}
      <label class="provider-checkbox"><input type="checkbox" id="provider-${name}-cwd_on_resume" ${mode.cwd_on_resume !== false ? 'checked' : ''}> Include working directory arguments when resuming</label>
      ${['resume', 'pin'].map(kind => `<label class="provider-checkbox"><input type="checkbox" data-session-toggle="${name}-${kind}" ${mode[`${kind}_args`] != null ? 'checked' : ''}> ${kind === 'resume' ? 'Support resuming a session' : 'Support choosing a session ID'}</label><div data-session-args="${name}-${kind}" ${mode[`${kind}_args`] != null ? '' : 'hidden'}>${providerArgs(`${name}-${kind}_args`, kind === 'resume' ? 'Resume arguments (replace base arguments)' : 'Session ID prefix arguments', mode[`${kind}_args`] || [])}</div>`).join('')}
      <p class="muted small">Use {{session_id}} in session arguments. {{cwd}} is available for automated launches with an explicit directory.</p>
    </details>`;
}
function providerTextValue(el) {
  return el.hasAttribute('data-original') ? JSON.parse(el.dataset.original) : el.value;
}
function readProviderArgs(root, name) {
  return [...root.querySelectorAll(`[data-args="${name}"] textarea`)].map(providerTextValue);
}
function readProviderMode(root, name) {
  const transport = root.querySelector(`#provider-${name}-transport`)?.value || 'inline_or_file';
  return {args: readProviderArgs(root, `${name}-args`), context_args: readProviderArgs(root, `${name}-context_args`), cwd_args: readProviderArgs(root, `${name}-cwd_args`),
    cwd_on_resume: root.querySelector(`#provider-${name}-cwd_on_resume`).checked, transport,
    stdin: transport === 'native_stdin' ? providerTextValue(root.querySelector(`#provider-${name}-stdin`)) : null,
    resume_args: root.querySelector(`[data-session-toggle="${name}-resume"]`).checked ? readProviderArgs(root, `${name}-resume_args`) : null,
    pin_args: root.querySelector(`[data-session-toggle="${name}-pin"]`).checked ? readProviderArgs(root, `${name}-pin_args`) : null};
}
function providerCredentialRow(target = '', source = '') {
  return `<div class="provider-credential"><label>Child environment name<input type="text" data-credential-target value="${htmlEscape(target)}" placeholder="OPENAI_API_KEY" autocomplete="off"></label><label>Local environment reference<input type="text" data-credential-source value="${htmlEscape(source)}" placeholder="MY_AGENT_KEY" autocomplete="off"></label><button type="button" class="secondary" data-remove-credential>Remove</button></div>`;
}
// Arrays are indivisible argv sequences. Reconcile other fields independently.
function mergeProviderDraft(base, mine, latest, path = '', conflicts = []) {
  const equal = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  if (equal(mine, base)) return structuredClone(latest);
  if (equal(latest, base) || equal(mine, latest)) return structuredClone(mine);
  if (base && mine && latest && [base, mine, latest].every(v => typeof v === 'object' && !Array.isArray(v))) {
    return Object.fromEntries([...new Set([...Object.keys(base), ...Object.keys(mine), ...Object.keys(latest)])].map(key => [key, mergeProviderDraft(base[key], mine[key], latest[key], path ? `${path}.${key}` : key, conflicts)]).filter(([, value]) => value !== undefined));
  }
  conflicts.push({path, mine, latest});
  return structuredClone(mine);
}
function openProviderEditor(catalog, id, restored) {
  if (automationEditor) return;
  const generation = captureNodeContextGeneration(), storageKey = providerDraftKey();
  let base = structuredClone(catalog), revisionConflict = false;
  const provider = structuredClone(restored || base.providers.find(p => p.id === id) || {
    id: '', name: '', executable: '', credentials: {}, output_format: 'plain',
    automated: {context_args: ['{{context}}']}, interactive: {context_args: ['{{context}}']},
  });
  const input = (key, label, help) => providerField(`provider-${key}`, label, `<input type="text" id="provider-${key}" value="${htmlEscape(provider[key])}" ${key === 'id' && id ? 'readonly' : ''} autocomplete="off">`, help);
  const root = automationModal(id ? `Edit ${provider.name}` : 'Add AI provider', `<div class="provider-editor">
    <p class="muted small">Shared across nodes. Changes apply to subsequent launches.</p>
    ${input('name', 'Name')}${input('executable', 'Executable', 'A command on PATH or an absolute path on the launching host.')}
    ${input('id', 'Stable ID', 'Identifies this configuration. Two configurations can use the same executable.')}
    <section class="automation-section"><h3>Automated launches</h3>${providerModeEditor('automated', provider.automated)}</section>
    <details class="automation-optional"><summary>Interactive terminal configuration</summary>${providerModeEditor('interactive', provider.interactive)}</details>
    <details class="automation-optional"><summary>Credentials and output</summary>
      <p class="muted small">Enter environment variable names only. Set their values in the launching host’s environment; never enter a credential here.</p>
      <div data-credentials>${Object.entries(provider.credentials || {}).map(([k, v]) => providerCredentialRow(k, v)).join('')}</div><button type="button" class="secondary" data-add-credential>Add credential reference</button>
      ${providerField('provider-output', 'Output format', `<select id="provider-output">${['plain', 'claude_json', 'codex_json', 'copilot_json'].map(v => `<option ${v === provider.output_format ? 'selected' : ''}>${v}</option>`).join('')}</select>`, 'Use plain text unless your CLI produces a supported structured format.')}
    </details><div data-provider-conflicts></div><button type="button" class="secondary" data-provider-rebase hidden>Load latest and reapply draft</button>
    <p class="muted small" role="status" data-provider-status>${restored ? 'Unsaved draft restored.' : ''}</p></div>`);
  automationEditor = root;
  const error = root.querySelector('[data-automation-error]'), save = root.querySelector('[data-save]'), remove = root.querySelector('[data-delete]');
  const current = () => root.isConnected && isNodeContextGenerationCurrent(generation);
  root._onClose = () => {
    if (!current()) return;
    const retained = !!providerStored(storageKey);
    for (const selector of ['#provider-restore', '#provider-discard']) { const button = document.querySelector(selector); if (button) button.hidden = !retained; }
  };
  remove.hidden = !id;
  function record() {
    return {id: root.querySelector('#provider-id').value, name: root.querySelector('#provider-name').value, executable: root.querySelector('#provider-executable').value,
      credentials: Object.fromEntries([...root.querySelectorAll('.provider-credential')].map(row => [row.querySelector('[data-credential-target]').value, row.querySelector('[data-credential-source]').value])),
      output_format: root.querySelector('#provider-output').value, automated: readProviderMode(root, 'automated'), interactive: readProviderMode(root, 'interactive')};
  }
  function retain() {
    root.dataset.nodeContextDirty = 'true';
    const draft = record();
    // Only reference names belong in browser storage; accidental pasted values stay in the open form.
    draft.credentials = Object.fromEntries(Object.entries(draft.credentials).filter(([target, source]) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(target) && /^[A-Za-z_][A-Za-z0-9_]*$/.test(source)));
    providerStored(storageKey, {base, id, record: draft});
    root.querySelector('[data-provider-status]').textContent = 'Draft retained in this browser.';
  }
  function renumber(group) {
    const rows = [...group.querySelectorAll('.provider-argument')];
    rows.forEach((row, i) => {
      const textarea = row.querySelector('textarea'), label = row.querySelector('label');
      textarea.id = `provider-${group.dataset.args}-${i}`; label.htmlFor = textarea.id; label.textContent = i + 1;
      textarea.setAttribute('aria-label', `${group.dataset.args.replaceAll('-', ' ')} argument ${i + 1}`);
      row.querySelector('[data-move="-1"]').disabled = i === 0;
      row.querySelector('[data-move="1"]').disabled = i === rows.length - 1;
    });
  }
  root.querySelectorAll('[data-args]').forEach(renumber);
  root.addEventListener('click', event => {
    const b = event.target.closest('button');
    if (!b) return;
    if (b.hasAttribute('data-add-argument')) {
      const group = b.closest('[data-args]');
      group.querySelector('[data-argument-rows]').insertAdjacentHTML('beforeend', providerArgRow(group.dataset.args, '', 0));
      renumber(group); group.querySelector('.provider-argument:last-child textarea').focus(); retain();
    }
    if (b.hasAttribute('data-remove-argument') || b.hasAttribute('data-move')) {
      const row = b.closest('.provider-argument'), group = row.closest('[data-args]');
      if (b.hasAttribute('data-remove-argument')) row.remove();
      else if (b.dataset.move === '-1' && row.previousElementSibling) row.previousElementSibling.before(row);
      else if (b.dataset.move === '1' && row.nextElementSibling) row.nextElementSibling.after(row);
      renumber(group); (row.isConnected ? row.querySelector('textarea') : group.querySelector('[data-add-argument]')).focus(); retain();
    }
    if (b.hasAttribute('data-add-credential')) { root.querySelector('[data-credentials]').insertAdjacentHTML('beforeend', providerCredentialRow()); retain(); }
    if (b.hasAttribute('data-remove-credential')) { b.closest('.provider-credential').remove(); retain(); }
  });
  let suggested = !id && !provider.id;
  root.querySelector('#provider-id').addEventListener('input', () => { suggested = false; });
  root.querySelector('#provider-name').addEventListener('input', event => {
    if (suggested) root.querySelector('#provider-id').value = event.target.value.toLowerCase().replace(/[^a-z0-9_-]+/g, '-').replace(/^-|-$/g, '');
  });
  root.addEventListener('input', event => { event.target.removeAttribute('data-original'); if (event.target.matches('[data-args] textarea')) event.target.rows = Math.max(1, Math.min(5, event.target.value.split('\n').length)); retain(); });
  root.addEventListener('change', event => {
    if (event.target.id === 'provider-automated-transport') {
      const stdin = event.target.value === 'native_stdin';
      root.querySelector('[data-stdin-row]').hidden = !stdin;
      const group = root.querySelector('[data-args="automated-context_args"]');
      const args = readProviderArgs(root, 'automated-context_args');
      if (stdin && args.length === 1 && args[0] === '{{context}}') renderInto(group.querySelector('[data-argument-rows]'), '');
      else if (!stdin && args.length === 0) renderInto(group.querySelector('[data-argument-rows]'), providerArgRow('automated-context_args', '{{context}}', 0));
      renumber(group);
    }
    if (event.target.dataset.sessionToggle) root.querySelector(`[data-session-args="${event.target.dataset.sessionToggle}"]`).hidden = !event.target.checked;
    retain();
  });
  async function submit(deleting) {
    if (!current()) { error.textContent = 'The project or node changed. Reopen this draft in its original context.'; return; }
    retain(); error.textContent = '';
    const value = record();
    if (deleting && base.default_provider === id) { error.textContent = 'Choose another system default before deleting this provider.'; return; }
    if (!deleting) {
      const invalid = validateProviderForm(root, value);
      if (invalid) { error.textContent = invalid; return; }
    }
    const controls = [...root.querySelectorAll('input, textarea, select, button:not([data-close])')].map(control => [control, control.disabled]);
    controls.forEach(([control]) => { control.disabled = true; }); save.textContent = 'Saving…';
    try {
      const updated = structuredClone(base), index = updated.providers.findIndex(p => p.id === id);
      if (deleting) updated.providers = updated.providers.filter(p => p.id !== id);
      else if (index < 0) updated.providers.push(value); else updated.providers[index] = value;
      await api('PUT', '/api/providers', updated);
      if (!current()) return;
      providerStored(storageKey, null); root._close();
      await refreshSettingsTab('runtime', {force: true});
      if (isNodeContextGenerationCurrent(generation)) document.querySelector('#provider-catalog-status').textContent = deleting ? 'Provider deleted.' : 'Provider saved.';
    } catch (failure) {
      if (!current()) return;
      error.textContent = failure.message;
      revisionConflict = failure.status === 409 && !/selected by node|before deleting|still selected/i.test(failure.message);
      root.querySelector('[data-provider-rebase]').hidden = !revisionConflict;
    } finally { if (current()) { controls.forEach(([control, disabled]) => { control.disabled = disabled; }); save.textContent = 'Save'; } }
  }
  save.onclick = () => submit(false); remove.onclick = () => submit(true);
  root.querySelector('[data-provider-rebase]').onclick = async () => {
    try {
      const latest = (await api('GET', '/api/providers')).catalog;
      if (!current()) return;
      const conflicts = [], mine = record(), original = base.providers.find(p => p.id === id), remote = latest.providers.find(p => p.id === (id || mine.id));
      const merged = mergeProviderDraft(original, mine, remote, '', conflicts);
      const panel = root.querySelector('[data-provider-conflicts]');
      renderInto(panel, `<h3>Review concurrent changes</h3><p class="muted small">Unrelated changes are retained. Choose which value to keep for each overlap.</p>${conflicts.map((c, i) => `<div class="form-row"><label for="provider-conflict-${i}">${htmlEscape(c.path || 'Provider (added or deleted remotely)')}</label><pre>${htmlEscape(JSON.stringify({draft: c.mine ?? '(deleted)', latest: c.latest ?? '(deleted)'}, null, 2))}</pre><select id="provider-conflict-${i}"><option value="">Choose a value</option><option value="mine">Keep my draft</option><option value="latest">Use latest</option></select></div>`).join('')}<button type="button" class="secondary" data-apply-rebase>Apply reviewed changes</button>`);
      panel.querySelectorAll('select').forEach(control => { control.value = ''; });
      save.disabled = true;
      error.textContent = conflicts.length ? 'Resolve the overlapping changes below before saving.' : 'No overlapping edits. Apply the merged draft, then save.';
      panel.querySelector('[data-apply-rebase]').onclick = () => {
        if (!current()) { error.textContent = 'The project or node changed. Reopen this draft in its original context.'; return; }
        if (JSON.stringify(record()) !== JSON.stringify(mine)) { error.textContent = 'Your draft changed during review. Load latest and reapply again to include those edits.'; return; }
        let resolved = structuredClone(merged);
        for (let i = 0; i < conflicts.length; i++) {
          const choice = panel.querySelector(`#provider-conflict-${i}`).value;
          if (!choice) { error.textContent = 'Choose a value for every overlapping change.'; return; }
          const conflict = conflicts[i], value = conflict[choice];
          if (!conflict.path) resolved = value;
          else { const parts = conflict.path.split('.'); let target = resolved; for (const part of parts.slice(0, -1)) target = target[part]; if (value === undefined) delete target[parts.at(-1)]; else target[parts.at(-1)] = value; }
        }
        if (!resolved) { providerStored(storageKey, null); root._close(); refreshSettingsTab('runtime', {force: true}); return; }
        base = latest;
        providerStored(storageKey, {base, id: remote?.id, record: resolved});
        root._close(); openProviderEditor(base, remote?.id, resolved);
      };
    } catch (failure) { if (current()) error.textContent = failure.message; }
  };
}
function validateProviderForm(root, record) {
  root.querySelectorAll('[aria-invalid]').forEach(el => el.removeAttribute('aria-invalid'));
  for (const key of ['name', 'executable', 'id']) {
    if (!record[key].trim() || record[key].includes('\0') || (key === 'id' && record[key] !== record[key].trim())) {
      const field = root.querySelector(`#provider-${key}`); field.setAttribute('aria-invalid', 'true'); field.focus(); return `Enter a valid ${key}.`;
    }
  }
  const names = [...root.querySelectorAll('[data-credential-target], [data-credential-source]')];
  const targets = names.filter(el => el.hasAttribute('data-credential-target')).map(el => el.value);
  for (const field of names) if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(field.value) || (field.hasAttribute('data-credential-target') && targets.filter(v => v === field.value).length > 1)) {
    field.closest('details').open = true; field.setAttribute('aria-invalid', 'true'); field.focus(); return 'Use unique child variable names and local environment reference names, never credential values.';
  }
  for (const field of root.querySelectorAll('[data-args] textarea, [id$="-stdin"]')) {
    if (field.closest('[hidden]')) continue;
    let tail = field.value, invalid = tail.includes('\0');
    while (tail.includes('{{')) {
      tail = tail.slice(tail.indexOf('{{') + 2); const end = tail.indexOf('}}');
      if (end < 0 || !['context', 'cwd', 'session_id'].includes(tail.slice(0, end))) { invalid = true; break; }
      tail = tail.slice(end + 2);
    }
    if (invalid) { let parent = field.parentElement; while (parent && parent !== root) { if (parent.tagName === 'DETAILS') parent.open = true; parent = parent.parentElement; } field.setAttribute('aria-invalid', 'true'); field.focus(); return 'Use supported templates: {{context}}, {{cwd}}, and {{session_id}} (session arguments only).'; }
  }
  for (const name of ['automated', 'interactive']) {
    const mode = record[name];
    if ([...mode.args, ...mode.context_args, ...mode.cwd_args, mode.stdin || ''].some(value => value.includes('{{session_id}}'))) return 'Use {{session_id}} only in resume or session ID prefix arguments.';
    if (mode.transport === 'native_stdin' && [...mode.args, ...mode.context_args, ...mode.cwd_args, ...(mode.resume_args || []), ...(mode.pin_args || [])].some(value => value.includes('{{context}}'))) return 'With standard input delivery, put {{context}} in the standard input template and remove it from argument rows.';
  }
  return '';
}
