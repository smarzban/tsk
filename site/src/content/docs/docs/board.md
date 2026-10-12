---
title: Board
description: Find tasks, switch projects, and keep work moving.
---

Click to navigate, or use the keyboard. The footer shows actions for the cursor or your marked task set; those actions are clickable.

## Navigate

| View | Key | Contents |
| --- | --- | --- |
| **desk** | `1` | Blocked/review and started tasks across all live projects; ready and open tasks from your desk |
| **selected project** | `2` | Tasks in the selected project |
| **projects** | `3` | Project overview |

Launching inside a Git repository opens that project. Outside Git, tsk opens **desk** and puts the current directory in the middle project tab.

In Herdr, `prefix+t` opens or focuses a board in the current workspace. If one is already open in another tab there, Herdr switches to that tab and focuses its pane, preserving its view and edits. Otherwise, it opens a board beside your work. Boards in other workspaces stay untouched; all boards share the same tasks.

The middle tab remembers your selected project. Press `2` to open it; if none is selected, `2` opens the project picker.

## Quit or go back

Press `ctrl+q` to quit from the board or a task view, including Help, pickers, and the wide project preview. While a text editor, board search, quick-add line, or palette query owns input, `ctrl+q` keeps its editing behavior instead. Unsaved drafts must be saved or cancelled before quitting, including drafts parked in another view while a project preview has focus. Save recovery must be resolved first; quit attempts leave its failure message intact.

`Esc` leaves multi-select and clears its marked tasks before it closes the current layer; an open filter, view, assignee, or base picker closes first and keeps the marks. At the full-board root, with multi-select inactive and no page, peek, popup, search, or header selection left to dismiss, it quits without confirmation. This applies on every tab, not just the desk. In either wide split, `Esc` closes the right column once any editor or overlay is dismissed, returning to the full-width board or projects index. Task drafts stay parked; a project preview with unsaved work refuses to close. After a narrow resize hides the right column, `Esc` treats the visible board/index as the root, with the same unsaved-draft protection. In quick capture, `Esc` closes the popup.

## Projects

Press `p` to choose a project, or open **projects** for an overview.

- Click a project to select it; double-click or press `Enter` to open it.
- Check the footer for the selected project's full path.

Projects Overview counts work in **NEEDS YOU**, **IN MOTION**, **ON DECK** (ready and open, including the inbox), and **DONE**. Archived tasks and archived projects are excluded. A dim `·` means zero. `here` marks the launch project. At 100 columns or wider, the overview also lists threads.

At **110 usable columns** or wider, Overview can preview the cursored project beside the index:

| Stage | What you see |
| --- | --- |
| Full board | Full-width projects index |
| Split | Index and a dim project preview; the index keeps focus |
| Rail | Narrow index and a live project board; the right column owns input |

The right preview names the selected project in its top row, in the space used by navigation tabs on the index. Clicking or moving the index selection opens Split automatically; press `→` or `l` to move from Split to Rail, and `←` or `h` to walk back. The project tab never opens a full-screen task stage. `Enter` on an index row still opens that project in tab 2, and choosing a thread with `v` drops back to the full-width index. The right column has its own selection, drawer, filters, task page, quick-add, and status actions; `Esc` from its board returns focus to the index.

## Search

Press `/` on any board tab to search the rows that tab currently shows. On the Projects overview it matches project names or paths. On the desk, a project board, or a cross-project thread or assignee view it matches tasks by title, notes, step text, thread, assignee (with or without its `@`), or task number such as `T12`, case-insensitively. Every whitespace-separated word must match somewhere in the same task.

Typing or pasting filters immediately. Empty sections disappear and section counts show only matches. Search combines with a project board's thread and assignee filter; the done drawer is searched only while it is open. In the wide Projects preview, `/` searches the right project board when that seat has focus.

Press `Enter` to pin the query and return to board keys. Navigation, task actions, the done drawer, and a second `Enter` then act on the filtered rows; the query remains in the footer. Press `Esc` while typing to clear and close search, or press it once on a pinned board to clear the query before normal `Esc` behavior resumes. Changing tab, project, or Projects view also clears it.

## Threads and assignees filter

A thread groups related tasks, such as `release` or `login-fix`. A thread name can span projects and the desk, so it also works as a cross-repo initiative. An [assignee](#assign) names the agent a task is for. On a project board both narrow it through one picker with two tabs, `threads · @assignees`; the desk's filter adds a projects tab.

| Where | Action |
| --- | --- |
| Desk | Press `t` or click the filter to narrow by thread, project, and assignee |
| Project board | Press `t` or click the filter to open **Filter**; choose a thread, an assignee, or both |
| Projects overview | Press `v` or click **Overview** to view one thread or one assignee across projects |

On a project board the threads tab lists `all`, the project's threads, then **Without a thread**; the `@assignees` tab lists `all`, every profile in `config.toml` as `@name`, any name still on tasks whose profile was removed, then `unassigned`. The two choices combine: `#release` plus `@claude` shows only release tasks assigned to claude, and the filter control reads `#release @claude`. `all` clears only its own tab's choice, and `✓` marks the active choice in each tab. The choice is for this session only, clears when you switch projects, and search (`/`) narrows inside it.

The desk filter has three tabs, **threads**, **projects**, and **@assignees**; press `Tab` or `→` for the next tab and `Shift+Tab` or `←` for the previous one (they wrap), or click a tab. The `@assignees` tab lists the same choices as a project board's. Each tab sets its own choice and they combine, so `herdr-tsk #release @claude` shows only that project's `release` tasks assigned to claude. The filter narrows every desk section, the done drawer included, and the control on the tab row names it (`all` when nothing is chosen). On a narrow desk with no filter, the `all` control is left off rather than taking a row of its own.

On the Projects overview the threads tab offers **Overview** and each thread; the `@assignees` tab offers one `@name` view per assignee (there is no `unassigned` view). An `@name` view lists that assignee's tasks from every project and the desk, grouped by status like a thread view, with done tasks in the drawer. One view applies at a time.

The picker opens on the threads tab (an `@name` overview view reopens on its own tab). `Tab`, `←`/`→`, or a click switches tabs and clears the typed filter. Type to filter the current tab, use arrows to select, `Enter` to choose, and `Esc` to close without a change. `j` and `k` are search text in these selectors.

Assign threads when [capturing](/docs/capture/#title-tokens) or [editing a task](/docs/task-page/#scope-thread-and-assignee).

## Agents

Assign a task to an agent, dispatch it into its own Git worktree and Herdr workspace, then clean up when you complete it. Agents are profiles you define in [`config.toml`](/docs/storage/#agent-profiles). Dispatch needs Herdr; on Windows it is a [preview](#dispatch-on-windows-preview). Assigning works everywhere.

### Assign

The assignee names one profile in `config.toml`. Press `@` on the board, peek, or task page, or choose **set assignee** in the palette, to open the assignee picker: every profile, then **none**. The current assignee is preselected, or the first profile when there is none. Type to filter, `↑`/`↓` to move, `Enter` to apply, and `Esc` to close without a change. With tasks marked, one choice assigns the whole set and one `ctrl+u` undoes it. With no profile defined, `@` says so and opens nothing. On the Projects overview `@` does nothing.

You can also assign with `!a name` in [quick-add](/docs/capture/#title-tokens), the task page's [Assignee field](/docs/task-page/#scope-thread-and-assignee) or footer, or `tsk add --assignee` and `tsk edit --assignee`. Rows carry the `#thread · project` label after the title; the peek footer reads `@assignee · ⎇ <base> · #thread · project`, leaving out what is unset and a default base.

### Dispatch

Starting an assigned task dispatches it: `ctrl+s`, the palette's **set status: started**, or [`tsk status N started`](/docs/cli/#status). tsk creates a branch and Git worktree from the task's [base](#base-branch), opens a Herdr workspace there, and runs the profile's command. It then records the dispatch on the task and sets it to started, in one save. If the launch fails, the task records no dispatch, keeps its status, and the status row says why. `ctrl+u` right after undoes the start only: the status goes back and the status row says `start undone · T12 back to ready · @claude kept running`; the agent and its worktree stay.

What a start does depends on the task:

| Task | Start |
| --- | --- |
| Unassigned | Sets started; to work on a task yourself, leave it unassigned |
| Assigned, never dispatched | Dispatches it, which sets started |
| Dispatched, agent still running | Sets started; Herdr is asked whether the agent is still in its pane, and when it cannot say, the start is plain |
| Dispatched, agent gone | Asks first: [relaunch](#relaunch) |
| Assigned, outside Herdr or on the desk | Sets started; the status row says why nothing launched |
| Done or archived | Sets started; never launches |

Names come from the task number and title. For T12 `Fix login timeout`:

| Name | Value |
| --- | --- |
| Branch | `tsk/t12-fix-login-timeout` |
| Worktree directory | `tsk-t12-fix-login-timeout` |
| Herdr workspace | `T12 Fix login timeout` |
| Herdr agent | `t12-claude`, for assignee `claude` |

The slug lowercases the title, joins its words (letters and digits in any script) with `-`, and keeps whole words within 30 characters; a longer title ends the workspace label with `…`. The worktree directory keeps only ASCII letters and digits, so `Café` checks out in `tsk-t12-caf`. When a branch, worktree, or directory of that name already exists, tsk appends `-2`, `-3`, and so on. tsk names the agent once Herdr detects it, so `herdr agent get t12-claude` finds it; if Herdr does not detect one within 30 seconds, the agent stays unnamed. Neither the board nor `tsk dispatch` waits for this.

A started task with a live dispatch shows `◉` instead of `●`.

To hand a task to an agent, assign it with `@`, then press `ctrl+s`.

Dispatch needs Herdr, a task in a project that is a Git repository, and an assignee with a profile. Where it cannot work at all, outside Herdr or on a desk task, starting an assigned task is a plain start and the status row says why: `started · no launch: not in Herdr` or `started · no launch: desk task has no repository`. Other launch refusals (an unknown agent, a base that cannot be resolved, a Git or Herdr failure) leave the task unstarted.

### Dispatch on Windows (preview)

Herdr on Windows is a preview, and so is dispatch there.

- A profile's `command` must find an `.exe` (`claude`) or a PowerShell script (`pi`, `codex`). Batch files (`.cmd`, `.bat`) are refused, because cmd.exe could run task text as commands; the pane says so and nothing starts.
- A PowerShell script is supported as a shim that hands its arguments to a program, like the `.ps1` files npm installs. tsk escapes each argument for that program's command line, so it arrives exactly as written. A script that reads `$args` itself sees the escaping instead: `"` arrives as `\"` and an empty argument as `""`.
- The [state directory](/docs/storage/) must not contain `$`, `` ` ``, `%`, `"`, or `!`. Dispatch refuses one that does with `unsafe-state-dir`.
- If Git's `core.longpaths` is off, dispatch says so once; turn it on with `git config --global core.longpaths true`.

Cleanup on Windows can stop with one of these, and keeps the task's dispatch until a later cleanup succeeds:

| Message | What to do |
| --- | --- |
| files in use | Close what is running in the worktree, then clean again. |
| path too long | Run `git config --global core.longpaths true`, then clean again. |
| removal timed out | Clean again. |
| partly removed | Follow the message, which says what is left and how to finish. Every commit is on the task's branch. |
| uncommitted changes | Commit or discard the changes in the worktree, then clean again. |

### Base branch

Dispatch starts from the task's base branch when one is set, otherwise from the repository's default branch (`origin/HEAD`), never from whatever your board or CLI has checked out. A local branch that tracks a remote starts from the remote branch. tsk fetches the remote first unless it was fetched in the last minute (the [fetch window](/docs/storage/#fetch-window)); offline, it starts from the local copy and says so. The task page then shows where the dispatch started, as `from <ref> @ <short sha>`.

Set a base with **set base** in the palette, the task page's `⎇` footer or **Base** field, `!b branch` in quick-add, or `tsk add --base` and `tsk edit --base`. The branch picker lists the repository default first, for example **default (main)**, then local and `origin/*` branches. It opens at once on the branches already on disk and shows **refreshing…** while a background fetch updates the list in place, keeping your selection; if the fetch fails it reads **offline, showing cached branches**. **default** clears the base. With tasks marked, one choice sets the whole set as one undo. A base must be an existing branch in the task's repository, not a tag or commit. There is no base key, and starting never asks for one.

### Start a marked set

With tasks marked, `ctrl+s` (or **set status: started** in the palette) starts the set as usual when no task in it would launch; assigned tasks that cannot launch here are named on the status row, such as `started · no launch: not in Herdr (T3, T4)`. When any would, it opens one card first. The card lists each task it will launch with its assignee and base (`from dispatch`, or `from default (main)`), then under **start only** the tasks it only starts (unassigned, already dispatched: relaunching stays one task at a time, an assigned desk task with `started · no launch: desk task has no repository`, and with **set status: started** done or archived tasks, listed as a status correction), then under **not started** each assigned task it cannot launch and why: unknown agent, or not a project in a git repo. Those stay unstarted. Rows show `checking…` while repositories are checked in the background. When nothing can launch, the card is titled `Start N tasks?` and `y` only starts. Below the rows, one option line says what `y` does (`▸ y  dispatch T4 · start T7 without launching · T9 not started`), then a dim `esc cancel`.

`y` starts the start-only tasks in one save, then launches every listed task in the background, each exactly as a single dispatch. The board stays usable while the status row counts `dispatching 2/3…`. Each task is saved as its launch lands, a failed launch does not stop the others, and the status row ends with what launched and what failed. A status you change while launches land is kept (`T12 kept done (changed meanwhile)`). If a save fails and you cancel [save recovery](#save-failures), the status row names the workspace each unrecorded agent is running in. `Esc` or `[x]` closes the card and keeps the marks; `y` clears them. Further dispatches and quitting wait until the launches finish. One `ctrl+u` undoes the start-only tasks; launches from a marked set are not undoable. `y` rechecks the start-only tasks first: one finished or archived after the card opened is left alone, and one assigned meanwhile is not started (`not started: T14 assigned since the card opened`), since the card never showed its launch.

### Relaunch

Starting a dispatched task whose agent is gone (its Herdr workspace was closed, its pane has no agent, or its worktree was cleaned up) opens a card: `T12 · relaunch @claude?`.

```
@claude is no longer running.

▸ y  relaunch @claude in the same worktree · start T12
  n  start T12 · don't relaunch @claude

worktree  ~/.herdr/worktrees/app/tsk-t12-fix

                                        esc cancel
```

| Key | Result |
| --- | --- |
| `y` | Relaunch the agent in the recorded worktree and start the task |
| `n` | Just start the task |
| `Esc` | Change nothing |

In the palette, **dispatch again** relaunches the cursor task without asking. Both act on the cursor task only. A relaunch keeps the recorded base and names, even if the task's base changed since. If the Herdr workspace was closed, it opens a new one on the kept worktree. After a cleanup it recreates the worktree, reopening the kept branch or recreating a deleted one from its original starting commit.

### Decision cards

The relaunch, start-anyway, and marked-set start cards, the archived-project launch prompt, and the [cleanup card](#complete-and-clean-up) all work the same way. Each option is one line starting with its key and saying what it does, the likely or safe one in bold, with one blank line between sections and a dim way out at the bottom right instead of a footer key list. `↑`/`↓` move the `▸` cursor (stopping at the ends) and `Enter` runs it; the cursor starts on the bold option: `y` on the relaunch card, since starting a dispatched task most likely means its agent, and `n` on the launch prompt, which changes nothing on disk. An `Enter` within a moment of a card opening is ignored, so a double tap of the key that opened it cannot run anything. Letter keys still run their option directly. A click on an option line selects it, a double click runs it, and a click on the way out cancels.

### Complete and clean up

With no marks, `ctrl+d` on a task with a live dispatch asks before completing. The card, titled `Done T12`, says where the task stands, then lists its options, one line each, starting with its key and saying first what happens to the pull request or branch, then what happens locally. Below them are the worktree and branch paths, and a dim `esc cancel` at the bottom right. For example, with an open pull request:

```text
PR #193 is still open:

▸ m  squash-merge PR #193 to main · mark done · remove worktree, branch and agent pane
  y  don't merge · mark done · remove worktree and agent pane · keep the branch and the PR open
  n  don't merge · mark done · keep everything

worktree  ~/.herdr/worktrees/…
branch    tsk/t12-…

                                                                               esc cancel
```

| Where the task stands | Options |
| --- | --- |
| `PR #193 is still open:` | `m` merges it ([below](#merge-the-pull-request)), `y`, `n` |
| `PR #194 merged into main ✓`, or `Merged into main ✓` | `y` mark done · remove worktree, branch and agent pane; `n` mark done · keep everything |
| `Not merged into main · no pull request` (or `PR #193 closed`, `PR #193 targets dev, not main`, `gh is not logged in (gh auth login)`, `no GitHub remote`; without `gh`, `squash-merged? delete it by hand`) | `y` mark done · remove worktree and agent pane · keep the branch (not merged); `n` |
| `PR #193 merged, but the branch has newer commits: the branch is kept`, or `PR #193 merged into dev, not main: the branch is kept` | `y` keeps the branch; `n` |
| `Can't tell if it's merged: offline (base fetch failed) · the branch is kept` (or `the check timed out`, `base no longer available`, `no recorded base`, `branch checked out in another worktree`) | `y` keeps the branch; `n` |
| `3 uncommitted files in the worktree (src/ui/input.rs, src/app.rs, …): commit or discard them to merge or tidy up` | only `n` don't merge · mark done · keep everything |
| `Checking merge into main…` | `y` marks done, removes the worktree, and deletes the branch if the check confirms the merge; `n` |

The likely option is bold: `m` with an open pull request, `y` when the branch is already merged. A line that would not apply is left out: no agent pane, no mention of one. `↑`/`↓` move the `▸` cursor between options (stopping at the ends) and `Enter` runs it; the cursor starts on the bold option. An `Enter` within a moment of the card opening is ignored, so a double tap of the key that opened it cannot merge by accident. `m`, `y` and `n` still run their option directly, and `Esc` or `[x]` changes nothing. With the mouse, a click on an option line (any of its wrapped rows) moves the cursor there, a double click runs it, and a click on `esc cancel` cancels. While the pull request lookup runs, the card reads `checking pull request…` and shows no options yet. When the card is taller than the frame, it drops its blank lines first, then scrolls with page keys or the wheel.

`y` before the merged check finishes still marks the task done at once, and the cleanup starts when the check lands. After `y` the board stays usable and the card reports progress (`Cleaning up…`), then the outcome: `Done T12 · cleaned`, `Done T12 · cleaned · branch kept (not merged)`, or `Cleanup refused: files in use`, closed with `esc close`. `Esc` hides the card while cleanup continues; the status row shows `cleaning 1 of 1…`, then the outcome, such as `done T12 · cleaned` or `done T12 · cleaned · branch kept (not merged)`. A clean outcome clears after a few seconds; one that kept something stays until your next action. Closing the project preview that started a cleanup does not stop it. While a cleanup runs, `ctrl+d` refuses with `cleanup still running; try again when it finishes`, and quitting shows `finishing cleanup…` and waits for it, up to 30 seconds.

A worktree that is already gone is marked cleaned without a card. Done tasks, archived tasks, and archived project views complete without one.

With tasks marked, `ctrl+d` on a set that holds live dispatches opens one card for the set, titled for example `Done 4 tasks`. Each task gets one row saying only where it stands:

```text
T7   PR #193 is still open
T8   PR #190 merged into main ✓
T9   not merged · no pull request
T10  3 uncommitted files · src/ui/input.rs, src/app.rs, …
```

Tasks without a dispatch read `no dispatch`, a dispatch whose worktree is already gone `can't tell if merged: worktree already gone` (its branch is kept), and a branch another worktree has checked out `can't tell if merged: branch checked out in another worktree`. Each option line names exactly the tasks it touches:

```text
▸ m  squash-merge PR #193 (T7) · mark T7 and T8 done · remove their worktrees, branches and agent panes · T9 and T10 unchanged
  y  don't merge · mark T7, T8 and T9 done · remove their worktrees and agent panes · delete T8's branch · keep T7's and T9's branches · T10 unchanged
  n  mark all 4 done · keep everything
```

| Key | Open PR | Already merged | Not merged | Uncommitted |
| --- | --- | --- | --- | --- |
| `m` | merge, then done and full tidy | done and full tidy | unchanged | unchanged |
| `y` | done and tidy, keep the branch and the PR open | done and full tidy | done and tidy, keep the branch | unchanged |
| `n` | done | done | done | done |

Past six tasks a line counts them (`mark 7 tasks done`) and the rows stay the detail. `m` never marks an unmerged task done; under `y` and `n`, tasks without a dispatch are marked done. The card opens scrolled to the selected option, and scrolls back to it after the frame is resized. `↑`/`↓` move between options and keep the selected one in view; page keys and the wheel scroll the rows. `Enter` never runs an option out of sight: when page keys scrolled the selected option away, the first `Enter` scrolls back to it and a second runs it. While the set works, each row says its step (`Merging PR #193 with gh…`, `Cleaning 1 of 2…`); then one line per task: `merged PR #193 (squash) · done · cleaned · remote branch deleted by GitHub`, `done · cleaned`, `unchanged · not merged`, `unchanged · 3 uncommitted files`, or `merge failed: <reason> · unchanged`. One failure never stops the others. The status row counts it all, naming the tasks that kept something: `merged 1 · done 2 · cleaned 2 · unchanged 2`. `Esc` before a choice keeps the marks. When `m` merges nothing (every merge failed, and no task was already merged), the card lists why for each task and closes with `esc close`; the marks stay for a retry and the status row reads `nothing merged · marks kept · T7 merge failed: <reason> · T9 not merged`. A marked set with no live dispatch completes at once.

Either way, completion is one save and one undo step, saved before any worktree is touched. One `ctrl+u` reverts every task the card marked done; it cannot un-merge a pull request or restore a removed worktree.

Cleanup closes the dispatch's Herdr workspace, removes its recorded worktree, and deletes its branch only when the branch is merged into the recorded base. It fetches the base first, so a merge on GitHub counts without a local pull. A squash or rebase merge leaves no Git ancestry, so when the branch reads unmerged and the [GitHub CLI](https://cli.github.com/) (`gh`) is installed and logged in, cleanup asks GitHub about the branch's pull request: it deletes the branch only when that pull request is merged into the recorded base **and** its head is exactly the local branch tip. A pull request merged into another branch keeps the branch (`PR merged into dev, not origin/main; delete by hand`). A merged pull request whose head differs keeps the branch (`PR merged but the branch has newer commits; delete by hand`). Without `gh`, nothing changes. It keeps the branch, and says why, when the branch is not merged, when the merge cannot be confirmed because the fetch failed, when the base no longer exists, or when the branch is checked out elsewhere or changed during cleanup. It never touches a worktree with uncommitted changes, the project root, or a path that does not match the recorded worktree, and it leaves alone a task relaunched after the card opened (`kept: changed since the card opened`). The task keeps its dispatch record, marked cleaned. From a script, use [`tsk clean`](/docs/cli/#clean).

### Merge the pull request

When `gh` is installed and logged in, the cleanup card also looks up each task's pull request by its branch name, in the background, and says where it stands (see the table above). Without `gh` on PATH the card never mentions a pull request. When a listed task with a clean worktree has an open pull request into its recorded base, the card offers `m`, worded after the merge method: `merge`, `squash-merge`, or `rebase-merge PR #193 to main`.

`m` runs `gh pr merge <branch> --<method> --match-head-commit <tip>` in the background, with the method from [`[merge]` in `config.toml`](/docs/storage/#merge-method) (a merge commit by default). It merges only when GitHub's pull request head is exactly the local branch tip. It never passes `--admin`, `--auto`, or `--delete-branch`, so branch protection, required checks, and reviews apply as on GitHub. While it runs the card reads `Merging PR #193 with gh…` and takes no choice; quitting (`ctrl+q`) waits, bounded by gh's own time limits, until the merge lands, done is saved, and its cleanup runs, then exits. A successful `gh` is not taken as proof: tsk asks GitHub again and goes on only when the pull request shows merged. A merge queue or auto-merge that only accepted the request leaves the task as it was, and the card says `PR #193 is queued, not merged yet · nothing changed`. On success the task is marked done and cleaned exactly as with `y`; the cleanup fetches the base again, and GitHub's answer confirms a squash or rebase merge, so the branch is deleted. tsk never deletes the remote branch; after the merge it looks again (briefly, since GitHub deletes a merged branch a moment later when the repository says so) and reports it truthfully: `Done T12 · merged PR #193 (squash) · cleaned · remote branch deleted by GitHub`, or `· remote branch kept`. When the merge fails (checks failing, conflicts, protection, review required), nothing changes: the task keeps its status, nothing is cleaned, and the card says `Merge failed: <gh's reason> · nothing changed` and closes with `esc close`. An invalid `[merge]` setting merges nothing and says why on the card.

On a marked set, `m` merges every listed task that has an open pull request into its base, one after another; one failure never stops the others. The tasks whose pull request merged, and those already merged, are marked done (one save, one undo step) and fully tidied. Every other task (its merge failed, no pull request, uncommitted changes, no dispatch) stays exactly as it was, and the finished card says why for each. From a script, use [`tsk status T12 done --merge`](/docs/cli/#status).

## Group and sort

Every task row carries a dim `#thread · project` label (`desk` for a desk task), at every width, including the narrow board rail beside an open task page. It follows the title two spaces after its last word; when it does not fit on the title's last line, it starts the next line at the title's indent and wraps like the title. Neither the title nor the label is ever truncated. A label leaves out what the view already says: a project board never names its own project, a thread filter drops the thread and a project filter the project, grouping drops its dimension inside the sections that actually group (the done drawer keeps it), and rows under **ON DECK · desk** never repeat `desk`. Archived rows carry the same label as done rows.

| Key | Action |
| --- | --- |
| `g` | Group by: **none**, **project**, **thread**, or **assignee** (a project board never offers project, the thread view never thread, an `@name` view never assignee) |
| `s` | Sort by: **recent**, **project**, **thread**, **title**, or **assignee** (with the same exceptions) |
| `f` | Fold or unfold the group the cursor is in: its sub-group, the inbox, or the archived group |
| `Shift+F` | Fold every group in view, or unfold them all when all are folded |

Grouping adds dim sub-headers such as `▾ herdr-tsk · 3`, `▾ #release · 2`, `▾ no thread · 1`, `▾ desk · 1`, `▾ @claude · 3`, or `▾ unassigned · 2` inside **NEEDS YOU**, **IN MOTION**, **ON DECK**, and the **inbox**, whose sub-groups sit under its own fold. The section header keeps its total; each sub-header counts its own rows. Groups are ordered hottest first: the group whose newest row changed status most recently leads. The done drawer and the archived group never group, and grouping the desk by project leaves **ON DECK · desk** and its inbox alone because they only hold desk tasks.

Sorting reorders rows inside each group, or inside each section while ungrouped (the inbox included). **recent** is each section's normal order; the others sort by project name (desk last), thread name (unthreaded last), title, or assignee (unassigned last), and ties keep the normal order. `N` notices still lead.

Every task list's header shows `group: none ▾` and `sort: recent ▾` beside its filter (`all ▾`, a project, or a thread); click one to open its picker. The desk and project boards paint them on the tab row, the cross-project thread view beside its View control, and the wide Projects preview on its own header row beside the project name. When they do not fit, the pair shortens to `g:none ▾ s:recent ▾`. On narrow frames where even that does not fit beside the tabs, each control still at its default (`group: none`, `sort: recent`, the desk's unfiltered `all`) drops first, so a default never costs a row or pushes out a choice you made. If what you changed still does not fit, it moves to its own row under the tabs (the defaults rejoin it there when that row has room); `g`, `s`, and `t` work either way. The Projects preview has one header row, so there its name shortens first, then defaults drop, then changed controls from the left, the filter last. The Projects overview lists projects, not tasks, so it shows only its View control. A fresh board starts ungrouped and sorted by recent, or from the desk and project board defaults in [`config.toml`](/docs/storage/#board-defaults). The desk, project boards, the cross-project thread view, and an `@name` view each keep their own choice for the session, and the wide Projects preview keeps its own (starting from the project board default). Session changes are never written back to `config.toml`.

Every sub-header is a cursor stop, open (`▾`) or folded (`▸`); a folded group shows only its sub-header. With the cursor on a sub-header, `←` or `h` folds it and `→` or `l` unfolds it, leaving the cursor on the header; when there is nothing to fold or unfold, the key keeps its usual meaning (the peek, or the wide stage slider). The inbox and archived headings fold the same way. `Enter` on a sub-header opens what it names: a project sub-header opens that project's board, `desk` opens the desk (on the desk itself it changes nothing), a `#thread` sub-header opens that thread's cross-project view, and an `@name` sub-header that assignee's `@name` view, as choosing them with `v` does. `no thread` and `unassigned` open nothing. With the cursor on a header, the status row offers `enter open` (when it opens something) and `← fold` or `→ unfold`. A click on any header (a sub-header, the inbox, or archived) only selects it. A double-click opens a sub-header like `Enter`; on a header that opens nothing (`no thread`, `unassigned`, `desk` on the desk, the inbox, archived) it folds or unfolds the header instead. Otherwise fold with `←`/`→`, `f`, or `Shift+F`. `Shift+M` on a sub-header unfolds it and marks every task in that group as one set; inside multi-select it adds the group to the marked set. Folding, or changing the grouping, sorting, or filter, clears multi-select marks.

## Status

| Section | Status |
| --- | --- |
| **NEEDS YOU** | `blocked` on you, `review` on you |
| **IN MOTION** | `started`, `blocked` on another task or on something else, and `review` on an agent or on something else |
| **ON DECK** | `ready`, `open` |
| ↳ **inbox** | `open` |
| Done drawer | `done` |

Status glyphs are `◌` open, `○` ready, `●` started, `■` blocked, `▲` review, and `✓` done. A started task with a live [dispatch](#dispatch) shows `◉` instead of `●`; any other status, or a cleaned dispatch, shows the normal glyph. A task blocked on another task or on something else shows `□`; a review on an agent or on something else shows `△`.

A row with something to say carries a dim **live line** under it, in the peek's `└─` style, wrapped and never cut. It is part of its row: selection never stops on it and a click on it acts on the task. When the row's title wraps, each wrapped title line carries a dim `│` in the `└` column, so the title and its live line (or its open peek) read as one block; the title keeps its wrap width. The right edge of a row stays empty.

| Row | Live line |
| --- | --- |
| Blocked on you | `@claude blocked on you · 2h`, or `blocked on you · 2h` when you set it yourself |
| Review on you | `@claude needs your review · 40m` / `needs your review · 40m` |
| Blocking task done | `T169 is done · unblock it · 1h` |
| Blocking task deleted | `T169 was deleted · unblock it · 1h` |
| Blocked elsewhere | `waiting on T169` / `waiting on design team` |
| Review handed elsewhere | `@pi reviewing` |
| Ready or open, running [after](#after) unfinished tasks | `after T202 (started), T205 (open)` |

The age is how long the row has been in that state: since the block or review round opened, or since the blocking task finished. A started task, a done or archived task, a [notice](#notices), and a waiting task whose prerequisites are all done have no live line. When the blocking task is done, the blocked task returns to NEEDS YOU; tsk never changes the status itself.

The [peek](#mouse) replaces the live line, and closing it brings the line back. Everything in a peek is dim. A blocked task's peek opens with the live line's text, then the why, then `Decide: <option> · <option>` when there are options or else the needs, then the footer. A review's peek opens with the live line's text, then what was done, then every check on one wrapped line (`○` open, `✓` passed, `✗` failed), then the footer. Neither shows the notes; `Enter` opens the task page for them. A notice, or a blocked or review task with no recorded block or round, peeks its notes as usual.

### Block with a reason

`ctrl+b` on a task that is not blocked opens the block card:

| Field | Value |
| --- | --- |
| why | What stops the work (optional) |
| on | `‹ you · task · other ›`, cycled with `←` / `→`; `task` takes a number such as `T12`, `other` any text |
| needs | What would unblock it (optional) |

`Tab` moves between fields, `Enter` blocks, and `Esc` cancels. `Enter` on an empty card blocks at once with no reason. A task number must be another task on the board; the card says so otherwise. With tasks marked, one card blocks the whole set as one save and one `ctrl+u` undo step, and `Esc` keeps the marks. `ctrl+b` on a blocked task unblocks it to ready, as do every other status change: leaving `blocked` closes its block. Palette **set status: blocked** opens the same card for the targets not yet blocked. If the save fails and you cancel, the card and the marks stay, ready to try again.

Agents block with a question from [the CLI](/docs/cli/#status) (`--why`, `--needs`, `--option`, `--on`). Answer on the [task page](/docs/task-page/#blocked), or press `r` on a blocked row, in NEEDS YOU or IN MOTION at any width, to reply right under it and its live line: the block's why and needs stay above the reply box, and its keys are the task page's (`Shift+Enter` saves, `ctrl+s` saves and unblocks, `Enter` breaks the line, `Esc` cancels). With tasks marked, `r` answers the cursor row only. On a row that is not blocked the status row says `not blocked`. The list scrolls to keep the caret in view. If the task leaves the list while you type (unblocked or finished elsewhere), the box moves below the list under the task's name, keeps your draft, and says `this block was closed or replaced elsewhere; reply kept`.

### Review with what was done

`ctrl+r` on a task that is not in review opens the review card:

| Field | Value |
| --- | --- |
| done | What was done (optional) |
| check | One thing to verify per line; `Shift+Enter` starts the next check |
| next | What comes after (optional) |
| on | `‹ you · agent · other ›`, cycled with `←` / `→`; `agent` takes a profile from [`config.toml`](/docs/storage/#agent-profiles), `other` any text |

`Tab` moves between fields, `Enter` puts the work up for review, and `Esc` cancels. `Enter` on an empty card sets review at once. With tasks marked, one card covers the whole set as one save and one `ctrl+u` undo step; tasks already in review keep their round. `ctrl+r` on a task in review returns it to ready. Palette **set status: review** opens the same card over the targets not yet in review.

Each review is a **round**. Agents set one from [the CLI](/docs/cli/#status) (`--done`, `--check`, `--next`, `--on`); running it again while the task is in review updates the same round. On the [task page](/docs/task-page/#review) you mark each check passed or failed and give feedback with `r`, which also opens the feedback box under a review row on the board. In the feedback box:

| Key | Action |
| --- | --- |
| `Shift+Enter` | Save the feedback; the task stays in review |
| `ctrl+s` | Send back: save the feedback and start the task. A running dispatched agent gets every feedback entry of the round, oldest first (the ones saved earlier with `Shift+Enter`, then this one), and the failed checks as one message: `[tsk T12 sent back] <feedback> Failed checks: <check>; <check>`. Two or more entries are numbered one per line (`1. …`, `2. …`), with the failed checks on the last line |
| `ctrl+d` | Approve: save the feedback and mark the task done, through the [cleanup card](#complete-and-clean-up) when its dispatch is live. Nothing is sent |
| `Esc` | Cancel |

Sending back takes the same route as a reply's `ctrl+s` ([below](#reply-to-a-running-agent)), with `feedback` in place of `reply` on the status row, except that an unassigned task also goes to started. An empty box sends back only when a check failed (`type feedback or fail a check first` otherwise). The round closes into history at the send-back, recorded as sent back; the agent's next review opens round N+1. A round closed by done is recorded as approved. A relaunched or newly dispatched agent finds the feedback and failed checks in the last `past_reviews` entry of `tsk list --json`.

### Reply to a running agent

When `ctrl+s` in a reply box starts an assigned task whose dispatched agent is still running, tsk sends that agent every reply you left on the block, oldest first, once the start is saved, as one message: `[tsk T12 unblocked] <your reply>`. That includes replies saved earlier with `Shift+Enter`; two or more are numbered one per line (`1. …`, `2. …`). Deleted replies are skipped, edited ones send their current text, and the agent's own replies are not echoed back. A reply [`tsk reply --send`](/docs/cli/#reply) already delivered is skipped too, unless you edited it since. A block or round closes when it is unblocked or sent back, so each reply goes out once. With no reply of yours on the block, or `ctrl+s` on the task page with no box open and nothing saved, it only unblocks and sends `[tsk T12 unblocked]` alone. Multi-line replies arrive intact.

`ctrl+b` on a blocked task (or palette **set status: ready** or **started**) unblocks the same way: once the change is saved, the running agent gets the block's replies, and the status row reads `ready · reply sent to @claude`. Unblocking to ready with no reply of yours sends nothing. Palette **set status: started** on a review sends it back the same way. A marked set does this for every blocked or in-review row, including the start-only rows of the [bulk start card](#dispatch), and one status row counts what went out: `started · sent back to 2 agents with 3 feedback entries · 2 replies sent to 1 agent · 1 not sent, kept on its task`. [`tsk status T12 started`](/docs/cli/#start) and `tsk status T12 ready` do the same from the CLI.

tsk sends only to the agent it dispatched: the pane's live agent must carry the name dispatch gave it (`t12-claude`), checked when the start is routed and again just before the send. Another agent in that pane, or one Herdr never named (naming can fail, for example when the name is taken), gets nothing. The status row says how it went:

| Status row | Meaning |
| --- | --- |
| `started · reply sent to @claude` | Herdr took the message; a busy agent gets it after its current turn. With more than one entry it counts them: `started · 3 replies sent to @claude`, `started · 2 feedback entries sent to @claude` |
| `started · @claude is waiting on a prompt; reply kept on the task` | The agent is on a permission prompt or question, so nothing was sent |
| `started · could not reach @claude; reply kept on the task` | Herdr failed or could not say whether the agent runs |
| `started · reply not sent: @claude is not in its pane; reply kept on the task` | The pane holds another agent, or an unnamed one |
| `started · reply not sent: not in Herdr` | The board runs outside Herdr, so it cannot reach the agent |

The task stays started in every case and nothing relaunches or retries; the reply is on the task either way. `Shift+Enter` (save only) never sends by itself: what it saved goes out with the unblock or send-back that closes the block or round. The other starts are unchanged: a task never dispatched dispatches, and its prompt points the agent at the answers; a gone agent asks before [relaunching](#relaunch); an unassigned task unblocks to ready. If the save fails, nothing is sent until [save recovery](#save-failures) retries it.

On your desk, **ON DECK** contains only desk tasks. On a project board, it contains that project's ready and open tasks. Ready tasks are the picked queue; open tasks are the untriaged inbox below it. Ready tasks sort by oldest pick first, open tasks by oldest capture first, and notice tasks lead within each group. The **inbox** group starts expanded; press `←` or `h` on its heading to fold it and `→` or `l` to unfold it, or `f` with the cursor inside it (when the inbox is grouped, `f` folds the cursor's sub-group instead). Use the filter to narrow the tasks, and [group and sort](#group-and-sort) to arrange them.

Sections hold their order while you work: NEEDS YOU, IN MOTION, DONE, and the drawer's ARCHIVED group keep the most recent status change on top, while ON DECK lists ready and inbox backlogs oldest first. (`N` tasks lead each group until you clear them.) Editing a task or ticking a step never moves it; setting a status moves it to the top of its new section.

Move the cursor with `↑`/`↓` or `j`/`k`. Press `Shift+M` to enter multi-select. While it is active, press `Space` to toggle the cursored task, hold `Shift` with `↑`/`↓` to mark the current task before moving, or click a task to toggle it. Marked rows show `▪`; the cursor remains `▸`. Removing the last mark leaves the mode active. `Shift+M` again while the board owns input, a task action, `Esc`, or a view change such as folding a group or switching tabs, projects, threads, or the done drawer exits the mode and clears the session-only set. Text entry keeps `Shift+M` as a capital `M`; `Esc` leaves multi-select before cancelling that surface, except in a filter, view, assignee, or base picker, where it closes the picker first.

On a task-board list, `ctrl+s`, `ctrl+n`, `ctrl+o`, `ctrl+d`, `ctrl+b`, `ctrl+r`, `ctrl+x`, and `ctrl+f` act on the marked set when it is non-empty. With no marks they act on the cursor. `Enter`, `ctrl+e`, and actions from the task page always use only the cursor; `ctrl+s` on a marked set with assigned tasks opens the [start card](#start-a-marked-set).

| Key | Action |
| --- | --- |
| `ctrl+s` | Start an open or ready task; an assigned one [dispatches](#dispatch); a task that still waits [asks first](#after) |
| `ctrl+n` | Set ready, the picked on-deck queue |
| `ctrl+o` | Set open, the inbox |
| `ctrl+d` | Mark done; for a live dispatch, offer [cleanup](#complete-and-clean-up) |
| `ctrl+b` | Block with a reason (the [block card](#block-with-a-reason)); on a blocked task, return it to ready |
| `ctrl+r` | Review with what was done (the [review card](#review-with-what-was-done)); on a task in review, return it to ready |
| `r` | Reply to a blocked row, or give feedback on a review row, inline ([reply](#block-with-a-reason)); cursor only |

`ctrl+s` starts each eligible open or ready task and leaves started, blocked, and review tasks unchanged. Marked done tasks are skipped, and the status row names them: `started N · skipped M done (ctrl+n or ctrl+o reopens them)`. Bulk block and review toggles are all-or-nothing: if every target already has that status they all return to ready, otherwise they all move to that status (blocking through one block card and review through one review card; tasks already blocked keep their block, tasks in review their round). Other status verbs are absolute, so repeating the current status does nothing. Done tasks can be sent directly to ready or open.

Agents can set any status with [the CLI](/docs/cli/#status). Task status does not change automatically when steps are checked or an agent stops; the one automatic start is a ready task whose last [prerequisite](#after) is done.

## After

A task can run **after** other tasks: it waits while any of them is not done. Task numbers are board-wide, so a prerequisite can be in any project or on the desk. A waiting ready or open row's live line reads `after T202 (started)`, gone once every prerequisite is done; nothing moves sections. Its peek lists each prerequisite with its status, `after T202 · started`, and a prerequisite's peek lists what waits on it, `before T203, T204`. The [task page](/docs/task-page/#after) footer shows the same.

Set it from the task page's **After** field, with **set after…** in the palette (the cursor task, or every marked task), with `!w T202` in [quick-add](/docs/capture/#title-tokens), or with [`tsk edit --after`](/docs/cli/#edit). The task picker lists the tasks that are not done, the task's own project first and then the others with their project. Type to filter by number or title, `Space` ticks several, `Enter` applies the ticked tasks (or the highlighted one when nothing is ticked), and **none** clears the list. A task cannot run after itself, and a choice that would close a loop is refused, for example `T202 already runs after T205`.

With tasks marked in order, **chain in order** makes each marked task run after the one marked before it, on top of what it already runs after. One save, one `ctrl+u`.

When the last prerequisite becomes **done** (review does not count), each waiting task in **ready** starts in the same save as the done, the way `ctrl+s` starts it; an assigned task never dispatched then [dispatches](#dispatch) right after that save, which records its agent. What a done releases is judged against the store as saved, so a prerequisite completed elsewhere (another board, the CLI, an agent) counts. While a marked-set dispatch is still landing, a released assigned task stays ready instead (`T203 waits: a dispatch is running; ctrl+s starts it`). The status row says `T203 started · T202 done`, and the started task's paper trail reads `started · after T202`. This happens from the board, the [cleanup card](#complete-and-clean-up), and [`tsk status N done`](/docs/cli/#status). Waiting tasks in open, blocked, or review do not move. A launch that fails never blocks the done: the task goes back to ready and the status row says why (`T203 back to ready: …`). If the launch ran but its record could not be saved, the status row says the agent is running and names its worktree; if the launch failed and the task could not be saved back to ready, it says `T203 is started but no agent launched; couldn't save it back to ready: …`. A task whose earlier agent is gone starts without a relaunch (`@claude gone, not relaunched`). `ctrl+u` on the done reverts the statuses only, `done undone · T202 back to started · T203 back to ready · @claude kept running`; a launched agent keeps running.

Starting a waiting task yourself asks first: `T203 runs after T202 (started).` under `Start anyway?`, then the option `y  start now, without waiting` and `esc cancel`. `y` (or `Enter`) starts it, `Esc` cancels. With tasks marked, the card lists each waiting one.

Deleting a task drops it from every other task's after list in the same save, and one `ctrl+u` puts the links back. A ready task left waiting on nothing does not start; the status row says `T203 no longer waits (T202 deleted)`. A task purged from trash leaves no links behind.

## Notices

Your first board open seeds four desk tasks with `N` ids (not `T`). They teach the board by being ordinary tasks: open, peek, change status, archive, or delete. `ctrl+d`, `ctrl+f`, and `ctrl+x` all dismiss a notice the same way; that starter task never re-seeds. After an upgrade, one `What's new in tsk` task can appear the same way. Details and the delivery record live under [storage](/docs/storage/#starter-guides-and-release-notes). Starter notices keep their seeded statuses so the tour order stays useful; a new release notice lands in NEEDS YOU as `review`, so it sits where you look first.

## Mouse

| Action | Result |
| --- | --- |
| Click a tab or selector | Change view or open its choices |
| Click the `group: …` or `sort: …` control | Open group by or sort by |
| Click a group sub-header, the inbox, or archived | Select it |
| Double-click a group sub-header | Open its project's board, the desk, or its thread's or assignee's view |
| Double-click a header that opens nothing (`no thread`, `desk` on the desk, the inbox, archived) | Fold or unfold it |
| Click a task outside multi-select | Peek in narrow panes; open or update details beside the board in wide panes |
| Click a task or its `T`/`N` number in multi-select | Move the cursor there and toggle its mark |
| Click the same task again in a narrow pane | Close its peek |
| Double-click a task | Open it full screen |
| Click its `T` or `N` number | Copy that id |
| Click a footer action | Run that action |
| Wheel or drag a scrollbar | Scroll |
| Drag across text | Select and copy on release |

Open peeks show notes, followed by one metadata footer ordered `@assignee · ⎇ <base> · #thread · project`, omitting unset parts, the default base, and whatever the view already says (a project board's own project, a filtered thread or assignee, an `@name` view's assignee, a grouped dimension). Rows themselves never show the assignee. A peeked row moves its label into that footer, so it never paints twice. A task with [after](#after) links lists them above the notes. A blocked or review task's peek shows its [live line and block or review](#status) instead of the notes. Below 110 columns, `→` or `l` opens a peek and `←` or `h` closes it. Peeks show up to five wrapped note lines; the [task page](/docs/task-page/) shows the rest.

## Wide stage slider

At **110 usable columns** or wider, use `→` / `l` and `←` / `h` to move through a four-stage slider:

| View | What you see |
| --- | --- |
| Board | Full-width board |
| Split | Board beside task details; selecting another task updates the details |
| Task | Task details beside a narrow board rail |
| Full screen | Task details only |

Press `Enter` from the board to open a task full screen. `Esc` returns to the view you left. The rail keeps the board's done drawer and archived fold state, so a done task opened from the drawer stays selected beside its page. From task focus, `Esc` returns focus to the board beside it. On Projects Overview, the slider stops at Rail: it shows a live project board beside the index and never opens a full-screen task; `Enter` in that right column opens the task page inside the column.

Click a task on the full-width board to open its details alongside it, keeping board focus. Click inside the task column to focus it. Click a task in the rail to bring back the split board. While editing, arrows move the text cursor instead. On the projects tab, click the right preview to focus its live board, or click an index row from Rail to rebind it and return to Split.

Narrowing the pane shows one surface; widening it restores the selected view. Each new session starts with the board alone.

| Pane size | Layout |
| --- | --- |
| 110 columns or wider | Board and task views |
| At least 78×24, below 110 columns | Single board or task page |
| Smaller | Compact layout; usable down to 40×10 |

## Completed tasks

Press `d` to open or close the done drawer. Select a done task and press `ctrl+n` to return it to ready, or `ctrl+o` to send it to the inbox.

The drawer's **archived** group starts closed. Press `→` or `l` on its heading (or double-click it) to expand, and `←` or `h` to close it. `f` with the cursor inside it folds it again; `Shift+F` folds or unfolds it with every other group.

## Archive

Archiving hides work without changing its status.

| Archive | Restore |
| --- | --- |
| Task: select it and press `ctrl+f`; `ctrl+u` right after brings it back | Open the done drawer's archived group, select it, then press `ctrl+u` or `ctrl+f` |
| Project: press `p`, select it, then `ctrl+f` | Switch to the picker's **archived** tab, select it, then `ctrl+u` or `ctrl+f` |

Use `Tab`, `←`, or `→` to switch project-picker tabs. Archiving or restoring tasks with `ctrl+f` is one [undo](#delete-and-undo) step, for one task or a marked set; archiving a project is not. An archive from `tsk archive` is not undoable on the board, and it changes the task, so an earlier undoable action on that task refuses (for example `T12 changed since it was completed (archived); nothing undone`).

### View an archived project

Press `Enter` on a project in the picker's **archived** tab. Its tasks open read-only. Press `ctrl+u` to restore the project, or `Esc`, `p`, or `1`–`3` to leave.

Archived projects are excluded from task-destination pickers. Restoring a project leaves individually archived tasks archived.

### Launch inside an archived project

tsk asks whether to restore it, with one option line each: `y  unarchive app · quick-add goes to it` and `n  keep app archived · quick-add goes to your desk this session`. The `▸` cursor starts on `n`; `↑`/`↓` and `Enter`, the letters, or a double click choose (see [decision cards](#decision-cards)). `Esc` or a click on `esc keep archived` keeps it archived too. Keeping it archived sends new captures to your desk for that session.

## Delete and undo

Press `ctrl+x` twice to delete the marked tasks, or the cursored task when nothing is marked. The confirmation and recovery messages show the task count for a marked set. `ctrl+Delete` is an alternative.

`ctrl+u` undoes the last of these, from the board or the task page, and the status row names the tasks it put back:

| Undone | Status row |
| --- | --- |
| A delete | `delete undone · T12` |
| A done: the task goes back to the status it had, such as started or blocked | `done undone · T12 back to started` |
| An archive or restore with `ctrl+f` | `archive undone · T12`, `unarchive undone · T12` |
| A block or review card | `block undone · T12 back to started` |
| Any other status change: a start that launched nothing, ready, open, a reopen, or an unblock, including the reply box's. Going back to blocked or review reopens the block or review round, with its replies | `status undone · T12 back to blocked` |
| A start that launched an agent: only the status goes back | `start undone · T12 back to ready · @claude kept running` |
| A step tick or untick, a new step, a removed step, or new step text (from the task page; the title and notes saved with it stay) | `steps undone · T12` |
| An assignee, base, or after change | `assignee undone · T12` |

Undo never stops an agent: when the undo takes a task with a live dispatch out of started, the row adds `· @claude kept running`. A marked set is one undo step and the row lists its tasks (`done undone · T3, T4, T5`), or counts them past three (`done undone · 5 tasks`); a completion's undo also names the tasks it had started (`· T203 back to ready`). Undo keeps the last 50 actions. With nothing left, `ctrl+u` says `nothing to undo`.

If a task changed since the action (edited, or archived from the CLI), undo refuses without changing anything and names it: `T12 changed since it was completed (archived); nothing undone`. When the action covered more than that task (a marked set, the tasks a completion started, or the links a deletion dropped), the row names the action's tasks first: `done of T3, T4, T5 not undone: T4 changed since it was completed (reopened)`, or `done of T1 not undone: T2 changed since it was started`. The refused step stays until the next save drops it; `ctrl+u` after that undoes the action before it and names that task. On an archived cursor, `ctrl+u` restores that task (`T12 unarchived`) unless the last action was an archive, which it undoes instead.

Deleted tasks remain available through [trash commands](/docs/cli/#trash) for a limited time. They do not appear on the board.

## Palette

Press `:` on the board or a task page and type to find an action. Use arrows or `Tab` to select, `Enter` to run, and `Esc` to close.

| Available actions | When |
| --- | --- |
| New task, undo, done drawer, group by…, sort by…, fold / unfold group, help, quit | Always |
| Set open/ready/started/blocked/review, edit notes, change scope, delete | A task is selected |
| Set assignee, set base, set after… | A task is selected |
| Dispatch again | A task with a dispatch record is selected |
| Chain in order | Two or more tasks are marked |
| Move step up, move step down | A task-page step is selected |
| Retry save, cancel save | A save has failed |

**Set assignee** opens the same picker as `@`; **set base** opens the branch picker; **set after…** opens the [task picker](#after). Each applies to the marked set when marks are present, with one save and one undo. **Chain in order** makes each marked task run after the one marked before it. **Set status: started** takes the same route as `ctrl+s`, including on blocked and review tasks: an assigned task dispatches, and with marks the [start card](#start-a-marked-set) opens. **Dispatch again** ignores and clears marks, then relaunches only the cursor.

Group by…, sort by…, and fold / unfold group act on the task list, so they are left out on the projects overview and over an open task page. Search matches letters in order: `ssr` finds `set status: review`.

## Help

Press `?` on the board, task page, or another non-text surface to open the searchable shortcut reference. A dim divider separates its focused search field from shortcuts, grouped one binding per row by function. Long descriptions wrap beneath the description column. The card uses at most half the terminal height, except below 15 rows where it may grow to six rows so one result stays visible. Type or paste to filter by a key, action, group, or related term; use arrows, page keys, or the wheel to scroll. `Esc` clears a nonempty search first, then closes Help.

When a text field already owns input, `?` remains text instead of opening Help.

## Save failures

If saving fails, the draft stays open.

- `r` or `Enter`: retry.
- `c` or `Esc`: cancel the pending change.

Resolve the failure before making another change. See [storage](/docs/storage/) for directory and backup information.
