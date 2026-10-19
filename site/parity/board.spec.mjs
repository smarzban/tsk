import { readReference } from "./reference.mjs";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { test, expect } from "@playwright/test";
test.beforeAll(async ({ browser }) => {
  const root = new URL("../../", import.meta.url);
  const files = [
    "site/public/board-demo.js",
    "site/public/board-wrap.js",
    "site/public/task-steps.js",
    "site/public/landing.js",
    "site/src/styles/landing.css",
    "tests/fixtures/demo-parity/store.json",
  ];
  const hashes = Object.fromEntries(
    await Promise.all(
      files.map(async (path) => [
        path,
        createHash("sha256")
          .update(await readFile(new URL(path, root)))
          .digest("hex"),
      ]),
    ),
  );
  await writeFile(
    new URL("../parity-reference/browser-provenance.json", import.meta.url),
    JSON.stringify(
      {
        kind: "actual Chrome screenshots cropped to board root; not approved baselines",
        browser: browser.version(),
        head: execFileSync("git", ["rev-parse", "HEAD"], {
          encoding: "utf8",
        }).trim(),
        diff: execFileSync("git", ["diff", "--stat"], { encoding: "utf8" }),
        hashes,
      },
      null,
      2,
    ),
  );
});
const id = (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const row = (page, n) => page.locator(`[data-task="${id(n)}"]`);
// Move the cursor down to task `n`, as the keyboard does; the list wraps at its end.
async function selectTask(page, n) {
  for (let step = 0; step < 30; step += 1) {
    if (/\bis-sel\b/.test((await row(page, n).getAttribute("class")) || ""))
      return;
    await page.keyboard.press("ArrowDown");
  }
  throw new Error(`T${n} never took the cursor`);
}
async function open(page, width) {
  page.on("pageerror", (error) => console.error(error));
  await page.goto("/");
  await page
    .locator("#tsk-demo")
    .evaluate((el, w) => (el.style.width = `${w}ch`), width);
  await expect(row(page, 12)).toBeVisible();
  await page.locator("#board-demo").focus();
}
// A row's painted lines: each prefix line (the live line's `│` gutter on wrapped title
// lines) beside its title line. `aligned` says each prefix line sits on its title line's row.
async function paintedRow(locator) {
  return locator.evaluate((el) => {
    const prefixEl = el.querySelector(".tsk-row-prefix");
    const prefix = prefixEl.textContent.split("\n");
    // Title lines, then any row-label lines that wrapped below them.
    const titles = [
      ...el.querySelectorAll(
        ".tsk-title-line, .tsk-row-title > .tsk-row-label",
      ),
    ];
    const lines = titles.map((line, i) =>
      `${prefix[i] ?? " ".repeat(prefix[0].length)}${line.textContent}`.trimEnd(),
    );
    // Offsets from each column's first row: inline and block boxes differ by leading.
    const offsets = (tops) =>
      [...new Set(tops.map(Math.round))]
        .sort((a, b) => a - b)
        .map((top, _, all) => top - all[0]);
    const range = document.createRange();
    range.selectNodeContents(prefixEl);
    const prefixRows = offsets([...range.getClientRects()].map((r) => r.top));
    const titleRows = offsets(
      titles.map((line) => line.getBoundingClientRect().top),
    );
    const aligned =
      prefix.length === 1 ||
      JSON.stringify(prefixRows) === JSON.stringify(titleRows);
    return { lines, aligned };
  });
}
async function capture(page, testInfo, name) {
  await page.locator("#tsk-demo").screenshot({
    path: testInfo.outputPath(`${name}.png`),
    animations: "disabled",
  });
}
for (const width of [78, 110]) {
  test(`root Escape releases demo focus without switching tabs at ${width} columns`, async ({
    page,
  }) => {
    for (const [tab, key] of [
      ["desk", "1"],
      ["project", "2"],
      ["projects", "3"],
    ]) {
      await open(page, width);
      await page.keyboard.press(key);
      await expect(page.locator(`[data-tab="${tab}"]`)).toHaveClass(/is-on/);
      await page.keyboard.press("Escape");
      await expect(page.locator(`[data-tab="${tab}"]`)).toHaveClass(/is-on/);
      await expect(page.locator("#board-demo")).not.toBeFocused();
    }
  });
}

test("multi-select gates task marking and ctrl click stays ordinary", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Space");
  await row(page, 12).dispatchEvent("click", { ctrlKey: true });
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clear",
  );

  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).toContainText("multi-select");
  await row(page, 12).click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "1 selected · esc clear",
  );
  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clear",
  );
});

test("multi-select preserves text input ownership and spends Escape first", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("+");
  const input = page.locator("#tsk-add");
  await input.fill("");
  await input.press("Shift+M");
  await expect(input).toHaveValue("M");

  await input.press("Escape");
  await expect(input).toBeVisible();
  await input.press("Escape");
  await expect(input).toHaveCount(0);
});

test("marked task sets complete, delete, and undo as one demo action", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Space");
  await expect(page.locator("#tsk-demo")).toContainText(
    "2 selected · esc clear",
  );
  await expect(row(page, 12).locator(".tsk-row-prefix")).toContainText("▪");
  await expect(row(page, 23).locator(".tsk-row-prefix")).toContainText("▪");

  await page.keyboard.press("d");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 23)).toHaveCount(0);
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 23)).toBeVisible();

  await page.keyboard.press("Shift+M");
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Space");
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "press x again to delete 2 tasks",
  );
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "deleted 2 tasks · u undo",
  );
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 23)).toHaveCount(0);
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 23)).toBeVisible();

  await page.keyboard.press("Shift+M");
  await row(page, 12).click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "1 selected · esc clear",
  );
  await page.keyboard.press("Escape");
  await page.keyboard.press("x");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Escape");
  await page.keyboard.press("x");
  await expect(row(page, 12)).toBeVisible();
  await expect(page.locator("#tsk-demo")).toContainText(
    "press x again to delete",
  );
  await expect(page.locator("#tsk-demo")).not.toContainText("delete 1 task");
  await page.keyboard.press("x");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(page.locator("#tsk-demo")).toContainText(
    'Deleted T12 "Check the release notes and the migration guide before the announcement goes out tomorrow" · u undo',
  );
  // The notice leaves the row after a few seconds; u still restores the task.
  await expect(page.locator("#tsk-demo")).not.toContainText("Deleted T12", {
    timeout: 6000,
  });
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
});

test("task-page demo actions ignore board marks and use the open task", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("Shift+M");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Space");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await page.keyboard.press("d");
  await page.keyboard.press("Escape");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 13)).toBeVisible();
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clear",
  );
});

test("Escape closes Help then collapses both wide splits", async ({ page }) => {
  for (const projects of [false, true]) {
    await open(page, 110);
    if (projects) {
      await page.keyboard.press("3");
      await page.keyboard.press("ArrowDown");
    } else {
      await page.keyboard.press("ArrowRight");
    }
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await page.keyboard.press("?");
    await page.keyboard.press("Escape");
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(".tsk-wide-split")).toHaveCount(0);
    await expect(page.locator("#board-demo")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.locator("#board-demo")).not.toBeFocused();
  }
});

// T12's page leads with the BLOCKED section the app paints: the live line, why and needs as
// plain text, the numbered options. Only the action line differs: the demo answers nothing.
for (const width of [40, 78, 109, 110]) {
  test(`blocked task page section at ${width} columns`, async ({ page }) => {
    await open(page, width);
    await row(page, 12).click();
    await page.keyboard.press("Enter");
    const lines = (
      await page.locator(".tsk-page-block > span").allTextContents()
    ).map((line) => line.trimEnd());
    expect(lines.slice(0, -2)).toEqual(
      await readReference(`blocked-page-${width}`),
    );
    expect(lines.at(-1)).toBe("b unblock");
    await expect(page.locator(".tsk-trail-heading")).toHaveText(
      /^PAPER TRAIL · \d+ ▸$/,
    );
  });
}

for (const width of [40, 78, 109, 110]) {
  test(`desk start open back and wrapping at ${width} columns`, async ({
    page,
  }, info) => {
    await open(page, width);
    await expect(page.locator(".tsk-attribution")).toHaveCount(0);
    // T24 waits on an agent, so it rides in IN MOTION.
    await expect(page.locator(".tsk-sec")).toContainText(["IN MOTION"]);
    await expect(row(page, 24).locator(".tsk-row-glyph")).toHaveText("□");
    // The blocked row's live line matches the app's at the fixture's clock.
    await expect(row(page, 12).locator(".tsk-live-line")).toHaveText(
      await readReference(`live-${width}`),
    );
    // Its wrapped title lines carry the live line's gutter, as the app paints them.
    await expect
      .poll(() => paintedRow(row(page, 12)))
      .toEqual({ lines: await readReference(`row-${width}`), aligned: true });
    await capture(page, info, "initial");
    await selectTask(page, 13);
    const lines = await row(page, 13)
      .locator(".tsk-title-line")
      .allTextContents();
    expect(lines.length).toBeGreaterThan(1);
    const appLines = await readReference(`title-${width}`);
    expect(lines.map((line) => line.trimEnd())).toEqual(appLines);
    expect(lines.join("")).toBe(
      "Read every word of this long title before starting the task so the continuation must align beneath its title and retain the final boundary marker END",
    );
    const bounds = await row(page, 13).evaluate((el) => {
      const r = el.getBoundingClientRect();
      return [...el.querySelectorAll(".tsk-title-line")].map((line) => {
        const range = document.createRange();
        range.selectNodeContents(line);
        const b = range.getBoundingClientRect();
        return { left: b.left - r.left, right: r.right - b.right };
      });
    });
    expect(
      Math.max(...bounds.map((b) => b.left)) -
        Math.min(...bounds.map((b) => b.left)),
    ).toBeLessThan(1);
    expect(Math.min(...bounds.map((b) => b.right))).toBeGreaterThanOrEqual(15);
    await page.keyboard.press("s");
    await expect(row(page, 13).locator(".tsk-row-glyph")).toHaveText("●");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
    await capture(page, info, "started");
    await page.keyboard.press("Enter");
    await expect(page.locator(".tsk-task-column")).toHaveAttribute(
      "data-status",
      "started",
    );
    await expect(page.locator(".tsk-page-notes")).toHaveText(
      "A plain note for the first matched task-page flow.",
    );
    await expect(page.locator(".tsk-list")).toHaveCount(0);
    await expect(page.locator(".tsk-foot")).toHaveCount(1);
    await capture(page, info, "page");
    await page.keyboard.press("Escape");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
    await capture(page, info, "back");
  });
}
// The rows a dispatch board adds: T23 a review on you (the asker's words under it), T24
// blocked on @pi (in IN MOTION, its label says what it waits on and what runs after it), and
// T25 assigned to @claude, running after T24.
for (const width of [40, 78, 109, 110])
  test(`dispatch board rows at ${width} columns`, async ({ page }) => {
    await open(page, width);
    for (const n of [23, 24, 25]) {
      await expect
        .poll(() => paintedRow(row(page, n)))
        .toEqual({
          lines: await readReference(`row-T${n}-${width}`),
          aligned: true,
        });
      const live = await readReference(`live-T${n}-${width}`);
      if (live.length)
        await expect(row(page, n).locator(".tsk-live-line")).toHaveText(live);
      else await expect(row(page, n).locator(".tsk-live-line")).toHaveCount(0);
    }
  });

for (const width of [40, 78, 109])
  test(`dispatch board peeks at ${width} columns`, async ({ page }) => {
    await open(page, width);
    for (const n of [23, 24, 25]) {
      await selectTask(page, n);
      await page.keyboard.press("ArrowRight");
      await expect(page.locator(".tsk-peek, .tsk-attribution")).toHaveText(
        await readReference(`peek-T${n}-${width}`),
      );
      await page.keyboard.press("ArrowLeft");
    }
  });

// The demo's keys are bare on purpose; the verbs, their words and their order are the app's.
test("verb bars carry the app's verbs for each status", async ({ page }) => {
  const bare = async (name) =>
    (await readReference(name))[0].replaceAll("ctrl+", "");
  await open(page, 78);
  const footer = page.locator(".tsk-verbs");
  await expect(footer).toHaveText(await bare("footer-T12-78"));
  for (const n of [23, 24, 25, 18]) {
    await selectTask(page, n);
    await expect(footer).toHaveText(await bare(`footer-T${n}-78`));
  }
  // The task page's bar follows its own task's status.
  for (const n of [12, 23]) {
    await selectTask(page, n);
    await page.keyboard.press("Enter");
    await expect(footer).toHaveText(await bare(`footer-page-T${n}-78`));
    await page.keyboard.press("Escape");
  }
});

test("a narrow verb bar drops whole hints and stays one row", async ({
  page,
}) => {
  await open(page, 40);
  const footer = page.locator(".tsk-verbs");
  await expect(footer).toHaveText("enter open · d done · ? help");
  const rows = await footer.evaluate(
    (el) =>
      el.getBoundingClientRect().height /
      parseFloat(getComputedStyle(el).lineHeight),
  );
  expect(Math.round(rows)).toBe(1);
});

for (const width of [40, 78, 109, 110])
  test(`review task page section at ${width} columns`, async ({ page }) => {
    await open(page, width);
    await selectTask(page, 23);
    await page.keyboard.press("Enter");
    const lines = (
      await page.locator(".tsk-page-block > span").allTextContents()
    ).map((line) => line.trimEnd());
    expect(lines.slice(0, -2)).toEqual(
      await readReference(`review-page-${width}`),
    );
    expect(lines.at(-1)).toBe("d done · r back to ready");
  });

test("the split's status row names its stage as the app does", async ({
  page,
}) => {
  await open(page, 110);
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-stage-hint")).toHaveText(
    (await readReference("status-hint-split-110"))[0],
  );
});

// Done on the last task another waits on starts the waiting one, and one undo reverses both.
test("a done releases the task that runs after it", async ({ page }) => {
  await open(page, 78);
  await selectTask(page, 24);
  await page.keyboard.press("d");
  await expect(row(page, 25).locator(".tsk-row-glyph")).toHaveText("●");
  // Started, it moves to IN MOTION, where a desk row names its scope.
  await expect(row(page, 25).locator(".tsk-row-label")).toHaveText("  desk");
  await page.keyboard.press("u");
  await expect(row(page, 24).locator(".tsk-row-glyph")).toHaveText("□");
  await expect(row(page, 25).locator(".tsk-row-glyph")).toHaveText("○");
  await expect(row(page, 25).locator(".tsk-row-label")).toHaveText(
    "  after T24",
  );
});

for (const width of [40, 78, 109])
  test(`peek attribution at ${width}`, async ({ page }, info) => {
    await open(page, width);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".tsk-attribution")).toHaveText(
      "    └─ #release · tsk-parity",
    );
    // A recorded block's peek replaces the live line: live text, why, Decide; no notes.
    await expect(page.locator(".tsk-peek, .tsk-attribution")).toHaveText(
      await readReference(`peek-${width}`),
    );
    await expect(row(page, 12).locator(".tsk-live-line")).toHaveCount(0);
    // The peek keeps the gutter on the wrapped title, so no gap opens above it.
    await expect
      .poll(() => paintedRow(row(page, 12)))
      .toEqual({
        lines: await readReference(`peek-row-${width}`),
        aligned: true,
      });
    await capture(page, info, "project-peek");
    await page.keyboard.press("Escape");
    await expect(page.locator(".tsk-attribution")).toHaveCount(0);
    await expect(row(page, 12).locator(".tsk-live-line")).toHaveText(
      await readReference(`live-${width}`),
    );
    await page.keyboard.press("2");
    await expect(page.locator(".tsk-tab")).toHaveCount(3);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowRight");
    // A project board leaves its own project out of the label.
    await expect(page.locator(".tsk-attribution")).toHaveText(
      "    └─ #release",
    );
    await capture(page, info, "thread-peek");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".tsk-attribution")).toHaveCount(0);
    await expect(page.locator(".tsk-peek")).toContainText(["no notes yet"]);
    await capture(page, info, "unlabeled-peek");
  });
for (const width of [110, 130])
  test(`project preview live line and block peek at ${width}`, async ({
    page,
  }) => {
    await open(page, width);
    await page.keyboard.press("3");
    await page.keyboard.press("ArrowDown");
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await page.keyboard.press("l");
    await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
    const previewRow = page.locator(`[data-preview-task="${id(12)}"]`);
    await expect(previewRow.locator(".tsk-live-line")).toHaveText(
      await readReference(`preview-live-${width}`),
    );
    await expect
      .poll(() => paintedRow(previewRow))
      .toEqual({
        lines: await readReference(`preview-row-${width}`),
        aligned: true,
      });
    await page.keyboard.press("l");
    const preview = page.locator(".tsk-project-preview");
    await expect(preview.locator(".tsk-peek, .tsk-attribution")).toHaveText(
      await readReference(`preview-peek-${width}`),
    );
    await expect(previewRow.locator(".tsk-live-line")).toHaveCount(0);
    await expect
      .poll(() => paintedRow(previewRow))
      .toEqual({
        lines: await readReference(`preview-peek-row-${width}`),
        aligned: true,
      });
    await page.keyboard.press("h");
    await expect(preview.locator(".tsk-peek")).toHaveCount(0);
    await expect(previewRow.locator(".tsk-live-line")).toHaveText(
      await readReference(`preview-closed-${width}`),
    );
  });

test("110-column boundary and rail mouse return", async ({ page }, info) => {
  await open(page, 109);
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-attribution")).toBeVisible();
  await page.locator("#tsk-demo").evaluate((el) => (el.style.width = "110ch"));
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(page.locator(".tsk-peek")).toHaveCount(0);
  await capture(page, info, "split-110");
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-wide-split.is-rail")).toBeVisible();
  await row(page, 13).click();
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(row(page, 13)).toHaveClass(/is-sel/);
});

test("h and l mirror task-detail navigation in every demo board", async ({
  page,
}) => {
  await open(page, 109);
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-peek").first()).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-peek")).toHaveCount(0);

  await open(page, 110);
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-wide-split.is-rail")).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-wide-split")).toHaveCount(0);

  await page.keyboard.press("3");
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
  await page.keyboard.press("l");
  await expect(page.locator(".tsk-peek").first()).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-peek")).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(page.locator(".tsk-task-column")).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
  await page.keyboard.press("h");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
});

for (const width of [110, 130]) {
  test(`row double-click survives reflow at ${width} columns`, async ({
    page,
  }) => {
    await open(page, width);
    // The reflow guard in board-demo.js fires only when the second click lands on the
    // same client coordinates as the first, after the split has moved the row from under
    // them. So both clicks must reuse one (x, y). The flake was upstream of that: a row
    // handle read before layout had an empty rect and the click went to (-4, 4). Wait for
    // a laid-out rect, then click twice at the same point.
    const result = await page.evaluate(async (taskId) => {
      const settle = () => new Promise((r) => requestAnimationFrame(() => r()));
      let el = null;
      for (let attempt = 0; attempt < 10 && !el; attempt += 1) {
        const candidate = document.querySelector(`[data-task="${taskId}"]`);
        if (candidate && candidate.getBoundingClientRect().width > 0)
          el = candidate;
        else await settle();
      }
      if (!el) throw new Error(`row ${taskId} never laid out`);
      const rect = el.getBoundingClientRect();
      const x = Math.min(rect.right - 4, window.innerWidth - 8);
      const y = rect.top + 4;
      const click = () => {
        const hit = document.elementFromPoint(x, y);
        if (!hit) throw new Error(`nothing at (${x}, ${y})`);
        hit.dispatchEvent(
          new MouseEvent("click", {
            bubbles: true,
            clientX: x,
            clientY: y,
            detail: 1,
          }),
        );
      };
      click();
      const split = !!document.querySelector(".tsk-wide-split.is-split");
      click();
      return { split, full: !document.querySelector(".tsk-list") };
    }, id(13));
    expect(result).toEqual({ split: true, full: true });
    await expect(page.locator(".tsk-task-column")).toContainText("END");
  });
}

test("projects overview opens a live project preview and keeps its task seat", async ({
  page,
}) => {
  await open(page, 110);
  await page.keyboard.press("3");
  await expect(page.locator(".tsk-project-row")).toHaveCount(1);
  await expect(page.locator(".tsk-wide-split")).toHaveCount(0);
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.locator(".tsk-project-row").click();
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("ArrowLeft");
  await expect(page.locator(".tsk-wide-split")).toHaveCount(0);
  await page.locator(".tsk-project-row").click();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(page.locator(".tsk-project-preview.is-preview")).toBeVisible();
  await page.locator("[data-preview-task]").first().click();
  await expect(page.locator(".tsk-wide-split.is-rail")).toBeVisible();
  await page.locator("[data-project-row]").first().click();
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-wide-split.is-rail")).toBeVisible();
  await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
  await expect(page.locator("[data-preview-task]")).not.toHaveCount(0);
  await page.keyboard.press("t");
  await expect(
    page.locator('[role="dialog"][aria-label="project filter"]'),
  ).toBeVisible();
  await page
    .locator("[data-preview-filter-option]")
    .filter({ hasText: "#release" })
    .click();
  await expect(
    page.locator('[role="dialog"][aria-label="project filter"]'),
  ).toHaveCount(0);
  await expect(page.locator("[data-preview-task]")).toHaveCount(2);
  await expect(
    page.locator("[data-preview-task]").filter({
      hasText: "Check the unlabeled project task",
    }),
  ).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(page.locator(".tsk-task-column")).toHaveAttribute(
    "data-status",
    "blocked",
  );
  await page.keyboard.press("ArrowLeft");
  await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
  await page.locator("[data-project-row]").first().click();
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
});

test("projects preview keeps an unsaved page draft when the index retakes focus", async ({
  page,
}) => {
  await open(page, 110);
  await page.keyboard.press("3");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await page.keyboard.press("e");
  await page.locator("#tsk-preview-edit").fill("Unsaved preview title");
  await page.locator("[data-project-row]").first().click();
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(page.locator("#board-demo")).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator("#tsk-preview-edit")).toHaveValue(
    "Unsaved preview title",
  );
});

for (const resolution of ["cancel", "save"]) {
  test(`projects preview refusal expires after ${resolution} and stays cleared on reopen`, async ({
    page,
  }) => {
    await open(page, 110);
    await page.keyboard.press("3");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    await page.keyboard.press("e");
    const editor = page.locator("#tsk-preview-edit");
    const originalDraft = await editor.inputValue();
    await editor.fill("Resolved preview title");
    await page.locator("[data-project-row]").first().click();
    await page.keyboard.press("Escape");
    const status = page.locator(".tsk-status-row");
    const refusal = "save or cancel edits before switching tasks";
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await expect(status).toContainText(refusal);
    await page.keyboard.press("ArrowRight");
    await expect(editor).toHaveValue("Resolved preview title");
    await expect(status).toContainText(refusal);
    await page.keyboard.press(resolution === "cancel" ? "Escape" : "Enter");
    await expect(editor).toHaveCount(0);
    await expect.soft(status).not.toContainText(refusal);
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator(".tsk-project-preview.is-live")).toBeVisible();
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await expect.soft(status).not.toContainText(refusal);
    await page.keyboard.press("Escape");
    await expect(page.locator(".tsk-wide-split")).toHaveCount(0);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
    await expect.soft(status).not.toContainText(refusal);
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("e");
    await expect(editor).toHaveValue(
      resolution === "cancel" ? originalDraft : "Resolved preview title",
    );
  });
}

async function expectFormRing(
  page,
  { preview = false, title, notes, start = "title" },
) {
  const column = page.locator(".tsk-task-column");
  const edit = preview ? "#tsk-preview-edit" : "#tsk-edit";
  const step = preview ? "[data-preview-step]" : "[data-step]";
  await expect(column).toHaveAttribute("data-edit-field", start);
  if (start === "title") {
    await page.locator(edit).fill(title);
    await page.keyboard.press("Tab");
    await expect(column).toHaveAttribute("data-edit-field", "notes");
    await expect(column).toContainText(title);
  }
  await page.locator(edit).fill(notes);
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "steps");
  const stepIds = await page
    .locator(step)
    .evaluateAll(
      (rows, attribute) => rows.map((row) => row.getAttribute(attribute)),
      preview ? "data-preview-step" : "data-step",
    );
  if (stepIds.length) {
    await expect(column).toHaveAttribute("data-edit-target", stepIds[0]);
    await expect(page.locator(step).first()).toHaveAttribute(
      "aria-selected",
      "true",
    );
    for (const id of stepIds.slice(1)) {
      await page.keyboard.press("Tab");
      await expect(column).toHaveAttribute("data-edit-target", id);
    }
    await page.keyboard.press("Tab");
  }
  await expect(column).toHaveAttribute("data-edit-target", "add");
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "thread");
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "scope");
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "title");
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-field", "scope");
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-field", "thread");
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-target", "add");
  for (const id of [...stepIds].reverse()) {
    await page.keyboard.press("Shift+Tab");
    await expect(column).toHaveAttribute("data-edit-target", id);
  }
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-field", "notes");
  await expect(column).toContainText(notes);
  await page.keyboard.press("Shift+Tab");
  await expect(column).toHaveAttribute("data-edit-field", "title");
}

test("task page and expanded quick-add keep the full reversible form ring", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator('[data-tab="project"]').click();
  await page.locator("#board-demo").focus();
  await page.keyboard.press("Enter");
  await page.keyboard.press("Control+e");
  await expectFormRing(page, {
    title: "Task title survives Tab",
    notes: "Task notes survive Tab",
  });
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");

  await page.keyboard.press("+");
  await page.locator("#tsk-add").fill("Quick ring");
  await page.keyboard.press("Tab");
  await expect(
    page.locator("[data-task]").filter({ hasText: "Quick ring" }),
  ).toHaveCount(0);
  await expectFormRing(page, {
    title: "Quick ring",
    notes: "Quick notes survive Tab",
    start: "notes",
  });
  await page.keyboard.press("Escape");
  await expect(page.locator("#tsk-add")).toHaveValue("Quick ring");
  await expect(
    page.locator("[data-task]").filter({ hasText: "Quick ring" }),
  ).toHaveCount(0);
});

test("Tab does not persist an empty task title", async ({ page }) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator('[data-tab="project"]').click();
  await page.locator("#board-demo").focus();
  await page.keyboard.press("Enter");
  const originalTitle = await page.locator(".tsk-task-header").textContent();
  await page.keyboard.press("Control+e");
  await page.locator("#tsk-edit").fill("");
  await page.keyboard.press("Tab");
  await page.keyboard.press("Escape");
  await expect(page.locator(".tsk-task-header")).toHaveText(
    originalTitle || "",
  );
});

test("project preview expanded quick-add uses the same ring without saving", async ({
  page,
}) => {
  await open(page, 110);
  await page.keyboard.press("3");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("+");
  await page.locator("#tsk-add").fill("Preview capture");
  await page.keyboard.press("Tab");
  await expect(page.locator(".tsk-task-column")).toContainText(
    "Preview capture",
  );
  await expectFormRing(page, {
    preview: true,
    title: "Preview capture",
    notes: "Preview notes survive Tab",
    start: "notes",
  });
  await page.keyboard.press("Escape");
  await expect(page.locator("#tsk-add")).toHaveValue("Preview capture");
  await page.keyboard.press("Escape");
  await expect(
    page.locator("[data-preview-task]").filter({ hasText: "Preview capture" }),
  ).toHaveCount(0);
});

test("landing column readout excludes board padding", async ({ page }) => {
  await open(page, 78);
  await page.evaluate(() => {
    const frame = document.getElementById("board-demo");
    const split = document.createElement("div");
    split.dataset.split = "";
    split.style.width = "1200px";
    frame.before(split);
    split.append(frame);
    const readout = document.createElement("span");
    readout.dataset.cols = "";
    split.append(readout);
    document.getElementById("tsk-demo").style.padding = "20px";
  });
  await page.addScriptTag({ url: "/public/landing.js" });
  await expect(page.locator("[data-cols] b")).toHaveText("78");
});

for (const width of [40, 78])
  test(`row labels flow after the title at ${width}`, async ({ page }) => {
    await open(page, width);
    await expect(row(page, 16).locator(".tsk-row-label")).toHaveText(
      width === 40 ? "#triage" : "  #triage",
    );
  });

test("sub-headers fold with arrows and Enter opens their board", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("g");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  const release = page.locator('[data-collapse$=":t:release"]').first();
  await expect(release).toContainText("▾");
  await release.click();
  await expect(release).toContainText("▾");
  await expect(release.locator("strong")).toHaveCount(1);
  await page.keyboard.press("h");
  await expect(release).toContainText("▸");
  await page.keyboard.press("l");
  await expect(release).toContainText("▾");
  await page.keyboard.press("h");
  await expect(release).toContainText("▸");
  await page.keyboard.press("h");
  await expect(release).toContainText("▸");
  await page.keyboard.press("l");
  await page.keyboard.press("Enter");
  await expect(page.locator("#board-demo")).toContainText("#release");
  await expect(row(page, 14)).toBeVisible();
  await expect(row(page, 16)).toHaveCount(0);
});

test("a click selects a header and a double-click opens or folds it", async ({
  page,
}) => {
  await open(page, 78);
  await page.keyboard.press("g");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  // Separate gestures wait out the 350 ms double-click window.
  const settle = () => page.waitForTimeout(400);
  const loose = page.locator('[data-collapse$=":none"]').first();
  await loose.click();
  await expect(loose).toContainText("▾");
  await settle();
  await loose.dblclick();
  await expect(loose).toContainText("▸");
  await settle();
  await loose.dblclick();
  await expect(loose).toContainText("▾");
  await settle();
  // A click elsewhere between two header clicks leaves two single clicks.
  await loose.click();
  await page.locator("#board-demo [data-task]").first().click();
  await loose.click();
  await expect(loose).toContainText("▾");
  await settle();
  const inbox = page.locator("[data-inbox-header]");
  await inbox.click();
  await expect(inbox).toContainText("▾");
  await settle();
  await inbox.dblclick();
  await expect(inbox).toContainText("▸");
  await settle();
  await page.locator('[data-collapse$=":t:release"]').first().dblclick();
  await expect(page.locator("#board-demo")).toContainText("#release");
  await expect(row(page, 14)).toBeVisible();
  await expect(row(page, 16)).toHaveCount(0);
});
