---
title: Steps
description: Add, check, rename, and remove checklist steps.
---

Steps are a flat, ordered checklist. Checking every step does not complete the task.

## Add

1. Open a task.
2. Click **+ step**, or press `ctrl+a`.
3. Type the step and press `Enter`.

`Enter` saves the step and opens another empty row. `Shift+Enter` saves it and finishes any active task edit. Click elsewhere to discard an empty new-step row.

New steps save independently, including during task editing. Cancelling the task edit does not undo steps already added.

## Complete

Select a step with a click, `Tab`, or the arrows. Press `Enter` to check or uncheck it.

| Mark | Meaning |
| --- | --- |
| `▪` | Open |
| `✓` | Done |
| `✗` | Marked for removal |

The heading shows completed/total steps. In view mode, status shortcuts change the task, not the selected step.

## Rename

1. Select a step and press `ctrl+e`.
2. Edit its text.
3. Press `Shift+Enter` to save the task edit.

`Enter` retains the rename in the edit session without saving that session. Moving to another existing step retains the previous draft. `Esc` cancels the field; cancelling task editing restores staged changes.

A step cannot be saved empty. If you clear a step's text, saving the task edit reopens that step with `text required` on its row and saves nothing until you type text, remove the step, or cancel.

## Remove

| Mode | Action |
| --- | --- |
| Task view | Select a step; press `ctrl+x` to mark it, then again to remove it |
| Task editing | Select a step; `ctrl+x` stages removal; `Shift+Enter` saves |

Cancelling task editing restores staged removals.

## Navigate

- In view mode, `Tab` / `Shift+Tab` cycle steps and **+ step**.
- `↓` activates the first stored step; arrows then move between steps.
- During task editing, `Tab` includes the task fields.
- While adding a step, arrows scroll the page.
- Wheel and scrollbar remain available during editing.

## CLI

Use the task number and the step's short ID from direct JSON output:

```sh
tsk list T12 --json
tsk steps T12 add "Reproduce the timeout"
tsk steps T12 toggle <step-short-id>
tsk steps T12 rename <step-short-id> "Reproduce on a slow connection"
tsk steps T12 remove <step-short-id>
```

A short ID is the shortest unique prefix of a step ID. Full UUIDs also work.

Read the task again with `tsk list T12 --json` before retrying an uncertain change. Repeating `add` adds another step; repeating `toggle` reverses it; repeating `remove` refuses after success. Repeating the same rename is safe.

Deleted tasks refuse step changes. [CLI errors and exit codes](/docs/cli/#exit-contract).
