---
name: tsk-cli
description: Work the user's tsk task board from the command line. Use when asked to add, update, edit, start, block, finish, archive, or restore a task on the board (or "tsk", "the tsk board", "the desk"), to add or tick steps, to answer "what's on the board", "what's next", "what's on deck", "what needs me", or to refine a task ("refine T12", "let's discuss T12", "what's missing from T12", "improve / rewrite this task"). Always `tsk add|list|status|reply|edit|steps|archive|dispatch|clean|trash`, never the TUI.
version: 1.5.0
---

# tsk: the user's task board

tsk is the user's task board. Tasks have a human status (`open` · `ready` · `started` · `blocked` ·
`review` · `done`), live on the **desk** (no project) or in a **project** (a Git repo root, named
by its basename), and may carry a **thread** label and an optional **assignee** that exactly matches a configured agent profile, plus an optional dispatch **base** branch. The user sees the board in a TUI; you never
open it. Every read and write goes through the CLI, and the user's board updates live.

`tsk --help` is the syntax reference: commands, statuses, and every flag. Run `tsk help <command>`
before the first use of a command in a session; this file only holds what `--help` cannot tell
you: how to behave on someone else's board.

## If tsk is not installed

Run `tsk --version`. If the command is unavailable, give the user https://gettsk.sh/docs/install.md
and stop. Never run an installer, package manager, or source build unless they asked.

## Board language

| The user says | Statuses | Read |
| --- | --- | --- |
| what needs me | `blocked`, `review` | `tsk list --json`, filter `status` |
| in motion | `started` | same |
| on deck, what's next | `ready` | `tsk list --ready --json` |
| inbox, untriaged | `open` | `tsk list --open --json` |
| other projects, everything live | the five live statuses | `tsk list --all --json`, `--desk`, `-p <project>` |
| done, archived, deleted | | `tsk list --done --json`, `--archived`, `--deleted` (each may combine with a scope flag) |
| one task, in full | | `tsk list T12 --json` (notes, steps with `short_id`, assignee, base, thread, dispatch, `activity`: who did what, newest first) |

`T12`, `t12`, `12`, and the UUID all address the same task. Prefer `T12`, it is what the user sees.
New tasks start `open` in the inbox; `ready` means the user picked it.

## Rules

1. **Human status is the user's.** When your work on a task is finished, set `review`. Set
   `done` only when the user says the task is done or told you to close it. Never mark tasks
   done from your own progress.
2. **Use a known project name or a full path.** A bare `-p name` must resolve to one existing
   project; unknown or ambiguous names refuse. An existing absolute directory is the only way to
   create a new project destination. `tsk edit` cannot move a task: re-add in the right scope and
   archive the stray.
3. **Read JSON, not presentation.** Always add `--json` to `tsk list`. Human output is for showing
   the user or troubleshooting interactively. Mutation acknowledgements may be human-only: trust
   the exit and refusal codes, then verify state with `tsk list … --json`.
4. **Never blind-retry.** Read the exit code, then act (table below). `add`, `status`, `edit`,
   `archive` are idempotent; `dispatch`, `steps toggle`, and `steps remove` are not, so run
   `tsk list T12 --json` before retrying any `steps` command.
5. **Never pass `--state-dir`** unless the user asked. It points at a different board.
6. **Ignore notice rows.** Rows the board paints as `N1`… are human-only (starter tasks and
   release notes). `tsk list --json` never shows them and no command addresses them.
7. Values that begin with `-` need the `=` form: `--title="-fix parser"`, `--notes="-5 degrees"`.
8. **Thread names** start with a letter or digit, then lowercase letters, digits, `-` and `.`, up
   to 32 characters. `--thread` lowercases the value; anything else is a usage error (exit 2). In a
   JSON plan a bad `thread` is an item refusal (`invalid-thread`).
9. **Assignees are exact agent profiles.** `--assignee` normalizes to lowercase, then must match a
   profile from `config.toml`; unknown names refuse with `unknown-agent`. Use `--unassign` to clear.
   `tsk list --assignee <name> --json` filters assigned work.
10. **Bases are branches in the task's repo.** `tsk add/edit --base <branch>` sets an explicit local
    or remote branch, never a tag or bare commit; unknown branches refuse with `unknown-base`.
    `--clear-base` restores the repository's remote default (`origin/HEAD`). Where the CLI runs
    never supplies the dispatch base. `tsk dispatch --base <branch>` overrides a first launch, not
    the task. `--again` always keeps the recorded base and ignores overrides or later task edits.

## Exit contract (all commands)

| Exit | Meaning | Do |
| --- | --- | --- |
| 0 | done, or already true | after a mutation, verify with `tsk list … --json`; otherwise nothing |
| 1 | refusal of one or more items; valid siblings persisted | fix and retry only the refused subset |
| 2 | usage or parse error, nothing persisted | correct the invocation, run again |
| 3 | store I/O, commit indeterminate | `tsk list … --json` (also `--done`, `--archived`), retry only what is missing |

`add`, `status`, `edit`, `steps`, `archive` and `unarchive` print a stable refusal code first
(`tsk status: unknown-task: T12 is not on the board`), listed per command under
`tsk help <command>`. Branch on the code; the message is not a contract. `trash restore` and
`project` refuse with a message only.

## Workflows

**What's on the board.** `tsk list --json` (this repo's project, or the desk outside Git). Group by
`status` in board language, lead with what needs the user, then in motion, then on deck.

**Start work on a task.** `tsk list T12 --json` for notes and steps. `tsk status T12 start
--no-dispatch`: a plain `start` on an assigned task launches its agent, and you are doing the work.
Tick steps as you go: `tsk steps T12 toggle <short_id>`.

**Dispatch assigned work.** Only when the user asks. Read the task, then `tsk dispatch T12` (or
`tsk status T12 started`, which dispatches an assigned task never dispatched): it launches the
assignee's agent in a new worktree and Herdr workspace and sets the task `started`.
`already-dispatched` (or `agent-gone` from `status started`) means a launch exists; use `--again`
only when the user wants it relaunched. A `no launch: <reason>` line after `status started` (outside Herdr, or a
desk task) means the task started but nothing launched; tell the user rather than retrying.
After exit 3, read the task before retrying, since an agent may already be running. Watch it with
`herdr agent get t12-<assignee>`. `tsk list T12 --json` shows the `dispatch` record (worktree,
branch, base, starting commit). The default prompt tells the launched agent to work only on its
branch, open a pull request into the base without merging, then set the task to review or blocked.

**Clean up a dispatch.** Only when the user asks, after review and merge: `tsk clean T12`, or
`tsk status T12 done --clean` when they also said the task is done. Never clean the worktree you are
running in. Cleanup removes a clean worktree and deletes the branch only when it is merged into the
recorded base; otherwise it keeps the branch and says why (`branch.reason` in `--json`). Uncommitted
work refuses with `dirty-worktree`. Deleting a kept branch is the user's decision.

**Hand back.** `tsk status T12 review --done "…" --check "…" [--check "…"] [--next "…"]`: what you
did, one `--check` per thing the user should verify (they mark each passed or failed), and what comes
after. `--on <agent>` hands the review to another agent profile instead of the user. Running it again
while the task is in review updates the same round; say in one line what to look at. Read the round
in `tsk list T12 --json` under `review` (`round`, `checks` with `state`, `feedback`, `answered`).
A message `[tsk T12 sent back] …` (with `Failed checks: …`) is the user's feedback: the task is
`started` again, the round is closed in `past_reviews`; address it, then set review again (round N+1).
`tsk reply T12 "…"` adds feedback to the open round. Never approve (set done) yourself.

**Block on the user.** `tsk status T12 blocked --why "…"` with your question; add `--needs "…"`
for what would unblock you and one `--option "…"` per choice the user can pick. `--on T9` waits on
another task, `--on "<text>"` on anything else; the default is the user. Running it again edits the
same block. Read answers in `tsk list T12 --json` under `block` (`replies`, `answered`). Unblocking
or a relaunch closes the block. On a relaunch, read whichever record closed last (`closed_at`): the
last `past_blocks` entry's `replies`, or the last `past_reviews` entry's `feedback` and failed `checks`
when it was `sent_back`.
`tsk reply T12 "…"` adds a reply to the open block (`not-blocked` when there is no block or review); it is not
idempotent, so read the task before retrying. When the user asks to send an answer to the task's
running agent, add `--send`; it sends only that reply and prints `sent to @<agent>` or `not sent: <reason>`.
A message `[tsk T12 unblocked] …` or `[tsk T12 reply] …` in your session is the user's answer to your
block. Text over 4 KB refuses with `text-too-long`. Never unblock a task yourself unless the user said so.

**Plan as steps.** When the user wants order of work tracked on the task: `tsk steps T12 add
"…"` per step, in order. Do not add steps for your own bookkeeping.

**Capture many.** `cat plan.json | tsk add` with
`[{"title": "…", "notes": "…", "project": "…", "thread": "…", "assignee": "…", "base": "…"}]`; only `title` is required, an
omitted `project` takes the default scope. The result lists `created`, `existing` and `failed`
items; on exit 1 retry only the `failed` items, never the whole plan. Full shape:
https://gettsk.sh/docs/cli.md#json-plans.

## Refine a task

Use when the user asks to refine, discuss, improve, rewrite, or find what's missing from a task,
or brings a rough idea that should become one. This is a shaping pass, not a build: no acceptance
criteria, no design document, no code. It ends with a better task on the board.

> **Hard gate: propose, then write.** Show the full rewrite (title and notes) and get a yes before
> any `tsk edit` or `tsk add`. `edit --notes` replaces the whole body; a silent write loses the
> user's own words.

1. **Read.** `tsk list T12 --json` for the task as written. Then `tsk list --json` (and `--all
   --json` when the task may belong elsewhere) for neighbours in the same project or thread:
   duplicates, a task this depends on or unblocks, a thread it should join.
2. **Ground in code.** Before saying anything, open the files, docs, and tests the task touches.
   Check the user's claims against what exists; when the code answers a question, do not ask it.
3. **Diverge briefly.** Two or three approaches with tradeoffs, recommendation first and why.
   Scale to the task: a bug fix gets two lines, a feature gets a short list. Draw a mock in a code
   block when the shape lands better than prose (a screen, a table, a CLI transcript).
4. **Grill, one question at a time.** Multiple-choice with a recommended answer. Prefer reading
   code over asking. Sharpen fuzzy or overloaded words into one canonical term and use only that
   term afterwards. When an answer exposes that the framing was wrong, go back to step 3 for that
   part instead of building on it.
5. **Settle and propose.** Present the rewrite in the notes shape below, then the exact commands
   you will run. Stop and wait.
6. **Write back on a yes.**
   - Exists: `tsk edit T12 --title "…" --notes "…"` (add `--assignee <profile>` or `--unassign` only when agreed).
   - Does not exist yet: `tsk add -t "…" -n "…"` in the agreed scope (`--desk`, `-p`), with
     `--thread` if agreed.
   - Steps only when the user wants order of work tracked: `tsk steps T12 add "…"` per step.
   - Never change status. `edit` cannot change scope or thread: for a scope change re-add in the
     right scope and archive the stray; say so before doing it.
7. **Stop.** The hand-off is the updated or created task. Report its number and title in one line.
   Do not offer builders, worktrees, or plans, and do not start building unless asked.

**Title**: the outcome, verb-first, under about 60 characters (`Make tsk --help the CLI reference`,
not `help improvements`).

**Notes shape** (the board renders `**bold**`, `*em*`, `` `code` ``, `#` headings, `-` lists,
fenced code):

```
Ask: <the user's words, verbatim, one or two lines>

Settled
- <one actionable sentence per decision>

Rules / edge cases
- <behaviour at the boundaries, refusals, what must not change>

Shape
- <files, modules, surfaces to touch; what not to touch>

Open
- <deferred question, and why it can wait>
```

Omit a heading that would be empty. Keep the user's terms once sharpened; the notes are the brief
a future agent (or the user) builds from without this conversation.
