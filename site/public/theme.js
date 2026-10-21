(() => {
  const KEY = "tsk-theme";

  function currentTheme() {
    const t = document.documentElement.dataset.theme;
    return t === "light" ? "light" : "dark";
  }

  function applyTheme(theme, { animate = false, origin } = {}) {
    const next = theme === "light" ? "light" : "dark";
    const root = document.documentElement;

    const commit = () => {
      root.dataset.theme = next;
      root.style.colorScheme = next;
      try {
        localStorage.setItem(KEY, next);
        // Keep Starlight's picker in sync when visiting /docs later.
        localStorage.setItem("starlight-theme", next);
      } catch (_) {}
      const btn = document.querySelector("[data-theme-toggle]");
      if (btn) {
        btn.setAttribute("aria-label", next === "dark" ? "Switch to light mode" : "Switch to dark mode");
        btn.setAttribute("aria-pressed", next === "dark" ? "true" : "false");
      }
    };

    if (!animate) {
      commit();
      return;
    }

    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (origin) {
      root.style.setProperty("--theme-x", `${origin.x}px`);
      root.style.setProperty("--theme-y", `${origin.y}px`);
    }

    if (!reduced && typeof document.startViewTransition === "function") {
      document.startViewTransition(commit);
      return;
    }

    commit();
  }

  // Ensure a theme is painted even if the head script was skipped: prefer the
  // saved choice, then dark. Never overwrite a saved choice with the default.
  if (!document.documentElement.dataset.theme) {
    let saved = null;
    try {
      saved = localStorage.getItem(KEY) || localStorage.getItem("starlight-theme");
    } catch (_) {}
    applyTheme(saved === "light" ? "light" : "dark");
  } else {
    applyTheme(currentTheme());
  }

  document.addEventListener("click", (event) => {
    const btn = event.target.closest("[data-theme-toggle]");
    if (!btn) return;
    const rect = btn.getBoundingClientRect();
    applyTheme(currentTheme() === "dark" ? "light" : "dark", {
      animate: true,
      origin: { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 },
    });
  });
})();
