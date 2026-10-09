import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
const root = resolve(import.meta.dirname, "..");
const fixture = JSON.parse(
  await readFile(resolve(root, "../tests/fixtures/demo-parity/store.json")),
);
const now = 1700000200000;
const tasks = fixture.tasks.map((t) => ({
  ...t,
  project: t.scope.project?.path.split("/").pop() ?? null,
  createdAt: t.created_at[0] * 1000,
  updatedAt: t.updated_at[0] * 1000,
  block: t.block && { ...t.block, at: t.block.at[0] * 1000 },
}));
const html = `<!doctype html><html data-theme="dark"><head><link rel="stylesheet" href="/src/styles/landing.css"><style>
:root { --font-mono: monospace; } body { margin:0; padding:20px; background:var(--bg); } #board-demo { width:max-content; outline:0; } #tsk-demo { box-sizing:content-box; width:78ch; height:24lh; min-height:0; padding:0; font:14px/20px monospace; }
</style></head><body><div id="board-demo" tabindex="0"><div id="tsk-demo" class="board"></div></div>
<script type="application/json" id="tsk-demo-fixture">${JSON.stringify({ tasks, now, selectedProject: "tsk-parity" })}</script><script type="module" src="/public/board-demo.js"></script></body></html>`;
createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.setHeader("Content-Type", "text/html");
      res.end(html);
      return;
    }
    const path = resolve(
      root,
      "." + new URL(req.url, "http://localhost").pathname,
    );
    if (!path.startsWith(root + "/")) {
      res.writeHead(403).end();
      return;
    }
    res.setHeader(
      "Content-Type",
      path.endsWith(".js") ? "text/javascript" : "text/css",
    );
    res.end(await readFile(path));
  } catch {
    res.writeHead(404).end();
  }
}).listen(4178, "127.0.0.1");
