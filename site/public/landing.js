(() => {
  // Mark JS availability before the first paint of any [data-reveal] band:
  // without this class every band stays fully visible (no-JS fallback).
  document.documentElement.classList.add("js");

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  // ── reveal on scroll ─────────────────────────────────────────────────────
  const reveal = document.querySelectorAll("[data-reveal]");
  if ("IntersectionObserver" in window && !reduced) {
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-in");
            io.unobserve(entry.target);
          }
        }
      },
      { threshold: 0.12, rootMargin: "0px 0px -6% 0px" },
    );
    reveal.forEach((el) => io.observe(el));
  } else {
    reveal.forEach((el) => el.classList.add("is-in"));
  }

  // ── status cycle: one row walking through the six human states ──────────
  const cycle = document.querySelector("[data-cycle]");
  if (cycle) {
    const steps = [
      {
        sec: "inbox",
        glyph: "◌",
        status: "open",
        key: "ctrl+n",
        next: "ready",
      },
      {
        sec: "ON DECK",
        glyph: "○",
        status: "ready",
        key: "ctrl+s",
        next: "started",
      },
      {
        sec: "IN MOTION",
        glyph: "●",
        status: "started",
        key: "ctrl+b",
        next: "blocked",
      },
      {
        sec: "NEEDS YOU",
        glyph: "■",
        status: "blocked",
        key: "ctrl+b",
        next: "ready",
      },
      {
        sec: "ON DECK",
        glyph: "○",
        status: "ready",
        key: "ctrl+r",
        next: "review",
      },
      {
        sec: "NEEDS YOU",
        glyph: "▲",
        status: "review",
        key: "ctrl+d",
        next: "done",
      },
      { sec: "DONE", glyph: "✓", status: "done", key: "ctrl+o", next: "open" },
    ];
    const sec = cycle.querySelector("[data-cycle-sec]");
    const glyph = cycle.querySelector("[data-cycle-glyph]");
    const key = cycle.querySelector("[data-cycle-key]");
    const next = cycle.querySelector("[data-cycle-next]");
    let i = 0;
    const paint = () => {
      const s = steps[i];
      sec.textContent = s.sec;
      glyph.textContent = s.glyph;
      key.textContent = s.key;
      next.textContent = s.next;
      cycle.classList.remove("is-tick");
      // restart the tick animation
      void cycle.offsetWidth;
      cycle.classList.add("is-tick");
    };
    paint();
    if (!reduced) {
      let timer = null;
      const start = () => {
        if (timer) return;
        timer = window.setInterval(() => {
          i = (i + 1) % steps.length;
          paint();
        }, 2200);
      };
      const stop = () => {
        if (!timer) return;
        window.clearInterval(timer);
        timer = null;
      };
      if ("IntersectionObserver" in window) {
        const io = new IntersectionObserver(
          (entries) => {
            entries.forEach((e) => (e.isIntersecting ? start() : stop()));
          },
          { threshold: 0.2 },
        );
        io.observe(cycle);
      } else {
        start();
      }
      document.addEventListener("visibilitychange", () => {
        if (document.hidden) stop();
      });
    }
  }

  // ── install tabs ─────────────────────────────────────────────────────────
  const install = document.querySelector("[data-install]");
  if (install) {
    const tabs = [...install.querySelectorAll("[data-install-tab]")];
    const panels = [...install.querySelectorAll("[data-install-panel]")];
    const select = (name, focus = false) => {
      tabs.forEach((tab) => {
        const on = tab.dataset.installTab === name;
        tab.setAttribute("aria-selected", on ? "true" : "false");
        tab.tabIndex = on ? 0 : -1;
        if (on && focus) tab.focus();
      });
      panels.forEach((panel) => {
        panel.hidden = panel.dataset.installPanel !== name;
      });
    };
    tabs.forEach((tab, idx) => {
      tab.addEventListener("click", () => select(tab.dataset.installTab));
      tab.addEventListener("keydown", (e) => {
        if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
        e.preventDefault();
        const delta = e.key === "ArrowRight" ? 1 : -1;
        const nextTab = tabs[(idx + delta + tabs.length) % tabs.length];
        select(nextTab.dataset.installTab, true);
      });
    });
  }

  // ── demo layouts: beside an agent (78 columns) or the full terminal ───────
  const split = document.querySelector("[data-split]");
  const board = document.getElementById("tsk-demo");
  const frame = document.getElementById("board-demo");
  if (split && board && frame) {
    const divider = split.querySelector("[data-divider]");
    const cols = document.querySelector("[data-cols]");
    const toggles = [...document.querySelectorAll("[data-layout]")];
    const DIVIDER = 9;
    const BESIDE_COLUMNS = 78;

    const charWidth = () => {
      const style = getComputedStyle(board);
      const context = document.createElement("canvas").getContext("2d");
      context.font = `${style.fontSize} ${style.fontFamily}`;
      return context.measureText("0").width;
    };
    const boardPadding = () => {
      const cs = getComputedStyle(board);
      return (
        (Number.parseFloat(cs.paddingLeft) || 0) +
        (Number.parseFloat(cs.paddingRight) || 0)
      );
    };
    // Match the demo: usable content width divided by the measured monospace cell.
    const columns = () =>
      Math.round((board.clientWidth - boardPadding()) / charWidth());
    const minAgent = 180;
    const maxAgent = () =>
      split.clientWidth - DIVIDER - (40 * charWidth() + boardPadding());

    const setAgent = (px) => {
      const w = Math.max(minAgent, Math.min(maxAgent(), Math.round(px)));
      split.style.setProperty("--agent-w", `${w}px`);
      if (divider) divider.setAttribute("aria-valuenow", String(w));
    };
    const agentForBoardColumns = (n) =>
      split.clientWidth - DIVIDER - (n * charWidth() + boardPadding());

    let currentLayout = "full";
    const setLayout = (name) => {
      currentLayout = name;
      const beside = name === "beside";
      split.classList.toggle("is-beside", beside);
      split.classList.toggle("is-full", !beside);
      toggles.forEach((t) =>
        t.setAttribute(
          "aria-pressed",
          t.dataset.layout === name ? "true" : "false",
        ),
      );
      if (beside) setAgent(agentForBoardColumns(BESIDE_COLUMNS));
      readout();
      // Full terminal opens straight into the split (stage A); beside an agent the board is
      // narrow, so it returns to the plain board. The demo module registers this listener
      // after the first call here; tsk:ready replays the selected layout through this event.
      frame.dispatchEvent(
        new CustomEvent("tsk:set-stage", {
          detail: beside ? "board" : "split",
        }),
      );
    };

    const readout = () => {
      if (!cols) return;
      const n = columns();
      const wide = n >= 110;
      cols.innerHTML = `<b>${n}</b> cols · ${wide ? "stage slider" : "peek"}`;
    };

    toggles.forEach((t) =>
      t.addEventListener("click", () => setLayout(t.dataset.layout)),
    );

    if (divider) {
      let dragging = false;
      divider.addEventListener("pointerdown", (e) => {
        dragging = true;
        divider.classList.add("is-dragging");
        divider.setPointerCapture(e.pointerId);
        e.preventDefault();
      });
      divider.addEventListener("pointermove", (e) => {
        if (!dragging) return;
        const left = split.getBoundingClientRect().left;
        setAgent(e.clientX - left - DIVIDER / 2);
      });
      const stop = (e) => {
        if (!dragging) return;
        dragging = false;
        divider.classList.remove("is-dragging");
        try {
          divider.releasePointerCapture(e.pointerId);
        } catch (_) {}
      };
      divider.addEventListener("pointerup", stop);
      divider.addEventListener("pointercancel", stop);
      divider.addEventListener("keydown", (e) => {
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        e.preventDefault();
        e.stopPropagation();
        const step = 4 * charWidth();
        const current =
          Number.parseFloat(
            getComputedStyle(split).getPropertyValue("--agent-w"),
          ) || 0;
        setAgent(current + (e.key === "ArrowLeft" ? -step : step));
      });
    }

    if ("ResizeObserver" in window) {
      new ResizeObserver(() => {
        if (split.classList.contains("is-beside")) {
          // Keep the board at 78 columns until the user drags the divider.
          if (!split.dataset.userSized)
            setAgent(agentForBoardColumns(BESIDE_COLUMNS));
        }
        readout();
      }).observe(split);
      new ResizeObserver(readout).observe(board);
      if (divider)
        divider.addEventListener("pointerdown", () => {
          split.dataset.userSized = "1";
        });
    }

    frame.addEventListener("tsk:ready", () => setLayout(currentLayout));
    setLayout("full");
  }

  // ── goto disclosure ──────────────────────────────────────────────────────
  const goto = document.querySelector("[data-goto]");
  const gotoSummary = goto && goto.querySelector(".goto-chip");
  if (goto && gotoSummary) {
    const close = () => {
      goto.open = false;
    };
    goto.addEventListener("toggle", () => {
      gotoSummary.setAttribute("aria-expanded", String(goto.open));
    });
    gotoSummary.setAttribute("aria-expanded", "false");
    gotoSummary.setAttribute("aria-haspopup", "true");
    goto.querySelector(".goto-menu")?.addEventListener("click", (e) => {
      if (e.target.closest("a")) close();
    });
    document.addEventListener("click", (e) => {
      if (!goto.open) return;
      if (!(e.target instanceof Node) || !goto.contains(e.target)) close();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && goto.open) {
        close();
        gotoSummary.focus();
        return;
      }
      if (e.key !== ":") return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target;
      if (!(t instanceof Element)) return;
      if (
        t.closest(
          "#board-demo, input, textarea, select, [contenteditable='true']",
        )
      ) {
        return;
      }
      e.preventDefault();
      goto.open = true;
      const first = goto.querySelector(".goto-menu a");
      if (first instanceof HTMLElement) first.focus();
    });
  }

  // ── board pin callouts ───────────────────────────────────────────────────
  const pinRoot = document.querySelector("[data-board-pins]");
  if (pinRoot) {
    const pins = [...pinRoot.querySelectorAll("[data-pin]")];
    const nEl = pinRoot.querySelector("[data-pin-n]");
    const bodyEl = pinRoot.querySelector("[data-pin-body]");
    const copies = new Map();
    pinRoot.querySelectorAll("[data-pin-copy]").forEach((tpl) => {
      copies.set(tpl.dataset.pinCopy, tpl.innerHTML);
    });
    let sticky = "1";
    const paint = (n) => {
      const html = copies.get(n);
      if (!html || !nEl || !bodyEl) return;
      nEl.textContent = n;
      bodyEl.innerHTML = html;
      pins.forEach((btn) => {
        const on = btn.dataset.pin === n;
        btn.classList.toggle("is-on", on);
        btn.setAttribute("aria-expanded", String(on));
        btn.closest(".s-row")?.classList.toggle("is-on", on);
      });
    };
    const show = (n, stick) => {
      if (stick) sticky = n;
      paint(n);
    };
    pins.forEach((btn) => {
      btn.addEventListener("mouseenter", () => show(btn.dataset.pin, false));
      btn.addEventListener("focus", () => show(btn.dataset.pin, false));
      btn.addEventListener("click", () => show(btn.dataset.pin, true));
    });
    pinRoot.addEventListener("mouseleave", () => paint(sticky));
    pinRoot.addEventListener("focusout", (e) => {
      if (!pinRoot.contains(e.relatedTarget)) paint(sticky);
    });
    paint(sticky);
  }

  // ── copy buttons ─────────────────────────────────────────────────────────
  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const wrap = btn.closest(".shell-wrap");
      const pre = wrap && wrap.querySelector("[data-copy-source]");
      if (!pre) return;
      // Strip the "$ " prompts so the paste runs as-is.
      const text = pre.innerText
        .split("\n")
        .map((line) => line.replace(/^\$\s?/, ""))
        .filter((line) => line.trim().length)
        .join("\n");
      let ok = false;
      try {
        await navigator.clipboard.writeText(text);
        ok = true;
      } catch (_) {
        ok = false;
      }
      const label = btn.textContent;
      btn.textContent = ok ? "copied" : "select to copy";
      btn.classList.toggle("is-done", ok);
      window.setTimeout(() => {
        btn.textContent = label;
        btn.classList.remove("is-done");
      }, 1400);
    });
  });
})();
