---
title: Task page
description: Read and edit a task's title, notes, project, thread, assignee, and base branch.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`. A double-click follows the task you first clicked, even when the first click resizes the columns.

At 110 usable columns or wider, click a task or press `→` / `l` to open details beside the board, keeping board focus. Use `→` / `l` and `←` / `h` to move between [board and task views](/docs/board/#wide-stage-slider). From the board-focused split, `Esc` closes the details column, keeping any parked draft. Press `Enter` from the board for full screen; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Click the task's `T` number to copy it. The page shows its status, notes, steps, assignee, base, project, thread, and dates. After a dispatch it also shows the worktree, branch, `from <recorded ref> @ <short sha>`, relative dispatch time, and whether the worktree was removed. Its footer is ordered `@assignee · ⎇ <base> · #thread · project`, then created and updated dates. The base slot is always visible: an unset base shows the repository's default branch, for example `⎇ main (default)`. The name is cached without blocking rendering; until it is known, the slot shows `⎇ default`. Long text and the metadata footer wrap, and footer controls remain clickable on each wrapped row.

## Edit

The page opens in view mode.

| Action | Key |
| --- | --- |
| Edit title, or the selected step | `ctrl+e` |
| Edit notes | `ctrl+e`, then `Tab` |
| Dispatch to the assigned agent; press twice to relaunch. Unassigned: pick an agent, then dispatch | `ctrl+g` |
| Assign: pick an agent profile or **none** | `@` |
| Move through editable fields | `Tab` / `Shift+Tab`, after starting an edit |
| Save the task edit | `Shift+Enter` |
| Cancel the current field | `Esc` or `ctrl+c` |

Clicking Title, Notes, Scope, or Thread does not start an edit from view mode. Start editing first; then click the field you want.

In view mode the footer's assignee is a quick control: click `@name` to open the assignee picker. An unassigned task shows `+ assign` there when `agents.toml` defines at least one profile; with no profiles the slot stays empty. Board rows and the peek never show `+ assign`. During an edit session, `@` and the click open the Assignee field's own list instead, so the edit's draft stays in charge.

Click the footer's `⎇` slot to choose a base branch. The same picker opens from palette **set base**, or the **Base** field during editing. It offers **default (main)** (for a repository whose default is `main`) first, then deduplicated local and `origin/*` branches. The picker opens immediately with a disabled **loading branches** row while a bounded background fetch fills the list. Type to filter, select a branch, or choose **default** to clear the explicit base. There is no dedicated base key, and dispatch never prompts for a base.

In view mode, `Tab` selects steps and **+ step**. It does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

During task editing, the forward order is Title → Notes → steps → **+ step** → Assignee → Base → Thread → Scope → Title. `Shift+Tab` reverses the ring. The footer follows the same left-to-right order: Assignee, Base, Thread, Scope. A field click moves the cursor and retains staged changes.

## Scope, thread, and assignee

**Scope** is the task's project or desk. **Thread** groups related work within a project. **Assignee** optionally names one configured agent profile. **Base** optionally selects an existing local or remote branch for dispatch, not a tag or commit. Without an explicit base, dispatch uses this task repository's remote default (`origin/HEAD`), regardless of where the board runs.

| Field | Change it |
| --- | --- |
| Scope | Select it while editing, press `Enter`, choose a destination, then `Enter` again |
| Thread | Select it, then press `Enter` or click again to edit its name |
| Assignee | Select it, press `Enter`, choose an exact profile name or **none**, then press `Enter` again |
| Base | Select it, press `Enter`, choose a branch or **default**, then press `Enter` again |

Scope and Assignee also support cycling without opening their lists with `Space`, `←`, or `→`. Archived projects are not offered.

Thread is optional and follows the [thread name rules](/docs/capture/#thread-names).

## Save

`Shift+Enter` saves the task's title, notes, scope, thread, assignee, base, and staged changes to existing steps. It revalidates the branch only when the base or project changes, so a pruned base does not block unrelated edits.

| While editing | `Enter` does this |
| --- | --- |
| Title | Move to Notes |
| Notes | Insert a line |
| Existing step | Keep the rename in the current edit session |
| New step | Save that step and open another empty row |

New steps save independently. Cancelling the task edit does not remove new steps already saved. [Step editing details](/docs/steps/).

Press `Esc` to discard changes to the current field. Other staged changes remain until you save or cancel the task edit. Cancelling the task edit also restores removed steps. Save or cancel before selecting a different task.

If another writer deletes the task, saving refuses. If a save fails, use [retry or cancel](/docs/board/#save-failures).

## Status and steps

In task view, status shortcuts act on the open task even when a step is selected. `ctrl+g` dispatches that task to its assigned agent; on an existing dispatch, the first press asks and the second relaunches. `ctrl+d` offers safe worktree cleanup before completing a live dispatch. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+n` sets ready and `ctrl+o` sets open; `ctrl+s` starts an open or ready task. `Enter` toggles the selected step only.

While editing a field, use its edit keys. Save or cancel to return to the task's status actions.

Scroll with the wheel or scrollbar while editing. Typing or moving the Notes cursor brings it back into view.

## Notes markdown

Notes display basic Markdown. Editing shows the original text.

| Write | Display |
| --- | --- |
| `**bold**` | Bold |
| `*emphasis*` or `_emphasis_` | Underlined |
| `` `code` `` | Dim, with backticks |
| `# Heading` through `###### Heading` | Bold and underlined |
| `- item` or `* item` | List |
| Triple-backtick fenced block | Dim block; inline styling disabled |

Unmatched asterisks and underscores inside words stay literal. Markdown checkboxes such as `- [ ]` are text; use [steps](/docs/steps/) for an interactive checklist.

Peeks use the same formatting, dimmed. Narrow-pane peeks show up to five wrapped lines; open the task for the full notes.
