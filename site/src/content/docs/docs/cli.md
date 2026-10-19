---
title: CLI
description: Commands, arguments, output, and retry rules for the shared task board.
---

The `tsk` command reads and changes the same tasks as the board, and a running board shows each change. This page is the reference for every command, flag, output, and exit code. Agents start with [tsk for agents](/docs/agents/), the workflow `tsk guide` prints.

```sh
tsk add -t "Fix login timeout"
tsk list
```

## Commands

| Command | Does |
| --- | --- |
| `tsk` | Open the board |
| `tsk capture` | Open [quick capture](/docs/capture/); saving or discarding exits |
| [`tsk add`](#add) | Add a task, or every task in a JSON plan |
| [`tsk list`](#list) | Read tasks |
| [`tsk status`](#status) | Set a task's status, block it, or put it up for review |
| [`tsk edit`](#edit) | Change a task's title, notes, assignee, base, or after list |
| [`tsk steps`](#steps) | Add, toggle, rename, move, or remove a step |
| [`tsk reply`](#reply) | Answer a block, or give feedback on a review |
| [`tsk dispatch`](#dispatch) | Launch a task's assigned agent in its own worktree |
| [`tsk clean`](#clean) | Remove a dispatched task's worktree and merged branch |
| [`tsk archive`](#archive) / [`unarchive`](#unarchive) | Hide a task, or put it back |
| [`tsk project`](#project) | Archive or unarchive a project |
| [`tsk trash`](#trash) | Restore a deleted task |
| [`tsk backup`](#backup) | Back the store up to git repositories |
| [`tsk restore`](#restore) | Put a backed-up version of the store back |
| [`tsk setup`](#setup) | Register Herdr, or install the agent skill |
| [`tsk update`](#update) | Install the latest stable release |
| [`tsk guide`](#guide) | Print the agent workflow |

- `tsk --help` lists the commands. `tsk help <command>` and `tsk <command> --help` print the same reference for one command: usage, flags, examples, refusals, and exit codes. Help wraps at 80 columns.
- `tsk --version` (or `-V`) prints `tsk <version>`. Put it before any command.
- Every command that reads or writes tasks takes `--state-dir <dir>` to use another store. `setup`, `update`, and `guide` do not.
- A value that starts with `-` needs the `=` form: `--title="-fix parser"`, `--notes="-5 degrees"`. [`tsk reply`](#reply) takes `--` before such text instead.
- Human output escapes terminal control characters stored in tasks. JSON keeps the text as stored.

## Statuses

| Status | Meaning |
| --- | --- |
| `open` | Captured, not yet picked (inbox) |
| `ready` | Picked, up next (on deck) |
| `started` | In motion |
| `blocked` | Waiting on something |
| `review` | Done by the agent, waiting on you |
| `done` | Closed |

New tasks start `open`. Nothing sets `done` automatically.

## Addresses

`T12`, `t12`, `12`, and the task's UUID all name the same task, in any project.

Rows the board paints as `N1`, `N2`… are notices: starter tasks and release notes. No command addresses them and `tsk list` leaves them out.

## Scope

`add` and filtered `list` use the repository you run them in, or the desk outside Git. A command given a task address ignores the scope.

| Flag | Scope |
| --- | --- |
| `--desk` | The desk |
| `-p <name>`, `--project <name>` | The one project on the board with that basename, ignoring case |
| `-p <path>` | An existing absolute directory (`/…`, `~/…`, `C:\…`, `~\…`) |
| `--all` (`list` only) | Every project and the desk |

- A bare name for `add` must match exactly one project the board knows: one with tasks, an archived one, or the repository you are in. Otherwise it refuses with `unknown-project`.
- Only an absolute path to an existing directory creates a new project. Relative and missing paths refuse.
- `tsk list -p <name>` never refuses: an unknown name lists nothing.

## add

Creates one task from flags, or every task in a JSON plan.

```text
tsk add -t <title> [-n <notes>] [-p <project> | --desk] [--thread <name>] [--assignee <name> | --unassign] [--base <branch> | --clear-base] [--json] [--state-dir <dir>]
tsk add [--file <path|->] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `-t`, `--title <title>` | Title, required without a plan |
| `-n`, `--notes <notes>` | Notes; blank notes are left out |
| `-p`, `--project`, `--desk` | [Scope](#scope) |
| `--thread <name>` | [Thread](/docs/capture/) label |
| `--assignee <name>` | An agent profile from [`config.toml`](/docs/storage/#configuration) |
| `--unassign` | No assignee |
| `--base <branch>` | Dispatch from this existing branch of the project ([base branch](/docs/dispatch/#base-branch)) |
| `--clear-base` | Dispatch from the project's default branch |
| `--file <path>`, `--file -` | Read a JSON plan from a file, or from stdin |
| `--json` | Print the result as one JSON object |

An add is safe to repeat. A live task with the same title, project, thread, assignee, and base counts as already there, even when done or archived: the add changes nothing and reports `existing`. Adding into an archived project refuses with `project-archived`.

Output is `added <title>`, or `task already exists`. With `--json`:

| Field | Value |
| --- | --- |
| `outcome` | `created` or `existing` |
| `id`, `number`, `title` | The task |
| `project` | Project path, or `null` on the desk |
| `assignee` | Agent profile, or `null` |

```sh
tsk add -t "Fix login timeout" -n "Reproduce on a slow connection" -p atlas --thread auth
```

### JSON plans

A plan is a JSON array of task objects, read from `--file` or piped to `tsk add`.

```json
[
  {"title": "Reproduce login timeout", "project": "atlas", "thread": "auth", "assignee": "claude"},
  {"title": "Draft release notes", "notes": "Include migration steps", "after": [202]}
]
```

| Field | Value |
| --- | --- |
| `title` | Required |
| `notes` | Notes |
| `project` | A project name or path. Omitted: the default [scope](#scope). `null`: the desk |
| `thread` | Thread name, or `null` |
| `assignee` | Agent profile, or `null` |
| `base` | Dispatch base branch, or `null` for the default branch |
| `after` | Tasks this one [runs after](/docs/board/#after), such as `[202]` or `["T202"]`, in any project |

The result is one JSON object with three arrays: `created`, `existing`, and `failed`. Each entry carries the item's index `i`. Created and existing entries add `id`, `number`, and `title`; failed ones add `title`, `code`, and `error`.

- Valid items save even when others fail. Retry only the failed items.
- An item's refusal codes include `invalid-item`, `invalid-thread`, and `invalid-after` (an unknown or done task).
- Item flags and `--file` do not mix. With item flags, piped input is ignored.
- A leading byte order mark, which Windows PowerShell 5.1 writes, is ignored.

```sh
cat plan.json | tsk add
```

## list

Reads the tasks in a scope, or one task in full.

```text
tsk list [<task>] [-p <project> | --desk | --all] [--thread <name> | --no-thread] [--assignee <name>] [--open | --ready | --done | --deleted | --archived] [--group <by>] [--sort <by>] [--json] [--state-dir <dir>]
```

| Flag | Lists |
| --- | --- |
| none | Live tasks: `open`, `ready`, `started`, `blocked`, and `review`, not archived |
| `<task>` | That task in full, wherever it is, including done, archived, and recently deleted. Takes no other filter |
| `--thread <name>` | Tasks on that thread. With `--all`, across every project and the desk |
| `--no-thread` | Tasks without a thread |
| `--assignee <name>` | Tasks assigned to that agent profile |
| `--open`, `--ready` | Only that status |
| `--done` | Done tasks |
| `--archived` | Archived tasks, and tasks in archived projects, in any status |
| `--deleted` | Deleted tasks, newest first, kept for 30 days ([deleted tasks](/docs/storage/#deleted-tasks)) |
| `--group <by>` | Live tasks under each status, grouped by `none`, `project`, `thread`, or `assignee`, hottest group first |
| `--sort <by>` | Live tasks ordered by `recent` (the default), `project`, `thread`, `title`, or `assignee` |
| `--json` | JSON instead of text |

`--group` and `--sort` work like the board's [group and sort](/docs/board/#group-and-sort). They refuse with `--done`, `--archived`, or `--deleted`, and never arrange by something every row shares: `project` needs `--all`, `thread` refuses with `--thread` or `--no-thread`, and `assignee` with `--assignee`.

Scope flags exclude each other, as do the status filters, and `--thread` with `--no-thread`. A flag that takes a value given twice is a usage error. An unknown task address exits 2.

```sh
tsk list --all --thread release --json
```

### Text output

- Tasks group by status in the order `STARTED`, `READY`, `OPEN`, `BLOCKED`, `REVIEW`, and keep the board's order inside each status.
- A row reads `- 12 Fix login timeout @claude ⎇ main #auth`. A blocked row adds `blocked: <why>` beneath.
- With `--all`, each status groups its rows by project and desk. A sort other than `recent` lists the rows flat instead, each ending with its scope.
- With `--group`, sub-headings name each group as the board paints it.
- One task in full adds its notes, steps, `@assignee`, `⎇ base`, `after` and `before` tasks, `#thread`, and its latest five [paper trail](/docs/task-page/#paper-trail) entries.
- Text wraps to the terminal width. Redirected output keeps each line whole.

### JSON fields

`--json` prints one array in the listed order. Every row has these fields:

| Field | Value |
| --- | --- |
| `id` | Task UUID |
| `number` | Task number: `12` for T12 |
| `title`, `status` | Title and status |
| `project` | Project path, or `null` on the desk |
| `thread`, `assignee`, `base` | Thread, agent profile, and explicit base branch, each `null` when unset |
| `after` | Tasks this one runs after, in order, each `{"task": 202, "done": false}`. A task gone from the board reads as done |
| `before` | Numbers of the tasks that run after this one |
| `archived` | With `--archived`: `archived` or `project archived` |
| `group` | With `--group`: the group label |

A task waits while any `after` entry has `done: false`. A filtered row does not say who a block or review waits on: read the task in full for that.

`tsk list T12 --json` returns the task in full: the fields above plus these.

| Field | Value |
| --- | --- |
| `notes` | Notes, or `null` |
| `steps` | Each step's `id`, `done`, `short_id` (for [`tsk steps`](#steps)), and `text` |
| `dispatch` | Present once dispatched: `argv`, `worktree`, `branch`, `base`, `base_ref`, `base_commit`, `base_remote`, `herdr_workspace_id`, `at`, and `cleaned: true` once cleaned |
| `block` | The open block, below |
| `review` | The open review round, below |
| `past_blocks`, `past_reviews` | Closed blocks and rounds, oldest first. Each adds `closed_at` and `closed_by` |
| `activity` | Every event on the task, newest first, below |

Times such as `at` are `[seconds, nanoseconds]` since the Unix epoch.

**Block**

| Field | Value |
| --- | --- |
| `why`, `needs` | What stops the work, what would unblock it; `null` when unset |
| `options` | Suggested answers |
| `on` | Who it waits on: `you`, `task:<number>`, or `other:<text>` |
| `by`, `at`, `edited` | Who opened it, when, and whether it was edited since |
| `replies` | Each `at`, `by`, `text`, plus `edited: true` and `sent: true` when they apply |
| `answered` | `true` when the last reply is yours |

**Review round**

| Field | Value |
| --- | --- |
| `round` | Round number, from 1 |
| `done`, `next` | What was done and what comes next; `null` when unset |
| `checks` | Each `text` and `state`: `open`, `passed`, or `failed` |
| `on` | Who reviews: `you`, `agent:<profile>`, or `other:<text>` |
| `by`, `at`, `edited`, `answered` | As on a block |
| `feedback` | Shaped like a block's `replies` |
| `resolution` | On a closed round: `sent_back` when a start closed it, `approved` when done did |

**Activity**

| Field | Value |
| --- | --- |
| `at` | When |
| `by` | `you`, the agent profile that made the change, or `null` on old events |
| `kind` | `created`, `edited`, `status_set`, `completed`, `reopened`, `soft_deleted`, `restored`, `archived`, `unarchived`, `step_added`, `step_checked`, `step_unchecked`, `step_renamed`, `step_removed`, `assigned`, `base_set`, `dispatched`, `cleaned`, `block_edited`, `replied`, `reply_edited`, `reply_deleted`, `review_edited`, or `check_set` |
| `detail` | What changed, or `null`: `from` and `to` on a status change; `assignee` on `assigned`; `base` on `base_set`; `branch`, `base`, `sha`, and `relaunch` on `dispatched`; `outcome` (`removed`, `branch_kept`, or `missing`) on `cleaned`; `fields` on `edited`; `after` on a start that finishing a prerequisite caused |

## status

Sets a task's status. Starting an assigned task launches its agent, and done starts the ready tasks that wait on it.

```text
tsk status <task> <status> [--state-dir <dir>]
tsk status <task> started [--no-dispatch | --again] [--force]
tsk status <task> done [--clean | --merge]
tsk status <task> blocked [--why <text>] [--needs <text>] [--option <text>]... [--on <who>]
tsk status <task> review [--done <text>] [--check <text>]... [--next <text>] [--on <who>]
```

`<status>` is `open`, `ready`, `started` (or `start`), `blocked`, `review`, or `done`. The command sets that status directly, and repeating it is safe. Output is `status T12 <status> <title>`, sometimes followed by a second line that says what else happened.

When an agent dispatched by tsk runs the command, it acts as that agent: blocks, reviews, and replies carry its profile name, and its own start never launches another copy. Everyone else acts as `you`. See [storage](/docs/storage/) for the `TSK_AGENT` variable behind this.

```sh
tsk status T12 ready
```

### Start

`started` does what starting does on the board: an assigned task gets its agent going.

| Task | `tsk status T12 started` |
| --- | --- |
| Unassigned, already started, done, or archived | Sets the status |
| Assigned, never dispatched | [Dispatches](#dispatch) it and prints `dispatched T12 to @claude in <worktree>` |
| Assigned, but outside Herdr or on the desk | Sets the status and prints `no launch: <reason>` |
| Dispatched, agent still running or Herdr cannot tell | Sets the status. Coming back from `blocked` or `review`, it also sends the agent your replies or feedback and prints what it sent |
| Dispatched, agent gone | Refuses with `agent-gone` |
| Runs after a task that is not done | Refuses with `after-not-done`, unless `--force` |

| Flag | Meaning |
| --- | --- |
| `--no-dispatch` | Only set the status: never launch or send |
| `--again` | Relaunch a dispatched task whose agent is gone, like [`tsk dispatch --again`](#dispatch) |
| `--force` | Start even though tasks it runs after are not done |

A launch that fails leaves the task unstarted. `ready` on a blocked task whose dispatched agent still runs sends that agent your replies, as the board does.

### Done

`done` closes the task. Each `ready` task that [runs after](/docs/board/#after) it, and now has every prerequisite done, starts the way `started` starts it. A second line says what happened, such as `T13 dispatched to @claude · T12 done`. The done stands even when such a start fails.

| Flag | Meaning |
| --- | --- |
| `--clean` | After done, [clean](#clean) the task's dispatch. With none, it prints `nothing to clean`. A cleanup refusal exits 1 and leaves the task done |
| `--merge` | First merge the task's pull request, then set done and clean |

`--merge` runs `gh pr merge` on the task's branch with the method set under `[merge]` in [`config.toml`](/docs/storage/#configuration). It never passes `--admin`, `--auto`, or `--delete-branch`, and it marks the task done only once GitHub shows the pull request merged. The first line names it: `merged T12 pull request #42 (squash)`. More in [merge the pull request](/docs/dispatch/#merge-the-pull-request).

| Refusal | When | Changed |
| --- | --- | --- |
| `not-dispatched`, `already-cleaned`, `dirty-worktree` | No live dispatch, or uncommitted changes in its worktree | Nothing |
| `gh-unavailable` | `gh` missing or not logged in, or no GitHub remote | Nothing |
| `no-pr`, `pr-not-open` | No pull request for the branch, or it is not open | Nothing |
| `pr-base-mismatch` | The pull request targets another branch than the dispatch base | Nothing |
| `merge-config` | The `[merge]` setting is invalid | Nothing |
| `merge-failed` | GitHub refused, the head is not the local branch tip, or a merge queue only accepted it | Nothing |

After a merge, a failure still exits non-zero: 3 when saving done failed, 1 for a cleanup refusal. If done was not saved, the pull request is merged and the task is not done: run `tsk status T12 done --clean`. A pull request that is already merged refuses `--merge` with `pr-not-open`; clean it with `tsk clean T12`.

```sh
tsk status T12 done --merge
```

### Block

`blocked` opens a block that says what stops the work. Answers come back as [replies](#reply).

| Flag | Meaning |
| --- | --- |
| `--why <text>` | What stops the work |
| `--needs <text>` | What would unblock it |
| `--option <text>` | One suggested answer; repeat for more |
| `--on <who>` | Who it waits on: `you` (the default), another task (`T202`), or any other text |

- Each text is at most 4 KB (`text-too-long`). `--on` must name another task on the board (`invalid-blocker`).
- A task has one open block. Running the command again edits it, replacing only the fields given.
- `blocked` with no flags opens a block with no reason.
- Leaving `blocked` by any route closes the block into `past_blocks`.
- A block on you shows under NEEDS YOU. One on another task or on anything else stays IN MOTION until that changes ([sections](/docs/board/#sections-and-status)).

```sh
tsk status T12 blocked --why "Which database?" --option postgres --option sqlite
```

### Review

`review` opens a review round with what was done and what to check. Feedback comes back as [replies](#reply).

| Flag | Meaning |
| --- | --- |
| `--done <text>` | What was done |
| `--check <text>` | One thing to check; repeat for more |
| `--next <text>` | What comes after |
| `--on <who>` | Who reviews: `you` (the default), an agent profile (`pi` or `@pi`), or any other text |

- Each text is at most 4 KB (`text-too-long`).
- Running the command again during the round edits it, replacing only the fields given. A check with unchanged text keeps its state.
- Leaving `review` closes the round into `past_reviews`. The next review opens the next round.
- A review on you shows under NEEDS YOU. One on an agent or on anything else stays IN MOTION.

```sh
tsk status T12 review --done "Opened PR #42" --check "tests pass" --next "docs"
```

## edit

Changes a task's title, notes, assignee, dispatch base, or the tasks it runs after. Scope and thread stay as they are: to move a task, add it again in the right scope and archive the old one.

```text
tsk edit <task> [--title <title>] [--notes <notes>] [--assignee <name> | --unassign] [--base <branch> | --clear-base] [--after <task>]... [--clear-after] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `-t`, `--title <title>` | Replace the title |
| `-n`, `--notes <notes>` | Replace the notes. Blank clears them; newlines and tabs are kept |
| `--assignee <name>` | Assign an agent profile from [`config.toml`](/docs/storage/#configuration) |
| `--unassign` | Clear the assignee |
| `--base <branch>` | Dispatch from this existing branch, not a tag or commit (`unknown-base`) |
| `--clear-base` | Dispatch from the project's default branch |
| `--after <task>` | Run after this task; repeat for more. Replaces the list |
| `--clear-after` | Run after nothing |

- `--after` takes tasks in any project. It refuses the task itself, a task not on the board, or a done one (`invalid-after`), and a link that would close a loop (`after-loop`). There is no `--before`: set the other task's `--after`.
- Output is `edited T12 <title>`. Repeating the same values is safe.

```sh
tsk edit T205 --after T202 --after T12
```

## steps

Adds, toggles, renames, moves, or removes one step. [Steps on the board](/docs/steps/).

```text
tsk steps <task> add <text> [<place>] [--state-dir <dir>]
tsk steps <task> toggle <step-short-id> [--state-dir <dir>]
tsk steps <task> rename <step-short-id> <text> [--state-dir <dir>]
tsk steps <task> move <step-short-id> <place> [--state-dir <dir>]
tsk steps <task> remove <step-short-id> [--state-dir <dir>]
```

`<place>` is `--before <step-short-id>`, `--after <step-short-id>`, `--first`, or `--last`. `add` appends without one and `move` needs one. A step's `short_id` comes from `tsk list T12 --json`; any prefix of its id that only it has works too.

| Action | Output | Safe to repeat |
| --- | --- | --- |
| `add` | `added <short-id> <text>` | No: adds another step |
| `toggle` | `toggled <short-id> [x] <text>` | No: flips it back |
| `rename` | `renamed <short-id> <text>` | Yes |
| `move` | `moved <short-id> to <position>/<count> <text>` | Yes |
| `remove` | `removed <short-id> <text>` | No: refuses the second time |

A moved step keeps its id and its checked state. Read the task before retrying an uncertain result.

```sh
tsk steps T12 add "Write the failing test" --first
```

## reply

Adds a reply to a task's open block, or feedback to its open review round.

```text
tsk reply <task> <text> [--send] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `--send` | Also send this reply, and only it, to the task's running agent. The status stays |

- Each run adds a reply, so read the task before retrying.
- Put `--` before text that starts with `-`.
- A task with no open block or round refuses with `not-blocked`. Empty text refuses with `empty-reply`, text over 4 KB with `text-too-long`.
- Output is `replied T12 as <author> <title>`.
- With `--send`, a second line says `sent to @claude` or `not sent: <reason>`. The reply is stored either way and the exit is 0. A reply that was not sent goes out later, when you unblock the task or send the review back.
- `--send` only sends your own replies, inside Herdr, to the agent tsk dispatched for the task. To answer and unblock in one step, use the board's [reply box](/docs/board/#block-review-reply).

```sh
tsk reply T12 "Use postgres" --send
```

## dispatch

Launches an assigned task's agent in its own Git worktree and Herdr workspace, then sets the task `started`. [Dispatch](/docs/dispatch/#dispatch) explains names, prompts, and what the agent gets.

```text
tsk dispatch <task> [--again] [--base <branch>] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `--base <branch>` | Start from this existing branch, for this launch only. The task's base stays |
| `--again` | Relaunch a dispatched task. Keeps the recorded base and recreates a cleaned worktree |

- Dispatch needs Herdr, a task in a project that is a Git repository, and an assignee with a profile in [`config.toml`](/docs/storage/#configuration).
- The base is `--base`, else the task's base, else the repository's default branch. Never the branch you run the command in. [Base branch](/docs/dispatch/#base-branch).
- A dispatched task refuses with `already-dispatched`. Use `--again` to relaunch on purpose.
- Output is `dispatched T12 to @claude in <worktree>`.
- A refusal changes nothing. Exit 3 after a launch means the agent may already run: read the task before retrying.
- On Windows dispatch is a preview ([Windows](/docs/dispatch/#windows-preview)).

```sh
tsk dispatch T12 --base main
```

## clean

Removes a dispatched task's worktree, and inside Herdr its workspace, without changing its status. To finish and clean in one step, use `tsk status T12 done --clean`.

```text
tsk clean <task> [--merge] [--json] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `--merge` | First merge the task's pull request, then set done and clean, as [`tsk status T12 done --merge`](#done) does |
| `--json` | Print the result as one JSON object |

- Uncommitted changes refuse with `dirty-worktree` and nothing is touched.
- The branch is deleted only when it is merged into the recorded base, or, with `gh`, when its pull request merged at the branch tip. Otherwise it stays and the output says why.
- A worktree already gone counts as cleaned.
- The task keeps its dispatch record, marked `cleaned`.
- What counts as merged, and what stays: [complete and clean up](/docs/dispatch/#complete-and-clean-up).

Output is one line: `cleaned T12: worktree <path> (removed), branch <name> (kept, <reason>), workspace <id> (removed)`. With `--json`:

| Field | Value |
| --- | --- |
| `number`, `title` | The task |
| `worktree` | `path`, and `outcome`: `removed` or `missing` |
| `branch` | `name`, `outcome` (`removed` or `kept`), and `reason` for a kept branch |
| `workspace` | `id`, and `outcome`: `removed`, `kept`, or `missing` |
| `warning` | A failed fetch, or a workspace Herdr would not close; `null` otherwise |
| `merge` | With `--merge`: `pr`, `url`, `method`, and `remote_branch` (`kept`, `deleted`, or `null`), plus `"status": "done"` |

When a step after a merge fails, `--merge --json` prints `number`, `merge`, `status` (`done`, or `null` when done was not saved), and `error` with `stage` (`done` or `cleanup`), `code`, and `message`.

```sh
tsk clean T12 --json
```

## archive

Hides a task from the working views. It keeps its status and number, and `tsk list --archived` still shows it.

```text
tsk archive <task> [--state-dir <dir>]
```

Output is `archived T12 <title>`. Repeating it is safe.

```sh
tsk archive T12
```

## unarchive

Puts an archived task back in the working views, in its status.

```text
tsk unarchive <task> [--state-dir <dir>]
```

Output is `unarchived T12 <title>`. Repeating it is safe.

```sh
tsk unarchive T12
```

## project

Archives or unarchives a whole project, by basename or path. Its tasks keep their statuses, and tasks archived on their own stay archived.

```text
tsk project archive <name> [--state-dir <dir>]
tsk project unarchive <name> [--state-dir <dir>]
```

Output is `archived project <name>` or `unarchived project <name>`. Repeating it is safe. A name with no tasks refuses: `no project named <name> has tasks`.

```sh
tsk project archive atlas
```

## trash

Restores a deleted task with its number.

```text
tsk trash restore <task> [--state-dir <dir>]
```

A deleted task first stays on the board, where undo brings it back. It moves to trash later ([deleted tasks](/docs/storage/#deleted-tasks)). A task that is not in trash refuses: `T12 is not in trash`. `tsk list --deleted` shows both kinds.

Output is `restored T12 <title>`.

```sh
tsk trash restore T12
```

## backup

Backs the task store and `config.toml` up to git repositories. Targets, frequency, and what is pushed: [git backups](/docs/storage/#git-backups).

```text
tsk backup setup [--repo <name>] [--state-dir <dir>]
tsk backup [now] [--state-dir <dir>]
tsk backup add <git-url> [--every <when>] [--state-dir <dir>]
tsk backup list [--state-dir <dir>]
tsk backup remove <target> [--state-dir <dir>]
```

| Action | Does |
| --- | --- |
| `setup [--repo <name>]` | Creates a private GitHub repository with `gh` (default `<you>/tsk-backup`), or reuses an empty one or one that holds tsk backups, then adds it. `--repo` takes `name`, `owner/name`, or `host/owner/name`. Refuses a public repository or one with other content |
| `now`, or nothing | Backs up to every target now. Exits 1 if any target failed |
| `add <git-url> [--every save\|daily]` | Adds a target to `config.toml` and tests it with a first backup. A failed first backup keeps the target and says what to fix. `--every` defaults to `save` |
| `list` | Shows each target with its frequency, last backup, and last error |
| `remove <target>` | Removes a target by its number in `list`, URL, or path. Backups already pushed stay |

```sh
tsk backup add git@gitlab.com:me/tsk-backup.git --every daily
```

## restore

Lists the backed-up versions of the store, or puts one back. [Restore](/docs/storage/#restore).

```text
tsk restore [<version>] [--all] [--state-dir <dir>]
```

| Flag | Meaning |
| --- | --- |
| `<version>` | Restore this version without asking |
| `--all` | List every version, not the newest 20 per machine |

- The list shows each version's id, time, host, task count, and target, newest first. In a terminal it then asks which to restore; Enter cancels.
- A restore keeps the replaced store beside it and prints its path. It never changes `config.toml`.
- It refuses, replacing nothing, when there are no targets, the id is unknown, a board or quick capture is open, or the version is from a newer tsk.
- `tsk restore T12` is not a version: it points at `tsk trash restore T12`.

```sh
tsk restore 3f9a1c2e
```

## setup

Registers tsk with Herdr, or installs the agent skill.

```text
tsk setup [--json]
tsk setup herdr [--check]
tsk setup agents [--yes [--force]] [--json]
tsk setup <agent> [--force] [--json]
tsk setup --skill-dir <path> [--force] [--json]
tsk setup --detected-ids | --skill-states
```

| Action | Does |
| --- | --- |
| none | In a terminal, offers the skill to every detected agent. Otherwise prints the targets |
| `herdr` | Registers the plugin and its shortcuts with Herdr 0.9 or later. [Herdr setup details](/docs/install/#herdr-setup-details) |
| `herdr --check` | Prints `bound` when both plugin commands are in the Herdr config, otherwise `unbound`. Changes nothing |
| `agents` | Installs or updates the skill for every detected agent. `--yes` skips the question |
| `<agent>` | Installs the skill for one agent, below |
| `--skill-dir <path>` | Installs the skill as `<path>/tsk-cli/SKILL.md` |

| Agent | Skills directory |
| --- | --- |
| `claude` | `~/.claude/skills/` |
| `pi` | `~/.pi/agent/skills/` |
| `omp` | The active OMP profile's skills directory, normally `~/.omp/agent/skills/` |
| `cursor` | `~/.cursor/skills/` |
| `grok` | `~/.grok/skills/` |
| `codex` | `~/.agents/skills/` |
| `opencode` | `~/.config/opencode/skills/` |

- An installed skill with the same version refuses with `skill-exists`; `--force` rewrites it. A different version updates without `--force`.
- Setup refuses a symlinked skills root, skill folder, or skill file.
- `--json` prints a machine-readable report. `--detected-ids` and `--skill-states` print plain lines for installers.
- Exit 0 on success, 1 on a refusal or failure, 2 for invalid arguments.

```sh
tsk setup claude
```

## update

Installs the latest stable release over a copy the installer put there, then refreshes a registered Herdr plugin and outdated agent skills. A Homebrew copy prints `brew update && brew upgrade tsk` instead. Reopen a running board afterwards. [Upgrade](/docs/install/#upgrade).

```sh
tsk update
```

## guide

Prints the [agent skill](/docs/agents/), the workflow `tsk setup` installs, without its frontmatter.

```sh
tsk guide
```

## Exit contract

| Exit | Meaning | Next step |
| --- | --- | --- |
| `0` | Done, or it was already so | Continue |
| `1` | Refused. In a plan, other items may have saved | Fix the cause; retry only what refused |
| `2` | Invalid arguments or input, or an invalid assignee profile in `config.toml`. Nothing saved | Fix the command or `config.toml` |
| `3` | Store error. The write may have saved | Read the task back before retrying |

A refusal prints `tsk <command>: <code>: <message>` on stderr, such as `tsk status: unknown-task: T99 is not on the board`. Branch on the code; the message may change.

| Command | Refusal codes |
| --- | --- |
| `add` | `empty-title`, `invalid-title`, `unknown-project`, `project-archived`, `unknown-agent`, `unknown-base`; in a plan also `invalid-item`, `invalid-thread`, `invalid-after` |
| `status` | `unknown-task`, `soft-deleted-task`, `text-too-long`, `invalid-blocker` |
| `status … started` | `after-not-done`, `agent-gone`, and the `dispatch` codes `needs-git-project`, `unknown-agent`, `agent-config`, `unknown-base`, `no-default-base`, `herdr-failed`, `unsupported-platform`, `unsafe-state-dir` |
| `status … done --clean` | The `clean` codes except `not-dispatched` and `already-cleaned` |
| `status … done --merge` | Every `clean` code, with `--merge` |
| `edit` | `unknown-task`, `soft-deleted-task`, `empty-title`, `invalid-title`, `unknown-agent`, `unknown-base`, `invalid-after`, `after-loop` |
| `steps` | `unknown-task`, `soft-deleted-task`, `empty-step-text`, `invalid-step-text`, `unknown-step`, `ambiguous-step`, `same-step` |
| `reply` | `unknown-task`, `soft-deleted-task`, `not-blocked`, `empty-reply`, `text-too-long` |
| `dispatch` | `unknown-task`, `soft-deleted-task`, `done-task`, `archived-task`, `no-assignee`, `unknown-agent`, `agent-config`, `already-dispatched`, `not-in-herdr`, `needs-git-project`, `unknown-base`, `no-default-base`, `herdr-failed`; on Windows `unsupported-platform`, `unsafe-state-dir` |
| `clean` | `unknown-task`, `not-dispatched`, `already-cleaned`, `dirty-worktree`, `worktree-mismatch`, `herdr-failed`; on Windows `files-in-use`, `path-too-long`, `removal-timed-out`, `partly-removed`; with `--merge` also `gh-unavailable`, `no-pr`, `pr-not-open`, `pr-base-mismatch`, `merge-failed`, `merge-config` |
| `archive`, `unarchive` | `unknown-task`, `soft-deleted-task` |

`project`, `trash`, `backup`, and `restore` refuse with a message only: `tsk <command>: <message>`, exit 1. `dispatch` and `clean` print `store-error` as the code of an exit 3.

After an uncertain `add`, check `tsk list --all --json`, and `--done` or `--archived` when a match could be there. With a task number, read the task directly.
