import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { extname, join } from "node:path";
import test from "node:test";

import { parseCapture } from "../public/capture.js";

const read = (path) => readFile(new URL(path, import.meta.url), "utf8");

test("no tracked file still points at the old preview domain", async () => {
  const repoRoot = new URL("../../", import.meta.url).pathname;
  // docs/specs is intentionally out of scope; the rest are untracked build or
  // local-only trees whose contents never ship.
  const skipDirs = new Set([
    ".git", ".review-panel", ".astro", "node_modules", "target", "dist",
    ".vercel", "specs", "technical",
  ]);
  const skipExt = new Set([".png", ".svg", ".ico", ".jpg", ".lock"]);
  // Assembled so this file itself does not contain the literal domain.
  const oldDomain = ["tsk-gules", "vercel", "app"].join(".");
  const offenders = [];
  const walk = async (dir) => {
    for (const entry of await readdir(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (!skipDirs.has(entry.name)) await walk(join(dir, entry.name));
      } else if (!skipExt.has(extname(entry.name))) {
        const text = await readFile(join(dir, entry.name), "utf8").catch(() => "");
        if (text.includes(oldDomain)) offenders.push(join(dir, entry.name));
      }
    }
  };
  await walk(repoRoot);
  assert.deepEqual(offenders, [], "stale preview-domain references remain");
});

test("crate, plugin, lockfile, and site share one release version", async () => {
  const [cargo, plugin, lockfile, siteVersion] = await Promise.all([
    read("../../Cargo.toml"),
    read("../../herdr-plugin.toml"),
    read("../../Cargo.lock"),
    read("../src/version.mjs"),
  ]);
  const cargoVersion = cargo.match(/^version = "([^"]+)"/m)?.[1];
  const pluginVersion = plugin.match(/^version = "([^"]+)"/m)?.[1];
  const lockVersion = lockfile.match(/\[\[package\]\]\nname = "tsk-tui"\nversion = "([^"]+)"/)?.[1];
  const renderedVersion = siteVersion.match(/VERSION = '([^']+)'/)?.[1];
  const changelog = await read("../../CHANGELOG.md");
  const releasedVersion = changelog.match(/^## v(\d+\.\d+\.\d+)/m)?.[1];
  assert.equal(cargoVersion, releasedVersion, "Cargo.toml must match the top CHANGELOG section");
  assert.deepEqual([pluginVersion, lockVersion, renderedVersion], [cargoVersion, cargoVersion, cargoVersion]);
});

test("the Astro site and sitemap build on the canonical URL", async () => {
  const config = await read("../astro.config.mjs");
  assert.match(config, /site: 'https:\/\/gettsk\.sh'/);
});

test("Vercel ignores unchanged files relative to the site root", async () => {
  const config = JSON.parse(await read("../vercel.json"));
  assert.equal(config.ignoreCommand, "git diff --quiet HEAD^ HEAD -- . ../skills");
});

test("demo capture keeps an absolute project path verbatim", () => {
  assert.deepEqual(parseCapture("Ship it !p /workspace/herdr-tsk !t Site-Docs", null), {
    title: "Ship it",
    project: "/workspace/herdr-tsk",
    thread: "site-docs",
  });
});

test("the shared theme script is loaded once from the Starlight head", async () => {
  const config = await read("../astro.config.mjs");
  const themeSelect = await read("../src/components/ThemeSelect.astro");
  assert.match(config, /attrs: \{ src: ['"]\/theme\.js['"] \}/);
  assert.doesNotMatch(themeSelect, /theme\.js/);
});

test("demo rows lead with copyable T task identifiers", async () => {
  const demo = await read("../public/board-demo.js");
  assert.match(demo, /let n = 12;/);
  assert.match(demo, /number: n\+\+,/);
  assert.doesNotMatch(demo, /bits\.push\(String\(task\.number\)\)/);
  assert.match(demo, /data-copy-task=/);
  assert.match(demo, /T\$\{task\.number\}/);
  assert.doesNotMatch(demo, /#\$\{task\.number\}/);
});

test("docs and demo describe the wide stage slider and threshold", async () => {
  const board = await read("../src/content/docs/docs/board.md");
  const keys = await read("../src/content/docs/docs/keys.md");
  const taskPage = await read("../src/content/docs/docs/task-page.md");
  const demo = await read("../public/board-demo.js");
  const styles = await read("../src/styles/landing.css");

  assert.match(board, /110 usable columns/);
  assert.match(board, /four-stage slider/);
  assert.match(board, /\| Task \| Task details beside a narrow board rail \|/);
  assert.doesNotMatch(board, /bordered|cyan/);
  assert.match(keys, /Wide stage slider/);
  assert.match(keys, /\| Full screen \| No change \| Task with rail \|/);
  assert.doesNotMatch(keys, /cyan/);
  assert.match(taskPage, /110 usable columns/);
  assert.match(taskPage, /Click the task's `T` number to copy it/);
  assert.doesNotMatch(taskPage, /bordered panel/);
  assert.match(demo, /const WIDE_SPLIT_MIN_COLUMNS = 110;/);
  assert.match(demo, /function stageRight\(\)/);
  assert.match(demo, /function stageLeft\(\)/);
  assert.match(demo, /state\.stageOrigin = state\.stage;/);
  assert.match(demo, /class="tsk-wide-split is-rail"/);
  assert.match(demo, /tsk-rule-column/);
  assert.match(demo, /board ▸ task    → task · ← close · enter open/);
  assert.match(styles, /\.tsk-wide-split\.is-split \{\s*grid-template-columns: minmax\(0, 2fr\) 1px minmax\(0, 3fr\);/);
  assert.match(styles, /\.tsk-wide-split\.is-rail \{\s*grid-template-columns: 32ch 1px/);
});

test("demo keeps project navigation, attribution, and search contracts", async () => {
  const demo = await read("../public/board-demo.js");
  assert.match(demo, /need: open/);
  assert.doesNotMatch(demo, /!t\.project && \(t\.status === "blocked" \|\| t\.status === "review"\)/);
  assert.match(demo, /state\.selectedProject = name;/);
  assert.match(demo, /const text = tab === "project" \? state\.selectedProject : label/);
  assert.match(demo, /state\.tasks\.filter\(\(t\) => t\.project && !t\.archived\)/);
  assert.match(demo, /id: `project:\${name}`/);
  assert.match(demo, /id: "nav:archived"/);
  assert.match(demo, /const row = selectedRow\(\);/);
  assert.match(demo, /row\?\.kind === "project"/);
  assert.match(demo, /selectedRow\(\)\?\.kind !== "task"/);
  assert.match(demo, /id="tsk-project-search"/);
  assert.ok(demo.includes('e.key === "/"'));
  assert.match(demo, /state\.searchQuery/);
  assert.match(demo, /taskMatchesSearch/);
  assert.match(demo, /searchPinned/);
  assert.match(
    demo,
    /if \(e\.key === "Escape"\) \{\s*e\.preventDefault\(\);\s*if \(state\.searchPinned\) \{[\s\S]*?\} else if \(taskPageActive\) \{/,
    "pinned search must clear before normal task-page Escape behavior resumes",
  );
  assert.match(demo, /data-project-row=/);
});

test("demo matches the quick-add, peek, and group-toggle contracts", async () => {
  const demo = await read("../public/board-demo.js");
  const landing = await read("../src/styles/landing.css");
  assert.match(demo, /if \(e\.key === "Enter" && !e\.ctrlKey && !e\.altKey && !e\.metaKey\)/);
  assert.match(demo, /saveDraft\(e\.shiftKey\)/);
  assert.match(demo, /id: "save", label: "enter save"/);
  assert.match(demo, /id: "details", label: "tab details"/);
  assert.match(demo, /id: "close", label: "esc close"/);
  // Closing the page by click must drop any open title/notes draft, or the next task
  // opens in edit mode carrying it; the verbs hide while an editor owns the footer.
  assert.match(
    demo,
    /if \(id === "close"\) \{\s*state\.overlay = null;\s*state\.editField = null;\s*state\.editDraft = "";\s*leaveTaskPage/,
  );
  assert.match(
    demo,
    /function pageVerbBar\(\)[\s\S]{0,240}?steps\.editor \|\| steps\.dirty \|\| state\.editField/,
  );
  assert.match(demo, /function runHint/);
  assert.match(demo, /data-hint=/);
  assert.match(demo, /"inbox"\}<\/span> · <span class="count">\$\{row\.count\}<\/span>/);
  assert.match(demo, /\["views & find", "z \/ D", "done drawer"/);
  assert.match(demo, /state\.helpQ/);
  assert.match(demo, /search keys or actions/);
  assert.match(demo, /tsk-help-divider/);
  assert.match(demo, /tsk-help-row/);
  assert.match(landing, /\.tsk-help-row span:last-child/);
  assert.match(landing, /overflow-wrap: anywhere/);
  assert.match(demo, /e\.key === "z" \|\| e\.key === "D"/);
  assert.doesNotMatch(demo, /saveDraft\(e\.ctrlKey \|\| e\.metaKey\)/);
  assert.doesNotMatch(demo, /thread #\$\{task\.thread\}|scope \$\{projectName\(task\)\}|created \$\{age\(/);
  assert.match(demo, /function toggleAllGroups\(\)/);
  assert.match(demo, /id: "groups", label: "toggle groups"/);
  assert.match(demo, /if \(e\.key === "g" && !e\.altKey && !e\.ctrlKey && !e\.metaKey\)/);
  assert.doesNotMatch(demo, /pushTask\(t, 1\)/);
  assert.match(landing, /\.demo-invitation p \{[^}]*max-width: none/);
});

test("the saved theme survives a visit to the docs", async () => {
  const config = await read("../astro.config.mjs");
  const init = config.indexOf("var k='tsk-theme'");
  const loader = config.indexOf("attrs: { src: '/theme.js' }");
  assert.ok(init > 0 && loader > init, "the inline theme init must run before theme.js");
  const theme = await read("../public/theme.js");
  assert.match(theme, /saved = localStorage\.getItem\(KEY\)/);
});

test("docs paint keys as keycaps and leave flags as code", async () => {
  const { isKeyName } = await import("../src/plugins/rehype-kbd.mjs");
  for (const key of ["ctrl+s", "Shift+Enter", "Enter", "Esc", "→", "j", "P", "+", ":", "?"]) {
    assert.ok(isKeyName(key), `${key} is a key`);
  }
  for (const code of ["--json", "-", "tsk add", "~/.tsk", "T30", "T", "i", "n", "global", "ctrl+"]) {
    assert.ok(!isKeyName(code), `${code} is not a key`);
  }
});

test("docs open on the two-party board and let agents set status", async () => {
  const overview = await read("../src/content/docs/docs/index.mdx");
  assert.match(overview, /a terminal task board for you and your agents/);
  assert.match(overview, /tsk status/);
  assert.doesNotMatch(overview, /Done is a human verb/);
  const cli = await read("../src/content/docs/docs/cli.md");
  assert.match(cli, /## For agents/);
  assert.match(cli, /tsk status <task> done/);
});

test("docs keep the phone layout until the right TOC fits", async () => {
  const css = await read("../src/styles/starlight.css");
  assert.match(css, /@media \(min-width: 50rem\) and \(max-width: 71\.99rem\)/);
  const links = await read("../src/components/DocsLinks.astro");
  assert.doesNotMatch(links, /install/);
});

test("attribution is peek-only in demo and static anatomy", async () => {
  const demo = await read("../public/board-demo.js");
  const page = await read("../src/pages/index.astro");
  const styles = await read("../src/styles/landing.css");
  const guide = await read("../src/content/docs/docs/board.md");
  assert.doesNotMatch(demo, /<span class="meta">/);
  assert.match(
    demo,
    /if \(task\.assignee\) bits\.push\(`@\$\{task\.assignee\}`\);\s*if \(task\.thread\) bits\.push\(`#\$\{task\.thread\}`\);\s*if \(task\.project\) bits\.push\(projectName\(task\)\);/,
    "peek metadata must be @assignee · #thread · project in that order",
  );
  assert.match(styles, /\.tsk-attribution\s*\{[^}]*white-space: pre-wrap;[^}]*overflow-wrap: anywhere;/);
  assert.match(styles, /\.tsk-row-main\s*\{[^}]*padding-right: 2ch;/);
  assert.doesNotMatch(page, /class="r meta"/);
  assert.match(page, /└─ tsk/);
  assert.doesNotMatch(guide, /section headers, row meta/);
});

test("landing header uses a goto menu plus docs and github", async () => {
  const page = await read("../src/pages/index.astro");
  assert.match(page, /class="goto"/);
  assert.match(page, /href: '#demo', label: 'demo'/);
  assert.doesNotMatch(page, /href: '#keys'/);
  assert.doesNotMatch(page, /id="keys"/);
  assert.ok(page.indexOf('id="why"') < page.indexOf('id="demo"'));
  assert.ok(page.indexOf('id="demo"') < page.indexOf('id="board"'));
  assert.ok(page.indexOf('id="board"') < page.indexOf('id="status"'));
  assert.doesNotMatch(page, /class="nav-demo"/);
  assert.doesNotMatch(page, /class="nav-icon"/);
  assert.match(page, /class="nav-text" href="\/docs\/">docs</);
  assert.match(page, /class="nav-text" href=\{REPO\}[^>]*>github</);
  assert.doesNotMatch(page, /class="nav-text" href="\/docs\/install\//);
  assert.match(page, /class="nav-dot"/);
  assert.match(page, /class="theme-toggle"/);
  const css = await read("../src/styles/landing.css");
  assert.match(css, /\.goto-chip/);
  assert.doesNotMatch(css, /\.theme-toggle \{[^}]*border-radius:\s*50%/);
  assert.doesNotMatch(css, /nav-demo/);
  assert.doesNotMatch(css, /\.nav-icon\b/);
  assert.doesNotMatch(css, /\.keygrid\b/);
  assert.doesNotMatch(css, /\.cta\b/);
  assert.doesNotMatch(css, /\.btn-primary\b/);
  const js = await read("../public/landing.js");
  assert.match(js, /\[data-goto\]/);
});

test("hero has the install one-liner and a jump to the demo", async () => {
  const page = await read("../src/pages/index.astro");
  assert.match(page, /class="hero-install"/);
  assert.match(page, /curl -fsSL https:\/\/gettsk.sh\/install.sh \| sh/);
  assert.match(page, /href="#demo">Try the board/);
  assert.match(page, /href="\/docs\/install\/">install guide →/);
  assert.match(page, /macOS &amp; Linux · MIT/);
  const meta = page.match(/class="hero-install-meta">([\s\S]*?)<\/p>/)[1];
  assert.ok(meta.indexOf("Try the board") < meta.indexOf("install guide"));
});

test("hero board pins reveal callout copy", async () => {
  const page = await read("../src/pages/index.astro");
  assert.match(page, /data-board-pins/);
  assert.match(page, /data-pin="5"/);
  assert.doesNotMatch(page, /data-pin="6"/);
  assert.equal((page.match(/data-pin="\d"/g) || []).length, 5);
  // One rule, then the scope line, then the verb bar: the footer the TUI paints.
  assert.match(page, /s-footrule[\s\S]*<span class="l meta">desk<\/span>/);
  assert.equal((page.match(/s-footrule/g) || []).length, 1);
  assert.doesNotMatch(page, /<span class="l meta">2 done<\/span>/);
  // The fixture's verb bar minus ctrl+b, so it sits on one line beside the pin gutter.
  const verbBar = await read("../../tests/fixtures/queue_board/board.txt");
  assert.ok(verbBar.includes("enter open · ctrl+d done · ctrl+n next · ctrl+o inbox · ctrl+b block · ? help"));
  assert.match(page, /enter open · ctrl\+d done · ctrl\+n next · ctrl\+o inbox · \? help/);
  const css = await read("../src/styles/landing.css");
  assert.match(css, /\.hero-side \.screen \{[^}]*white-space: pre-wrap/);
  assert.match(css, /\.hero-side \.s-row \.l \{[^}]*text-overflow: clip/);
  assert.doesNotMatch(css, /\.goto-label \{\s*display: none/);
  assert.match(page, /Three tabs, always on screen/);
  assert.match(page, /title: 'A task\.'/);
  assert.doesNotMatch(page, /title: 'A row\.'/);
  assert.match(page, /aria-label=\{pinLabel\(1\)\}/);
  const js = await read("../public/landing.js");
  assert.match(js, /data-board-pins/);
  assert.match(js, /focusout/);
});

test("the live-demo caption spans the board width", async () => {
  const css = await read("../src/styles/landing.css");
  assert.match(css, /\.stage-note \{/);
  assert.doesNotMatch(css, /\.stage-note \{[^}]*max-width/);
});

test("demo fields stay 16px on touch so phones do not zoom", async () => {
  const page = await read("../src/pages/index.astro");
  const css = await read("../src/styles/landing.css");
  assert.match(page, /<meta name="viewport" content="width=device-width, initial-scale=1" \/>/);
  assert.doesNotMatch(page, /maximum-scale/);
  assert.match(
    css,
    /@media \(pointer: coarse\) \{[\s\S]*?\.board input\.tsk-field,[\s\S]*?\.board textarea\.tsk-field \{[\s\S]*?font-size:\s*max\(16px,\s*1em\)/,
  );
});

test("landing agents section points at markdown twins", async () => {
  const page = await read("../src/pages/index.astro");
  assert.match(page, /class="agent-handoff"/);
  assert.match(page, /Every docs page is\s+also Markdown/);
  assert.match(page, /https:\/\/gettsk\.sh\/docs\/cli\.md/);
  assert.match(page, /https:\/\/gettsk\.sh\/docs\/agents\.md/);
  assert.doesNotMatch(page, /gettsk\.sh\/llms\.txt/);
  assert.match(page, /class="cli-side"[\s\S]*tsk setup grok/);
  assert.match(page, /class="cli-main"[\s\S]*id="agents"/);
  assert.doesNotMatch(page, /Hand it to your agent/);
  assert.match(page, /id="agents" aria-labelledby="agents-heading"/);
  assert.match(page, /<h3 class="side-title" id="agents-heading">Paste this into your agent<\/h3>/);
  assert.ok(page.indexOf("class=\"agent-handoff\"") < page.indexOf("Everything an agent needs"));
  assert.ok(page.indexOf('id="cli"') < page.indexOf('id="agents"'));
  assert.ok(page.indexOf('id="agents"') < page.indexOf('id="install"'));
  assert.doesNotMatch(page, /class="band[^"]*" id="agents"/);
});

test("quickstarts use the installer, Herdr setup, and task capture", async () => {
  for (const file of ["../../README.md", "../src/content/docs/docs/install.md", "../src/pages/index.astro"]) {
    const text = await read(file);
    assert.ok(text.includes("curl -fsSL https://gettsk.sh/install.sh | sh"), file);
    for (const command of ["brew install smarzban/tap/tsk", "tsk setup herdr", "herdr server reload-config", 'tsk add -t "your task title"', "prefix+t", "prefix+a"]) {
      assert.ok(text.includes(command), `${file}: ${command}`);
    }
    assert.ok(!text.includes("-o install-tsk.sh"), file);
  }
});
