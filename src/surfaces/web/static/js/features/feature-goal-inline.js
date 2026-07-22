// ---- Feature Goal inline authoring ------------------------------------------

const FEATURE_GOAL_EDITABLE_STATUSES = new Set(["backlog", "todo"]);
const FEATURE_GOAL_PLACEMENT_UNORDERED = "unordered";
const FEATURE_GOAL_PLACEMENT_FIRST = "first";

function featureGoalCanInlineEdit(goal) {
  return FEATURE_GOAL_EDITABLE_STATUSES.has(goal?.status || "");
}

function featureGoalLatestPrompt(goal) {
  const rounds = goal?.rounds || [];
  return rounds.length ? (rounds[rounds.length - 1]?.prompt || "") : "";
}

function featureGoalPlacementValue(goal, goals) {
  const order = Number(goal?.feature_order || 0);
  if (order < 1) return FEATURE_GOAL_PLACEMENT_UNORDERED;
  const ordered = (goals || [])
    .filter((candidate) => Number(candidate?.feature_order || 0) > 0)
    .slice()
    .sort((a, b) => Number(a.feature_order) - Number(b.feature_order));
  const index = ordered.findIndex((candidate) => candidate.id === goal.id);
  if (index <= 0) return FEATURE_GOAL_PLACEMENT_FIRST;
  return ordered[index - 1].id;
}

function renderFeatureGoalPlacementOptions(goals, editingGoal = null) {
  const selected = editingGoal
    ? featureGoalPlacementValue(editingGoal, goals)
    : FEATURE_GOAL_PLACEMENT_UNORDERED;
  const ordered = (goals || [])
    .filter((goal) => goal.id !== editingGoal?.id && Number(goal?.feature_order || 0) > 0)
    .slice()
    .sort((a, b) => Number(a.feature_order) - Number(b.feature_order));
  const option = (value, label) => `
    <option value="${htmlEscape(value)}" ${selected === value ? "selected" : ""}>${htmlEscape(label)}</option>`;
  return [
    option(FEATURE_GOAL_PLACEMENT_UNORDERED, "Independent (not ordered)"),
    option(FEATURE_GOAL_PLACEMENT_FIRST, "First in sequence"),
    ...ordered.map((goal) => option(goal.id, `After ${goal.name || goal.id}`)),
  ].join("");
}

function renderFeatureGoalInlineComposer(goals, reporter = "") {
  return `
    <section class="feature-goal-composer" data-feature-goal-composer data-mode="create"
             aria-labelledby="feature-goal-composer-title" data-testid="feature-goal-composer">
      <div class="feature-goal-composer-head">
        <div>
          <h3 id="feature-goal-composer-title" data-feature-composer-title>Add a Goal</h3>
          <p class="muted small" data-feature-composer-context>
            Create it here without leaving this Feature. The name is generated from the prompt unless you provide one.
          </p>
        </div>
        <button type="button" class="secondary small" data-feature-composer-reset
                data-testid="feature-goal-composer-reset" hidden>New Goal</button>
      </div>
      <form data-feature-goal-form data-testid="feature-goal-form" novalidate>
        <input type="hidden" name="goal_id" value="">
        <div class="muted small feature-goal-composer-reporter">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter || "none selected")}</strong>
          — change in the top-right reporter selector.
        </div>
        <div class="feature-goal-composer-fields">
          <div class="form-row feature-goal-prompt-field">
            <label for="feature-goal-prompt">Prompt <span aria-hidden="true">*</span></label>
            <textarea id="feature-goal-prompt" name="prompt" rows="4" required
                      aria-describedby="feature-goal-prompt-help feature-goal-form-status"
                      data-testid="feature-goal-prompt"
                      placeholder="Describe what the agent should accomplish in this Feature."></textarea>
            <span id="feature-goal-prompt-help" class="muted small">Required. Ctrl/⌘ + Enter saves.</span>
          </div>
          <div class="feature-goal-composer-side">
            <div class="form-row">
              <label for="feature-goal-name">Name</label>
              <input id="feature-goal-name" name="name" type="text" maxlength="80"
                     data-testid="feature-goal-name" placeholder="Generated from prompt">
            </div>
            <div class="feature-goal-composer-pair">
              <div class="form-row">
                <label for="feature-goal-priority">Priority</label>
                <select id="feature-goal-priority" name="priority" data-testid="feature-goal-priority">
                  <option value="low">Low</option>
                  <option value="medium">Medium</option>
                  <option value="high">High</option>
                </select>
              </div>
              <div class="form-row">
                <label for="feature-goal-placement">Sequence / dependency</label>
                <select id="feature-goal-placement" name="placement"
                        aria-describedby="feature-goal-placement-help"
                        data-testid="feature-goal-placement">
                  ${renderFeatureGoalPlacementOptions(goals)}
                </select>
              </div>
            </div>
            <span id="feature-goal-placement-help" class="muted small">
              Ordered Goals run top to bottom; “After” names the prerequisite.
            </span>
          </div>
        </div>
        <div data-feature-goal-duplicate></div>
        <div class="feature-goal-composer-actions">
          <span id="feature-goal-form-status" class="small" role="status" aria-live="polite"
                data-feature-goal-form-status data-testid="feature-goal-form-status"></span>
          <span class="spacer"></span>
          <button type="button" class="secondary" data-feature-composer-cancel
                  data-testid="feature-goal-composer-cancel" hidden>Cancel edit</button>
          <button type="submit" data-feature-composer-submit data-testid="feature-goal-submit">Create Goal</button>
        </div>
      </form>
    </section>`;
}

async function applyFeatureGoalPlacement(featureId, goal, placement, request = api) {
  const ordered = Number(goal?.feature_order || 0) > 0;
  const base = `/api/features/${encodeURIComponent(featureId)}/goals/${encodeURIComponent(goal.id)}`;
  if (placement === FEATURE_GOAL_PLACEMENT_UNORDERED) {
    if (ordered) await request("POST", `${base}/unorder`);
    return;
  }
  if (!ordered) await request("POST", `${base}/order`);
  const body = placement === FEATURE_GOAL_PLACEMENT_FIRST
    ? { order: 1 }
    : { after: placement };
  await request("POST", `${base}/reorder`, body);
}

async function saveFeatureGoalInline(featureId, editingGoal, fields, request = api) {
  const { reporter, prompt, name, priority, duplicateDecision = "" } = fields;
  if (!editingGoal) {
    return request("POST", "/api/goals", {
      reporter, prompt, priority, feature_id: featureId,
      ...(name ? { name } : {}),
      ...(duplicateDecision ? { duplicate_decision: duplicateDecision } : {}),
    });
  }
  const metadata = await request("PATCH", `/api/goals/${encodeURIComponent(editingGoal.id)}`, {
    name, priority,
  });
  const roundMethod = (editingGoal.rounds || []).length ? "PATCH" : "POST";
  const roundPath = roundMethod === "PATCH"
    ? `/api/goals/${encodeURIComponent(editingGoal.id)}/rounds/latest`
    : `/api/goals/${encodeURIComponent(editingGoal.id)}/rounds`;
  await request(roundMethod, roundPath, {
    reporter,
    assignee: editingGoal.assignee || reporter,
    prompt,
  });
  return { goal: { ...editingGoal, ...(metadata.goal || {}), id: editingGoal.id } };
}

function bindFeatureGoalInlineComposer(root, feature, { goalPage = 1, navigateAway = false } = {}) {
  const composer = root.querySelector("[data-feature-goal-composer]");
  const form = composer?.querySelector("[data-feature-goal-form]");
  if (!composer || !form) return;
  const title = composer.querySelector("[data-feature-composer-title]");
  const context = composer.querySelector("[data-feature-composer-context]");
  const resetButton = composer.querySelector("[data-feature-composer-reset]");
  const cancelButton = composer.querySelector("[data-feature-composer-cancel]");
  const submitButton = composer.querySelector("[data-feature-composer-submit]");
  const status = composer.querySelector("[data-feature-goal-form-status]");
  const duplicateRoot = composer.querySelector("[data-feature-goal-duplicate]");
  let editingGoal = null;
  let duplicateDecision = "";
  let duplicateDecisionKey = "";

  const setStatus = (message, kind = "") => {
    status.textContent = message || "";
    status.className = `small feature-goal-form-status ${kind}`.trim();
  };
  const resetDuplicate = () => {
    duplicateDecision = "";
    duplicateDecisionKey = "";
    duplicateRoot.innerHTML = "";
    submitButton.textContent = editingGoal ? "Save Goal" : "Create Goal";
  };
  const reset = ({ focus = false } = {}) => {
    editingGoal = null;
    composer.dataset.mode = "create";
    form.reset();
    form.elements.goal_id.value = "";
    form.elements.placement.innerHTML = renderFeatureGoalPlacementOptions(feature.goals || []);
    title.textContent = "Add a Goal";
    context.textContent = "Create it here without leaving this Feature. The name is generated from the prompt unless you provide one.";
    resetButton.hidden = true;
    cancelButton.hidden = true;
    setStatus("");
    resetDuplicate();
    if (focus) form.elements.prompt.focus();
  };
  const showDuplicate = (match) => {
    duplicateRoot.innerHTML = renderGoalDuplicatePrompt(match);
    duplicateRoot.querySelector('[data-duplicate-decision="duplicate"]')
      ?.addEventListener("click", () => {
        duplicateDecision = "duplicate";
        duplicateDecisionKey = form.elements.prompt.value.trim();
        form.requestSubmit();
      });
    duplicateRoot.querySelector('[data-duplicate-decision="original"]')
      ?.addEventListener("click", () => {
        duplicateDecision = "original";
        duplicateDecisionKey = form.elements.prompt.value.trim();
        duplicateRoot.querySelectorAll("[data-duplicate-decision]").forEach((button) => {
          button.classList.toggle("selected", button.dataset.duplicateDecision === "original");
        });
        submitButton.textContent = "Create anyway";
      });
    duplicateRoot.querySelector('[data-duplicate-decision="move_original_to_backlog"]')
      ?.addEventListener("click", () => {
        duplicateDecision = "move_original_to_backlog";
        duplicateDecisionKey = form.elements.prompt.value.trim();
        form.requestSubmit();
      });
  };
  const reload = async ({ focusComposer = false } = {}) => {
    const data = await api("GET", `/api/features/${encodeURIComponent(feature.id)}`, undefined, { cache: false });
    openFeatureModal(data.feature, { goalPage, navigateAway, focusComposer });
  };
  const startEdit = async (goalId) => {
    setStatus("Loading Goal…");
    try {
      const result = await api("GET", `/api/goals/${encodeURIComponent(goalId)}`, undefined, { cache: false });
      const goal = result.goal;
      if (!featureGoalCanInlineEdit(goal)) {
        setStatus(`${workflowStatusLabel(goal.status)} Goals cannot be edited.`, "error");
        return;
      }
      editingGoal = goal;
      composer.dataset.mode = "edit";
      form.elements.goal_id.value = goal.id;
      form.elements.prompt.value = featureGoalLatestPrompt(goal);
      form.elements.name.value = goal.name || "";
      form.elements.priority.value = goal.priority || "low";
      form.elements.placement.innerHTML = renderFeatureGoalPlacementOptions(feature.goals || [], goal);
      title.textContent = `Edit ${goal.name || goal.id}`;
      context.textContent = `Editing ${goal.id} in this Feature. Only backlog and to-do Goals are editable.`;
      resetButton.hidden = false;
      cancelButton.hidden = false;
      resetDuplicate();
      setStatus("");
      form.elements.prompt.focus();
      composer.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
    } catch (error) {
      setStatus(error.message || "Could not load Goal", "error");
      await showActionError(error, "Could not edit Goal");
    }
  };

  root.querySelectorAll("[data-feature-edit-goal]").forEach((button) => {
    button.addEventListener("click", () => startEdit(button.dataset.featureEditGoal));
  });
  resetButton.addEventListener("click", () => reset({ focus: true }));
  cancelButton.addEventListener("click", () => reset({ focus: true }));
  form.elements.prompt.addEventListener("input", resetDuplicate);
  form.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      form.requestSubmit();
    }
  });
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const reporter = state.lastReporter || "";
    const prompt = form.elements.prompt.value.trim();
    const name = form.elements.name.value.trim();
    const priority = form.elements.priority.value;
    const placement = form.elements.placement.value;
    if (!reporter) {
      setStatus("Pick a reporter in the top-right selector first.", "error");
      form.elements.prompt.focus();
      return;
    }
    if (!prompt) {
      setStatus("Prompt is required.", "error");
      form.elements.prompt.focus();
      return;
    }
    if (editingGoal && !name) {
      setStatus("Name is required when editing.", "error");
      form.elements.name.focus();
      return;
    }
    setStatus(editingGoal ? "Saving…" : "Creating…");
    submitButton.disabled = true;
    try {
      const duplicateKey = prompt;
      const decision = duplicateDecision && duplicateDecisionKey === duplicateKey
        ? duplicateDecision
        : "";
      const saved = await saveFeatureGoalInline(feature.id, editingGoal, {
        reporter, prompt, name, priority, duplicateDecision: decision,
      });
      if (saved?.created === false) {
          const moved = saved.duplicate_action === "move_original_to_backlog" && saved.move?.moved;
          toast(moved ? "Original Goal moved to backlog; duplicate not created" : "Duplicate not created", "info");
          await reload({ focusComposer: true });
          return;
      }
      const savedGoal = saved.goal;
      await applyFeatureGoalPlacement(feature.id, savedGoal, placement);
      toast(editingGoal ? "Goal updated" : "Goal created", "success");
      await reload({ focusComposer: true });
    } catch (error) {
      if (!editingGoal && error.code === "duplicate_goal" && error.error?.duplicate?.match) {
        duplicateDecision = "move_original_to_backlog";
        duplicateDecisionKey = prompt;
        submitButton.textContent = "Move original to backlog";
        showDuplicate(error.error.duplicate.match);
        setStatus("Choose how to handle the possible duplicate.", "error");
      } else {
        setStatus(error.message || "Goal could not be saved.", "error");
        await showActionError(error, editingGoal ? "Goal update failed" : "Goal creation failed");
      }
    } finally {
      if (root.isConnected) submitButton.disabled = false;
    }
  });

  root._featureComposerReset = reset;
  root._featureComposerHasDraft = () => !!(
    editingGoal
    || form.elements.prompt.value.trim()
    || form.elements.name.value.trim()
    || form.elements.priority.value !== "low"
    || form.elements.placement.value !== FEATURE_GOAL_PLACEMENT_UNORDERED
  );
  if (root.querySelector(".feature-modal") && root.isConnected) {
    reset({ focus: false });
  }
}
