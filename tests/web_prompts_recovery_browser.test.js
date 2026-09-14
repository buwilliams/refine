const assert=require('node:assert/strict');
const test=require('node:test');
const {openApp,apiFixture,SKIP}=require('./support/web_app');

test('Agent-type reset previews all its launch paths and single-template reset stays scoped', {skip:SKIP},async()=>{
  const defs={'terminal-session':'{{instructions}}','chat-session':'{{instructions}}','planning-agent':'{{templates.purpose}}',purpose:'Purpose default',agent:'General default','goal-agent':'Goal default','manual-skill':'{{skill}}','goal-agents-session':'{{goal_prompt}}',workflow:'{{templates.supervised-skill}}','supervised-skill':'{{skill}}'};
  const rows=Object.entries(defs).map(([id,prompt])=>({name:id,item:{id,prompt:id==='planning-agent'?'Edited instructions':prompt,revision:1},default_prompt:prompt,customized:id==='planning-agent',usage:{kind:'template',group:'workflow'}}));
  const writes=[];
  const app=await openApp({fixture(path,request){
    if(path==='/api/templates')return {items:rows};
    if(path==='/api/templates/reset'){
      const body=request.postDataJSON();writes.push(body);
      for(const row of rows.filter(row=>row.item.id in body.revisions)){row.item.prompt=row.default_prompt;row.item.revision++;row.customized=false;}
      return {reset:Object.keys(body.revisions)};
    }
    return apiFixture(path);
  }});
  try{
    const {page}=app;await page.goto(`${app.origin}/#/settings/templates`);
    await page.locator('.resource-map-overview > summary').click();
    await page.locator('[data-resource-map-type]').selectOption('planning');
    await page.locator('[data-reset-agent-type]').click();
    const modal=page.getByTestId('automation-modal');await modal.waitFor();
    assert.match(await modal.innerText(),/terminal-session/);assert.match(await modal.innerText(),/chat-session/);assert.match(await modal.innerText(),/purpose/);
    assert.match(await modal.innerText(),/Shared templates also affect/);
    await modal.locator('[data-save]').click();await modal.waitFor({state:'detached'});
    assert.deepEqual(Object.keys(writes[0].revisions).sort(),['chat-session','planning-agent','purpose','terminal-session']);
    rows.find(row=>row.item.id==='purpose').item.prompt='Custom project purpose';
    await page.locator('[data-resource-reset="purpose"]').click();await modal.waitFor();
    await modal.locator('[data-save]').click();await modal.waitFor({state:'detached'});
    assert.deepEqual(Object.keys(writes[1].revisions),['purpose']);
    assert.deepEqual(app.pageErrors,[]);
  }finally{await app.close();}
});

test('Goal Prompts loads actual retained text on demand and identifies missing history', {skip:SKIP},async()=>{
  let reads=0;
  const prompt='<script>literal</script>\n{{literal}}\n'+'Complete prompt 世界\n'.repeat(1000);
  const app=await openApp({fixture(path){
    if(path==='/api/goals/GOAL1/prompts'){reads++;return {items:[{id:'actual',round_idx:0,step:'plan',prompt,bytes:Buffer.byteLength(prompt),started_at:'2026-09-14T00:00:00Z'},{id:'old',round_idx:0,prompt:null},{id:'another-round',round_idx:1,prompt:'Other round'}]};}
    return apiFixture(path);
  }});
  try{
    const {page}=app;await page.goto(`${app.origin}/#/goals/GOAL1`);
    await page.locator('[data-round-tab="prompts"]').click();
    await page.locator('[data-goal-prompt-select]').waitFor();
    assert.equal(reads,1);assert.equal(await page.locator('[data-goal-prompt-select] option').count(),2);
    assert.equal(await page.locator('[data-goal-prompt-text]').textContent(),prompt);
    assert.equal(await page.locator('[data-goal-prompt-text] script').count(),0);
    await page.locator('[data-goal-prompt-select]').selectOption('1');
    assert.match(await page.locator('[data-goal-prompt-empty]').innerText(),/not retained/);
    assert.equal(await page.locator('[data-goal-prompt-copy]').isDisabled(),true);
    assert.deepEqual(app.pageErrors,[]);
  }finally{await app.close();}
});
