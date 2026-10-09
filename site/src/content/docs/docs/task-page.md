---
title: Task page
description: Read and edit a task's title, notes, project, thread, assignee, and base branch.
---

Read notes and steps, then edit the task when you need to change it.

## Open

Double-click a task or select it and press `Enter`. A double-click follows the task you first clicked, even when the first click resizes the columns.

At 110 usable columns or wider, click a task or press `→` / `l` to open details beside the board, keeping board focus. Use `→` / `l` and `←` / `h` to move between [board and task views](/docs/board/#wide-stage-slider). From the board-focused split, `Esc` closes the details column, keeping any parked draft. Press `Enter` from the board for full screen; `Esc` or click **esc close** returns. In view mode, `ctrl+q` quits the whole board rather than closing the page; save or cancel unsaved edits first.

Click the task's `T` number to copy it. The page shows its status, notes, steps, assignee, base, project, thread, and its [paper trail](#paper-trail). After a [dispatch](/docs/board/#dispatch) it also shows the worktree, branch, `from <ref> @ <short sha>`, when it was dispatched, and whether the worktree has been cleaned up. Its footer is ordered `@assignee · ⎇ <base> · #thread · project`; when the task was created and last changed is on the paper trail. The base slot is always visible: an unset base shows the repository's default branch, for example `⎇ main (default)`, or `⎇ default` until that name is known. Long text and the metadata footer wrap, and footer controls remain clickable on each wrapped row.

## Edit

The page opens in view mode.

| Action | Key |
| --- | --- |
| Edit title, or the selected step | `ctrl+e` |
| Edit notes | `ctrl+e`, then `Tab` |
| Assign: pick an agent profile or **none** | `@` |
| Move through editable fields | `Tab` / `Shift+Tab`, after starting an edit |
| Save the task edit | `Shift+Enter` |
| Cancel the current field | `Esc` or `ctrl+c` |

Clicking Title, Notes, Scope, or Thread does not start an edit from view mode. Start editing first; then click the field you want.

In view mode the footer's assignee is a quick control: click `@name` to open the assignee picker. An unassigned task shows `+ assign` there when `config.toml` defines at least one profile; with no profiles the slot stays empty. Board rows and the peek never show `+ assign`. During an edit session, `@` and the click open the Assignee field's own list instead, so the edit's draft stays in charge.

Click the footer's `⎇` slot to choose the [base branch](/docs/board/#base-branch) a dispatch starts from. The same picker opens from palette **set base** or the **Base** field during editing. It lists the repository default first, for example **default (main)**, then local and `origin/*` branches, and refreshes in place after a background fetch. Type to filter, select a branch, or choose **default** to clear the base.

In view mode, `Tab` selects steps, **+ step**, then the paper trail's closed blocks and review rounds. It does not cycle task fields. Status shortcuts remain available on the task page; use `ctrl+e`, then `Tab` to reach Notes.

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
| Save the reply and unblock: to ready, or for an assigned task to started (a [start](/docs/board/#dispatch) that may dispatch or relaunch, or [sends the reply](/docs/board/#reply-to-a-running-agent) to its running agent) | `ctrl+s` |
| New line in the reply | `Enter` |
| Cancel the reply | `Esc` |
| Edit why, on, and needs (heading selected) | `ctrl+e` |
| Edit your selected reply | `ctrl+e` |
| Soft-delete your selected reply | `ctrl+x` |

The reply box opens under the last reply and wraps like the notes. It belongs to the block it opened on: if that block is closed or replaced elsewhere while you type, saving refuses and keeps your draft. Agent blocks can be edited like your own; an agent's replies cannot be edited or deleted. The same reply box opens under a blocked row on the board with `r`. A block on another task that is now done asks `T169 done, unblock? ctrl+b` under the heading.

## Review

A task in review opens with a REVIEW section in the same place. Its heading reads, for example, `REVIEW · round 2 · on you · @claude 40m ── PR #41 · r feedback`: the round, who it is on, who set it and when, and the pull request it names (the first `/pull/<n>` link or `PR #<n>` in done, next, the checks, or the notes). Below it come `done`, the checks, `next`, and your feedback as `└ you 5m  <text>`. Checks show `○` open or `✗` failed; passed checks fold into a dim `N passed ▸` line under them. The section shows only while the task is in review; closed rounds stay in `tsk list --json` under `past_reviews`. Checks are the review's own list and never become steps.

In view mode `Tab` walks the heading, the checks, the `N passed` line (and the passed checks when unfolded), and the feedback before the steps and **+ step**.

| Action | Key |
| --- | --- |
| Cycle a check: open → passed → failed | `Enter` on the check |
| Show or fold the passed checks (`▸` / `▾`) | `Enter` on the `N passed` line |
| Give feedback | `r` |
| Save the feedback; stay in review | `Shift+Enter` |
| Send back: started, and the feedback with the failed checks [goes to the running agent](/docs/board/#review-with-what-was-done) | `ctrl+s` |
| Approve: done (cleanup card when the dispatch is live); nothing is sent | `ctrl+d` |
| New line in the feedback | `Enter` |
| Cancel the feedback | `Esc` |
| Edit done, checks, next, and on (heading selected) | `ctrl+e` |
| Edit your selected feedback | `ctrl+e` |
| Soft-delete your selected feedback | `ctrl+x` |

The feedback box reads `feedback to @claude…` while empty and belongs to the round it opened on, like the reply box. Editing the checks keeps the state of any check whose text is unchanged.

## Paper trail

Below the steps, the PAPER TRAIL lists who did what to the task, newest first: `open → started · @claude 2m`, `assigned @claude · you 1h`, `created · you 3d`. An entry by an agent names its profile (`@claude`, from the `TSK_AGENT` its dispatch set); everything else is `you`.

| Entry | Example |
| --- | --- |
| Created | `created` |
| Status change | `review → started`, `review → done` |
| Assignee, base | `assigned @claude`, `unassigned`, `base ⎇ origin/dev`, `base cleared` |
| Dispatch | `dispatched tsk/t12-fix from origin/main @ 3333b79`, or `relaunched …` |
| Cleanup | `cleaned · worktree removed, branch kept` |
| Edits | `title edited`, `notes edited`, `title and notes edited` (which fields, never the text) |
| Steps | `step added`, `step checked` |
| Archive and delete | `archived`, `unarchived`, `deleted`, `restored` |
| A closed block | `blocked on you · need creds · 1 reply ▸` |
| A closed review round | `review round 2 · sent back · 1 failed ▸` |

The same entry repeated by the same author within a minute groups into one: `3 steps checked`. The open block or review round stays in its BLOCKED or REVIEW section above the notes; once it closes, it moves here.

Status changes, edits, and the other automatic entries are dim; closed blocks and review rounds are normal weight and end in `▸`.

| Action | Key |
| --- | --- |
| Show every entry, or only the latest five | `a`, or click `+ N earlier` |
| Expand a closed block or review round in place (`▾`), or fold it | `Enter` on it, or click it |
| Select the closed records | `Tab` past **+ step** |

An expanded block shows who blocked it and when, `why`, `needs`, the options, every reply, and who closed it. An expanded review round shows who set it, who it was on, `done`, each check with `○` open, `✓` passed, or `✗` failed, `next`, and the feedback.

Entries recorded before this version (store v9 and older) show what happened and when, without who or what changed: `status changed`, `edited`, `assignee changed`.

## Status and steps

In task view, status shortcuts act on the open task even when a step is selected. `ctrl+s` on an assigned task dispatches it to its agent; on a dispatched task whose agent is gone it asks before relaunching ([Relaunch](/docs/board/#relaunch)). To hand the task over, assign it with `@`, then press `ctrl+s`. `ctrl+d` on a live dispatch offers [cleanup](/docs/board/#complete-and-clean-up) as it completes the task. Multi-select and marks retained from the board are ignored and clear when an action runs. `ctrl+n` sets ready and `ctrl+o` sets open; `ctrl+s` starts an open or ready task. `Enter` toggles the selected step only.

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
