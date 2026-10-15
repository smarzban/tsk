---
title: Steps
description: Add, check, rename, reorder, and remove checklist steps.
---

Steps are a flat, ordered checklist. Checking every step does not complete the task.

## Add

1. Open a task.
2. Click **+ step**, or press `ctrl+a`.
3. Type the step and press `Enter`.

While a text field is open, `ctrl+a` is line start instead; leave the field first. With a step selected, `ctrl+a` opens the new row directly below it; otherwise the step goes at the end. Clicking **+ step** always adds at the end. `Enter` saves the step and opens another empty row below the one just added, so a burst of steps keeps its typed order. `Shift+Enter` saves it and finishes any active task edit. Click elsewhere to discard an empty new-step row.

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

## Reorder

Select a step and press `alt+↑` or `alt+↓` to move it one place. The selection follows the step. You can also press `:` and run **move step up** or **move step down**.

A moved step keeps its ID, short ID, and checked state. Moves save at once, including during task editing, and skip steps staged for removal; cancelling the task edit does not undo them.

`alt+↑` / `alt+↓` is the one changing shortcut without `ctrl`: macOS reserves `ctrl+arrows` for Mission Control. Your terminal must send `alt+↑` as one modified key (xterm's default); a terminal set to prefix Option or Alt with `Esc` delivers `Esc` and then the arrow, which closes the task page.

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
tsk steps T12 add "Capture a trace" --after <step-short-id>
tsk steps T12 toggle <step-short-id>
tsk steps T12 rename <step-short-id> "Reproduce on a slow connection"
tsk steps T12 move <step-short-id> --before <other-short-id>
tsk steps T12 move <step-short-id> --first
tsk steps T12 remove <step-short-id>
```

`add` takes an optional place and `move` requires one: `--before <step-short-id>`, `--after <step-short-id>`, `--first`, or `--last`. Placing a step relative to itself refuses with `same-step`.

A short ID is the shortest unique prefix of a step ID. Full UUIDs also work.

Read the task again with `tsk list T12 --json` before retrying an uncertain change. Repeating `add` adds another step; repeating `toggle` reverses it; repeating `remove` refuses after success. Repeating the same rename or move is safe.

Deleted tasks refuse step changes. [CLI errors and exit codes](/docs/cli/#exit-contract).
