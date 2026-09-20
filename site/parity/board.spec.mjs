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
async function open(page, width) {
  page.on("pageerror", (error) => console.error(error));
  await page.goto("/");
  await page
    .locator("#tsk-demo")
    .evaluate((el, w) => (el.style.width = `${w}ch`), width);
  await expect(row(page, 12)).toBeVisible();
  await page.locator("#board-demo").focus();
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
    "selected · esc clears",
  );

  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).toContainText("multi-select");
  await row(page, 12).click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "1 selected · esc clears",
  );
  await page.keyboard.press("Shift+M");
  await expect(page.locator("#tsk-demo")).not.toContainText(
    "selected · esc clears",
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
    "2 selected · esc clears",
  );
  await expect(row(page, 12).locator(".tsk-row-prefix")).toContainText("▪");
  await expect(row(page, 13).locator(".tsk-row-prefix")).toContainText("▪");

  await page.keyboard.press("d");
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 13)).toHaveCount(0);
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 13)).toBeVisible();

  await page.keyboard.press("Shift+M");
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Space");
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "press x again to delete 2 tasks",
  );
  await page.keyboard.press("x");
  await expect(page.locator("#tsk-demo")).toContainText(
    "deleted 2 tasks · u restores",
  );
  await expect(row(page, 12)).toHaveCount(0);
  await expect(row(page, 13)).toHaveCount(0);
  await page.keyboard.press("u");
  await expect(row(page, 12)).toBeVisible();
  await expect(row(page, 13)).toBeVisible();

  await page.keyboard.press("Shift+M");
  await row(page, 12).click();
  await expect(page.locator("#tsk-demo")).toContainText(
    "1 selected · esc clears",
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
    'Deleted "Check the release notes" · u undo',
  );
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
    "selected · esc clears",
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

for (const width of [40, 78, 109, 110]) {
  test(`desk start open back and wrapping at ${width} columns`, async ({
    page,
  }, info) => {
    await open(page, width);
    await expect(page.locator(".tsk-attribution")).toHaveCount(0);
    await expect(page.locator(".tsk-sec")).not.toContainText(["IN MOTION"]);
    await capture(page, info, "initial");
    await page.keyboard.press("ArrowDown");
    await expect(row(page, 13)).toHaveClass(/is-sel/);
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
for (const width of [40, 78, 109])
  test(`peek attribution at ${width}`, async ({ page }, info) => {
    await open(page, width);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator(".tsk-attribution")).toHaveText(
      "    └─ tsk-parity",
    );
    await capture(page, info, "project-peek");
    await page.keyboard.press("Escape");
    await expect(page.locator(".tsk-attribution")).toHaveCount(0);
    await page.keyboard.press("2");
    await expect(page.locator(".tsk-tab")).toHaveCount(3);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowRight");
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
  await expect(page.locator(".tsk-peek")).toBeVisible();
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
  await expect(page.locator(".tsk-peek")).toBeVisible();
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
    page.locator('[role="dialog"][aria-label="project thread filter"]'),
  ).toBeVisible();
  await page
    .locator("[data-preview-filter-option]")
    .filter({ hasText: "#release" })
    .click();
  await expect(
    page.locator('[role="dialog"][aria-label="project thread filter"]'),
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
