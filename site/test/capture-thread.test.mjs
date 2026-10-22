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
    parseCapture("Ship it !a Reviewer !b main !w T12 !w 12", null),
    {
      title: "Ship it",
      project: null,
      thread: undefined,
      assignee: "reviewer",
      base: "main",
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
