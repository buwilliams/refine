// ---- System / Instances -----------------------------------------------------

function renderSettingsInstancesTab({
  instances, instanceCounts, activeInstanceId,
}) {
  return `
    <section class="settings-section">
      <h3>Instances</h3>
      <p class="scope-label muted small">Project-wide</p>
      <table class="table">
        <thead><tr><th>Name</th><th>ID</th><th>Gaps</th><th></th></tr></thead>
        <tbody>
          ${instances.map((inst) => {
            const counts = instanceCounts[inst.id] || {};
            const total = Object.values(counts).reduce((a, b) => a + Number(b || 0), 0);
            const isActive = inst.id === activeInstanceId;
            return `<tr>
              <td>${htmlEscape(inst.display_name || inst.id)} ${isActive ? `<span class="filter-pill">active</span>` : ""}${inst.archived ? ` <span class="muted small">archived</span>` : ""}</td>
              <td><code>${htmlEscape(inst.id)}</code></td>
              <td class="muted small">${total}</td>
              <td class="actions">
                <button class="secondary" data-instance-activate="${htmlEscape(inst.id)}" ${isActive || inst.archived ? "disabled" : ""}>Activate</button>
                <button class="secondary" data-instance-rename="${htmlEscape(inst.id)}" data-name="${htmlEscape(inst.display_name || inst.id)}">Rename</button>
                <button class="danger" data-instance-archive="${htmlEscape(inst.id)}" ${isActive ? "disabled" : ""}>Archive</button>
              </td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="instance-add">Create instance</button>
      </div>
    </section>`;
}

function bindSettingsInstancesTab() {
  $("#instance-add")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const name = await modalPrompt("Instance name", "",
                                   { title: "Create instance" });
    if (!name || !name.trim()) return;
    await withButtonBusy(btn, "Creating...", async () => {
      try {
        await api("POST", "/api/instances", { display_name: name.trim() });
        await refreshSettingsTab("instances", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-instance-activate]").forEach((b) => b.addEventListener("click", async () => {
    await withButtonBusy(b, "Activating...", async () => {
      try {
        const result = await api("POST", "/api/instances/activate", { instance_id: b.dataset.instanceActivate });
        state.project = {
          ...(state.project || {}),
          instances: result.instances || state.project?.instances || [],
          active_instance_id: result.active_instance_id || "",
          active_instance: result.active_instance || null,
        };
        updateActiveInstanceLabel();
        await refreshInstanceScopedState();
        toast("Instance activated", "info");
        await refreshSettingsTab("instances", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-instance-rename]").forEach((b) => b.addEventListener("click", async () => {
    const name = await modalPrompt("Instance name", b.dataset.name || "",
                                   { title: "Rename instance" });
    if (!name || !name.trim()) return;
    await withButtonBusy(b, "Renaming...", async () => {
      try {
        await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceRename), {
          display_name: name.trim(),
        });
        await refreshSettingsTab("instances", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-instance-archive]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Archive this instance? Gap ownership IDs stay unchanged and can still be transferred.",
      { title: "Archive instance", okLabel: "Archive", danger: true },
    );
    if (!ok) return;
    await withButtonBusy(b, "Archiving...", async () => {
      try {
        await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceArchive), {
          archived: true,
        });
        await refreshSettingsTab("instances", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
}
