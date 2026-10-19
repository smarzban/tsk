---
title: Steps
description: Add, check, rename, reorder, and remove a task's checklist steps.
---

Steps are a task's flat, ordered checklist, on the [task page](/docs/task-page/). Checking every step does not complete the task. Step keys: [Task page keys](/docs/keys/#task-page).

## Add

1. Open the task.
2. Press `ctrl+a`, or click **+ step**.
3. Type the step and press `Enter`. An empty row opens for the next one.

- With a step selected, `ctrl+a` adds below it; otherwise the step goes at the end. **+ step** always adds at the end.
- `shift+Enter` saves the step and any open task edit.
- Click elsewhere to discard an empty row.
- In a text field, `ctrl+a` moves to the line start. Leave the field first.

## Check

Select a step with `Tab`, the arrows, or a click, then press `Enter` to check or uncheck it.

| Mark | Meaning |
| --- | --- |
| `▪` | Open |
| `✓` | Done |
| `✗` | Marked for removal |

The steps heading shows done and total.

## Rename

1. Select the step and press `ctrl+e`.
2. Edit its text.
3. Press `shift+Enter` to save the task edit.

`Enter` keeps the rename in the edit without saving it. A step cannot be empty. Saving a cleared step shows `text required` on its row and saves nothing until you type text, remove the step, or cancel.

## Reorder

Select a step and press `alt+↑` or `alt+↓` to move it one place, or run **move step up** or **move step down** from the palette. The step keeps its checked state, and the selection follows it. [Terminal setup for `alt+↑`](/docs/keys/#task-page).

## Remove

- In view mode, the first `ctrl+x` marks the selected step `✗`. The second removes it.
- While editing, `ctrl+x` hides the step until `shift+Enter` saves the removal. Cancelling the edit restores it.

## Saving and undo

- New steps and moves save at once, even during a task edit. Cancelling the edit keeps them.
- Each check, uncheck, new step, removal, or move is one `ctrl+u` [undo](/docs/board/#delete-and-undo).
- The step changes saved by one task edit undo together, without its title or notes.
- Agents and scripts change steps with [`tsk steps`](/docs/cli/#steps).
