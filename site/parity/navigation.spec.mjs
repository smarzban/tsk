import { test, expect } from "@playwright/test";
test("project navigation, counted views and selection match the app", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page.locator("[data-task]").first()).toContainText("▸ ■ T12");
  await expect(page.locator("[data-task]").first()).toHaveCSS(
    "background-color",
    "rgba(0, 0, 0, 0)",
  );
  await expect(page.locator(".tsk-tab-group.is-on .tsk-tab")).toHaveCSS(
    "text-decoration-line",
    "underline",
  );
  await expect(page.locator(".tsk-tab-group.is-on")).toHaveCSS(
    "text-decoration-line",
    "none",
  );
  for (const inactive of await page
    .locator(".tsk-tab-group:not(.is-on)")
    .all()) {
    await expect(inactive).toHaveCSS("text-decoration-line", "none");
    const colors = await inactive.evaluate((el) => [
      getComputedStyle(el).color,
      getComputedStyle(document.querySelector(".tsk-tab-group.is-on")).color,
    ]);
    expect(colors[0]).not.toBe(colors[1]);
  }
  await page.locator('[data-tab="project"]').click();
  await expect(page.locator(".tsk-tab-group.is-on .tsk-tab")).toHaveCSS(
    "text-decoration-line",
    "underline",
  );
  await expect(page.locator(".tsk-tab-group.is-on .tsk-tab-arrow")).toHaveCSS(
    "text-decoration-line",
    "none",
  );
  await expect(page.locator("[data-filter]")).toHaveText("all ▾");
  await page.locator("[data-filter]").click();
  await expect(page.getByRole("dialog", { name: "Filter" })).toBeVisible();
  await page.getByText("Without a thread 4", { exact: false }).click();
  await expect(page.locator("[data-task]")).toHaveCount(4);
  await page.locator('[data-tab="projects"]').click();
  await expect(page.locator(".tsk-project-legend")).toContainText("NEEDS YOU");
  await expect(page.locator("[data-filter]")).toHaveText("Overview ▾");
  await page.locator("[data-project-row]").first().click();
  await expect(page.locator(".tsk-project-legend")).toBeVisible();
  await page.locator("[data-filter]").click();
  await page.getByText("#release 2", { exact: false }).click();
  await expect(page.locator("[data-task]")).toHaveCount(2);
  await page.locator("[data-chip]").click();
  await expect(
    page.getByRole("dialog", { name: "project", exact: true }),
  ).toBeVisible();
});

test("full terminal opens with board and task side by side", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await expect(page.locator("[data-split]")).toHaveClass(/is-full/);
  await expect(page.locator("[data-layout]").first()).toHaveAttribute(
    "data-layout",
    "full",
  );
  await expect(page.locator('[data-layout="full"]')).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.locator(".tsk-wide-split.is-split")).toBeVisible();
  await expect(
    page.locator(".tsk-board-surface [data-task]").first(),
  ).toBeVisible();
  await expect(page.locator(".tsk-task-header")).toBeVisible();
});

for (const scope of ["project", "projects"])
  test(`done drawer keeps the thread in ${scope}`, async ({ page }) => {
    await page.goto("http://127.0.0.1:4180/");
    await page.locator(`[data-tab="${scope}"]`).click();
    await page.locator("[data-filter]").click();
    await page
      .locator("[data-filter-option]")
      .filter({ hasText: "#auth" })
      .click();
    await page.locator("#board-demo").focus();
    await page.keyboard.press("z");
    await expect(page.locator("[data-task]")).toHaveCount(3);
    await expect(
      page.getByRole("button", { name: /Add a health check/ }),
    ).toHaveCount(0);
  });

test("landing seeds keep ready picks and inbox rows visible on desk and launchpad", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  for (const title of [
    "Plan the support handoff",
    "Clean up stale local branches",
    "Sort feedback from the pilot",
    "Record the retry runbook",
  ])
    await expect(
      page.locator("[data-task]").filter({ hasText: title }),
    ).toHaveCount(1);
  await page.locator('[data-tab="project"]').click();
  for (const title of [
    "Add audit events for admin changes",
    "Trace worker queue saturation",
    "Document regional failover checks",
  ])
    await expect(
      page.locator("[data-task]").filter({ hasText: title }),
    ).toHaveCount(1);
});

test("project counts, thread labels and counted menu order match the app", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator('[data-tab="projects"]').click();
  const project = page.locator('[data-project-row="launchpad"]');
  await expect(project.locator(":scope > span").nth(1)).toHaveText(
    "#api #auth #ops",
  );
  await expect(project.locator(":scope > span").nth(2)).toHaveText("2");
  await expect(project.locator(":scope > span").nth(3)).toHaveText("1");
  await expect(project.locator(":scope > span").nth(4)).toHaveText("5");
  await expect(project.locator(":scope > span").nth(5)).toHaveText("1");
  await page.locator('[data-tab="project"]').click();
  await page.locator("[data-filter]").click();
  await expect(page.locator("[data-filter-option]").nth(1)).toContainText(
    "#auth  3",
  );
  await expect(page.locator("[data-filter-option]").nth(2)).toContainText(
    "#ops  3",
  );
  await expect(page.locator("[data-filter-option]").nth(3)).toContainText(
    "#api  2",
  );
});

test("the Filter picker's threads and @assignees tabs combine like the app", async ({
  page,
}) => {
  await page.goto("http://127.0.0.1:4180/");
  await page.locator('[data-tab="project"]').click();
  await page.locator("[data-filter]").click();
  const picker = page.getByRole("dialog", { name: "Filter" });
  await expect(picker.locator(".tsk-filter-tab.is-on")).toHaveText("threads");
  await expect(page.locator("[data-filter-option]").first()).toContainText(
    "all",
  );
  await expect(page.locator("[data-filter-option]").first()).toContainText("✓");
  await page.locator("#board-demo").focus();
  await page.keyboard.press("Tab");
  await expect(picker.locator(".tsk-filter-tab.is-on")).toHaveText(
    "@assignees",
  );
  await page
    .locator("[data-filter-option]")
    .filter({ hasText: "unassigned" })
    .click();
  await expect(page.locator("[data-filter]")).toHaveText("unassigned ▾");
  await page.locator("[data-filter]").click();
  await page.locator('[data-filter-tab="threads"]').click();
  await page
    .locator("[data-filter-option]")
    .filter({ hasText: "#auth" })
    .click();
  await expect(page.locator("[data-filter]")).toHaveText("#auth unassigned ▾");
});
