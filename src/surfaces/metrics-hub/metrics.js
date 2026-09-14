'use strict';
let snapshot = null;
let selected = 'month';
const $ = selector => document.querySelector(selector);
const escapeHtml = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const number = (value,digits=0) => value == null ? '—' : Number(value).toLocaleString(undefined,{maximumFractionDigits:digits});
const percent = value => value == null ? '—' : `${number(value*100,1)}%`;
const date = value => new Date(value).toLocaleString(undefined,{timeZone:'UTC',month:'short',day:'numeric',...(selected==='day'?{hour:'2-digit',minute:'2-digit'}:{})});
const duration = hours => hours == null ? '—' : hours < 1 ? `${number(hours*60)} min` : hours < 48 ? `${number(hours,1)} hours` : `${number(hours/24,1)} days`;
const durations = {day:'24 hours',week:'7 days',month:'30 days',quarter:'90 days',year:'365 days'};
function bars(entries) {
  const max = Math.max(1,...entries.map(([,value])=>value));
  return entries.length ? entries.map(([label,value])=>`<div class="bar-row"><span>${escapeHtml(label)}</span><div class="bar-track"><span class="bar-fill" style="width:${value/max*100}%"></span></div><span class="bar-value">${number(value)}</span></div>`).join('') : '<p>No recorded activity in this window.</p>';
}
function delta(value,previous,digits=0) { if(value==null || previous==null) return 'No prior comparison available';const diff=value-previous;return `${diff>0?'+':''}${number(diff,digits)} vs. previous ${durations[selected]}`; }
function render() {
  const ready = !!snapshot?.generated_at;
  $('[data-empty]').hidden = ready; $('[data-report]').hidden = !ready; $('[data-download]').disabled = !ready;
  if(!ready) { $('[data-freshness]').textContent='No saved snapshot yet'; return; }
  $('[data-project]').textContent=snapshot.target_app || 'Target app';
  const age=(Date.now()-Date.parse(snapshot.generated_at))/3600000;
  $('[data-freshness]').textContent=`Updated ${new Date(snapshot.generated_at).toLocaleString()}${age>24?' · More than 24 hours old':''}`;
  $('[data-freshness]').classList.toggle('stale',age>24);
  const p=snapshot.periods[selected], prev=p.previous;
  $('[data-range]').textContent=`Last ${durations[selected]} · ${new Date(p.start).toLocaleString(undefined,{timeZone:'UTC'})} – ${new Date(p.end).toLocaleString(undefined,{timeZone:'UTC'})} UTC`;
  const kpis=[['Goals created',number(p.goals_created),delta(p.goals_created,prev.goals_created)],['Goals delivered',number(p.goals_delivered),delta(p.goals_delivered,prev.goals_delivered)],['Rounds per Goal',number(p.rounds_per_goal,2),delta(p.rounds_per_goal,prev.rounds_per_goal,2)],['Median delivery time',duration(p.median_delivery_hours),`${number(p.delivery_time_samples)} recorded deliveries with valid timing`]];
  $('[data-kpis]').innerHTML=kpis.map(([label,value,note])=>`<div class="metric"><p class="metric-label">${label}</p><p class="metric-value">${value}</p><p class="metric-note">${note}</p></div>`).join('');
  const buckets=p.trend, max=Math.max(1,...buckets.flatMap(b=>[b.created,b.delivered]));const width=1000,height=220,left=35,plot=950,step=plot/buckets.length;
  const marks=buckets.map((b,i)=>{const x=left+i*step;return `<g><title>${escapeHtml(b.start)} – ${escapeHtml(b.end)}: ${b.created} created, ${b.delivered} delivered</title><rect class="created" x="${x}" y="${height-b.created/max*175}" width="${step*.38}" height="${b.created/max*175}" rx="2"/><rect class="delivered" x="${x+step*.4}" y="${height-b.delivered/max*175}" width="${step*.38}" height="${b.delivered/max*175}" rx="2"/></g>`;}).join('');
  $('[data-trend]').innerHTML=`<svg viewBox="0 0 ${width} 260" role="img" aria-label="${p.goals_created} Goals created and ${p.goals_delivered} delivered in the selected period"><line class="chart-line" x1="${left}" y1="${height}" x2="990" y2="${height}"/><line class="chart-line" x1="${left}" y1="45" x2="990" y2="45"/><text class="chart-label" x="0" y="49">${max}</text><text class="chart-label" x="0" y="224">0</text>${marks}<text class="chart-label" x="${left}" y="249">${date(p.start)}</text><text class="chart-label" x="985" y="249" text-anchor="end">${date(p.end)} · UTC</text></svg>`;
  $('[data-trend-table]').innerHTML=`<table><thead><tr><th>Bucket start (UTC)</th><th>Created</th><th>Delivered</th></tr></thead><tbody>${buckets.map(b=>`<tr><td>${escapeHtml(b.start)}</td><td>${b.created}</td><td>${b.delivered}</td></tr>`).join('')}</tbody></table>`;
  $('[data-distribution]').innerHTML=bars(p.round_distribution.map((v,i)=>[["No rounds","1 round","2 rounds","3+ rounds"][i],v]));
  $('[data-iteration]').innerHTML=`<div><strong>${percent(p.multiple_round_share)}</strong><span>Goals with multiple rounds</span></div><div><strong>${percent(p.single_round_delivery_share)}</strong><span>Deliveries in one round</span></div><div><strong>${number(p.rounds_created)}</strong><span>Rounds started in window</span></div>`;
  const statusOrder=['backlog','todo','plan','implement','quality','governance','review','done','failed','cancelled'];
  $('[data-statuses]').innerHTML=bars(Object.entries(snapshot.current.statuses).sort((a,b)=>statusOrder.indexOf(a[0])-statusOrder.indexOf(b[0])));
  $('[data-aging]').innerHTML=`<div><strong>${number(snapshot.current.median_age_days,1)} days</strong><span>Median age of unfinished Goals</span></div><div><strong>${number(snapshot.current.quiet_seven_days)}</strong><span>Unfinished, no update for 7 days</span></div>`;
  $('[data-nodes]').innerHTML=bars(Object.entries(p.nodes).sort((a,b)=>b[1]-a[1]));
  $('[data-reporters]').innerHTML=bars(Object.entries(p.reporters).sort((a,b)=>b[1]-a[1]));
  const coverage=snapshot.coverage;
  $('[data-coverage]').textContent=`Coverage: ${number(snapshot.current.goals)} retained Goals. ${number(coverage.done_without_integration_time)} Done Goals lack a recorded final integration date; ${number(coverage.goals_without_created_time)} Goals and ${number(coverage.rounds_without_created_time)} rounds lack a valid creation date.`;
}
async function request(url,options) { const r=await fetch(url,options);if(!r.ok) {let message=`Unable to update statistics (${r.status})`;try{const error=await r.json();message=error.error?.message || error.error || message;}catch{}throw Error(typeof message==='string'?message:JSON.stringify(message));}return r.json(); }
function showError(error) { $('[data-error]').textContent=error.message; $('[data-error]').hidden=false; }
document.querySelectorAll('[data-period]').forEach(button=>button.onclick=()=>{selected=button.dataset.period;document.querySelectorAll('[data-period]').forEach(b=>b.setAttribute('aria-pressed',String(b===button)));render();});
$('[data-refresh]').onclick=async()=>{const button=$('[data-refresh]');button.disabled=true;button.textContent='Refreshing…';$('[data-error]').hidden=true;try{snapshot=await request('/api/hub/metrics/refresh',{method:'POST',headers:{'Content-Type':'application/json'},body:'{}'});render();}catch(error){showError(error);}finally{button.disabled=false;button.textContent='Refresh statistics';}};
$('[data-download]').onclick=()=>{const url=URL.createObjectURL(new Blob([JSON.stringify(snapshot,null,2)],{type:'application/json'}));const link=document.createElement('a');link.href=url;link.download='refine-metrics.json';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);};
request('data.json').then(data=>{snapshot=data;render();}).catch(error=>{$('[data-freshness]').textContent='Saved statistics unavailable';showError(error);});
