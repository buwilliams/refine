// ---- System / Quality -------------------------------------------------------

function renderSettingsQualityTab(quality) {
  return `
    <section class="settings-section">
      <h3>Business requirements</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        Product behavior and requirements the Quality agent checks against tests.
      </p>
      <textarea id="s-quality-business-requirements" rows="9">${htmlEscape(quality.business_requirements || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Instructions</h3>
      <p class="muted small" style="margin-top:0">
        How the Quality agent should choose and evaluate test coverage.
      </p>
      <textarea id="s-quality-instructions" rows="9">${htmlEscape(quality.instructions || "")}</textarea>
      ${quality.configured ? "" : `
        <p class="muted small" style="color:var(--warn)">
          Quality can run once business requirements and instructions are both filled in.
        </p>`}
      <div class="actions" style="margin-top:10px">
        <button id="s-quality-save">Save quality</button>
      </div>
    </section>`;
}

function bindSettingsQualityTab() {
  $("#s-quality-save")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-quality-save"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/quality", {
          business_requirements: $("#s-quality-business-requirements").value,
          instructions: $("#s-quality-instructions").value,
        });
        toast("Quality saved", "info");
        await refreshSettingsTab("quality", { force: true });
      } catch (e) { await showActionError(e); }
    });
  });
}
