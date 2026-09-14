// Existing help entry points now open the shared product documentation.
function openGuide({ itemId = "" } = {}) {
  const path = itemId ? `product/reference.md#${encodeURIComponent(itemId)}` : "product/get-started.md";
  window.open(`/hub/sites/refine/${path}`, "_blank", "noopener");
}
function initGuide() {
  document.addEventListener("click", event => {
    const button = event.target.closest("[data-guide-label-item]");
    if (!button) return;
    event.preventDefault();
    openGuide({ itemId: button.dataset.guideLabelItem });
  });
}
registerCommand({ id: "hub.refine", title: "Open Refine Hub", group: "Knowledge Hub", aliases: ["guide", "documentation", "release notes"], run: () => window.open("/hub/sites/refine/", "_blank", "noopener") });
