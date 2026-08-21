// ---- Missions: detail -------------------------------------------------------

async function renderMissionDetail(route) {
  if (renderNoProjectIfDetached("Missions")) return;
  const nodeGeneration = captureNodeContextGeneration();
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
  const primaryAction = missionPrimaryAction(mission);
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
        </div>
      </div>
      <div class="mission-detail-intent card" data-testid="mission-intent">
        <div class="modal-title compact">Intent</div>
        <p>${htmlEscape(mission.intent || "")}</p>
      </div>
      <div class="mission-detail-rollup card" data-testid="mission-rollup">
        <div class="modal-title compact">Contained Goals</div>
        <p class="muted small">
          ${rollup.goal_count || 0} goals · ${rollup.done_count || 0} done · ${rollup.active_count || 0} active
          · ${rollup.failed_count || 0} failed · ${rollup.cancelled_count || 0} cancelled
        </p>
      </div>
      <div class="mission-detail-actions" data-testid="mission-actions">
        ${primaryAction ? `<button class="primary" id="mission-primary-action" data-testid="mission-primary-action">${htmlEscape(primaryAction.label)}</button>` : ""}
        ${status !== "cancelled" && status !== "done" && status !== "failed"
          ? `<button class="secondary" id="mission-cancel" data-testid="mission-cancel">Cancel Mission</button>` : ""}
      </div>
      <div class="mission-detail-goals card" data-testid="mission-goals">
        <div class="modal-title compact">Goals</div>
        ${goals.length ? `
          <div class="table-scroll">
            <table class="table work-items-table mobile-card-table">
              <thead><tr><th>Goal</th><th>Status</th><th>Priority</th><th>Updated</th></tr></thead>
              <tbody>
                ${goals.map((goal) => `
                  <tr>
                    <td><a href="#/goals/${encodeURIComponent(goal.id)}">${htmlEscape(goal.name || goal.id)}</a></td>
                    <td><span class="status-pill ${htmlEscape(goal.status || "backlog")}">${workflowStatusLabel(goal.status || "backlog")}</span></td>
                    <td>${htmlEscape(goal.priority || "low")}</td>
                    <td class="muted small">${fmtTime(goal.updated)}</td>
                  </tr>`).join("")}
              </tbody>
            </table>
          </div>` : `<p class="muted">No Goals are bound to this Mission yet.</p>`}
      </div>
    </div>
  `;
  if (primaryAction) {
    bindOnce($("#mission-primary-action"), "click", () => primaryAction.run(mission));
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
}

function missionPrimaryAction(mission) {
  const status = mission.status || "draft";
  const id = mission.id;
  switch (status) {
    case "draft":
      return {
        label: "Begin investigation",
        run: async (m) => {
          try {
            await api("POST", `/api/missions/${encodeURIComponent(id)}/start`);
            toast("Investigation started", "info");
            location.hash = `#/missions/${encodeURIComponent(id)}`;
          } catch (e) {
            showActionError(e);
          }
        },
      };
    case "review":
      return {
        label: "Review Outcome",
        run: async (m) => {
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
        run: async (m) => {
          location.hash = `#/missions/${encodeURIComponent(id)}`;
        },
      };
    default:
      return null;
  }
}
