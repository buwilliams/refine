const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const {openApp,SKIP}=require('./support/web_app');
const assets=path.join(__dirname,'../src/surfaces/metrics-hub');
function report() {
  const result={generated_at:new Date().toISOString(),target_app:'Example app',current:{goals:20,statuses:{done:10,plan:6,failed:4},median_age_days:3,quiet_seven_days:2},coverage:{done_without_integration_time:1,goals_without_created_time:0,rounds_without_created_time:0},periods:{}};
  for(const [i,key] of ['day','week','month','quarter','year'].entries()) result.periods[key]={start:'2026-09-01T00:00:00Z',end:'2026-09-14T00:00:00Z',goals_created:i+3,goals_delivered:2,rounds_per_goal:1.5,median_delivery_hours:12,delivery_time_samples:2,round_distribution:[0,2,1,0],multiple_round_share:.33,single_round_delivery_share:.5,rounds_created:5,nodes:{'Node A':5},reporters:{'<b>Alice</b>':3},previous:{goals_created:1,goals_delivered:1,rounds_per_goal:1},trend:[{start:'2026-09-01T00:00:00Z',end:'2026-09-14T00:00:00Z',created:i+3,delivered:2}]};
  return result;
}
test('Metrics Hub refreshes, switches horizons, and retains a usable snapshot after refresh failure', {skip:SKIP}, async()=>{
  const app=await openApp();const {page}=app;let refreshes=0,fail=false;
  try{
    await page.route('**/hub/sites/refine/hub.css',route=>route.fulfill({contentType:'text/css',body:fs.readFileSync(path.join(__dirname,'../src/surfaces/refine-hub/hub.css'),'utf8')}));
    await page.route('**/hub/sites/refine-metrics/**',route=>{
      const name=new URL(route.request().url()).pathname.split('/').pop() || 'index.html';
      if(name==='data.json')return route.fulfill({contentType:'application/json',body:JSON.stringify({generated_at:null})});
      return route.fulfill({contentType:name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html',body:fs.readFileSync(path.join(assets,name),'utf8')});
    });
    await page.route('**/api/hub/metrics/refresh',route=>{refreshes++;assert.equal(route.request().method(),'POST');return route.fulfill({status:fail?500:200,contentType:'application/json',body:JSON.stringify(fail?{error:'Refresh failed; previous snapshot retained'}:report())});});
    await page.goto(`${app.origin}/hub/sites/refine-metrics/`);
    await page.locator('[data-empty]').waitFor();assert.equal(refreshes,0);
    await page.locator('[data-refresh]').click();await page.locator('[data-report]').waitFor();
    assert.equal(await page.locator('.metric-value').first().textContent(),'5');
    await page.locator('[data-period="year"]').click();assert.equal(await page.locator('.metric-value').first().textContent(),'7');
    assert.equal(await page.locator('[data-period="year"]').getAttribute('aria-pressed'),'true');
    assert.equal(await page.locator('[data-reporters] b').count(),0);
    assert.match(await page.locator('[data-reporters]').textContent(),/<b>Alice<\/b>/);
    await page.locator('.chart-data summary').click();assert.equal(await page.locator('[data-trend-table] tbody tr').count(),1);
    for(const theme of ['light','dark']){await page.emulateMedia({colorScheme:theme});await page.setViewportSize({width:1440,height:1050});await page.evaluate(()=>scrollTo(0,0));await page.screenshot({path:`/tmp/metrics-hub-${theme}.png`,fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);}
    await page.setViewportSize({width:390,height:844});await page.screenshot({path:'/tmp/metrics-hub-mobile.png',fullPage:true});assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false);
    fail=true;await page.locator('[data-refresh]').click();await page.locator('[data-error]').waitFor();assert.equal(await page.locator('.metric-value').first().textContent(),'7');assert.equal(await page.locator('[data-refresh]').isEnabled(),true);assert.equal(refreshes,2);assert.deepEqual(app.pageErrors,[]);
  }finally{await app.close();}
});
