// ---- System / Governance ----------------------------------------------------

function renderGovernanceRuleRows(rules) {
  const rows = (rules || []).map((rule) => `
    <div class="governance-rule-row">
      <input type="text" class="governance-rule-input"
             value="${htmlEscape(rule.text || "")}"
             data-rule-id="${htmlEscape(rule.id || "")}"
             data-created="${htmlEscape(rule.created || "")}"
             data-source="${htmlEscape(rule.source || "manual")}">
      <button class="danger" data-governance-remove-rule>Remove</button>
    </div>`).join("");
  return rows || `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
}

function renderGovernanceRules(rules) {
  return `
    <div id="governance-rules-list">
      ${renderGovernanceRuleRows(rules)}
    </div>`;
}

function collectGovernanceRules() {
  return $$(".governance-rule-input").map((input) => ({
    id: input.dataset.ruleId || "",
    text: (input.value || "").trim(),
    created: input.dataset.created || "",
    source: input.dataset.source || "manual",
  })).filter((rule) => rule.text);
}

function bindGovernanceRuleButtons() {
  $$("[data-governance-remove-rule]").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".governance-rule-row")?.remove();
      if (!$(".governance-rule-input")) {
        $("#governance-rules-list").innerHTML = `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
      }
    });
  });
}

function addGovernanceRuleRow(text = "") {
  const list = $("#governance-rules-list");
  if (!list) return;
  list.querySelector("[data-empty-governance-rules]")?.remove();
  const row = document.createElement("div");
  row.className = "governance-rule-row";
  row.innerHTML = `
    <input type="text" class="governance-rule-input"
           value="${htmlEscape(text)}"
           data-rule-id="" data-created="" data-source="manual">
    <button class="danger" data-governance-remove-rule>Remove</button>
  `;
  list.appendChild(row);
  row.querySelector("[data-governance-remove-rule]").addEventListener("click", () => {
    row.remove();
    if (!$(".governance-rule-input")) {
      list.innerHTML = `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
    }
  });
  row.querySelector("input")?.focus();
}


function renderSettingsGovernanceTab(gov) {
  return `
    <section class="settings-section">
      <h3>Product</h3>
      <p class="scope-label muted small">Project-wide</p>
      <p class="muted small" style="margin-top:0">
        The what and why: who the product is for, what problems it solves,
        and what success looks like.
      </p>
      <textarea id="s-governance-product" rows="7">${htmlEscape(gov.product || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Constitution</h3>
      <p class="muted small" style="margin-top:0">
        Non-negotiable principles for the entire project.
      </p>
      <textarea id="s-governance-constitution" rows="7">${htmlEscape(gov.constitution || "")}</textarea>
    </section>

    <section class="settings-section">
      <h3>Rules</h3>
      <p class="muted small" style="margin-top:0">
        One-line rules the Governance agent applies before implementation.
      </p>
      ${gov.configured ? "" : `
        <p class="muted small" style="color:var(--warn)">
          Governance is incomplete. Gap execution continues until Product and Constitution are both filled in.
        </p>`}
      ${renderGovernanceRules(gov.rules || [])}
      <div class="actions" style="margin-top:10px">
        <button class="secondary" id="s-governance-add-rule">Add rule</button>
        <button class="secondary" id="s-governance-generate">Generate rules</button>
        <span class="spacer"></span>
        <button id="s-governance-save">Save governance</button>
      </div>
    </section>`;
}

function bindSettingsGovernanceTab() {
  bindGovernanceRuleButtons();
  $("#s-governance-add-rule")?.addEventListener("click", () => addGovernanceRuleRow());
  $("#s-governance-save")?.addEventListener("click", async () => {
    await withButtonBusy($("#s-governance-save"), "Saving…", async () => {
      try {
        await api("PATCH", "/api/governance", {
          product: $("#s-governance-product").value,
          constitution: $("#s-governance-constitution").value,
          rules: collectGovernanceRules(),
        });
        toast("Governance saved", "info");
        await refreshSettings();
      } catch (e) { await showActionError(e); }
    });
  });
  $("#s-governance-generate")?.addEventListener("click", async () => {
    const product = ($("#s-governance-product")?.value || "").trim();
    const constitution = ($("#s-governance-constitution")?.value || "").trim();
    if (!product || !constitution) {
      toast("Product and Constitution are required to generate rules", "error");
      return;
    }
    await withButtonBusy($("#s-governance-generate"), "Generating…", async () => {
      try {
        const r = await api("POST", "/api/governance/generate-rules", {
          product, constitution,
        });
        $("#governance-rules-list").innerHTML = renderGovernanceRuleRows(r.rules || []);
        bindGovernanceRuleButtons();
        toast("Rules generated — review and save", "info");
      } catch (e) { await showActionError(e); }
    });
  });
}
