# tsk site

Marketing page and **user guide** for tsk. Lives in this repo as `site/`, the same
split herdr uses for its own website. Astro + Starlight; the homepage is a custom page, not the Starlight index.
How-to pages are `src/content/docs/docs/`.

This directory is **not** part of the `tsk` binary. `cargo build` ignores it.

## Landing page

`src/pages/index.astro` is one page with its styles in `src/styles/landing.css`
and behaviour in `public/landing.js`. One mono face (JetBrains Mono), colour
tokens on `:root` with a light override on `html[data-theme="light"]`, and one
accent. The live demo is `public/board-demo.js`; the page only sizes it (the
beside-an-agent / full-terminal layouts and the divider) and asks for a stage
through the `tsk:set-stage` event. `public/og.png` is a rendered still; regenerate
it if the headline or tagline changes.

## Local

```bash
cd site
npm install
npm run dev
```

## Build

```bash
cd site
npm run build
```

Output is `site/dist/`.

## Deploy

Vercel should use this repository with **Root Directory** `site`.
`vercel.json` pins install / build / output. Production is
https://gettsk.sh.

Ignore builds that do not touch `site/**` or `skills/**` so a Rust-only
commit does not rebuild the landing page (see `vercel.json`).
