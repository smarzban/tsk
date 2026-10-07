---
title: CLI
description: Commands, arguments, output, and retry rules for the shared task board.
---

The CLI reads and updates the same tasks as the board. A running board picks up saved changes automatically.

```sh
tsk add -t "Fix login timeout"
tsk list
```

## For agents

Install the skill with `tsk setup pi`, or use your [agent's setup target](#setup). `tsk guide` prints the workflow.

1. Read the task with `tsk list T12`.
2. Set its status with `tsk status T12 started` (or `ready` to pick it from the inbox).
3. Update notes or steps as work progresses.
4. Set `review`, `blocked`, `open`, or `done` explicitly.

The CLI can mark a task done with `tsk status <task> done`. Agent lifecycle does not change task status automatically.

Use `--json` on `add`, `list`, or `clean` for machine-readable output. Read the [exit contract](#exit-contract) before retrying a write.

## Commands

| Command | Action |
| --- | --- |
| `tsk` | Open the board |
| `tsk capture` | Open capture; save or discard exits |
| `tsk add` | Add a task or JSON plan |
| `tsk list` | Read tasks |
| `tsk status` | Set task status |
| `tsk edit` | Replace title or notes; assign or unassign |
| `tsk dispatch` | Launch an assigned task's agent in its own worktree |
| `tsk clean` | Remove a dispatched worktree and its merged branch |
| `tsk steps` | Add, toggle, rename, or remove steps |
| `tsk archive` / `tsk unarchive` | Hide or restore a task |
| `tsk project archive` / `tsk project unarchive` | Hide or restore a project |
| `tsk trash restore` | Restore a task from trash |
| `tsk setup` | Configure Herdr or install an agent skill |
| `tsk update` | Upgrade an installer-managed copy, or print Homebrew guidance |
| `tsk guide` | Print the agent workflow |
| `tsk help [<command>]` | Show the CLI reference or one command's reference |
| `tsk --help` | Show the CLI reference |
| `tsk --version` / `tsk -V` | Print the installed version |

`tsk --help` is the syntax reference. `tsk help` prints the same reference, and `tsk help <command>` is identical to `tsk <command> --help`. Help always wraps at 80 columns, including redirected output. Internal Herdr launcher flags are intentionally absent.

Data commands accept `--state-dir <dir>`. Setup, update, and guide do not use that flag.

### Statuses

| Status | Meaning |
| --- | --- |
| `open` | Captured, not yet picked (inbox) |
| `ready` | Picked, up next (on deck) |
| `started` | In motion |
| `blocked` | Waiting on something |
| `review` | Done by the agent, waiting on you |
| `done` | Closed |

### Task addresses

`T12`, `t12`, `12`, and a task UUID identify the same task. Direct lookup ignores the current project.

Board notice tasks paint as `N1`… (starter tasks and `What's new in tsk`). Those ids are board-only: the CLI does not accept `N1`, and default `tsk list` omits them. Agents ignore them.

### Scope

Without a scope flag, `add` and filtered `list` use the launch repository inside Git, or desk outside Git. Commands addressed to a task ignore that default. The outside-Git current directory remains available through `-p /full/path`. A launch repository reached through a symlink (macOS reports `/tmp/repo` as `/private/tmp/repo`) uses the spelling already on the board, so it stays one project.

| Flag | Scope |
| --- | --- |
| `--desk` | Desk |
| `-p name` / `--project name` | Existing project uniquely matching that basename, ignoring case |
| `-p /path` or `-p C:\path` | Existing absolute directory, creating a project there if needed |
| `--all` on list | All scopes |

For adds, a bare project name must match exactly one project the board already knows: a project with tasks, an archived project, or the repository you launched from. Missing and ambiguous names refuse with `unknown-project`, for example `tsk add: unknown-project: project atlss is not on the board`. A new project destination requires an existing absolute directory (`/…` or `~/…` on macOS/Linux, `C:\…` or `~\…` on Windows); relative and nonexistent paths refuse. Project-filtered reads remain permissive, so `tsk list -p typo` returns an empty list.

## add

```sh
tsk add -t "Fix login timeout" -n "Reproduce on a slow connection" --thread auth --assignee reviewer --base main
# new tasks start open; use `tsk status T<n> ready` when picked
tsk add -t "Buy coffee" --desk
tsk add -t "Draft release notes" -p atlas --json
```

| Option | Purpose |
| --- | --- |
| `-t`, `--title` | Required task title |
| `-n`, `--notes` | Optional notes |
| `-p`, `--project` or `--desk` | Destination |
| `--thread` | [Thread name](/docs/capture/#thread-names) |
| `--assignee` | Exact configured agent profile name |
| `--unassign` | Explicitly create without an assignee |
| `--base <branch>` | Explicit dispatch base, an existing local or remote branch in the task's project repository |
| `--clear-base` | Explicitly create without a base; conflicts with `--base` |
| `--json` | One result object |
| `--file <path>` or `--file -` | Read a JSON plan |
| `--state-dir <dir>` | Alternate state directory |

Duplicate detection compares the trimmed title, resolved project, normalized thread, assignee, and explicit base. A matching non-deleted task succeeds without changing the task (`outcome: existing` in JSON), even if done or individually archived. Adding into an archived project refuses.

Plain output: `added <title>` or `task already exists`.

JSON output: `outcome` (`created` or `existing`), `id`, `number`, `title`, `project` (`null` for desk), and `assignee` (`null` when unassigned).

Values starting with `-` use equals syntax: `--title="-fix parser"`, `--notes="-5 degrees"`, `--project="-maintenance"`, `--file=...`, or `--state-dir=...`.

Blank notes are omitted. C0 control characters in titles are rejected before trimming.

### JSON plans

```json
[
  {"title": "Reproduce login timeout", "project": "atlas", "thread": "auth", "assignee": "reviewer", "base": "main"},
  {"title": "Draft release notes", "notes": "Include migration instructions"}
]
```

```sh
tsk add --file plan.json
cat plan.json | tsk add
```

| Field | Meaning |
| --- | --- |
| `title` | Required string |
| `notes` | Optional notes |
| `project` omitted | Invocation default |
| `project: null` | Desk |
| `project` string | Project name or path |
| `thread` omitted or `null` | No thread |
| `thread` string | Normalized thread |
| `assignee` omitted or `null` | Unassigned |
| `assignee` string | Exact configured agent profile name |
| `base` omitted or `null` | Dispatch from the repository's default branch |
| `base` string | Existing local or remote branch in the task's project repository |

Output contains `created`, `existing`, and `failed` arrays. Items carry their input index `i`; failures include `code` and `error`. Successful entries include task ID, number, and title. Notes and assignees are not echoed; read the task back when needed.

Valid items persist even if another item fails. Retry only failed or confirmed-missing items.

Do not mix item flags with `--file`. Piped input is ignored when item flags are present.

## list

```sh
tsk list
tsk list T12
tsk list --all --json
tsk list -p atlas --thread auth --assignee reviewer
tsk list --done --all
tsk list --open --all
tsk list --ready --all
tsk list --archived --all
tsk list --deleted --all
```

```text
tsk list [<task>] [-p <project> | --desk | --all] [--thread <name>] [--assignee <name>] [--open | --ready | --done | --deleted | --archived] [--json] [--state-dir <dir>]
```

| Filter | Result |
| --- | --- |
| Default | Open, ready, started, blocked, and review tasks; excludes archived, deleted, and notice (`N`) tasks |
| `--open` | Inbox tasks with status `open` |
| `--ready` | Picked on-deck tasks with status `ready` |
| `--done` | Completed tasks |
| `--archived` | Individually archived tasks and tasks in archived projects, across statuses |
| `--deleted` | Live soft-deleted tasks and `trash.jsonl` entries, deduped by task with the live copy winning, newest deletion first. Trash is retained for 30 days, see [storage](/docs/storage/#deleted-tasks). |
| `--thread` | Filter within the selected scope |
| `--assignee` | Filter by exact normalized assignee name |

Scope flags are mutually exclusive. So are `--open`, `--ready`, `--done`, `--deleted`, and `--archived`.

A direct task address searches the main store, including done, archived, and recently deleted tasks. It cannot be combined with scope, thread, assignee, or status filters. A missing task exits 2. A task already moved to `trash.jsonl` is not addressable; find it with `tsk list --deleted` (optionally `-p`), or restore it with `tsk trash restore`.

Human output groups by status in `STARTED`, `READY`, `OPEN`, `BLOCKED`, `REVIEW` order; filtered rows include the task number, assignee, base, and thread, and `--all` adds scope labels using a unique concise trailing path or desk. List output and errors wrap to the attached terminal width with hanging indentation. Task rows, scope labels, notes, steps, archived marks, and threads use the same wrapping behavior, supported from 50 columns. Redirected output keeps stored logical lines. Command help is reference text and instead always wraps at 80 columns.

Single-task output removes metadata from the title row and presents notes, steps, `@assignee`, `⎇ <base>` when explicitly set, then `#thread` as separate blocks. A blank line separates adjacent blocks that exist. Human step rows show state and text without machine-oriented short IDs.

JSON returns an array with `id`, `number`, `title`, `status`, `project`, `assignee`, `base`, and `thread`. Direct lookup returns the complete task, including `notes` (`null` when absent) and `steps` (an empty array when absent). Its fields are ordered `id`, `number`, `project`, `status`, `title`, `notes`, `steps`, `assignee`, `base`, `thread`, then `dispatch` when a record exists; each JSON step retains its `short_id` for step commands. `base` is the task's explicit base branch or `null`. `dispatch` is the launch record: `argv`, `worktree`, `branch`, `base` (the ref it started from), `base_ref` (the same ref, fully qualified), `base_commit` (the starting commit), `base_remote` (when the base has a remote), `herdr_workspace_id`, `at`, and `cleaned` once cleaned up. Filtered listings do not include `dispatch`. Archived listings include an `archived` mark: `archived` or `project archived`.

## status

```sh
tsk status T12 open
tsk status T12 ready
tsk status T12 started
tsk status T12 review
tsk status T12 done --clean
```

Accepts `open`, `ready`, `started` (or `start`), `blocked`, `review`, and `done`.

Unlike keyboard toggles, this command sets the requested status directly. Repeating the same value is safe. `--clean` is valid only with `done`: tsk saves done first, then runs [`tsk clean`](#clean) when the task has a live dispatch. With no dispatch, or one already cleaned, it exits 0 and prints `nothing to clean`. A real cleanup refusal (such as `dirty-worktree`) exits 1 and leaves the task done.

Output: `status T12 <status> <title>`. The output uses `started`, even when the input was `start`.

## edit

```sh
tsk edit T12 --title "Fix timeout on slow connections"
tsk edit T12 --notes "Reproduced with a delayed response"
tsk edit T12 --assignee reviewer
tsk edit T12 --unassign
tsk edit T12 --base dispatch
tsk edit T12 --clear-base
```

Requires `--title`, `--notes`, `--assignee`, `--unassign`, `--base`, `--clear-base`, or a combination. `--assignee` and `--unassign` conflict; so do `--base` and `--clear-base`. Scope and thread stay unchanged. `--base` must be an existing branch in the task's repository, not a tag or commit; an unknown branch refuses with `unknown-base`. `--clear-base` returns the task to the repository's default branch.

Blank notes clear the field. Notes preserve newlines and tabs. Use `--title=...` or `--notes=...` for values starting with `-`.

Output: `edited T12 <title>`. Repeating the same values is safe.

## dispatch

```sh
tsk dispatch T12
tsk dispatch T12 --again
tsk dispatch T12 --base dispatch
```

Launches an assigned task's agent in its own Git worktree and Herdr workspace, then sets the task to `started`. It needs Herdr on macOS or Linux (on Windows it refuses with `unsupported-platform`), a task in a project that is a Git repository, and an assignee with a profile in [`config.toml`](/docs/storage/#agent-profiles).

tsk creates a branch and worktree from the base, opens a Herdr workspace there, and runs the profile's rendered command in its root pane. Only after the launch succeeds does it save the dispatch record and set `started`, in one write. Dispatch is not undoable. Names follow the task number and title, as on the [board](/docs/board/#dispatch): branch `tsk/t12-fix-login-timeout`, worktree directory `tsk-t12-fix-login-timeout`, workspace `T12 Fix login timeout`, with `-2`, `-3` appended when a name is taken. tsk then names the agent `t12-<assignee>` once Herdr detects it, so `herdr agent get t12-claude` finds it; an undetected agent stays unnamed and the dispatch still succeeds.

The base is the one-off `--base <branch>` when given, then the task's `base`, otherwise the repository's default branch (`origin/HEAD`), never the branch checked out where you run the command. `--base` does not change the task. A base must be an existing local or remote branch in the task's repository, not a tag or commit; an unknown branch refuses with `unknown-base`, and a repository without `origin/HEAD` needs an explicit base. tsk fetches the base's remote first unless it was fetched in the last 60 seconds ([fetch window](/docs/storage/#fetch-window)), so a newly pushed branch works; this also applies when `tsk add` or `tsk edit` sets a remote branch as the base. A local branch that tracks a remote starts from the remote branch. Offline, dispatch starts from the local copy and says so. `{base}` in agent templates is the short branch name, such as `dispatch` for `origin/dispatch`.

A dispatched task refuses with `already-dispatched`. `--again` relaunches deliberately: it reruns the command in the recorded workspace and keeps the recorded base, ignoring `--base` and later changes to the task's base. After a cleanup it recreates the worktree, reopening the kept branch or recreating a deleted one from its original starting commit. Status changes never remove the record.

Output: `dispatched T12 to @implementer in /path/to/worktree`.

Refusal codes: `unknown-task`, `soft-deleted-task`, `no-assignee`, `unknown-agent`, `agent-config`, `not-in-herdr`, `unsupported-platform`, `needs-git-project`, `done-task`, `archived-task`, `already-dispatched`, `unknown-base`, and `herdr-failed`. A refusal leaves the task unchanged. A storage failure after a successful launch exits 3; read the task before retrying, because the agent may already be running.

## clean

```sh
tsk clean T12
tsk clean T12 --json
```

Removes a dispatched task's worktree after review, without changing its status. To complete and clean in one step, use `tsk status T12 done --clean`.

- A worktree with uncommitted changes refuses with `dirty-worktree` and nothing is touched.
- Otherwise tsk checks that the recorded path is a Git worktree of the project, not its root, and matches the recorded Herdr workspace (`worktree-mismatch` if not). It closes that workspace when running inside Herdr and removes the worktree.
- The branch is deleted only when it is merged into the recorded base. tsk fetches the base first (unless inside the [fetch window](/docs/storage/#fetch-window)), so a merge on GitHub counts without a local pull.
- The branch is kept, with a reason, when it is not merged (squash merges do not count: "not merged into origin/main; squash-merged? delete by hand"), when the fetch fails and the merge cannot be confirmed (`could not reach <remote> to confirm the merge`), when the base no longer exists, or when the branch is checked out elsewhere or changes during cleanup.
- A worktree that is already gone counts as cleaned.

The task keeps its dispatch record, marked `cleaned`. Human output says whether the worktree, branch, and workspace were removed or kept. `--json` returns the same outcomes, with `branch.reason` for a kept branch (`null` when deleted) and `warning` naming a failed fetch (`null` otherwise).

Refusal codes: `unknown-task`, `not-dispatched`, `already-cleaned`, `dirty-worktree`, `worktree-mismatch`, and `herdr-failed`; these exit 1. Store failures exit 3 with `store-error`. There is no force option.

## steps

```text
tsk steps <task> add <text> [--state-dir <dir>]
tsk steps <task> toggle <step-short-id> [--state-dir <dir>]
tsk steps <task> rename <step-short-id> <text> [--state-dir <dir>]
tsk steps <task> remove <step-short-id> [--state-dir <dir>]
```

Read short IDs with `tsk list T12 --json`. Full step UUIDs also work.

| Action | Output prefix | Safe to repeat unchanged? |
| --- | --- | --- |
| Add | `added <short-id> <text>` | No; creates another step |
| Toggle | `toggled <short-id> [x] <text>` | No; flips the value again |
| Rename | `renamed <short-id> <text>` | Yes |
| Remove | `removed <short-id> <text>` | No; refuses after removal |

Read back with `tsk list T12 --json` before retrying an uncertain result. [Using steps on the board](/docs/steps/).

## archive and unarchive

```sh
tsk archive T12
tsk unarchive T12
```

Keep the task's status and number. Archiving removes it from working views, and it remains available through `tsk list --archived` until `tsk unarchive` returns it. Repeating either command is safe.

Output: `archived T12 <title>` or `unarchived T12 <title>`.

Unknown tasks refuse with `T12 is not on the board`; deleted tasks with `T12 is deleted`.

## project archive / unarchive

```sh
tsk project archive atlas
tsk project unarchive atlas
```

Accepts a project basename or path. Archiving removes the whole project from working views, while unarchiving returns its tasks in their existing statuses. Repeating an action is safe. An unknown project refuses with `no project named <name> has tasks`.

Restoring a project preserves task statuses and leaves individually archived tasks archived.

Output: `archived project <name>` or `unarchived project <name>`.

## trash

```sh
tsk list --deleted --all
tsk trash restore T12
```

Restore returns a task from trash with its original number, no soft-delete flag, a restored event, and a new revision. A task absent from trash, or already live, refuses with `T12 is not in trash`.

Recent deletions may still be in the main store; use board undo until they move to trash. [Retention and storage](/docs/storage/#deleted-tasks).

## help and version

```sh
tsk --help
tsk help list
tsk list --help
tsk --version
```

Use top-level help to find a command, then use either one-command form for its flags, examples, refusals, and exit contract. `--version` and `-V` print `tsk <version>`. They are global flags, so place them before a command.

## update

```sh
tsk update
```

For an installer-managed copy, downloads and runs the official installer for the latest stable release, then refreshes an already registered Herdr plugin and any outdated installed agent skills ([details](/docs/install/#upgrade)). Windows stages the verified executable, returns after starting a detached helper, and replaces `tsk.exe` after the running process exits, so success means installed or staged. For a Homebrew copy, it prints `brew update && brew upgrade tsk`; Homebrew remains responsible for its own upgrades. Reopen a running board after an upgrade.

## setup

```sh
tsk setup
tsk setup herdr
tsk setup agents --yes
tsk setup pi
tsk setup --skill-dir /path/to/skills
tsk setup --detected-ids
tsk setup --skill-states
tsk setup herdr --check
```

On a TTY, bare `tsk setup` detects global agent skill roots and asks once to install or update the embedded skill for every detected agent. Without a TTY it prints guidance (and the list of named targets) and does not hang. `tsk setup --json` prints a detection report. Two plain probes exist for installers: `tsk setup --detected-ids` prints space-separated detected agent ids, and `tsk setup --skill-states` prints an `embedded <version>` line followed by one tab-separated `id state version path` line per detected agent (`missing`, `current`, `outdated`, `blocked-symlink`).

### Herdr

Requires Herdr 0.9+ on PATH; Herdr's supported `0.9.0-preview.*` Windows builds qualify. Registers the installed binary and adds **prefix+t** and **prefix+a**. A plugin command you already bound to another key is left alone; setup never adds the default chord beside it. Shortcut conflicts require confirmation; noninteractive conflicts stop before writes. `tsk setup herdr --check` prints `bound` when both plugin commands are already in the config (on any keys), otherwise `unbound`, and changes nothing. On macOS/Linux it uses `HERDR_CONFIG_PATH`, then `XDG_CONFIG_HOME/herdr/config.toml`, then `~/.config/herdr/config.toml`. On Windows it uses `HERDR_CONFIG_PATH`, then `%APPDATA%\herdr\config.toml`, with `%USERPROFILE%\AppData\Roaming\herdr\config.toml` as the fallback. Windows setup installs PowerShell launchers; Herdr's Windows plugin support is preview.

[Reload, upgrades, and removal](/docs/install/#herdr-setup-with-an-installed-binary).

### Agent skills

| Target | Skills directory |
| --- | --- |
| `pi` | `~/.pi/agent/skills/` |
| `omp` | Active OMP profile's user skills directory (normally `~/.omp/agent/skills/`) |
| `claude` | `~/.claude/skills/` |
| `cursor` | `~/.cursor/skills/` |
| `grok` | `~/.grok/skills/` |
| `codex` | `~/.agents/skills/` |
| `opencode` | `~/.config/opencode/skills/` |
| `--skill-dir <path>` | The supplied directory |

Setup writes `tsk-cli/SKILL.md` under the selected directory. OMP follows `OMP_PROFILE`, using legacy `PI_PROFILE` only when `OMP_PROFILE` is unset; an empty, whitespace, or `default` value explicitly selects the default profile. It also follows `PI_CONFIG_DIR` and the default profile's `PI_CODING_AGENT_DIR`; invalid profile names are refused. Skill `version:` is independent of the crate version. A matching installed version exits 1 with `skill-exists`; a missing or different version updates without `--force`. `--force` always overwrites. `tsk setup agents --yes` installs or updates every detected agent without asking; `tsk setup agents --yes --force` also rewrites copies whose version already matches. `--force` without `--yes` is a usage error for `agents`. Only global skill roots are offered (project-local roots are out of scope).

| Option | Action |
| --- | --- |
| `--yes` | With `agents`, install/update detected agents without asking |
| `--force` | Replace an existing skill even when versions match |
| `--json` | Machine-readable outcome |
| `--detected-ids` | Print detected agent ids (installer use) |
| `--skill-states` | Print each detected agent's skill state (installer use) |

Symlinks at the skills root, skill directory, or file are refused. Herdr setup does not accept these agent options.

Setup exits 0 for success/help/list/detection, 1 for setup failure or `skill-exists`, or 2 for invalid arguments.

## guide

```sh
tsk guide
```

Prints the embedded [agent skill](/docs/agents/) without YAML frontmatter. Exits 0. It is the same workflow installed by agent setup.

## Exit contract

For data commands:

| Exit | Meaning | Next step |
| --- | --- | --- |
| `0` | Success, including an already-existing task or unchanged value | Continue |
| `1` | Refusal; a plan may have saved other items | Correct refusals; retry only failed items |
| `2` | Invalid arguments, input, or assignee profile configuration; nothing saved | Fix the invocation or `config.toml` |
| `3` | Storage error; a write may have committed | Read back before retrying |

After an uncertain add, inspect `tsk list --all --json`. Also check `--done` and `--archived` when a duplicate could be hidden there. If you know the task number, use direct lookup.

| Command | Refusal codes |
| --- | --- |
| Add | `empty-title`, `invalid-title`, `invalid-thread`, `invalid-item`, `unknown-project`, `unknown-agent`, `unknown-base`, `project-archived` |
| Edit | `empty-title`, `invalid-title`, `unknown-task`, `soft-deleted-task`, `unknown-agent`, `unknown-base` |
| Dispatch | `unknown-task`, `soft-deleted-task`, `no-assignee`, `unknown-agent`, `agent-config`, `not-in-herdr`, `unsupported-platform`, `needs-git-project`, `done-task`, `archived-task`, `already-dispatched`, `unknown-base`, `herdr-failed` |
| Clean | `unknown-task`, `not-dispatched`, `already-cleaned`, `dirty-worktree`, `worktree-mismatch`, `herdr-failed` |
| Steps | `empty-step-text`, `invalid-step-text`, `unknown-task`, `soft-deleted-task`, `unknown-step`, `ambiguous-step` |
| Status | `unknown-task`, `soft-deleted-task`; with `--clean`, cleanup codes above except `not-dispatched` and `already-cleaned` |
| Archive / unarchive | `unknown-task`, `soft-deleted-task` |

A refusal prints as `tsk <command>: <code>: <message>` on stderr, for example `tsk status: unknown-task: T99 is not on the board`. Branch on the code; the message is for people and may change.

Invalid thread flags fail argument parsing with exit 2; an invalid thread in a JSON plan is an item refusal with exit 1. When add or edit supplies an assignee, an invalid `config.toml` also exits 2 without saving. Unknown agents and unknown or archived project refusals save nothing for that item; other valid plan items can still save.

Human-readable output escapes stored terminal control characters. JSON retains the underlying text.

## Herdr helper

`tsk --find-board-pane` reads Herdr `pane list` JSON from stdin and prints the ID of the pane labelled `tsk`. It exits 1 if none matches.
