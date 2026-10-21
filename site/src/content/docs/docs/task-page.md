---
title: Task page
description: Read a task in full, edit its fields, and answer blocks and reviews.
---

The task page shows one task in full: notes, steps, metadata, any open block or review, and its paper trail. Edit the task here, and answer an agent's question or review. Every key on the page: [Task page keys](/docs/keys/#task-page).

## Open

- Select a task and press `Enter`, or double-click it. `Esc` or a click on **esc close** returns to the board.
- At 110 usable columns or wider, a click or `→` shows the task beside the board instead. [Wide layout](/docs/board/#wide-layout).
- In view mode, `ctrl+q` quits the whole board, not just the page.

## What the page shows

From the top: the title and status, a BLOCKED or REVIEW section while the task has one, the notes, the [steps](/docs/steps/), and the [paper trail](#paper-trail). After a [dispatch](/docs/dispatch/#dispatch), the page also shows the worktree, branch, `from <ref> @ <short sha>`, when it was dispatched, and whether the worktree was cleaned up.

Click the task's `T` number to copy it.

The footer reads `@assignee · ⎇ <base> · after T202 · before T205 · #thread · project`:

| Slot | Shows |
| --- | --- |
| `@assignee` | The [assignee](/docs/dispatch/#assign): an agent profile, you, or a person. An unassigned task shows `+ assign` when at least one [agent profile](/docs/storage/#configuration) exists. Click to pick |
| `⎇ <base>` | The base branch. Unset, it shows the repository default, such as `⎇ main (default)`, or `⎇ default` on the desk. Click to pick |
| `after T202` | The tasks this one [runs after](/docs/board/#after). Shown only when set, or during an edit. Click to pick |
| `before T205` | The tasks that run after this one. Read-only: it changes when you edit theirs. Click a number to open that task |
| `#thread · project` | The thread and the project, or desk |

Below it, the action bar lists the page's keys. In view mode it reads `ctrl+e edit`, the task's status actions, `esc close`, and `: commands`. During an edit it reads `shift+enter save`, `ctrl+a add step`, `esc cancel`, and `: commands`. Each one is clickable; `: commands` opens the command palette.

## Edit

The page opens in view mode. `ctrl+e` starts an edit at the title, or at the selected step. `Tab` then moves through the fields in this order, and `shift+Tab` goes back:

Title → Notes → steps → **+ step** → Assignee → Base → After → Thread → Scope → Title

The footer shows the same fields in the same order. Clicking a field during an edit moves there and keeps what you changed. A click does not start an edit.

| Field | Holds |
| --- | --- |
| Title | One line, required |
| Notes | Free text, shown as [Markdown](#notes-markdown) |
| Steps | The checklist. [Steps](/docs/steps/) |
| Assignee | An agent profile, you, or none. The task's current person stays on the list |
| Base | An existing local or remote branch that dispatch starts from, not a tag or commit. Unset uses the repository default. [Base branch](/docs/dispatch/#base-branch) |
| After | The tasks this one waits for |
| Thread | A [thread name](/docs/capture/#thread-names), optional |
| Scope | The project, or desk. Archived projects are not offered |

- `Enter` on Assignee, Base, After, or Scope opens its picker. `Space` or `←` / `→` cycles Assignee and Scope in place.
- To assign a new person, use `@` outside an edit and type their name.
- `Enter` or a second click on Thread opens its editor.
- The Base picker lists the repository default first, such as **default (main)**, then local and `origin/*` branches. It refreshes after a background fetch. **default** clears the base.
- The After picker is a search box over the other tasks. [Task picker](/docs/board/#task-picker).

### Without an edit

In view mode, three fields save at once:

- `@`, or a click on the footer's `@name` or `+ assign`, opens the assignee picker. During an edit, `@` opens the Assignee field's list instead.
- A click on the footer's `⎇` slot, or **set base** in the palette, opens the base picker.
- A click on the footer's `after` slot, or **set after (wait for)…** in the palette, opens the task picker.

## Save and cancel

- `shift+Enter` saves the whole edit: title, notes, scope, thread, assignee, base, after, and staged step renames and removals.
- `Esc` cancels the field you are in. A second `Esc` cancels the rest of the edit and restores staged step removals.
- New steps and step moves save at once, so cancelling does not undo them.
- The base is checked only when you change it or the project, so a deleted branch never blocks other edits.
- Save or cancel before you open another task.
- If another writer deleted the task, saving refuses. If a save fails, [retry or cancel](/docs/board/#save-failures).

## Blocked

A blocked task opens with a BLOCKED section between the title and the notes:

```
@claude blocked on you · 2h

The login test fails only on CI. Which way?

 1  Skip it on CI
 2  Fix the CI image

claude · 3m   Recommend 2; it is a one-line change.
you · 1m      Go with 2.

1-2 choose · r reply · ctrl+s reply and unblock · w waiting on
────────────────────────────────────────────────
```

- The top line says who raised the block, who it waits on, and for how long, like the peek: `@claude blocked on you · 2h`, `@claude blocked on T202 · 5m`, or `T202 is done · unblock it · 1h`.
- Below it come the why, the needs, the numbered options, and the replies. Each reply shows `you` or the agent profile, and its age. A deleted reply leaves a dim `deleted` stub.
- The last line lists only the keys that work at the cursor. A block that waits on another task or on something else shows `ctrl+b unblock` there.
- When the block closes, it moves to the paper trail.

### Reply box

From the page:

| Action | Key |
| --- | --- |
| Open the reply box | `r` |
| Open it starting from option N | `1`…`9`, or `Enter` on the option |
| Change who it [waits on](#waiting-on) | `w`, or `Enter` on the top line |
| Edit why, on, and needs (top line selected), or your selected reply | `ctrl+e` |
| Delete your selected reply | `ctrl+x` |

In the box:

| Action | Key |
| --- | --- |
| New line | `Enter` |
| Save the reply | `shift+Enter` |
| Save the reply and unblock | `ctrl+s` |
| Cancel | `Esc` |

- A saved reply waits. Unblocking sends every reply the agent has not received yet.
- Unblocking moves an unassigned task to ready. An assigned task starts: it dispatches, asks before relaunching a gone agent, or sends the replies to its running agent. [Dispatch](/docs/dispatch/#dispatch).
- `ctrl+s` outside the box, with nothing typed, unblocks the same way.
- `r` on a blocked board row opens the same box under the row.
- The box belongs to the block it opened on. If that block closes or is replaced elsewhere, saving refuses and keeps your draft.
- You can edit an agent's block, but not its replies.

## Review

A task in review opens with a REVIEW section in the same place:

```
@claude needs your review · 40m

Built the widget; empty input handled.

Check
 ✓ A works
 ○ B works on empty input
 ○ C works

you · 5m      B still fails on an empty string.

tab next field · enter cycle check · r feedback · ctrl+s send back · ctrl+d approve
w waiting on
───────────────────────────────────────────────────────────────────────────────────
```

- The top line says who asked for the review, who it is on, and for how long: `@claude needs your review · 40m`, or `@claude needs a review from @pi · 1m`.
- Below it come what was done, what is next, the checks, and your feedback. Checks are `✓` passed, `✗` failed, or `○` open.
- Passed checks fold into a dim `N passed ▸` line. A check you pass stays in place until the page opens again, or until you fold the line.
- The page opens on the first open check.
- Checks belong to the review and never become steps.
- When the round closes, it moves to the paper trail. The page shows no round number.

### Feedback box

From the page:

| Action | Key |
| --- | --- |
| Cycle the selected check: open → passed → failed → open | `Enter`, or click it |
| Show / fold the passed checks | `Enter` on the `N passed` line, or click it |
| Open the feedback box | `r` |
| Send back, with nothing typed | `ctrl+s` |
| Approve | `ctrl+d` |
| Change who it [waits on](#waiting-on) | `w`, or `Enter` on the top line |
| Edit done, checks, next, and on (top line selected), or your selected feedback | `ctrl+e` |
| Delete your selected feedback | `ctrl+x` |

In the box:

| Action | Key |
| --- | --- |
| New line | `Enter` |
| Save the feedback; the task stays in review | `shift+Enter` |
| Send back | `ctrl+s` |
| Approve | `ctrl+d` |
| Cancel | `Esc` |

- Send back sets the task to started and sends the round's feedback and failed checks to its running agent. With nothing typed, it needs a failed check.
- Approve marks the task done, through the [cleanup card](/docs/dispatch/#complete-and-clean-up) when its dispatch is live. Nothing is sent.
- Saved feedback waits for the send-back.
- The empty box reads `feedback to @claude…`. Like the reply box, it belongs to the round it opened on.
- Editing the checks keeps the state of every check whose text is unchanged.

## Waiting on

Hand an open block or review to someone else without touching the rest of it. On a blocked or review page, press `w` wherever the cursor is, or `Enter` on the top line. On the board, use **set waiting on…** in the palette for the selected task or a marked set.

1. The picker lists you, every agent profile, and the current choice, then **type a name…**. The current choice is selected and marked, such as `@sam · current`.
2. `←` / `→` or `↑` / `↓` move through the choices. Type to filter, or to give a name.
3. `Enter` saves. `Esc` changes nothing.

A typed name reads like an [assignee name](/docs/dispatch/#assignee-names): `you` and `me` are you, a profile's name is that agent, and anything else is kept as typed, such as `coordinator`. On a block, a task number such as `T202` makes it wait on that task.

- The block or review round stays the same: its text, checks, and replies are kept.
- The top line keeps its sentence and its age, and names the new one: `@claude blocked on you · 42s` becomes `@claude blocked on @pi · 48s`.
- The paper trail records one entry, such as `on: you → @pi`. One `ctrl+u` undoes it, for one task or a marked set.
- With tasks marked, a task with no open block or review refuses the whole set.
- From the CLI: [`tsk edit T12 --on`](/docs/cli/#edit).

## Paper trail

Below the steps, the paper trail lists who did what to the task, newest first. It opens collapsed as `PAPER TRAIL · 14 ▸`. Press `g`, or `Enter` on the heading, to expand it.

- An agent's entries name its profile, such as `@claude`. Yours read `you`, or your [configured name](/docs/storage/#board-defaults).
- The same entry by the same author within a minute groups into one: `3 steps checked`.
- Closed blocks and review rounds end in `▸`. `Enter` or a click expands one in place.
- The trail stays expanded while you stay on the task, and collapses when another task opens.

| Entry | Example |
| --- | --- |
| Created | `created · you 3d` |
| Status change | `open → started · @claude 2m`, `started · after T202` (its last prerequisite was done) |
| Assignee, base | `assigned @claude`, `unassigned`, `base ⎇ origin/dev`, `base cleared` |
| Waiting on | `on: you → @pi` |
| Dispatch | `dispatched tsk/t12-fix from origin/main @ 3333b79`, or `relaunched …` |
| Cleanup | `cleaned · worktree removed, branch kept` |
| Edits | `title edited`, `notes edited`, `after edited`: which fields, never the text |
| Steps | `step added`, `step checked` |
| Archive and delete | `archived`, `unarchived`, `deleted`, `restored` |
| A closed block | `blocked on you · need creds · 1 reply ▸` |
| A closed review round | `review round 2 · sent back · 1 failed ▸` |

An expanded block shows who blocked it and when, the why, the needs, the options, every reply, and who closed it. An expanded review round shows who set it, who it was on, what was done, each check, what is next, and the feedback.

## Notes markdown

Notes display basic Markdown. Editing shows the original text.

| Write | Display |
| --- | --- |
| `**bold**` | Bold |
| `*emphasis*` or `_emphasis_` | Underlined |
| `` `code` `` | Dim, with backticks |
| `# Heading` through `###### Heading` | Bold and underlined |
| `- item` or `* item` | List |
| Triple-backtick fenced block | Dim block, no inline styling |

- Unmatched asterisks and underscores inside words stay literal.
- `- [ ]` checkboxes are plain text. Use [steps](/docs/steps/) for a checklist.
- The board's peek uses the same formatting, dimmed.
