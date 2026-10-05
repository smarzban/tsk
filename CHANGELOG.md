# Changelog

One `## vX.Y.Z` section per release, newest first, with `## Unreleased` on top. Inside a
section the subsections are, in this order and only when non-empty: `### Breaking`,
`### Added`, `### Changed`, `### Fixed`. Every user-visible change lands here in the PR
that makes it, written for a user, not a contributor: omit demo alignment, CI wiring,
review history, and other maintainer-only work. On release the version's section becomes
the GitHub release notes verbatim.

## Unreleased

### Breaking

- Store format 6 adds optional task assignees, explicit base branches, and dispatch records. Rolling back refuses the file; restore `tsk.json.v5`.

### Added

- Tasks can be assigned to exact configured agent profiles from quick-add (`!a`), the task page, an anchored Assignee picker, the palette (including marked sets), or CLI add/edit flags. Press `@` on the board, peek, or task page for a quick assignee picker (profiles, then **none**), or click the task page footer's `@name`, shown as `+ assign` on an unassigned task when profiles exist. The board peek footer names assignee, an explicit base when set, thread, then project; CLI output shows assignees, and `tsk list --assignee` filters them.
- Agent launch profiles can be defined as `[agent.<name>]` tables in `<state dir>/config.toml` (unknown top-level settings are ignored) with a command template, an always-appended default or custom prompt, and environment values. The default prompt names the task, worktree, branch, and base branch, points the agent at `tsk guide`, the task notes, and the repo's agent instructions, and asks it to work only on its branch, run the project's checks, open a pull request into the base without merging, then set the task to review or blocked. The first full board open seeds a commented starter file when none exists, explaining profiles and placeholders, quoting the default prompt, and showing examples for Claude Code, Codex, Pi, Grok, and any other terminal agent.
- Filter a project board by assignee: `t` now opens a **Filter** picker with `threads · @assignees` tabs (`Tab` switches). Pick `@name` (including names whose profile was removed) or `unassigned`; a thread and an assignee combine, and the filter control reads `#release @claude`. On Projects, `v` adds `@name` views listing one assignee's tasks across projects. Board search (`/`) now matches a task's assignee, with or without its `@`.
- Dispatch an assigned project task with `ctrl+g`, the palette, or `tsk dispatch T<n>`. On an unassigned task, `ctrl+g` opens the assignee picker, saves the choice, then dispatches. tsk creates a Git worktree and Herdr workspace with short names cut from the title (branch `tsk/t<n>-<slug>` keeping whole title words within 30 characters, letters and digits in any script, worktree `tsk-t<n>-<slug>` (ASCII letters and digits only), workspace `T<n>` plus the title up to the same cut and `…`, suffixed `-2`, `-3` when the branch, worktree, or checkout directory is taken), launches the agent there and names it `t<n>-<assignee>` in Herdr when Herdr detects it, then records the dispatch and starts the task in one save. Set an explicit base branch with `!b`, the task-page footer or Base field, palette **set base** (including marked sets), or `tsk add/edit --base`; `--clear-base` restores the default and `tsk dispatch --base` overrides one launch. Bases must be branches in the task repository, not tags or commits; unknown branches refuse with `unknown-base`. Without an explicit base, dispatch uses that repository's remote default (`origin/HEAD`), never the board's or CLI's checkout. The task footer shows the default branch from a cache, falling back to `⎇ default`; the branch picker opens at once on the local and origin branches already on disk and refreshes them in place after a background fetch, keeping your selection. A bounded best-effort fetch refreshes the base, using a local branch's remote upstream when present and reporting offline fallback. The recorded ref and commit appear on the task page, and `{base}` in agent templates is the short base branch name. Dispatch runs on macOS and Linux only; Windows refuses it with `unsupported-platform`. A second `ctrl+g`, **dispatch again**, or `--again` relaunches with the recorded base, ignoring later overrides; a cleaned record reopens a retained branch or recreates a removed branch from its original starting commit. Dispatch, the branch picker and cleanup share a 60-second fetch window, so a remote fetched that recently is not fetched again, even across `tsk` commands. Completing from the board can safely clean the worktree first, from a card that opens at once and shows `checking…` until a background fetch confirms merged status (`y` meanwhile is queued, and keeps the branch if the check never finishes), while `tsk clean T<n> [--json]` and `tsk status T<n> done --clean` provide the same cleanup for scripts. Marking done a marked set that holds live dispatches opens one card listing each of them with its merged status and what `y` will do; `y` cleans every clean worktree and completes the whole set as one undo step, `n` completes the set and keeps everything, and `Esc` keeps the marks. The card's `[x]` and footer choices are clickable. Cleanup refuses dirty worktrees, keeps unmerged branches, removes merged branches, retains a cleaned dispatch record, and only live started dispatches show `◉`.

### Fixed

- The verb legend on an assigned task shows dispatch as `ctrl+g dispatch`, matching its chord, instead of a bare `g`.
- With tasks marked, `Esc` in the thread or view picker closes the picker instead of clearing the marks behind it and leaving it open.
- The task page's project chooser lists options top to bottom, so `↓` moves the highlight down instead of up.
- Completing a dispatched task from the board always offers worktree cleanup again: a stale prunable worktree left behind by another tool no longer silences the offer and completes the task without a prompt.
- Dispatch and cleanup failures report Herdr's error message in plain language instead of pasting its raw JSON error document onto the status line.
- Dispatch cleanup is bound to the recorded non-root Git worktree and matching Herdr workspace, fetches before checking branch ancestry against the recorded base instead of the current checkout, so remote merges count without a local pull, and keeps legacy branches whose base is unknown. New records retain the fully qualified base namespace. When cleanup cannot fetch the recorded base, it never trusts cached refs: the clean worktree is removed but the branch is kept with `could not reach <remote> to confirm the merge` (the card shows `not confirmed (offline)`), so a force-pushed or reset base cannot cost an unmerged branch; missing or pruned bases keep the branch without blocking clean worktree removal. Squash hints appear only for failed ancestry, and other kept branches report their actual reason. Branch-picker reopens share one in-flight lookup per project, and cleanup worktree listings and heavy status/ancestry queries use a separate 10-second deadline with safe timeout refusals or branch retention. Task metadata footers wrap with matching clickable regions, and unchanged bases do not block unrelated task edits.
- Board completion now converges an already-missing dispatched worktree to cleaned in the same save, while already-done or archived tasks and archived project views bypass the cleanup prompt.
- A malformed `config.toml` no longer blocks the board or CLI add/edit operations that do not assign a task; assignment reports the configuration error without saving.
- Assignee choices remain available after task-page selection changes and in the wide projects preview; mouse picking follows the same marked-set route as the keyboard, and stale marked targets cannot affect a later task.

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
