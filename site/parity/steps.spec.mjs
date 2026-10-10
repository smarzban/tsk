import { readReference } from "./reference.mjs";
import { test, expect } from "@playwright/test";
async function open(page, width) {
  await page.goto("/");
  await page
    .locator("#tsk-demo")
    .evaluate((el, w) => (el.style.width = `${w}ch`), width);
  await page.locator("#board-demo").focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("s");
  await page.keyboard.press("Enter");
}
for (const width of [40, 78, 109, 110])
  test(`task page and step actions at ${width}`, async ({ page }, info) => {
    await open(page, width);
    await expect(page.locator(".tsk-steps-heading")).toHaveText("steps 1/2");
    await expect(page.locator("[data-step]")).toHaveCount(2);
    const reference = await readReference(`steps-${width}`);
    expect(
      (
        await page
          .locator("[data-step]")
          .nth(1)
          .locator(".tsk-step-text > span")
          .allTextContents()
      ).map((x) => x.trimEnd()),
    ).toEqual(reference);

    await expect(page.locator(".tsk-page-meta")).toContainText("⎇ default");
    // The dates moved from the footer to the paper trail.
    await expect(page.locator(".tsk-page-meta")).not.toContainText("created");
    await expect(page.locator(".tsk-meta-row")).toHaveCount(1);
    // The paper trail opens collapsed and dim; `g` shows every entry and folds them again.
    const heading = page.locator(".tsk-trail-heading");
    await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▸$/);
    await expect(heading).toHaveClass(/\bdim\b/);
    await expect(page.locator(".tsk-trail-entry")).toHaveCount(0);
    await page.keyboard.press("g");
    await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▾$/);
    await expect(page.locator(".tsk-trail-entry").last()).toContainText(
      "created · you",
    );
    await page.keyboard.press("g");
    await expect(page.locator(".tsk-trail-entry")).toHaveCount(0);
    const scrollbarRows = await readReference(`page-scrollbar-${width}`);
    const chrome = await page.locator("#tsk-demo").evaluate((el) => {
      const origin = el.getBoundingClientRect();
      const track = el.querySelector(".tsk-page-scrollbar:not([hidden])");
      if (!track) return { rows: [], atFrameEdge: true, overflow: false };
      const lineHeight = parseFloat(getComputedStyle(el).lineHeight);
      const body = el.querySelector(".tsk-task-surface");
      return {
        rows: [...track.children]
          .filter((row) => row.textContent === "▌")
          .map((row) =>
            Math.round(
              (row.getBoundingClientRect().top - origin.top) / lineHeight,
            ),
          ),
        atFrameEdge:
          Math.abs(track.getBoundingClientRect().right - origin.right) < 1,
        overflow: body.scrollHeight > body.clientHeight,
      };
    });
    expect(chrome.rows).toEqual(scrollbarRows);
    expect(chrome.atFrameEdge).toBe(true);
    expect(chrome.overflow).toBe(scrollbarRows.length > 0);
    if (width < 110) {
      const fits = await page.locator(".tsk-narrow-header").evaluate((el) => {
        const title = el.querySelector(".tsk-page-title > span"),
          status = el.querySelector(".tsk-state-slot");
        const range = document.createRange();
        range.selectNodeContents(title);
        return (
          !status.textContent ||
          range.getBoundingClientRect().right <=
            status.getBoundingClientRect().left
        );
      });
      expect(fits).toBe(true);
    }

    await page
      .locator("#tsk-demo")
      .screenshot({ path: info.outputPath("page.png") });
    await page.keyboard.press("Tab");
    await expect(page.locator("[data-step]").first()).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await page.keyboard.press("Enter");
    await expect(page.locator(".tsk-steps-heading")).toHaveText("steps 2/2");
    await expect(page.locator(".tsk-task-column")).toHaveAttribute(
      "data-status",
      "started",
    );
    await page.keyboard.press("e");
    await page.locator("#tsk-step-edit").fill("Renamed first step");
    await page.keyboard.press("Enter");
    await expect(page.locator(".tsk-task-column")).toHaveAttribute(
      "data-edit-state",
      "unsaved",
    );
    await page.keyboard.press("Escape");
    await expect(page.locator("[data-step]").first()).toContainText(
      "Read the sample notes",
    );
    await page.keyboard.press("e");
    await page.locator("#tsk-step-edit").fill("Saved rename");
    await page.keyboard.press("Shift+Enter");
    await expect(page.locator("[data-step]").first()).toContainText(
      "Saved rename",
    );
    await page.keyboard.press("a");
    await page.keyboard.press("Enter");
    await expect(page.locator(".tsk-step-refusal")).toHaveText("text required");
    await page.locator("#tsk-step-edit").fill("New step");
    await page.keyboard.press("Enter");
    await expect(page.locator("[data-step]")).toHaveCount(3);
    await expect(page.locator("#tsk-step-edit")).toHaveValue("");
    await page.locator("#tsk-step-edit").fill("Another step");
    await page.keyboard.press("Shift+Enter");
    await expect(page.locator("[data-step]")).toHaveCount(4);
    await page.locator("[data-step]").last().click();
    await page.keyboard.press("x");
    await expect(page.locator("[data-step]")).toHaveCount(4);
    await page.keyboard.press("x");
    await expect(page.locator("[data-step]")).toHaveCount(3);
    await page
      .locator("#tsk-demo")
      .screenshot({ path: info.outputPath("steps-edited.png") });
    await page.keyboard.press("Escape");
    await page.keyboard.press("Enter");
    await expect(page.locator("[data-step]").first()).toContainText(
      "Saved rename",
    );
    await expect(page.locator("[data-step]")).toHaveCount(3);
  });

// The TUI keeps the paper trail painted through an edit session, expanded or not, so the page
// body never jumps; its records take no Tab stops then.
test("the paper trail stays painted while a field is edited", async ({
  page,
}) => {
  await open(page, 78);
  const column = page.locator(".tsk-task-column");
  const heading = page.locator(".tsk-trail-heading");
  // A click on the heading expands the trail, like `g`.
  await heading.click();
  await expect(heading).toHaveText(/PAPER TRAIL · \d+ ▾$/);
  await expect(page.locator(".tsk-trail-entry").first()).toContainText(
    "ready → started · you",
  );
  await page.locator("#board-demo").focus();
  await page.keyboard.press("e");
  await expect(column).toHaveAttribute("data-edit-field", "title");
  await expect(heading).toHaveText(/PAPER TRAIL · \d+ ▾$/);
  await page.keyboard.press("Tab");
  await expect(column).toHaveAttribute("data-edit-field", "notes");
  await expect(heading).toHaveText(/PAPER TRAIL · \d+ ▾$/);
  await expect(page.locator(".tsk-trail-entry").last()).toContainText(
    "created · you",
  );
  await page.keyboard.press("Escape");
});

test("landing project task page keeps its default base visible", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator('[data-tab="project"]').click();
  await page.locator("#board-demo").focus();
  await page.keyboard.press("Enter");
  await expect(page.locator(".tsk-page-meta")).toContainText(
    "⎇ main (default)",
  );
});

// The trail collapses on every page open, like the app's fresh task form: expand it, close the
// page, open it again.
test("the paper trail collapses when the page is reopened", async ({
  page,
}) => {
  await open(page, 78);
  const heading = page.locator(".tsk-trail-heading");
  await page.keyboard.press("g");
  await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▾$/);
  await page.keyboard.press("Escape");
  await expect(heading).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▸$/);
  await expect(page.locator(".tsk-trail-entry")).toHaveCount(0);
});

// Tab walks the steps, `+ step`, then the trail heading, where Enter expands it; Tab wraps on.
test("Tab reaches the paper trail heading after + step and Enter expands it", async ({
  page,
}) => {
  await open(page, 78);
  const heading = page.locator(".tsk-trail-heading");
  for (let i = 0; i < 4; i += 1) await page.keyboard.press("Tab");
  await expect(heading).toHaveAttribute("aria-selected", "true");
  await expect(heading).toHaveText(/^▸ PAPER TRAIL · \d+ ▸$/);
  await page.keyboard.press("Enter");
  await expect(heading).toHaveText(/^▸ PAPER TRAIL · \d+ ▾$/);
  await expect(page.locator("[data-step]")).toHaveCount(2);
  await page.keyboard.press("Tab");
  await expect(heading).toHaveAttribute("aria-selected", "false");
  await expect(page.locator("[data-step]").first()).toHaveAttribute(
    "aria-selected",
    "true",
  );
});

// The project preview's page behaves the same: Tab reaches the heading, and the trail resets
// when another task's page opens and when the first one opens again.
test("the project preview page's trail is a Tab stop and resets on reopen", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .locator("#tsk-demo")
    .evaluate((el, w) => (el.style.width = `${w}ch`), 110);
  await page.locator("#board-demo").focus();
  await page.keyboard.press("3");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  const heading = page.locator(".tsk-trail-heading");
  await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▸$/);
  for (let i = 0; i < 12; i += 1) {
    if ((await heading.getAttribute("aria-selected")) === "true") break;
    await page.keyboard.press("Tab");
  }
  await expect(heading).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(heading).toHaveText(/PAPER TRAIL · \d+ ▾$/);
  const column = page.locator(".tsk-task-column");
  const first = await column.getAttribute("aria-label");
  // Another task's page opens collapsed.
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(column).not.toHaveAttribute("aria-label", first);
  await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▸$/);
  // And so does the first one, opened again.
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("Enter");
  await expect(column).toHaveAttribute("aria-label", first);
  await expect(heading).toHaveText(/^PAPER TRAIL · \d+ ▸$/);
});
