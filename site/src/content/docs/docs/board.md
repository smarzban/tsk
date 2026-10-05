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

`Esc` leaves multi-select and clears its marked tasks before it closes the current layer; an open thread, view, or assignee picker closes first and keeps the marks. At the full-board root, with multi-select inactive and no page, peek, popup, search, or header selection left to dismiss, it quits without confirmation. This applies on every tab, not just the desk. In either wide split, `Esc` closes the right column once any editor or overlay is dismissed, returning to the full-width board or projects index. Task drafts stay parked; a project preview with unsaved work refuses to close. After a narrow resize hides the right column, `Esc` treats the visible board/index as the root, with the same unsaved-draft protection. Quick capture's popup-close behavior is unchanged.

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

Press `/` on any board tab to search the rows that tab currently shows. On the Projects overview it matches project names or paths. On the desk, a project board, or a cross-project thread view it matches tasks by title, notes, step text, thread, assignee (with or without its `@`), or task number such as `T12`, case-insensitively. Every whitespace-separated word must match somewhere in the same task.

Typing or pasting filters immediately. Empty sections disappear and section counts show only matches. Search combines with a project board's thread and assignee filter; the done drawer is searched only while it is open. In the wide Projects preview, `/` searches the right project board when that seat has focus.

Press `Enter` to pin the query and return to board keys. Navigation, task actions, the done drawer, and a second `Enter` then act on the filtered rows; the query remains in the footer. Press `Esc` while typing to clear and close search, or press it once on a pinned board to clear the query before normal `Esc` behavior resumes. Changing tab, project, or Projects view also clears it.

## Threads and assignees filter

A thread groups related tasks within a project, such as `release` or `login-fix`. An [assignee](#assignees) names the agent profile a task belongs to. Both narrow a board through one picker with two tabs, `threads · @assignees`.

| Where | Action |
| --- | --- |
| Project board | Press `t` or click the filter to open **Filter**; choose a thread, an assignee, or both |
| Projects overview | Press `v` or click **Overview** to view one thread or one assignee across projects |

On a project board the threads tab lists `all`, the project's threads, then **Without a thread**; the `@assignees` tab lists `all`, every profile in `config.toml` as `@name`, any name still on tasks whose profile was removed, then `unassigned`. The two choices combine: `#release` plus `@claude` shows only release tasks assigned to claude, and the filter control reads `#release @claude`. `all` clears only its own tab's choice, and `✓` marks the active choice in each tab. The choice is for this session only, clears when you switch projects, and search (`/`) narrows inside it.

On the Projects overview the threads tab offers **Overview** and each thread; the `@assignees` tab offers one `@name` view per assignee (there is no `unassigned` view). An `@name` view lists that assignee's tasks from every project and the desk, grouped by status like a thread view, with done tasks in the drawer. One view applies at a time.

The picker opens on the threads tab (an `@name` overview view reopens on its own tab). `Tab` or a click switches tabs and clears the typed filter. Type to filter the current tab, use arrows to select, `Enter` to choose, and `Esc` to close without a change. `j` and `k` are search text in these selectors.

Assign threads when [capturing](/docs/capture/#title-tokens) or [editing a task](/docs/task-page/#scope-thread-and-assignee).

## Assignees

An optional assignee links a task to the exact name of an agent profile in [`config.toml`](/docs/storage/#agent-profiles). Rows stay a title; the peek (`→`) footer names `@assignee · ⎇ <base> · #thread · project`, in that order, for whatever is set. The peek shows a base only when explicitly set, not the default. Press `@` (or use **set assignee** in the palette) to open the assignee picker: every profile in `config.toml`, then **none**. The cursor task's assignee is preselected, or the first profile when it has none. `↑`/`↓` move, typing filters, `Enter` applies, and `Esc` closes without a change. With marked tasks, one choice updates the whole set and one `ctrl+u` reverses it. With no profile defined, `@` says so and opens nothing. On the Projects overview `@` does nothing.

Press `ctrl+g`, or choose **dispatch to @name** from the palette, to send the cursored task to its assigned agent. On an unassigned task `ctrl+g` opens the assignee picker first: `Enter` on a profile saves the assignment, then dispatches; **none** or `Esc` changes nothing. If the launch then fails, the task stays assigned. With no profile defined, `ctrl+g` refuses: "no agent assigned: press @ or add a profile to config.toml". tsk creates a dedicated Git worktree and Herdr workspace, renders the profile command there, starts the task, then records the worktree, branch, workspace, command argv, and time as one save. The branch is `tsk/t<n>-<slug>`, where the slug is the title lowercased with every run of characters other than letters and digits (in any script) turned into one `-`, keeping whole words while it stays within 30 characters (a longer first word is cut at 30); a title of only symbols gives plain `tsk/t<n>`. Herdr names the worktree directory from the branch with every run of characters other than ASCII letters and digits as one `-` (`tsk-t<n>-<slug>`, so `Café` checks out in `tsk-t<n>-caf`), and the Herdr workspace is labelled `T<n>` plus the title up to the same cut, ending in `…` when the title was cut. If a local or remote branch, a registered worktree, or a directory at the checkout path already has that name, tsk appends `-2`, `-3`, and so on. A relaunch keeps the recorded names. If creating or launching fails, task state does not change. After saving, tsk names the Herdr agent `t<number>-<assignee>` in the background (for example `t12-claude`, so `herdr agent get t12-claude` finds it; dots become hyphens and the name is cut to Herdr's 32 characters). If Herdr detects no agent within a few seconds, the name is taken, or a relaunch finds an agent still running in the pane, the agent stays unnamed and dispatch still succeeds. Dispatch requires Herdr, a project-scoped task in a Git repository, and a non-done, non-archived task with a known assignee. Dispatch runs on macOS and Linux only; on Windows `ctrl+g` refuses with "dispatch needs herdr on macOS or Linux".

With tasks marked, `ctrl+g` (or **dispatch N marked** in the palette) opens one card instead: a row per task it will launch with its assignee and base (`from dispatch`, or `from default (main)` for the repository default), then the skipped tasks with the same reason a single dispatch would refuse with (unassigned, unknown agent, already dispatched, not a project in a git repo, done, archived). The card opens at once; its rows say `checking…` while the repositories are checked in the background, and a task outside a git repository then moves to the skipped rows. Unassigned tasks are skipped, not prompted; press `@` on the set first. `y` launches each listed task exactly as a single dispatch would, in the background while the board stays usable; the status row counts `dispatching 2/3…` and each launch is saved (record and started) as it lands. A failed launch does not stop the others, and the status row ends with what launched and what failed and why. A task dispatched elsewhere meanwhile keeps that record and is reported as failed; a task whose status you changed after `y` gets its record but keeps your status (`T12 kept done (changed meanwhile)`). If a save fails, the launches wait in save recovery: Retry keeps them, Cancel discards their records and the status row says `launched but not recorded: T12 (agent running in workspace w5)`. Leaving the Projects preview that started the launches does not stop them being recorded. Marks clear when the launches start; `Esc` or `[x]` changes nothing and keeps them. If nothing marked can be dispatched, `ctrl+g` refuses on the status row ("nothing to dispatch: unassigned") without a card. While launches are landing, another dispatch and quitting wait until they finish, and if the task store cannot be read, other changes are refused on the status row ("nothing changed") instead of closing the board. A failed save of a landing never closes a task edit you have open: Retry or Cancel leaves its drafts as they were. **dispatch again** stays cursor-only.

Choose **set base** in the palette to open a query-filtered branch picker. It offers **default (main)** (for a repository whose default is `main`) first, then deduplicated local and `origin/*` branches. It opens at once on the branches already on disk. Unless origin was fetched in the last minute (the [fetch window](/docs/storage/#fetch-window)), a disabled **refreshing…** row shows while a bounded background fetch runs; the list then refreshes in place and keeps your selection. If the fetch fails, the row reads **offline, showing cached branches**. Reopening the picker while its project fetch is running reuses that fetch, so cancellation and reopen cannot pile up fetches. Choosing **default** clears the explicit base. A choice applies to all marked tasks as one save and one undo, and each branch must exist in its task's repository. The same picker opens from the task page's `⎇` footer or edit **Base** field. There is no dedicated base key; `ctrl+g` never prompts for one.

Dispatch starts from an explicit task base when set; otherwise it uses the task repository's remote default (`origin/HEAD`). The board's or CLI's checkout never supplies the default. A local branch with a remote upstream uses that upstream after a bounded best-effort fetch; a local-only branch is used directly. Offline dispatch falls back to the local ref and reports it. The dispatch records the actual ref and commit, shown on the task page as `from <ref> @ <short sha>`.

On a task with a dispatch record, the first `ctrl+g` names its worktree and asks for another press; the second relaunches there. The palette offers **dispatch again** instead. Relaunch keeps the recorded base, even if the task's explicit base changed. A cleaned record recreates the worktree, reopening a retained branch or recreating a removed branch from its recorded original starting commit (falling back to the recorded base ref for legacy records).

## Status

| Section | Status |
| --- | --- |
| **NEEDS YOU** | `blocked`, `review` |
| **IN MOTION** | `started` |
| **ON DECK** | `ready`, `open` |
| ↳ **inbox** | `open` |
| Done drawer | `done` |

Status glyphs are `◌` open, `○` ready, `●` started, `■` blocked, `▲` review, and `✓` done. A started task with a live dispatch uses `◉` instead of `●`; changing its human status or cleaning the dispatch restores the normal glyph.

On your desk, **ON DECK** contains only desk tasks. On a project board, it contains that project's ready and open tasks. Ready tasks are the picked queue; open tasks are the untriaged inbox below it. Ready tasks sort by oldest pick first, open tasks by oldest capture first, and notice tasks lead within each group. The **inbox** group starts expanded; press `Enter` on its heading or `g` while the done drawer is closed to fold or unfold it. With the drawer open and archived tasks available, `g` addresses its archived group; otherwise it addresses the inbox. Use the thread and assignee filter to narrow the tasks.

Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE, and the drawer's ARCHIVED group keep the most recent status change on top, while ON DECK lists ready and inbox backlogs oldest first. (`N` tasks lead each group until you clear them.) Editing a task or ticking a step never moves it; setting a status moves it to the top of its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching tabs, projects, threads, or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface, except in a thread, view, or assignee picker, where it closes the picker first.

On a task-board list, `ctrl+s`, `ctrl+n`, `ctrl+o`, `ctrl+d`, `ctrl+b`, `ctrl+r`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and actions from the task page always use only the cursor; `ctrl+g` on a marked set opens the bulk dispatch card.

| Key | Action |
| --- | --- |
| `ctrl+s` | Start an open or ready task |
| `ctrl+n` | Set ready, the picked on-deck queue |
| `ctrl+o` | Set open, the inbox |
| `ctrl+d` | Mark done; offer to clean a live dispatched worktree |
| `ctrl+b` | Set blocked; press again to return to ready |
| `ctrl+r` | Set review; press again to return to ready |

`ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Bulk block and review toggles are all-or-nothing: if every target already has that status they all return to ready, otherwise they all move to that status. Other status verbs are absolute, so repeating the current status does nothing. Done tasks can be sent directly to ready or open.

With no marks, `ctrl+d` on a live dispatched worktree asks before completing. The card, titled `Done T<n> · clean up?`, says what `y` will do: its first line is `Merged into <recorded ref> ✓`, `Not merged into <recorded ref> (squash-merged? delete it by hand)`, or, when the fetch could not reach the remote, `Merge into <recorded ref> not confirmed (offline), so the branch stays.`, then one row each for the branch (`delete branch` or `keep branch`), the worktree (`remove worktree`, with your home directory shown as `~`) and the agent pane (`close`). It opens at once from the refs already on disk with `Checking merge into <recorded ref>…` while a background fetch of the recorded base runs, then fills in merged status; inside the [fetch window](/docs/storage/#fetch-window) the cached status is shown directly. Pressing `y` before the check finishes queues the cleanup: the card says `Cleaning up once the check finishes.` and stays responsive, and cleanup runs as soon as the check lands, even if the card sits in a project preview that a narrowing pane has parked. If the check has not finished within its bound (the fetch plus the ancestry query deadlines), the clean worktree is still removed but the branch is kept, because its merge was never confirmed. `y` safely cleans then marks done, `n` marks done and keeps everything, and `Esc` changes nothing. The footer choices and `[x]` (same as `Esc`) are clickable. A worktree with uncommitted changes cannot be cleaned: the card is titled `Done T<n> · can't clean up`, shows that the branch, worktree and agent pane all stay, and offers only `n mark done, keep everything` or cancel. If the recorded worktree is already missing, the dispatch is marked cleaned and the task completed in one save without a card. Already-done tasks, archived tasks, and archived project views never open this card.

With tasks marked, `ctrl+d` on a set that holds live dispatches opens one card for the whole set, titled for example `Done 4 tasks · clean up 2 of 3?` (how many dispatched worktrees `y` can clean). When a narrow pane leaves no room for that, the title shortens to `Done 4 tasks` and the question moves to the card's first line, so the counts are never cut. Each dispatched task gets a row with its merged status (`checking…` until its own background check lands, then `merged ✓`, `not merged into <ref>`, `not confirmed (offline)`, or `uncommitted changes`) and, below it, what `y` will do for it (`delete branch · remove worktree · close pane`, `keep branch · …`, or `keep everything`); bulk rows leave out paths and branch names. Marked tasks with no dispatch are listed as `+ T<n> has no dispatch, just marked done`, and dispatches whose worktree is already gone converge to cleaned with either choice. `y` cleans every clean worktree under the same rules as the single card (a branch whose check never finished or could not reach the remote is kept), keeps dirty ones, then marks the whole set done; `n` marks the whole set done and keeps everything; `Esc` changes nothing and keeps the marks. Either way the completion is one save and one undo entry, and undo reverses only the completion, never the cleanup. A long set scrolls with `↑`/`↓` or the wheel, like the help card. A marked set with no live dispatch completes at once, as before.

Cleanup removes a clean recorded worktree and its matching Herdr workspace. It refuses a path that is the project root, is not registered to that project, or disagrees with the recorded Herdr workspace. After a bounded best-effort fetch (skipped inside the [fetch window](/docs/storage/#fetch-window)), it checks ancestry against the recorded base, so a remote merge counts without a local pull. It removes the branch only when it is merged into the recorded dispatch base; an unmerged branch or a legacy dispatch with no base is kept. Worktree listings, status and ancestry checks have a separate 10-second deadline, rather than the short metadata deadline. A preflight timeout refuses before removal; if the final branch recheck times out after worktree removal, the branch stays with an explanation. If the fetch fails or times out, merged status is not confirmed: refs on disk may predate a force-push or reset of the remote base, so the clean worktree is still removed but the branch is always kept, the card says `not confirmed (offline)` and `keep branch`, and shows why the fetch failed. Inside the fetch window the refs on disk are fresh and count as confirmed. A local base without a remote upstream has nothing to fetch, so its ancestry is checked on local refs. A missing or pruned base does not prevent clean worktree removal: the branch stays and the card says the base is no longer available. Squash merges do not establish ancestry: only a failed ancestry check gets the explanation "not merged into origin/main; squash-merged? delete by hand". Branches checked out elsewhere or changed during cleanup stay with their own reason. The dispatch record remains marked cleaned, and undo reverses only the completion. Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops.

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
