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

Press `ctrl+g`, or choose **dispatch to @name** in the palette, to send the cursor task to its assignee. tsk creates a branch and Git worktree from the task's [base](#base-branch), opens a Herdr workspace there, and runs the profile's command. It then records the dispatch on the task and sets it to started, in one save. If the launch fails, the task records no dispatch and keeps its status. Dispatch is not undoable.

Names come from the task number and title. For T12 `Fix login timeout`:

| Name | Value |
| --- | --- |
| Branch | `tsk/t12-fix-login-timeout` |
| Worktree directory | `tsk-t12-fix-login-timeout` |
| Herdr workspace | `T12 Fix login timeout` |
| Herdr agent | `t12-claude`, for assignee `claude` |

The slug lowercases the title, joins its words (letters and digits in any script) with `-`, and keeps whole words within 30 characters; a longer title ends the workspace label with `…`. The worktree directory keeps only ASCII letters and digits, so `Café` checks out in `tsk-t12-caf`. When a branch, worktree, or directory of that name already exists, tsk appends `-2`, `-3`, and so on. tsk names the agent once Herdr detects it, so `herdr agent get t12-claude` finds it; if Herdr does not detect one within a few seconds, the agent stays unnamed.

A started task with a live dispatch shows `◉` instead of `●`.

On an unassigned task `ctrl+g` opens the assignee picker first: choosing a profile saves the assignment, then dispatches; **none** or `Esc` changes nothing. With no profile defined, `ctrl+g` refuses: "no agent assigned: press @ or add a profile to config.toml".

Dispatch needs a task in a project that is a Git repository, an assignee with a profile, and a status other than done; archived tasks are refused.

### Dispatch on Windows (preview)

Herdr on Windows is a preview, and so is dispatch there.

- A profile's `command` must find an `.exe` (`claude`) or a PowerShell script (`pi`, `codex`). Batch files (`.cmd`, `.bat`) are refused, because cmd.exe could run task text as commands; the pane says so and nothing starts.
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

Set a base with **set base** in the palette, the task page's `⎇` footer or **Base** field, `!b branch` in quick-add, or `tsk add --base` and `tsk edit --base`. The branch picker lists the repository default first, for example **default (main)**, then local and `origin/*` branches. It opens at once on the branches already on disk and shows **refreshing…** while a background fetch updates the list in place, keeping your selection; if the fetch fails it reads **offline, showing cached branches**. **default** clears the base. With tasks marked, one choice sets the whole set as one undo. A base must be an existing branch in the task's repository, not a tag or commit. There is no base key, and `ctrl+g` never asks for one.

### Dispatch a marked set

With tasks marked, `ctrl+g` (or **dispatch N marked** in the palette) opens one card. It lists each task it will launch with its assignee and base (`from dispatch`, or `from default (main)`), then each task it skips and why: unassigned, unknown agent, already dispatched, not a project in a git repo, done, or archived. Rows show `checking…` while repositories are checked in the background. Unassigned tasks are skipped, not prompted; assign the set with `@` first. If nothing can be dispatched, `ctrl+g` says why on the status row, such as "nothing to dispatch: unassigned", and opens no card.

`y` launches every listed task in the background, each exactly as a single dispatch. The board stays usable while the status row counts `dispatching 2/3…`. Each task is saved as its launch lands, a failed launch does not stop the others, and the status row ends with what launched and what failed. A status you change while launches land is kept (`T12 kept done (changed meanwhile)`). If a save fails and you cancel [save recovery](#save-failures), the status row names the workspace each unrecorded agent is running in. `Esc` or `[x]` closes the card and keeps the marks; `y` clears them. Further dispatches and quitting wait until the launches finish.

### Relaunch

On a dispatched task, the first `ctrl+g` names its worktree and asks for another press; the second relaunches the agent there. In the palette this is **dispatch again**, which acts on the cursor task only. A relaunch keeps the recorded base and names, even if the task's base changed since. After a cleanup it recreates the worktree, reopening the kept branch or recreating a deleted one from its original starting commit.

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
| **NEEDS YOU** | `blocked`, `review` |
| **IN MOTION** | `started` |
| **ON DECK** | `ready`, `open` |
| ↳ **inbox** | `open` |
| Done drawer | `done` |

Status glyphs are `◌` open, `○` ready, `●` started, `■` blocked, `▲` review, and `✓` done. A started task with a live [dispatch](#dispatch) shows `◉` instead of `●`; any other status, or a cleaned dispatch, shows the normal glyph.

On your desk, **ON DECK** contains only desk tasks. On a project board, it contains that project's ready and open tasks. Ready tasks are the picked queue; open tasks are the untriaged inbox below it. Ready tasks sort by oldest pick first, open tasks by oldest capture first, and notice tasks lead within each group. The **inbox** group starts expanded; press `Enter` on its heading or `g` while the done drawer is closed to fold or unfold it. With the drawer open and archived tasks available, `g` addresses its archived group; otherwise it addresses the inbox. Use the thread and assignee filter to narrow the tasks.

Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE, and the drawer's ARCHIVED group keep the most recent status change on top, while ON DECK lists ready and inbox backlogs oldest first. (`N` tasks lead each group until you clear them.) Editing a task or ticking a step never moves it; setting a status moves it to the top of its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching tabs, projects, threads, or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface, except in a filter, view, assignee, or base picker, where it closes the picker first.

On a task-board list, `ctrl+s`, `ctrl+n`, `ctrl+o`, `ctrl+d`, `ctrl+b`, `ctrl+r`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and actions from the task page always use only the cursor; `ctrl+g` on a marked set opens the [bulk dispatch card](#dispatch-a-marked-set).

| Key | Action |
| --- | --- |
| `ctrl+s` | Start an open or ready task |
| `ctrl+n` | Set ready, the picked on-deck queue |
| `ctrl+o` | Set open, the inbox |
| `ctrl+d` | Mark done; for a live dispatch, offer [cleanup](#complete-and-clean-up) |
| `ctrl+b` | Set blocked; press again to return to ready |
| `ctrl+r` | Set review; press again to return to ready |

`ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Bulk block and review toggles are all-or-nothing: if every target already has that status they all return to ready, otherwise they all move to that status. Other status verbs are absolute, so repeating the current status does nothing. Done tasks can be sent directly to ready or open.

Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops.

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

Open peeks show notes, followed by one metadata footer ordered `@assignee · ⎇ <base> · #thread · project`, omitting unset parts and hiding the default base. Below 110 columns, `→` or `l` opens a peek and `←` or `h` closes it. Peeks show up to five wrapped note lines; the [task page](/docs/task-page/) shows the rest.

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

One `ctrl+u` undoes the whole marked deletion or completion. If another writer has changed any task in that batch since the action, undo refuses without changing any of them and remains available to retry. On an archived cursor with no marks, `ctrl+u` restores that task instead.

Deleted tasks remain available through [trash commands](/docs/cli/#trash) for a limited time. They do not appear on the board.

## Palette

Press `:` and type to find an action. Use arrows or `Tab` to select, `Enter` to run, and `Esc` to close.

| Available actions | When |
| --- | --- |
| New task, undo, done drawer, help, quit | Always |
| Set open/ready/started/blocked/review, edit notes, change scope, delete | A task is selected |
| Set assignee, set base | A task is selected |
| Dispatch to @name | An assigned task without a dispatch record is selected |
| Dispatch again | A task with a dispatch record is selected |
| Retry save, cancel save | A save has failed |

**Set assignee** opens the same picker as `@`; **set base** opens the branch picker. Both apply to the marked set when marks are present, with one save and one undo. With marks, **dispatch N marked** opens the bulk dispatch card for the set; **dispatch again** ignores and clears marks, then relaunches only the cursor.

Search matches letters in order: `ssr` finds `set status: review`.

## Help

Press `?` on the board, task page, or another non-text surface to open the searchable shortcut reference. A dim divider separates its focused search field from shortcuts, grouped one binding per row by function. Long descriptions wrap beneath the description column. The card uses at most half the terminal height, except below 15 rows where it may grow to six rows so one result stays visible. Type or paste to filter by a key, action, group, or related term; use arrows, page keys, or the wheel to scroll. `Esc` clears a nonempty search first, then closes Help.

When a text field already owns input, `?` remains text instead of opening Help.

## Save failures

If saving fails, the draft stays open.

- `r` or `Enter`: retry.
- `c` or `Esc`: cancel the pending change.

Resolve the failure before making another change. See [storage](/docs/storage/) for directory and backup information.
