---
title: Keys
description: Every keyboard shortcut, by surface.
---

Every key tsk binds, by surface. Press `?` on the board or the task page for the same list, searchable. Clicks are on [Mouse](/docs/board/#mouse).

Status, edit, delete, and archive keys take `ctrl`. Bare letters move, open, and filter.

## Board

| Action | Key |
| --- | --- |
| Move up / down through tasks and group headers | `↑` / `↓` or `k` / `j` |
| Open the task page | `Enter` |
| Peek / close the peek, below 110 columns | `→` / `←` or `l` / `h` |
| Add a task | `+` |
| Edit title | `ctrl+e` |
| Start; an assigned task [dispatches](/docs/dispatch/#dispatch) to its agent | `ctrl+s` |
| Set ready | `ctrl+n` |
| Set open (back to the inbox) | `ctrl+o` |
| Mark done | `ctrl+d` |
| Toggle blocked / ready; blocking opens the block card | `ctrl+b` |
| Toggle review / ready; review opens the review card | `ctrl+r` |
| Delete, with a second press to confirm | `ctrl+x` or `ctrl+Delete` |
| Undo the last change ([what undoes](/docs/board/#delete-and-undo)) | `ctrl+u` |
| Archive / restore | `ctrl+f` |
| Assign to an agent, you, or a person, or **none** | `@` |
| Reply to a blocked row, or give feedback on a review row | `r` |
| Open / close the done drawer | `d` |
| Group by none, project, thread, or assignee | `g` |
| Sort by recent, project, thread, title, or assignee | `s` |
| Fold / unfold the cursor's group | `f` |
| Fold / unfold every group in view | `shift+F` |
| Open the desk / last project / Projects view | `1` / `2` / `3` |
| Pick a project | `p` |
| Filter by thread, project, or assignee | `t` |
| Choose a view on the Projects view | `v` |
| Search the board | `/` |
| Command palette | `:` |
| All keys | `?` |

- On the desk, `t` filters by thread, project, or assignee. On a project board it filters by thread or assignee.
- Bare `d`, `s`, and `f` open the drawer, sort, and fold. With `ctrl` they mark done, start, and archive.

### Group headers

Group sub-headers, the inbox, and archived in the done drawer are headers.

| Action | Key |
| --- | --- |
| Fold an open header / unfold a folded one | `←` / `→` or `h` / `l` |
| Open a sub-header's board: its project, the desk, or its thread's or assignee's view | `Enter` |
| Mark every task under a sub-header, unfolding it | `shift+M` |

- With nothing to fold or unfold, `←` and `→` keep their usual meaning: the peek, or the wide slider.
- `no thread`, `unassigned`, `desk` on the desk, the inbox, and archived open nothing.

### Multi-select

| Action | Key |
| --- | --- |
| Start / stop multi-select | `shift+M` |
| Mark / unmark the cursor task | `Space` |
| Mark the current task, then move | `shift+↑` / `shift+↓` |

- With tasks marked, status keys, `ctrl+x`, `ctrl+f`, `@`, and the palette's set commands act on the whole set. `Enter`, `ctrl+e`, and task-page keys stay on the cursor.
- `ctrl+s` starts each open or ready task in the set and skips the rest.
- `ctrl+b` and `ctrl+r` skip done tasks and send the rest to blocked or review through one card. If every one already has that status, they all return to ready.
- `ctrl+n` and `ctrl+o` also reopen done tasks.
- One `ctrl+u` reverses the whole set.
- Removing the last mark keeps multi-select on. `shift+M` on a task, `Esc`, any task action, or a change of view leaves it and clears the marks.
- In a text field, `shift+M` types `M`.

## Cards

`ctrl+b` opens the block card and `ctrl+r` the review card. One card covers a whole marked set.

| Action | Key |
| --- | --- |
| Next / previous field: why · on · needs, or done · check · next · on | `Tab` / `shift+Tab` |
| Cycle the on field: you · task · other, or you · agent · other | `←` / `→` |
| New check line, in the review card's check field | `shift+Enter` |
| Block, or set review; an empty card does it with no text | `Enter` |
| Cancel; the marks stay | `Esc` or `ctrl+c` |

### Decision cards

A decision card asks before a start or a cleanup. [What each card does](/docs/dispatch/#decision-cards).

| Card | Options |
| --- | --- |
| Start anyway: the task still waits on others | `y` start |
| Relaunch: the assigned agent is gone | `y` relaunch · `n` just start |
| Start a marked set | `y` start and launch every listed task |
| Cleanup: done with a live dispatch | `m` merge and tidy up · `y` tidy up · `n` complete only |
| Archived project at launch | `y` restore · `n` keep archived |

| Action | Key |
| --- | --- |
| Run an option | its letter |
| Move the `▸` cursor, then run it | `↑` / `↓`, then `Enter` |
| Scroll a long card | page keys or the wheel |
| Cancel, keeping any marks | `Esc`, or click `[x]` or `esc cancel` |
| Keep the project archived (archived-project card) | `Esc` or `n` |

- A card shows only the options that apply.
- An `Enter` pressed as the card opens is ignored.
- A click selects an option line; a double-click runs it.
- After cleanup starts, `Esc` hides the card while cleanup continues.

## Task page

In view mode, before you start an edit:

| Action | Key |
| --- | --- |
| Close the page | `Esc` |
| Move among steps (`↓` selects the first); otherwise scroll | `↑` / `↓` or `k` / `j` |
| Walk the top line, options or checks, replies, steps, **+ step**, and the paper trail | `Tab` / `shift+Tab` |
| Check / uncheck the selected step | `Enter` |
| Add a step below the selected one, else at the end | `ctrl+a` |
| Move the selected step up / down | `alt+↑` / `alt+↓` |
| Edit the title, or the selected step, block, review, or reply | `ctrl+e` |
| Mark the selected step for removal, then remove it | `ctrl+x` |
| Delete your selected reply; with nothing selected, delete the task | `ctrl+x` |
| Assign to an agent, you, or a person | `@` |
| Expand / collapse the paper trail | `g`, or `Enter` on its heading |
| Expand / fold a closed block or review round | `Enter` |
| Status, undo, and archive, on this task | the [board's](#board) `ctrl` keys |
| Command palette | `:` |
| All keys | `?` |

On a blocked page or a review:

| Action | Key |
| --- | --- |
| Reply to the block, or give feedback on the review | `r` |
| Reply with option N, prefilled | `1`…`9`, or `Enter` on the option |
| Unblock, or send the review back, with nothing typed | `ctrl+s` |
| Approve the review | `ctrl+d` |
| Change who it [waits on](/docs/task-page/#waiting-on), top line selected | `Enter` |
| Cycle the selected check: open → passed → failed → open | `Enter` |
| Show / fold the passed checks | `Enter` on the `N passed` line |

- Keys inside the reply and feedback box: [Blocked](/docs/task-page/#blocked) and [Review](/docs/task-page/#review).
- Digits pick options on the task page. `1` `2` `3` switch views only on the board.
- `alt+↑` / `alt+↓` also work while editing, with a step selected and no field open.
- Your terminal must send `alt+↑` as one modified key, as xterm does by default. A terminal that sends Option or Alt as a leading `Esc` closes the page instead.

## Text editing

| Action | Key |
| --- | --- |
| Save the task edit | `shift+Enter` |
| Cancel the field | `Esc` or `ctrl+c` |
| Next / previous field | `Tab` / `shift+Tab` |
| Title: move to Notes | `Enter` |
| Notes: new line | `Enter` |
| Notes: move through wrapped lines | `↑` / `↓` |
| New step: save it and open the next row | `Enter` |
| Existing step: keep the rename in this edit | `Enter` |
| Add a step from a selected step, **+ step**, the Scope row, or a selected Thread | `ctrl+a` |
| Stage removal of the selected step | `ctrl+x` |
| Open the Scope, Assignee, Base, or After picker; confirm the choice | `Enter` |
| Cycle Scope or Assignee without opening it | `Space` or `←` / `→` |
| Open / close the Thread editor | `Enter` |
| Line start / end | `Home` / `End` or `ctrl+a` / `ctrl+e` |
| Word left / right | `alt+←` / `alt+→`, `ctrl+←` / `ctrl+→`, or `alt+b` / `alt+f` |
| Delete a character backward / forward | `Backspace` / `Delete` |
| Delete a word backward | `alt+Backspace` or `ctrl+w` |
| Delete a word forward | `alt+Delete` or `alt+d` |
| Delete to line start / end | `ctrl+u` / `ctrl+k` |

In the step editor, `ctrl+d` and `ctrl+o` still act on the task, and `ctrl+x` stages the step's removal until the task edit is saved. In a new step's row, `ctrl+x` discards that row, as `Esc` does.

- Field order: [Edit](/docs/task-page/#edit).
- The word and line keys work in every text field: Title, Notes, steps, Thread, and quick-add.
- A word is a run of non-space characters, as with Option+arrow in macOS. In Notes, word moves cross line breaks and a word delete stops at one.
- In Notes, `ctrl+u` at a line start joins the line above, and `ctrl+k` at a line end joins the next line.
- Search, the palette, Help, and the pickers you type into have no cursor. There, `alt+Backspace` and `ctrl+w` delete the query's last word, and `ctrl+u` and `ctrl+k` do nothing.
- Outside text fields, `ctrl+u` is undo.
- Paste keeps line breaks in Notes and turns them into spaces in one-line fields.
- On macOS, `Cmd+←` and `Cmd+→` arrive as `ctrl+a` and `ctrl+e`, and `Cmd+Backspace` as `ctrl+u`.
- On macOS, Option reaches tsk only when the terminal sends it as Meta. Turn on Terminal.app's **Use Option as Meta key**, or set iTerm2's Option key to **Esc+**. If Option+B types `∫`, use `ctrl+←`, `ctrl+→`, and `ctrl+w`.
- On Windows, AltGr letters type as characters.

### If shift+Enter does not save

Some terminals send a plain line feed for `shift+Enter`, such as Ghostty with `keybind = shift+enter=text:\n`. tsk reads it as `ctrl+j` and changes nothing. In a form it shows `Shift+Enter sent Ctrl+J: check terminal keys`. Remove the binding, or map `shift+Enter` to a sequence that keeps the Shift modifier.

## Quick-add

| Action | Key |
| --- | --- |
| Save and close | `Enter` |
| Save and keep adding | `shift+Enter` |
| Open notes and details | `Tab` |
| Cancel the line, or return from details | `Esc` |

In the Herdr popup, `shift+Enter` saves and closes it. `Esc` or `ctrl+c` discards the popup, after first closing an open step editor or picker. [Capture](/docs/capture/).

## Pickers, search, palette, help

| Surface | Keys |
| --- | --- |
| Project picker (`p`) | `↑` / `↓` or `j` / `k` select · `Tab` or `←` / `→` switch tabs · `Enter` opens · `Esc` or `q` closes |
| Project picker, archive | `ctrl+f` archives the selected project · on the archived tab, `ctrl+f` or `ctrl+u` restores |
| Archived project's board | `ctrl+u` restores · `Esc`, `p`, or `1`–`3` leaves |
| Filter and view pickers (`t`, `v`) | type to filter · `↑` / `↓` select · `Tab` or `←` / `→` switch tabs, `shift+Tab` back · `Enter` chooses · `Esc` closes |
| Group, sort, assignee, and base pickers | type to filter · `↑` / `↓` or `Tab` select · `Enter` applies · `Esc` closes with no change |
| Assignee and waiting-on pickers, **type a name…** | the row shows what you typed, such as `@dana` · `Enter` applies it |
| Waiting-on picker | as the assignee picker, and `←` / `→` also move |
| After picker | as above, and `Space` ticks several · `Enter` applies the ticked tasks or the highlighted one · **none** clears |
| Board search (`/`) | type or paste · `Backspace` · `Enter` pins the query · `Esc` clears and closes |
| Palette (`:`) | type to filter · `↑` / `↓` or `Tab` / `shift+Tab` select · `Enter` runs · `Esc` closes |
| Help (`?`) | type to filter by key or action · `↑` / `↓`, page keys, or the wheel scroll · `Esc` clears the search, then closes |
| Failed save | `r` or `Enter` retries · `c` or `Esc` cancels |

- The base and after pickers have no key. Open them from the palette (**set base**, **set after (wait for)…**), the task page footer, or the edit form's Base and After fields.
- `h`, `j`, `k`, `l`, `?`, and `/` are text wherever you type: text fields, search, the palette, Help, and the pickers you type into.

## Wide slider

At 110 usable columns or wider, `→` / `l` and `←` / `h` move through the wide stage slider while you are not editing. [Wide layout](/docs/board/#wide-layout).

| View | `→` / `l` | `←` / `h` | `Esc` |
| --- | --- | --- | --- |
| Board | Split | No change | Quit at root |
| Split, board focused | Task with rail | Board | Board |
| Task with rail | Full screen | Split | Split |
| Full screen | No change | Task with rail | Return to the view remembered when opening |

`Enter` from the board opens full screen and remembers the view to return to. `Tab` never changes the view.

On the Projects view:

| Stage | `→` / `l` | `←` / `h` | `Esc` |
| --- | --- | --- | --- |
| Full board | Split preview | No change | Quit at root |
| Split, index focused | Rail | Full board | Full board |
| Rail, right board focused | Narrow board keys | Return to index | Return to index |

With the right board focused, board keys act on that board.

## Esc and quit

| Action | Key |
| --- | --- |
| Close the top layer: a picker, the palette, Help, search, the peek, the task page, or the right column | `Esc` |
| Quit at the board root, with no confirmation | `Esc` |
| Quit from any surface except text entry | `ctrl+q` |
| Quit from the board, the task page, quick-add details, Help, or the archived-project card | `ctrl+c` |
| Cancel a field, an edit, the block or review card, or the reply box | `Esc` or `ctrl+c` |

- With tasks marked, `Esc` first leaves multi-select and clears the marks. In a picker you type into, a card, or the reply box, it closes that first and keeps them.
- At the board root, `Esc` quits from any tab. It does not go back to the desk. With the cursor on the inbox or archived header, it does nothing.
- On an archived project's board, `Esc` returns to the desk.
- In a wide split, `Esc` closes the right column once no editor or overlay is open. If narrowing the terminal hides the right column, `Esc` quits at once.
- `ctrl+q` does not quit from a text field, board search, the quick-add line, the palette, the block or review card, or the reply box.
- Unsaved work blocks quitting and closing the Projects view's right board. Save or cancel the draft first. During a failed save, retry or cancel it first.
