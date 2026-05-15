// ---- Gaps: new --------------------------------------------------------------

async function renderGapNew() {
  // The "New Gap" screen is a modal layered over the gaps list — render the
  // list underneath so the URL #/gaps/new still has meaningful context, then
  // open the modal on top.
  await renderGapsList();
  openNewGapModal();
}

let _newGapModalOpen = false;

function openNewGapModal() {
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
      <div class="modal-title" id="new-gap-title">New Gap</div>
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

  async function submit() {
    const currentReporter = state.lastReporter || "";
    if (!currentReporter) return toast("Pick a reporter in the top-right selector", "error");
    const fd = new FormData(form);
    const actual = (fd.get("actual") || "").toString().trim();
    const target = (fd.get("target") || "").toString().trim();
    const priority = (fd.get("priority") || "low").toString();
    if (!actual && !target) return toast("Provide actual or target", "error");
    try {
      await api("POST", "/api/gaps", {
        reporter: currentReporter, actual, target, priority,
      });
      toast("Gap created", "info");
      // Stay on whatever screen the modal was layered over — Dashboard,
      // Gaps list, etc. `close(true)` only re-routes if we came in via
      // the `#/gaps/new` deep link; otherwise the underlying hash is
      // preserved so the user doesn't lose their place.
      close(true);
    } catch (err) {
      toast(err.message, "error");
    }
  }

  const firstField = root.querySelector("textarea[name='actual']");
  if (firstField) firstField.focus();
}
