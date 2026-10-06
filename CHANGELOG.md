# Changelog

One `## vX.Y.Z` section per release, newest first, with `## Unreleased

## v0.12.0

### Breaking

- Store format 6 adds task assignees, dispatch base branches, and dispatch records. Rolling back refuses the file; restore `tsk.json.v5`.

### Added

- Agent profiles: define agents as `[agent.<name>]` tables in `<state dir>/config.toml`, each a command template with an optional prompt and environment. Templates take `{number}`, `{title}`, `{notes}`, `{steps}`, `{worktree}`, `{branch}`, and `{base}`. A profile without its own prompt sends a default that hands the agent its task and asks it to work only on its branch, open a pull request into the base without merging, then set the task to review or blocked. The first board open writes a commented starter file with examples for Claude Code, Codex, Pi, Grok, and any other terminal agent.
- Assignees: assign a task to a profile with `!a name` in quick-add, `@` on the board or task page, the task page's Assignee field or footer, palette **set assignee** (marked sets included), or `tsk add/edit --assignee`. The peek and task page show `@name`. On a project board `t` opens a **Filter** with `threads · @assignees` tabs, on Projects `v` offers one view per assignee across projects, `/` matches assignees, and `tsk list --assignee` filters them.
- Dispatch: `ctrl+g`, palette **dispatch to @name**, or `tsk dispatch T<n>` creates a branch, Git worktree, and Herdr workspace named after the task (`tsk/t12-fix-login-timeout`, workspace `T12 Fix login timeout`), launches the assigned agent there as `t12-<name>`, and starts the task. An unassigned task asks for an agent first. With tasks marked, `ctrl+g` opens one card listing what will launch and what is skipped, and `y` launches them all in the background. `ctrl+g` twice, **dispatch again**, or `tsk dispatch --again` relaunches a dispatched task. A started task with a live dispatch shows `◉`. Dispatch needs Herdr on macOS or Linux.
- Base branches: dispatch starts from the task's base branch, set with `!b branch`, the task page's `⎇` footer or Base field, palette **set base**, or `tsk add/edit --base`, and otherwise from the repository's default branch. The branch picker opens at once and refreshes after a background fetch. The task page shows the branch and commit each dispatch started from.
- Completion and cleanup: `ctrl+d` on a dispatched task shows whether its branch is merged and what cleanup will remove. `y` completes the task and removes the worktree, Herdr workspace, and merged branch in the background; `n` completes it and keeps everything. A marked set gets one card for all its dispatches and one undo step. Cleanup never removes a worktree with uncommitted changes and keeps every branch it cannot confirm is merged into its base. `tsk clean T<n>` and `tsk status T<n> done --clean` do the same from scripts.
- CLI and JSON: `tsk add` and `tsk edit` take `--assignee`, `--unassign`, `--base`, and `--clear-base`, and JSON plans take `assignee` and `base`. `tsk list --json` includes `assignee` and `base`; `tsk list T<n> --json` adds the dispatch record; `tsk clean --json` reports what was removed and kept. Agent skill 1.5.0 covers dispatch and cleanup; `tsk update` refreshes installed copies, or rerun `tsk setup`.

### Fixed

- A project reached through a symlinked path, such as `/tmp/repo` and `/private/tmp/repo` on macOS, is one project on the board, in quick add, and in the CLI.
- With tasks marked, `Esc` in a project board's filter picker closes the picker and keeps the marks.
- The task page's project chooser lists options top to bottom, so `↓` moves the highlight down.

## v0.11.6

### Added

- Windows 10/11 support on ARM64 and x86-64: native release builds, a PowerShell installer (`irm https://www.gettsk.sh/install.ps1 | iex`), and Herdr integration as a preview.
- `h` and `l` mirror the left and right arrows for closing and opening task details in non-text board and task views. Thanks @pisceskkk.

### Changed

- `tsk update` downloads the installer completely before running it, always follows the latest stable release, refuses to downgrade, and says why a Herdr or skill refresh failed.

## v0.10.1

### Changed

- `tsk update` prints the version it replaces and the one it installs, re-registers an already set-up Herdr plugin without asking, and updates outdated installed agent skills (one `[Y/n]` ask on a terminal, unattended otherwise). It never installs a skill for an agent that had none; with nothing installed it offers the first-install ask.
- `tsk setup herdr` leaves a plugin command you bound to another key alone instead of adding the default chord beside it.

## v0.10.0

### Breaking

- Store format 5: one undo entry can cover a marked set. Rolling back refuses the file; restore `tsk.json.v4`.

### Added

- Multi-select: `Shift+M`, then `space`, `Shift+↑`/`Shift+↓`, or a click marks tasks; status, archive, and delete act on the set. One `ctrl+u` undoes a marked done or delete.
- `/` searches every board tab by title, notes, steps, thread, and task number (project names on the Projects overview). `Enter` pins the filter for normal keys; `Esc` clears it.

### Changed

- `!p name`, `tsk add -p name`, and JSON plan projects refuse unknown or ambiguous names (`unknown-project`) instead of creating a stray project. A new project needs an existing absolute path (`/…` or `~/…`).
- `ctrl+q` quits from every non-editor surface, including task pages and the wide project preview. `Esc` closes layers and wide splits, then quits at the full-board root on any tab. Unsaved drafts still block quitting.
- Agent skill 1.3.0 (`unknown-project` refusal, search and multi-select); rerun `tsk setup` to update installed copies.

## v0.9.0

### Breaking

- Store format 4: existing `ready` tasks move to the new `open` inbox on first save. Rolling back refuses the file; restore `tsk.json.v3`.
- `ctrl+n` now sets ready (it no longer opens Notes) and `ctrl+o` sets open (it no longer reopens to ready).

### Added

- `open` status for captured, untriaged tasks: an expandable `inbox` group under ON DECK, `ctrl+n` picks a task for ready, `ctrl+o` sends it back.
- `tsk status T<n> open`, `tsk list --open`, `tsk list --ready`.
- Projects tab at 110+ columns: the cursored project previews beside the index, `→` makes it a live board, `←` and `Esc` walk back.
- Projects index counts ON DECK and DONE next to NEEDS YOU and IN MOTION.
- `tsk list T<n>` prints the whole task: notes, steps, and thread, in human and JSON form.
- `tsk help <command>` as an alias for `--help`, and `tsk --version` / `-V`.

### Changed

- `tsk --help` is the CLI reference: commands grouped with a Statuses block, and one 80-column skeleton per command (usage, options, examples, refusals, exit codes). Long-form detail lives in the CLI guide.
- Refusals print as `code: message` on stderr for every task command; `archive` and `unarchive` gain stable codes.
- The agent skill (`tsk guide`, `tsk setup <agent>`) is slimmed to rules of engagement, board language, the exit contract, and workflows, reads JSON by default, and gains a `Refine a task` section. Skill 1.2.0; rerun `tsk setup` to update installed copies.
- Task page and expanded quick-add share one Tab ring: Title, Notes, steps, + step, Thread, Scope, back to Title; `Shift+Tab` reverses it.
- Task-page footer shows the thread before the project.
- Inbox heading paints in normal text with no blank row under it.
- `ctrl+s` starts open tasks as well as ready ones.
- Board help (`?`) is a searchable shortcut reference: type to filter by key or action, `Esc` clears then closes.
- Human `tsk list` output wraps to the terminal width, supported from 50 columns.
- The starter tour mentions the projects preview and `tsk help`; the upgrade notice covers this release's features.
- The upgrade notice (`What's new in tsk`) arrives in `review` under NEEDS YOU; the last starter guide seeds as `open` so a fresh board shows both ON DECK groups.
- The Homebrew caveat also points at `tsk setup agents`.

### Fixed

- `tsk setup agents --yes --force` rewrites every detected agent skill even when the version already matches.
- `tsk setup herdr` no longer prints the plugin root on success and reports a previous registration whose root is gone before re-registering.
- The selected task-page footer control is no longer dim inside its highlight.
- Expanded quick-add in a project preview keeps its draft title in the column header and styles the active notes line as an editor.

## v0.8.2

### Breaking

- Task store format 3, migrated automatically on first open. Rolling back to 0.7.x afterwards refuses the file; restore the `tsk.json.v2` backup written beside it if you need to.

### Added

- Starter tour on first board open: four desk tasks (`N1` to `N4`) that teach the tabs, sections, status keys, the task page, and the CLI, each cleared for good with `ctrl+d`, `ctrl+f`, or `ctrl+x`. Agents never see them: `tsk list` hides `N` rows.
- What's new on your desk: after an upgrade, one `N` row can summarise the release with a link to the changelog. This release carries none, so a 0.7.x upgrader sees only the tour.
- `tsk update` upgrades installer-managed copies in place; Homebrew copies print `brew update && brew upgrade tsk`. The board nudges once a day when a newer release is out (`TSK_NO_UPDATE_CHECK` disables).
- `tsk setup` detects the coding agents on your machine and offers to install the tsk skill for each; `tsk setup agents --yes` does it unattended. The curl installer asks the same question once when Herdr is on PATH.
- Added support for OMP with `tsk setup omp`, thanks @bnivanov.
- Outside Git, the current directory is available as a project: `2` opens its board, quick-add files there once it is open. The desk stays the default for capture and the CLI.

### Changed

- The agent skill (`tsk guide`, `tsk setup <agent>`) is rewritten around a quick-reference table and rules of engagement: agents hand work back with `review` and leave `done` to you. Skill version 1.1.0; rerun `tsk setup` to update installed copies.
- Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE and the drawer's ARCHIVED group keep the most recent status change on top, and ON DECK lists its backlog oldest first, `N` rows leading. Editing a task or ticking a step no longer jumps it to the top.

### Fixed

- On a wide board, clicking a task opens its details beside the board and keeps board focus; a double-click during column reflow opens the task you clicked, not a newly exposed control.
- `prefix+t` opens or focuses one board per Herdr workspace, across tabs, and returns to the tab you pressed it from.
- `tsk setup herdr` on a Herdr older than 0.9 says so (`herdr 0.6.8 found; tsk needs 0.9.0 or newer`) and stops before touching the config, instead of failing on `herdr config check` with a usage dump.
- Setup errors that quote Herdr's output keep their line breaks instead of printing a literal `\u{000a}`.

## v0.7.0

### Added

- `tsk guide` prints the agent workflow, and `tsk setup <claude|pi|cursor|grok|codex|opencode|--skill-dir>` installs it as a skill. `tsk --help` ends with a pointer for agents.
- The installer adds `~/.local/bin` to PATH for Bash and Zsh.

### Fixed

- At 110 columns or wider, `Tab` on the quick-add line opened a draft page that was never painted.
- A reopen request replaced by another of the same size in the same instant could be delivered as the first one.

## v0.6.0

### Added

- Native macOS and Linux binaries, a checksum-verifying install script, and a Homebrew formula.
- `tsk setup herdr` registers the installed binary with Herdr and adds `prefix+t` (board) and `prefix+a` (capture).
- CLI: `tsk status`, `tsk edit`, and `tsk steps rename|remove`.
- **NEEDS YOU** section: blocked and review tasks at the top of the board.
- Projects index with search, per-project status counts, and persistent desk · project · projects navigation.
- Quick capture (`tsk capture`) opens the task editor in a Herdr popup; the expanded quick-add takes threads and steps.

## v0.5.0

### Added

- Wide layout: at 110 columns or wider the board and task page form a four-stage slider driven by `←` / `→`.
- Archive for tasks (`ctrl+f`) and projects (from the `p` picker), with a collapsible archived group in the done drawer and read-only focus for archived projects. CLI: `tsk archive`, `tsk unarchive`, `tsk project archive|unarchive`, `tsk list --archived`.
- Trash: deleted tasks move to `trash.jsonl` once undo can no longer reach them and purge after 30 days. `tsk list --deleted` and `tsk trash restore T<n>`.

### Changed

- State files are private on Unix: `~/.tsk` is `0700` and its files `0600`; symlinked state roots are refused.
- Store format 2: a `projects` map; a v1 store migrates on load and keeps a `tsk.json.v1` backup.
- Undo history is capped at 50 entries.
- License is MIT.

## v0.4.0

- Markdown in notes (view and peek): bold, emphasis, code, fenced blocks, headings, lists, in mono modifiers only.
- Mouse text selection: drag to highlight, release to copy via OSC 52.
- Board and task-page scrollbars with click and drag; the current section header pins under the tabs.
- Modal overlays for help, palette, and project picker share one centered card.

## v0.3.0

- Threads: an optional normalized label per task, set with `--thread` on add or in plan JSON, filtered with `list --thread`.

## v0.2.0

- Steps: an ordered checklist on the task page, with `steps <task> add|toggle` on the CLI. Step progress never changes task status.

## v0.1.0

- Queue board for capture and human status (`ready`, `started`, `blocked`, `review`, `done`) inside Herdr, with peek, task page, project scope, done drawer, and undo.
