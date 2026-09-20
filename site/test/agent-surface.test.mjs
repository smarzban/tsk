import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import test from "node:test";

import {
  formatTwin,
  htmlUrlForSlug,
  newestCommitIso,
  serializeJsonLd,
  slugFromDocsId,
  sourceFileName,
  stripFrontmatter,
  twinUrlForSlug,
} from "../src/lib/agent-docs.mjs";

const siteRoot = dirname(fileURLToPath(new URL("../package.json", import.meta.url)));
const repoRoot = dirname(siteRoot);
const docsDir = join(siteRoot, "src/content/docs/docs");
const distDir = join(siteRoot, "dist");

const read = (path) => readFile(path, "utf8");

function ensureDist() {
  if (existsSync(join(distDir, "index.html"))) return;
  const result = spawnSync("npm", ["run", "build"], {
    cwd: siteRoot,
    encoding: "utf8",
    stdio: "pipe",
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
}

async function docsFiles() {
  const names = await readdir(docsDir);
  return names.filter((name) => name.endsWith(".md") || name.endsWith(".mdx"));
}

test("markdown_twin_formatter_prefixes_title_html_source_and_updated", () => {
  const rendered = formatTwin({
    title: "CLI",
    slug: "cli",
    sourcePath: "site/src/content/docs/docs/cli.md",
    updatedIso: "2026-09-09T00:00:00.000Z",
    body: "The same store backs the board.\n",
  });
  assert.equal(
    rendered,
    [
      "# CLI",
      "",
      "- html: https://gettsk.sh/docs/cli/",
      "- source: https://github.com/smarzban/tsk/blob/main/site/src/content/docs/docs/cli.md",
      "- updated: 2026-09-09T00:00:00.000Z",
      "",
      "The same store backs the board.",
      "",
    ].join("\n"),
  );
  assert.equal(htmlUrlForSlug("index"), "https://gettsk.sh/docs/");
  assert.equal(twinUrlForSlug("index"), "https://gettsk.sh/docs/index.md");
  assert.equal(slugFromDocsId("docs/cli"), "cli");
  assert.equal(slugFromDocsId("docs/index"), "index");
  assert.equal(sourceFileName("src/content/docs/docs/index.mdx"), "index.mdx");
});

test("serializeJsonLd_escapes_script_breakout_and_round_trips", () => {
  const raw = serializeJsonLd({ headline: "</script><script>alert(1)</script>" });
  assert.doesNotMatch(raw, /<\/script>/i);
  assert.match(raw, /\\u003c/);
  assert.equal(JSON.parse(raw).headline, "</script><script>alert(1)</script>");
});

test("newestCommitIso_falls_back_to_mtime_when_git_history_is_missing", () => {
  const dir = mkdtempSync(join(tmpdir(), "tsk-twin-"));
  const filePath = join(dir, "orphan.md");
  writeFileSync(filePath, "hi\n");
  try {
    const iso = newestCommitIso(filePath);
    assert.match(iso, /^\d{4}-\d{2}-\d{2}T/);
    const mtimeIso = statSync(filePath).mtime.toISOString();
    assert.equal(iso, mtimeIso);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("dist_has_a_markdown_twin_for_every_docs_entry_with_matching_body", async () => {
  ensureDist();
  const files = await docsFiles();
  assert.ok(files.length >= 8, "docs collection should have the current pages");
  for (const fileName of files) {
    const slug = fileName.replace(/\.(md|mdx)$/, "");
    const twinPath = join(distDir, "docs", `${slug}.md`);
    assert.equal(existsSync(twinPath), true, `missing twin ${twinPath}`);
    const twin = await read(twinPath);
    const source = await read(join(docsDir, fileName));
    const body = stripFrontmatter(source).replace(/\s+$/, "");
    const afterHeader = twin.replace(/^# .*\n\n(?:- .*\n){3}\n/, "").replace(/\s+$/, "");
    assert.equal(afterHeader, body, `${fileName} twin body mismatch`);
    const title = source.match(/^title:\s*(.+)$/m)?.[1]?.replace(/^"|"$/g, "");
    assert.match(twin, new RegExp(`^# ${title}\\n`));
    assert.match(twin, /- html: https:\/\/gettsk\.sh\/docs\//);
    const sourceLine =
      slug === "agents"
        ? "- source: https://github.com/smarzban/tsk/blob/main/skills/tsk-cli/SKILL.md"
        : `- source: https://github.com/smarzban/tsk/blob/main/site/src/content/docs/docs/${fileName}`;
    assert.match(twin, new RegExp(sourceLine.replaceAll(".", "\\.")));
    assert.match(twin, /- updated: \d{4}-\d{2}-\d{2}T/);
  }
});

test("sitemap_xml_contains_no_md_urls", async () => {
  ensureDist();
  const names = await readdir(distDir);
  const sitemaps = names.filter((name) => name.startsWith("sitemap") && name.endsWith(".xml"));
  assert.ok(sitemaps.length > 0, "expected a built sitemap");
  for (const name of sitemaps) {
    const xml = await read(join(distDir, name));
    assert.doesNotMatch(xml, /\.md</);
    assert.doesNotMatch(xml, /\.md</);
    assert.doesNotMatch(xml, /docs\/[^<]+\.md/);
  }
});

test("docs_html_has_rel_alternate_to_the_twin_once_and_landing_has_none", async () => {
  ensureDist();
  const files = await docsFiles();
  for (const fileName of files) {
    const slug = fileName.replace(/\.(md|mdx)$/, "");
    const htmlPath =
      slug === "index"
        ? join(distDir, "docs", "index.html")
        : join(distDir, "docs", slug, "index.html");
    const html = await read(htmlPath);
    const href = twinUrlForSlug(slug);
    const tag = `<link rel="alternate" type="text/markdown" href="${href}">`;
    const matches = html.split(tag).length - 1;
    assert.equal(matches, 1, `${slug} should contain the alternate link once`);
  }
  const landing = await read(join(distDir, "index.html"));
  assert.doesNotMatch(landing, /rel="alternate" type="text\/markdown"/);
});

test("vercel serves the PowerShell installer as plain text", async () => {
  const config = JSON.parse(await read(join(siteRoot, "vercel.json")));
  const route = config.headers.find((entry) => entry.source === "/install.ps1");
  assert.ok(route, "missing /install.ps1 headers");
  assert.deepEqual(route.headers, [
    { key: "Content-Type", value: "text/plain; charset=utf-8" },
    { key: "X-Content-Type-Options", value: "nosniff" },
  ]);
});

test("vercel_json_has_no_accept_rewrites_and_no_regex_lookaheads", async () => {
  // Vercel's route parser rejects lookaheads inside :param patterns, and static
  // files win over rewrites anyway. Markdown is reached by .md URLs and rel=alternate.
  const config = JSON.parse(await read(join(siteRoot, "vercel.json")));
  assert.equal(config.rewrites, undefined, "no rewrites expected");
  for (const entry of [...(config.headers || []), ...(config.redirects || [])]) {
    assert.doesNotMatch(String(entry.source), /\(\?/, `${entry.source} uses a lookahead`);
  }
  const markdown = (config.headers || []).find((entry) => String(entry.source) === "/docs/(.*).md");
  assert.ok(markdown, "expected the Content-Type header for .md twins");
});

test("vercel_json_passes_vercels_own_route_validator", async () => {
  // Same check `vercel deploy` runs client-side before uploading.
  const { getTransformedRoutes } = await import("@vercel/routing-utils");
  const config = JSON.parse(await read(join(siteRoot, "vercel.json")));
  const result = getTransformedRoutes(config);
  assert.equal(result.error, null, result.error?.message);
});

test("robots_txt_allows_star_and_listed_crawlers_and_names_the_sitemap", async () => {
  const robots = await read(join(siteRoot, "public/robots.txt"));
  assert.match(robots, /^User-agent: \*\nAllow: \//m);
  for (const agent of [
    "GPTBot",
    "ClaudeBot",
    "Claude-SearchBot",
    "PerplexityBot",
    "Google-Extended",
    "Applebot-Extended",
    "Bingbot",
  ]) {
    assert.match(
      robots,
      new RegExp(`User-agent: ${agent}\nAllow: /`),
      `missing allow for ${agent}`,
    );
  }
  assert.match(robots, /Sitemap: https:\/\/gettsk\.sh\/sitemap-index\.xml/);
});

test("every_docs_entry_has_description_and_an_answer_first_paragraph", async () => {
  const files = await docsFiles();
  assert.ok(files.length >= 8, "docs collection should have the current pages");
  for (const fileName of files) {
    const source = await read(join(docsDir, fileName));
    const description = source.match(/^description:\s*(.+)$/m)?.[1]?.replace(/^"|"$/g, "").trim();
    assert.ok(description, `${fileName} needs a description`);
    const body = stripFrontmatter(source);
    const first = body.split(/\r?\n/).map((line) => line.trim()).find((line) => line.length > 0) || "";
    assert.ok(first, `${fileName} needs a first paragraph`);
    assert.equal(
      /^#|^-|^`|^:::|^</.test(first),
      false,
      `${fileName} should open on a paragraph, not ${first}`,
    );
  }
});

const DEFINITION_SENTENCE =
  "tsk is a terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.";

const SIDEBAR_TITLES = [
  "Overview",
  "Install",
  "Board",
  "Keys",
  "Capture",
  "Task page",
  "Steps",
  "tsk for agents",
  "CLI",
  "Storage",
];

test("llms_txt_is_under_60_lines_starts_with_definition_sentence_and_has_no_key_chords", async () => {
  const text = await read(join(siteRoot, "public/llms.txt"));
  const lines = text.replace(/\s+$/, "").split("\n");
  assert.ok(lines.length < 60, `llms.txt is ${lines.length} lines`);
  assert.equal(lines[0], "# tsk");
  assert.match(text, new RegExp(`^> ${DEFINITION_SENTENCE}$`, "m"));
  assert.doesNotMatch(text, /ctrl\+/i);
  assert.doesNotMatch(text, /prefix\+/);
  assert.doesNotMatch(text, /\bj\/k\b/);
});

test("every_llms_txt_gettsk_sh_link_except_agents_md_resolves_in_dist", async () => {
  ensureDist();
  const text = await read(join(siteRoot, "public/llms.txt"));
  const links = [...text.matchAll(/https:\/\/gettsk\.sh(\/[^\s)]+)/g)].map((match) => match[1]);
  assert.ok(links.length > 0, "expected gettsk.sh links");
  for (const path of links) {
    const filePath = join(distDir, path.replace(/^\//, ""));
    assert.equal(existsSync(filePath), true, `missing ${filePath} for ${path}`);
  }
});

test("llms_full_txt_contains_every_docs_title_in_sidebar_order", async () => {
  ensureDist();
  const text = await read(join(distDir, "llms-full.txt"));
  let cursor = 0;
  for (const title of SIDEBAR_TITLES) {
    const heading = `# ${title}`;
    const index = text.indexOf(heading, cursor);
    assert.ok(index >= 0, `missing title ${title}`);
    cursor = index + heading.length;
  }
});

function jsonLdBlocks(html) {
  return [...html.matchAll(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/g)].map(
    (match) => JSON.parse(match[1]),
  );
}

test("landing_jsonld_is_software_application_with_required_fields", async () => {
  ensureDist();
  const html = await read(join(distDir, "index.html"));
  const blobs = jsonLdBlocks(html);
  const app = blobs.find((blob) => blob["@type"] === "SoftwareApplication");
  assert.ok(app, "missing SoftwareApplication json-ld");
  assert.equal(app.name, "tsk");
  assert.equal(app.applicationCategory, "DeveloperApplication");
  assert.match(String(app.operatingSystem), /macOS/);
  assert.match(String(app.operatingSystem), /Linux/);
  assert.match(String(app.license), /MIT/i);
  assert.equal(app.codeRepository, "https://github.com/smarzban/tsk");
  const version = (await read(join(siteRoot, "src/version.mjs"))).match(/VERSION = '([^']+)'/)?.[1];
  assert.equal(app.softwareVersion, version);
  assert.equal(String(app.offers?.price), "0");
});

test("docs_jsonld_is_tech_article_with_headline_description_date_and_is_part_of", async () => {
  ensureDist();
  const files = await docsFiles();
  for (const fileName of files) {
    const slug = fileName.replace(/\.(md|mdx)$/, "");
    const htmlPath =
      slug === "index"
        ? join(distDir, "docs", "index.html")
        : join(distDir, "docs", slug, "index.html");
    const source = await read(join(docsDir, fileName));
    const title = source.match(/^title:\s*(.+)$/m)?.[1]?.replace(/^"|"$/g, "");
    const description = source.match(/^description:\s*(.+)$/m)?.[1]?.replace(/^"|"$/g, "");
    const html = await read(htmlPath);
    const article = jsonLdBlocks(html).find((blob) => blob["@type"] === "TechArticle");
    assert.ok(article, `${slug} missing TechArticle json-ld`);
    assert.equal(article.headline, title);
    assert.equal(article.description, description);
    assert.match(String(article.dateModified), /^\d{4}-\d{2}-\d{2}T/);
    assert.equal(article.isPartOf?.url, "https://gettsk.sh/");
    // Rich Results test flagged author and image as missing (optional). Both are real.
    assert.equal(article.author?.["@type"], "Organization");
    assert.equal(article.author?.name, "tsk");
    assert.equal(article.author?.url, "https://gettsk.sh/");
    assert.equal(article.image, "https://gettsk.sh/og.png");
  }
});

test("definition_sentence_appears_in_discovery_surfaces_and_readme_keeps_the_tagline", async () => {
  const files = [
    join(siteRoot, "src/pages/index.astro"),
    join(siteRoot, "public/llms.txt"),
    join(docsDir, "index.mdx"),
  ];
  for (const filePath of files) {
    const text = await read(filePath);
    assert.ok(
      text.includes(DEFINITION_SENTENCE),
      `${filePath} is missing the definition sentence`,
    );
  }
  const readme = await read(join(repoRoot, "README.md"));
  assert.ok(readme.includes("A terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them."));
});

const NOTICE =
  "tsk is a terminal task board for you and your agents: one shared queue, a TUI for you, a CLI for them.";

function syncAgentsPage() {
  const result = spawnSync("node", ["scripts/sync-agents-page.mjs"], {
    cwd: siteRoot,
    encoding: "utf8",
    stdio: "pipe",
  });
  assert.equal(result.status, 0, result.stderr || result.stdout || "sync-agents-page failed");
}

test("agents_page_notice_plus_skill_body_matches_source", async () => {
  syncAgentsPage();
  const generated = await read(join(docsDir, "agents.md"));
  const skill = await read(join(repoRoot, "skills/tsk-cli/SKILL.md"));
  const skillBody = stripFrontmatter(skill)
    .replace(/^# [^\n]+\n+/, "")
    .replace(/\s+$/, "");
  const body = stripFrontmatter(generated).replace(/\s+$/, "");
  assert.match(generated, /^title:\s*tsk for agents/m);
  assert.doesNotMatch(body, /^# /m);
  assert.equal(body, `${NOTICE}\n\n${skillBody}`);
});

test("dist_docs_agents_md_exists", async () => {
  ensureDist();
  assert.equal(existsSync(join(distDir, "docs/agents.md")), true);
  assert.equal(existsSync(join(distDir, "docs/agents/index.html")), true);
});

test("llms_txt_agents_md_link_resolves", async () => {
  ensureDist();
  assert.equal(existsSync(join(distDir, "docs/agents.md")), true);
});

test("definition_sentence_appears_in_agents_page", async () => {
  syncAgentsPage();
  const generated = await read(join(docsDir, "agents.md"));
  assert.ok(generated.includes(DEFINITION_SENTENCE));
});

test("agents_markdown_twin_points_source_and_updated_at_the_skill", async () => {
  ensureDist();
  const twin = await read(join(distDir, "docs/agents.md"));
  assert.match(
    twin,
    /- source: https:\/\/github.com\/smarzban\/tsk\/blob\/main\/skills\/tsk-cli\/SKILL.md/,
  );
  const expected = newestCommitIso(join(repoRoot, "skills/tsk-cli/SKILL.md"));
  assert.match(twin, new RegExp(`- updated: ${expected.replaceAll(".", "\\.")}`));
});

test("npm_start_and_deploy_paths_watch_the_skill", async () => {
  const pkg = JSON.parse(await read(join(siteRoot, "package.json")));
  assert.match(String(pkg.scripts.dev), /sync-agents-page/);
  assert.equal(pkg.scripts.start, "npm run dev");
  const siteYml = await read(join(repoRoot, ".github/workflows/site.yml"));
  const vercelYml = await read(join(repoRoot, ".github/workflows/vercel.yml"));
  assert.match(siteYml, /skills\/\*\*/);
  assert.match(vercelYml, /skills\/\*\*/);
  const vercelJson = JSON.parse(await read(join(siteRoot, "vercel.json")));
  assert.match(String(vercelJson.ignoreCommand), /\.\.\/skills/);
});
