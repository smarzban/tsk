---
title: Keys
description: Keyboard shortcuts by action and surface.
---

Use the mouse, the keyboard, or both. Tabs, selectors, tasks, and footer actions are clickable. Press `Shift+M` to enter multi-select, then click tasks or their numbers to toggle marks. Outside multi-select, double-click a task to open it.

Press `?` on the board, task page, or another non-text surface for a searchable list of all shortcuts. Status and delete shortcuts use **Ctrl**; navigation uses bare keys. The website demo uses bare status keys because browsers reserve control chords.

## Board

| Action | Key |
| --- | --- |
| Select previous / next task | `↑` / `↓` or `k` / `j` |
| Enter / leave multi-select | `Shift+M` |
| Mark current task, then move in multi-select | `Shift+↑` / `Shift+↓` |
| Mark / unmark cursored task in multi-select | `Space` |
| Open task full screen | `Enter` |
| Peek / close peek below 110 columns | `→` / `←` |
| Move through wide views | `→` / `←` |
| Add task | `+` |
| Edit title | `ctrl+e` |
| Start open or ready task | `ctrl+s` |
| Set ready, the picked queue | `ctrl+n` |
| Set open, the inbox | `ctrl+o` |
| Mark done | `ctrl+d` |
| Toggle blocked / ready | `ctrl+b` |
| Toggle review / ready | `ctrl+r` |
| Delete, with a second press to confirm | `ctrl+x` or `ctrl+Delete` |
| Undo completion/deletion; restore an archived selection | `ctrl+u` |
| Archive / restore selected task | `ctrl+f` |
| Open / close done drawer | `d` |
| Expand / collapse its archived group | `g` |
| Desk / selected project / projects | `1` / `2` / `3` |
| Project picker | `p` |
| Project thread filter | `t` |
| Cross-project view selector | `v` on Projects |
| Search current board rows | `/` |
| Command palette | `:` |
| All shortcuts | `?` |
| Close a layer; quit at the full-board root | `Esc` |
| Quit outside text entry | `ctrl+q` |
| Quit from board view | `ctrl+c` |

`ctrl+q` quits from task views, Help, and pickers too, including the wide project preview. It does not quit while editing text, using board search, typing a quick-add line, or filtering the palette. Save or cancel any unsaved draft first. During save recovery, resolve the pending save instead.

`Esc` leaves multi-select and clears its marks first, then closes the current layer. At the full-board root on any tab, with multi-select inactive, it quits without confirmation rather than switching back to the desk. In either wide split, `Esc` closes the right column after any editor or overlay is dismissed. A further `Esc` at the full-board root quits. A project preview with unsaved work refuses to close. If narrowing the terminal hides the right column, the visible board/index is already at the root: `Esc` quits without an extra collapse, unless a parked draft blocks quitting.

`Shift+M` enters or leaves multi-select while the board owns input. While it is active, `Space`, shifted arrows, and a plain task click change marks instead of opening a task. Removing the last mark leaves the mode active. `Shift+M` again from the board, `Esc`, a task action, or a lens change leaves it and clears the set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface.

When tasks are marked, the status, delete, and archive shortcuts act on that set; without marks they act on the cursor. `Enter` and `ctrl+e` remain cursor-only. `ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Bulk block and review toggles send every target to blocked or review unless all targets already have that status, in which case they all return to ready. `ctrl+n` and `ctrl+o` can send done tasks directly to ready or open. One `ctrl+u` reverses an entire marked completion or deletion.

## Task page

These keys apply in **view mode**:

| Action | Key |
| --- | --- |
| Select steps and **+ step** | `Tab` / `Shift+Tab` |
| Activate first step, then move among steps | `↓`, then `↑` / `↓` |
| Toggle selected step | `Enter` |
| Add step | `ctrl+a` |
| Edit title or selected step | `ctrl+e` |
| Change task status | Board status shortcuts above |
| Mark/remove selected step; otherwise confirm/delete task | `ctrl+x` |
| All shortcuts | `?` |
| Close task | `Esc` |
| Quit the board | `ctrl+q` |

Click a step to select it. Click **+ step** to add. Field clicks become editable only after task editing starts.

## Editing

| Action | Key |
| --- | --- |
| Next / previous field | `Tab` / `Shift+Tab` |
| Save task edit | `Shift+Enter` |
| Cancel field | `Esc` or `ctrl+c` |
| Move from Title to Notes | `Enter` in Title |
| New line | `Enter` in Notes |
| Add another step | `ctrl+a` on the task page |
| Save new step and open next empty row | `Enter` in new step |
| Retain existing-step rename in session | `Enter` in existing step |
| Stage selected-step removal | `ctrl+x` in task edit |
| Open Scope picker / confirm selection | `Enter` |
| Cycle Scope | `Space` or `←` / `→` |
| Cycle Assignee / confirm | `Space` or `←` / `→`; `Enter` |
| Open / close selected Thread editor | `Enter` |
| Line start / end | `Home` / `End` |
| Word left / right | `ctrl+←` / `ctrl+→` |
| Delete backward / forward | `Backspace` / `Delete` |
| Move through wrapped notes | `↑` / `↓` |

`ctrl+e` moves to line end in text editors. `ctrl+a` moves to line start in quick-add; on the task page it adds a step instead. Paste preserves line breaks in Notes and converts them to spaces in single-line fields.

Editing keys take precedence over view-mode status shortcuts. In the step editor, `ctrl+d` and `ctrl+o` still address the task; `ctrl+x` removes the step being edited, staged until the task edit is saved; `ctrl+a` adds another step. Notes are reached with `ctrl+e`, then `Tab`. `Alt+Enter` does not save the task edit.

## Quick-add

| Action | Key |
| --- | --- |
| Save and close | `Enter` |
| Save and keep adding | `Shift+Enter` |
| Expand details | `Tab` |
| Cancel line / return from details | `Esc` |

In Herdr quick capture, `Shift+Enter` saves and closes the popup. `Esc` closes an active step editor or scope picker first; otherwise it discards the popup. During save recovery, it cancels the pending save. [Capture guide](/docs/capture/).

## Pickers

| Surface | Controls |
| --- | --- |
| Project picker | Arrows or `j`/`k` select; `Tab` or `←`/`→` switch tabs; `Enter` opens; `?` opens Help; `Esc` or `q` closes |
| Project archive | `ctrl+f` archives; on archived tab, `ctrl+f` or `ctrl+u` restores |
| Archived project view | `ctrl+u` restores; `Esc`, `p`, or `1`–`3` leaves |
| Thread/view selector | Type or paste to filter; arrows or `Tab` select; `Enter` chooses; `Esc` closes |
| Board search | Type or paste; `Backspace` edits; `Enter` pins; `Esc` clears and closes |
| Palette | Type to filter; arrows or `Tab` select; `Enter` runs; `Esc` closes. **set assignee** offers exact agent profiles and **unassigned** |
| Help | Type or paste to filter by key or action; arrows, page keys, or wheel scroll; `Esc` clears the search, then closes |
| Archived-project launch prompt | `y` restores; `n` or `Esc` keeps archived; `?` opens Help |
| Failed save | `r` or `Enter` retries; `c` or `Esc` cancels |

`j` and `k` are text in board search, thread/view filters, and Help search, not navigation. `?` and `/` are text in every input field, including Help search.

## Wide stage slider

At 110 usable columns or wider, arrows move through these views when you are not editing:

| View | `→` | `←` | `Esc` |
| --- | --- | --- | --- |
| Board | Split | No change | Quit at root |
| Split, board focused | Task with rail | Board | Board |
| Task with rail | Full screen | Split | Split |
| Full screen | No change | Task with rail | Return to the view remembered when opening |

`Enter` from the board opens full screen and remembers the previous view. `Tab` navigates task fields or steps; it does not switch wide views.

On **Projects Overview** at 110+ columns, clicking a row or moving the index selection opens the Split preview automatically. The project slider is shorter:

| Stage | `→` | `←` | `Esc` |
| --- | --- | --- | --- |
| Full board | Split preview | No change | Quit at root |
| Split, index focused | Rail | Full board | Full board |
| Rail, right board focused | Narrow board keys | Return to index | Return to index |

The right seat keeps its own project-board cursor, multi-select, marked set, and state. Its `→` / `←` are peek controls, and `Enter` opens a task page inside the column. `ctrl+d`, `d`, `g`, `+`, and the other board actions apply to the right seat while it is focused.

[Board views and mouse behavior](/docs/board/#wide-stage-slider).
