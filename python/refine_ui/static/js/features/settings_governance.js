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

function scheduleAutosaveSettingsGovernance() {
  autosaveSettingsGovernance().catch((e) =>
    showActionError(e, "Governance autosave failed"));
}

function bindGovernanceRuleButtons() {
  $$("[data-governance-remove-rule]").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".governance-rule-row")?.remove();
      if (!$(".governance-rule-input")) {
        $("#governance-rules-list").innerHTML = `<p class="muted small" data-empty-governance-rules>No rules yet.</p>`;
      }
      scheduleAutosaveSettingsGovernance();
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
    scheduleAutosaveSettingsGovernance();
  });
  row.querySelector("input")?.addEventListener("change", scheduleAutosaveSettingsGovernance);
  row.querySelector("input")?.focus();
}


function renderSettingsGovernanceTab(gov) {
  return `
    ${renderSettingsMarkdownField({
      id: "s-governance-product",
      title: "Product",
      value: gov.product || "",
      scope: "Project-wide",
      description: "The what and why: who the product is for, what problems it solves, and what success looks like.",
      rows: 7,
      guideItemId: "governance-product",
    })}

    ${renderSettingsMarkdownField({
      id: "s-governance-constitution",
      title: "Constitution",
      value: gov.constitution || "",
      description: "Non-negotiable principles for the entire project.",
      rows: 7,
      guideItemId: "governance-constitution",
    })}

    <section class="settings-section">
      <h3>${renderSettingsGuideLabel("Rules", "governance-rules")}</h3>
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
      </div>
    </section>`;
}

async function autosaveSettingsGovernance() {
  await api("PATCH", "/api/governance", {
    product: $("#s-governance-product").value,
    constitution: $("#s-governance-constitution").value,
    rules: collectGovernanceRules(),
  });
}

function bindSettingsGovernanceTab(tabSlug = "governance") {
  bindGovernanceRuleButtons();
  const root = document.querySelector(`[data-tab-pane="${tabSlug}"]`);
  bindSettingsMarkdownFields(root);
  bindSettingsAutosave(
    root,
    "#s-governance-product, #s-governance-constitution, .governance-rule-input",
    autosaveSettingsGovernance,
  );
  $("#s-governance-add-rule")?.addEventListener("click", () => addGovernanceRuleRow());
  $("#s-governance-generate")?.addEventListener("click", async (e) => {
    const btn = e.currentTarget;
    const product = ($("#s-governance-product")?.value || "").trim();
    const constitution = ($("#s-governance-constitution")?.value || "").trim();
    if (!product || !constitution) {
      toast("Product and Constitution are required to generate rules", "error");
      return;
    }
    await withButtonBusy(btn, "Generating…", async () => {
      try {
        const r = await api("POST", "/api/governance/generate-rules", {
          product, constitution,
        });
        $("#governance-rules-list").innerHTML = renderGovernanceRuleRows(r.rules || []);
        bindGovernanceRuleButtons();
        await autosaveSettingsGovernance();
        toast("Rules generated and saved", "info");
      } catch (e) { await showActionError(e); }
    });
  });
}
