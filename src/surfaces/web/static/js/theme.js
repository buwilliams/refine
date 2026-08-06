// ---- Color theme -----------------------------------------------------------

// Theme selection is a browser-local surface preference. Apply it from the
// document head so the initial frame uses the right palette instead of flashing
// the light theme while the rest of the application starts.
(() => {
  const STORAGE_KEY = "refine_color_theme";
  const DARK_QUERY = "(prefers-color-scheme: dark)";

  function storedTheme() {
    try {
      const value = localStorage.getItem(STORAGE_KEY);
      return value === "light" || value === "dark" ? value : null;
    } catch {
      return null;
    }
  }

  function systemTheme() {
    try {
      return window.matchMedia?.(DARK_QUERY)?.matches ? "dark" : "light";
    } catch {
      return "light";
    }
  }

  function currentTheme() {
    return document.documentElement.dataset.theme || storedTheme() || systemTheme();
  }

  function syncThemeControl(theme) {
    const button = document.getElementById("btn-theme-toggle");
    if (!button) return;
    const dark = theme === "dark";
    button.setAttribute("aria-pressed", String(dark));
    button.setAttribute("aria-label", dark ? "Use light mode" : "Use dark mode");
    button.title = dark ? "Switch to light mode" : "Switch to dark mode";
    const status = button.querySelector(".nav-theme-status");
    if (status) status.textContent = dark ? "On" : "Off";
  }

  function applyTheme(theme, { persist = false, notify = false } = {}) {
    const next = theme === "dark" ? "dark" : "light";
    document.documentElement.dataset.theme = next;
    document.documentElement.style.colorScheme = next;
    if (persist) {
      try {
        localStorage.setItem(STORAGE_KEY, next);
      } catch {}
    }
    syncThemeControl(next);
    if (notify) {
      window.dispatchEvent(new CustomEvent("refine-theme-change", {
        detail: { theme: next },
      }));
    }
    return next;
  }

  function toggleTheme() {
    return applyTheme(currentTheme() === "dark" ? "light" : "dark", {
      persist: true,
      notify: true,
    });
  }

  function bindThemeControl() {
    syncThemeControl(currentTheme());
    const button = document.getElementById("btn-theme-toggle");
    if (!button || button.dataset.themeBound === "true") return;
    button.dataset.themeBound = "true";
    button.addEventListener("click", toggleTheme);
  }

  applyTheme(storedTheme() || systemTheme());

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bindThemeControl, { once: true });
  } else {
    bindThemeControl();
  }

  try {
    window.matchMedia?.(DARK_QUERY)?.addEventListener?.("change", (event) => {
      if (!storedTheme()) applyTheme(event.matches ? "dark" : "light", { notify: true });
    });
  } catch {}

  window.RefineTheme = {
    apply: (theme) => applyTheme(theme, { persist: true, notify: true }),
    current: currentTheme,
    toggle: toggleTheme,
  };
})();
