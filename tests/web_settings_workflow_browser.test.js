const assert = require('node:assert/strict');
const test = require('node:test');
const {openApp, apiFixture, SKIP} = require('./support/web_app');

const steps = ['backlog', 'todo', 'plan', 'implement', 'quality', 'governance', 'review', 'done', 'failed', 'cancelled'];
const sources = [...steps.flatMap(step => ['enter', 'success', 'error', 'exit'].map(hook => `workflow.${step}.${hook}`)), 'node.startup.ready', 'node.example.ready'];
function fixture() {
  let revision = 5;
  const requests = [];
  const skills = [
    {id: 'review', name: 'Review instructions', prompt: 'Check {{current_round_goal}} using {{refine_executable}} {{templates.purpose}} {{templates.architecture}}', enabled: true, scope: {}, parameters: [], trigger_source: 'workflow.review.success'},
    {id: 'context', name: 'Shared project context', prompt: 'Project context', enabled: true, scope: {}, parameters: [], trigger_source: 'workflow.review.success'},
    {id: 'startup', name: 'Startup work', prompt: 'Startup', enabled: false, scope: {node_id: 'node-a'}, parameters: [], trigger_source: 'node.startup.ready'},
    {id: 'custom', name: 'Custom work', prompt: 'Custom', enabled: true, scope: {}, parameters: [], trigger_source: 'custom'},
  ];
  const events = skills.map((skill, index) => ({id: `event-${index}`, source: skill.trigger_source === 'custom' ? null : skill.trigger_source, enabled: true, bindings: [{id: `binding-${index}`, skill_id: skill.id, enabled: true, order: skill.id === 'context' ? -1 : 2, mode: skill.id === 'context' ? 'context' : 'blocking', scope: {}}]}));
  const templates = ['workflow', 'supervised-skill', 'goal-agents-session', 'context-skill', 'goal-completion', 'manual-skill', 'purpose', 'architecture'].map(id => ({name: id, item: {id, revision: 0, prompt: '{{skill}}'}, usage: {kind: 'template', description: 'Sample template', group: 'workflow'}}));
  return {requests, fixture(path, request) {
    if (request.method() !== 'GET') requests.push({path, method: request.method(), body: request.postDataJSON()});
    if (path === '/api/skills/catalog') return {sources: ['custom', ...sources]};
    if (path === '/api/event-definitions/catalog') return {sources, completion_contract: {schema_version: 1}};
    if (path === '/api/event-definitions') return {revision, items: events};
    if (path === '/api/skills') return {revision, items: skills};
    if (path.startsWith('/api/skills/')) {
      const id = path.split('/').pop();
      if (request.method() === 'PUT') {
        const body = request.postDataJSON();
        assert.equal(body.revision, revision);
        if (body.event_bindings) {
          events.forEach(event => { event.bindings = event.bindings.filter(binding => binding.skill_id !== id); });
          body.event_bindings.forEach(assignment => events.find(event => event.id === assignment.event_id).bindings.push(assignment.binding));
        }
        const existing = skills.find(skill => skill.id === id);
        if (existing) Object.assign(existing, body.item); else skills.push({...body.item, trigger_source: body.trigger?.source});
        if (body.trigger) events.push({id: 'added', source: body.trigger.source, enabled: true, bindings: [{...body.trigger, skill_id: id, enabled: true}]});
        revision++;
      }
      const item = skills.find(skill => skill.id === id);
      return {revision, item, trigger: {source: item?.trigger_source, mode: 'blocking', order: 2, inputs: {}}};
    }
    if (path === '/api/templates') return {items: templates};
    if (path.endsWith('/preview')) return {prompt: path.includes('context-skill') ? 'Rendered project context' : 'Rendered complete prompt'};
    if (path.startsWith('/api/templates/')) return {...templates.find(row => row.item.id === path.split('/').pop()), default_prompt: '{{skill}}', variables: []};
    return apiFixture(path);
  }};
}

test('Workflow covers every step and hook, system events, custom actions, and shared resources', {skip: SKIP}, async () => {
  const data = fixture(), app = await openApp(data);
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/workflow`);
    await page.locator('[data-workflow-step="plan"]').waitFor();
    assert.equal(await page.locator('[data-workflow-step]').count(), 10);
    assert.equal(await page.locator('.settings-tabs [href="#/settings/skills"]').count(), 0);
    for (const step of steps) {
      await page.locator(`[data-workflow-step="${step}"]`).click();
      assert.equal(await page.locator('[data-workflow-hook]').count(), 4);
      for (const hook of ['enter', 'success', 'error', 'exit']) {
        await page.locator(`[data-workflow-hook="${hook}"]`).click();
        assert.equal(await page.locator('[data-workflow-add]').getAttribute('data-workflow-add'), `workflow.${step}.${hook}`);
      }
    }
    await page.locator('[data-workflow-step="review"]').click();
    await page.locator('[data-workflow-hook="success"]').click();
    assert.equal(await page.locator('[data-workflow-hook="success"]').innerText(), 'On success (2)');
    assert.equal(await page.locator('[data-workflow-assignment="review"].workflow-skill-card button').count(), 3);
    assert.deepEqual(await page.locator('[data-workflow-assignment]').evaluateAll(rows => rows.map(row => row.dataset.workflowAssignment)), ['context', 'review']);
    assert.match(await page.locator('[data-workflow-assignment="context"]').innerText(), /Order -1 · Context only · Project/);
    assert.equal(await page.locator(".workflow-prompts").count(), 0);
    assert.equal(await page.getByRole("button", {name:"Edit Review instructions Skill",exact:true}).count(), 1);
    await page.locator('[data-workflow-view="goals"]').press('ArrowRight');
    assert.equal(await page.locator('[data-workflow-view="system"]').getAttribute('aria-selected'), 'true');
    assert.match(await page.locator('[data-workflow-assignment="startup"]').innerText(), /Node: node-a · Disabled/);
    await page.locator('[data-workflow-system="node.example.ready"]').click();
    assert.match(await page.locator('.workflow-empty').innerText(), /No Skills assigned/);
    await page.locator('[data-workflow-view="custom"]').click();
    assert.equal(await page.locator('[data-workflow-assignment="custom"]').count(), 1);
    assert.equal(await page.locator("[data-workflow-template]").count(), 0);
    await page.locator('[data-workflow-view="resources"]').click();
    await page.locator('[data-testid="settings-templates"]').waitFor();
    assert.equal(await page.locator('[data-resource-skill]').count(), 4);
    assert.equal(await page.locator('[data-template-id="purpose"]').count(), 1);
    assert.equal(await page.locator('[data-template-id="architecture"]').count(), 1);
    assert.equal(await page.locator('.template-map').count(), 0);
    await page.setViewportSize({width: 390, height: 844});
    await page.locator('[data-workflow-view="goals"]').click();
    assert.equal(await page.locator('[data-testid="settings-workflow"]').evaluate(el => el.scrollWidth > el.clientWidth), false);
    assert.equal(data.requests.length, 0);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Workflow previews assigned Skills with context and session wrapper without running them', {skip: SKIP}, async () => {
  const data = fixture(), app = await openApp(data);
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/workflow`);
    await page.locator('[data-workflow-step="review"]').click();
    await page.locator('[data-workflow-hook="success"]').click();
    await page.locator('[data-workflow-assignment="review"] [data-workflow-preview]').click();
    const modal = page.locator('.template-editor-modal');
    await modal.locator('[data-template-result]').filter({hasText: 'Rendered complete prompt'}).waitFor();
    assert.equal(await modal.getByRole('tab').count(), 1);
    await modal.getByRole('tab').press('Home');
    assert.equal(await modal.getByRole('tab').getAttribute('aria-selected'), 'true');
    assert.equal(await modal.locator('[data-save]').isVisible(), false);
    const workflow = data.requests.find(request => request.path === '/api/templates/workflow/preview');
    assert.equal(workflow.body.values.skill.template, 'Check {{current_round_goal}} using {{refine_executable}} {{templates.purpose}} {{templates.architecture}}');
    assert.equal(JSON.parse(workflow.body.values.execution).role, 'task');
    assert.equal(workflow.body.values.attached_skills, 'Rendered project context');
    assert.equal(workflow.body.values.workflow_step, 'review');
    assert.equal(workflow.body.values.completion_contract, '{"schema_version":1}');
    const session = data.requests.find(request => request.path === '/api/templates/goal-agents-session/preview');
    assert.equal(session.body.values.goal_prompt, 'Rendered complete prompt');
    assert.deepEqual(session.body.values.completion_contract, {template: '{{templates.goal-completion}}'});
    assert.ok(data.requests.every(request => request.path.endsWith('/preview')));
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Adding a Skill from a non-agent hook preserves the selected trigger through save and refresh', {skip: SKIP}, async () => {
  const data = fixture(), app = await openApp(data);
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/workflow`);
    await page.locator('[data-workflow-step="done"]').click();
    await page.locator('[data-workflow-hook="exit"]').click();
    await page.locator('[data-workflow-add]').click();
    const modal = page.locator('[data-testid="automation-modal"]');
    await modal.locator('#automation-prompt').fill('Publish the result.');
    await modal.getByRole('tab', {name: 'Settings', exact: true}).click();
    assert.equal(await modal.locator('[data-trigger-source]').inputValue(), 'workflow.done.exit');
    await modal.locator('#automation-name').fill('Publish result');
    await modal.locator('[data-save]').click();
    await modal.waitFor({state: 'detached'});
    await page.locator('.workflow-assignment', {hasText: 'Publish result'}).waitFor();
    const save = data.requests.find(request => request.method === 'PUT');
    assert.equal(save.body.trigger.source, 'workflow.done.exit');
    assert.equal(save.body.item.prompt, 'Publish the result.');
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Existing Skills can be reused and assignment edits preserve other triggers', {skip: SKIP}, async () => {
  const data = fixture(), app = await openApp(data);
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/workflow`);
    await page.locator('[data-workflow-view="system"]').click();
    await page.locator('[data-workflow-existing]').click();
    const modal = page.locator('[data-testid="automation-modal"]');
    await modal.locator('#assignment-skill').selectOption('review');
    await modal.locator('#assignment-mode').selectOption('background');
    await modal.locator('#assignment-order').fill('7');
    await modal.locator('[data-save]').click();
    await modal.waitFor({state:'detached'});
    const card = page.locator('[data-workflow-assignment="review"]');
    assert.match(await card.innerText(), /Order 7 · Background/);
    let save = data.requests.at(-1).body;
    assert.equal(save.event_bindings.length, 2);
    assert.deepEqual(save.event_bindings.find(row => row.event_id === 'event-0').binding, {id:'binding-0',skill_id:'review',enabled:true,order:2,mode:'blocking',scope:{}});
    await card.locator('[data-workflow-skill]').click();
    await modal.locator('[data-skill-tab="settings"]').click();
    assert.equal(await modal.locator('[data-trigger-source]').isVisible(), false);
    await modal.locator('#automation-name').fill('Shared review');
    await modal.locator('[data-save]').click();
    await modal.waitFor({state:'detached'});
    assert.equal(data.requests.at(-1).body.trigger, undefined);
    assert.equal(data.requests.at(-1).body.event_bindings, undefined);
    await card.locator('[data-workflow-assignment-edit]').click();
    await modal.locator('#assignment-order').fill('9');
    await modal.locator('[data-save]').click();
    await modal.waitFor({state:'detached'});
    assert.match(await card.innerText(), /Order 9 · Background/);
    await card.locator('[data-workflow-assignment-edit]').click();
    await modal.locator('[data-delete]').click();
    await modal.waitFor({state:'detached'});
    assert.equal(await card.count(), 0);
    save = data.requests.at(-1).body;
    assert.equal(save.event_bindings.length, 1);
    assert.equal(save.event_bindings[0].event_id, 'event-0');
    await page.locator('[data-workflow-view="goals"]').click();
    await page.locator('[data-workflow-step="review"]').click();
    await page.locator('[data-workflow-hook="success"]').click();
    assert.match(await card.innerText(), /Shared review/);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
