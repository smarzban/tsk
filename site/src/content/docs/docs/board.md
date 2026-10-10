---
title: Board
description: Find tasks, switch projects, and keep work moving.
---

Click to navigate, or use the keyboard. The footer shows actions for the cursor or your marked task set; those actions are clickable.

## Navigate

| View | Key | Contents |
| --- | --- | --- |
| **desk** | `1` | Blocked/review and started tasks across all live projects; ready and open tasks from your desk |
| **selected project** | `2` | Tasks in the selected project |
| **projects** | `3` | Project overview |

Launching inside a Git repository opens that project. Outside Git, tsk opens **desk** and puts the current directory in the middle project tab.

In Herdr, `prefix+t` opens or focuses a board in the current workspace. If one is already open in another tab there, Herdr switches to that tab and focuses its pane, preserving its view and edits. Otherwise, it opens a board beside your work. Boards in other workspaces stay untouched; all boards share the same tasks.

The middle tab remembers your selected project. Press `2` to open it; if none is selected, `2` opens the project picker.

## Quit or go back

Press `ctrl+q` to quit from the board or a task view, including Help, pickers, and the wide project preview. While a text editor, board search, quick-add line, or palette query owns input, `ctrl+q` keeps its editing behavior instead. Unsaved drafts must be saved or cancelled before quitting, including drafts parked in another view while a project preview has focus. Save recovery must be resolved first; quit attempts leave its failure message intact.

`Esc` leaves multi-select and clears its marked tasks before it closes the current layer; an open filter, view, assignee, or base picker closes first and keeps the marks. At the full-board root, with multi-select inactive and no page, peek, popup, search, or header selection left to dismiss, it quits without confirmation. This applies on every tab, not just the desk. In either wide split, `Esc` closes the right column once any editor or overlay is dismissed, returning to the full-width board or projects index. Task drafts stay parked; a project preview with unsaved work refuses to close. After a narrow resize hides the right column, `Esc` treats the visible board/index as the root, with the same unsaved-draft protection. In quick capture, `Esc` closes the popup.

## Projects

Press `p` to choose a project, or open **projects** for an overview.

- Click a project to select it; double-click or press `Enter` to open it.
- Check the footer for the selected project's full path.

Projects Overview counts work in **NEEDS YOU**, **IN MOTION**, **ON DECK** (ready and open, including the inbox), and **DONE**. Archived tasks and archived projects are excluded. A dim `·` means zero. `here` marks the launch project. At 100 columns or wider, the overview also lists threads.

At **110 usable columns** or wider, Overview can preview the cursored project beside the index:

| Stage | What you see |
| --- | --- |
| Full board | Full-width projects index |
| Split | Index and a dim project preview; the index keeps focus |
| Rail | Narrow index and a live project board; the right column owns input |

The right preview names the selected project in its top row, in the space used by navigation tabs on the index. Clicking or moving the index selection opens Split automatically; press `→` or `l` to move from Split to Rail, and `←` or `h` to walk back. The project tab never opens a full-screen task stage. `Enter` on an index row still opens that project in tab 2, and choosing a thread with `v` drops back to the full-width index. The right column has its own selection, drawer, filters, task page, quick-add, and status actions; `Esc` from its board returns focus to the index.

## Search

Press `/` on any board tab to search the rows that tab currently shows. On the Projects overview it matches project names or paths. On the desk, a project board, or a cross-project thread or assignee view it matches tasks by title, notes, step text, thread, assignee (with or without its `@`), or task number such as `T12`, case-insensitively. Every whitespace-separated word must match somewhere in the same task.

Typing or pasting filters immediately. Empty sections disappear and section counts show only matches. Search combines with a project board's thread and assignee filter; the done drawer is searched only while it is open. In the wide Projects preview, `/` searches the right project board when that seat has focus.

Press `Enter` to pin the query and return to board keys. Navigation, task actions, the done drawer, and a second `Enter` then act on the filtered rows; the query remains in the footer. Press `Esc` while typing to clear and close search, or press it once on a pinned board to clear the query before normal `Esc` behavior resumes. Changing tab, project, or Projects view also clears it.

## Threads and assignees filter

A thread groups related tasks within a project, such as `release` or `login-fix`. An [assignee](#assign) names the agent a task is for. Both narrow a board through one picker with two tabs, `threads · @assignees`.

| Where | Action |
| --- | --- |
| Project board | Press `t` or click the filter to open **Filter**; choose a thread, an assignee, or both |
| Projects overview | Press `v` or click **Overview** to view one thread or one assignee across projects |

On a project board the threads tab lists `all`, the project's threads, then **Without a thread**; the `@assignees` tab lists `all`, every profile in `config.toml` as `@name`, any name still on tasks whose profile was removed, then `unassigned`. The two choices combine: `#release` plus `@claude` shows only release tasks assigned to claude, and the filter control reads `#release @claude`. `all` clears only its own tab's choice, and `✓` marks the active choice in each tab. The choice is for this session only, clears when you switch projects, and search (`/`) narrows inside it.

On the Projects overview the threads tab offers **Overview** and each thread; the `@assignees` tab offers one `@name` view per assignee (there is no `unassigned` view). An `@name` view lists that assignee's tasks from every project and the desk, grouped by status like a thread view, with done tasks in the drawer. One view applies at a time.

The picker opens on the threads tab (an `@name` overview view reopens on its own tab). `Tab` or a click switches tabs and clears the typed filter. Type to filter the current tab, use arrows to select, `Enter` to choose, and `Esc` to close without a change. `j` and `k` are search text in these selectors.

Assign threads when [capturing](/docs/capture/#title-tokens) or [editing a task](/docs/task-page/#scope-thread-and-assignee).

## Agents

Assign a task to an agent, dispatch it into its own Git worktree and Herdr workspace, then clean up when you complete it. Agents are profiles you define in [`config.toml`](/docs/storage/#agent-profiles). Dispatch needs Herdr; on Windows it is a [preview](#dispatch-on-windows-preview). Assigning works everywhere.

### Assign

The assignee names one profile in `config.toml`. Press `@` on the board, peek, or task page, or choose **set assignee** in the palette, to open the assignee picker: every profile, then **none**. The current assignee is preselected, or the first profile when there is none. Type to filter, `↑`/`↓` to move, `Enter` to apply, and `Esc` to close without a change. With tasks marked, one choice assigns the whole set and one `ctrl+u` undoes it. With no profile defined, `@` says so and opens nothing. On the Projects overview `@` does nothing.

You can also assign with `!a name` in [quick-add](/docs/capture/#title-tokens), the task page's [Assignee field](/docs/task-page/#scope-thread-and-assignee) or footer, or `tsk add --assignee` and `tsk edit --assignee`. Rows stay a title; the peek footer reads `@assignee · ⎇ <base> · #thread · project`, leaving out what is unset and a default base.

### Dispatch

Starting an assigned task dispatches it: `ctrl+s`, the palette's **set status: started**, or [`tsk status N started`](/docs/cli/#status). tsk creates a branch and Git worktree from the task's [base](#base-branch), opens a Herdr workspace there, and runs the profile's command. It then records the dispatch on the task and sets it to started, in one save. If the launch fails, the task records no dispatch, keeps its status, and the status row says why. `ctrl+u` right after undoes the start only: the status goes back and the status row says `start undone · @claude kept running`; the agent and its worktree stay.

What a start does depends on the task:

| Task | Start |
| --- | --- |
| Unassigned | Sets started; to work on a task yourself, leave it unassigned |
| Assigned, never dispatched | Dispatches it, which sets started |
| Dispatched, agent still running | Sets started; Herdr is asked whether the agent is still in its pane, and when it cannot say, the start is plain |
| Dispatched, agent gone | Asks first: [relaunch](#relaunch) |
| Assigned, outside Herdr or on the desk | Sets started; the status row says why nothing launched |
| Done or archived | Sets started; never launches |

Names come from the task number and title. For T12 `Fix login timeout`:

| Name | Value |
| --- | --- |
| Branch | `tsk/t12-fix-login-timeout` |
| Worktree directory | `tsk-t12-fix-login-timeout` |
| Herdr workspace | `T12 Fix login timeout` |
| Herdr agent | `t12-claude`, for assignee `claude` |

The slug lowercases the title, joins its words (letters and digits in any script) with `-`, and keeps whole words within 30 characters; a longer title ends the workspace label with `…`. The worktree directory keeps only ASCII letters and digits, so `Café` checks out in `tsk-t12-caf`. When a branch, worktree, or directory of that name already exists, tsk appends `-2`, `-3`, and so on. tsk names the agent once Herdr detects it, so `herdr agent get t12-claude` finds it; if Herdr does not detect one within 30 seconds, the agent stays unnamed. Neither the board nor `tsk dispatch` waits for this.

A started task with a live dispatch shows `◉` instead of `●`.

To hand a task to an agent, assign it with `@`, then press `ctrl+s`.

Dispatch needs Herdr, a task in a project that is a Git repository, and an assignee with a profile. Where it cannot work at all, outside Herdr or on a desk task, starting an assigned task is a plain start and the status row says why: `started · no launch: not in Herdr` or `started · no launch: desk task has no repository`. Other launch refusals (an unknown agent, a base that cannot be resolved, a Git or Herdr failure) leave the task unstarted.

### Dispatch on Windows (preview)

Herdr on Windows is a preview, and so is dispatch there.

- A profile's `command` must find an `.exe` (`claude`) or a PowerShell script (`pi`, `codex`). Batch files (`.cmd`, `.bat`) are refused, because cmd.exe could run task text as commands; the pane says so and nothing starts.
- A PowerShell script is supported as a shim that hands its arguments to a program, like the `.ps1` files npm installs. tsk escapes each argument for that program's command line, so it arrives exactly as written. A script that reads `$args` itself sees the escaping instead: `"` arrives as `\"` and an empty argument as `""`.
- The [state directory](/docs/storage/) must not contain `$`, `` ` ``, `%`, `"`, or `!`. Dispatch refuses one that does with `unsafe-state-dir`.
- If Git's `core.longpaths` is off, dispatch says so once; turn it on with `git config --global core.longpaths true`.

Cleanup on Windows can stop with one of these, and keeps the task's dispatch until a later cleanup succeeds:

| Message | What to do |
| --- | --- |
| files in use | Close what is running in the worktree, then clean again. |
| path too long | Run `git config --global core.longpaths true`, then clean again. |
| removal timed out | Clean again. |
| partly removed | Follow the message, which says what is left and how to finish. Every commit is on the task's branch. |
| uncommitted changes | Commit or discard the changes in the worktree, then clean again. |

### Base branch

Dispatch starts from the task's base branch when one is set, otherwise from the repository's default branch (`origin/HEAD`), never from whatever your board or CLI has checked out. A local branch that tracks a remote starts from the remote branch. tsk fetches the remote first unless it was fetched in the last minute (the [fetch window](/docs/storage/#fetch-window)); offline, it starts from the local copy and says so. The task page then shows where the dispatch started, as `from <ref> @ <short sha>`.

Set a base with **set base** in the palette, the task page's `⎇` footer or **Base** field, `!b branch` in quick-add, or `tsk add --base` and `tsk edit --base`. The branch picker lists the repository default first, for example **default (main)**, then local and `origin/*` branches. It opens at once on the branches already on disk and shows **refreshing…** while a background fetch updates the list in place, keeping your selection; if the fetch fails it reads **offline, showing cached branches**. **default** clears the base. With tasks marked, one choice sets the whole set as one undo. A base must be an existing branch in the task's repository, not a tag or commit. There is no base key, and starting never asks for one.

### Start a marked set

With tasks marked, `ctrl+s` (or **set status: started** in the palette) starts the set as usual when no task in it would launch; assigned tasks that cannot launch here are named on the status row, such as `started · no launch: not in Herdr (T3, T4)`. When any would, it opens one card first. The card lists each task it will launch with its assignee and base (`from dispatch`, or `from default (main)`), then under **start only** the tasks it only starts (unassigned, already dispatched: relaunching stays one task at a time, an assigned desk task with `started · no launch: desk task has no repository`, and with **set status: started** done or archived tasks, listed as a status correction), then under **not started** each assigned task it cannot launch and why: unknown agent, or not a project in a git repo. Those stay unstarted. Rows show `checking…` while repositories are checked in the background. When nothing can launch, the card is titled `Start N tasks?` and `y` only starts.

`y` starts the start-only tasks in one save, then launches every listed task in the background, each exactly as a single dispatch. The board stays usable while the status row counts `dispatching 2/3…`. Each task is saved as its launch lands, a failed launch does not stop the others, and the status row ends with what launched and what failed. A status you change while launches land is kept (`T12 kept done (changed meanwhile)`). If a save fails and you cancel [save recovery](#save-failures), the status row names the workspace each unrecorded agent is running in. `Esc` or `[x]` closes the card and keeps the marks; `y` clears them. Further dispatches and quitting wait until the launches finish. One `ctrl+u` undoes the start-only tasks; launches from a marked set are not undoable. `y` rechecks the start-only tasks first: one finished or archived after the card opened is left alone, and one assigned meanwhile is not started (`not started: T14 assigned since the card opened`), since the card never showed its launch.

### Relaunch

Starting a dispatched task whose agent is gone (its Herdr workspace was closed, its pane has no agent, or its worktree was cleaned up) opens a card: `T12 · relaunch @claude?`.

| Key | Result |
| --- | --- |
| `y` | Relaunch the agent in the recorded worktree and start the task |
| `n` | Just start the task |
| `Esc` | Change nothing |

In the palette, **dispatch again** relaunches the cursor task without asking. Both act on the cursor task only. A relaunch keeps the recorded base and names, even if the task's base changed since. If the Herdr workspace was closed, it opens a new one on the kept worktree. After a cleanup it recreates the worktree, reopening the kept branch or recreating a deleted one from its original starting commit.

### Complete and clean up

With no marks, `ctrl+d` on a task with a live dispatch asks before completing. The card, `Done T12 · clean up?`, opens at once with `Checking merge into origin/main…` while a background fetch checks the branch against its recorded base. It then reads `Merged into origin/main ✓`, `Not merged into origin/main (squash-merged? delete it by hand)`, or, when the remote cannot be reached, `Merge into origin/main not confirmed (offline), so the branch stays.`, followed by what `y` does to the branch (`delete branch` or `keep branch`), the worktree (`remove worktree`), and the agent pane (`close`).

| Key | Result |
| --- | --- |
| `y` | Mark done now and clean up in the background |
| `n` | Mark done and keep everything |
| `Esc` or `[x]` | Change nothing |

The choices and `[x]` are clickable. `y` before the check finishes still marks the task done at once, and the cleanup starts when the check lands. After `y` the board stays usable and the card reports progress: `removing worktree…`, then `✓ cleaned · branch deleted`, `✓ cleaned · branch kept` with the reason, or `kept: <reason>`. `Esc` hides the card while cleanup continues; the status row shows `cleaning 1 of 1…`, then the outcome, such as `done T12 · cleaned` or `done T12 · cleaned · branch kept (not merged)`. A clean outcome clears after a few seconds; one that kept something stays until your next action. Closing the project preview that started a cleanup does not stop it. While a cleanup runs, `ctrl+d` refuses with `cleanup still running; try again when it finishes`, and quitting shows `finishing cleanup…` and waits for it, up to 30 seconds.

A worktree with uncommitted changes cannot be cleaned: the card reads `Done T12 · can't clean up` and offers only `n` or cancel. A worktree that is already gone is marked cleaned without a card. Done tasks, archived tasks, and archived project views complete without one.

With tasks marked, `ctrl+d` on a set that holds live dispatches opens one card for the set, titled for example `Done 4 tasks · clean up 2 of 3?`. Each dispatch gets a row with its merged status (`checking…`, then `merged ✓`, `not merged into <ref>`, `not confirmed (offline)`, or `uncommitted changes`) and what `y` will do (`delete branch · remove worktree · close pane`, `keep branch · …`, or `keep everything`). Tasks without a dispatch are listed as `+ T<n> has no dispatch, just marked done`. `y` marks the whole set done and cleans every clean worktree in the background, updating each row as it lands (`Done 5 tasks · cleaning 2 of 4`); one failure never stops the others. The outcome counts, naming only tasks that kept something: `done 5 · cleaned 4 · kept T14 (uncommitted changes)`. `n` marks the set done and keeps everything; `Esc` before `y` keeps the marks. A long set scrolls with `↑`/`↓`, page keys, or the wheel. A marked set with no live dispatch completes at once.

Either way, completion is one save and one undo step, saved before any worktree is touched. Undo restores the status, not a removed worktree.

Cleanup closes the dispatch's Herdr workspace, removes its recorded worktree, and deletes its branch only when the branch is merged into the recorded base. It fetches the base first, so a merge on GitHub counts without a local pull. It keeps the branch, and says why, when the branch is not merged (squash merges do not count), when the merge cannot be confirmed because the fetch failed, when the base no longer exists, or when the branch is checked out elsewhere or changed during cleanup. It never touches a worktree with uncommitted changes, the project root, or a path that does not match the recorded worktree, and it leaves alone a task relaunched after the card opened (`kept: changed since the card opened`). The task keeps its dispatch record, marked cleaned. From a script, use [`tsk clean`](/docs/cli/#clean).

## Status

| Section | Status |
| --- | --- |
| **NEEDS YOU** | `blocked` on you, `review` on you |
| **IN MOTION** | `started`, `blocked` on another task or on something else, and `review` on an agent or on something else |
| **ON DECK** | `ready`, `open` |
| ↳ **inbox** | `open` |
| Done drawer | `done` |

Status glyphs are `◌` open, `○` ready, `●` started, `■` blocked, `▲` review, and `✓` done. A started task with a live [dispatch](#dispatch) shows `◉` instead of `●`; any other status, or a cleaned dispatch, shows the normal glyph. A task blocked on another task or on something else shows `□`; a review on an agent or on something else shows `△`.

A row with something to say carries a dim **live line** under it, in the peek's `└─` style, wrapped and never cut. It is part of its row: selection never stops on it and a click on it acts on the task. The right edge of a row stays empty.

| Row | Live line |
| --- | --- |
| Blocked on you | `@claude blocked on you · 2h`, or `blocked on you · 2h` when you set it yourself |
| Review on you | `@claude needs your review · 40m` / `needs your review · 40m` |
| Blocking task done | `T169 is done · unblock it · 1h` |
| Blocking task deleted | `T169 was deleted · unblock it · 1h` |
| Blocked elsewhere | `waiting on T169` / `waiting on design team` |
| Review handed elsewhere | `@pi reviewing` |
| Ready or open, running [after](#after) unfinished tasks | `after T202 (started), T205 (open)` |

The age is how long the row has been in that state: since the block or review round opened, or since the blocking task finished. A started task, a done or archived task, a [notice](#notices), and a waiting task whose prerequisites are all done have no live line. When the blocking task is done, the blocked task returns to NEEDS YOU; tsk never changes the status itself.

The [peek](#mouse) replaces the live line, and closing it brings the line back. A blocked task's peek opens with the live line's text, then the why, then `Decide: <option> · <option>` when there are options or else the needs, then the footer. A review's peek opens with the live line's text, then what was done, then every check on one wrapped line (`○` open, `✓` passed, `✗` failed), then the footer. Neither shows the notes; `Enter` opens the task page for them. A notice, or a blocked or review task with no recorded block or round, peeks its notes as usual.

### Block with a reason

`ctrl+b` on a task that is not blocked opens the block card:

| Field | Value |
| --- | --- |
| why | What stops the work (optional) |
| on | `‹ you · task · other ›`, cycled with `←` / `→`; `task` takes a number such as `T12`, `other` any text |
| needs | What would unblock it (optional) |

`Tab` moves between fields, `Enter` blocks, and `Esc` cancels. `Enter` on an empty card blocks at once with no reason. A task number must be another task on the board; the card says so otherwise. With tasks marked, one card blocks the whole set as one save and one `ctrl+u` undo step, and `Esc` keeps the marks. `ctrl+b` on a blocked task unblocks it to ready, as do every other status change: leaving `blocked` closes its block. Palette **set status: blocked** opens the same card for the targets not yet blocked. If the save fails and you cancel, the card and the marks stay, ready to try again.

Agents block with a question from [the CLI](/docs/cli/#status) (`--why`, `--needs`, `--option`, `--on`). Answer on the [task page](/docs/task-page/#blocked), or press `r` on a blocked row, in NEEDS YOU or IN MOTION at any width, to reply right under it and its live line: the block's why and needs stay above the reply box, and its keys are the task page's (`Shift+Enter` saves, `ctrl+s` saves and unblocks, `Enter` breaks the line, `Esc` cancels). With tasks marked, `r` answers the cursor row only. On a row that is not blocked the status row says `not blocked`. The list scrolls to keep the caret in view. If the task leaves the list while you type (unblocked or finished elsewhere), the box moves below the list under the task's name, keeps your draft, and says `this block was closed or replaced elsewhere; reply kept`.

### Review with what was done

`ctrl+r` on a task that is not in review opens the review card:

| Field | Value |
| --- | --- |
| done | What was done (optional) |
| check | One thing to verify per line; `Shift+Enter` starts the next check |
| next | What comes after (optional) |
| on | `‹ you · agent · other ›`, cycled with `←` / `→`; `agent` takes a profile from [`config.toml`](/docs/storage/#agent-profiles), `other` any text |

`Tab` moves between fields, `Enter` puts the work up for review, and `Esc` cancels. `Enter` on an empty card sets review at once. With tasks marked, one card covers the whole set as one save and one `ctrl+u` undo step; tasks already in review keep their round. `ctrl+r` on a task in review returns it to ready. Palette **set status: review** opens the same card over the targets not yet in review.

Each review is a **round**. Agents set one from [the CLI](/docs/cli/#status) (`--done`, `--check`, `--next`, `--on`); running it again while the task is in review updates the same round. On the [task page](/docs/task-page/#review) you mark each check passed or failed and give feedback with `r`, which also opens the feedback box under a review row on the board. In the feedback box:

| Key | Action |
| --- | --- |
| `Shift+Enter` | Save the feedback; the task stays in review |
| `ctrl+s` | Send back: save the feedback and start the task. A running dispatched agent gets this feedback and the failed checks as one message: `[tsk T12 sent back] <feedback> Failed checks: <check>; <check>` |
| `ctrl+d` | Approve: save the feedback and mark the task done, through the [cleanup card](#complete-and-clean-up) when its dispatch is live. Nothing is sent |
| `Esc` | Cancel |

Sending back takes the same route as a reply's `ctrl+s` ([below](#reply-to-a-running-agent)), with `feedback` in place of `reply` on the status row, except that an unassigned task also goes to started. An empty box sends back only when a check failed (`type feedback or fail a check first` otherwise). The round closes into history at the send-back, recorded as sent back; the agent's next review opens round N+1. A round closed by done is recorded as approved. A relaunched or newly dispatched agent finds the feedback and failed checks in the last `past_reviews` entry of `tsk list --json`.

### Reply to a running agent

When `ctrl+s` in a reply box starts an assigned task whose dispatched agent is still running, tsk sends that agent the reply you just saved, once the start is saved, as one message: `[tsk T12 unblocked] <your reply>`. Earlier replies are never sent again. With an empty box, or `ctrl+s` on the task page with no box open, it only unblocks and sends `[tsk T12 unblocked]` alone. Multi-line replies arrive intact.

tsk sends only to the agent it dispatched: the pane's live agent must carry the name dispatch gave it (`t12-claude`), checked when the start is routed and again just before the send. Another agent in that pane, or one Herdr never named (naming can fail, for example when the name is taken), gets nothing. The status row says how it went:

| Status row | Meaning |
| --- | --- |
| `started · reply sent to @claude` | Herdr took the message; a busy agent gets it after its current turn |
| `started · @claude is waiting on a prompt; reply kept on the task` | The agent is on a permission prompt or question, so nothing was sent |
| `started · could not reach @claude; reply kept on the task` | Herdr failed or could not say whether the agent runs |
| `started · reply not sent: @claude is not in its pane; reply kept on the task` | The pane holds another agent, or an unnamed one |
| `started · reply not sent: not in Herdr` | The board runs outside Herdr, so it cannot reach the agent |

The task stays started in every case and nothing relaunches or retries; the reply is on the task either way. `Shift+Enter` (save only) never sends. The other starts are unchanged: a task never dispatched dispatches, and its prompt points the agent at the answers; a gone agent asks before [relaunching](#relaunch); an unassigned task unblocks to ready. If the save fails, nothing is sent until [save recovery](#save-failures) retries it.

On your desk, **ON DECK** contains only desk tasks. On a project board, it contains that project's ready and open tasks. Ready tasks are the picked queue; open tasks are the untriaged inbox below it. Ready tasks sort by oldest pick first, open tasks by oldest capture first, and notice tasks lead within each group. The **inbox** group starts expanded; press `Enter` on its heading or `g` while the done drawer is closed to fold or unfold it. With the drawer open and archived tasks available, `g` addresses its archived group; otherwise it addresses the inbox. Use the thread and assignee filter to narrow the tasks.

Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE, and the drawer's ARCHIVED group keep the most recent status change on top, while ON DECK lists ready and inbox backlogs oldest first. (`N` tasks lead each group until you clear them.) Editing a task or ticking a step never moves it; setting a status moves it to the top of its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching tabs, projects, threads, or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface, except in a filter, view, assignee, or base picker, where it closes the picker first.

On a task-board list, `ctrl+s`, `ctrl+n`, `ctrl+o`, `ctrl+d`, `ctrl+b`, `ctrl+r`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and actions from the task page always use only the cursor; `ctrl+s` on a marked set with assigned tasks opens the [start card](#start-a-marked-set).

| Key | Action |
| --- | --- |
| `ctrl+s` | Start an open or ready task; an assigned one [dispatches](#dispatch); a task that still waits [asks first](#after) |
| `ctrl+n` | Set ready, the picked on-deck queue |
| `ctrl+o` | Set open, the inbox |
| `ctrl+d` | Mark done; for a live dispatch, offer [cleanup](#complete-and-clean-up) |
| `ctrl+b` | Block with a reason (the [block card](#block-with-a-reason)); on a blocked task, return it to ready |
| `ctrl+r` | Review with what was done (the [review card](#review-with-what-was-done)); on a task in review, return it to ready |
| `r` | Reply to a blocked row, or give feedback on a review row, inline ([reply](#block-with-a-reason)); cursor only |

`ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Bulk block and review toggles are all-or-nothing: if every target already has that status they all return to ready, otherwise they all move to that status (blocking through one block card and review through one review card; tasks already blocked keep their block, tasks in review their round). Other status verbs are absolute, so repeating the current status does nothing. Done tasks can be sent directly to ready or open.

Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops; the one automatic start is a ready task whose last [prerequisite](#after) is done.

## After

A task can run **after** other tasks: it waits while any of them is not done. Task numbers are board-wide, so a prerequisite can be in any project or on the desk. A waiting ready or open row's live line reads `after T202 (started)`, gone once every prerequisite is done; nothing moves sections. Its peek lists each prerequisite with its status, `after T202 · started`, and a prerequisite's peek lists what waits on it, `before T203, T204`. The [task page](/docs/task-page/#after) footer shows the same.

Set it from the task page's **After** field, with **set after…** in the palette (the cursor task, or every marked task), with `!w T202` in [quick-add](/docs/capture/#title-tokens), or with [`tsk edit --after`](/docs/cli/#edit). The task picker lists the tasks that are not done, the task's own project first and then the others with their project. Type to filter by number or title, `Space` ticks several, `Enter` applies the ticked tasks (or the highlighted one when nothing is ticked), and **none** clears the list. A task cannot run after itself, and a choice that would close a loop is refused, for example `T202 already runs after T205`.

With tasks marked in order, **chain in order** makes each marked task run after the one marked before it, on top of what it already runs after. One save, one `ctrl+u`.

When the last prerequisite becomes **done** (review does not count), each waiting task in **ready** starts in the same save as the done, the way `ctrl+s` starts it; an assigned task never dispatched then [dispatches](#dispatch) right after that save, which records its agent. What a done releases is judged against the store as saved, so a prerequisite completed elsewhere (another board, the CLI, an agent) counts. While a marked-set dispatch is still landing, a released assigned task stays ready instead (`T203 waits: a dispatch is running; ctrl+s starts it`). The status row says `T203 started · T202 done`, and the started task's paper trail reads `started · after T202`. This happens from the board, the [cleanup card](#complete-and-clean-up), and [`tsk status N done`](/docs/cli/#status). Waiting tasks in open, blocked, or review do not move. A launch that fails never blocks the done: the task goes back to ready and the status row says why (`T203 back to ready: …`). If the launch ran but its record could not be saved, the status row says the agent is running and names its worktree; if the launch failed and the task could not be saved back to ready, it says `T203 is started but no agent launched; couldn't save it back to ready: …`. A task whose earlier agent is gone starts without a relaunch (`@claude gone, not relaunched`). `ctrl+u` on the done reverts the statuses only, `done undone · T203 back to ready · @claude kept running`; a launched agent keeps running.

Starting a waiting task yourself asks first: `T203 runs after T202 (started). Start anyway?` `y` starts it, `Esc` cancels. With tasks marked, the card lists each waiting one.

Deleting a task drops it from every other task's after list in the same save, and one `ctrl+u` puts the links back. A ready task left waiting on nothing does not start; the status row says `T203 no longer waits (T202 deleted)`. A task purged from trash leaves no links behind.

## Notices

Your first board open seeds four desk tasks with `N` ids (not `T`). They teach the board by being ordinary tasks: open, peek, change status, archive, or delete. `ctrl+d`, `ctrl+f`, and `ctrl+x` all dismiss a notice the same way; that starter task never re-seeds. After an upgrade, one `What's new in tsk` task can appear the same way. Details and the delivery record live under [storage](/docs/storage/#starter-guides-and-release-notes). Starter notices keep their seeded statuses so the tour order stays useful; a new release notice lands in NEEDS YOU as `review`, so it sits where you look first.

## Mouse

| Action | Result |
| --- | --- |
| Click a tab or selector | Change view or open its choices |
| Click a task outside multi-select | Peek in narrow panes; open or update details beside the board in wide panes |
| Click a task or its `T`/`N` number in multi-select | Move the cursor there and toggle its mark |
| Click the same task again in a narrow pane | Close its peek |
| Double-click a task | Open it full screen |
| Click its `T` or `N` number | Copy that id |
| Click a footer action | Run that action |
| Wheel or drag a scrollbar | Scroll |
| Drag across text | Select and copy on release |

Open peeks show notes, followed by one metadata footer ordered `@assignee · ⎇ <base> · #thread · project`, omitting unset parts and hiding the default base. A task with [after](#after) links lists them above the notes. A blocked or review task's peek shows its [live line and block or review](#status) instead of the notes. Below 110 columns, `→` or `l` opens a peek and `←` or `h` closes it. Peeks show up to five wrapped note lines; the [task page](/docs/task-page/) shows the rest.

## Wide stage slider

At **110 usable columns** or wider, use `→` / `l` and `←` / `h` to move through a four-stage slider:

| View | What you see |
| --- | --- |
| Board | Full-width board |
| Split | Board beside task details; selecting another task updates the details |
| Task | Task details beside a narrow board rail |
| Full screen | Task details only |

Press `Enter` from the board to open a task full screen. `Esc` returns to the view you left. From task focus, `Esc` returns focus to the board beside it. On Projects Overview, the slider stops at Rail: it shows a live project board beside the index and never opens a full-screen task; `Enter` in that right column opens the task page inside the column.

Click a task on the full-width board to open its details alongside it, keeping board focus. Click inside the task column to focus it. Click a task in the rail to bring back the split board. While editing, arrows move the text cursor instead. On the projects tab, click the right preview to focus its live board, or click an index row from Rail to rebind it and return to Split.

Narrowing the pane shows one surface; widening it restores the selected view. Each new session starts with the board alone.

| Pane size | Layout |
| --- | --- |
| 110 columns or wider | Board and task views |
| At least 78×24, below 110 columns | Single board or task page |
| Smaller | Compact layout; usable down to 40×10 |

## Completed tasks

Press `d` to open or close the done drawer. Select a done task and press `ctrl+n` to return it to ready, or `ctrl+o` to send it to the inbox.

The drawer's **archived** group starts closed. Click its heading or press `Enter` on it to expand. `g` folds or unfolds the group while the drawer is open.

## Archive

Archiving hides work without changing its status.

| Archive | Restore |
| --- | --- |
| Task: select it and press `ctrl+f` | Open the done drawer's archived group, select it, then press `ctrl+u` or `ctrl+f` |
| Project: press `p`, select it, then `ctrl+f` | Switch to the picker's **archived** tab, select it, then `ctrl+u` or `ctrl+f` |

Use `Tab`, `←`, or `→` to switch project-picker tabs. Archiving has no undo entry.

### View an archived project

Press `Enter` on a project in the picker's **archived** tab. Its tasks open read-only. Press `ctrl+u` to restore the project, or `Esc`, `p`, or `1`–`3` to leave.

Archived projects are excluded from task-destination pickers. Restoring a project leaves individually archived tasks archived.

### Launch inside an archived project

tsk asks whether to restore it. Choose `y` to restore, or `n`/`Esc` to keep it archived. Keeping it archived sends new captures to your desk for that session.

## Delete and undo

Press `ctrl+x` twice to delete the marked tasks, or the cursored task when nothing is marked. The confirmation and recovery messages show the task count for a marked set. `ctrl+Delete` is an alternative.

One `ctrl+u` undoes the whole marked deletion, completion, or block, including the [after](#after) links a deletion dropped and the starts a completion made. If another writer has changed any task in that batch since the action, undo refuses without changing any of them and remains available to retry. On an archived cursor with no marks, `ctrl+u` restores that task instead.

Deleted tasks remain available through [trash commands](/docs/cli/#trash) for a limited time. They do not appear on the board.

## Palette

Press `:` and type to find an action. Use arrows or `Tab` to select, `Enter` to run, and `Esc` to close.

| Available actions | When |
| --- | --- |
| New task, undo, done drawer, help, quit | Always |
| Set open/ready/started/blocked/review, edit notes, change scope, delete | A task is selected |
| Set assignee, set base, set after… | A task is selected |
| Dispatch again | A task with a dispatch record is selected |
| Chain in order | Two or more tasks are marked |
| Retry save, cancel save | A save has failed |

**Set assignee** opens the same picker as `@`; **set base** opens the branch picker; **set after…** opens the [task picker](#after). Each applies to the marked set when marks are present, with one save and one undo. **Chain in order** makes each marked task run after the one marked before it. **Set status: started** takes the same route as `ctrl+s`, including on blocked and review tasks: an assigned task dispatches, and with marks the [start card](#start-a-marked-set) opens. **Dispatch again** ignores and clears marks, then relaunches only the cursor.

Search matches letters in order: `ssr` finds `set status: review`.

## Help

Press `?` on the board, task page, or another non-text surface to open the searchable shortcut reference. A dim divider separates its focused search field from shortcuts, grouped one binding per row by function. Long descriptions wrap beneath the description column. The card uses at most half the terminal height, except below 15 rows where it may grow to six rows so one result stays visible. Type or paste to filter by a key, action, group, or related term; use arrows, page keys, or the wheel to scroll. `Esc` clears a nonempty search first, then closes Help.

When a text field already owns input, `?` remains text instead of opening Help.

## Save failures

If saving fails, the draft stays open.

- `r` or `Enter`: retry.
- `c` or `Esc`: cancel the pending change.

Resolve the failure before making another change. See [storage](/docs/storage/) for directory and backup information.
