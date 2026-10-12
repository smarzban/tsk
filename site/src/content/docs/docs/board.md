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

`Esc` leaves multi-select and clears its marked tasks before it closes the current layer. At the full-board root, with multi-select inactive and no page, peek, popup, search, or header selection left to dismiss, it quits without confirmation. This applies on every tab, not just the desk. In either wide split, `Esc` closes the right column once any editor or overlay is dismissed, returning to the full-width board or projects index. Task drafts stay parked; a project preview with unsaved work refuses to close. After a narrow resize hides the right column, `Esc` treats the visible board/index as the root, with the same unsaved-draft protection. Quick capture's popup-close behavior is unchanged.

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

Press `/` on any board tab to search the rows that tab currently shows. On the Projects overview it matches project names or paths. On the desk, a project board, or a cross-project thread view it matches tasks by title, notes, step text, thread, or task number such as `T12`, case-insensitively. Every whitespace-separated word must match somewhere in the same task.

Typing or pasting filters immediately. Empty sections disappear and section counts show only matches. Search combines with a project thread filter; the done drawer is searched only while it is open. In the wide Projects preview, `/` searches the right project board when that seat has focus.

Press `Enter` to pin the query and return to board keys. Navigation, task actions, the done drawer, and a second `Enter` then act on the filtered rows; the query remains in the footer. Press `Esc` while typing to clear and close search, or press it once on a pinned board to clear the query before normal `Esc` behavior resumes. Changing tab, project, or Projects view also clears it.

## Threads

A thread groups related tasks, such as `release` or `login-fix`. A thread name can span projects and the desk, so it also works as a cross-repo initiative.

| Where | Action |
| --- | --- |
| Desk | Press `t` or click the filter to narrow by thread and project |
| Project board | Press `t` or click the filter to choose a thread |
| Projects overview | Press `v` or click **Overview** to view a thread across projects |

Type to filter the choices. Use arrows or `Tab` to select, `Enter` to choose, and `Esc` to close. `j` and `k` are search text in these selectors.

The desk filter has two tabs, **threads** and **projects**; press `←` or `→` (or click a tab) to switch. Each tab sets its own choice and the two combine, so `herdr-tsk #release` shows only that project's `release` tasks. The filter narrows every desk section, the done drawer included, and the control on the tab row names it (`all` when nothing is chosen). On a narrow desk with no filter, the `all` control is left off rather than taking a row of its own.

Assign threads when [capturing](/docs/capture/#title-tokens) or [editing a task](/docs/task-page/#scope-and-thread).

## Group and sort

Every task row carries a dim `#thread · project` label (`desk` for a desk task), at every width, including the narrow board rail beside an open task page. It follows the title two spaces after its last word; when it does not fit on the title's last line, it starts the next line at the title's indent and wraps like the title. Neither the title nor the label is ever truncated. A label leaves out what the view already says: a project board never names its own project, a thread filter drops the thread and a project filter the project, grouping drops its dimension inside the sections that actually group (the done drawer keeps it), and rows under **ON DECK · desk** never repeat `desk`. Archived rows carry the same label as done rows.

| Key | Action |
| --- | --- |
| `g` | Group by: **none**, **project**, or **thread** (a project board offers none or thread) |
| `s` | Sort by: **recent**, **project**, **thread**, or **title** (a project board has no project sort) |
| `f` | Fold or unfold the group the cursor is in: its sub-group, the inbox, or the archived group |
| `Shift+F` | Fold every group in view, or unfold them all when all are folded |

Grouping adds dim sub-headers such as `▾ herdr-tsk · 3`, `▾ #release · 2`, `▾ no thread · 1`, or `▾ desk · 1` inside **NEEDS YOU**, **IN MOTION**, **ON DECK**, and the **inbox**, whose sub-groups sit under its own fold. The section header keeps its total; each sub-header counts its own rows. Groups are ordered hottest first: the group whose newest row changed status most recently leads. The done drawer and the archived group never group, and grouping the desk by project leaves **ON DECK · desk** and its inbox alone because they only hold desk tasks.

Sorting reorders rows inside each group, or inside each section while ungrouped (the inbox included). **recent** is each section's normal order; the others sort by project name (desk last), thread name (unthreaded last), or title, and ties keep the normal order. `N` notices still lead.

Every task list's header shows `group: none ▾` and `sort: recent ▾` beside its filter (`all ▾`, a project, or a thread); click one to open its picker. The desk and project boards paint them on the tab row, the cross-project thread view beside its View control, and the wide Projects preview on its own header row beside the project name. When they do not fit, the pair shortens to `g:none ▾ s:recent ▾`. On narrow frames where even that does not fit beside the tabs, each control still at its default (`group: none`, `sort: recent`, the desk's unfiltered `all`) drops first, so a default never costs a row or pushes out a choice you made. If what you changed still does not fit, it moves to its own row under the tabs (the defaults rejoin it there when that row has room); `g`, `s`, and `t` work either way. The Projects preview has one header row, so there its name shortens first, then defaults drop, then changed controls from the left, the filter last. The Projects overview lists projects, not tasks, so it shows only its View control. A fresh board starts ungrouped and sorted by recent. The desk, project boards, and the cross-project thread view (which groups by project) each keep their own choice for the session, and the wide Projects preview keeps its own.

Every sub-header is a cursor stop, open (`▾`) or folded (`▸`); a folded group shows only its sub-header. With the cursor on a sub-header, `←` or `h` folds it and `→` or `l` unfolds it, leaving the cursor on the header; when there is nothing to fold or unfold, the key keeps its usual meaning (the peek, or the wide stage slider). The inbox and archived headings fold the same way. `Enter` on a sub-header opens what it names: a project sub-header opens that project's board, `desk` opens the desk (on the desk itself it changes nothing), and a `#thread` sub-header opens that thread's cross-project view, as choosing it with `v` does. `no thread` opens nothing. With the cursor on a header, the status row offers `enter open` (when it opens something) and `← fold` or `→ unfold`. A click on a sub-header folds or unfolds it; a double-click opens it like `Enter` and leaves the fold as it was. `Shift+M` on a sub-header unfolds it and marks every task in that group as one set; inside multi-select it adds the group to the marked set. Folding, or changing the grouping, sorting, or filter, clears multi-select marks.

## Status

| Section | Status |
| --- | --- |
| **NEEDS YOU** | `blocked`, `review` |
| **IN MOTION** | `started` |
| **ON DECK** | `ready`, `open` |
| ↳ **inbox** | `open` |
| Done drawer | `done` |

On your desk, **ON DECK** contains only desk tasks. On a project board, it contains that project's ready and open tasks. Ready tasks are the picked queue; open tasks are the untriaged inbox below it. Ready tasks sort by oldest pick first, open tasks by oldest capture first, and notice tasks lead within each group. The **inbox** group starts expanded; press `←` or `h` on its heading to fold it and `→` or `l` to unfold it, or `f` with the cursor inside it (when the inbox is grouped, `f` folds the cursor's sub-group instead). Use the filter to narrow the tasks, and [group and sort](#group-and-sort) to arrange them.

Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE, and the drawer's ARCHIVED group keep the most recent status change on top, while ON DECK lists ready and inbox backlogs oldest first. (`N` tasks lead each group until you clear them.) Editing a task or ticking a step never moves it; setting a status moves it to the top of its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching tabs, projects, threads, or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface.

On a task-board list, `ctrl+s`, `ctrl+n`, `ctrl+o`, `ctrl+d`, `ctrl+b`, `ctrl+r`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and actions from the task page always use only the cursor.

| Key | Action |
| --- | --- |
| `ctrl+s` | Start an open or ready task |
| `ctrl+n` | Set ready, the picked on-deck queue |
| `ctrl+o` | Set open, the inbox |
| `ctrl+d` | Mark done |
| `ctrl+b` | Set blocked; press again to return to ready |
| `ctrl+r` | Set review; press again to return to ready |

`ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Marked done tasks are skipped, and the status row names them: `started N · skipped M done (ctrl+n or ctrl+o reopens them)`. Bulk block and review toggles are all-or-nothing: if every target already has that status they all return to ready, otherwise they all move to that status. Other status verbs are absolute, so repeating the current status does nothing. Done tasks can be sent directly to ready or open.

Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops.

## Notices

Your first board open seeds four desk tasks with `N` ids (not `T`). They teach the board by being ordinary tasks: open, peek, change status, archive, or delete. `ctrl+d`, `ctrl+f`, and `ctrl+x` all dismiss a notice the same way; that starter task never re-seeds. After an upgrade, one `What's new in tsk` task can appear the same way. Details and the delivery record live under [storage](/docs/storage/#starter-guides-and-release-notes). Starter notices keep their seeded statuses so the tour order stays useful; a new release notice lands in NEEDS YOU as `review`, so it sits where you look first.

## Mouse

| Action | Result |
| --- | --- |
| Click a tab or selector | Change view or open its choices |
| Click the `group: …` or `sort: …` control | Open group by or sort by |
| Click a group sub-header | Fold or unfold that group |
| Double-click a group sub-header | Open its project's board, the desk, or its thread's view |
| Click a task outside multi-select | Peek in narrow panes; open or update details beside the board in wide panes |
| Click a task or its `T`/`N` number in multi-select | Move the cursor there and toggle its mark |
| Click the same task again in a narrow pane | Close its peek |
| Double-click a task | Open it full screen |
| Click its `T` or `N` number | Copy that id |
| Click a footer action | Run that action |
| Wheel or drag a scrollbar | Scroll |
| Drag across text | Select and copy on release |

Open peeks show notes and close on the row's full `#thread · project` label (`└─ #release · tsk`); a peeked row moves its label into the peek, so it never paints twice. Below 110 columns, `→` or `l` opens a peek and `←` or `h` closes it. Peeks show up to five wrapped note lines; the [task page](/docs/task-page/) shows the rest.

## Wide stage slider

At **110 usable columns** or wider, use `→` / `l` and `←` / `h` to move through a four-stage slider:

| View | What you see |
| --- | --- |
| Board | Full-width board |
| Split | Board beside task details; selecting another task updates the details |
| Task | Task details beside a narrow board rail |
| Full screen | Task details only |

Press `Enter` from the board to open a task full screen. `Esc` returns to the view you left. The rail keeps the board's done drawer and archived fold state, so a done task opened from the drawer stays selected beside its page. From task focus, `Esc` returns focus to the board beside it. On Projects Overview, the slider stops at Rail: it shows a live project board beside the index and never opens a full-screen task; `Enter` in that right column opens the task page inside the column.

Click a task on the full-width board to open its details alongside it, keeping board focus. Click inside the task column to focus it. Click a task in the rail to bring back the split board. While editing, arrows move the text cursor instead. On the projects tab, click the right preview to focus its live board, or click an index row from Rail to rebind it and return to Split.

Narrowing the pane shows one surface; widening it restores the selected view. Each new session starts with the board alone.

| Pane size | Layout |
| --- | --- |
| 110 columns or wider | Board and task views |
| At least 78×24, below 110 columns | Single board or task page |
| Smaller | Compact layout; usable down to 40×10 |

## Completed tasks

Press `d` to open or close the done drawer. Select a done task and press `ctrl+n` to return it to ready, or `ctrl+o` to send it to the inbox.

The drawer's **archived** group starts closed. Click its heading or press `→` or `l` on it to expand, and `←` or `h` to close it. `f` with the cursor inside it folds it again; `Shift+F` folds or unfolds it with every other group.

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

Press `:` on the board or a task page and type to find an action. Use arrows or `Tab` to select, `Enter` to run, and `Esc` to close.

| Available actions | When |
| --- | --- |
| New task, undo, done drawer, group by…, sort by…, fold / unfold group, help, quit | Always |
| Set open/ready/started/blocked/review, edit notes, change scope, delete | A task is selected |
| Move step up, move step down | A task-page step is selected |
| Retry save, cancel save | A save has failed |

Group by…, sort by…, and fold / unfold group act on the task list, so they are left out on the projects overview and over an open task page. Search matches letters in order: `ssr` finds `set status: review`.

## Help

Press `?` on the board, task page, or another non-text surface to open the searchable shortcut reference. A dim divider separates its focused search field from shortcuts, grouped one binding per row by function. Long descriptions wrap beneath the description column. The card uses at most half the terminal height, except below 15 rows where it may grow to six rows so one result stays visible. Type or paste to filter by a key, action, group, or related term; use arrows, page keys, or the wheel to scroll. `Esc` clears a nonempty search first, then closes Help.

When a text field already owns input, `?` remains text instead of opening Help.

## Save failures

If saving fails, the draft stays open.

- `r` or `Enter`: retry.
- `c` or `Esc`: cancel the pending change.

Resolve the failure before making another change. See [storage](/docs/storage/) for directory and backup information.
