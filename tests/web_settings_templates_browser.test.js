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
      return {item, name: 'Workflow', default_prompt: '{{skill}}', variables: [{name: 'skill', description: 'Selected Skill'}]};
    }
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(`${app.origin}/#/settings/templates`);
    await page.locator('[data-template-id="workflow"]').click();
    const dialog = page.locator('[data-testid="automation-modal"]');
    assert.equal(await dialog.locator('[data-delete]').count(), 0);
    await dialog.locator('#template-prompt').fill('Only {{skill}}');
    await dialog.locator('summary', {hasText: 'Preview with sample values'}).click();
    await dialog.locator('#template-values').fill(JSON.stringify({skill: {template: 'Run {{refine_executable}} — {{current_round_goal}}'}, current_round_goal: 'Keep {{skill}}'}));
    await dialog.locator('[data-template-preview]').click();
    await page.waitForFunction(() => document.querySelector('[data-template-result]')?.textContent.includes('/bin/refine'));
    assert.equal(requests[0].prompt, 'Only {{skill}}');
    assert.equal(requests[0].values.skill.template, 'Run {{refine_executable}} — {{current_round_goal}}');
    await dialog.locator('[data-save]').click();
    await dialog.waitFor({state: 'detached'});
    assert.deepEqual(requests[1], {revision: 0, prompt: 'Only {{skill}}'});
    await page.locator('[data-template-id="workflow"]').click();
    await dialog.locator('#template-prompt').fill('My retained draft');
    await page.route('**/api/templates/workflow', route => route.request().method() === 'PUT'
      ? route.fulfill({status: 409, contentType: 'application/json', body: JSON.stringify({error: 'Template changed'})}) : route.fallback());
    await dialog.locator('[data-save]').click();
    await page.waitForFunction(() => document.querySelector('[data-automation-error]')?.textContent.length > 0);
    assert.equal(await dialog.locator('#template-prompt').inputValue(), 'My retained draft');
    assert.deepEqual(app.pageErrors, []);
  } finally { await app.close(); }
});
