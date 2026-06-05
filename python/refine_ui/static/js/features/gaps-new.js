// ---- Gaps: new --------------------------------------------------------------

async function renderGapNew() {
  // The "New Gap" screen is a modal layered over the gaps list — render the
  // list underneath so the URL #/gaps/new still has meaningful context, then
  // open the modal on top.
  await renderGapsList();
  openNewGapModal();
}

let _newGapModalOpen = false;

function openNewGapModal(options = {}) {
  if (_newGapModalOpen) return;
  const reporter = state.lastReporter || "";
  if (!reporter) {
    toast("Pick a reporter in the top-right selector first", "error");
    return;
  }
  _newGapModalOpen = true;

  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="new-gap-title" style="max-width:560px">
      <div class="modal-title" id="new-gap-title">${options.featureId ? "New Feature Gap" : "New Gap"}</div>
      <div class="modal-body">
        <div class="muted small" style="margin-bottom:8px">
          Submitting as <strong class="js-reporter-name">${htmlEscape(reporter)}</strong>
          — change in the top-right reporter selector.
        </div>
        <form id="new-gap-form">
          <div class="form-row">
            <label>Actual (current behavior)</label>
            <textarea name="actual" placeholder="What's happening today?"></textarea>
          </div>
          <div class="form-row">
            <label>Target (desired behavior)</label>
            <textarea name="target" placeholder="What should be happening?"></textarea>
          </div>
          <div class="form-row">
            <label>Priority</label>
            <select name="priority">
              <option value="low" selected>Low (default)</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </div>
          <p class="muted small">
            A name will be auto-generated from the text above — you can rename
            the Gap on its detail page afterwards. High-priority Gaps run before
            medium, and medium before low.
          </p>
        </form>
      </div>
      <div class="modal-actions">
        <button class="secondary" data-cancel>Cancel</button>
        <button data-ok>Create Gap</button>
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
    _newGapModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
    // If the modal was opened via the #/gaps/new route, send the user back
    // to the gaps list when they dismiss it (so the URL no longer points at
    // a "screen" that no longer exists).
    if (navigateAway && location.hash.startsWith("#/gaps/new")) {
      location.hash = "#/gaps";
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

  const form = root.querySelector("#new-gap-form");
  form.addEventListener("submit", (e) => { e.preventDefault(); submit(); });
  $$("#new-gap-form textarea[name='actual'], #new-gap-form textarea[name='target']", root).forEach((field) => {
    field.addEventListener("input", () => {
      duplicateDecision = "";
      duplicateDecisionKey = "";
      root.querySelector("#new-gap-duplicate")?.remove();
      const ok = root.querySelector("[data-ok]");
      if (ok) ok.textContent = "Create Gap";
    });
  });

  async function submit() {
    const currentReporter = state.lastReporter || "";
    if (!currentReporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    const priority = (fd.get("priority") || "low").toString();
    if (!actual && !target) return toast("Provide actual or target", "error");
    const duplicateKey = `${actual}\n${target}`;
    const effectiveDuplicateDecision = (
      duplicateDecision && duplicateDecisionKey === duplicateKey
    ) ? duplicateDecision : "";
    try {
      const r = await api("POST", "/api/gaps", {
        reporter: currentReporter, actual, target, priority,
        ...(options.featureId ? { feature_id: options.featureId } : {}),
        duplicate_decision: effectiveDuplicateDecision,
      });
      if (r?.created === false) {
        const move = r.move || {};
        if (r.duplicate_action === "move_original_to_backlog") {
          if (move.moved) {
            toast("Original Gap moved to backlog; duplicate not created", "info");
          } else if (move.reason === "protected_status") {
            toast(`Original Gap is ${move.from}; duplicate not created`, "info");
          } else if (move.reason === "already_backlog") {
            toast("Original Gap is already in backlog; duplicate not created", "info");
          } else {
            toast("Duplicate not created", "info");
          }
        } else {
          toast("Duplicate not created", "info");
        }
        close(true);
        return;
      }
      toast("Gap created", "info");
      if (typeof options.onSaved === "function") {
        await options.onSaved(r);
      }
      // Stay on whatever screen the modal was layered over — Dashboard,
      // Gaps list, etc. `close(true)` only re-routes if we came in via
      // the `#/gaps/new` deep link; otherwise the underlying hash is
      // preserved so the user doesn't lose their place.
      close(true);
    } catch (err) {
      if (err.code === "duplicate_gap" && err.error?.duplicate?.match) {
        duplicateDecision = "";
        duplicateDecisionKey = duplicateKey;
        const ok = root.querySelector("[data-ok]");
        if (ok) ok.textContent = "Move original to backlog";
        drawNewGapDuplicatePrompt(root, err.error.duplicate.match, {
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

  const firstField = root.querySelector("textarea[name='actual']");
  if (firstField) firstField.focus();
}

function drawNewGapDuplicatePrompt(root, match, {
  onIgnore,
  onImport,
  onMoveOriginal,
}) {
  let prompt = root.querySelector("#new-gap-duplicate");
  if (!prompt) {
    prompt = document.createElement("div");
    prompt.id = "new-gap-duplicate";
    const form = root.querySelector("#new-gap-form");
    form?.prepend(prompt);
  }
  prompt.innerHTML = renderGapDuplicatePrompt(match);
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

function renderGapDuplicatePrompt(match) {
  return `
    <div class="import-duplicate">
      <div class="small" style="font-weight:600">Possible duplicate</div>
      <p class="muted small" style="margin:4px 0">
        ${htmlEscape(match.name || match.id)} · ${htmlEscape(match.node_display_name || match.node_id || "Default")}
        · ${htmlEscape(match.status || "")}
      </p>
      <div class="import-duplicate-content">
        <div>
          <div class="small muted">Matched actual</div>
          <p>${htmlEscape(match.actual || "")}</p>
        </div>
        <div>
          <div class="small muted">Matched target</div>
          <p>${htmlEscape(match.target || "")}</p>
        </div>
      </div>
      <div class="actions import-duplicate-actions">
        <button type="button" data-duplicate-decision="move_original_to_backlog" class="selected">Yes, move original to backlog</button>
        <button type="button" class="secondary" data-duplicate-decision="duplicate">Yes, ignore</button>
        <button type="button" class="secondary" data-duplicate-decision="original">No, import</button>
      </div>
    </div>`;
}
