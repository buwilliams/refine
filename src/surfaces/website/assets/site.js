(function () {
  const origin = window.location.origin;
  const CHECK_ICON = '<svg aria-hidden="true" viewBox="0 0 24 24"><path d="M20 6 9 17l-5-5"></path></svg>';
  const copyButtonState = new WeakMap();

  function absoluteUrl(path) {
    return new URL(path, origin).toString();
  }

  function fillOriginText() {
    document.querySelectorAll("[data-origin-template]").forEach((element) => {
      element.textContent = element.dataset.originTemplate.replaceAll("{origin}", origin);
    });
    document.querySelectorAll("[data-origin-path]").forEach((element) => {
      element.textContent = absoluteUrl(element.dataset.originPath);
    });
  }

  async function copyText(text, button) {
    await navigator.clipboard.writeText(text);
    const state = copyButtonState.get(button) || {
      label: button.getAttribute("aria-label") || "Copy",
      html: button.innerHTML,
      timeout: null,
    };
    if (state.timeout) {
      window.clearTimeout(state.timeout);
    }
    button.classList.add("copied");
    button.setAttribute("aria-label", "Copied");
    button.innerHTML = CHECK_ICON;
    state.timeout = window.setTimeout(() => {
      button.classList.remove("copied");
      button.setAttribute("aria-label", state.label);
      button.innerHTML = state.html;
      state.timeout = null;
    }, 1400);
    copyButtonState.set(button, state);
  }

  function wireCopyButtons() {
    document.querySelectorAll("[data-copy-path], [data-copy-template]").forEach((button) => {
      button.addEventListener("click", async () => {
        const text = button.dataset.copyTemplate
          ? button.dataset.copyTemplate.replaceAll("{origin}", origin)
          : absoluteUrl(button.dataset.copyPath);
        try {
          await copyText(text, button);
        } catch (_error) {
          button.classList.add("copy-failed");
        }
      });
    });
  }

  fillOriginText();
  wireCopyButtons();
})();
