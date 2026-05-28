// ---- System / Guidance ------------------------------------------------------

function renderGuidanceRows(items) {
  const rows = (items || []).map((item, idx) => renderGuidanceRow(item, idx)).join("");
  if (!rows) return `<p class="muted" data-guidance-empty>No guidance configured.</p>`;
  return `
    <table class="table guidance-table">
      <thead><tr><th>Name</th><th>Status</th><th>Rule</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>`;
}

function renderGuidanceRow(item = {}, idx = 0) {
  const enabled = item.enabled !== false;
  const statusClass = enabled ? "guidance-enabled" : "guidance-disabled";
  const statusText = enabled ? "Enabled" : "Disabled";
  return `
    <tr class="guidance-table-row" data-guidance-row data-guidance-open="${idx}"
        role="button" tabindex="0">
      <td>${htmlEscape(item.name || "Untitled guidance")}</td>
      <td><span class="status-pill ${statusClass}">${statusText}</span></td>
      <td class="muted small">${htmlEscape(item.rule || "No rule provided.")}</td>
    </tr>`;
}

let _guidanceModalOpen = false;

function openGuidanceModal(items, index = null, refreshTab = "guidance") {
  if (_guidanceModalOpen) return;
  _guidanceModalOpen = true;
  const editing = Number.isInteger(index);
  const current = editing ? (items[index] || {}) : {};
  let guidanceEnabled = current.enabled !== false;
  const root = document.createElement("div");
  root.className = "modal-backdrop";
  root.innerHTML = `
    <div class="modal" role="dialog" aria-modal="true"
         aria-labelledby="guidance-modal-title" style="max-width:680px">
      <div class="modal-title" id="guidance-modal-title">${editing ? "Edit guidance" : "New guidance"}</div>
      <div class="modal-body" style="max-height:70vh;overflow:auto">
        <form id="guidance-form">
          <div class="form-row">
            <label>Name</label>
            <input type="text" name="name"
                   value="${htmlEscape(current.name || "")}"
                   placeholder="e.g. Frontend accessibility">
          </div>
          <div class="form-row">
            <label>Rule</label>
            <textarea name="rule" rows="4"
                      placeholder="When should this guidance apply?">${htmlEscape(current.rule || "")}</textarea>
          </div>
          <div class="form-row">
            <label>Instructions</label>
            <textarea name="instructions" rows="8"
                      placeholder="What additional context should the agent receive?">${htmlEscape(current.instructions || "")}</textarea>
          </div>
          <div class="form-row guidance-status-row">
            <label>Status</label>
            <div class="guidance-status-control">
              <span class="status-pill ${guidanceEnabled ? "guidance-enabled" : "guidance-disabled"}" data-enabled-status>
                ${guidanceEnabled ? "Enabled" : "Disabled"}
              </span>
              <button class="secondary" type="button" data-toggle-enabled>
                ${guidanceEnabled ? "Disable guidance" : "Enable guidance"}
              </button>
            </div>
          </div>
        </form>
      </div>
      <div class="modal-actions">
        ${editing ? '<button class="danger" type="button" data-delete>Delete guidance</button><span class="spacer"></span>' : ""}
        <button class="secondary" type="button" data-cancel>Cancel</button>
        <button type="button" data-ok>${editing ? "Save guidance" : "Create guidance"}</button>
      </div>
    </div>`;
  document.body.appendChild(root);

  let closed = false;
  function close() {
    if (closed) return;
    closed = true;
    _guidanceModalOpen = false;
    document.removeEventListener("keydown", onKey, true);
    root.remove();
  }
  function onKey(e) {
    if (!root.contains(e.target)) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (
      e.key === "Enter"
      && e.target?.tagName !== "TEXTAREA"
      && !e.target?.closest("button")
    ) {
      e.preventDefault();
      submit();
    }
  }
  async function submit() {
    const form = root.querySelector("#guidance-form");
    const fd = new FormData(form);
    const item = {
      name: (fd.get("name") || "").toString().trim(),
      rule: (fd.get("rule") || "").toString().trim(),
      instructions: (fd.get("instructions") || "").toString().trim(),
      enabled: guidanceEnabled,
    };
    if (!item.name || !item.rule || !item.instructions) {
      toast("Name, rule, and instructions are required", "error");
      return;
    }
    const next = [...items];
    if (editing) next[index] = item;
    else next.push(item);
    await withButtonBusy(root.querySelector("[data-ok]"), "Saving…", async () => {
      try {
        await api("PUT", "/api/guidance", { guidance: next });
        toast(editing ? "Guidance saved" : "Guidance created", "info");
        close();
        await refreshSettingsTab(refreshTab, { force: true });
      } catch (e) { await showActionError(e); }
    });
  }
  async function remove() {
    if (!editing) return;
    const ok = await modalConfirm(
      `Delete guidance "${current.name || "Untitled guidance"}"?`,
      { title: "Delete guidance", okLabel: "Delete", danger: true },
    );
    if (!ok) return;
    const next = items.filter((_item, idx) => idx !== index);
    await withButtonBusy(root.querySelector("[data-delete]"), "Deleting…", async () => {
      try {
        await api("PUT", "/api/guidance", { guidance: next });
        toast("Guidance deleted", "info");
        close();
        await refreshSettingsTab(refreshTab, { force: true });
      } catch (e) { await showActionError(e); }
    });
  }

  document.addEventListener("keydown", onKey, true);
  function updateGuidanceEnabled() {
    const status = root.querySelector("[data-enabled-status]");
    const toggle = root.querySelector("[data-toggle-enabled]");
    if (status) {
      status.textContent = guidanceEnabled ? "Enabled" : "Disabled";
      status.className = `status-pill ${guidanceEnabled ? "guidance-enabled" : "guidance-disabled"}`;
    }
    if (toggle) {
      toggle.textContent = guidanceEnabled ? "Disable guidance" : "Enable guidance";
    }
  }
  root.addEventListener("click", (e) => {
    if (e.target === root) close();
  });
  root.querySelector("[data-cancel]").addEventListener("click", close);
  root.querySelector("[data-ok]").addEventListener("click", submit);
  root.querySelector("[data-delete]")?.addEventListener("click", remove);
  root.querySelector("[data-toggle-enabled]")?.addEventListener("click", () => {
    guidanceEnabled = !guidanceEnabled;
    updateGuidanceEnabled();
  });
  root.querySelector("[name='name']")?.focus();
}


function renderSettingsGuidanceTab(guidanceItems) {
  return `
    <section class="settings-section">
      <h3>Guidance</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Guidance is classified against each Gap before work starts. Accepted
        guidance instructions are prepended to the agent's work prompt.
      </p>
      <div id="guidance-list">
        ${renderGuidanceRows(guidanceItems)}
      </div>
      <div class="actions" style="margin-top:10px">
        <button class="secondary" id="guidance-add">Add guidance</button>
      </div>
    </section>`;
}

function bindSettingsGuidanceTab(guidanceItems, refreshTab = "guidance") {
  $("#guidance-list")?.addEventListener("click", (e) => {
    const row = e.target.closest("[data-guidance-open]");
    if (row) openGuidanceModal(guidanceItems, Number(row.dataset.guidanceOpen), refreshTab);
  });
  $("#guidance-list")?.addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const row = e.target.closest("[data-guidance-open]");
    if (!row) return;
    e.preventDefault();
    openGuidanceModal(guidanceItems, Number(row.dataset.guidanceOpen), refreshTab);
  });
  $("#guidance-add")?.addEventListener("click", () => {
    openGuidanceModal(guidanceItems, null, refreshTab);
  });
}
