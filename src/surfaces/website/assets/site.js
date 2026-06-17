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

  function closeMenu(toggle, panel) {
    toggle.setAttribute("aria-expanded", "false");
    toggle.setAttribute("aria-label", "Open navigation menu");
    panel.hidden = true;
  }

  function openMenu(toggle, panel) {
    toggle.setAttribute("aria-expanded", "true");
    toggle.setAttribute("aria-label", "Close navigation menu");
    panel.hidden = false;
  }

  function wireMenus() {
    document.querySelectorAll("[data-menu-toggle]").forEach((toggle) => {
      const panelId = toggle.getAttribute("aria-controls");
      const panel = panelId ? document.getElementById(panelId) : null;
      if (!panel) {
        return;
      }

      toggle.addEventListener("click", () => {
        if (panel.hidden) {
          openMenu(toggle, panel);
        } else {
          closeMenu(toggle, panel);
        }
      });

      panel.querySelectorAll("a").forEach((link) => {
        link.addEventListener("click", () => closeMenu(toggle, panel));
      });

      document.addEventListener("click", (event) => {
        if (panel.hidden || toggle.contains(event.target) || panel.contains(event.target)) {
          return;
        }
        closeMenu(toggle, panel);
      });

      document.addEventListener("keydown", (event) => {
        if (event.key === "Escape" && !panel.hidden) {
          closeMenu(toggle, panel);
          toggle.focus();
        }
      });
    });
  }

  fillOriginText();
  wireCopyButtons();
  wireMenus();
})();
