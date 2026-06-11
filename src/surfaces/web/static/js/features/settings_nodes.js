// ---- System / Nodes -----------------------------------------------------

function renderSettingsNodesTab({
  nodes, nodeCounts, activeNodeId,
}) {
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Nodes", "node-manage")}</h3>
      <p class="scope-label muted small">Project-wide</p>
      <table class="table" data-testid="node-settings-table">
        <thead><tr><th>Name</th><th>ID</th><th>Gaps</th><th>Remote host</th><th>Refine</th><th>Status</th><th></th></tr></thead>
        <tbody>
          ${nodes.map((inst) => {
            const counts = nodeCounts[inst.id] || {};
            const total = Object.values(counts).reduce((a, b) => a + Number(b || 0), 0);
            const isActive = inst.id === activeNodeId;
            const hasRemote = Boolean(inst.ssh_host);
            return `<tr data-testid="node-settings-row" data-node-id="${htmlEscape(inst.id)}">
              <td data-testid="node-settings-name">${htmlEscape(inst.display_name || inst.id)} ${isActive ? `<span class="filter-pill">active</span>` : ""}${inst.archived ? ` <span class="muted small">archived</span>` : ""}</td>
              <td data-testid="node-settings-id"><code>${htmlEscape(inst.id)}</code></td>
              <td class="muted small">${total}</td>
              <td data-testid="node-settings-remote-host">${htmlEscape(inst.ssh_host || "")}</td>
              <td data-testid="node-settings-refine-port">${htmlEscape(String(inst.refine_port || 8080))}</td>
              <td data-testid="node-settings-status">${inst.enabled === false ? "disabled" : htmlEscape(inst.health?.status || (hasRemote ? "enabled" : "local"))}</td>
              <td class="actions">
                <button class="secondary" data-node-activate="${htmlEscape(inst.id)}" data-testid="node-activate" ${isActive || inst.archived ? "disabled" : ""}>Activate</button>
                <button class="secondary" data-node-rename="${htmlEscape(inst.id)}" data-name="${htmlEscape(inst.display_name || inst.id)}" data-testid="node-rename">Rename</button>
                <button class="secondary"
                        data-node-remote-configure="${htmlEscape(inst.id)}"
                        data-testid="node-remote-configure"
                        data-name="${htmlEscape(inst.display_name || inst.id)}"
                        data-ssh-host="${htmlEscape(inst.ssh_host || "")}"
                        data-ssh-port="${htmlEscape(String(inst.ssh_port || 22))}"
                        data-refine-checkout="${htmlEscape(inst.refine_checkout || "~/refine")}"
                        data-target-app-path="${htmlEscape(inst.target_app_path || "")}"
                        data-refine-port="${htmlEscape(String(inst.refine_port || 8080))}">Remote</button>
                <button class="secondary" data-node-remote-bootstrap="${htmlEscape(inst.id)}" data-testid="node-remote-bootstrap" ${hasRemote && !inst.archived ? "" : "disabled"}>Bootstrap</button>
                <button class="secondary" data-node-remote-toggle="${htmlEscape(inst.id)}" data-enabled="${inst.enabled === false ? "0" : "1"}" data-testid="node-remote-toggle" ${inst.archived ? "disabled" : ""}>${inst.enabled === false ? "Enable" : "Disable"}</button>
                <button class="danger" data-node-archive="${htmlEscape(inst.id)}" data-testid="node-archive" ${isActive ? "disabled" : ""}>Archive</button>
              </td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="node-add" data-testid="node-add">Create node</button>
        <button class="secondary" id="node-remote-add" data-testid="node-remote-add">Register remote node</button>
      </div>
    </section>`;
}

function bindSettingsNodesTab() {
  $("#node-add")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const name = await modalPrompt("Node name", "",
                                   { title: "Create node" });
    if (!name || !name.trim()) return;
    await withButtonBusy(btn, "Creating...", async () => {
      try {
        await api("POST", "/api/nodes", { display_name: name.trim() });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-node-activate]").forEach((b) => b.addEventListener("click", async () => {
    await withButtonBusy(b, "Activating...", async () => {
      try {
        const result = await api("POST", "/api/nodes/activate", { node_id: b.dataset.nodeActivate });
        state.project = {
          ...(state.project || {}),
          nodes: result.nodes || state.project?.nodes || [],
          active_node_id: result.active_node_id || "",
          active_node: result.active_node || null,
        };
        updateActiveNodeLabel();
        await refreshNodeScopedState();
        toast("Node activated", "info");
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-node-rename]").forEach((b) => b.addEventListener("click", async () => {
    const name = await modalPrompt("Node name", b.dataset.name || "",
                                   { title: "Rename node" });
    if (!name || !name.trim()) return;
    await withButtonBusy(b, "Renaming...", async () => {
      try {
        await api("PATCH", "/api/nodes/" + encodeURIComponent(b.dataset.nodeRename), {
          display_name: name.trim(),
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-node-archive]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Archive this node? Gap ownership IDs stay unchanged and can still be transferred.",
      { title: "Archive node", okLabel: "Archive", danger: true },
    );
    if (!ok) return;
    await withButtonBusy(b, "Archiving...", async () => {
      try {
        await api("PATCH", "/api/nodes/" + encodeURIComponent(b.dataset.nodeArchive), {
          archived: true,
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $("#node-remote-add")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const id = await modalPrompt("Node ID", "", { title: "Register remote node" });
    if (!id || !id.trim()) return;
    const sshHost = await modalPrompt("SSH host", "", { title: "Register remote node" });
    if (!sshHost || !sshHost.trim()) return;
    const targetAppPath = await modalPrompt("Target app path", "", { title: "Register remote node" });
    if (targetAppPath == null) return;
    await withButtonBusy(btn, "Registering...", async () => {
      try {
        await api("POST", "/api/cluster/nodes", {
          id: id.trim(),
          display_name: id.trim(),
          ssh_host: sshHost.trim(),
          target_app_path: targetAppPath.trim(),
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-node-remote-configure]").forEach((b) => b.addEventListener("click", async () => {
    const displayName = await modalPrompt("Display name", b.dataset.name || "", { title: "Configure remote node" });
    if (displayName == null) return;
    const sshHost = await modalPrompt("SSH host", b.dataset.sshHost || "", { title: "Configure remote node" });
    if (sshHost == null || !sshHost.trim()) return;
    const sshPort = await modalPrompt("SSH port", b.dataset.sshPort || "22", { title: "Configure remote node" });
    if (sshPort == null) return;
    const refineCheckout = await modalPrompt("Refine checkout path", b.dataset.refineCheckout || "~/refine", { title: "Configure remote node" });
    if (refineCheckout == null) return;
    const targetAppPath = await modalPrompt("Target app path", b.dataset.targetAppPath || "", { title: "Configure remote node" });
    if (targetAppPath == null) return;
    const refinePort = await modalPrompt("Refine UI port", b.dataset.refinePort || "8080", { title: "Configure remote node" });
    if (refinePort == null) return;
    await withButtonBusy(b, "Saving...", async () => {
      try {
        await api("PATCH", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.nodeRemoteConfigure), {
          display_name: displayName.trim() || b.dataset.nodeRemoteConfigure,
          ssh_host: sshHost.trim(),
          ssh_port: Number(sshPort) || 22,
          refine_checkout: refineCheckout.trim() || "~/refine",
          target_app_path: targetAppPath.trim(),
          refine_port: Number(refinePort) || 8080,
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-node-remote-toggle]").forEach((b) => b.addEventListener("click", async () => {
    const enabled = b.dataset.enabled !== "1";
    await withButtonBusy(b, enabled ? "Enabling..." : "Disabling...", async () => {
      try {
        await api("PATCH", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.nodeRemoteToggle), {
          enabled,
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-node-remote-bootstrap]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Bootstrap this node over SSH using the current host user?",
      { title: "Bootstrap node", okLabel: "Bootstrap" },
    );
    if (!ok) return;
    await withButtonBusy(b, "Bootstrapping...", async () => {
      try {
        const result = await api("POST", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.nodeRemoteBootstrap) + "/bootstrap", {});
        toast(result.ok ? "Node bootstrapped" : "Node bootstrap failed", result.ok ? "info" : "error");
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
}
