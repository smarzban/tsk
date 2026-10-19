// Shared helpers for docs markdown twins. Keep this free of astro: imports so
// site tests can load it with node --test.

import { spawnSync } from "node:child_process";
import { statSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";

export const SITE = "https://gettsk.sh";
export const DEFINITION_SENTENCE =
  "tsk is a terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.";
export const SIDEBAR_SLUGS = [
  "index",
  "install",
  "board",
  "dispatch",
  "keys",
  "capture",
  "task-page",
  "steps",
  "agents",
  "cli",
  "storage",
];
export const SOURCE_REPO_BASE = "https://github.com/smarzban/tsk/blob/main";
export const AGENTS_SKILL_REPO_PATH = "skills/tsk-cli/SKILL.md";
export function agentsSkillFsPath() {
  return resolve(process.cwd(), "../skills/tsk-cli/SKILL.md");
}

export function slugFromDocsId(id) {
  const value = String(id);
  if (value === "docs" || value === "docs/index" || value === "index") return "index";
  const rest = value.replace(/^docs\//, "");
  return rest === "" ? "index" : rest;
}

export function htmlUrlForSlug(slug) {
  return slug === "index" ? `${SITE}/docs/` : `${SITE}/docs/${slug}/`;
}

export function twinUrlForSlug(slug) {
  return `${SITE}/docs/${slug}.md`;
}

export function sourceFileName(filePath) {
  const normalized = String(filePath).replaceAll("\\", "/");
  const parts = normalized.split("/");
  return parts[parts.length - 1];
}

export function stripFrontmatter(source) {
  const match = String(source).match(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/);
  if (!match) return String(source);
  return String(source).slice(match[0].length).replace(/^\r?\n/, "");
}

export function newestCommitIso(filePath) {
  const absolute = resolve(process.cwd(), filePath);
  const result = spawnSync(
    "git",
    ["log", "--format=%ct", "--max-count=1", "--", basename(absolute)],
    { cwd: dirname(absolute), encoding: "utf-8" },
  );
  const timestamp = Number(String(result.stdout || "").trim());
  if (!result.error && Number.isFinite(timestamp) && timestamp > 0) {
    return new Date(timestamp * 1000).toISOString();
  }
  return statSync(absolute).mtime.toISOString();
}

export function serializeJsonLd(value) {
  return JSON.stringify(value).replace(/</g, "\\u003c");
}

export function twinSource(slug, filePath) {
  if (slug === "agents") {
    return {
      sourcePath: AGENTS_SKILL_REPO_PATH,
      updatedIso: newestCommitIso(agentsSkillFsPath()),
    };
  }
  return {
    sourcePath: `site/src/content/docs/docs/${sourceFileName(filePath)}`,
    updatedIso: newestCommitIso(filePath),
  };
}

export function formatTwin({ title, slug, sourcePath, fileName, updatedIso, body }) {
  const trimmed = String(body).replace(/^\uFEFF/, "").replace(/\s+$/, "");
  const path = sourcePath || `site/src/content/docs/docs/${fileName}`;
  return [
    `# ${title}`,
    "",
    `- html: ${htmlUrlForSlug(slug)}`,
    `- source: ${SOURCE_REPO_BASE}/${path}`,
    `- updated: ${updatedIso}`,
    "",
    trimmed,
    "",
  ].join("\n");
}
