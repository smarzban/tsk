import { test, expect } from "@playwright/test";

test("board illustration uses an arrow without a filled selection", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  const row = page.locator(".screen .s-task.is-sel");
  await expect(row).toContainText("▸ ▲ T31");
  await expect(row).toHaveCSS("background-image", "none");
  await expect(row).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(row.locator(".sel-text")).toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );
});

for (const scope of ["project", "projects"]) {
  test(`archived drawer respects the ${scope} thread filter`, async ({
    page,
  }) => {
    await page.goto("http://127.0.0.1:4180/");
    await page.locator(`[data-tab="${scope}"]`).click();
    await page.locator("[data-filter]").click();
    await page
      .locator("[data-filter-option]")
      .filter({ hasText: "#api" })
      .click();
    await page.locator("#board-demo").focus();
    await page.keyboard.press("z");
    await page.locator("[data-archived-header]").dblclick();
    await expect(
      page
        .locator("[data-task]")
        .filter({ hasText: "Prototype a GraphQL gateway" }),
    ).toHaveCount(1);
    await page.locator("[data-filter]").click();
    await page
      .locator("[data-filter-option]")
      .filter({ hasText: "#auth" })
      .click();
    await expect(
      page
        .locator("[data-task]")
        .filter({ hasText: "Prototype a GraphQL gateway" }),
    ).toHaveCount(0);
    await expect(page.locator("[data-archived-header]")).toHaveCount(0);
  });
}

test("project picker lists destinations and opens the chosen project", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator("[data-chip]").click();
  expect(
    await page
      .locator("[data-pick]")
      .evaluateAll((rows) => rows.map((row) => row.dataset.pick)),
  ).toEqual(["desk", "launchpad", "website"]);
  await page.locator('[data-pick="website"]').click();
  await expect(page.locator('[data-tab="project"]')).toHaveText("website");
  await expect(page.locator('[data-tab="project"]')).toHaveClass(/is-on/);
  await expect(page.locator('[role="dialog"]')).toHaveCount(0);
});
