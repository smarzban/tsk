---
title: Task page
description: Read and edit a task's title, notes, project, thread, and assignee.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`. A double-click follows the task you first clicked, even when the first click resizes the columns.

At 110 usable columns or wider, click a task or press `→` to open details beside the board, keeping board focus. Use `→` and `←` to move between [board and task views](/docs/board/#wide-stage-slider). From the board-focused split, `Esc` closes the details column, keeping any parked draft. Press `Enter` from the board for full screen; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Click the task's `T` number to copy it. The page shows its status, notes, steps, assignee, project, thread, and dates. Its footer puts `@assignee` before `#thread`, then the project and created and updated dates. Long text wraps.

## Edit

The page opens in view mode.

| Action | Key |
| --- | --- |
| Edit title, or the selected step | `ctrl+e` |
| Edit notes | `ctrl+e`, then `Tab` |
| Move through editable fields | `Tab` / `Shift+Tab`, after starting an edit |
| Save the task edit | `Shift+Enter` |
| Cancel the current field | `Esc` or `ctrl+c` |

Clicking Title, Notes, Scope, Assignee, or Thread does not start an edit from view mode. Start editing first; then click the field you want.

In view mode, `Tab` selects steps and **+ step**. It does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

During task editing, the forward order is Title → Notes → steps → **+ step** → Thread → Scope → Assignee → Title. `Shift+Tab` reverses the ring. A field click moves the cursor and retains staged changes.

## Scope, thread, and assignee

**Scope** is the task's project or desk. **Thread** groups related work within a project. **Assignee** optionally names one configured agent profile.

| Field | Change it |
| --- | --- |
| Scope | Select it while editing, press `Enter`, choose a destination, then `Enter` again |
| Thread | Select it, then press `Enter` or click again to edit its name |
| Assignee | Select it, then use `Space` or arrows to cycle through exact profile names and **unassigned**; `Enter` confirms |

Scope also supports cycling with `Space`, `←`, or `→`. Archived projects are not offered.

Thread is optional and follows the [thread name rules](/docs/capture/#thread-names).

## Save

`Shift+Enter` saves the task's title, notes, scope, thread, assignee, and staged changes to existing steps.

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

In task view, status shortcuts act on the open task even when a step is selected. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+n` sets ready and `ctrl+o` sets open; `ctrl+s` starts an open or ready task. `Enter` toggles the selected step only.

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
