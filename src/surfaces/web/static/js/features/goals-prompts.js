function renderGoalPrompts(goalId, round) {
  return `<section id="goal-prompts-${htmlEscape(goalId)}-${round}" data-goal-prompts="${htmlEscape(goalId)}" data-prompt-round="${round}"><p class="muted">Open Prompts to load recorded agent launches.</p></section>`;
}
async function loadGoalPrompts(root, force = false) {
  if (!root || (root.dataset.loaded && !force)) return;
  root.dataset.loaded = 'true';
  preserveDuringMorph(root);
  const generation = captureNodeContextGeneration();
  renderInto(root, '<p class="muted" role="status">Loading recorded prompts…</p>');
  try {
    const data = await api('GET', `/api/goals/${encodeURIComponent(root.dataset.goalPrompts)}/prompts`);
    if (!root.isConnected || !isNodeContextGenerationCurrent(generation)) return;
    const items = (data.items || []).filter(item => item.round_idx == null || item.round_idx === Number(root.dataset.promptRound));
    renderInto(root, `<p class="muted">The complete Refine prompt recorded at launch, using the templates and values from that run. Provider instructions and later terminal messages are not part of this record.</p>
      <div class="actions goal-prompt-controls"><label>Agent launch<select data-goal-prompt-select ${items.length ? '' : 'disabled'}>${items.map((item,index) => `<option value="${index}">${htmlEscape([item.step || item.label || 'Agent', item.skill_id, fmtTime(item.started_at), item.round_idx == null ? 'Round not recorded' : null].filter(Boolean).join(' · '))}</option>`).join('')}</select></label><button type="button" class="secondary" data-goal-prompts-refresh>Refresh</button><button type="button" class="secondary" data-goal-prompt-copy disabled>Copy prompt</button></div>
      <p class="muted small" data-goal-prompt-info></p><pre class="goal-recorded-prompt" data-goal-prompt-text></pre><p class="muted" data-goal-prompt-empty></p>`, () => {
      const show = () => {
        const item = items[Number(root.querySelector('[data-goal-prompt-select]').value)];
        const recorded = typeof item?.prompt === 'string';
        root.querySelector('[data-goal-prompt-info]').textContent = item ? [item.provider, item.state, item.id, recorded ? `${item.bytes ?? new TextEncoder().encode(item.prompt).length} bytes` : ''].filter(Boolean).join(' · ') : '';
        const text = root.querySelector('[data-goal-prompt-text]'); text.textContent = recorded ? item.prompt : ''; text.hidden = !recorded;
        root.querySelector('[data-goal-prompt-empty]').textContent = !item ? 'No recorded agent prompts for this round on this node.' : recorded ? '' : 'The full prompt was not retained for this launch. It cannot be reconstructed exactly from today’s templates.';
        root.querySelector('[data-goal-prompt-copy]').disabled = !recorded;
      };
      root.querySelector('[data-goal-prompt-select]').onchange = show;
      root.querySelector('[data-goal-prompts-refresh]').onclick = () => loadGoalPrompts(root,true);
      root.querySelector('[data-goal-prompt-copy]').onclick = async () => {
        try { await navigator.clipboard.writeText(root.querySelector('[data-goal-prompt-text]').textContent); toast('Prompt copied','success'); } catch (error) { showActionError(error); }
      };
      show();
    });
  } catch (error) {
    if (!root.isConnected || !isNodeContextGenerationCurrent(generation)) return;
    renderInto(root, `<p role="alert">${htmlEscape(error.message)}</p><button type="button" class="secondary" data-goal-prompts-retry>Retry</button>`, () => { root.querySelector('[data-goal-prompts-retry]').onclick = () => loadGoalPrompts(root,true); });
  }
}
