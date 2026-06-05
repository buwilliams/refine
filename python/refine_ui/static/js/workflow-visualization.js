// ---- Workflow visualization ------------------------------------------------

const AGENT_MANAGED_WORKFLOW_STATUSES = new Set([
  "todo",
  "in-progress",
  "qa",
  "ready-merge",
  "awaiting-rebuild",
]);

const WORKFLOW_VISUALIZATION_LABELS = {
  "ready-merge": "Ready merge",
  "awaiting-rebuild": "Rebuild",
};

function workflowVisualizationLabel(status) {
  return WORKFLOW_VISUALIZATION_LABELS[status] || workflowStatusLabel(status);
}

function renderWorkflowVisualization({
  counts = {},
  statuses = workflowStatuses(),
  hrefForStatus = null,
  className = "",
} = {}) {
  const classes = ["card-grid", "workflow-status-grid", className]
    .filter(Boolean)
    .join(" ");
  return `
    <section class="${classes}">
      ${statuses.map((s) => {
        const count = counts[s] || 0;
        const agentManaged = AGENT_MANAGED_WORKFLOW_STATUSES.has(s);
        const label = workflowStatusLabel(s);
        const displayLabel = workflowVisualizationLabel(s);
        const body = `
          <div class="workflow-status-head">
            ${agentManaged ? `<span class="workflow-agent-indicator" aria-label="AI-managed automation">AI</span>` : ""}
            <div class="workflow-status-label">${displayLabel}</div>
          </div>
          <div class="workflow-status-count">${fmtCount(count)}</div>`;
        const attrs = `class="card workflow-status-card ${s}${agentManaged ? " workflow-status-card-agent" : ""}" title="${count} ${label} gap${count === 1 ? "" : "s"}${agentManaged ? " - agent-managed automation" : ""}"`;
        const href = hrefForStatus ? hrefForStatus(s) : "";
        return href
          ? `<a ${attrs} href="${htmlEscape(href)}" style="text-decoration:none;color:inherit">${body}</a>`
          : `<div ${attrs}>${body}</div>`;
      }).join("")}
    </section>`;
}
