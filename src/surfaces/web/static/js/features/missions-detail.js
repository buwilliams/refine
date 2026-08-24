// ---- Missions: detail workbench ----------------------------------------------
//
// A full-page workbench over the local daemon: header with status, Round,
// coordinator, and snapshot; URL-backed sections for Overview, Plan, Work,
// Context, Review, and Outcome. The browser never advances Mission state
// itself; every action calls a Mission capability and rereads authoritative
// state.

const MISSION_SECTIONS = ["overview", "plan", "work", "context", "review", "outcome"];

function missionSectionFromHash() {
  const hashQs = new URLSearchParams(location.hash.split("?")[1] || "");
  const section = (hashQs.get("section") || "overview").toLowerCase();
  return MISSION_SECTIONS.includes(section) ? section : "overview";
}

function missionSectionLink(id, section) {
  return `#/missions/${encodeURIComponent(id)}?section=${section}`;
}

async function renderMissionDetail(route) {
  if (renderNoProjectIfDetached("Missions")) return;
  const nodeGeneration = captureNodeContextGeneration();
  const section = missionSectionFromHash();
  $("#main").innerHTML = `<p class="muted">Loading Mission…</p>`;
  let data;
  try {
    data = await api("GET", `/api/missions/${encodeURIComponent(route.id)}`);
  } catch (e) {
    if (!isNodeContextGenerationCurrent(nodeGeneration)) return;
    $("#main").innerHTML = `<p class="muted">${htmlEscape(e.message)}</p>`;
    return;
  }
  if (!isNodeContextGenerationCurrent(nodeGeneration)) return;
  const mission = data.mission || {};
  const rollup = data.rollup || {};
  const goals = data.goals || [];
  const status = mission.status || "draft";
  const round = missionCurrentRound(mission);
  const plan = (round && round.plan) || null;
  const pendingAmendment = round && round.plan_amendments && round.plan_amendments.length
    ? round.plan_amendments[round.plan_amendments.length - 1]
    : null;
  const approved = round && round.phase_evidence && round.phase_evidence.plan_approval;
  const effectivePlan = (!approved && pendingAmendment) || plan;
  const effectiveDigest = effectivePlan && effectivePlan.effective_digest;
  const primaryAction = missionPrimaryAction(mission, { effectiveDigest, approved: Boolean(approved) });
  const snapshots = (round && round.snapshots) || [];
  const latestSnapshot = snapshots.length ? snapshots[snapshots.length - 1] : null;

  $("#main").innerHTML = `
    <div class="mission-detail" data-testid="mission-detail">
      <div class="mission-detail-head">
        <div class="mission-detail-title-row">
          <h2>${htmlEscape(mission.name || "Untitled Mission")}</h2>
          <span class="status-pill ${htmlEscape(status)}" data-testid="mission-status-pill">${missionStatusLabel(status)}</span>
          ${mission.current_round ? `<span class="muted small">Round #${mission.current_round}</span>` : ""}
        </div>
        <div class="mission-detail-meta muted small" data-testid="mission-metadata">
          ID <code>${htmlEscape(mission.id)}</code> · created ${fmtTime(mission.created)} · updated ${fmtTime(mission.updated)}
          · reporter <strong>${htmlEscape(mission.reporter || "unreported")}</strong>
          ${mission.coordinator_node_id ? ` · coordinator <strong>${htmlEscape(mission.coordinator_node_id)}</strong>` : ""}
          ${latestSnapshot ? ` · snapshot ${latestSnapshot.version}` : ""}
        </div>
        <nav class="mission-sections" aria-label="Mission sections" data-testid="mission-sections">
          ${MISSION_SECTIONS.map((s) => `
            <a href="${missionSectionLink(mission.id, s)}"
               class="mission-section-link ${s === section ? "active" : ""}">${missionSectionLabel(s)}</a>`).join("")}
        </nav>
      </div>
      <div class="mission-detail-actions" data-testid="mission-actions">
        ${primaryAction ? `<button class="primary" id="mission-primary-action" data-testid="mission-primary-action">${htmlEscape(primaryAction.label)}</button>` : ""}
        ${status !== "cancelled" && status !== "done" && status !== "failed"
    ? `<button class="secondary" id="mission-cancel" data-testid="mission-cancel">Cancel Mission</button>` : ""}
      </div>
      <div id="mission-section-body" class="mission-section-body">
        ${renderMissionSection(section, { mission, round, plan, effectivePlan, effectiveDigest, approved: Boolean(approved), pendingAmendment, rollup, goals, snapshots, latestSnapshot })}
      </div>
    </div>
  `;
  bindMissionDetailActions(route, { mission, effectiveDigest, approved: Boolean(approved) });
}

function missionCurrentRound(mission) {
  if (!mission.rounds || !mission.rounds.length) return null;
  const number = mission.current_round || mission.rounds[mission.rounds.length - 1].number;
  return mission.rounds.find((r) => r.number === number) || mission.rounds[mission.rounds.length - 1];
}

function missionSectionLabel(section) {
  return { overview: "Overview", plan: "Plan", work: "Work", context: "Context", review: "Review", outcome: "Outcome" }[section] || section;
}

function renderMissionSection(section, ctx) {
  switch (section) {
    case "plan": return renderMissionPlanSection(ctx);
    case "work": return renderMissionWorkSection(ctx);
    case "context": return renderMissionContextSection(ctx);
    case "review": return renderMissionReviewSection(ctx);
    case "outcome": return renderMissionOutcomeSection(ctx);
    default: return renderMissionOverviewSection(ctx);
  }
}

function renderMissionOverviewSection({ mission, rollup, goals, latestSnapshot, round }) {
  const criteria = mission.success_criteria || [];
  const receipts = (round && round.reconciliation_receipts) || [];
  return `
    <div class="mission-detail-intent card" data-testid="mission-intent">
      <div class="modal-title compact">Intent</div>
      <p>${htmlEscape(mission.intent || "")}</p>
      ${criteria.length ? `
        <div class="modal-title compact">Success criteria</div>
        <ul class="mission-criteria">
          ${criteria.map((criterion) => `<li><code>${htmlEscape(criterion.id)}</code> ${htmlEscape(criterion.description)}</li>`).join("")}
        </ul>` : ""}
    </div>
    <div class="mission-detail-rollup card" data-testid="mission-rollup">
      <div class="modal-title compact">Contained Goals</div>
      <p class="muted small">
        ${rollup.goal_count || 0} goals · ${rollup.done_count || 0} done · ${rollup.active_count || 0} active
        · ${rollup.failed_count || 0} failed · ${rollup.cancelled_count || 0} cancelled
      </p>
    </div>
    ${latestSnapshot ? `
    <div class="card" data-testid="mission-snapshot">
      <div class="modal-title compact">Current snapshot</div>
      <p class="muted small">
        version ${latestSnapshot.version} · ${latestSnapshot.digest ? `<code>${htmlEscape(latestSnapshot.digest.slice(0, 19))}…</code>` : "no digest"}
        · ${(latestSnapshot.knowledge_index || []).length} accepted assertions
        · ${(latestSnapshot.artifact_refs || []).length} artifacts
      </p>
    </div>` : ""}
    ${receipts.length ? `
    <div class="card" data-testid="mission-reconciliation">
      <div class="modal-title compact">Reconciliation history</div>
      <p class="muted small">${receipts.length} receipt(s); latest attempt <code>${htmlEscape(receipts[receipts.length - 1].attempt)}</code></p>
    </div>` : ""}
    ${goals.length ? renderMissionGoalsTable(goals) : `<div class="card"><p class="muted">No Goals are bound to this Mission yet.</p></div>`}
  `;
}

function renderMissionGoalsTable(goals) {
  return `
    <div class="mission-detail-goals card" data-testid="mission-goals">
      <div class="modal-title compact">Goals</div>
      <div class="table-scroll">
        <table class="table work-items-table mobile-card-table">
          <thead><tr><th>Goal</th><th>Status</th><th>Priority</th><th>Updated</th></tr></thead>
          <tbody>
            ${goals.map((goal) => `
              <tr>
                <td><a href="#/goals/${encodeURIComponent(goal.id)}">${htmlEscape(goal.name || goal.id)}</a>
                  ${goal.mission ? `<span class="muted small"> · ${htmlEscape(goal.mission.mission_goal_key)}</span>` : ""}</td>
                <td><span class="status-pill ${htmlEscape(goal.status || "backlog")}">${workflowStatusLabel(goal.status || "backlog")}</span></td>
                <td>${htmlEscape(goal.priority || "low")}</td>
                <td class="muted small">${fmtTime(goal.updated)}</td>
              </tr>`).join("")}
          </tbody>
        </table>
      </div>
    </div>
  `;
}

function renderMissionPlanSection({ mission, plan, effectivePlan, effectiveDigest, approved, pendingAmendment }) {
  if (!effectivePlan) {
    return `<div class="card"><p class="muted">No plan has been drafted yet. The planning agents draft one after investigation.</p></div>`;
  }
  const waves = effectivePlan.waves || [];
  return `
    <div class="card" data-testid="mission-plan-summary">
      <div class="modal-title compact">${approved && !pendingAmendment ? "Approved plan" : pendingAmendment ? "Pending amendment" : "Drafted plan"}</div>
      <p>${htmlEscape(effectivePlan.summary || "")}</p>
      <p class="muted small">
        effective digest <code>${htmlEscape(effectiveDigest || "—")}</code>
        ${effectivePlan.criteria_coverage && effectivePlan.criteria_coverage.length ? ` · covers ${effectivePlan.criteria_coverage.length} criterion(ies)` : ""}
      </p>
      ${!approved ? `
        <button class="primary" id="mission-approve-plan" data-testid="mission-approve-plan"
                ${effectiveDigest ? "" : "disabled"}>Approve plan</button>` : ""}
    </div>
    ${waves.map((wave) => `
      <div class="card mission-wave" data-testid="mission-wave">
        <div class="modal-title compact">Wave ${wave.number}: ${htmlEscape(wave.purpose || "")}</div>
        ${(wave.goal_specs || []).map((spec) => `
          <div class="mission-goal-spec" data-testid="mission-goal-spec">
            <strong>${htmlEscape(spec.name)}</strong>
            <span class="muted small">key <code>${htmlEscape(spec.mission_goal_key)}</code>
              · ${spec.required ? "required" : "optional"}
              ${spec.role ? ` · ${htmlEscape(spec.role)}` : ""}
              ${spec.preferred_node ? ` · prefers ${htmlEscape(spec.preferred_node)}` : ""}
              ${spec.feature_id ? ` · feature ${htmlEscape(spec.feature_id)}` : ""}
            </span>
            <p class="small">${htmlEscape(spec.prompt || "")}</p>
            ${spec.criterion_ids && spec.criterion_ids.length ? `<p class="muted small">advances: ${spec.criterion_ids.map(htmlEscape).join(", ")}</p>` : ""}
          </div>`).join("") || `<p class="muted small">No Goal specifications in this wave.</p>`}
      </div>`).join("")}
    <div class="card" id="mission-distribution-preview" data-testid="mission-distribution">
      <div class="modal-title compact">Distribution preview</div>
      <p class="muted small">Loading…</p>
    </div>
  `;
}

function renderMissionWorkSection({ mission, goals, plan, round }) {
  const waves = (plan && plan.waves) || [];
  const boundByKey = new Map((goals || []).map((goal) => [
    goal.mission ? goal.mission.mission_goal_key : goal.id,
    goal,
  ]));
  const distribution = round && round.phase_evidence && round.phase_evidence.distribution;
  return `
    ${waves.map((wave) => {
    const receipts = distribution && distribution[wave.number];
    return `
      <div class="card mission-wave" data-testid="mission-work-wave">
        <div class="modal-title compact">Wave ${wave.number}: ${htmlEscape(wave.purpose || "")}</div>
        <div class="table-scroll">
          <table class="table work-items-table mobile-card-table">
            <thead><tr><th>Goal</th><th>Status</th><th>Node</th><th>Distribution</th></tr></thead>
            <tbody>
              ${(wave.goal_specs || []).map((spec) => {
      const goal = boundByKey.get(spec.mission_goal_key);
      const assignment = receipts && receipts.assignments
        ? receipts.assignments.find((a) => a.mission_goal_key === spec.mission_goal_key)
        : null;
      return `
                <tr>
                  <td>${goal ? `<a href="#/goals/${encodeURIComponent(goal.id)}">${htmlEscape(goal.name || goal.id)}</a>` : `<span class="muted">not materialized</span>`}</td>
                  <td>${goal ? `<span class="status-pill ${htmlEscape(goal.status || "backlog")}">${workflowStatusLabel(goal.status || "backlog")}</span>` : "—"}</td>
                  <td class="muted small">${htmlEscape((goal && goal.node_id) || "default")}</td>
                  <td class="muted small">${assignment
        ? `${htmlEscape(assignment.target_node_id || "—")}${assignment.reason ? ` (${htmlEscape(assignment.reason)})` : ""}`
        : "—"}</td>
                </tr>`;
    }).join("")}
            </tbody>
          </table>
        </div>
      </div>`;
  }).join("") || `<div class="card"><p class="muted">No approved plan yet.</p></div>`}
  `;
}

function renderMissionContextSection({ mission }) {
  // The Context section loads its own projection; the placeholder is
  // replaced after render because the section body is one fetch away.
  return `<div class="card" id="mission-context-card" data-testid="mission-context-card">
    <div class="modal-title compact">Mission context</div>
    <p class="muted">Loading…</p>
  </div>`;
}

function renderMissionReviewSection({ mission, round }) {
  const outcome = round && round.outcome;
  const criteria = (mission.success_criteria || []);
  if (!outcome) {
    return `<div class="card"><p class="muted">The candidate Outcome appears here after Synthesis.</p></div>`;
  }
  const results = new Map((outcome.criteria_results || []).map((result) => [result.criterion_id, result]));
  return `
    <div class="card" data-testid="mission-review-matrix">
      <div class="modal-title compact">Criteria matrix</div>
      <div class="table-scroll">
        <table class="table work-items-table mobile-card-table">
          <thead><tr><th>Criterion</th><th>Result</th><th>Evidence</th></tr></thead>
          <tbody>
            ${criteria.map((criterion) => {
    const result = results.get(criterion.id);
    return `
              <tr>
                <td><code>${htmlEscape(criterion.id)}</code> ${htmlEscape(criterion.description)}</td>
                <td><span class="status-pill ${result ? criterionPillClass(result.result) : "backlog"}">${result ? htmlEscape(result.result) : "not judged"}</span></td>
                <td class="muted small">${result && result.evidence ? result.evidence.map(htmlEscape).join("; ") : "—"}</td>
              </tr>`;
  }).join("")}
          </tbody>
        </table>
      </div>
      <p class="muted small">manifest digest <code>${htmlEscape(outcome.manifest_digest || "—")}</code></p>
    </div>
  `;
}

function renderMissionOutcomeSection({ mission, round }) {
  const outcome = round && round.outcome;
  const publication = round && round.outcome_publication;
  if (!outcome) {
    return `<div class="card"><p class="muted">No Outcome has been published for this Mission${mission.status === "done" ? " in this Round" : " yet"}.</p></div>`;
  }
  return `
    <div class="card" data-testid="mission-outcome-manifest">
      <div class="modal-title compact">Outcome manifest</div>
      <p class="muted small">
        Round ${outcome.mission_round}
        · manifest digest <code>${htmlEscape(outcome.manifest_digest || "—")}</code>
        · final snapshot ${outcome.final_snapshot || "—"}
      </p>
      ${(outcome.artifact_refs || []).length ? `
        <div class="modal-title compact">Artifacts</div>
        <ul>
          ${outcome.artifact_refs.map((artifact) => `<li><code>${htmlEscape(artifact.key)}</code> ${htmlEscape(artifact.title || "")} <span class="muted small">${htmlEscape(artifact.authority || "")}</span></li>`).join("")}
        </ul>` : ""}
      ${(outcome.target_commit_refs || []).length ? `
        <div class="modal-title compact">Target commits</div>
        <ul>${outcome.target_commit_refs.map((commit) => `<li><code>${htmlEscape(commit)}</code></li>`).join("")}</ul>` : ""}
    </div>
    ${publication ? `
    <div class="card" data-testid="mission-outcome-publication">
      <div class="modal-title compact">Publication receipt</div>
      <p class="muted small">
        state commit <code>${htmlEscape(publication.outcome_state_commit || "—")}</code>
        · ${publication.verified_path_digests ? publication.verified_path_digests.length : 0} verified path(s)
        · published by ${htmlEscape(publication.published_by || "—")} ${publication.verified_at ? `at ${fmtTime(publication.verified_at)}` : ""}
      </p>
    </div>` : ""}
  `;
}

function criterionPillClass(result) {
  switch (result) {
    case "met": return "done";
    case "partial": return "review";
    case "unmet":
    case "contradicted": return "failed";
    case "waived": return "cancelled";
    default: return "backlog";
  }
}

async function hydrateMissionSectionExtras(route) {
  const section = missionSectionFromHash();
  if (section === "plan") await hydrateMissionDistributionPreview(route);
  if (section === "context") await hydrateMissionContext(route);
}

async function hydrateMissionDistributionPreview(route) {
  const host = $("#mission-distribution-preview");
  if (!host) return;
  try {
    const data = await api(
      "GET",
      `/api/missions/${encodeURIComponent(route.id)}/distribution?wave=1`
    );
    const distribution = data.distribution || {};
    const nodes = distribution.eligible_nodes || [];
    const assignments = distribution.assignments || [];
    host.innerHTML = `
      <div class="modal-title compact">Distribution preview</div>
      <p class="muted small">eligible nodes: ${nodes.length ? nodes.map(htmlEscape).join(", ") : "none"}</p>
      ${nodes.length === 0 ? `<p class="small">No eligible node is enabled and healthy; distribution waits for capacity.</p>` : `
      <ul class="small">
        ${assignments.map((assignment) => `<li>
          <code>${htmlEscape(assignment.mission_goal_key)}</code> → ${htmlEscape(assignment.target_node_id || "—")}
          ${assignment.reason ? `<span class="muted">(${htmlEscape(assignment.reason)})</span>` : ""}
        </li>`).join("")}
      </ul>`}
    `;
  } catch (e) {
    host.innerHTML = `
      <div class="modal-title compact">Distribution preview</div>
      <p class="muted small">${htmlEscape(e.message)}</p>
    `;
  }
}

async function hydrateMissionContext(route) {
  const host = $("#mission-context-card");
  if (!host) return;
  try {
    const data = await api("GET", `/api/missions/${encodeURIComponent(route.id)}/context`);
    const context = data.context || {};
    const assertions = context.assertions || [];
    const artifacts = context.artifacts || [];
    const contradictions = context.open_contradictions || [];
    const decisions = context.open_decisions || [];
    host.innerHTML = `
      <div class="modal-title compact">Accepted knowledge (snapshot ${context.latest_snapshot || 0})</div>
      ${assertions.length ? `
      <div class="table-scroll">
        <table class="table work-items-table mobile-card-table">
          <thead><tr><th>Claim</th><th>Kind</th><th>Authority</th><th>State</th></tr></thead>
          <tbody>
            ${assertions.map((assertion) => `
              <tr>
                <td class="small">${htmlEscape(assertion.claim || "")}</td>
                <td class="muted small">${htmlEscape(assertion.kind || "")}</td>
                <td class="muted small">${htmlEscape(assertion.authority || "")}${assertion.qualified ? ` <span class="muted">(${htmlEscape(assertion.qualified)})</span>` : ""}</td>
                <td class="muted small">${htmlEscape(assertion.state || "")}</td>
              </tr>`).join("")}
          </tbody>
        </table>
      </div>` : `<p class="muted small">No accepted knowledge yet.</p>`}
      ${artifacts.length ? `
        <div class="modal-title compact">Artifacts</div>
        <ul class="small">
          ${artifacts.map((artifact) => `<li><code>${htmlEscape(artifact.key)}</code> ${htmlEscape(artifact.title || "")} <span class="muted">${htmlEscape(artifact.authority || "")}</span></li>`).join("")}
        </ul>` : ""}
      ${contradictions.length ? `
        <div class="modal-title compact">Open contradictions</div>
        <ul class="small">
          ${contradictions.map((contradiction) => `<li class="small">${htmlEscape(contradiction.claim || "")} <span class="muted">(${htmlEscape(contradiction.state || "")})</span></li>`).join("")}
        </ul>` : ""}
      ${decisions.length ? `
        <div class="modal-title compact">Open decisions</div>
        <ul class="small">
          ${decisions.map((decision) => `<li><code>${htmlEscape(decision.id)}</code> ${htmlEscape(decision.summary)}
            ${decision.choices && decision.choices.length ? `<span class="muted">(${decision.choices.map(htmlEscape).join(" / ")})</span>` : ""}
          </li>`).join("")}
        </ul>` : ""}
    `;
  } catch (e) {
    host.innerHTML = `
      <div class="modal-title compact">Mission context</div>
      <p class="muted small">${htmlEscape(e.message)}</p>
    `;
  }
}

function bindMissionDetailActions(route, { mission, effectiveDigest, approved }) {
  const primary = missionPrimaryAction(mission, { effectiveDigest, approved });
  if (primary) {
    bindOnce($("#mission-primary-action"), "click", () => primary.run(mission));
  }
  const cancelBtn = $("#mission-cancel");
  if (cancelBtn) {
    bindOnce(cancelBtn, "click", async () => {
      const ok = await modalConfirm("Cancel this Mission? Active child Goals are not cancelled.", {
        title: "Cancel Mission",
        okLabel: "Cancel Mission",
        danger: true,
      });
      if (!ok) return;
      try {
        await api("POST", `/api/missions/${encodeURIComponent(mission.id)}/cancel`);
        toast("Mission cancelled", "info");
        renderMissionDetail(route);
      } catch (e) {
        showActionError(e);
      }
    });
  }
  const approveBtn = $("#mission-approve-plan");
  if (approveBtn) {
    bindOnce(approveBtn, "click", async () => {
      const ok = await modalConfirm(
        `Approve this plan and authorize Goal materialization and distribution?\n\nEffective digest: ${effectiveDigest || "—"}`,
        { title: "Approve plan", okLabel: "Approve" }
      );
      if (!ok) return;
      try {
        await api("POST", `/api/missions/${encodeURIComponent(mission.id)}/approve-plan`, {
          plan_digest: effectiveDigest,
          actor: mission.reporter || "",
          rationale: "approved from the Mission workbench",
        });
        toast("Plan approved", "success");
        renderMissionDetail(route);
      } catch (e) {
        showActionError(e);
      }
    });
  }
  hydrateMissionSectionExtras(route);
}

function missionPrimaryAction(mission, { effectiveDigest, approved } = {}) {
  const status = mission.status || "draft";
  const id = mission.id;
  switch (status) {
    case "draft":
      return {
        label: "Begin investigation",
        run: async () => {
          try {
            await api("POST", `/api/missions/${encodeURIComponent(id)}/start`);
            toast("Investigation started", "info");
            location.hash = `#/missions/${encodeURIComponent(id)}`;
          } catch (e) {
            showActionError(e);
          }
        },
      };
    case "plan":
      if (!approved && effectiveDigest) {
        return {
          label: "Review and approve plan",
          run: async () => {
            const ok = await modalConfirm(
              `Approve this plan and authorize Goal materialization and distribution?\n\nEffective digest: ${effectiveDigest}`,
              { title: "Approve plan", okLabel: "Approve" }
            );
            if (!ok) return;
            try {
              await api("POST", `/api/missions/${encodeURIComponent(id)}/approve-plan`, {
                plan_digest: effectiveDigest,
                actor: mission.reporter || "",
                rationale: "approved from the Mission workbench",
              });
              toast("Plan approved", "success");
              renderMissionDetail({ id });
            } catch (e) {
              showActionError(e);
            }
          },
        };
      }
      return null;
    case "review":
      return {
        label: "Review Outcome",
        run: async () => {
          const ok = await modalConfirm("Approve this Mission Outcome and authorize consolidation?", {
            title: "Approve Outcome",
            okLabel: "Approve",
          });
          if (!ok) return;
          try {
            await api("POST", `/api/missions/${encodeURIComponent(id)}/approve-outcome`);
            toast("Outcome approved", "success");
            location.hash = `#/missions/${encodeURIComponent(id)}`;
          } catch (e) {
            showActionError(e);
          }
        },
      };
    case "done":
      return {
        label: "Start new Round",
        run: async () => {
          const ok = await modalConfirm("Append a new Round to this Done Mission? Its prior Outcome remains immutable.", {
            title: "Start new Round",
            okLabel: "Append Round",
          });
          if (!ok) return;
          try {
            await api("POST", `/api/missions/${encodeURIComponent(id)}/rounds`, {
              reporter: mission.reporter || "",
              prompt: "continue this Mission with a new Round",
            });
            toast("Round appended", "success");
            renderMissionDetail({ id });
          } catch (e) {
            showActionError(e);
          }
        },
      };
    default:
      return null;
  }
}
