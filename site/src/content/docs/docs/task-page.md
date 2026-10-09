---
title: Task page
description: Read and edit a task's title, notes, project, thread, assignee, and base branch.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`. A double-click follows the task you first clicked, even when the first click resizes the columns.

At 110 usable columns or wider, click a task or press `→` / `l` to open details beside the board, keeping board focus. Use `→` / `l` and `←` / `h` to move between [board and task views](/docs/board/#wide-stage-slider). From the board-focused split, `Esc` closes the details column, keeping any parked draft. Press `Enter` from the board for full screen; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Click the task's `T` number to copy it. The page shows its status, notes, steps, assignee, base, project, thread, and dates. After a [dispatch](/docs/board/#dispatch) it also shows the worktree, branch, `from <ref> @ <short sha>`, when it was dispatched, and whether the worktree has been cleaned up. Its footer is ordered `@assignee · ⎇ <base> · #thread · project`, then created and updated dates. The base slot is always visible: an unset base shows the repository's default branch, for example `⎇ main (default)`, or `⎇ default` until that name is known. Long text and the metadata footer wrap, and footer controls remain clickable on each wrapped row.

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

In view mode the footer's assignee is a quick control: click `@name` to open the assignee picker. An unassigned task shows `+ assign` there when `config.toml` defines at least one profile; with no profiles the slot stays empty. Board rows and the peek never show `+ assign`. During an edit session, `@` and the click open the Assignee field's own list instead, so the edit's draft stays in charge.

Click the footer's `⎇` slot to choose the [base branch](/docs/board/#base-branch) a dispatch starts from. The same picker opens from palette **set base** or the **Base** field during editing. It lists the repository default first, for example **default (main)**, then local and `origin/*` branches, and refreshes in place after a background fetch. Type to filter, select a branch, or choose **default** to clear the base.

In view mode, `Tab` selects steps and **+ step**. It does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

During task editing, the forward order is Title → Notes → steps → **+ step** → Assignee → Base → Thread → Scope → Title. `Shift+Tab` reverses the ring. The footer follows the same left-to-right order: Assignee, Base, Thread, Scope. A field click moves the cursor and retains staged changes.

## Scope, thread, and assignee

**Scope** is the task's project or desk. **Thread** groups related work within a project. **Assignee** optionally names one configured agent profile. **Base** optionally names an existing local or remote branch for dispatch to start from, not a tag or commit. Without one, dispatch uses the repository's default branch (`origin/HEAD`).

| Field | Change it |
| --- | --- |
| Scope | Select it while editing, press `Enter`, choose a destination, then `Enter` again |
| Thread | Select it, then press `Enter` or click again to edit its name |
| Assignee | Select it, press `Enter`, choose an exact profile name or **none**, then press `Enter` again |
| Base | Select it, press `Enter`, choose a branch or **default**, then press `Enter` again |

Scope and Assignee also support cycling without opening their lists with `Space`, `←`, or `→`. Archived projects are not offered.

Thread is optional and follows the [thread name rules](/docs/capture/#thread-names).

## Save

`Shift+Enter` saves the task's title, notes, scope, thread, assignee, base, and staged changes to existing steps. The base is checked only when you change it or the project, so a deleted base branch never blocks other edits.

| While editing | `Enter` does this |
| --- | --- |
| Title | Move to Notes |
| Notes | Insert a line |
| Existing step | Keep the rename in the current edit session |
| New step | Save that step and open another empty row |

New steps save independently. Cancelling the task edit does not remove new steps already saved. [Step editing details](/docs/steps/).

Press `Esc` to discard changes to the current field. Other staged changes remain until you save or cancel the task edit. Cancelling the task edit also restores removed steps. Save or cancel before selecting a different task.

If another writer deletes the task, saving refuses. If a save fails, use [retry or cancel](/docs/board/#save-failures).

## Blocked

A blocked task's page opens with a BLOCKED section between the title and the notes, closed by a full-width rule. Its heading reads, for example, `BLOCKED · on you · @claude 1h ──── r reply`: who or what the block waits on, who blocked it and when, and `edited` once the block changed. Below it come `why`, `needs`, the suggested options as `○` rows, and the replies as `└ <author> <age>  <text>`. A deleted reply leaves a dim `deleted` stub. The section shows only while the task is blocked; the closed block stays in `tsk list --json` under `past_blocks`.

In view mode `Tab` walks the heading, the options, and the replies before the steps and **+ step**.

| Action | Key |
| --- | --- |
| Reply | `r` |
| Reply with an option, prefilled | `Enter` on the option |
| Save the reply | `Shift+Enter` |
| Save the reply and unblock to ready | `ctrl+s` |
| New line in the reply | `Enter` |
| Cancel the reply | `Esc` |
| Edit why, on, and needs (heading selected) | `ctrl+e` |
| Edit your selected reply | `ctrl+e` |
| Soft-delete your selected reply | `ctrl+x` |

The reply box opens under the last reply and wraps like the notes. It belongs to the block it opened on: if that block is closed or replaced elsewhere while you type, saving refuses and keeps your draft. Agent blocks can be edited like your own; an agent's replies cannot be edited or deleted. A block on another task that is now done asks `T169 done, unblock? ctrl+b` under the heading.

## Status and steps

In task view, status shortcuts act on the open task even when a step is selected. `ctrl+g` dispatches that task to its assigned agent; on an existing dispatch, the first press asks and the second relaunches. `ctrl+d` on a live dispatch offers [cleanup](/docs/board/#complete-and-clean-up) as it completes the task. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+n` sets ready and `ctrl+o` sets open; `ctrl+s` starts an open or ready task. `Enter` toggles the selected step only.

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
