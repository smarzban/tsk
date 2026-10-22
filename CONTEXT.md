# Glossary

Canonical terms for this repo, one short definition each. The docs pages own the behaviour;
this file names things.

## CLI contract

- **data command**: a verb that reads or writes the board: `add`, `list`, `status`, `edit`,
  `steps`, `reply`, `dispatch`, `clean`, `archive`, `unarchive`, `project`, `trash`,
  `backup`, `restore`. Takes `--state-dir` and follows the exit contract.
- **add command**: `tsk add`, which creates tasks without opening a TUI.
- **list command**: `tsk list`, which prints tasks without changing them.
- **status command**: `tsk status`, which sets a task's human status. `start` is an alias
  for `started`.
- **edit command**: `tsk edit`, which changes a task's title, notes, assignee, base, or
  `after` list. It never changes scope or thread.
- **bulk add**: one `add` of many items. It reports each item and makes one durable write
  of the successes.
- **invocation default**: the scope `add` uses when no project or `--desk` is given: the
  current repo when known, otherwise the desk.
- **tiny result**: the JSON report of a plan-shaped `add`. `created` and `existing` rows
  carry index, id, and title; `failed` rows carry index, title (or null), error code, and
  human text. Notes are not echoed.
- **exit contract**: what a data command's exit code means: `0` done or already true, `1`
  refusals (retry only the failed items), `2` usage error (nothing saved), `3` storage
  error (read back before retrying). See `cli.md`, Exit contract.
- **error code**: a stable token on a refusal, such as `empty-title` or `unknown-project`.
  Scripts match the code; the human text is not a contract.
- **unknown-project**: the refusal when `add` names a project tsk cannot resolve: a bare
  name that matches no known project or several, or a path that is not an existing
  absolute directory. `tsk project archive` and `unarchive` use it for a project with no
  tasks. Reads such as `list -p` stay permissive.
- **persisted**: a later `list` against the same store returns the task.
- **nothing persisted**: a later `list` against the same store shows no task from that
  invocation.
- **listed**: not soft-deleted, so a `list` view returns it; done and archived tasks need
  `--done` or `--archived`.
- **exactly once**: one `list` row per task id.
- **global flags**: the closed set `--help`, `--version` (`-V`), and the launcher lookups
  `--find-board-pane` and `--find-board-tab`. Each works alone, before any command;
  nothing else selects a surface.

## Tasks & steps

- **human status**: `open`, `ready`, `started`, `blocked`, `review`, or `done`. The board's
  source of truth, never derived from agent state.
- **task number**: the board-wide integer a task gets when it is created, shown as `T12`.
  Unique across the desk, every project and thread, done and deleted tasks included, and
  never reused. An address, not identity and not an order.
- **working task**: a task that is not done, archived, or soft-deleted.
- **desk**: the scope for tasks not tied to a project. Selected by `--desk` and by a bare
  `!p`.
- **project**: the directory a task belongs to, usually a Git repo root, named by its
  basename.
- **thread**: an optional name grouping tasks in the same scope. Not an entity: no
  identity beyond the name, no status, no lifecycle. It shows on the board while at least
  one working task in its scope carries it; the task keeps the name whatever its status.
- **thread token**: the `!t name` capture directive, sibling of `!p`; bare `!t` means
  unthreaded. Stripped from the saved title.
- **unthreaded**: carrying no thread. Unthreaded tasks stay in the project's flat board.
- **base token**: the `!b branch` capture directive, sibling of `!p` / `!t` / `!a`. Sets
  the task's dispatch base branch. Stripped from the saved title.
- **after token**: the `!w T12` capture directive ("wait for"), sibling of `!p` / `!t` /
  `!a` / `!b`. Adds a prerequisite by task number (`!w 12` works too). Repeatable.
  Stripped from the saved title.
- **after / before**: a task runs *after* the tasks it names by number and waits while any
  of them is not done. Each unfinished prerequisite shows its waiters as *before*. When the
  last prerequisite is done, a waiting `ready` task starts.
- **block**: what a `blocked` task carries: why, what it needs, optional numbered options,
  and who or what it waits on (you, a task, an agent, or something else). It closes when
  the task leaves `blocked`.
- **reply**: an entry in a block's thread, from you or the agent. Unblocking sends the
  unsent replies to the running agent; `tsk reply --send` sends one at once.
- **review round**: what a task in `review` carries: what was done, checks, what is next,
  and who the review is on. Rounds are numbered from 1 and close as approved (done), sent
  back (started), or unresolved by any other route out of review.
- **check**: one item a review round asks the reviewer to verify: open, passed, or failed.
  Checks never become steps.
- **feedback**: a reply in a review round. A send-back sends the unsent feedback and the
  failed checks to the running agent.
- **paper trail**: a task's history, newest first: who did what and when, including its
  closed blocks and review rounds. Shown on the task page and by `tsk list T12`.
- **steps**: the flat ordered list of steps on one task, one level deep. Order changes only
  by an explicit insert (`steps add --before/--after`, `ctrl+a` below the step cursor) or a
  move.
- **step**: one entry in a task's steps: a stable identity, one line of text, and a done
  flag.
- **step done**: a step's checked state. Progress only: it never implies or changes a task
  status.
- **toggle**: flip one step's done flag. The only step change that is not a text change.
- **move**: change one step's position in its task's list. The step keeps its identity,
  short id, and done flag; a move onto its own slot changes nothing.
- **step short id**: an unambiguous prefix of a step's identity, used to address a step
  within one task from the CLI.
- **step cursor**: the task page's focus on a step row. Inactive when the page opens; a
  bare `↓` activates it and `↑` from the first step deactivates it. While active, bare
  arrows move it and modifier verbs act on its step; a click on a step row moves it there.

## Board surfaces

- **NEEDS YOU**: the section of blocked and review tasks that wait on you, including a block
  on a task that is now done or deleted. The desk shows every scope's; a project board
  shows its own. Omitted when empty; sits above IN MOTION.
- **IN MOTION**: started tasks, plus blocked and review tasks that wait on an agent, an
  unfinished task, or something else.
- **ON DECK**: ready and open tasks. On the desk it holds desk tasks only.
- **row label**: the dim `#thread · project` after a row's title (`desk` for a desk task).
  It leaves out what the view already says, and leads with `waiting on …`, `after …`, or
  `before …` when they apply.
- **live line**: the dim `└─` line under a row for what is addressed to you: the asker's
  words on a block or review on you, or a blocking task now done or deleted.
- **peek**: the dim preview under a row, opened below 110 columns by a click or `→`. It
  shows the notes, or a block's or review's summary, then a metadata footer, and replaces
  the live line while open.
- **notice**: a seeded desk task with an `N` id: a starter task or a `What's new in tsk`
  release note. Human-only: the CLI never lists or addresses it.
- **wide stage**: the slider position of a board 110 columns or wider: `0` full board, `A`
  split, `G` rail, `F` full task. Focus and layout follow it. Session-only; it survives
  resizes.
- **rail**: stage `G`'s 32-column dim board column: tabs, section rules, and wrapped task
  rows, without the meta column. It keeps the board's done drawer and archived fold state.
- **single-pane view**: a presentation that shows only the focused board or task surface.
- **focused surface**: the board or task view that owns interaction. At 110 columns or
  wider it follows the wide stage (`0`/`A` board, `G`/`F` task); below, the single-pane
  view.
- **session**: one board process from launch to quit. Session-only state (folds, wide
  stage, the launch card having been shown) resets on relaunch.

## Archive & hiding

- **archived**: a task flag meaning "kept, off the radar". The task keeps its human status,
  leaves every working lens, and shows only in the done drawer's archived group. Not a
  status, not a place; nothing expires.
- **archived project**: a project carrying the archived flag. It leaves the Projects view,
  the desk's sections, and the picker's main list; it shows only on the picker's archived
  tab. Its tasks' own archived flags are independent of it.
- **project record**: the store's entry for one project, keyed by its path. It exists only
  while the project is archived; the projects the board shows are still derived from
  tasks.
- **archived group**: the `archived · n` fold and its dimmed rows inside the done drawer.
  Folded by default; the fold is session-only.
- **hidden**: archived, or in an archived project. The one test every working lens filters
  on; distinct from soft-deleted.
- **working lens**: any view meant for current work: the desk, a project board, the
  Projects view, a thread or assignee view, NEEDS YOU, IN MOTION, ON DECK, the rail, and
  default `tsk list`. Hidden tasks and projects never show in one.
- **file**: the `ctrl+f` verb. On a task it toggles the archived flag, with an undo entry.
  In the project picker it archives the selected project, or unarchives it on the archived
  tab, with no undo entry. `ctrl+u` on an archived selection also unarchives.

## Select & search

- **multi-select**: the board mode `shift+M` enters and leaves. While it is on, `Space`,
  `shift+↑`/`shift+↓`, and a plain click mark tasks; outside it those keys keep their usual
  meaning. Session-only; it clears on a verb, `Esc`, or a lens change.
- **marked set**: the tasks marked in multi-select, painted `▪` in the gutter beside the
  cursor's `▸`. Status, archive, delete, and assignee verbs act on the set when it is
  non-empty, otherwise on the cursor. Never persisted.
- **batch undo**: the one undo entry a verb on a marked set records. One `ctrl+u` reverses
  all of it; a stale batch refuses without changing any task. A start that dispatches is
  the exception: only its start-only tasks share the entry, and its launches are not
  undoable.
- **board search**: the `/` filter over the rows the current view shows: project rows on
  the Projects view, task rows (title, notes, steps, thread, number) everywhere else. Every
  query word must match. **Pinned** once `Enter` hands keys back to the board; `Esc` clears
  it before closing anything else.

## Agents & dispatch

- **agent profile**: a named entry in `config.toml` that says how to launch one agent: an
  argv template with placeholders, an optional prompt template, and optional environment.
  tsk knows nothing about any harness's flags.
- **assignee**: the name a task may carry, shown as `@name`: an agent profile, the maintainer
  (`you`), or a person. It never changes status or triggers anything by itself.
- **assignee kind**: whether an assignee is an agent or a person, stored on the task. Only an
  agent dispatches; a removed profile stays an agent. Older stores' assignees are agents.
- **name rule**: how every typed assignee or "on" value reads: `you`, `me`, or the configured
  `[board] me` name is you; a profile name is that agent; any other name is a person (or
  other text for "on"); a task number is a task for a block's "on".
- **owner name**: `[board] me` in `config.toml`, shown wherever the board names you as a
  handle. Stored data and JSON keep `you`.
- **assignee token**: the `!a name` capture directive, sibling of `!p` and `!t`; bare `!a`
  means unassigned. The name follows the name rule.
- **unassigned**: carrying no assignee.
- **dispatch**: what starting an assigned, never-dispatched task does (`ctrl+s`,
  `tsk status T12 started`, `tsk dispatch`). It creates a worktree from the task's base,
  launches the assignee's profile there in a Herdr workspace, records the launch on the
  task, and sets `started`.
- **start route**: how a start treats a task: plain for an unassigned task, a running agent,
  or the agent's own start; dispatch for an assigned task never dispatched; ask (board) or
  refuse `agent-gone` (CLI) when a dispatched agent is gone.
- **dispatch record**: what dispatch writes on the task: the rendered command, worktree
  path, branch, base and starting commit, Herdr workspace, and time. It stays through later
  status changes and is marked cleaned after cleanup; a stale record renders as-is.
- **dispatched**: carrying a dispatch record. Independent of human status.
- **base**: the branch a dispatch starts from: the task's explicit base, otherwise the
  repository's remote default (`origin/HEAD`).
- **cleanup**: removing a dispatch's worktree, Herdr workspace, and merged branch, offered
  when the task is completed. It never removes uncommitted work or a branch it cannot
  confirm is merged, by Git ancestry or, through `gh`, a pull request merged at exactly the
  branch tip.
- **merge (a task's PR)**: `m` on the cleanup card, or `--merge` on `tsk status T12 done`
  and `tsk clean T12`. Merges the task's open pull request through `gh` with the merge
  method, then completes and cleans up. tsk finds the pull request by branch name, stores
  nothing about it, and never deletes remote branches.
- **merge method**: `[merge] method` in `config.toml`: `merge`, `squash`, or `rebase`.

## Store & backup

- **soft-deleted**: a deleted task still in the store, so undo can bring it back. It moves
  to trash once undo can no longer restore it, or after seven days.
- **backup**: a copy of the store and `config.toml` (secrets removed) committed to a git
  repository set up with `tsk backup setup` or `tsk backup add`. `tsk restore` puts a
  version back.

## Docs

- **guide**: the agent workflow doc, `skills/tsk-cli/SKILL.md`, printed by `tsk guide` and
  published at `/docs/agents/`.
- **twin**: the Markdown version of a docs page, at `/docs/<slug>.md`.
- **bootstrap prompt**: the short text a person pastes to an agent to start it on tsk
  (`Use tsk for my task board. Run tsk guide and follow it; …`).
- **definition sentence**: `tsk is a terminal task board for you and your agents: one
  shared queue, a TUI for you, a CLI for them.`
