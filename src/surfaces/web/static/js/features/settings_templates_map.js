// The map follows saved template references, plus launch-supplied composition slots.
// Slot alternatives are marked conditional: a launch chooses the applicable piece.
let templateCatalogView = "workflow";
let templateMapRoot = "workflow";
let templateMapExpanded = new Set(["workflow"]);
let templateMapObserver = null;
const templateCatalogViews = {workflow: "Goal workflow", interactive: "Interactive agents", tasks: "System tasks", all: "All entries"};
const templateCompositionSlots = {
  "supervised-skill": {attached_skills: ["context-skill"], continuation: ["workflow-continuation"], observational: ["workflow-observational"]},
  "manual-skill": {},
  "fleet-manage": {message: ["fleet-distribute"]},
  "terminal-session": {
    instructions: ["planning-agent", "agent", "goal-agent", "chat-standalone"],
    workflow_context: ["terminal-profiles-toolbar-agent-workflow", "terminal-profiles-general-agent-workflow"],
    active_refine: ["terminal-profiles-active-refine"], goal_attachment: ["terminal-profiles-attached-goal"],
    feature_attachment: ["terminal-profiles-attached-feature"], profile_context: ["terminal-profiles-plan", "terminal-profiles-goal-diagnostic"],
    supplemental_attachment: ["terminal-profiles-supplemental-context"],
  },
  "chat-session": {instructions: ["planning-agent", "goal-agent", "chat-feature", "chat-standalone"], context: ["chat-context-unavailable"]},
  "goal-agents-session": {goal_prompt: ["workflow", "supervised-skill"], completion_contract: ["goal-completion"]},
  "conflict-resolution": {context: ["sync-resolve-state-conflict"], ancestry_context: ["conflict-ancestry"], feedback_context: ["conflict-feedback"]},
};

function templateComposition(data) {
  const rows = new Map((data.items || []).map(row => [row.item.id, row]));
  const edges = [];
  rows.forEach((row, id) => {
    const names = new Set([...row.item.prompt.matchAll(/(?<!\\){{\s*([\w.-]+)\s*}}/g)].map(match => match[1]));
    const targets = new Map();
    names.forEach(name => {
      if (name.startsWith("templates.")) targets.set(name.slice(10), false);
      else (templateCompositionSlots[id]?.[name] || []).forEach(target => { if (!targets.has(target)) targets.set(target, true); });
    });
    targets.forEach((conditional, to) => { if (rows.has(to)) edges.push({from: id, to, conditional}); });
    if (names.has("skill")) {
      edges.push({from: id, to: "$skill", conditional: true});
    }
  });
  return {rows, edges};
}

function renderTemplateMap(data) {
  const {rows, edges} = templateComposition(data);
  if (!rows.has(templateMapRoot)) templateMapRoot = rows.has("workflow") ? "workflow" : rows.keys().next().value;
  if (!templateMapRoot) return "";
  const levels = new Map([[templateMapRoot, 0]]), visibleEdges = [];
  // Stop at an already traversed node: shared partials appear once, not as copies.
  const visit = (id, ancestors = []) => {
    if (!templateMapExpanded.has(id)) return;
    for (const edge of edges.filter(edge => edge.from === id)) {
      if (ancestors.includes(edge.to) || edge.to === id) continue;
      visibleEdges.push(edge);
      const seen = levels.has(edge.to);
      levels.set(edge.to, Math.max(levels.get(edge.to) || 0, levels.get(id) + 1));
      if (!seen) visit(edge.to, [...ancestors, id]);
    }
  };
  visit(templateMapRoot);
  for (let pass = 0; pass < levels.size; pass++) {
    let changed = false;
    visibleEdges.forEach(edge => {
      const next = levels.get(edge.from) + 1;
      if (next > levels.get(edge.to)) { levels.set(edge.to, next); changed = true; }
    });
    if (!changed) break;
  }
  const columns = [];
  levels.forEach((level, id) => { (columns[level] ||= []).push(id); });
  const card = id => {
    if (id === "$skill") return `<div class="template-map-node template-map-skill" data-map-node="$skill"><span class="muted small">Skill</span><a href="#/settings/skills">Assigned Skill</a><p class="muted small">Selected for this step or task.</p></div>`;
    const row = rows.get(id), children = edges.filter(edge => edge.from === id);
    return `<div class="template-map-node" data-map-node="${htmlEscape(id)}">
      <span class="muted small">${row.usage?.kind === "partial" ? "Partial" : "Template"}</span>
      <button type="button" class="template-map-name" data-template-id="${htmlEscape(id)}">${htmlEscape(row.name)}</button>
      ${children.length ? `<button type="button" class="template-map-expand" data-map-expand="${htmlEscape(id)}" aria-expanded="${templateMapExpanded.has(id)}">${templateMapExpanded.has(id) ? "Hide" : "Show"} ${children.length} included ${children.length === 1 ? "piece" : "pieces"}</button>` : `<span class="muted small">${row.customized ? "Customized" : "Default"}</span>`}
    </div>`;
  };
  return `<div class="template-map-scroll" tabindex="0" aria-label="Template composition; scroll to explore">
    <div class="template-map" data-map-edges="${htmlEscape(JSON.stringify(visibleEdges))}"><svg class="template-map-lines" aria-hidden="true"></svg>
      ${columns.map(column => `<div class="template-map-column">${column.map(card).join("")}</div>`).join("")}
    </div></div>
    <p class="muted small template-map-legend">Lines mean “includes.” Dashed lines are selected by the launch when needed. Click a name to edit; expand a node to see its pieces.</p>`;
}

function renderTemplatesCatalog(data = {}) {
  const descriptions = {
    workflow: "All four steps use Workflow with the Skill assigned to that step. Shared partials keep the common context in one place.",
    interactive: "Toolbar and CLI terminals use Terminal Session. Managed chats use Chat Session. Each includes instructions for the selected agent mode.",
    tasks: "Imports, releases, fleet management, target-app operations, and state repair each have a specific starting template.",
    all: "Every editable entry, including reusable partials and reference entries without a current built-in launch.",
  };
  const {edges, rows} = templateComposition(data);
  const primary = ["workflow", "planning-agent", "agent", "goal-agent", "terminal-session", "chat-session"];
  const rank = row => primary.includes(row.item.id) ? primary.indexOf(row.item.id) : row.usage?.group === "delivery" ? 9 : row.usage?.kind === "partial" ? 8 : 7;
  const items = [...rows.values()].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
  return `<section class="settings-section" data-testid="settings-templates">
    <h3>Templates</h3><p class="muted">Templates build the prompts Refine sends to agents. Partials are reusable pieces included inside those prompts.</p>
    <div class="flat-tabs template-catalog-tabs" role="tablist" aria-label="Template uses">${Object.entries(templateCatalogViews).map(([key, label]) => `<button type="button" role="tab" id="template-view-${key}" aria-controls="template-catalog-panel" data-template-view="${key}" aria-selected="${key === templateCatalogView}" tabindex="${key === templateCatalogView ? 0 : -1}">${label}</button>`).join("")}</div>
    <div role="tabpanel" id="template-catalog-panel" aria-labelledby="template-view-${templateCatalogView}"><p class="muted">${descriptions[templateCatalogView]}</p>
    ${templateCatalogView === "workflow" ? `<ol class="template-workflow-steps" aria-label="Goal workflow">${[["Plan", "Define the approach"], ["Implement", "Build the plan"], ["Quality", "Test the outcome"], ["Governance", "Verify the Goal"]].map(([name, detail]) => `<li><strong>${name}</strong><span class="muted small">${detail}</span></li>`).join("")}</ol>` : ""}
    <div data-template-map-host>${renderTemplateMap(data)}</div>
    <label class="template-catalog-search">Find a template or partial<input type="search" data-template-catalog-search placeholder="Search names and where they are used…"></label>
    <div class="template-catalog-table"><table class="table"><thead><tr><th>Name</th><th>Type</th><th>Where Refine uses it</th><th>Composition</th></tr></thead><tbody>
      ${items.map(row => {
        const id = row.item.id, parents = [...new Set(edges.filter(edge => edge.to === id).map(edge => rows.get(edge.from).name))];
        const group = row.usage?.group || "workflow";
        const visible = templateCatalogView === "all" || group === templateCatalogView || (group === "delivery" && templateCatalogView !== "all");
        return `<tr data-template-catalog-row data-template-group="${htmlEscape(group)}" data-template-search="${htmlEscape(`${row.name} ${row.usage?.description || ""} ${parents.join(" ")}`.toLowerCase())}"${visible ? "" : " hidden"}>
          <td><button type="button" class="template-list-name" data-template-id="${htmlEscape(id)}">${htmlEscape(row.name)}</button>${row.customized ? '<span class="muted small">Customized</span>' : ""}</td>
          <td>${row.usage?.kind === "partial" ? "Partial" : "Template"}</td><td>${htmlEscape(row.usage?.description || "Agent prompt content.")}${parents.length ? `<p class="muted small">Included by ${htmlEscape(parents.join(", "))}</p>` : ""}</td>
          <td><button type="button" class="secondary" data-template-show-map="${htmlEscape(id)}" aria-label="Show composition of ${htmlEscape(row.name)}">Explore</button></td></tr>`;
      }).join("")}</tbody></table></div><p class="muted" data-template-catalog-empty hidden>No matching templates or partials.</p>
    </div></section>`;
}

function bindTemplateMap(data) {
  const host = document.querySelector("[data-template-map-host]");
  if (!host) return;
  host.querySelectorAll("[data-template-id]").forEach(button => { button.onclick = () => openTemplateEditor(button.dataset.templateId); });
  host.querySelectorAll("[data-map-expand]").forEach(button => { button.onclick = () => {
    const id = button.dataset.mapExpand;
    if (templateMapExpanded.has(id)) templateMapExpanded.delete(id); else templateMapExpanded.add(id);
    renderInto(host, renderTemplateMap(data), () => bindTemplateMap(data));
    host.querySelector(`[data-map-expand="${CSS.escape(id)}"]`)?.focus({preventScroll: true});
  }; });
  const graph = host.querySelector(".template-map");
  const draw = () => {
    if (!graph?.isConnected) return;
    const box = graph.getBoundingClientRect(), svg = graph.querySelector("svg");
    svg.setAttribute("width", graph.offsetWidth); svg.setAttribute("height", graph.offsetHeight);
    // Only SVG geometry is replaced; interactive controls use the shared morph contract.
    const paths = JSON.parse(graph.dataset.mapEdges).flatMap(edge => {
      const start = graph.querySelector(`[data-map-node="${CSS.escape(edge.from)}"]`)?.getBoundingClientRect();
      const end = graph.querySelector(`[data-map-node="${CSS.escape(edge.to)}"]`)?.getBoundingClientRect();
      if (!start || !end) return [];
      const x1 = start.right - box.left, y1 = start.top + start.height / 2 - box.top;
      const x2 = end.left - box.left, y2 = end.top + end.height / 2 - box.top, middle = (x1 + x2) / 2;
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", `M${x1},${y1} C${middle},${y1} ${middle},${y2} ${x2},${y2}`);
      if (edge.conditional) path.setAttribute("stroke-dasharray", "5 4");
      return [path];
    });
    svg.replaceChildren(...paths);
  };
  templateMapObserver?.disconnect();
  templateMapObserver = new ResizeObserver(draw);
  if (graph) { templateMapObserver.observe(graph); requestAnimationFrame(draw); }
}

function bindTemplatesCatalog(data) {
  const section = document.querySelector('[data-testid="settings-templates"]');
  if (!section) return;
  section.querySelectorAll("[data-template-id]").forEach(button => { button.onclick = () => openTemplateEditor(button.dataset.templateId); });
  section.querySelectorAll("[data-template-show-map]").forEach(button => { button.onclick = () => {
    templateMapRoot = button.dataset.templateShowMap;
    templateMapExpanded = new Set([templateMapRoot]);
    renderInto(section.querySelector("[data-template-map-host]"), renderTemplateMap(data), () => bindTemplateMap(data));
    section.querySelector("[data-template-map-host]").scrollIntoView({behavior: "smooth", block: "nearest"});
  }; });
  section.querySelectorAll("[data-template-view]").forEach(button => {
    const select = key => {
      templateCatalogView = key;
      templateMapRoot = {workflow: "workflow", interactive: "terminal-session", tasks: "source-upgrade", all: "workflow"}[key];
      templateMapExpanded = new Set(key === "interactive" ? [] : [templateMapRoot]);
      renderInto(document.getElementById("template-catalog-surface"), renderTemplatesCatalog(data), () => bindTemplatesCatalog(data));
      document.querySelector(`[data-template-view="${key}"]`)?.focus({preventScroll: true});
    };
    button.onclick = () => select(button.dataset.templateView);
    button.onkeydown = event => {
      const keys = Object.keys(templateCatalogViews), index = keys.indexOf(button.dataset.templateView);
      const next = event.key === "ArrowRight" ? (index + 1) % keys.length : event.key === "ArrowLeft" ? (index + keys.length - 1) % keys.length : event.key === "Home" ? 0 : event.key === "End" ? keys.length - 1 : null;
      if (next === null) return;
      event.preventDefault(); select(keys[next]);
    };
  });
  section.querySelector("[data-template-catalog-search]").oninput = event => {
    const query = event.target.value.trim().toLowerCase();
    let count = 0;
    section.querySelectorAll("[data-template-catalog-row]").forEach(row => {
      row.hidden = query ? !row.dataset.templateSearch.includes(query) : !(templateCatalogView === "all" || row.dataset.templateGroup === templateCatalogView || row.dataset.templateGroup === "delivery");
      if (!row.hidden) count++;
    });
    section.querySelector("[data-template-catalog-empty]").hidden = count > 0;
  };
  bindTemplateMap(data);
}
