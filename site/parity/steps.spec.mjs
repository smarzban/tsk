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
    await expect(page.locator(".tsk-trail-heading")).toHaveText("PAPER TRAIL");
    await expect(page.locator(".tsk-trail-entry").last()).toContainText(
      "created · you",
    );
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
