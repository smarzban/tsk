import assert from "node:assert/strict";
import test from "node:test";

import { parseCapture } from "../public/capture.js";

test("demo capture keeps a dotted thread name", () => {
  assert.equal(parseCapture("Ship it !t V0.0.6", null).thread, "v0.0.6");
});

test("demo capture refuses an illegal thread name", () => {
  const parsed = parseCapture("Ship it !t start_name", null);
  assert.equal(parsed.thread, null);
  assert.equal(parsed.refuse, "thread: use letters, numbers, hyphens, and dots");
});

test("demo capture still lowercases Site-Docs to site-docs", () => {
  assert.equal(parseCapture("Ship it !t Site-Docs", null).thread, "site-docs");
});

test("demo capture lifts assignee, base, and after tokens", () => {
  assert.deepEqual(
    parseCapture("Ship it !a Reviewer !b main !w T12 !w 12", "atlas"),
    {
      title: "Ship it",
      project: "atlas",
      thread: undefined,
      assignee: "reviewer",
      base: "main",
      due: undefined,
      after: [12],
      refuse: "",
    },
  );
});

test("demo capture leaves a hash word in the title after a bare token", () => {
  assert.deepEqual(parseCapture("Ship it !p #atlas", null), {
    title: "Ship it #atlas",
    project: null,
    thread: undefined,
    assignee: undefined,
    base: undefined,
    due: undefined,
    after: [],
    refuse: "",
  });
});

test("demo capture refuses a bad assignee and a non-number !w", () => {
  assert.equal(
    parseCapture("Ship it !a start_name", null).refuse,
    "assignee: use letters, numbers, hyphens, and dots",
  );
  assert.equal(
    parseCapture("Ship it !w abc", null).refuse,
    "!w needs a task number like T12",
  );
});

test("demo capture refuses a base on a desk task, like the board", () => {
  assert.equal(
    parseCapture("Buy coffee !p !b main", "atlas").refuse,
    "base requires a project task",
  );
  assert.equal(
    parseCapture("Buy coffee !b main", null).refuse,
    "base requires a project task",
  );
  assert.equal(parseCapture("Buy coffee !b main !p atlas", null).refuse, "");
});

test("demo capture lifts a due day and refuses a bad or missing one", () => {
  const parsed = parseCapture("Ship it !d 2026-11-30 soon", null);
  assert.equal(parsed.title, "Ship it soon");
  assert.equal(parsed.due, "2026-11-30");
  assert.equal(parsed.refuse, "");
  assert.equal(
    parseCapture("Ship !d 2026-02-30", null).refuse,
    "due 2026-02-30: that day does not exist in the calendar",
  );
  assert.equal(
    parseCapture("Ship !d tomorrow", null).refuse,
    "due tomorrow: a due date is a day written YYYY-MM-DD",
  );
  assert.equal(
    parseCapture("Ship !d", null).refuse,
    "!d needs a day like 2026-11-30",
  );
});
