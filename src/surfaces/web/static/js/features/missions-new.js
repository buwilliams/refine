// ---- Missions: new ----------------------------------------------------------

function renderMissionNew() {
  if (renderNoProjectIfDetached("Missions")) return;
  renderBanners([]);
  $("#main").innerHTML = `
    <div class="page-title-row">
      <h2>New Mission</h2>
    </div>
    <div class="card mission-new-card" data-testid="mission-new-card">
      <label for="mission-name">Name</label>
      <input type="text" id="mission-name" class="modal-input" data-testid="mission-name" placeholder="e.g. Modernize authentication">
      <label for="mission-intent">Desired outcome</label>
      <textarea id="mission-intent" data-testid="mission-intent" placeholder="What should this Mission achieve?"></textarea>
      <label for="mission-reporter">Reporter</label>
      <select id="mission-reporter" class="modal-input" data-testid="mission-reporter">
        <option value="">— pick reporter —</option>
        ${(state.reporters || []).map((r) =>
          `<option value="${htmlEscape(r.name)}" ${r.name === (state.lastReporter || "") ? "selected" : ""}>${htmlEscape(r.name)}</option>`).join("")}
      </select>
      <div class="modal-actions">
        <button class="secondary" id="mission-create-cancel" data-testid="mission-create-cancel">Cancel</button>
        <button id="mission-create-submit" data-testid="mission-create-submit">Create</button>
      </div>
    </div>
  `;
  bindOnce($("#mission-create-cancel"), "click", () => {
    location.hash = "#/missions";
  });
  bindOnce($("#mission-create-submit"), "click", async () => {
    const nodeGeneration = captureNodeContextGeneration();
    const body = {
      name: $("#mission-name").value.trim() || "",
      intent: $("#mission-intent").value.trim() || "",
      reporter: $("#mission-reporter").value.trim() || state.lastReporter || "",
    };
    if (!body.name) {
      toast("Mission name is required", "error");
      return;
    }
    if (!body.intent) {
      toast("Mission intent is required", "error");
      return;
    }
    try {
      const saved = await api("POST", "/api/missions", body);
      if (!isNodeContextGenerationCurrent(nodeGeneration)) return;
      toast("Mission created", "success");
      location.hash = `#/missions/${encodeURIComponent(saved.mission.id)}`;
    } catch (e) {
      if (!isNodeContextGenerationCurrent(nodeGeneration)) return;
      showActionError(e);
    }
  });
  $("#mission-name")?.focus();
}
