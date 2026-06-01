// ---- System / Nodes -----------------------------------------------------

function renderSettingsNodesTab({
  nodes, nodeCounts, activeNodeId, clusterNodes = [],
}) {
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Nodes", "node-manage")}</h3>
      <p class="scope-label muted small">Project-wide</p>
      <table class="table">
        <thead><tr><th>Name</th><th>ID</th><th>Gaps</th><th></th></tr></thead>
        <tbody>
          ${nodes.map((inst) => {
            const counts = nodeCounts[inst.id] || {};
            const total = Object.values(counts).reduce((a, b) => a + Number(b || 0), 0);
            const isActive = inst.id === activeNodeId;
            return `<tr>
              <td>${htmlEscape(inst.display_name || inst.id)} ${isActive ? `<span class="filter-pill">active</span>` : ""}${inst.archived ? ` <span class="muted small">archived</span>` : ""}</td>
              <td><code>${htmlEscape(inst.id)}</code></td>
              <td class="muted small">${total}</td>
              <td class="actions">
                <button class="secondary" data-node-activate="${htmlEscape(inst.id)}" ${isActive || inst.archived ? "disabled" : ""}>Activate</button>
                <button class="secondary" data-node-rename="${htmlEscape(inst.id)}" data-name="${htmlEscape(inst.display_name || inst.id)}">Rename</button>
                <button class="danger" data-node-archive="${htmlEscape(inst.id)}" ${isActive ? "disabled" : ""}>Archive</button>
              </td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="node-add">Create node</button>
      </div>
    </section>
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Cluster", "cluster-manage")}</h3>
      <p class="scope-label muted small">Git-synced remote nodes</p>
      <table class="table">
        <thead><tr><th>Name</th><th>Host</th><th>Port</th><th>Refine</th><th>Status</th><th></th></tr></thead>
        <tbody>
          ${(clusterNodes || []).map((node) => `
            <tr>
              <td>${htmlEscape(node.display_name || node.id)}<br><code>${htmlEscape(node.id)}</code></td>
              <td>${htmlEscape(node.ssh_host || "")}</td>
              <td>${htmlEscape(String(node.ssh_port || 22))}</td>
              <td>${htmlEscape(String(node.refine_port || 8080))}</td>
              <td>${node.enabled === false ? "disabled" : htmlEscape(node.health?.status || "enabled")}</td>
              <td class="actions">
                <button class="secondary"
                        data-cluster-configure="${htmlEscape(node.id)}"
                        data-name="${htmlEscape(node.display_name || node.id)}"
                        data-ssh-host="${htmlEscape(node.ssh_host || "")}"
                        data-ssh-port="${htmlEscape(String(node.ssh_port || 22))}"
                        data-refine-checkout="${htmlEscape(node.refine_checkout || "~/refine")}"
                        data-target-app-path="${htmlEscape(node.target_app_path || "")}"
                        data-refine-port="${htmlEscape(String(node.refine_port || 8080))}">Configure</button>
                <button class="secondary" data-cluster-bootstrap="${htmlEscape(node.id)}">Bootstrap</button>
                <button class="secondary" data-cluster-toggle="${htmlEscape(node.id)}" data-enabled="${node.enabled === false ? "0" : "1"}">${node.enabled === false ? "Enable" : "Disable"}</button>
              </td>
            </tr>`).join("") || `<tr><td colspan="6" class="muted">No cluster nodes registered.</td></tr>`}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="cluster-node-add">Register cluster node</button>
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
        await refreshSettingsTab("nodes", { force: true });
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
        await refreshSettingsTab("nodes", { force: true });
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
        await refreshSettingsTab("nodes", { force: true });
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
        await refreshSettingsTab("nodes", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $("#cluster-node-add")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const id = await modalPrompt("Node ID", "", { title: "Register cluster node" });
    if (!id || !id.trim()) return;
    const sshHost = await modalPrompt("SSH host", "", { title: "Register cluster node" });
    if (!sshHost || !sshHost.trim()) return;
    const targetAppPath = await modalPrompt("Target app path", "", { title: "Register cluster node" });
    if (targetAppPath == null) return;
    await withButtonBusy(btn, "Registering...", async () => {
      try {
        await api("POST", "/api/cluster/nodes", {
          id: id.trim(),
          display_name: id.trim(),
          ssh_host: sshHost.trim(),
          target_app_path: targetAppPath.trim(),
        });
        await refreshSettingsTab("nodes", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
  $$("[data-cluster-configure]").forEach((b) => b.addEventListener("click", async () => {
    const displayName = await modalPrompt("Display name", b.dataset.name || "", { title: "Configure cluster node" });
    if (displayName == null) return;
    const sshHost = await modalPrompt("SSH host", b.dataset.sshHost || "", { title: "Configure cluster node" });
    if (sshHost == null || !sshHost.trim()) return;
    const sshPort = await modalPrompt("SSH port", b.dataset.sshPort || "22", { title: "Configure cluster node" });
    if (sshPort == null) return;
    const refineCheckout = await modalPrompt("Refine checkout path", b.dataset.refineCheckout || "~/refine", { title: "Configure cluster node" });
    if (refineCheckout == null) return;
    const targetAppPath = await modalPrompt("Target app path", b.dataset.targetAppPath || "", { title: "Configure cluster node" });
    if (targetAppPath == null) return;
    const refinePort = await modalPrompt("Refine UI port", b.dataset.refinePort || "8080", { title: "Configure cluster node" });
    if (refinePort == null) return;
    await withButtonBusy(b, "Saving...", async () => {
      try {
        await api("PATCH", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.clusterConfigure), {
          display_name: displayName.trim() || b.dataset.clusterConfigure,
          ssh_host: sshHost.trim(),
          ssh_port: Number(sshPort) || 22,
          refine_checkout: refineCheckout.trim() || "~/refine",
          target_app_path: targetAppPath.trim(),
          refine_port: Number(refinePort) || 8080,
        });
        await refreshSettingsTab("nodes", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-cluster-toggle]").forEach((b) => b.addEventListener("click", async () => {
    const enabled = b.dataset.enabled !== "1";
    await withButtonBusy(b, enabled ? "Enabling..." : "Disabling...", async () => {
      try {
        await api("PATCH", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.clusterToggle), {
          enabled,
        });
        await refreshSettingsTab("nodes", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-cluster-bootstrap]").forEach((b) => b.addEventListener("click", async () => {
    const ok = await modalConfirm(
      "Bootstrap this cluster node over SSH using the current host user?",
      { title: "Bootstrap cluster node", okLabel: "Bootstrap" },
    );
    if (!ok) return;
    await withButtonBusy(b, "Bootstrapping...", async () => {
      try {
        const result = await api("POST", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.clusterBootstrap) + "/bootstrap", {});
        toast(result.ok ? "Cluster node bootstrapped" : "Cluster bootstrap failed", result.ok ? "info" : "error");
        await refreshSettingsTab("nodes", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
}
