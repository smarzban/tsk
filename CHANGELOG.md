# Changelog

One `## vX.Y.Z` section per release, newest first, with `## Unreleased` on top. Inside a
section the subsections are, in this order and only when non-empty: `### Breaking`,
`### Added`, `### Changed`, `### Fixed`. Every user-visible change lands here in the PR
that makes it, written for a user, not a contributor: omit demo alignment, CI wiring,
review history, and other maintainer-only work. On release the version's section becomes
the GitHub release notes verbatim.

Each change is one line of at most 30 words; details live in the docs. A fix to something
first added in the same Unreleased section is not an entry: edit that entry instead.

## Unreleased

### Breaking

- A new store format (assignees, dispatch, blocks, reviews, after, paper trail, wider undo). Older tsk refuses it; to roll back, restore the `tsk.json.v5` copy.

### Added

- Agent profiles in `config.toml`. The first board open writes a commented starter with examples for Claude Code, Codex, Pi, and Grok.
- Assign a task to an agent profile with `@`, `!a name`, or `tsk add`/`tsk edit --assignee`. `v` on Projects shows one assignee's tasks across projects.
- Dispatch: in Herdr, starting an assigned project task (`ctrl+s`, `tsk status T12 started`, `tsk dispatch`) launches its agent in its own Git worktree and workspace. Windows: preview.
- Set the branch a dispatch starts from with `!b branch`, the task page's Base field, or `tsk add`/`tsk edit --base`; otherwise it is the repository's default branch.
- A dispatched task whose agent is gone asks before relaunching in the same worktree (`--again` in the CLI). Starting a marked set opens one card listing what launches.
- Completing a dispatched task offers to remove its worktree and merged branch; with `gh`, squash and rebase merges count. `tsk clean` does the same from a script.
- `m` on the cleanup card merges the task's pull request with `gh`, then completes the task and cleans up. `[merge] method` in `config.toml` picks the method; CLI: `--merge`.
- Block with a reason: `ctrl+b` asks why and what you need. Answer on the task page, under the row (`r`), or with `tsk reply`. Unblocking sends replies to the agent.
- Review with checks: `ctrl+r` asks what was done and what to check. Mark checks, give feedback (`r`, `tsk reply`), then approve (`ctrl+d`) or send back (`ctrl+s`).
- Paper trail: the task page lists who did what, newest first, below the steps, and drops created and updated dates from its footer. `tsk list T12` shows it as `activity`.
- After: a task can wait for other tasks in any project (`!w T202`, the After field, `tsk edit --after`). When the last one is done, a waiting ready task starts.
- Agents block and hand back for review with `tsk status`, and use `tsk dispatch`, `tsk clean`, and `tsk reply`. Agent skill 1.6.0 covers them; `tsk update` refreshes installed copies.
- `ctrl+u` undoes status changes, step changes, and archiving; a marked set is one step. Undoing a start that launched an agent leaves the agent running. Thanks @LukasGold (#173).
- Group and sort by assignee with `g` and `s` or `tsk list --group assignee` and `--sort assignee`. The `t` filter gains an `@assignees` tab.
- `[board]` in `config.toml` sets the group and sort a fresh desk or project board opens with.
- Back up tasks and `config.toml` to git on every save or daily. `tsk backup setup` makes a private GitHub repository with `gh`; `tsk backup add <git-url>` adds any other.
- `tsk restore` lists the newest 20 backed-up versions per machine (`--all` for every one) and puts the one you pick back, keeping the replaced store.

### Changed

- `ctrl+b` and `ctrl+r` open the block and review cards; `Enter` on an empty card still blocks or sets review at once.
- A footer too narrow for its actions drops whole actions, least likely first, instead of cutting the last one with `…`.
- Word jumps match macOS Option+arrow: `alt+→` or `ctrl+→` lands at the end of the next word, `alt+←` or `ctrl+←` at the start of the previous one.
- A row whose title wraps above an open peek or live line shows a dim `│` beside each wrapped line, so the row reads as one block.
- Help, footers, and the palette name each action verb first and in full (`ctrl+b` **block / unblock**). Help's **SURFACE CONTROLS** group is renamed **PICKERS & EDITING**.
- The archived-project launch card puts each option on its own line; `↑`/`↓` and `Enter` pick one, as on the cleanup card.

### Fixed

- `ctrl+u` names what it undid or why it refused, and undoing a done puts back the task's earlier status instead of open. Thanks @LukasGold (#173).
- Saving a task with a step's text cleared reopens that step with `text required` on its row, as for an empty new step. Thanks @LukasGold (#173).
- A project reached through a symlinked path, such as `/tmp/repo` and `/private/tmp/repo` on macOS, is one project.
- With tasks marked, `Esc` in the thread or view picker closes the picker and keeps the marks.
- With tasks marked, `ctrl+b` and `ctrl+r` skip done tasks, as starting does, and the status row counts them.
- `Esc` on a selected Thread on the task page cancels only the thread, not the whole edit.
- `ctrl+x` in a new step's row discards that row, instead of removing the step above it or deleting a task.

## v0.12.0

### Added

- Board rows show their thread and project as a dim `#thread · project` label (`desk` for a desk task), leaving out whatever the view already says. Thanks @LukasGold (#175).
- Group, sort, and fold the board: `g` groups by project or thread, `s` sorts by recent, project, thread, or title, `f` folds the group under the cursor, and `Shift+F` folds every group. Group headers are cursor stops: `←` / `→` fold and unfold, `Enter` opens the group's board, and `Shift+M` marks the whole group. Also as clickable `group:` and `sort:` controls and in the `:` palette. Thanks @LukasGold and @GalShnapp (#175, #158).
- Filter the desk by thread and project with `t` (or the `all ▾` control), so a thread that spans repositories reads as one initiative. Thanks @GalShnapp (#158).
- Reorder steps: `alt+↑` / `alt+↓` moves the selected step and `ctrl+a` adds a step below it. From the CLI, `tsk steps T12 move <id> --before|--after <id>` (or `--first` / `--last`), and `tsk steps T12 add` takes the same place flags. Thanks @LukasGold (#159).
- Text editing keys in every field: macOS word moves and deletes (`alt+←` / `alt+→`, `alt+Backspace` or `ctrl+w`, `alt+Delete` or `alt+d`), and `ctrl+u` / `ctrl+k` to delete to the line start or end. The footer shows how to start a new line in Notes. Thanks @maximilliangeorge (#204).
- `tsk list --group project|thread`, `--sort recent|project|thread|title`, and `--no-thread`. `tsk list --all --thread X` lists a thread across every project and the desk.

### Changed

- In a text field `ctrl+a` moves to the line start, so macOS `Cmd+←` no longer adds a step while you type. It still adds a step when no field is open (#204).
- Bare `g` opens **group by** instead of folding the inbox or archived group; fold with `f` or `Shift+F`. `Enter` and a single click on those headings no longer fold them.
- Agent skill 1.5.0 covers threads across projects, grouping, and step reordering. Rerun `tsk setup` to update installed copies.

### Fixed

- Windows: `tsk update` and the board's update check no longer crash on the first HTTPS request, and `tsk add --file` and piped JSON plans accept the byte order mark Windows PowerShell 5.1 writes.
- Windows: characters typed with AltGr, such as `[ ] { } @` on a German layout, insert in every text field. Thanks @LukasGold (#178).
- `tsk setup herdr` follows a symlinked Herdr config, as dotfiles managers create, instead of failing with `Too many levels of symbolic links`. Thanks @yasith (#129).
- `tsk setup` shows terminal control characters in paths as visible escapes instead of printing them raw.
- `tsk list` orders tasks within each status like the board does. Thanks @dangerousdan (#123).
- `tsk list` refuses a repeated `-p`, `--thread`, or `--state-dir` (exit 2) instead of silently keeping the last one.
- A form that receives `Shift+Enter` as `ctrl+j` (such as Ghostty's `shift+enter=text:\n` binding) says so instead of ignoring the key. Thanks @WAWLixiong and @Rag0n (#125).
- Done drawer and archive: Help says where archived tasks live, `ctrl+s` on a marked set says how many done tasks it skipped, and at 110+ columns the Task view keeps the done drawer and archived fold. Thanks @LukasGold (#174).
- The `Deleted T12 "…" · ctrl+u undo` line clears on your next move or after a few seconds; the footer keeps offering undo while it applies.
- The task page's project chooser lists top to bottom, so `↓` moves down.

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
