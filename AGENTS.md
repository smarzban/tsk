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

Docs pages live in `site/src/content/docs/docs/`.

| Topic | Source of truth |
| --- | --- |
| Board surfaces, tabs, drawer, archive, wide slider | `board.md` |
| Task page, editing, steps, block and review sections, paper trail | `task-page.md`, `steps.md` |
| Quick-add and `tsk capture` | `capture.md` |
| Every key of every surface | `keys.md`, code in `src/ui/input.rs` |
| CLI verbs, exit codes, error codes | `cli.md`, code in `src/cli/` |
| Terms | `CONTEXT.md` |
| Storage, env vars, update check, `config.toml` profiles, backups | `storage.md` |
| Assignee, dispatch, cleanup | `dispatch.md`, code in `src/dispatch.rs` |
| Install, Homebrew, `tsk setup herdr` | `install.md` |
| Packaging, release steps, installer contracts | `packaging/README.md` |
| Agent skill | `skills/tsk-cli/SKILL.md` (`tsk guide`, `/docs/agents/`) |
| Coordinator skill | `skills/tsk-coordinator/SKILL.md` (`/docs/coordinator/`) |
| Exact rendered output | golden fixtures in `tests/fixtures/` (`tests/queue_board_render.rs`) |

If the docs and the code disagree, the code is the bug or the docs are; fix one in the
same PR, never leave them apart.

### Where things live

| Path | What |
| --- | --- |
| `src/app.rs` | keyboard boundary, save recovery allowlist |
| `src/ui/` | chrome: `input`, `mouse`, `render`, `edit` (the one wrap engine), `markdown`, `queue` (board sections), `tier` (width tiers, wide stage) |
| `src/ui/board/` | board model · apply · commands · chrome · draw; `block` (block and review cards), `cleanup_card`, `dispatch_card`, `launch_card`, `selector` (shared option cursor) |
| `src/cli/` | headless verbs, `parser`, `router`, `presenter` |
| `src/store.rs`, `src/domain/` | `tsk.json` format, migrations, trash; `domain/` holds `after`, `block` (blocks, reviews, replies), `undo`, `events` |
| `src/activity.rs` | paper trail entries for the task page and `tsk list T12` |
| `src/backup.rs` | git backups and the versions `tsk restore` picks from |
| `src/agents.rs` | `config.toml` profile loader, prompt/argv rendering, starter seed |
| `src/dispatch.rs`, `src/dispatch/pr.rs` | dispatch and cleanup engine behind a `DispatchHost` seam (git, herdr, and `gh` calls), cleanup guardrails, pull request merge |
| `src/git_base.rs` | dispatch base branches, resolved in the task's repository |
| `src/context.rs`, `src/scope.rs` | invocation context from Herdr and the cwd, project path resolution |
| `src/save_recovery.rs` | retry or cancel of one failed save |
| `src/fsperm.rs` | private file modes for the state directory |
| `src/setup.rs`, `src/setup/`, `src/setup_agent.rs` | `tsk setup herdr`, agent skill install (`tsk-cli` and `tsk-coordinator`) |
| `src/board_pane.rs` | `--find-board-pane` lookup for the launchers |
| `src/guides.rs`, `src/announcements.rs`, `src/delivery.rs` | seeded notice tasks |
| `src/update.rs` | release check, `tsk update` |
| `scripts/open-board.sh`, `scripts/open-capture.sh`, `scripts/*.ps1` | Herdr launchers (embedded via `src/setup.rs`) |
| `scripts/release.py`, `.github/workflows/release.yml`, `packaging/` | native packaging |
| `site/` | landing page + Starlight docs (Astro), shell and PowerShell installers, `llms.txt`, `board-demo.js` |
| `tests/` | integration tests, golden fixtures, `tests/packaging/` (Python) |

## Build, test, verify

| Job | Command |
| --- | --- |
| Build | `cargo build --release` |
| Test | `cargo test` (plain, parallel; 1000+ tests, well under a minute warm) |
| Green bar | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` |
| One integration suite | `cargo test --test integration <module>::` |
| Targeted run | `cargo test --lib` or `cargo test --test <name>` |
| Regenerate golden fixtures | `cargo test --test integration queue_board_render::regenerate_golden_fixtures -- --ignored` |
| Packaging tests | `python3 -m unittest discover -s tests/packaging` (Python 3.11+) |
| Site | `npm ci && npm test && npm run build` in `site/` |

### Run the green bar

1. `pgrep -lx 'cargo|rustc'` must print nothing: a second cargo holds the `target/` lock.
   Wait; do not kill a process you did not start.
2. Run the green bar once at the end, as a single chain, not after every edit. It takes
   under a minute warm, about three after a lib change.
3. Add `cargo build --release` only before a live smoke or packaging work.

CI runs the same chain plus the release build and is the gate that matters.

### Rules

- `herdr-plugin.toml` launches `./target/release/tsk`: rebuild before live smoke. A running
  board keeps the old binary until you quit it.
- `cargo test <filter>` still compiles every test binary; use `--lib` or `--test <name>`.
- Integration suites share `tests/integration.rs`. Six process-state-mutating suites keep
  dedicated targets, as do `queue_board_bench` and the site's `demo_parity` reference
  generator, so parity jobs do not compile unrelated suites.
- `autotests = false` prevents duplicate binaries: register a new suite in the shared
  harness, or as an explicit `[[test]]` when isolation is necessary. The layout regression
  test checks every root suite is registered.
- Toolchain is pinned in `rust-toolchain.toml` and matched by CI (`rustfmt` + `clippy`,
  ubuntu and macos). `clippy::if_same_then_else` fails the bar: merge the conditions.
- Temp state dirs need a per-binary atomic counter, not just `SystemTime::now()`.
- Golden fixtures only regenerate (command above); never hand-edit the `.txt` files.
- Packaging tests: set `TSK_TEST_BINARY` to a built binary to include the installer and
  setup PTY smoke. Installer tests must isolate `HOME` and `ZDOTDIR`, never touch the
  caller's startup files.
- `npm test` validates `site/vercel.json` with Vercel's own route parser. Never edit that
  file without it: a bad pattern fails every production deploy silently.

A regression test must fail without its fix:

1. Write the test, with content that actually crosses the boundary under test.
2. Revert the fix hunk by hand. Never `git checkout <file>`: it discards every other edit
   in that file.
3. Watch the test fail.
4. Restore the fix.

### CI

- CI runs on pushes to `main` and PRs targeting `main`.
- A site-only push skips the Rust matrix. A site-only PR still runs it (the `Verify` checks
  are required), but a change probe skips every expensive step, so each job goes green in
  seconds.
- Installer-only changes run the `Installer` workflow (packaging tests, ShellCheck, and
  native x86-64 and ARM64 Windows PowerShell smokes) on the PR and again on the push:
  gettsk.sh serves both installers straight from `main`.
- Vercel production deploys only on pushes to `main`.

### Troubleshooting

- **Slow bar on macOS.** If the bar takes tens of minutes with idle CPU, Gatekeeper is
  scanning each freshly linked test binary on first launch.
  `cp target/debug/deps/<any test binary> /tmp/x && time /tmp/x` shows seconds instead of
  milliseconds. The fix is the maintainer's: add the terminal app under System Settings →
  Privacy & Security → Developer Tools, then relaunch it and any server under it (herdr).
  Report and stop; do not keep polling.
- **Large `target/`.** Never delete anything under `target/` unasked. Cargo does not
  garbage-collect old artifacts, so the directory grows with every branch. If
  `du -sh target` is above 10 GB, say so and suggest the maintainer run
  `rm -rf target/debug/incremental` at session end, when no cargo is running (then
  optionally `cargo test --no-run` in the background so the next session is warm).
  `cargo clean` is the full reset and costs one cold build of everything.

### Isolated state

Anything that writes tasks or touches herdr config runs against isolated roots:
`TSK_STATE_DIR` for tsk; additionally `XDG_CONFIG_HOME`, `XDG_STATE_HOME` **and**
`HERDR_SOCKET_PATH` for `tsk setup herdr`. Neither the state override nor
`HERDR_CONFIG_PATH` alone isolates Herdr's running session or plugin registry. Never smoke
setup against, or relink, a daily plugin.

### Live herdr smoke

When the change touches the board, host integration, or panes, unit and render tests
alone are not enough. If `HERDR_ENV=1`: rebuild in-repo, drive the real path with herdr,
read the pane, and fix anything that only fails live. If `HERDR_ENV` is unset, say that
live smoke was not run. Wide (110 columns or more) smoke needs a full-width Herdr tab, not
a split pane; splitting the smoke pane right is the cheap way to drive it below 110 and
back.

## Docs ship with the feature

A change that adds, removes, or alters user-visible behaviour lands in the same PR as its
docs. User-visible means a key, verb, palette command, mouse target, status message, CLI
flag or output, exit code, board section, file, or env var. The agent skills count too:
`tsk-cli` is also `tsk guide` and `/docs/agents/`; `tsk-coordinator` is `/docs/coordinator/`.

In the same PR:

- [ ] The page under `site/src/content/docs/docs/` that owns the fact
      ([one owner per fact](#one-owner-per-fact)).
- [ ] The landing page (`site/src/pages/index.astro`) and demo (`site/public/board-demo.js`),
      when they show it.
- [ ] `site/public/llms.txt`.
- [ ] The README, when the claim lives there.
- [ ] `CHANGELOG.md` under Unreleased, in the right subsection (Breaking · Added · Changed ·
      Fixed; format at the top of that file, line rules [below](#per-surface)).
- [ ] The skill version, when the skill changed ([below](#agent-skill-version)).
- [ ] The docs diffed against `src/`: keymaps in `src/ui/input.rs`, palette in
      `src/ui/board/commands.rs`, CLI in `src/cli/`.

Remove docs for what you remove. Docs describe only what ships; upcoming work is marked as
such.

The web demo uses bare verb keys on purpose (browsers reserve control chords); do not
"fix" that to match the TUI.

### Agent skill version

The skills' frontmatter `version:` moves **once per release**, not once per edit.
`tsk setup <agent>` rewrites an installed skill only when the version differs, so a skill
edit without a bump since the last release leaves every installed copy silently stale.

`skills/tsk-cli` and `skills/tsk-coordinator` share one version and install as a unit: an
edit to either moves both `version:` lines together. The installer probe keeps one row per
agent, keyed on `tsk-cli`, so old installers keep working.

1. Find the version the last release shipped: `git show vX.Y.Z:skills/tsk-cli/SKILL.md`.
2. On the first skill change after that release, bump to one minor above it. Never go
   below a shipped version: those copies are on users' disks.
3. On every further edit while that number is unreleased, keep it.
4. After every edit, refresh the FNV-1a content hash pin in `embedded_skill_declares_semver`
   or `embedded_coordinator_skill_shares_the_version_and_pins_its_hash`
   (`src/setup_agent.rs`). On failure the assertion's `left` value is the new hash; paste
   it in.

## Writing docs

Every page, notice, and changelog line is written for its reader, in this shape.

### Shape

- Open with one or two sentences: what this page covers and who it is for.
- One heading per surface or task. A screen of text without a heading is too long.
- Keys, verbs, flags, options, refusal codes: tables. Procedures: numbered steps. Everything
  else: short lists.
- Paragraphs ≤ 60 words, one idea each. Sentences ≤ 25 words. No semicolon chains, no
  nested parentheses.

### Content

- Present tense, what happens. Not why it was built, not what it used to do ("no longer",
  "before, …", "now also").
- Edge cases only when a user will hit them. Exhaustive cases belong in tests.
- Quote exact output only where the reader must recognise it; one example, not the
  catalogue.
- No internals in user docs: no file paths, Rust names, store field names, store versions,
  locking, retry timings.
- Examples use the canonical tasks (T12, T202, T205) and `PR #42`. Never real task, PR, or
  review-finding numbers; issue numbers only in changelog credits.
- Words: maintainer, not owner. No brief, coordinator, golden, fixture, or invariant in
  user-facing text.

### One owner per fact

Each fact lives on one page, the one [Product reference](#product-reference) names; other
pages link to it instead of repeating it.

### Notation and terms

- Keys: `ctrl+d`, `alt+←`, `shift+M`, `Enter`, `Esc`, `Tab`, `Space` (lowercase
  modifiers, named keys capitalised).
- One name per thing: desk, project, thread, assignee, agent profile, dispatch, worktree,
  workspace, block, review, check, round, paper trail, Projects view.

### Per surface

- **CHANGELOG**: one line per user-visible change, ≤ 30 words; details live in the docs. A
  fix to something first added in the same Unreleased section is not an entry.
- **Agent skill**: workflow first (numbered), then one-line rules that link to `cli.md`. No
  human-only material.
- **In-app text** (starter tasks, What's new, starter config): shorter than the docs, keys
  in backticks.
- **Contributor docs** (this file, `CONTEXT.md`, `packaging/README.md`): the same shape;
  file paths and code names are fine here.

## Invariants

Things that look like bugs or cleanups but are load-bearing. Change them only with the
migration or design work they imply. What the behaviour *is* lives in the docs
(`storage.md`, `board.md`, `keys.md`); this list is only the *why not to touch it*.

### Store and state

- Human status is source of truth. Never auto-complete tasks from agent status. The one
  automatic status change is a `ready` task whose last `after` prerequisite became done: it
  starts through `dispatch::start_route`, like any start. Undoing a reopen back to done
  releases nothing: the release belonged to the original done.
- A done and the starts it releases land together. Every done path (board, cleanup card,
  missing-worktree converge, CLI) saves through `save_releasing_with_host` or its CLI twin,
  which decides the release under the save's lock on the merged state, never a stale copy.
  The done and every released start share one save and one undo entry.
- A released assigned task's agent launches after that save; a failed launch puts the task
  back to ready. The mechanism is in the doc comments of `src/dispatch.rs`.
- A delete drops its number from every other task's `after` in the same undo entry
  (`UndoEntry::SetAfter` leaves); trash purge drops leftovers as bookkeeping. A number no
  live task carries waits on nothing. A delete never rewrites a task already soft-deleted:
  its own delete's undo entry must keep matching.
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
  reverses the whole set, and a stale batch refuses without touching any task. The one
  exception is a start that dispatches: its launches land one by one and are not undoable
  (see Assignee and dispatch).
- The state dir must be a real directory on a local disk (flock plus rename replace);
  permission hardening refuses a symlink rather than chmodding its target. Host-injected
  `HERDR_PLUGIN_*` dirs are ignored on purpose.
- Search is an in-memory filter over `DomainState`. Do not migrate to SQLite until open
  or save is felt-slow, or the file is regularly above ~10 MB.
- Starter tasks and announcements seed on the full board open only, never quick capture,
  the CLI, or the installer, and dedupe by catalog id so a lost `delivery.json`
  converges. `src/announcements/catalog.toml` ids are positive and increasing in file
  order and the changelog link lives in `notes`, never `title`; the parser refuses
  otherwise. Append one entry per release worth a board notice.

### UI

- Mutating verbs always take Ctrl. Bare letters do nothing unless they carry a bare
  route; the keymap resolves one letter by modifier class (`d` drawer vs `ctrl+d` done).
  The one exception is `alt+↑`/`alt+↓` (move the selected step on the task page): macOS
  takes ctrl+arrows for Mission Control. Do not add another non-Ctrl mutating chord.
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

### Assignee and dispatch

- Assignee is a label naming a `config.toml` profile; it never changes status or triggers
  anything by itself, it only decides what a start does (below). Names normalize with the
  thread normalizer and exact-match a defined profile at the boundary; a task keeps a name
  whose profile was removed and renders it as-is.
- Profiles are argv templates plus an optional prompt. tsk substitutes
  `{number} {title} {notes} {steps} {worktree} {branch} {base}` and nothing else; the
  prompt is appended as the last argument; the launch is a per-workspace launcher script in
  the state dir (`launchers/`, owner-only on Unix): POSIX `sh` on Unix, PowerShell on
  Windows. The pane types only the fixed line that runs it (`sh_launch_line`,
  `powershell_launch_line`); task text and `env` values never go through the pane's shell,
  whichever it is. On Unix the script re-runs itself through `$SHELL -l -c` (a fixed string)
  for the login environment, then execs the argv. tsk carries no knowledge of any harness's flags. A malformed
  `config.toml` never blocks the board or CLI work that does not assign.
- Starting an assigned task that was never dispatched dispatches it. Every start goes
  through `dispatch::start_route`:
  - unassigned, a running agent, or the assignee's own start (`TSK_AGENT`) is a plain
    status change;
  - uncertainty (Herdr cannot answer, not in Herdr) never launches;
  - a gone agent asks (board) or refuses `agent-gone` (CLI).

  Outside `HERDR_ENV=1` `tsk dispatch` refuses; a start there, or on a desk task, is plain
  and says why (`dispatch::launch_unavailable`).
- Dispatch sets `started` in the same save as the record; a failed launch leaves the task
  unstarted. Only a single-task start that dispatched is undoable (`UndoEntry::Start`:
  status only, the agent keeps running).
- A marked set gets one confirm card, then every listed task takes the single-task path
  (check, launch, commit in `src/dispatch.rs`) off the event loop and is saved as it lands.
  Quit and further dispatches wait for a running batch, or launched agents lose their
  record. Relaunch (the relaunch card, `dispatch again`) stays cursor-only; dispatched
  tasks in a marked set only start.
- The dispatch record stays on the task through every later status change. `◉` is derived
  only from `record present && !record.cleaned && status == started`, never from agent or
  pane state.
- Cleanup never deletes uncommitted work or a branch it cannot confirm is merged into the
  recorded base (a failed fetch keeps the branch), and only removes the recorded worktree
  (registered with git, matching the herdr entry's path, never the project root). It runs
  off the event loop after the completion is saved; quit waits for it. A missing worktree
  converges to `cleaned`. CLI `status done` never prompts.
- Every git and herdr call goes through the `DispatchHost` seam so tests use a fake host.
  Changes to the real host need a live herdr smoke against a throwaway repo under `/tmp`,
  never this one.

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

## Cutting a release

[`packaging/README.md`](packaging/README.md#release-steps) owns the release steps. Hard
rules:

- Tags are `vX.Y.Z` only, never `-rc`; push one only with maintainer approval.
- `Prepare release` makes a draft and never publishes; publishing is a maintainer step.
- Rehearse as a pre-release with isolated state, then promote; the tap updates only after.
- Never run `tsk update` on a pre-release copy: see the
  [rehearsal traps](packaging/README.md#rehearsal-traps).
