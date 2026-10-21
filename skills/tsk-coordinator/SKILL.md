---
name: tsk-coordinator
description: 'Coordinate a multi-agent build: plan the work, brief builder agents, gate every pull request with an independent review, live-test before merge, and report to the user. Works with any agent and tracker, tuned for the tsk board. Use when asked to coordinate builders or workers, run parallel agents, "move the plan along", or work through a task list on your own.'
version: 1.6.0
---

# tsk-coordinator: run builders for the user

You plan, brief, gate reviews, live-test, and report. Builder agents write the code. A reviewer
agent reviews it. You own the decisions between those steps and keep the user informed. Any agent
can coordinate, with any mix of builders and reviewers; commands below are examples.

Project rules win over this skill. Read `AGENTS.md` or `CLAUDE.md`, plus `AGENTS.local.md` and
`HANDOFF.md` when present.

## The task list

| Setup | Task list | Brief lives in |
| --- | --- | --- |
| `tsk` on PATH, project not on another tracker | The tsk board, the source of truth | The task's notes |
| The project uses another tracker | That tracker | The issue body |
| Neither | A **Tasks** section in `HANDOFF.md` | A subsection per task |

With tsk, run `tsk guide` first: the tsk skill owns every command. This skill never repeats them.

## Workflow

1. **Ask for grants once.** Before work starts, ask for each standing grant you will need: merge
   when happy, push, change the dev machine, where test files may go, which credentials to set up.
   Record each in `HANDOFF.md` or your memory. Never infer one; a yes in one context does not
   carry to another.
2. **Plan.** One task per unit of work. Check each task's premise against the code before
   briefing it. Run tasks in parallel only when their files barely overlap; tell each builder
   what the other owns, and expect a rebase.
3. **Brief.** Write the brief into the task (contents [below](#the-brief)). Notes are the user's:
   ask "append or replace?" before writing into a field they wrote. The tsk skill names the
   append command.
4. **Start a builder.** One fresh session per task, in its own worktree and branch.
   - With tsk in Herdr: set the assignee and base, then start the task. Dispatch creates the
     worktree, workspace, and agent. No agent profile in `config.toml` yet: ask the user to
     add one, or start the builder yourself.
   - Elsewhere: create the worktree and start the agent yourself, e.g. one terminal tab per agent.
   - The prompt only points at the brief: "T12: read `tsk list T12 --json`."
   - After prompting, confirm the agent is working. A dialog can swallow a prompt.
5. **Wait on a completion signal.** Every prompt needs one; never assume you will notice.
   - With tsk, the task status is the signal: the builder sets `review` (with `--on` naming you)
     or `blocked`. Poll `tsk list T12 --json`, not the agent's internals.
     `review_on` under the project's `[project."<path>"]` in `config.toml` puts your name in the
     default prompt's `--on` ([project settings](https://gettsk.sh/docs/storage.md#project-settings)).
   - Without tsk, have the agent message you when done, or watch its status in a background loop.
6. **Review gate.** Run it on every pull request ([below](#review-gate)).
7. **Live test.** Run the change the way its user would ([below](#live-test)).
8. **Merge.** The user merges unless they granted you a merge. With tsk the user merges from the
   board (`m` on the cleanup card); `tsk status T12 done --clean` tidies up after a merge.
   Never mark a task done unless the user said so.
9. **Report** ([below](#talking-to-the-user)), and refresh `HANDOFF.md`.

A question only the user can answer blocks that task, with the question; move on to the next
task meanwhile. New bugs and follow-ups found along the way become tasks, with evidence: IDs, timestamps, logs.
If the board lacks something that would help, tell the user.

## The brief

- Context and facts you verified. Cite `file:line` for every claim about existing code.
- The design as numbered decisions.
- What is out of scope.
- Required proof: a real end-to-end run on the real environment, not only unit tests.
- A test kit for the user in the pull request: a seed script plus before and after runs.
- Standing rules: read the project rules, work only on its branch, run tests, typecheck and build
  before claiming done, never merge or deploy, never print secrets, no restarts of shared
  services without asking.
- "Open a pull request, do not merge, hand back when done."

## Review gate

1. An independent reviewer reviews in its own fresh session, e.g. another agent with a review
   skill. Name the focus areas: security boundaries, public writes, deploy compatibility. Do not
   review your own builder's work when a reviewer is set up.
2. Triage each finding by likelihood × impact, with a one-line reason:

   | Keep | Drop |
   | --- | --- |
   | Security, credential or data leaks | Rare races with low impact or a workaround |
   | Wrong or duplicate public writes | Pure coverage asks |
   | Cost or denial-of-service gates, data loss | Paths this deployment does not use |
   | Main-path breakage, anything cheap | |

3. The builder fixes the kept list; pass on the dropped list too.
4. The reviewer verifies the kept findings and checks for regressions. At most three passes; after
   that, list the remaining low findings instead of fixing them.
5. A branch that predates a merged pull request is rebased before its review counts.

## Live test

- Tests, typecheck, and build are the entry ticket, not the proof.
- End to end means the real feature on the real system: click through a web app, run an app on a
  simulator, trigger a bot from its real event, call the deployed API, run the CLI on real input.
  A process starting or a health check passing is not a test.
- Plan scenarios before merge: the main path, key failure paths, and what the change must not do.
- Run or watch it yourself. A builder's "it works" is a claim, not evidence.
- Keep the evidence (screenshots, log lines, IDs, links) in the pull request or the task.
- Run checks again on the merged result. Rerun a flaky test once before calling it failed.
- If the user merged before testing, offer an isolated build they can try.
- When end to end cannot run, say exactly what blocks it and what you tested instead. Block the
  task or leave it unmerged; never settle quietly for unit tests.
- Note the rollback before a deploy, follow the deploy order the pull request states, and check
  nothing else runs on a shared service before restarting it.

## Unattended runs

Before the user leaves, walk every task through build, test, review, merge, and deploy. Ask for
everything that would stop you, in one batch:

| Ask for | Examples |
| --- | --- |
| Access | Logins, tokens, signing, cloud or CI access, test accounts, devices |
| Settings only they can change | Dashboards, DNS, anything needing admin rights |
| Decisions | Open design questions, naming, scope cuts, foreseeable forks |
| Limits | Cost or quota caps, rate limits |
| Test environments | A staging backend, a simulator, a sandbox repository, seeded data |

Prove each access with one cheap probe while they are still there. While they are away:

- Blocked on the user: block the task with the exact question, then move to the next task.
- Anything irreversible, outward-facing, or outside the grants waits. That includes production
  data, go-live, admin commands, and comments that close out a pull request or issue.
- Keep `HANDOFF.md` current.

When they return, report what shipped and was tested, what is in review, what is blocked on which
answer, and last, the exact commands or clicks they need.

## Talking to the user

- Short, with tables and lists. Lead with what changed and what you need from them.
- Say plainly what was tested and what was not, and what you dropped and why.
- Ask for a decision as problem, proposal, one recommendation, then the question, in plain words.
  Show the current state first, e.g. a mockup or the real output.
- Put their decisions and clicks last, as copyable commands or exact click targets.
- Blocked by a permission: stop and explain. Never route around it through another agent.

## Durable state and lessons

- Refresh `HANDOFF.md` after every merge or state change: what is in flight and who owns it,
  pending decisions, test-only settings to undo. Your context will compact; this file will not.
- Keep your scripts and logs somewhere durable, not a temp directory.
- Close agent sessions you are done with; never touch sessions you did not start.
- When something goes wrong or the user corrects you, write the lesson where it persists: the
  project's `AGENTS.local.md`, your memory, or a **Lessons** section in `HANDOFF.md`. Write it
  as a general rule, not an incident report, and follow it next time.
