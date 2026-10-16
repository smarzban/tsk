import assert from "node:assert/strict";
import test from "node:test";

import { parseCapture } from "../public/capture.js";

test("demo capture keeps a dotted thread name", () => {
  assert.equal(parseCapture("Ship it !t V0.0.6", null).thread, "v0.0.6");
});

test("demo capture refuses an illegal thread name", () => {
  assert.equal(parseCapture("Ship it !t start_name", null).thread, null);
});

test("demo capture still lowercases Site-Docs to site-docs", () => {
  assert.equal(parseCapture("Ship it !t Site-Docs", null).thread, "site-docs");
});
