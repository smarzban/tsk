#!/usr/bin/env node
// Generate the gitignored /docs/agents/ and /docs/coordinator/ sources from the skills.

import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { SKILL_PAGES, stripFrontmatter } from "../src/lib/agent-docs.mjs";

const siteRoot = dirname(fileURLToPath(new URL("../package.json", import.meta.url)));
const repoRoot = dirname(siteRoot);
const destDir = join(siteRoot, "src/content/docs/docs");

mkdirSync(destDir, { recursive: true });
for (const [slug, page] of Object.entries(SKILL_PAGES)) {
  const skill = readFileSync(join(repoRoot, page.skillPath), "utf8");
  const body = stripFrontmatter(skill)
    .replace(/^﻿/, "")
    .replace(/^# [^\n]+\n+/, "")
    .replace(/\s+$/, "");
  const dest = join(destDir, `${slug}.md`);
  const text = [
    "---",
    `title: ${page.title}`,
    `description: ${page.description}`,
    "---",
    "",
    page.lead,
    "",
    body,
    "",
  ].join("\n");
  // Tests run this beside parallel readers of the docs: skip an unchanged page, and replace
  // a changed one whole so no reader sees it half written.
  if (existsSync(dest) && readFileSync(dest, "utf8") === text) continue;
  writeFileSync(`${dest}.tmp`, text);
  renameSync(`${dest}.tmp`, dest);
}
