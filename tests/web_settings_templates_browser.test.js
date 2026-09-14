const assert = require('node:assert/strict');
const test = require('node:test');
const {openApp, apiFixture, SKIP} = require('./support/web_app');

test('Templates editor previews nested values, saves revisions, and retains conflicting drafts', {skip: SKIP}, async () => {
  const item = {id: 'workflow', revision: 0, prompt: '{{skill}}'};
  const requests = [];
  const app = await openApp({fixture(path, request) {
    if (path === '/api/templates') return {items: [{name: 'Workflow', item, customized: item.revision > 0}]};
    if (path === '/api/templates/workflow/preview') { requests.push(request.postDataJSON()); return {prompt: 'Run /bin/refine — Keep {{skill}}'}; }
    if (path === '/api/templates/workflow') {
      if (request.method() === 'PUT') { const body = request.postDataJSON(); requests.push(body); item.prompt = body.prompt; item.revision++; }
      return {item, name: 'Workflow', default_prompt: '{{skill}}', variables: [{name: 'skill', description: 'Selected Skill'}, {name: 'message', description: 'User message'}]};
    }
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/templates`);
    await page.locator('[data-template-id="workflow"]').first().focus();
    await page.keyboard.press('Enter');
    const dialog = page.locator('[data-testid="automation-modal"]');
    await dialog.waitFor();
    assert.equal(await dialog.locator('[data-delete]').count(), 0);
    assert.equal(await dialog.getByRole('tab').count(), 4);
    await dialog.getByRole('tab', {name: 'Variables', exact: true}).click();
    await dialog.locator('#template-variable-search').fill('User message');
    assert.equal(await dialog.locator('.template-variable:visible').count(), 1);
    await dialog.getByRole('button', {name: 'Insert message', exact: true}).click();
    assert.equal(await dialog.getByRole('tab', {name: 'Edit', exact: true}).getAttribute('aria-selected'), 'true');
    assert.match(await dialog.locator('#template-prompt').inputValue(), /{{message}}/);
    await dialog.getByRole('tab', {name: 'Edit', exact: true}).press('End');
    assert.equal(await dialog.getByRole('tab', {name: 'Default', exact: true}).getAttribute('aria-selected'), 'true');
    await dialog.getByRole('tab', {name: 'Default', exact: true}).press('Home');
    await dialog.locator('#template-prompt').fill('Only {{skill}}');
    await dialog.getByRole('tab', {name: 'Preview', exact: true}).click();
    await page.waitForFunction(() => document.querySelector('[data-template-preview-status]')?.textContent === 'Preview ready');
    await dialog.locator('summary', {hasText: 'Sample values'}).click();
    await dialog.locator('#template-values').fill(JSON.stringify({skill: {template: 'Run {{refine_executable}} — {{current_round_goal}}'}, current_round_goal: 'Keep {{skill}}'}));
    await dialog.locator('[data-template-preview]').click();
    await page.waitForFunction(() => document.querySelector('[data-template-preview-status]')?.textContent === 'Preview ready');
    assert.equal(requests[0].prompt, 'Only {{skill}}');
    assert.equal(requests[1].values.skill.template, 'Run {{refine_executable}} — {{current_round_goal}}');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.deepEqual(requests[2], {revision: 0, prompt: 'Only {{skill}}'});
    await page.locator('[data-template-id="workflow"]').first().click();
    await dialog.waitFor();
    await dialog.locator('#template-prompt').fill('My retained draft');
    await page.route('**/api/templates/workflow', route => route.request().method() === 'PUT'
      ? route.fulfill({status: 409, contentType: 'application/json', body: JSON.stringify({error: 'Template changed'})}) : route.fallback());
    await dialog.locator('[data-save]').click();
    await page.waitForFunction(() => document.querySelector('[data-automation-error]')?.textContent.length > 0);
    assert.equal(await dialog.locator('#template-prompt').inputValue(), 'My retained draft');
    await page.setViewportSize({width: 390, height: 844});
    const layout = await dialog.evaluate(el => ({overflows: el.scrollWidth > el.clientWidth, saveBottom: el.querySelector('[data-save]').getBoundingClientRect().bottom, viewport: innerHeight}));
    assert.equal(layout.overflows, false);
    assert.ok(layout.saveBottom <= layout.viewport);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Resource catalog explains uses, filters types, and follows edited references', {skip: SKIP}, async () => {
  const rows = [
    {item: {id: 'workflow', revision: 0, prompt: '{{templates.workflow-context}}\n{{templates.supervised-skill}}'}, name: 'Workflow', usage: {kind: 'template', group: 'workflow', description: 'Used in all four workflow steps.'}},
    {item: {id: 'workflow-context', revision: 0, prompt: 'Shared coordination context'}, name: 'Workflow Context', usage: {kind: 'partial', group: 'workflow', description: 'Shared workflow guidance.'}},
    {item: {id: 'supervised-skill', revision: 0, prompt: '{{templates.workflow-context}}\n{{skill}}'}, name: 'Supervised Skill', usage: {kind: 'template', group: 'workflow', description: 'Combines the assigned Skill with context.'}},
    {item: {id: 'source-upgrade', revision: 0, prompt: 'Upgrade instructions'}, name: 'Source Upgrade', usage: {kind: 'template', group: 'tasks', description: 'Used to prepare a source upgrade.'}},
  ];
  const app = await openApp({fixture(path, request) {
    if (path === '/api/templates') return {items: rows};
    if (path === '/api/templates/workflow') {
      if (request.method() === 'PUT') { rows[0].item.prompt = request.postDataJSON().prompt; rows[0].item.revision++; }
      return {...rows[0], variables: [], default_prompt: '{{templates.workflow-context}}\n{{templates.supervised-skill}}'};
    }
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/templates`);
    await page.locator('[data-template-catalog-row]').first().waitFor();
    assert.equal(await page.locator('.template-map').count(), 0);
    assert.equal(await page.locator('[data-template-catalog-row] button').count(), 0);
    assert.equal(await page.locator('[data-template-view]').count(), 0);
    const partialRow = page.locator('[data-template-catalog-row][data-template-id="workflow-context"]');
    assert.match(await partialRow.innerText(), /Partial/);
    assert.match(await partialRow.innerText(), /Included by .*Workflow/);
    await page.locator('[data-template-catalog-search]').fill('source upgrade');
    assert.equal(await page.locator('[data-template-catalog-row]:visible').count(), 1);
    await page.locator('[data-template-catalog-search]').fill('');
    await page.locator('[data-template-id="workflow"]').first().click();
    const dialog = page.locator('.template-editor-modal');
    await dialog.waitFor();
    await dialog.locator('#template-prompt').fill('No included partials');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.match(await partialRow.innerText(), /Included by Supervised Skill/);
    assert.doesNotMatch(await partialRow.innerText(), /Included by Workflow/);
    await page.locator('[data-resource-type]').selectOption('Partial');
    assert.equal(await page.locator('[data-template-catalog-row]:visible').count(), 1);
    await page.locator('[data-template-catalog-search]').fill('does not exist');
    assert.equal(await page.locator('[data-template-catalog-empty]').isVisible(), true);
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});

test('Resource pagination stays bounded and search reaches entries on later pages', {skip: SKIP}, async () => {
  const rows = Array.from({length:101}, (_,i) => ({item:{id:`entry-${i}`,prompt:'Instructions',revision:0},name:`Resource ${String(i).padStart(2,'0')}`,usage:{kind:'partial',description:'Shared guidance'}}));
  const app = await openApp({fixture(path) { return path === '/api/templates' ? {items:rows} : apiFixture(path); }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/templates`);
    await page.locator('[data-resource-next]').click();
    assert.equal(await page.locator('[data-template-catalog-row]:visible').count(),50);
    assert.equal(await page.locator('[data-resource-range]').innerText(),'51–100 of 101 resources');
    await page.locator('[data-resource-next]').click();
    assert.equal(await page.locator('[data-template-catalog-row]:visible').count(),1);
    assert.equal(await page.locator('[data-resource-next]').isDisabled(),true);
    await page.locator('[data-template-catalog-search]').fill('Resource 02');
    assert.equal(await page.locator('[data-template-catalog-row]:visible').count(),1);
    assert.equal(await page.locator('[data-template-id="entry-2"]').isVisible(),true);
    assert.equal(await page.locator('[data-resource-previous]').isDisabled(),true);
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});
