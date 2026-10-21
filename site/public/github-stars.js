(() => {
  // Refresh the built-in star counts from GitHub, at most once an hour per tab.
  const slots = document.querySelectorAll("[data-gh-stars]");
  if (!slots.length) return;
  const KEY = "tsk-gh-stars";
  const HOUR = 3600 * 1000;

  const format = (count) => {
    if (count < 1000) return String(count);
    const thousands = count / 1000;
    return `${thousands < 10 ? Math.floor(thousands * 10) / 10 : Math.floor(thousands)}k`;
  };

  const show = (count) => {
    slots.forEach((slot) => {
      slot.querySelector("[data-gh-stars-count]").textContent = format(count);
      slot.setAttribute("aria-label", `${count} GitHub stars`);
      slot.hidden = false;
    });
  };

  try {
    const cached = JSON.parse(sessionStorage.getItem(KEY) || "null");
    if (cached && Date.now() - cached.at < HOUR) {
      show(cached.count);
      return;
    }
  } catch {}

  fetch("https://api.github.com/repos/smarzban/tsk", {
    headers: { Accept: "application/vnd.github+json" },
  })
    .then((response) => (response.ok ? response.json() : null))
    .then((repo) => {
      const count = repo && repo.stargazers_count;
      if (!Number.isInteger(count)) return;
      show(count);
      try {
        sessionStorage.setItem(KEY, JSON.stringify({ count, at: Date.now() }));
      } catch {}
    })
    .catch(() => {});
})();
