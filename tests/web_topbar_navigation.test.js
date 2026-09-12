const assert = require('node:assert/strict');
const test = require('node:test');
const { openApp, apiFixture, SKIP } = require('./support/web_app');

function navigationFixture() {
  const data = { skills: [{id: 'run', name: 'Run report', enabled: true}], sites: [{item: {id: 'draft', name: 'Draft'}}] };
  const fixture = (path, request) => {
    if (path === '/api/skills') return {revision: '1', items: data.skills, manual_skill_ids: ['run']};
    if (path === '/api/skills/catalog') return {sources: ['custom']};
    if (path === '/api/skills/run') return {item: data.skills[0]};
    if (path === '/api/skills/run/inputs') return {parameters: [{name: 'count', kind: 'number', required: true}]};
    if (path === '/api/hub/sites') return {sites: data.sites};
    return apiFixture(path, request);
  };
  return {data, fixture};
}

test('topbar order, exclusive placement, keyboard dismissal, creation and management', {skip: SKIP}, async () => {
  const {fixture} = navigationFixture();
  const app = await openApp({fixture}); const {page} = app;
  try {
    await page.goto(`${app.origin}/#/goals`);
    assert.deepEqual(await page.locator('.topbar-actions > details > summary .nav-context-main span, .topbar-actions > .nav-create-group > a').allTextContents(), ['Skills', 'Sites', 'Settings', '+ New Goal']);
    assert.equal(await page.locator('#nav-context-menu #nav-manual-skills, #nav-context-menu #nav-knowledge-hub').count(), 0);
    await page.getByTestId('skills-menu-toggle').focus(); await page.keyboard.press('Enter');
    await page.locator('[data-manual-skill]').waitFor({state:'visible'});
    assert.deepEqual(await page.locator('#nav-manual-skills button, #nav-manual-skills a').allTextContents(), ['Run report', 'New Skill', 'Manage Skills']);
    assert.equal(await page.locator('#nav-manual-skills hr + [role=group]').count(), 1);
    await page.keyboard.press('Tab');
    assert.equal(await page.locator('[data-manual-skill]').evaluate(el => el === document.activeElement), true);
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('.topbar-actions details[open]').count(), 0);
    assert.equal(await page.getByTestId('skills-menu-toggle').evaluate(el => el === document.activeElement), true);
    await page.getByTestId('skills-menu-toggle').click();
    await page.getByTestId('sites-menu-toggle').click();
    assert.equal(await page.locator('.topbar-actions details[open]').getAttribute('id'), 'nav-sites-menu');
    assert.deepEqual(await page.locator('#nav-knowledge-hub button, #nav-knowledge-hub a').allTextContents(), ['Draft', 'New Site', 'Manage Sites']);
    assert.equal(await page.locator('#nav-knowledge-hub hr + [role=group]').count(), 1);
    await page.locator('[data-hub-add]').click();
    await page.locator('#hub-site-name').waitFor({state:'visible'});
    assert.equal(await page.locator('.topbar-actions details[open]').count(), 0);
    await page.locator('[data-close]').click();
    await page.getByTestId('skills-menu-toggle').click(); await page.locator('[data-add-skill]').click();
    await page.getByTestId('automation-modal').waitFor({state:'visible'});
    assert.match(await page.locator('.modal-title').innerText(), /New Skill/);
    await page.locator('[data-close]').click();
    for (const [menu, label, route] of [['skills','Manage Skills','skills'], ['sites','Manage Sites','knowledge-hub']]) {
      await page.getByTestId(`${menu}-menu-toggle`).click();
      await page.getByRole('link', {name:label, exact:true}).click();
      await page.waitForURL(`**/#/settings/${route}`);
      assert.equal(await page.locator('.topbar-actions details[open]').count(), 0);
    }
    await page.getByTestId('sites-menu-toggle').click(); await page.locator('.brand').click();
    assert.equal(await page.locator('.topbar-actions details[open]').count(), 0);
    assert.deepEqual(app.pageErrors, []);
  } finally {await app.close();}
});

test('empty and failed lists retain actions; refresh and stale context preserve current entries', {skip: SKIP}, async () => {
  const {data, fixture} = navigationFixture(); data.skills=[]; data.sites=[];
  const app=await openApp({fixture}); const {page}=app;
  try {
    await page.goto(`${app.origin}/#/goals`);
    for (const menu of ['skills','sites']) {
      await page.getByTestId(`${menu}-menu-toggle`).click();
      assert.equal(await page.locator(`#nav-${menu}-menu [role=group] > *`).count(), 2);
    }
    await page.route('**/api/skills?*', route=>route.fulfill({status:503,body:'unavailable'}));
    await page.route('**/api/hub/sites', route=>route.fulfill({status:503,body:'unavailable'}));
    await page.evaluate(async()=>{await refreshManualSkills();await refreshKnowledgeHub();});
    assert.equal(await page.locator('.topbar-actions [role=status]').count(),2);
    assert.equal(await page.locator('.topbar-actions [role=group] > *').count(),4);
    await page.unroute('**/api/skills?*'); await page.unroute('**/api/hub/sites');
    data.skills=[{id:'run', name:'Changed Skill',enabled:true},{id:'disabled',name:'Hidden Skill',enabled:false}];
    data.sites=[{item:{id:'updated',name:'Changed Site'}}];
    await page.evaluate(()=>{initSSE();sseSource.dispatchEvent(new MessageEvent('api_mutation',{data:'/api/skills/run'}));sseSource.dispatchEvent(new MessageEvent('api_mutation',{data:'/api/hub/sites/updated'}));});
    await page.locator('[data-manual-skill]').filter({hasText:'Changed Skill'}).waitFor({state:'attached'});
    await page.locator('[data-hub-open]').filter({hasText:'Changed Site'}).waitFor({state:'attached'});
    assert.equal(await page.locator('[data-manual-skill]').count(),1);
    data.skills[0].name='New node Skill'; data.sites[0].item.name='New node Site';
    await page.evaluate(()=>applyAuthoritativeNodeContext({...state.project, attached:true, active_node_id:'second'}, {nodes:[{id:'second'}]}, {changed:true}));
    await page.locator('[data-manual-skill]').filter({hasText:'New node Skill'}).waitFor({state:'attached'});
    await page.locator('[data-hub-open]').filter({hasText:'New node Site'}).waitFor({state:'attached'});
    // Resolve an old request after a newer refresh, then reject an old-context request.
    await page.evaluate(async()=>{
      const original=api;
      for (const [matches, refresh, stale] of [
        [path=>path==='/api/hub/sites',refreshKnowledgeHub,{sites:[{item:{id:'stale',name:'Stale Site'}}]}],
        [path=>path.startsWith('/api/skills?'),refreshManualSkills,{items:[{id:'stale',name:'Stale Skill',enabled:true}],manual_skill_ids:['stale']}],
      ]) {
        let resolveOld;
        api=(method,path,...rest)=>matches(path)?new Promise(resolve=>{resolveOld=resolve;}):original(method,path,...rest);
        const old=refresh(); api=original; await refresh();
        resolveOld(stale); await old;
        let rejectOld;
        api=(method,path,...rest)=>matches(path)?new Promise((_,reject)=>{rejectOld=reject;}):original(method,path,...rest);
        const failed=refresh(); nodeContextGeneration++; api=original;
        rejectOld(new Error('old node')); await failed;
      }
    });
    assert.equal(await page.locator('[data-hub-open]').textContent(),'New node Site');
    assert.equal(await page.locator('#nav-knowledge-hub [role=status]').count(),0);
    assert.equal(await page.locator('[data-manual-skill]').textContent(),'New node Skill');
    assert.equal(await page.locator('#nav-manual-skills [role=status]').count(),0);
  } finally {await app.close();}
});

test('Skill typed preflight launches an agent tab and draft Site opens its preview', {skip:SKIP}, async()=>{
  const {fixture}=navigationFixture(); const app=await openApp({fixture}); const {page}=app;
  try {
    await page.goto(`${app.origin}/#/goals`);
    await page.evaluate(()=>{window.launches=[];createToolbarTab=async(...args)=>window.launches.push(args);});
    await page.getByTestId('skills-menu-toggle').click(); await page.locator('[data-manual-skill]').click();
    await page.getByLabel('count *').fill('3'); await page.getByRole('button',{name:'Run Skill',exact:true}).click();
    assert.deepEqual(await page.evaluate(()=>window.launches),[['skill',{label:'Run report',skillLaunch:{id:'run',parameters:{count:3}}}]]);
    await page.context().route('**/hub/preview/draft/',r=>r.fulfill({body:'Draft preview'}));
    await page.getByTestId('sites-menu-toggle').click();
    const popupPromise=page.waitForEvent('popup'); await page.locator('[data-hub-open]').click();
    const popup=await popupPromise; await popup.waitForURL(`${app.origin}/hub/preview/draft/`);await popup.close();
    assert.equal(await page.locator('.topbar-actions details[open]').count(),0);
  }finally{await app.close();}
});

for (const width of [1440, 800, 375]) for (const theme of ['light','dark']) {
  test(`long menu remains usable at ${width}px in ${theme}`,{skip:SKIP},async()=>{
    const {data,fixture}=navigationFixture();data.sites=Array.from({length:60},(_,i)=>({item:{id:`site-${i}`,name:`Site ${i} with a long descriptive name`}}));
    data.skills=Array.from({length:60},(_,i)=>({id:`skill-${i}`,name:`Skill ${i} with a long descriptive name`,enabled:true}));
    const longListFixture=(path,request)=>path==='/api/skills'?{items:data.skills,manual_skill_ids:data.skills.map(skill=>skill.id)}:fixture(path,request);
    const app=await openApp({fixture:longListFixture});const {page}=app;
    try {
      await page.setViewportSize({width,height:700});await page.goto(`${app.origin}/#/goals`);
      await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
      for(const menu of ['skills','sites','context']){
        await page.getByTestId(`${menu}-menu-toggle`).click();
        const panel=page.locator(`#nav-${menu}-menu > .nav-menu-panel`);
        const box=await panel.boundingBox();assert.ok(box.x>=0 && box.x+box.width<=width,JSON.stringify(box));
        assert.ok(box.y+box.height<=700,JSON.stringify(box));
        if(menu==='sites'||menu==='skills'){
          const action=page.getByRole('link',{name:menu==='sites'?'Manage Sites':'Manage Skills',exact:true});
          await action.scrollIntoViewIfNeeded();
          assert.ok(await action.isVisible());
          assert.ok(await panel.evaluate(el=>el.scrollHeight>el.clientHeight));
        }
      }
    }finally{await app.close();}
  });
}
