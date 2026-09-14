const assert = require('node:assert/strict');
const test = require('node:test');
const {openApp, apiFixture, SKIP} = require('./support/web_app');

test('Quick Actions controls pause and start independently, refresh availability, and link to configuration', {skip: SKIP}, async () => {
  let paused = false, running = true;
  const writes = [];
  const app = await openApp({fixture(path, request) {
    if (path === '/api/workflow/pause') { writes.push({path, body: request.postDataJSON()}); paused = request.postDataJSON().paused; return {ok:true}; }
    if (path.startsWith('/api/processes/background-workers/workflow/')) { writes.push({path}); running = path.endsWith('/start'); return {ok:true}; }
    if (path === '/api/processes') return {paused, runner_reachable:true, workflow_health:{healthy:true,state:paused?'paused':'idle'}, background_workers:[{worker_kind:'workflow',management_actions:[running?'stop_background_worker':'start_background_worker']}]};
    return apiFixture(path);
  }});
  try {
    const {page} = app;
    await page.goto(app.origin);
    await page.locator('#nav-create-menu > summary').click();
    const actions = page.locator('#quick-workflow-actions');
    await actions.getByRole('button', {name:'Pause workflow',exact:true}).click();
    assert.equal(writes.length,0);
    await page.getByTestId('modal-ok').click();
    await page.locator('#nav-create-menu > summary').click();
    await actions.getByRole('button', {name:'Unpause workflow',exact:true}).click();
    await actions.getByRole('button', {name:'Pause workflow',exact:true}).waitFor();
    assert.deepEqual(writes.map(write=>write.body),[{paused:true},{paused:false}]);
    await actions.getByRole('button', {name:'Stop workflow worker',exact:true}).click();
    await actions.getByRole('button', {name:'Start workflow worker',exact:true}).click();
    await actions.getByRole('button', {name:'Stop workflow worker',exact:true}).waitFor();
    assert.equal(writes[2].path,'/api/processes/background-workers/workflow/stop');
    assert.equal(writes[3].path,'/api/processes/background-workers/workflow/start');
    assert.equal(await actions.getByRole('link',{name:'Configure workflow'}).getAttribute('href'),'#/settings/workflow');
    assert.equal(await actions.innerText(),'');
    assert.equal(await page.locator('#nav-create-menu #btn-refine-issue-menu').count(),0);
    const bounds=await page.locator('.quick-status-row').first().evaluate(el=>({label:el.querySelector('a').getBoundingClientRect().bottom,actions:el.querySelector('.quick-status-actions').getBoundingClientRect().top}));
    assert.ok(bounds.actions >= bounds.label);
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});

test('Target application actions honor configuration, confirm start and stop, and refresh their icons', {skip: SKIP}, async () => {
  let state = 'stopped', configured = true;
  const writes = [];
  const app = await openApp({fixture(path, request) {
    if (path === '/api/target-app/status') return {state,has_start_action:configured,has_stop_action:configured};
    if (['/api/target-app/start','/api/target-app/stop'].includes(path)) {writes.push(path);state=path.endsWith('/start')?'running':'stopped';return {ok:true,state};}
    return apiFixture(path);
  }});
  try {
    const {page}=app;
    await page.goto(app.origin);
    await page.locator('#nav-create-menu > summary').click();
    const actions=page.locator('#quick-target-actions');
    await actions.getByRole('button',{name:'Start target application',exact:true}).click();
    assert.equal(writes.length,0);
    await page.getByTestId('modal-ok').click();
    await page.locator('#nav-create-menu > summary').click();
    await actions.getByRole('button',{name:'Stop target application',exact:true}).click();
    await page.getByTestId('modal-ok').click();
    await page.locator('#nav-create-menu > summary').click();
    await actions.getByRole('button',{name:'Start target application',exact:true}).waitFor();
    assert.deepEqual(writes,['/api/target-app/start','/api/target-app/stop']);
    await page.waitForFunction(()=>!document.querySelector("#quick-target-actions button").disabled);
    configured=false;
    await page.evaluate(()=>{ state.screenDataCache.clear(); return refreshTargetAppToggle(); });
    assert.equal(await actions.getByRole('button').isDisabled(),true);
    assert.equal(await actions.getByRole('link',{name:'Configure target application'}).getAttribute('href'),'#/settings/target-app');
    assert.equal(await actions.innerText(),'');
    await page.setViewportSize({width:390,height:844});
    assert.equal(await page.locator('.nav-context-panel').evaluate(el=>el.scrollWidth>el.clientWidth),false);
    assert.deepEqual(app.pageErrors,[]);
  } finally {await app.close();}
});

test('Custom topbar pickers share split borders, support keyboard selection, and retain Reporter after cancelled creation', {skip:SKIP}, async()=>{
  const app=await openApp({fixture(path){
    if(path==='/api/reporters')return {reporters:[{name:'Reporter'},{name:'Reviewer'}]};
    return apiFixture(path);
  }});
  try{
    const {page}=app;
    await page.goto(app.origin);
    const picker=page.locator('[data-topbar-picker="reporter"]');
    await page.waitForFunction(()=>document.querySelector('#global-reporter').value==='Reporter');
    assert.equal(await page.locator('#global-reporter').isVisible(),false);
    for (const mode of ['light','dark']) {
      await page.evaluate(mode=>document.documentElement.dataset.theme=mode,mode);
      const colors=await picker.locator('[role="option"][aria-selected="false"]').first().evaluate(el=>({text:getComputedStyle(el).color,expected:getComputedStyle(document.documentElement).getPropertyValue('--color-text').trim()}));
      assert.equal(colors.text, await page.evaluate(color=>{const el=document.createElement('span');el.style.color=color;document.body.append(el);const value=getComputedStyle(el).color;el.remove();return value;},colors.expected));
    }
    await page.evaluate(()=>document.documentElement.dataset.theme='light');
    await picker.locator('summary').press('ArrowDown');
    await page.keyboard.press('ArrowDown');
    await page.keyboard.press('Enter');
    assert.equal(await picker.locator('[data-picker-value]').textContent(),'Reviewer');
    assert.equal(await page.evaluate(()=>localStorage.getItem('refine_last_reporter')),'Reviewer');
    await picker.locator('summary').click();
    await picker.getByRole('option',{name:'+ Add new reporter…',exact:true}).click();
    await page.getByTestId('modal-cancel').click();
    await page.waitForFunction(()=>document.querySelector('[data-topbar-picker="reporter"] [data-picker-value]').textContent==='Reviewer');
    const heights=await page.locator('.nav-picker-summary, .nav-create-menu > summary, #btn-new-goal').evaluateAll(els=>els.map(el=>el.getBoundingClientRect().height));
    assert.deepEqual(heights,[34,34,34,34]);
    assert.equal(await picker.locator('.nav-context-more').evaluate(el=>getComputedStyle(el).borderLeftWidth),'1px');
    await picker.locator('summary').click();
    await page.locator('[data-topbar-picker="node"] > summary').click();
    assert.equal(await picker.getAttribute('open'),null);
    assert.deepEqual(app.pageErrors,[]);
  }finally{await app.close();}
});
