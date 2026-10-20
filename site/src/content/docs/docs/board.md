---
title: Board
description: Find tasks, switch projects, and keep work moving.
---

The board is where you see and move your tasks. This page covers its views, sections, and tools. [Keys](/docs/keys/) lists every shortcut, and [Dispatch](/docs/dispatch/) covers handing tasks to agents.

Click, or use the keyboard. The footer lists the actions for the cursor or the marked set, and each one is clickable. It ends with `: commands` and `? help`. In a narrow pane the footer shortens its labels, then drops whole actions, but keeps the main action, the way out, `: commands`, and `? help`. Only a pane too narrow even for those drops `: commands`.

## Views

| View | Tab | Shows |
| --- | --- | --- |
| **desk** | `1` | Blocked, review, and started tasks from every live project; ready and open tasks from your desk |
| **project** | `2` | The selected project's tasks |
| **projects** | `3` | The [Projects view](#projects) |

Inside a Git repository the board opens on that project. Outside Git it opens on the desk and puts the current directory in the project tab. The project tab remembers the selected project; with none selected, it opens the project picker.

In Herdr, `prefix+t` opens or focuses the board in the current workspace ([Add to Herdr](/docs/install/#add-to-herdr)). All boards share the same tasks.

## Quit and go back

- `Esc` closes the current layer: a peek, picker, search, task page, or the right column of a wide split. At the full-board root it quits, on any tab.
- `ctrl+q` quits from anywhere except text entry. `ctrl+c` quits from the board, a task page, and Help.
- Unsaved drafts must be saved or cancelled first, and a [failed save](#save-failures) resolved.

In multi-select, `Esc` clears the marks first. The full order is under [Esc and quit](/docs/keys/#esc-and-quit).

## Projects

Press `p` for the project picker, or open the **Projects view** for an overview. Click a project to select it; double-click it or press `Enter` to open it on the project tab. The footer shows the selected project's full path.

The Projects view counts each project's work:

| Column | Counts |
| --- | --- |
| **NEEDS YOU** | Blocked or in review on you |
| **IN MOTION** | Started, or waiting on someone else |
| **ON DECK** | Ready and open, the inbox included |
| **DONE** | Done |

Archived tasks and archived projects are left out. A dim `·` means zero, and `here` marks the project the board opened in. At 100 columns or wider, the view also lists threads. At 110 it can show a project's board beside it ([Wide layout](#wide-layout)).

## Search

Press `/` to filter the rows the current tab shows. Typing filters at once: empty sections disappear, and section counts show only matches.

| Tab | Matches |
| --- | --- |
| Projects view, on **Overview** | Project names and paths |
| Desk, project board, thread or assignee view | Title, notes, step text, thread, assignee (`claude` or `@claude`), task number (`T12`) |

In a task list, every word you type must match somewhere in the same task, ignoring case. Search narrows inside the [filter](#filter). The done drawer is searched only while it is open.

`Enter` pins the query and hands keys back to the board, so actions work on the filtered rows. `Esc` clears the query. Changing tab, project, or Projects view clears it too.

## Filter

A **thread** groups related tasks, such as `release` or `login-fix`, and can span projects and the desk. An **assignee** names who a task is for: an [agent profile, you, or a person](/docs/dispatch/#assign). Press `t` or click the filter control to narrow a board by either.

| Where | Filter |
| --- | --- |
| Project board | Two tabs: `threads · @assignees` |
| Desk | Three tabs: `threads · projects · @assignees` |
| Projects view | `v` opens one thread or one assignee across all projects |

- On a project board, the threads tab lists `all`, the threads, then **Without a thread**.
- The `@assignees` tab lists `all`, then `@you` when a task is yours, each profile as `@name`, the other names on tasks (people, and profiles since removed), then `unassigned`.
- Choices from different tabs combine: `#release @claude` shows release tasks assigned to claude. `all` clears its own tab.

The desk's tabs work the same way, and its projects tab also offers `desk`. The desk filter lasts for the session. A project board's filter clears when you leave that board. A thread or `@name` view lists matching tasks from every project and the desk, grouped by status. There is no `unassigned` view.

Give a task a thread when you [capture](/docs/capture/) or [edit](/docs/task-page/#edit) it.

## Group and sort

Press `g` to group, `s` to sort, or click the `group: none ▾` and `sort: recent ▾` controls beside the filter.

| | Choices |
| --- | --- |
| Group by | none, project, thread, assignee |
| Sort by | recent, project, thread, title, assignee |

Neither offers what the view already fixes: a project board has no project choice, a thread view no thread, an `@name` view no assignee.

Grouping adds dim sub-headers inside NEEDS YOU, IN MOTION, ON DECK, and the inbox, such as `▾ #release · 2` or `▾ unassigned · 1`. Each sub-header counts its rows. The group whose newest row changed status most recently comes first. The done drawer and its archived group never group.

Sorting reorders rows inside each group, or inside each section when ungrouped. **recent** is the section's normal order. The others sort by project (desk last), thread (unthreaded last), title, or assignee (yours first, unassigned last). Notices still lead.

The desk, project boards, thread views, and `@name` views each keep their own choice for the session. The desk and project boards start from the board defaults in [`config.toml`](/docs/storage/#configuration), if set; everything else starts ungrouped and sorted by recent. Session changes are never written back.

In a narrow pane the controls shorten to `g:none ▾ s:recent ▾`, and controls still at their default drop first.

### Fold

Every sub-header is a cursor stop. You can fold the group under the cursor, or every group at once, and the inbox and archived headings fold the same way. A folded group shows only its sub-header.

`Enter` on a sub-header opens what it names: a project's board, the desk, or a thread's or assignee's view. `no thread` and `unassigned` open nothing. `shift+M` on a sub-header marks every task in the group.

## Sections and status

| Section | Holds |
| --- | --- |
| **NEEDS YOU** | `blocked` on you, or on a task that is done or deleted; `review` on you |
| **IN MOTION** | `started`; `blocked` on another task or something else; `review` on an agent or something else |
| **ON DECK** | `ready`, then `open` in the **inbox** |
| Done drawer | `done` |

On the desk, ON DECK holds only desk tasks. On a project board, it holds that project's.

| Glyph | Status |
| --- | --- |
| `◌` | open |
| `○` | ready |
| `●` | started |
| `◉` | started, with a live [dispatch](/docs/dispatch/#dispatch) |
| `■` / `□` | blocked on you / elsewhere |
| `▲` / `△` | review on you / elsewhere |
| `✓` | done |

NEEDS YOU, IN MOTION, and the done drawer put the most recent status change on top. ON DECK lists ready tasks oldest pick first, so a task you set ready joins the end. The inbox keeps capture order, oldest first. Editing a task or ticking a step never moves it.

The status shortcuts set open, ready, started, done, blocked, and review ([Keys](/docs/keys/#board)). Repeating the current status does nothing, except block and review: on a blocked or in-review task, they return it to ready. A done task can go straight back to ready or open.

Status never changes on its own when steps are ticked or an agent stops. The one automatic change is a ready task starting when its last [prerequisite](#after) is done.

### Rows

Each row carries a dim label after its title: `#thread · project`, or `desk` for a desk task. The label leaves out what the view already says, such as a project board's own project. Titles and labels wrap and are never cut.

A row that waits leads its label with why:

| Lead | Means |
| --- | --- |
| `waiting on T12`, `waiting on @pi` | Blocked on another task, an agent, or something else |
| `@pi reviewing` | In review with someone else |
| `after T202, T205` | Ready or open, waiting on unfinished [prerequisites](#after) |
| `before T205` | Unfinished tasks wait on this one |

A row addressed to you carries a dim **live line** under it, starting `└─`, with the asker's own words:

| Row | Live line |
| --- | --- |
| Blocked on you | Their newest reply, else the block's needs or why: `@claude: the client id from the provider · 2h` |
| In review on you | Their newest reply, else what was done: `@claude: Built the widget; empty input now handled. · 40m` |
| Blocked on a task that is now done or deleted | `T202 is done · unblock it · 1h` |

The age is how old the message is, and the asker is left out when it is you. A block or review with nothing said has no live line. Neither do rows waiting elsewhere, started and done rows, or notices. When a blocking task is done, the blocked task returns to NEEDS YOU, but its status does not change.

### Peek

Below 110 columns, a click or `→` opens a **peek** under the row. A wider pane opens the details beside the board instead ([Wide layout](#wide-layout)). The peek shows up to five wrapped note lines, then a footer: `@assignee · ⎇ <base> · #thread · project`, without what is unset or already said.

A blocked task's peek shows the block instead of notes: who it waits on, the why, then the options or needs. A review's peek shows what was done and every check: `○` open, `✓` passed, `✗` failed. A task with prerequisites lists them with their status, such as `after T202 · started`. The [task page](/docs/task-page/) shows everything.

## Multi-select

`shift+M` turns on multi-select. Mark tasks with `Space`, `shift+↑`/`shift+↓`, or a click; marked rows show `▪`.

With tasks marked, these act on the whole set as one save and one undo step: status changes, delete, archive, assignee, base, and after. `Enter`, editing, task-page actions, and relaunch stay on the cursor.

- Starting a set that would launch agents opens [one card first](/docs/dispatch/#start-a-marked-set).
- Starting moves only open and ready tasks. It skips marked done tasks, and the status row counts them.
- Block and review also skip marked done tasks, and the status row counts them. The rest move together: if every one already has that status, all return to ready. Otherwise all move to it, through one card.

Marks last for this session only. Any action, `Esc`, or a view change clears them: fold, group, sort, filter, tab, project, or done drawer. Removing the last mark leaves the mode on.

## Block, review, reply

`ctrl+b` blocks a task and `ctrl+r` puts it up for review. Both open a card, and `Enter` on an empty card sets the status with nothing said.

| Card | Fields |
| --- | --- |
| Block | **why** it stops · **on**: you, another task, or other text · **needs**: what would unblock it |
| Review | **done**: what was done · **check**: one thing to verify per line · **next** · **on**: you, an agent profile, or other text |

A block or review on you lands in NEEDS YOU. One on another task, an agent, or something else goes to IN MOTION. Leaving `blocked` closes the block. Card keys are under [Keys](/docs/keys/#cards).

To hand an open block or review to someone else later, change who it [waits on](/docs/task-page/#waiting-on) from the task page or the palette. The block or round keeps everything else.

Agents block and ask for review from the [CLI](/docs/cli/#status). Each review is a **round**: after a send-back, the agent's next review opens the next round.

Press `r` on a blocked or review row to reply, or give feedback, right under it. The box and its keys are the [task page's](/docs/task-page/#blocked). `r` answers the cursor row only. On any other row the status row says `not blocked or in review`.

### Replies reach a running agent

You can unblock a dispatched task whose agent still runs, or send its review back. Once that is saved, tsk sends your answers to the agent:

- An unblock sends every reply you left on the block, oldest first: `[tsk T12 unblocked] <your reply>`. With no reply of yours, a start sends the bare tag and a return to ready sends nothing.
- A send-back sends every feedback entry of the round, then the failed checks: `[tsk T12 sent back] <feedback> Failed checks: <check>; <check>`.

Two or more entries are numbered, one per line. Each reply goes out once: one already sent with [`tsk reply --send`](/docs/cli/#reply) is skipped. Saving a reply without unblocking sends nothing.

tsk sends only to the agent it dispatched, named like `t12-claude`. The status row says how it went, for example `started · reply sent to @claude`.

When nothing was sent, the status row says why: the agent is waiting on a prompt, cannot be reached, or is not in its pane. Outside Herdr nothing is sent. Your reply stays on the task either way, and nothing relaunches or retries.

## After

A task can run **after** other tasks: it waits while any of them is not done. Task numbers are board-wide, so a prerequisite can be in any project or on the desk.

Set it with:

- the task page's **After** field,
- palette **set after (wait for)…**, for the cursor task or every marked task,
- `!w T202` in [quick-add](/docs/capture/),
- [`tsk edit --after`](/docs/cli/#edit).

The task picker lists the other tasks that are not done, this project's first. `Space` ticks several, and **none** clears the list. A loop is refused: `T202 already runs after T205`. Palette **chain marked tasks in order** makes each marked task run after the one marked before it, in one undo step.

When the last prerequisite is done, each waiting **ready** task starts in the same save, and an assigned one [dispatches](/docs/dispatch/#dispatch). Review does not count as done. The status row says so, for example `T205 started · T202 done`. Waiting tasks in open, blocked, or review stay put.

- A launch that fails puts the task back to ready and never blocks the done.
- `ctrl+u` on the done reverts the statuses only. A launched agent keeps running.
- Starting a waiting task yourself asks first: `Start anyway?`.
- Deleting a task removes it from every after list, and `ctrl+u` restores the links. A ready task left waiting on nothing does not start.

## Notices

Your first board open adds four starter tasks to the desk, numbered `N1`… instead of `T`. They teach the board by being ordinary tasks: open, peek, change status, archive, or delete them. Done, archive, and delete all dismiss a notice for good.

After an upgrade, one `What's new in tsk` notice can appear in NEEDS YOU, in review. The CLI never shows notices. See [Starter tasks](/docs/storage/#starter-tasks).

## Mouse

| Action | Result |
| --- | --- |
| Click a tab, selector, or the `group:` or `sort:` control | Change view, or open its choices |
| Click a task | Peek it in a narrow pane; open its details beside the board in a wide one |
| Click the same task again, in a narrow pane | Close its peek |
| Double-click a task | Open it full screen |
| Click a `T` or `N` number | Copy the id |
| Click a task or its number in multi-select | Toggle its mark |
| Click a header | Select it |
| Double-click a sub-header | Open what it names |
| Double-click a header that opens nothing: `no thread`, `unassigned`, `desk` on the desk, the inbox, archived | Fold or unfold it |
| Click a footer action | Run it |
| Wheel, or drag a scrollbar | Scroll |
| Drag across text | Select it; it is copied on release |

## Wide layout

At **110 usable columns** or wider, `→` and `←` move through a four-stage slider:

| View | What you see |
| --- | --- |
| Board | Full-width board |
| Split | Board beside task details; selecting another task updates them |
| Task | Task details beside a narrow board rail |
| Full screen | Task details only |

`Enter` on the board opens a task full screen, and `Esc` returns to the view you left. Clicking a task on the full-width board opens Split, with focus still on the board. Click inside the task column to focus it. While you edit, arrows move the text cursor instead.

On the Projects view the slider stops at Rail:

| View | What you see |
| --- | --- |
| Full board | The Projects view, full width |
| Split | The Projects view beside a dim preview of the selected project, keeping focus |
| Rail | A narrow Projects view beside that project's live board, which takes input |

Moving the selection opens Split. The right board has its own selection, drawer, filters, task page, and quick-add, and never opens a task full screen. `Esc` returns focus to the Projects view.

| Pane size | Layout |
| --- | --- |
| 110 columns or wider | Board and task side by side |
| At least 78×24, below 110 columns | One board or task page |
| Smaller | Compact layout, usable down to 40×10 |

Narrowing the pane shows one surface, and widening it restores the view. Each new session starts with the board alone.

## Done drawer and archive

`d` opens or closes the done drawer. Set a done task to ready or open to bring it back.

Archiving hides a task or project without changing its status. Archive the selected task with `ctrl+f`. It moves to the drawer's **archived** group, which starts folded; select it there and press `ctrl+f` or `ctrl+u` to restore it. Archiving or restoring tasks is one undo step, for one task or a marked set.

Archive a project from the project picker with `ctrl+f`. Its **archived** tab lists archived projects: `Enter` views one read-only, and `ctrl+f` or `ctrl+u` restores it. Restoring a project leaves tasks you archived one by one archived. Archived projects are not offered as destinations.

Archiving a project cannot be undone, nor can an archive made with `tsk archive`.

Launching the board inside an archived project asks whether to restore it. Keeping it archived sends new captures to your desk for that session. The cursor starts on keep, as on any [decision card](/docs/dispatch/#decision-cards).

## Delete and undo

`ctrl+x`, pressed twice, deletes the marked tasks, or the cursor task when nothing is marked. The status row then reads, for example, `Deleted T12 "Fix login timeout" · ctrl+u undo`.

`ctrl+u` undoes the last change, from the board or the task page. The status row names the tasks it put back, such as `done undone · T12 back to started`. Undo covers:

- delete, done, archive, and restore,
- block and review cards, and every other status change,
- a start that dispatched, leaving the agent running,
- step changes on the task page,
- assignee, base, after, and waiting-on changes.

A marked set is one undo step. Undo keeps the last 50 actions and never stops an agent.

If a task changed since the action, undo refuses and changes nothing: `T12 changed since it was completed (archived); nothing undone`. On an archived cursor, `ctrl+u` restores that task, unless the last action was an archive.

Deleted tasks wait in the [trash](/docs/storage/#deleted-tasks) for a while. They never appear on the board.

## Palette

Press `:` on the board or a task page and type to find an action. Matching takes letters in order: `rev` finds **set status: review**.

| Available actions | When |
| --- | --- |
| Add task, undo, show / hide done drawer, show all keys, quit | Always |
| Group by…, sort by…, fold / unfold group | A task list has focus |
| Set open/ready/started/blocked/review, edit notes, move to project / desk, delete task | A task is selected |
| Set assignee, set base, set after (wait for)… | A task is selected |
| Set waiting on… | A task with an open block or review is selected or marked |
| Dispatch again | A task with a dispatch record is selected |
| Chain marked tasks in order | Two or more tasks are marked |
| Move step up, move step down | A task-page step is selected |
| Retry save, cancel save | A save has failed |

A task list has focus unless the Projects view or a task page does. **Set assignee**, **set waiting on…**, **set base**, and **set after** act on the marked set when there is one. **Set waiting on…** needs an open block or review on every task it acts on. **Set status: started** works like `ctrl+s`, so a task assigned to an agent dispatches. **Dispatch again** clears the marks and [relaunches](/docs/dispatch/#relaunch) the cursor task. It is not offered for a task assigned to you or a person.

## Help

`?` opens a searchable list of every shortcut, on any surface that is not taking text. Type to filter by key, action, or group. `Esc` clears the search, then closes Help.

## Save failures

If a save fails, your change stays open and the status row says why. Press `r` or `Enter` to retry, or `c` or `Esc` to cancel the change. Resolve it before you make another change or quit. [Storage](/docs/storage/) covers the state directory and backups.
