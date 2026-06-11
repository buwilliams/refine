// ---- System / Nodes -----------------------------------------------------

function renderSettingsNodesTab({
  nodes, nodeCounts, activeNodeId,
}) {
  return `
    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Nodes", "node-manage")}</h3>
      <p class="scope-label muted small">Project-wide</p>
      <table class="table" data-testid="node-settings-table">
        <thead><tr><th>Name</th><th>ID</th><th>Gaps</th><th>Host</th><th>Refine</th><th>Status</th><th></th></tr></thead>
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
              <td data-testid="node-settings-refine-port">${htmlEscape(String(inst.refine_port || 8082))}</td>
              <td data-testid="node-settings-status">${inst.enabled === false ? "disabled" : htmlEscape(inst.health?.status || (hasRemote ? "enabled" : "local"))}</td>
              <td class="actions node-row-actions">
                <div class="nav-create-group node-action-group">
                  <button data-node-activate="${htmlEscape(inst.id)}"
                          data-testid="node-activate"
                          class="node-action-primary"
                          ${isActive || inst.archived ? "disabled" : ""}>Activate</button>
                  <details class="nav-menu node-action-menu">
                    <summary class="btn secondary node-action-more"
                             aria-label="More node actions"
                             data-testid="node-action-menu-toggle"></summary>
                    <div class="nav-menu-panel node-action-panel">
                      <button class="nav-menu-item" type="button"
                              data-node-rename="${htmlEscape(inst.id)}"
                              data-name="${htmlEscape(inst.display_name || inst.id)}"
                              data-testid="node-rename">Rename</button>
                      <button class="nav-menu-item" type="button"
                              data-node-remote-configure="${htmlEscape(inst.id)}"
                              data-testid="node-remote-configure"
                              data-name="${htmlEscape(inst.display_name || inst.id)}"
                              data-ssh-host="${htmlEscape(inst.ssh_host || "")}"
                              data-ssh-user="${htmlEscape(inst.ssh_user || "")}"
                              data-ssh-identity-path="${htmlEscape(inst.ssh_identity_path || "")}"
                              data-ssh-port="${htmlEscape(String(inst.ssh_port || 22))}"
                              data-refine-checkout="${htmlEscape(inst.refine_checkout || "~/refine")}"
                              data-target-app-path="${htmlEscape(inst.target_app_path || "")}"
                              data-refine-port="${htmlEscape(String(inst.refine_port || 8082))}">Connection</button>
                      <button class="nav-menu-item" type="button"
                              data-node-remote-bootstrap="${htmlEscape(inst.id)}"
                              data-testid="node-remote-bootstrap"
                              ${hasRemote && !inst.archived ? "" : "disabled"}>Bootstrap</button>
                      <button class="nav-menu-item" type="button"
                              data-node-remote-toggle="${htmlEscape(inst.id)}"
                              data-enabled="${inst.enabled === false ? "0" : "1"}"
                              data-testid="node-remote-toggle"
                              ${hasRemote && !inst.archived ? "" : "disabled"}>${inst.enabled === false ? "Enable" : "Disable"}</button>
                      <button class="nav-menu-item danger" type="button"
                              data-node-archive="${htmlEscape(inst.id)}"
                              data-testid="node-archive"
                              ${isActive ? "disabled" : ""}>Archive</button>
                    </div>
                  </details>
                </div>
              </td>
            </tr>`;
          }).join("")}
        </tbody>
      </table>
      <div class="actions" style="margin-top:8px">
        <button id="node-add" data-testid="node-add">Create node</button>
      </div>
    </section>`;
}

function bindSettingsNodesTab() {
  const closeNodeActionMenu = (button) => {
    const menu = button.closest(".node-action-menu");
    if (menu) menu.open = false;
  };
  $$("[data-testid='node-action-menu-toggle']").forEach((summary) => {
    summary.addEventListener("click", () => {
      $$(".node-action-menu[open]").forEach((menu) => {
        if (!menu.contains(summary)) menu.open = false;
      });
    });
  });
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
    closeNodeActionMenu(b);
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
    closeNodeActionMenu(b);
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
  $$("[data-node-remote-configure]").forEach((b) => b.addEventListener("click", async () => {
    closeNodeActionMenu(b);
    const payload = await openNodeConnectionModal(b);
    if (!payload) return;
    await withButtonBusy(b, "Saving...", async () => {
      try {
        await api("PATCH", "/api/cluster/nodes/" + encodeURIComponent(b.dataset.nodeRemoteConfigure), {
          ...payload,
          display_name: payload.display_name || b.dataset.nodeRemoteConfigure,
        });
        await refreshSettingsTab("application", { force: true });
      } catch (e) { await showActionError(e); }
    });
  }));
  $$("[data-node-remote-toggle]").forEach((b) => b.addEventListener("click", async () => {
    closeNodeActionMenu(b);
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
    closeNodeActionMenu(b);
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

function openNodeConnectionModal(button) {
  return new Promise((resolve) => {
    const root = document.createElement("div");
    root.className = "modal-backdrop";
    root.dataset.testid = "modal-backdrop";
    root.innerHTML = `
      <div class="modal" role="dialog" aria-modal="true" data-testid="node-connection-modal" style="max-width:680px">
        <div class="modal-title">Configure node connection</div>
        <form data-node-connection-form>
          <div class="modal-body">
            <div class="form-grid two">
              <div class="form-row"><label>Display name</label>
                <input class="modal-input" type="text" name="display_name" data-testid="node-connection-display-name"
                       value="${htmlEscape(button.dataset.name || "")}"></div>
              <div class="form-row"><label>SSH host</label>
                <input type="text" name="ssh_host" data-testid="node-connection-ssh-host"
                       value="${htmlEscape(button.dataset.sshHost || "")}"></div>
              <div class="form-row"><label>SSH user</label>
                <input type="text" name="ssh_user" data-testid="node-connection-ssh-user"
                       value="${htmlEscape(button.dataset.sshUser || "")}"></div>
              <div class="form-row"><label>SSH port</label>
                <input type="number" name="ssh_port" data-testid="node-connection-ssh-port"
                       value="${htmlEscape(button.dataset.sshPort || "22")}"></div>
              <div class="form-row"><label>Identity path</label>
                <input type="text" name="ssh_identity_path" data-testid="node-connection-identity-path"
                       value="${htmlEscape(button.dataset.sshIdentityPath || "")}"></div>
              <div class="form-row"><label>Refine UI port</label>
                <input type="number" name="refine_port" data-testid="node-connection-refine-port"
                       value="${htmlEscape(button.dataset.refinePort || "8082")}"></div>
            </div>
            <div class="form-row"><label>Refine checkout path</label>
              <input type="text" name="refine_checkout" data-testid="node-connection-refine-checkout"
                     value="${htmlEscape(button.dataset.refineCheckout || "~/refine")}"></div>
            <div class="form-row"><label>Target app path</label>
              <input type="text" name="target_app_path" data-testid="node-connection-target-app-path"
                     value="${htmlEscape(button.dataset.targetAppPath || "")}"></div>
            <div class="form-error" data-node-connection-error style="display:none"></div>
          </div>
          <div class="modal-actions">
            <button type="button" class="secondary" data-cancel data-testid="node-connection-cancel">Cancel</button>
            <button type="submit" data-ok data-testid="node-connection-save">Save</button>
          </div>
        </form>
      </div>`;
    document.body.appendChild(root);

    let resolved = false;
    const form = root.querySelector("[data-node-connection-form]");
    const error = root.querySelector("[data-node-connection-error]");

    function close(value) {
      if (resolved) return;
      resolved = true;
      document.removeEventListener("keydown", onKey, true);
      root.remove();
      resolve(value);
    }

    function onKey(event) {
      if (event.key === "Escape") {
        event.preventDefault();
        close(null);
      }
    }

    function value(name) {
      return String(form.elements[name]?.value || "").trim();
    }

    function submit() {
      const sshHost = value("ssh_host");
      if (!sshHost) {
        error.textContent = "SSH host is required to attach a remote connection.";
        error.style.display = "";
        form.elements.ssh_host?.focus();
        return;
      }
      close({
        display_name: value("display_name"),
        ssh_host: sshHost,
        ssh_user: value("ssh_user"),
        ssh_identity_path: value("ssh_identity_path"),
        ssh_port: Number(value("ssh_port")) || 22,
        refine_checkout: value("refine_checkout") || "~/refine",
        target_app_path: value("target_app_path"),
        refine_port: Number(value("refine_port")) || 8082,
      });
    }

    document.addEventListener("keydown", onKey, true);
    root.addEventListener("click", (event) => {
      if (event.target === root) close(null);
    });
    root.querySelector("[data-cancel]")?.addEventListener("click", () => close(null));
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      submit();
    });

    const focus = form.elements.ssh_host || form.querySelector(".modal-input");
    focus?.focus();
    focus?.select?.();
  });
}
