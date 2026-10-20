---
title: Dispatch
description: Hand a task to an agent in its own worktree and Herdr workspace, then merge and clean up.
---

Dispatch hands a task to a coding agent. The agent gets its own Git branch, worktree, and Herdr workspace. When you complete the task, tsk tidies them up.

Dispatch needs [Herdr](/docs/install/#add-to-herdr), a task in a project that is a Git repository, and an agent assignee. Assigning works everywhere. On Windows, dispatch is a [preview](#windows-preview).

## Assign

An assignee says who does the task: an agent profile from [`config.toml`](/docs/storage/#configuration), you, or another person. Assigning only labels the task: it decides what a later start does.

| Where | How |
| --- | --- |
| Board, peek, or task page | `@` opens the assignee picker: every profile, `@you`, the people already assigned, **type a name…**, then **none** |
| Palette | **set assignee** |
| Quick-add | `!a name` ([capture](/docs/capture/)) |
| Task page | The **Assignee** field, or the footer's `@name` |
| CLI | `tsk add --assignee`, `tsk edit --assignee` ([CLI](/docs/cli/#edit)) |

With tasks marked, one choice assigns the whole set, and one `ctrl+u` undoes it. Rows never show the assignee. The peek and task page footers show it as `@claude`.

### Assignee names

Every place you type an assignee reads the name the same way:

| You type | The assignee |
| --- | --- |
| `you`, `me`, or your [configured name](/docs/storage/#board-defaults) | You, shown as `@you` |
| An agent profile's name, with or without `@` | That agent |
| Any other name | A person, such as `@dana` |

- In the picker, type the name and choose its row. A typed profile name picks the listed profile.
- The CLI says when a name is not a profile, so a typo shows: `assigned T12 to @dana (not an agent profile)`.
- A name follows the [thread name](/docs/capture/#thread-names) rules and is stored in lowercase.
- Starting a task assigned to you or a person only sets it started. It never dispatches and never asks to relaunch.
- A task keeps the name of a profile you later remove. It stays an agent, so its start refuses until the profile is back.

## Dispatch

Start an assigned task to dispatch it: `ctrl+s`, palette **set status: started**, or [`tsk status T12 started`](/docs/cli/#status). A dispatch:

1. creates a branch and worktree from the task's [base branch](#base-branch),
2. opens a Herdr workspace in the worktree,
3. runs the assignee's profile command there,
4. records the dispatch on the task and sets it to started, in one save.

If the launch fails, the task stays unstarted with no dispatch, and the status row says why. tsk removes the worktree, workspace, and new branch that launch created. A worktree with changes stays, but its workspace still closes. The message names anything left. A started task with a live dispatch shows `◉` instead of `●`.

The agent's instructions come from the profile's prompt. Without one, a default prompt asks it to open a pull request into the base and set the task to review. See [Configuration](/docs/storage/#configuration).

`ctrl+u` right after a dispatch undoes the start only. The agent keeps running in its worktree, and the status row says so:

```text
start undone · T12 back to ready · @claude kept running
```

### What a start does

| Task | Start |
| --- | --- |
| Unassigned | Sets started |
| Assigned to you or a person | Sets started; never launches |
| Assigned to an agent, never dispatched | Dispatches |
| Dispatched, agent still running | Sets started |
| Dispatched, agent gone | Asks first: [relaunch](#relaunch) |
| Assigned, outside Herdr or on the desk | Sets started; the status row says why nothing launched |
| Done or archived | Sets started; never launches |

When Herdr cannot say whether the agent still runs, the start is plain and launches nothing. Outside Herdr the status row reads `started · no launch: not in Herdr`. Other refusals leave the task unstarted: an unknown agent, a base that cannot be resolved, or a Git or Herdr failure.

### Names

Names come from the task number and title. For T12 `Fix login timeout`, assigned to `claude`:

| Name | Value |
| --- | --- |
| Branch | `tsk/t12-fix-login-timeout` |
| Worktree directory | `tsk-t12-fix-login-timeout` |
| Herdr workspace | `T12 Fix login timeout` |
| Herdr agent | `t12-claude` |

- The title part keeps whole words within 30 characters.
- The worktree directory keeps only ASCII letters and digits.
- When a name is taken, tsk appends `-2`, `-3`, and so on.
- tsk names the agent once Herdr detects it, so `herdr agent get t12-claude` finds it. If Herdr detects no agent within 30 seconds, it stays unnamed.

The task page shows the worktree, the branch, and where the dispatch started, such as `from origin/main @ 3333b79`.

## Base branch

A dispatch starts from the task's base branch, or from the repository's default branch (`origin/HEAD`) when none is set. It never starts from whatever your board or terminal has checked out.

- A local branch that tracks a remote starts from the remote branch.
- tsk fetches the remote first, unless it fetched it in the last 60 seconds.
- Offline, it starts from the local copy and says so.

Set a base with palette **set base**, the task page's `⎇` footer or **Base** field, or `!b branch` in [quick-add](/docs/capture/). From the CLI, use `tsk add --base` or `tsk edit --base`. With tasks marked, one choice sets the whole set as one undo step.

A base must be an existing branch, not a tag or commit. Starting never asks for one.

The branch picker lists the repository default first, such as **default (main)**, then local and `origin/*` branches. It opens on the branches already on disk and refreshes in place after a background fetch. **default** clears the base.

## Start a marked set

`ctrl+s` on a marked set starts it as usual when no task in it would launch. When any would, one card opens first, titled with the number it launches, such as `Dispatch 2 tasks?`:

| Group | Tasks |
| --- | --- |
| Top | Each task it will launch, with its assignee and base |
| **start only** | Tasks it only starts: unassigned, already dispatched, or assigned desk tasks |
| **not started** | Assigned tasks it cannot launch, with the reason: an unknown agent, or no Git repository |

A launch row reads like `@claude  from default (main)`, with `checking…` while its repository is checked. A line under the rows says what `y` does, for example `▸ y  dispatch T12 · start T205 without launching · T202 not started`. When the checks find nothing to launch, the card is titled `Start 3 tasks?` and `y` only starts.

`y` starts the start-only tasks in one save, then launches each listed task in the background, exactly as a single dispatch:

- The board stays usable, and the status row counts `dispatching 2/3…`.
- Each task saves as its launch lands. One failed launch does not stop the others.
- A status you change meanwhile is kept.
- Quitting and further dispatches are refused until every launch lands.

`Esc` closes the card and keeps the marks. One `ctrl+u` undoes the start-only tasks; launches from a marked set cannot be undone. Relaunching stays one task at a time.

## Relaunch

Starting a dispatched task whose agent is gone opens a card. The agent is gone when its workspace was closed, its pane has no agent, or its worktree was cleaned up.

```text
@claude is no longer running.

▸ y  relaunch @claude in the same worktree · start T12
  n  start T12 · don't relaunch @claude

worktree  ~/.herdr/worktrees/app/tsk-t12-fix-login-timeout

                                        esc cancel
```

`y` relaunches and starts, `n` only starts, and `Esc` changes nothing. Palette **dispatch again** relaunches the cursor task without asking.

A relaunch keeps the recorded base and names, even if the task's base changed since. It opens a new workspace on the kept worktree. After a cleanup, it recreates the worktree from the kept branch, or from the original starting commit when the branch was deleted.

## Decision cards

The relaunch, start-anyway, and marked-set start cards, the [cleanup card](#complete-and-clean-up), and the [archived-project prompt](/docs/board/#done-drawer-and-archive) work the same way:

- Each option is one line: its key, then what it does. The likely or safe option is bold.
- `↑`/`↓` move the `▸` cursor and `Enter` runs it. The cursor starts on the bold option.
- An `Enter` in the first moment after a card opens is ignored, so a double tap cannot run anything.
- Letter keys run their option directly.
- Click an option to select it, double-click to run it. Click the dim way out, such as `esc cancel`, to cancel.

Every card key is listed under [Keys](/docs/keys/#cards).

## Complete and clean up

`ctrl+d` on a task with a live dispatch opens the cleanup card, titled `Done T12`. It says where the branch stands, then offers what to do with the pull request, the branch, and the local checkout. With an open pull request:

```text
PR #42 is still open:

▸ m  merge PR #42 to main · mark done · remove worktree, branch and agent pane
  y  don't merge · mark done · remove worktree and agent pane · keep the branch and the PR open
  n  don't merge · mark done · keep everything

worktree  ~/.herdr/worktrees/app/tsk-t12-fix-login-timeout
branch    tsk/t12-fix-login-timeout

                                                     esc cancel
```

On the card, **agent pane** means the dispatch's Herdr workspace.

| Where the branch stands | Options |
| --- | --- |
| Open pull request | `m` merges it ([below](#merge-the-pull-request)); `y` keeps the branch and pull request; `n` keeps everything |
| Merged, such as `Merged into main ✓` | `y` removes the worktree, branch, and workspace; `n` keeps everything |
| Not merged, or the merge cannot be confirmed | `y` removes the worktree and workspace and keeps the branch; `n` keeps everything |
| Uncommitted files in the worktree | Only `n`: commit or discard them first to tidy up |

Every option marks the task done, and options that do not apply are left out. Without the GitHub CLI (`gh`) logged in, the card never mentions a pull request.

### After you choose

The task is marked done at once. Cleanup runs in the background while the board stays usable. The card shows progress, then the outcome, such as `Done T12 · cleaned · branch kept (not merged)`. `Esc` hides the card, and the status row carries on (`cleaning 1 of 1…`).

- `ctrl+d` refuses while a cleanup runs.
- Quitting waits for a running cleanup, up to 30 seconds.
- A worktree that is already gone is marked cleaned without a card, and its branch is kept. Inside Herdr, its workspace closes.
- Done tasks, archived tasks, and archived project views complete without a card.

Completion is one save and one undo step, saved before any worktree is touched. `ctrl+u` reverts the status. It cannot un-merge a pull request or restore a removed worktree.

### What cleanup removes

Cleanup closes the dispatch's Herdr workspace and removes its worktree. It deletes the branch only when the branch is merged into the recorded base:

- It fetches the base first, so a merge on GitHub counts without a local pull.
- A squash or rebase merge leaves no Git ancestry, so with `gh` logged in, cleanup asks GitHub. It deletes the branch only when the pull request merged into the recorded base and its head is exactly the local branch tip.
- Otherwise it keeps the branch and says why: not merged, fetch failed, base gone, or branch checked out elsewhere.

Cleanup never touches uncommitted changes, the project root, or any path other than the recorded worktree. tsk never deletes remote branches. The task keeps its dispatch record, marked cleaned. From a script, use [`tsk clean`](/docs/cli/#clean).

### A marked set

`ctrl+d` on a marked set holding live dispatches opens one card, titled for example `Done 3 tasks`. Each task gets one row saying where it stands:

```text
T12   PR #42 is still open
T202  not merged · no pull request
T205  2 uncommitted files · app.py, README.md
```

Each option line names the tasks it touches. What each key does to each task:

| Key | Open pull request | Merged | Not merged | Uncommitted |
| --- | --- | --- | --- | --- |
| `m` | Merge, done, full tidy | Done, full tidy | Done, tidy | Unchanged |
| `y` | Done, tidy | Done, full tidy | Done, tidy | Unchanged |
| `n` | Done | Done | Done | Done |

A full tidy removes the worktree, branch, and workspace; a tidy keeps the branch. Tasks without a dispatch are marked done under every key. One failure never stops the others, and the status row ends with a count that names the tasks that kept something. `Esc` keeps the marks.

## Merge the pull request

With `gh` installed and logged in, the cleanup card looks up each task's pull request by its branch. When a task with a clean worktree has an open pull request into its recorded base, the card offers `m`. The option is worded after the merge method: `merge`, `squash-merge`, or `rebase-merge PR #42 to main`.

The method comes from [`[merge]` in `config.toml`](/docs/storage/#configuration). The default is a merge commit.

`m` merges with `gh` in the background:

- It merges only when the pull request's head is exactly the local branch tip.
- Branch protection, required checks, and reviews apply as on GitHub.
- tsk then asks GitHub again and goes on only when the pull request shows merged. A merge queue that only accepted it changes nothing: `PR #42 is queued, not merged yet · nothing changed`.
- On success the task is marked done and cleaned as with `y`. The outcome says whether GitHub deleted the remote branch.
- When the merge fails, nothing changes, and the card shows gh's reason.

Quitting waits for a running merge. On a marked set, `m` merges each open pull request in turn, and one failure never stops the others. From a script, use [`tsk status T12 done --merge`](/docs/cli/#status).

## Windows (preview)

Herdr on Windows is a preview, and so is dispatch there.

- A profile's `command` must find an `.exe` or a PowerShell script. A batch file (`.cmd`, `.bat`) is refused, because cmd.exe could run task text as commands. Dispatch refuses it with `agent-config` before it creates anything. When tsk cannot see where the command leads, the workspace opens and its pane refuses instead.
- A PowerShell script works as a shim that hands its arguments to a program, like the `.ps1` files npm installs. A script that reads `$args` itself sees the arguments escaped.
- The [state directory](/docs/storage/#location) must not contain `$`, `` ` ``, `%`, `"`, or `!`. Dispatch refuses one that does.
- If Git's `core.longpaths` is off, each dispatch says so. Turn it on with `git config --global core.longpaths true`.

Cleanup on Windows can stop with one of these. The task keeps its dispatch until a later cleanup succeeds.

| Message | What to do |
| --- | --- |
| files in use | Close what runs in the worktree, then clean again |
| path too long | Turn on `core.longpaths`, then clean again |
| removal timed out | Clean again |
| partly removed | Follow the message. Every commit is on the task's branch |
| uncommitted changes | Commit or discard the changes, then clean again |
