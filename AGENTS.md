# AGENTS.md

Standing rules for agents working in this repo. Product behaviour is documented on the
site, not here: when you need to know what the board does, read the docs; when you need
to know how to change it safely, read this file.

## Routing

Would this instruction make sense to a stranger who cloned this repo? If no, it
belongs in `AGENTS.local.md`.

A gitignored `AGENTS.local.md` may exist beside this file; if present, read it
before starting. Edits go to `AGENTS.md` or `AGENTS.local.md`, never `CLAUDE.md`
(a frozen pointer). The `@AGENTS.local.md` line below is Claude Code include syntax;
other agents read the file manually.

If private-routed content appears and no `AGENTS.local.md` exists yet, create one.
The committed `.gitignore` already covers it.

A gitignored `HANDOFF.md` may hold this clone's live working state. Read it first if
present, and refresh it when you stop mid-work.

@AGENTS.local.md

## What this is

**tsk** is a terminal task board: a queue board for capture and human-status verbs
(`open` · `ready` · `started` · `blocked` · `review` · `done`). It ships as the herdr plugin
`herdr-tsk`; the built binary (`tsk`) also runs standalone.

For scriptable board work use `tsk add`, `tsk list`, `tsk status`, `tsk edit`,
`tsk steps`; read `skills/tsk-cli/SKILL.md` first for retry and scope-check rules.

### Product reference

| Topic | Source of truth |
| --- | --- |
| Board surfaces, tabs, drawer, archive, wide slider | `site/src/content/docs/docs/board.md` |
| Task page, editing, steps | `task-page.md`, `steps.md` |
| Quick-add and `tsk capture` | `capture.md` |
| Every key of every surface | `keys.md`, code in `src/ui/input.rs` |
| CLI verbs, exit codes, error codes | `cli.md`, code in `src/cli/`, glossary in `CONTEXT.md` |
| Storage, env vars, update check | `storage.md` |
| Install, Homebrew, `tsk setup herdr` | `install.md`, `packaging/README.md` |
| Agent skill | `skills/tsk-cli/SKILL.md` (`tsk guide`, `/docs/agents/`) |
| Exact rendered output | golden fixtures in `tests/fixtures/` (`tests/queue_board_render.rs`) |

If the docs and the code disagree, the code is the bug or the docs are; fix one in the
same PR, never leave them apart.

### Where things live

| Path | What |
| --- | --- |
| `src/app.rs` | keyboard boundary, save recovery allowlist |
| `src/ui/` | chrome: `board/` (model · apply · commands · chrome · draw), `input`, `mouse`, `render`, `edit` (the one wrap engine), `markdown` |
| `src/cli/` | headless verbs, `parser`, `router`, `presenter` |
| `src/store.rs`, `src/domain/` | `tsk.json` format, migrations, trash |
| `src/setup.rs`, `src/setup/`, `src/setup_agent.rs` | `tsk setup herdr`, agent skill install |
| `src/guides.rs`, `src/announcements.rs`, `src/delivery.rs` | seeded notice tasks |
| `src/update.rs` | release check, `tsk update` |
| `scripts/open-board.sh`, `scripts/open-capture.sh`, `scripts/*.ps1` | Herdr launchers (embedded via `src/setup.rs`) |
| `scripts/release.py`, `.github/workflows/release.yml`, `packaging/` | native packaging |
| `site/` | landing page + Starlight docs (Astro), shell and PowerShell installers, `llms.txt`, `board-demo.js` |
| `tests/` | integration tests, golden fixtures, `tests/packaging/` (Python) |

## Build, test, verify

- Build: `cargo build --release`. `herdr-plugin.toml` launches `./target/release/tsk`;
  rebuild before live smoke. A running board keeps the old binary until you quit it.
- Test: `cargo test` (plain, parallel; 1000+ tests, well under a minute warm).
- Green bar, run once at the end as a single chain, not after every edit:
  `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
  (under a minute warm, about three after a lib change). CI runs the same chain plus the
  release build and is the gate that matters. Add `cargo build --release` only before a
  live smoke or packaging work.
- Before starting it, `pgrep -lx 'cargo|rustc'` must print nothing; a second cargo holds the
  `target/` lock. Wait, do not kill a process you did not start.
- Integration suites share `tests/integration.rs`; run one with
  `cargo test --test integration <module>::`. Six process-state-mutating suites retain
  dedicated targets, as do `queue_board_bench` and the site's `demo_parity` reference
  generator (so parity jobs do not compile unrelated suites). `autotests = false` prevents duplicate
  binaries: register new suites in the shared harness (or an explicit `[[test]]` when
  isolation is necessary). The layout regression test checks every root suite is registered.
- For a targeted run use `--lib` or `--test <name>`; `cargo test <filter>` still compiles
  every test binary.
- macOS: if the bar takes tens of minutes with idle CPU, Gatekeeper is scanning each freshly
  linked test binary on first launch (`cp target/debug/deps/<any test binary> /tmp/x && time
  /tmp/x` shows seconds instead of milliseconds). Fix is the owner's: add the terminal app
  under System Settings → Privacy & Security → Developer Tools and relaunch it and any
  server under it (herdr). Report and stop, do not keep polling.
- Never delete anything under `target/` unasked. Cargo does not garbage-collect old
  artifacts, so the directory grows with every branch. If `du -sh target` is above 10 GB,
  say so and suggest the owner run `rm -rf target/debug/incremental` at session end when
  no cargo is running (then optionally `cargo test --no-run` in the background so the next
  session is warm). `cargo clean` is the full reset and costs one cold build of everything.
- Toolchain is pinned in `rust-toolchain.toml` and matched by CI (`rustfmt` + `clippy`,
  ubuntu and macos). `clippy::if_same_then_else` fails the bar: merge the conditions.
- A regression test must fail without its fix. Write it, revert the fix, watch it fail,
  restore the fix. Use content that actually crosses the boundary under test. Revert the
  hunk by hand, never `git checkout <file>`, which discards every other edit in that file.
- Temp state dirs need a per-binary atomic counter, not just `SystemTime::now()`.
- Golden fixtures regenerate via
  `cargo test --test integration queue_board_render::regenerate_golden_fixtures -- --ignored`;
  never hand-edit the `.txt` files.
- Packaging tests: `python3 -m unittest discover -s tests/packaging` (Python 3.11+); set
  `TSK_TEST_BINARY` to a built binary to include the installer and setup PTY smoke.
  Installer tests must isolate `HOME` and `ZDOTDIR`, never the caller's startup files.
- Site CI is `npm ci && npm test && npm run build` in `site/`. `npm test` validates
  `site/vercel.json` with Vercel's own route parser; never edit that file without it, a
  bad pattern fails every production deploy silently.
- CI runs on pushes to `main` and PRs targeting `main`. Site-only changes skip the Rust
  matrix; installer-only changes run the `Installer` workflow (packaging tests, ShellCheck,
  and native x86-64 and ARM64 Windows PowerShell smokes) on the PR and again on the push,
  since gettsk.sh serves both installers straight from `main`.
  Vercel production deploys only on pushes to `main`.

### Isolated state

Anything that writes tasks or touches herdr config runs against isolated roots:
`TSK_STATE_DIR` for tsk; additionally `XDG_CONFIG_HOME`, `XDG_STATE_HOME` **and**
`HERDR_SOCKET_PATH` for `tsk setup herdr` (the state override
alone does not isolate Herdr's plugin registry). Never smoke setup against, or relink,
a daily plugin.

### Live herdr smoke

When the change touches the board, host integration, or panes, unit and render tests
alone are not enough. If `HERDR_ENV=1`: rebuild in-repo, drive the real path with herdr,
read the pane, and fix anything that only fails live. If `HERDR_ENV` is unset, say that
live smoke was not run. Wide (110 columns or more) smoke needs a full-width Herdr tab, not
a split pane; splitting the smoke pane right is the cheap way to drive it below 110 and
back.

## Docs ship with the feature

Any change that adds, removes, or alters user-visible behaviour (a key, verb, palette
command, mouse target, status message, CLI flag or output, exit code, board section,
file or env var) lands in the same PR as its `site/` update: the relevant page under
`site/src/content/docs/docs/`, the landing page (`site/src/pages/index.astro`) and demo
(`site/public/board-demo.js`) when they show it, `site/public/llms.txt`, the README when
the claim lives there, and `CHANGELOG.md` under Unreleased in the right subsection
(Breaking · Added · Changed · Fixed, format defined at the top of that file). Remove
docs for what you remove. Docs describe only what ships; upcoming work is marked as such.

Before calling a docs pass done, diff the guide against `src/`: keymaps in
`src/ui/input.rs`, palette in `src/ui/board/commands.rs`, CLI in `src/cli/`.

The web demo uses bare verb keys on purpose (browsers reserve control chords); do not
"fix" that to match the TUI.

The skill's frontmatter `version:` moves **once per release**, not once per edit. The first
skill change after a release bumps it one minor above the version the last release shipped
(check `git show vX.Y.Z:skills/tsk-cli/SKILL.md`); every further edit while that number is
still unreleased keeps it and only refreshes the FNV-1a content hash pin in
`embedded_skill_declares_semver` (`src/setup_agent.rs`). Never go below a shipped version:
those copies are on users' disks. On failure the assertion's `left` value is the new hash;
paste it in. `tsk setup <agent>` compares the version against the installed copy and only
rewrites on a difference, so a skill edit without a bump since the last release leaves every
installed skill silently stale. The skill is also `tsk guide` and `/docs/agents/`, so it
counts as user-visible.

## Invariants

Things that look like bugs or cleanups but are load-bearing. Change them only with the
migration or design work they imply. What the behaviour *is* lives in the docs
(`storage.md`, `board.md`, `keys.md`); this list is only the *why not to touch it*.

### Store and state

- Human status is source of truth. Never auto-complete tasks from agent status.
- The projectless scope displays as `desk` but serializes as `global` and is
  `TaskScope::Global` internally. Do not rename either without a store migration.
- Existing task scopes never move when scope resolution rules change.
- Archive is a flag, never a move: archived tasks and projects stay in `tsk.json`.
- Store format is versioned (`STORE_FORMAT_VERSION`). Any schema change bumps it and adds
  a `vN → vN+1` step to `MIGRATIONS` in `src/store.rs`. `deny_unknown_fields` stays on
  `Task` and `DomainState` so an older binary refuses a newer file instead of dropping
  fields. Migrated loads write `tsk.json.v<N>` beside the live file on first save.
- Trash (`trash.jsonl`) is durable before the live document loses a task, is rewritten
  atomically, and readers dedupe by id and skip torn lines. Nothing on the board reads it.
- A bulk verb on a marked set is one domain transaction, one save, and one undo entry
  (`UndoEntry::Batch`, store v5). Never persist it as per-task entries: one `ctrl+u`
  reverses the whole set, and a stale batch refuses without touching any task.
- The state dir must be a real directory on a local disk (flock plus rename replace);
  permission hardening refuses a symlink rather than chmodding its target. Host-injected
  `HERDR_PLUGIN_*` dirs are ignored on purpose.
- Search is an in-memory filter over `DomainState`. Do not migrate to SQLite until open
  or save is felt-slow, or the file is regularly above ~10 MB.
- Starter tasks and announcements seed on the full board open only, never quick capture,
  the CLI, or the installer, and dedupe by catalog id so a lost `delivery.json`
  converges. `src/announcements/catalog.toml` ids are positive and increasing in file
  order and the changelog link lives in `notes`, never `title`; the parser refuses
  otherwise. Append one entry per release worth a board notice. An id that shipped in any
  release, pre-releases included, is used forever: removing its entry leaves a comment in the
  catalog header and the next entry takes a higher id.

### UI

- Mutating verbs always take Ctrl. Bare letters do nothing unless they carry a bare
  route; the keymap resolves one letter by modifier class (`d` drawer vs `ctrl+d` done).
- Mono modifiers only, no colour, at every width. There is no theme module.
- Text wraps, never truncates. One wrap engine (`edit::wrap_text`) feeds every surface;
  `escaped_draft_rows` remains only for single-line Title/Thread editors.
- `src/ui/capture.rs` is unreachable from the binary and stays that way; creation
  detail lives on the task page and the expanded quick-add draft owns the whole frame.
- `Enter` on a stored step is `ToggleStep`, resolved at the keyboard boundary in
  `src/app.rs` so the save baseline loads first.
- `map_edit` and `map_board_form_key` share `map_form_edit_key`. Save-recovery
  `r`/`c`/Esc must reach Retry/Cancel even if a form is allocated (`src/app.rs`
  allowlist).
- Help is a presentation override, not the underlying input state. Checks such as
  `has_unsaved_work` use `help_return_mode`; its scroll horizon is measured in wrapped
  screen rows recorded by the renderer, never unwrapped catalog rows.
- A centered overlay in wide split owns mouse input across the whole frame before the
  focused-column gate runs; otherwise its task-column half is visibly inert.
- A form and its input mode must outlive the save call. Never clear the form or switch
  mode before the persistence boundary confirms: a cancelled failed save otherwise
  leaves an edit mode with no form, which no key can escape.
- Anything painted on the status-row slot hides `status_message`. A surface living
  there owns its own refusals and clears them on close, or the message is invisible and
  then leaks onto the board afterwards.
- Selection may only rest on a row the current lens paints. `seed_selection` and
  reanchor fallbacks must never pin an invisible task. The same holds for marks: the
  multi-select set is session-only, lives beside the cursor, and clears at every verb and
  lens boundary rather than pinning hidden rows. Status verbs act on the set when it is
  non-empty; `Enter`, `ctrl+e`, and task-page actions stay cursor-only.
- Board search is a per-lens filter applied in `queue_view` after `queue::query_board`,
  never a second row source. Overview rows filter by project name; everything else by task
  content. `Enter` pins the query and hands keys back to the board; `Esc` clears it first.
- `sync_from_domain` never moves the user's tab or selection for tasks merged from disk;
  the one exception is an otherwise-empty view surfacing the first arriving task. A
  pinned save pins the selection only when the current lens renders the saved task.

### Host integration

- `tsk setup herdr` registers embedded plugin assets from `src/setup.rs` using the stable
  invoked executable path, not a canonicalized Cellar path. Re-running does not duplicate
  bindings; a noninteractive conflict aborts before writes. Herdr 0.9+ required.
- `prefix+t` opens or focuses the board within the invoking workspace, across tabs.
  `scripts/open-board.sh` scopes lookup with `HERDR_WORKSPACE_ID`, activates
  `HERDR_TAB_ID` before creation, and anchors the split to `HERDR_PANE_ID` (Herdr rejects
  `--workspace` for split placement). It refuses missing context or a failed pane
  listing. On reuse call `herdr tab focus` before `plugin pane focus`; API `focused: true`
  alone is not visible-navigation evidence, confirm the displayed tab.
- Pane label matching is exact against `board_pane::BOARD_PANE_LABEL`; the manifest pane
  title must equal it.
- `tsk setup --skill-states` and `tsk setup herdr --check` are installer probes: the site
  serves the newest platform installer to every `tsk update`, and each update path parses
  these stable probes. Keep their output shape stable, and keep the installers' fallback
  for a binary that lacks them (empty probe output means "old binary", never "no agents").
- `tsk setup herdr` never adds the default chord for a plugin command the user bound on
  another key; a binding on the default key still goes through conflict repair. `tsk update`
  refreshes a bound plugin noninteractively and relies on both.

### Windows installer

- Users run `site/public/install.ps1` as `irm … | iex` inside their own session, so it has
  no top-level `param()` (it would overwrite the caller's `$Help`/`$NoPathUpdate`) and runs its
  whole body in its own scope via `$__tskInstallerBody`; state lives in `$TskInstallerState`,
  never `$script:`. Only a file run (the block's `.File` is set) forwards `$args` or
  dot-sources: under `iex`, `$args` belong to the enclosing command. Tests dot-source the file
  and override hooks guarded by `Test-Path Function:\…`; keep those guards.
- Document only the in-session `irm https://www.gettsk.sh/install.ps1 | iex`. Microsoft
  Defender flags the `powershell -c "irm … | iex"` command-line shape
  (`Trojan:Win32/Commando.A!ml`) and PowerShell reports the block as a misleading
  `Missing closing '}'` parse error; a site test refuses that form in the docs. Use the `www`
  host (the apex 308-redirects); `site/vercel.json` serves `/install.ps1` as `text/plain`, and
  the post-deploy Vercel job checks it with Windows PowerShell 5.1.
- Windows smokes can run natively when a Windows host is available to the agent; otherwise
  they are owner steps. A custom `TSK_INSTALL_DIR` still edits the user PATH, so a smoke
  defines `Update-InstallerPath` first or cleans PATH afterwards.

## Cutting a release

Full detail and user contracts: `packaging/README.md`. Short form:

**Version bump (one PR).** Update `Cargo.toml`, `Cargo.lock`, `herdr-plugin.toml`, and
`site/src/version.mjs` together; rename `CHANGELOG.md` Unreleased to `## vX.Y.Z` and open
a fresh empty Unreleased above it; append a `[[announcement]]` if the release deserves a
board notice. Versions that only ever shipped as pre-releases get no CHANGELOG section of their
own: fold them into the next stable version's section, a few short user-facing lines.
`scripts/release.py check-version vX.Y.Z` must pass. Merge, then push the stable tag `vX.Y.Z` with owner
approval. Tags are `v[0-9]+.[0-9]+.[0-9]+` only: the workflow, `release.py`, and
both installers refuse anything else, so there are no `-rc` tags.

**Build (owner-run).** Dispatch `Prepare release` with the existing tag. It pins the tag
to one commit, tests and builds six targets from it, and creates a **draft** with the
archives, `SHA256SUMS`, both installers, and a version-pinned `tsk.rb`. It refuses a moved
tag or an existing release; it never publishes or updates the tap. The draft is named with
`--title "$TAG"`: an unnamed GitHub release displays the tagged commit's subject instead.

**Test release (pre-release).** Flip the draft rather than publishing:
`gh release edit vX.Y.Z --draft=false --prerelease`. GitHub excludes pre-releases from
`releases/latest`, so the default installer path and the board's update nudge stay on the
previous stable while the assets are public. It is listed on `/releases` with a
`Pre-release` badge and notifies release watchers; only a draft is fully hidden, and a
draft cannot serve the curl one-liner. While it is a pre-release, its notes carry a short
"Testing this pre-release" block with the pinned `TSK_VERSION` commands; remove it at
promotion. Exercise the real user flow with isolated state:

```
TSK_VERSION=vX.Y.Z TSK_INSTALL_DIR=/tmp/tsk-rc/bin sh -c "$(curl -fsSL https://gettsk.sh/install.sh)"
TSK_STATE_DIR=/tmp/tsk-rc/state /tmp/tsk-rc/bin/tsk
gh release download vX.Y.Z -p tsk.rb -D /tmp/tsk-rc && HOMEBREW_DEVELOPER=1 brew install --formula /tmp/tsk-rc/tsk.rb
```

The site serves both installers from `main`; if one changed in this release fetch that
installer from `releases/download/vX.Y.Z/` instead. Smoke `tsk setup herdr` under isolated
roots. Rehearse the update path too, against a throwaway `HOME` holding an outdated skill
and a Herdr config on a custom key:
`env HOME=/tmp/x/home XDG_CONFIG_HOME=/tmp/x/home/.config HERDR_SOCKET_PATH=/tmp/x/none.sock TSK_VERSION=vX.Y.Z TSK_UPDATE=1 TSK_CURRENT_VERSION=vPREV TSK_INSTALL_DIR=/tmp/x/bin sh /tmp/x/install.sh`. A failed rehearsal burns the tag: fix forward with the next patch version and leave the bad
pre-release in place. A published version is never deleted, moved, or reused.

Rehearsal traps, learned the hard way:

- Never run `tsk update` on a pre-release copy before promotion. It follows
  `releases/latest`, which is still the previous stable, so it downgrades the binary,
  and the downgraded binary then refuses the store the pre-release migrated.
- Never point a candidate binary at a store you did not create for the rehearsal. Pass
  the state override as `env TSK_STATE_DIR=… /path/tsk`, and in a Herdr pane
  read until an idle prompt first: a pending shell prompt can eat the first character of a
  `VAR=value` prefix, and the candidate then opens and migrates the real `~/.tsk`.
- `brew install --formula tsk.rb` on a machine with the tap's `tsk` installed replaces the
  daily copy (same formula name) and removes the old keg. Use a throwaway machine or
  `brew unlink` first and expect to `brew reinstall smarzban/tap/tsk` afterwards.
- Public installer flows plus Linux, Intel, and Windows smokes are owner steps unless the
  agent has a host for them (see Windows installer above). The agent verifies checksums, the
  archive binaries, the upgrade path and setup, and says which of these it did not run.

**Official release.** Replace the draft's skeleton and checklist with the version's
`CHANGELOG.md` section verbatim (keep the one-line Install section), then promote the tested
assets, no rebuild: `gh release edit vX.Y.Z --prerelease=false --latest`. Confirm
`https://github.com/smarzban/tsk/releases/latest` redirects to the new tag. Then,
and only then, copy the generated `tsk.rb` to `Formula/tsk.rb` in `smarzban/homebrew-tap`
and push; a release alone never updates Homebrew. Finish with a smoke of the public
one-liner and the tap on a clean machine.
