// Workflow decisions are shared capabilities, independent of Skill execution.
async function openWorkflowControl(goal) {
  const nodeGeneration = captureNodeContextGeneration();
  const data = await api("GET", `/api/workflow/goals/${encodeURIComponent(goal.id)}`);
  if (!isNodeContextGenerationCurrent(nodeGeneration)) return;
  const root = hubModal("Control workflow outcome", `<p>Move this Goal with context or explicitly override its normal workflow gates. Overrides remain in its history.</p><label>Action<select data-action><option value="move">Move to step</option><option value="integrate">Force candidate integration</option></select></label><label>Destination<select data-to>${["backlog","todo","plan","implement","quality","governance","review","done","failed","cancelled"].map(s=>`<option ${s===data.status?"selected":""}>${s}</option>`).join("")}</select></label><label>Reason<input data-reason required></label><label>Additional context<textarea data-context rows="8"></textarea></label><label><input type="checkbox" data-force>Force this decision and record bypassed requirements</label><p data-force-help>Forced Done changes status only; it does not integrate code.</p><button data-apply>Apply decision</button>`);
  root.querySelector("[data-action]").onchange = () => {const integrate=root.querySelector("[data-action]").value==="integrate";root.querySelector("[data-to]").disabled=integrate;if(integrate)root.querySelector("[data-to]").value="governance";};
  root.querySelector("[data-apply]").onclick = () => hubAction(root, async () => {
    const reason=root.querySelector("[data-reason]").value.trim();if(!reason)throw new Error("Enter a reason.");
    await hubApi(root,"POST",`/api/workflow/goals/${encodeURIComponent(goal.id)}/${root.querySelector("[data-action]").value}`,{to:root.querySelector("[data-to]").value,reason,context:root.querySelector("[data-context]").value,force:root.querySelector("[data-force]").checked,expected_revision:data.workflow_revision||0,request_id:hubId(),actor:state.lastReporter||"operator"});
    root._close();if(typeof loadGoalDetail==="function")await loadGoalDetail(goal.id);
  });
}
function renderWorkflowOutcome(goal) {
  const pending=goal.pending_workflow_outcome;
  const controls=goal.workflow_controls||[];
  return `${pending?.state==="pending"?`<div class="banner warn" role="status">Handling error: ${htmlEscape(pending.message||"")}</div>`:""}${controls.length?`<details><summary>Workflow decisions (${controls.length})</summary><ul>${controls.slice(-20).map(c=>`<li>${htmlEscape(c.at)} — ${htmlEscape(c.from)} → ${htmlEscape(c.to)}${c.forced?" (explicit override)":""}: ${htmlEscape(c.request?.reason||"")}</li>`).join("")}</ul></details>`:""}`;
}
