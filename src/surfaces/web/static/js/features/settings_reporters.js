// ---- System / Reporters -----------------------------------------------------

function renderSettingsReportersTab(reps, activeNodeLabel) {
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Reporters", "reporter-manage")}</h3>
      <p class="scope-label muted small">Node: ${htmlEscape(activeNodeLabel)}</p>
      <table class="table" data-testid="reporters-table">
        <thead><tr><th>Name</th><th></th></tr></thead>
        <tbody>
          ${reps.map((r) => `<tr data-testid="reporter-row" data-reporter-id="${htmlEscape(r.id)}" data-reporter-name="${htmlEscape(r.name)}">
            <td data-testid="reporter-name">${htmlEscape(r.name)}</td>
            <td class="actions">
              <button class="secondary" data-testid="reporter-rename" data-rename="${r.id}" data-name="${htmlEscape(r.name)}">Rename</button>
              <button class="secondary" data-testid="reporter-merge" data-rmerge="${r.id}" data-name="${htmlEscape(r.name)}" ${reps.length > 1 ? "" : "disabled"}>Merge</button>
              <button class="danger" data-testid="reporter-remove" data-rdel="${r.id}">Remove</button>
            </td>
          </tr>`).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="r-add" data-testid="reporter-add">+ Add reporter</button>
      </div>
      <p class="muted small" style="margin-top:6px">
        Renaming a reporter cascades through every Gap's rounds so historical
        data stays in sync. Merging a reporter moves its Gaps to another
        reporter and removes the old reporter from the dropdown. Removing a
        reporter only affects the dropdown — historical rounds keep their
        original reporter string so audit history is preserved.
      </p>
    </section>`;
}

function bindSettingsReportersTab() {
  $$("[data-rmerge]").forEach((b) => b.addEventListener("click", async () => {
    const oldName = b.dataset.name;
    const targetId = await openReporterMergeModal({
      id: b.dataset.rmerge,
      name: oldName,
    });
    if (!targetId) return;
    const target = (state.reporters || []).find((r) => String(r.id) === String(targetId));
    await withButtonBusy(b, "Merging...", async () => {
      try {
        const r = await api("POST", `/api/reporters/${b.dataset.rmerge}/merge`, {
          target_id: Number(targetId),
        });
        const newName = r.new || target?.name || "";
        if (state.lastReporter === oldName && newName) setLastReporter(newName);
        await refreshReporters();
        await refreshCurrentSettingsSurface({ force: true });
        toast(`Merged ${oldName} into ${newName || "selected reporter"}`, "info");
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-rdel]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Remove this reporter from the dropdown? Historical rounds keep their original reporter string.",
      { title: "Remove reporter", okLabel: "Remove", danger: true },
    );
    if (!ok) return;
    await withButtonBusy(b, "Removing...", async () => {
      try { await api("DELETE", "/api/reporters/" + b.dataset.rdel); await refreshCurrentSettingsSurface({ force: true }); }
      catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-rename]").forEach((b) => b.addEventListener("click", async () => {
    const oldName = b.dataset.name;
    const name = await modalPrompt("New name", oldName,
                                   { title: "Rename reporter" });
    if (!name || !name.trim()) return;
    const newName = name.trim();
    await withButtonBusy(b, "Renaming...", async () => {
      try {
        await api("PATCH", "/api/reporters/" + b.dataset.rename, { name: newName });
        if (state.lastReporter === oldName) setLastReporter(newName);
        await refreshReporters();
        await refreshCurrentSettingsSurface({ force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $("#r-add").addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const name = await modalPrompt("Reporter name", "",
                                   { title: "Add reporter" });
    if (!name || !name.trim()) return;
    await withButtonBusy(btn, "Adding...", async () => {
      try {
        await api("POST", "/api/reporters", { name: name.trim() });
        await refreshReporters();
        await refreshCurrentSettingsSurface({ force: true });
      }
      catch (e) { await showActionError(e); }
    });
  });
}

function openReporterMergeModal(source) {
  const targets = (state.reporters || [])
    .filter((r) => String(r.id) !== String(source.id));
  if (!targets.length) {
    toast("Add another reporter before merging.", "error");
    return Promise.resolve(null);
  }
  const body = () => `
    <div class="modal-title">Merge reporter</div>
    <div class="modal-body">
      <p class="muted" style="margin-top:0">
        Move every Gap reported by <strong>${htmlEscape(source.name)}</strong>
        to another reporter, then remove <strong>${htmlEscape(source.name)}</strong>
        from the dropdown.
      </p>
      <label>${renderSettingsGuideLabel("Merge into", "reporter-merge-into")}</label>
      <select class="modal-input" data-testid="reporter-merge-target" style="width:100%">
        ${targets.map((r) => `
          <option value="${r.id}">${htmlEscape(r.name)}</option>
        `).join("")}
      </select>
    </div>
    <div class="modal-actions">
      <button class="secondary" data-cancel data-testid="modal-cancel">Cancel</button>
      <button class="danger" data-ok data-testid="modal-ok">Merge</button>
    </div>`;
  return _openModal(body, { cancel: null, ok: "" }, ".modal-input");
}
