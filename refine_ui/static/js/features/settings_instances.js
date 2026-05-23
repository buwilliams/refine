// ---- System / Instances -----------------------------------------------------

function renderSettingsInstancesTab(instances, instanceCounts, activeInstanceId, transferTargetInstances) {
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
        <button class="secondary" id="s-project-sync-now">Trigger sync repo</button>
        <button id="instance-add">Create instance</button>
      </div>
    </section>
    <section class="settings-section">
      <h3>Transfer Gaps</h3>
      <p class="muted small" style="margin-top:0">
        Transfers matching Gaps to another instance. If active work is present,
        Refine pauses agents, stops agent processes, cancels in-progress and
        ready-merge and awaiting-rebuild Gaps, then transfers them.
      </p>
      <div class="form-grid two">
        <div class="form-row"><label>From</label>
          <select id="instance-transfer-source">
            <option value="">All instances</option>
            ${instances.map((inst) => `<option value="${htmlEscape(inst.id)}">${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
          </select></div>
        <div class="form-row"><label>To</label>
          <select id="instance-transfer-target">
            ${transferTargetInstances.map((inst) => `<option value="${htmlEscape(inst.id)}" ${inst.id === activeInstanceId ? "selected" : ""}>${htmlEscape(inst.display_name || inst.id)}</option>`).join("")}
          </select></div>
      </div>
      <div class="actions"><button class="warn" id="instance-transfer">Transfer matching Gaps</button></div>
    </section>`;
}

function bindSettingsInstancesTab() {
  $("#instance-add")?.addEventListener("click", async () => {
    const name = await modalPrompt("Instance name", "",
                                   { title: "Create instance" });
    if (!name || !name.trim()) return;
    try {
      await api("POST", "/api/instances", { display_name: name.trim() });
      await refreshSettingsTab("instances", { force: true });
    } catch (e) { await showActionError(e); }
  });
  $$("[data-instance-activate]").forEach((b) => b.addEventListener("click", async () => {
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
  }));
  $$("[data-instance-rename]").forEach((b) => b.addEventListener("click", async () => {
    const name = await modalPrompt("Instance name", b.dataset.name || "",
                                   { title: "Rename instance" });
    if (!name || !name.trim()) return;
    try {
      await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceRename), {
        display_name: name.trim(),
      });
      await refreshSettingsTab("instances", { force: true });
    } catch (e) { await showActionError(e); }
  }));
  $$("[data-instance-archive]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Archive this instance? Gap ownership IDs stay unchanged and can still be transferred.",
      { title: "Archive instance", okLabel: "Archive", danger: true },
    );
    if (!ok) return;
    try {
      await api("PATCH", "/api/instances/" + encodeURIComponent(b.dataset.instanceArchive), {
        archived: true,
      });
      await refreshSettingsTab("instances", { force: true });
    } catch (e) { await showActionError(e); }
  }));
  $("#instance-transfer")?.addEventListener("click", async () => {
    const source = $("#instance-transfer-source")?.value || "";
    const target = $("#instance-transfer-target")?.value || "";
    if (!target) return;
    const ok = await modalConfirm(
      "Refine will pause agents, stop all running agent processes, mark matching " +
      "in-progress, ready-merge, and awaiting-rebuild Gaps as cancelled, then transfer all matching " +
      "Gaps to the selected instance.",
      {
        title: "Transfer Gaps",
        okLabel: "Pause, cancel, and transfer",
        cancelLabel: "Keep Gaps unchanged",
        danger: true,
      },
    );
    if (!ok) return;
    try {
      const r = await api("POST", "/api/instances/transfer-gaps", {
        source_instance_id: source,
        target_instance_id: target,
        cancel_active: true,
      });
      toast(
        `Transferred ${r.updated}; cancelled ${r.cancelled || 0}; ` +
        `stopped ${r.stopped_processes || 0} processes; skipped ${r.skipped}.`,
        "info",
      );
      await refreshSettingsTab("instances", { force: true });
    } catch (e) { await showActionError(e); }
  });
  $("#s-project-sync-now")?.addEventListener("click", async () => {
    const btn = $("#s-project-sync-now");
    await withButtonBusy(btn, "Syncing…", async () => {
      try {
        await syncProjectUpdates();
        await refreshReporters({ selectFallback: true });
        await refreshSettingsTab("instances", { force: true });
      } catch {
        // syncProjectUpdates already surfaced the specific git error.
      }
    });
  });
}
