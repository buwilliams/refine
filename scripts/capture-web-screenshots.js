// Capture the real Web UI with a deterministic example workspace, without
// contacting a live daemon or launching processes. Run with Node + Playwright.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {createHash} = require('node:crypto');
const {openApp, apiFixture, GOAL, SKIP} = require('../tests/support/web_app');

const reporter = 'Alex';
const nodes = [
  {id:'local',display_name:'Workstation',enabled:true,state_sync_health:{status:'healthy'}},
  {id:'build',display_name:'Build server',ssh_host:'build.example.com',enabled:true,health:{status:'ready'},state_sync_health:{status:'healthy'}},
  {id:'review',display_name:'Review server',ssh_host:'review.example.com',enabled:true,health:{status:'ready'},state_sync_health:{status:'healthy'}},
];
const names = ['Consolidate workspace navigation','Add accessible keyboard shortcuts','Improve search result previews','Retain filters across navigation','Show agent activity in the rail','Document workspace settings','Add project onboarding','Improve review summaries'];
const statuses = ['review','review','implement','quality','todo','done','done','backlog'];
const goals = names.map((name,i)=>({...GOAL,id:`EXAMPLE${String(i+1).padStart(3,'0')}`,name,status:statuses[i],reporter,assignee:reporter,node_id:i===2?'build':'local',node_display_name:i===2?'Build server':'Workstation',created:'2026-09-14T12:00:00Z',updated:'2026-09-15T12:00:00Z',rounds:[{prompt:name,reporter,created:'2026-09-14T12:00:00Z',logs:[]}]}));
const entries = [
  {name:'src',path:'src',type:'directory'},
  {name:'tests',path:'tests',type:'directory'},
  {name:'README.md',path:'README.md',type:'file'},
  {name:'Cargo.toml',path:'Cargo.toml',type:'file'},
];
function fixture(url,request) {
  const params = new URL(request.url()).searchParams;
  if(url==='/api/project/status')return {attached:true,target_root:'/workspace/example-app',nodes,active_node_id:'local',registry_enabled:true,apps:[]};
  if(url==='/api/nodes')return {nodes,active_node_id:'local',counts:{local:{done:12,review:2},build:{implement:2},review:{quality:1}}};
  if(url==='/api/skills')return {revision:1,items:[
    {id:'review-release',name:'Review release',enabled:true,scope:{}},
    {id:'refresh-reports',name:'Refresh reports',enabled:true,scope:{}},
  ],manual_skill_ids:['review-release','refresh-reports']};
  if(url==='/api/hub/sites')return {sites:[
    {revision:'example-1',item:{id:'metrics',name:'Metrics Hub',publication:{}}},
    {revision:'example-2',item:{id:'project-guide',name:'Project guide',publication:{}}},
  ]};
  if(url==='/api/todos')return {reporter,lists:[
    {id:'release',name:'Release checklist',items:[
      {id:'navigation',text:'Review navigation on desktop and mobile',done:false},
      {id:'screenshots',text:'Refresh website screenshots and product documentation',done:false},
      {id:'notes',text:'Check release notes and review the candidate',done:false},
      {id:'keyboard',text:'Verify keyboard navigation and Search',done:true},
    ]},
    {id:'ideas',name:'Ideas to explore',items:[{id:'onboarding',text:'Simplify project onboarding',done:false}]},
  ]};
  if(url==='/api/reporters')return {reporters:[{name:reporter},{name:'Sam'}]};
  if(url==='/api/dashboard')return {counts:{backlog:4,todo:6,plan:1,implement:2,quality:1,governance:1,review:2,done:24,failed:0,cancelled:1},needs_attention:[],contributor_rankings:[{rank:1,contributor:'Alex',total:24,done:18},{rank:2,contributor:'Sam',total:12,done:6}]};
  if(url==='/api/goals') {
    const selected=params.get('status')?goals.filter(g=>g.status===params.get('status')):goals;
    return {goals:selected,facets:{status_counts:goals.reduce((counts,goal)=>({...counts,[goal.status]:(counts[goal.status]||0)+1}),{})},page:{page:1,total:selected.length}};
  }
  if(url.startsWith('/api/goals/'))return {goal:goals.find(g=>url.endsWith(g.id))||goals[0]};
  if(url==='/api/processes')return {runner_reachable:true,workflow_health:{healthy:true,state:'running'},paused:false,target_app:{state:"running",has_start_action:true,has_stop_action:true,has_build_action:true,has_status_checks:true},processes:[
    {id:'daemon',kind:'daemon',status:'running',pid:4100,label:'Refine daemon',memory_used_bytes:157286400,processor_used_percent:0.4},
    {id:'agent-1',kind:'agent',provider:'codex',status:'running',pid:4201,memory_used_bytes:268435456,processor_used_percent:12.8,label:'Search result previews',goal_id:goals[2].id},
    {id:'agent-2',kind:'agent',provider:'claude',status:'running',pid:4202,memory_used_bytes:201326592,processor_used_percent:8.2,label:'Navigation verification',goal_id:goals[3].id},
  ],background_workers:[{worker_kind:'workflow',status:'running',pid:4101,management_actions:['stop_background_worker']},{worker_kind:'state_sync',status:'running',pid:4102,management_actions:['stop_background_worker']}]};
  if(url==='/api/files/tree')return {path:params.get('path')||'',entries,has_more:false};
  if(url==='/api/files/read')return {path:'README.md',previewable:true,kind:'text',content:'# Example App\n\nA shared workspace for planning, delivery, and review.\n\n## Development\n\nRun the application locally and open Refine to manage work.\n\n## Workflow\n\n1. Describe the desired outcome as a Goal.\n2. Review the implementation and its evidence.\n3. Accept the result or add another round of feedback.\n',has_more:false};
  if(url==='/api/target-app/status')return {state:'running',has_start_action:true,has_stop_action:true,has_build_action:true,has_status_checks:true};
  return apiFixture(url);
}
(async()=>{
  if(SKIP)throw Error(SKIP);
  const app=await openApp({fixture,selectedReporter:reporter});
  try {
    const {page}=app;
    // Keep the rail fully visible at the website screenshot height.
    await page.addInitScript(() => {
      localStorage.setItem("rail-skills-section", "false");
      localStorage.setItem("rail-hubs-section", "false");
    });
    await page.setViewportSize({width:1549,height:1084});
    await page.emulateMedia({colorScheme:'light',reducedMotion:'reduce'});
    const assets=path.resolve(__dirname,'../src/surfaces/website/assets');
    const release=path.resolve(__dirname,'../src/surfaces/refine-hub/releases/images/4.3.2');
    fs.mkdirSync(release,{recursive:true});
    async function capture(file) {
      await page.evaluate(()=>document.fonts.ready);
      await page.waitForTimeout(200);
      assert.deepEqual(app.pageErrors,[]);
      await page.screenshot({path:path.join(assets,file)});
      console.log(file);
    }
    await page.goto(app.origin, {waitUntil:'networkidle'});
    await page.locator('[data-testid="dashboard-review-count"]').waitFor();
    await page.waitForFunction(()=>document.querySelector('[data-testid="dashboard-review-count"]').textContent==='2');
    await capture('ss-1-dashboard.png');
    fs.copyFileSync(path.join(assets,'ss-1-dashboard.png'),path.join(release,'dashboard.png'));
    await page.locator('[data-testid="nav-goals"]').click();
    await page.getByText(names[0],{exact:true}).first().waitFor();
    await capture('ss-2-goals.png');
    await page.locator('[data-testid="toolbar-add"]').click();
    await page.locator('[data-add-toolbar-tab="files"]').click();
    await page.locator('[data-testid="toolbar-files-panel"]').waitFor();
    await page.evaluate(()=>loadFile('README.md'));
    await page.getByText('# Example App',{exact:false}).first().waitFor();
    await capture('ss-3-dev-tools.png');
    fs.copyFileSync(path.join(assets,'ss-3-dev-tools.png'),path.join(release,'files.png'));
    await page.goto(`${app.origin}/#/settings/application`);
    await page.locator('[data-testid="node-settings-table"]').waitFor();
    await page.locator('[data-testid="node-settings-table"]').scrollIntoViewIfNeeded();
    await capture('ss-4-nodes.png');
    await page.locator('[data-testid="nav-dashboard"]').click();
    await page.locator('#dash').waitFor();
    await page.keyboard.press('Control+k');
    await page.locator('#command-palette-input').fill('goal');
    await capture('ss-5-search.png');
    await page.keyboard.press('Escape');
    await page.locator('[data-testid="nav-control"]').click();
    await page.locator('[data-tab-pane="processes"].active').waitFor();
    await capture('ss-6-processes.png');
    fs.copyFileSync(path.join(assets,'ss-6-processes.png'),path.join(release,'control.png'));
    await page.locator('[data-testid="nav-dashboard"]').click();
    await page.locator('#dash').waitFor();
    await page.locator('#rail-main-section > summary').click();
    await page.locator('#rail-skills-section > summary').click();
    await page.locator('#rail-hubs-section > summary').click();
    await page.locator('[data-manual-skill="review-release"]').waitFor();
    await page.locator('[data-hub-open="metrics"]').waitFor();
    await page.locator('[data-testid="toolbar-add"]').click();
    await page.locator('[data-add-toolbar-tab="todo"]').waitFor();
    await capture('ss-7-tools.png');
    fs.copyFileSync(path.join(assets,'ss-7-tools.png'),path.join(release,'tools.png'));
    await page.locator('[data-add-toolbar-tab="todo"]').click();
    await page.locator('[data-testid="todo-list-title"]').waitFor();
    await page.getByText('Review navigation on desktop and mobile',{exact:true}).waitFor();
    await page.locator('#rail-skills-section > summary').click();
    await page.locator('#rail-hubs-section > summary').click();
    await page.locator('#rail-main-section > summary').click();
    await capture('ss-8-todo.png');
    fs.copyFileSync(path.join(assets,'ss-8-todo.png'),path.join(release,'todo.png'));

    // Content-based URL versions invalidate old screenshots after any recapture.
    const version = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex').slice(0,12);
    const website = path.join(assets,'../index.html');
    fs.writeFileSync(website,fs.readFileSync(website,'utf8').replace(
      /\/src\/surfaces\/website\/assets\/(ss-[\w-]+\.png)(?:\?v=[^"\s]+)?/g,
      (_,file) => `/src/surfaces/website/assets/${file}?v=${version(path.join(assets,file))}`,
    ));
    for (const relative of ['releases/4.3.2.md','product/navigation.md']) {
      const file = path.resolve(__dirname,'../src/surfaces/refine-hub',relative);
      fs.writeFileSync(file,fs.readFileSync(file,'utf8').replace(
        /(images\/4\.3\.2\/([\w-]+\.png))(?:\?v=[^)\s"]+)?/g,
        (_,url,name) => `${url}?v=${version(path.join(release,name))}`,
      ));
    }
  } finally {await app.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
