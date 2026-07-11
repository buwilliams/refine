// ---- Goals: new --------------------------------------------------------------

async function renderGoalNew() {
  // The "New Goal" screen is a modal layered over the goals list — render the
  // list underneath so the URL #/goals/new still has meaningful context, then
  // open the modal on top.
  await renderGoalsList();
  openNewGoalModal();
}

let _newGoalModalOpen = false;

function openNewGoalModal(options = {}) {
  if (_newGoalModalOpen) return;
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  _newGoalModalOpen = true;

  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="new-goal-title" data-testid="new-goal-modal" style="max-width:560px">
      <div class="modal-title" id="new-goal-title">${options.featureId ? "New Feature Goal" : "New Goal"}</div>
      <div class="modal-body">
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
          — change in the top-right reporter selector.
        </div>
        <form id="new-goal-form">
          <div class="form-row">
            <label>Prompt</label>
            <textarea name="prompt" data-testid="new-goal-prompt" placeholder="Describe what the agent should accomplish."></textarea>
          </div>
          <div class="form-row">
            <label>Priority</label>
            <select name="priority" data-testid="new-goal-priority">
              <option value="low" selected>Low (default)</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </div>
          <p class="muted small">
            A name will be auto-generated from the text above — you can rename
            the Goal on its detail page afterwards. High-priority Goals run before
            medium, and medium before low.
          </p>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel data-testid="new-goal-cancel">Cancel</button>
        <button data-ok data-testid="new-goal-submit">Create Goal</button>
      </div>
    </div>
  `;
  document.body.appendChild(root);

  let closed = false;
  let duplicateDecision = "";
  let duplicateDecisionKey = "";
  function close(navigateAway) {
    if (closed) return;
    closed = true;
    _newGoalModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    // If the modal was opened via the #/goals/new route, send the user back
    // to the goals list when they dismiss it (so the URL no longer points at
    // a "screen" that no longer exists).
    if (navigateAway && location.hash.startsWith("#/goals/new")) {
      location.hash = "#/goals";
    }
  }
  function onKey(e) {
    if (e.key === "Escape") {
      e.preventDefault();
      close(true);
    } else if (e.key === "Enter") {
      // Allow Enter inside textareas to insert newlines.
      if (e.target && e.target.tagName === "TEXTAREA") return;
      e.preventDefault();
      submit();
    }
  }
  document.addEventListener("keydown", onKey, true);
  root.addEventListener("click", (e) => {
    if (e.target === root) close(true);
  });
  root.querySelector("[data-cancel]").addEventListener("click", () => close(true));
  root.querySelector("[data-ok]").addEventListener("click", submit);

  const form = root.querySelector("#new-goal-form");
  form.addEventListener("submit", (e) => { e.preventDefault(); submit(); });
  $$("#new-goal-form textarea[name='prompt']", root).forEach((field) => {
    field.addEventListener("input", () => {
      duplicateDecision = "";
      duplicateDecisionKey = "";
      root.querySelector("#new-goal-duplicate")?.remove();
      const ok = root.querySelector("[data-ok]");
      if (ok) ok.textContent = "Create Goal";
    });
  });

  async function submit() {
    const currentReporter = state.lastReporter || "";
    if (!currentReporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const prompt = (fd.get("prompt") || "").toString().trim();
    const priority = (fd.get("priority") || "low").toString();
    if (!prompt) return toast("Provide a prompt", "error");
    const duplicateKey = prompt;
    const effectiveDuplicateDecision = (
      duplicateDecision && duplicateDecisionKey === duplicateKey
    ) ? duplicateDecision : "";
    try {
      const r = await api("POST", "/api/goals", {
        reporter: currentReporter, prompt, priority,
        ...(options.featureId ? { feature_id: options.featureId } : {}),
        duplicate_decision: effectiveDuplicateDecision,
      });
      if (r?.created === false) {
        const move = r.move || {};
        if (r.duplicate_action === "move_original_to_backlog") {
          if (move.moved) {
            toast("Original Goal moved to backlog; duplicate not created", "info");
          } else if (move.reason === "protected_status") {
            toast(`Original Goal is ${move.from}; duplicate not created`, "info");
          } else if (move.reason === "already_backlog") {
            toast("Original Goal is already in backlog; duplicate not created", "info");
          } else {
            toast("Duplicate not created", "info");
          }
        } else {
          toast("Duplicate not created", "info");
        }
        close(true);
        return;
      }
      toast("Goal created", "info");
      if (typeof options.onSaved === "function") {
        await options.onSaved(r);
      }
      // Stay on whatever screen the modal was layered over — Dashboard,
      // Goals list, etc. `close(true)` only re-routes if we came in via
      // the `#/goals/new` deep link; otherwise the underlying hash is
      // preserved so the user doesn't lose their place.
      close(true);
    } catch (err) {
      if (err.code === "duplicate_goal" && err.error?.duplicate?.match) {
        duplicateDecision = "";
        duplicateDecisionKey = duplicateKey;
        const ok = root.querySelector("[data-ok]");
        if (ok) ok.textContent = "Move original to backlog";
        drawNewGoalDuplicatePrompt(root, err.error.duplicate.match, {
          onIgnore: () => {
            duplicateDecision = "duplicate";
            duplicateDecisionKey = duplicateKey;
            submit();
          },
          onImport: () => {
            duplicateDecision = "original";
            duplicateDecisionKey = duplicateKey;
            const ok = root.querySelector("[data-ok]");
            if (ok) ok.textContent = "Create anyway";
          },
          onMoveOriginal: () => {
            duplicateDecision = "move_original_to_backlog";
            duplicateDecisionKey = duplicateKey;
            submit();
          },
        });
        duplicateDecision = "move_original_to_backlog";
        return;
      }
      toast(err.message, "error");
    }
  }

  const firstField = root.querySelector("textarea[name='prompt']");
  if (firstField) firstField.focus();
}

function drawNewGoalDuplicatePrompt(root, match, {
  onIgnore,
  onImport,
  onMoveOriginal,
}) {
  let prompt = root.querySelector("#new-goal-duplicate");
  if (!prompt) {
    prompt = document.createElement("div");
    prompt.id = "new-goal-duplicate";
    const form = root.querySelector("#new-goal-form");
    form?.prepend(prompt);
  }
  prompt.innerHTML = renderGoalDuplicatePrompt(match);
  prompt.querySelector('[data-duplicate-decision="duplicate"]')
    ?.addEventListener("click", onIgnore);
  prompt.querySelector('[data-duplicate-decision="original"]')
    ?.addEventListener("click", () => {
      prompt.querySelectorAll("[data-duplicate-decision]").forEach((btn) => {
        btn.classList.toggle(
          "selected",
          btn.dataset.duplicateDecision === "original",
        );
      });
      onImport();
    });
  prompt.querySelector('[data-duplicate-decision="move_original_to_backlog"]')
    ?.addEventListener("click", onMoveOriginal);
}

function renderGoalDuplicatePrompt(match) {
  return `
    <div class="import-duplicate" data-testid="new-goal-duplicate">
      <div class="small" style="font-weight:600">Possible duplicate</div>
      <p class="muted small" style="margin:4px 0">
        ${htmlEscape(match.name || match.id)} · ${htmlEscape(match.node_display_name || match.node_id || "Default")}
        · ${htmlEscape(match.status || "")}
      </p>
      <div class="import-duplicate-content">
        <div>
          <div class="small muted">Matched prompt</div>
          <p>${htmlEscape(match.prompt || "")}</p>
        </div>
      </div>
      <div class="actions import-duplicate-actions">
        <button type="button" data-duplicate-decision="move_original_to_backlog" data-testid="new-goal-duplicate-move" class="selected">Yes, move original to backlog</button>
        <button type="button" class="secondary" data-duplicate-decision="duplicate" data-testid="new-goal-duplicate-ignore">Yes, ignore</button>
        <button type="button" class="secondary" data-duplicate-decision="original" data-testid="new-goal-duplicate-import">No, import</button>
      </div>
    </div>`;
}
