//! The cleanup card (`ctrl+d` on a dispatched task, or on a marked set): each option line says
//! what it does to the PR, the branch and the worktree, the `▸` cursor and `Enter` run one, and
//! each option line is its own mouse target.

use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::agents::MergeMethod;
use tsk_tui::dispatch::pr::{pr_view_outcome, MergeJob, PrLookupError, PrProbeResult, PrState};
use tsk_tui::dispatch::PullRequest;
use tsk_tui::domain::{Dispatch, DomainState};
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, draw_board, BoardModel, BulkCleanup, CleanupChoice, CleanupPr,
    CleanupPrompt, CleanupRow, CleanupRowState, CleanupRun, CleanupRunRow, MergedPr,
    CLEANUP_ENTER_GUARD, ROW_DOUBLE_CLICK_WINDOW,
};
use tsk_tui::ui::input::BoardIntent;
use tsk_tui::ui::mouse::map_board_mouse;
use tsk_tui::ui::render::{QueueHitMap, QueueHitTarget};
use uuid::Uuid;

const WIDTHS: [u16; 3] = [40, 78, 110];

fn record(number: u64) -> Dispatch {
    Dispatch {
        argv: vec!["agent".into()],
        worktree: format!("/tmp/tsk-t{number}-card"),
        branch: format!("tsk/t{number}-card"),
        base: Some("origin/main".into()),
        base_ref: Some("refs/remotes/origin/main".into()),
        base_commit: None,
        base_remote: Some("origin".into()),
        herdr_workspace_id: "w1".into(),
        at: std::time::SystemTime::UNIX_EPOCH,
        cleaned: false,
    }
}

/// A clean, unmerged row whose PR lookup found nothing yet.
fn row(number: u64) -> CleanupRow {
    CleanupRow {
        merge_check: None,
        check_failed: false,
        pr: CleanupPr {
            landed: Some(PrProbeResult {
                lookup: Ok(None),
                tip: Some(format!("tip-{number}")),
            }),
            ..CleanupPr::default()
        },
        unreachable_remote: None,
        inspected: Some(record(number)),
        number,
        task_id: Uuid::from_u128(u128::from(number)),
        worktree: format!("/tmp/tsk-t{number}-card"),
        branch: format!("tsk/t{number}-card"),
        base: "origin/main".into(),
        dirty: false,
        dirty_files: Vec::new(),
        checked_out_elsewhere: false,
        branch_merged: false,
        base_available: true,
        warning: None,
        workspace_exists: true,
    }
}

/// `row` with a PR in `state` into `base`, at the branch tip or not.
fn with_pr(mut row: CleanupRow, pr: u64, state: PrState, base: &str, at_tip: bool) -> CleanupRow {
    let tip = format!("tip-{}", row.number);
    row.pr.landed = Some(PrProbeResult {
        lookup: Ok(Some(PullRequest {
            number: pr,
            url: String::new(),
            state,
            head_oid: if at_tip { tip.clone() } else { "older".into() },
            base: base.into(),
        })),
        tip: Some(tip),
    });
    row
}

fn open_pr(number: u64, pr: u64) -> CleanupRow {
    with_pr(row(number), pr, PrState::Open, "main", true)
}

fn merged(number: u64, pr: u64) -> CleanupRow {
    with_pr(row(number), pr, PrState::Merged, "main", true)
}

fn dirty(mut row: CleanupRow, files: &[&str]) -> CleanupRow {
    row.dirty = true;
    row.dirty_files = files.iter().map(|file| file.to_string()).collect();
    row
}

fn single(row: CleanupRow) -> BoardModel {
    card(CleanupPrompt::single(row))
}

fn card(mut prompt: CleanupPrompt) -> BoardModel {
    prompt.method = MergeMethod::Squash;
    let domain = DomainState::new();
    let mut model = BoardModel::from_domain(&domain, None);
    model.begin_cleanup_prompt(prompt);
    model
}

fn bulk(rows: Vec<CleanupRow>, plain: &[&str]) -> BoardModel {
    let targets = rows.iter().map(|row| row.task_id).collect();
    card(CleanupPrompt::bulk(
        rows,
        BulkCleanup {
            targets,
            plain: plain.iter().map(|name| name.to_string()).collect(),
            ..BulkCleanup::default()
        },
    ))
}

fn buffer(model: &BoardModel, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            draw_board(frame, model);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let buffer = buffer(model, width, height);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

/// The card's body text, rows joined by spaces with the borders gone, so a wrapped sentence
/// reads whole whatever the width.
fn body(model: &BoardModel, width: u16, height: u16) -> String {
    rows(model, width, height)
        .iter()
        .filter_map(|row| {
            let start = row.find('│')?;
            let end = row.rfind('│')?;
            (end > start).then(|| row[start + '│'.len_utf8()..end].trim().to_string())
        })
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn assert_says(model: &BoardModel, texts: &[&str]) {
    for width in WIDTHS {
        let said = body(model, width, 40);
        for text in texts {
            assert!(said.contains(text), "{width} lost {text:?}:\n{said}");
        }
    }
}

fn options(model: &BoardModel) -> Vec<(CleanupChoice, String, bool)> {
    model
        .cleanup_prompt()
        .expect("card")
        .options()
        .into_iter()
        .map(|option| (option.choice, option.text, option.bold))
        .collect()
}

/// Whether the painted row holding `text` is bold.
fn bold_row(model: &BoardModel, width: u16, text: &str) -> bool {
    let buffer = buffer(model, width, 40);
    let lines = rows(model, width, 40);
    let y = lines
        .iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("{text:?} painted:\n{}", lines.join("\n")));
    let x = lines[y]
        .find(text)
        .map(|byte| lines[y][..byte].chars().count())
        .unwrap();
    buffer[(x as u16, y as u16)]
        .modifier
        .contains(Modifier::BOLD)
}

#[test]
fn a_an_open_pr_offers_the_merge_bold_with_the_cursor_on_it() {
    let model = single(open_pr(7, 193));
    assert_says(
        &model,
        &[
            "PR #193 is still open:",
            "▸ m squash-merge PR #193 to main · mark done · remove worktree, branch and agent pane",
            "y don't merge · mark done · remove worktree and agent pane · keep the branch and the PR open",
            "n don't merge · mark done · keep everything",
            "worktree /tmp/tsk-t7-card",
            "branch tsk/t7-card",
            "esc cancel",
        ],
    );
    for width in WIDTHS {
        assert!(bold_row(&model, width, "m  squash"), "{width}");
        assert!(!bold_row(&model, width, "y  don't"), "{width}");
        let painted = rows(&model, width, 40).join("\n");
        assert!(
            !painted.contains("y clean up"),
            "no footer keys:\n{painted}"
        );
    }
    // Sections in order with one blank row between each when the card fits.
    let painted = body(&model, 110, 40);
    let order = ["PR #193", "▸ m", "worktree", "esc cancel"];
    let at = order.map(|text| painted.find(text).unwrap());
    assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{painted}");
}

#[test]
fn the_merge_verb_follows_the_merge_method() {
    for (method, verb) in [
        (MergeMethod::Merge, "m merge PR #193"),
        (MergeMethod::Squash, "m squash-merge PR #193"),
        (MergeMethod::Rebase, "m rebase-merge PR #193"),
    ] {
        let mut prompt = CleanupPrompt::single(open_pr(7, 193));
        prompt.method = method;
        let domain = DomainState::new();
        let mut model = BoardModel::from_domain(&domain, None);
        model.begin_cleanup_prompt(prompt);
        assert!(body(&model, 110, 40).contains(verb), "{method:?}");
    }
}

#[test]
fn b_uncommitted_files_leave_only_n() {
    let files = ["src/ui/input.rs", "src/app.rs", "README.md"];
    let model = single(dirty(open_pr(7, 193), &files));
    assert_says(
        &model,
        &[
            "PR #193 is still open · 3 uncommitted files in the worktree (src/ui/input.rs, src/app.rs, …): commit or discard them to merge or tidy up",
            "▸ n don't merge · mark done · keep everything",
        ],
    );
    assert_eq!(options(&model).len(), 1);
    assert!(options(&model)[0].2, "the only option is the likely one");
    let without_pr = single(dirty(row(7), &files[..1]));
    assert_says(
        &without_pr,
        &["1 uncommitted file in the worktree (src/ui/input.rs): commit or discard it to merge"],
    );
    assert!(!body(&without_pr, 110, 40).contains("PR #"));
}

#[test]
fn c_already_merged_offers_y_bold_and_no_merge() {
    let model = single(merged(7, 194));
    assert_says(
        &model,
        &[
            "PR #194 merged into main ✓",
            "▸ y mark done · remove worktree, branch and agent pane",
            "n mark done · keep everything",
        ],
    );
    assert!(bold_row(&model, 110, "y  mark done"));
    assert!(!options(&model)
        .iter()
        .any(|(choice, ..)| *choice == CleanupChoice::Merge));

    let mut ancestry = row(7);
    ancestry.branch_merged = true;
    ancestry.pr.landed = Some(PrProbeResult {
        lookup: Err(PrLookupError::Missing),
        tip: None,
    });
    assert_says(&single(ancestry), &["Merged into main ✓"]);
}

#[test]
fn d_not_merged_names_why_and_keeps_the_branch() {
    let not_logged_in = {
        let mut row = row(7);
        row.pr.landed = Some(PrProbeResult {
            lookup: Err(PrLookupError::NotLoggedIn),
            tip: None,
        });
        row
    };
    // gh is logged in, but the repository's remotes are not GitHub: its advice still names
    // `gh auth login`.
    let no_github = {
        let mut row = row(7);
        row.pr.landed = Some(PrProbeResult {
            lookup: pr_view_outcome(
                Some(1),
                b"",
                b"none of the git remotes configured for this repository point to a known GitHub host. To tell gh about a new GitHub host, please use gh auth login\n",
            ),
            tip: None,
        });
        row
    };
    let no_gh = {
        let mut row = row(7);
        row.pr.landed = Some(PrProbeResult {
            lookup: Err(PrLookupError::Missing),
            tip: None,
        });
        row
    };
    for (row, verdict) in [
        (row(7), "Not merged into main · no pull request"),
        (
            with_pr(row(7), 193, PrState::Closed, "main", true),
            "Not merged into main · PR #193 closed",
        ),
        (
            with_pr(row(7), 193, PrState::Open, "dev", true),
            "Not merged into main · PR #193 targets dev, not main",
        ),
        (
            not_logged_in,
            "Not merged into main · gh is not logged in (gh auth login)",
        ),
        (
            no_gh,
            "Not merged into main · squash-merged? delete it by hand",
        ),
        (no_github, "Not merged into main · no GitHub remote"),
    ] {
        let model = single(row);
        assert_says(
            &model,
            &[
                verdict,
                "y mark done · remove worktree and agent pane · keep the branch (not merged)",
                "n mark done · keep everything",
            ],
        );
        let said = body(&model, 110, 40);
        if !verdict.contains("squash") {
            assert!(!said.contains("squash-merged?"), "{said}");
        }
        assert!(!said.contains(" m "), "{said}");
    }
}

#[test]
fn e_a_merged_pr_the_branch_outgrew_keeps_the_branch() {
    for (row, verdict) in [
        (
            with_pr(row(7), 193, PrState::Merged, "main", false),
            "PR #193 merged, but the branch has newer commits: the branch is kept",
        ),
        (
            with_pr(row(7), 193, PrState::Merged, "dev", true),
            "PR #193 merged into dev, not main: the branch is kept",
        ),
    ] {
        assert_says(
            &single(row),
            &[
                verdict,
                "y mark done · remove worktree and agent pane · keep the branch",
                "n mark done · keep everything",
            ],
        );
    }
}

#[test]
fn f_an_unconfirmed_merge_says_why_and_keeps_the_branch() {
    let offline = {
        let mut row = row(7);
        row.branch_merged = true;
        row.unreachable_remote = Some("origin".into());
        row
    };
    let timed_out = {
        let mut row = row(7);
        row.check_failed = true;
        row.warning = Some("ancestry check timed out".into());
        row
    };
    let gone = {
        let mut row = row(7);
        row.base_available = false;
        row
    };
    let no_base = {
        let mut row = row(7);
        row.inspected.as_mut().unwrap().base_ref = None;
        row.base_available = false;
        row
    };
    // Merged, even at its PR's tip, but a second worktree holds the branch: deletion keeps it.
    let elsewhere = {
        let mut row = merged(7, 194);
        row.checked_out_elsewhere = true;
        row
    };
    for (row, reason) in [
        (elsewhere, "branch checked out in another worktree"),
        (offline, "offline (base fetch failed)"),
        (timed_out, "the check timed out"),
        (gone, "base no longer available"),
        (no_base, "no recorded base"),
    ] {
        let model = single(row);
        assert_says(
            &model,
            &[
                &format!("Can't tell if it's merged: {reason} · the branch is kept"),
                "y mark done · remove worktree and agent pane · keep the branch",
            ],
        );
        assert!(!body(&model, 110, 40).contains('✓'));
        assert!(!options(&model)
            .iter()
            .any(|(_, text, _)| text.contains("branch and")));
    }
}

#[test]
fn f_a_marked_set_names_a_gone_worktree_and_keeps_a_branch_checked_out_elsewhere() {
    let mut elsewhere = merged(8, 194);
    elsewhere.checked_out_elsewhere = true;
    let mut model = bulk(vec![merged(7, 193), elsewhere], &[]);
    {
        let prompt = model.cleanup_prompt_mut().expect("card");
        let bulk = prompt.bulk.as_mut().expect("bulk");
        bulk.targets.push(Uuid::from_u128(9));
        bulk.missing
            .push((Uuid::from_u128(9), "T9".into(), record(9)));
    }
    assert_says(
        &model,
        &[
            "T8 can't tell if merged: branch checked out in another worktree",
            "T9 can't tell if merged: worktree already gone",
            "delete T7's branch · keep T8's branch",
        ],
    );
}

#[test]
fn g_while_the_pr_lookup_or_a_merge_runs_the_card_offers_nothing() {
    let mut checking = row(7);
    checking.pr.landed = None;
    checking.pr.probe = Some(Default::default());
    let model = single(checking);
    assert_says(&model, &["Checking pull request…", "esc cancel"]);
    assert!(options(&model).is_empty());
}

#[test]
fn g_while_m_merges_the_card_names_the_pr_and_offers_nothing() {
    let mut prompt = CleanupPrompt::single(open_pr(7, 193));
    prompt.merging = Some((MergeJob::new(1), vec![Uuid::from_u128(7)]));
    let model = card(prompt);
    assert_says(&model, &["Merging PR #193 with gh…"]);
    assert!(options(&model).is_empty());

    let mut model = bulk(vec![open_pr(7, 193), open_pr(8, 190)], &[]);
    model.cleanup_prompt_mut().unwrap().merging = Some((
        MergeJob::new(2),
        vec![Uuid::from_u128(7), Uuid::from_u128(8)],
    ));
    assert_says(
        &model,
        &["T7 Merging PR #193 with gh…", "T8 Merging PR #190 with gh…"],
    );
    assert!(options(&model).is_empty());
}

#[test]
fn g_while_the_cleanup_runs_the_card_counts_it() {
    let mut model = single(merged(7, 193));
    let mut run = finished_run(vec![run_row(7, CleanupRowState::Removing)], None);
    run.settled = false;
    model.begin_cleanup_run(run);
    assert_says(&model, &["Cleaning up…"]);

    let mut model = bulk(vec![merged(7, 193), merged(8, 190)], &[]);
    let mut run = finished_run(
        vec![
            run_row(7, CleanupRowState::Removing),
            run_row(8, CleanupRowState::Queued),
        ],
        Some((2, 2)),
    );
    run.settled = false;
    model.begin_cleanup_run(run);
    assert_says(&model, &["T7 Cleaning 1 of 2…", "T8 waiting"]);
}

#[test]
fn a_task_without_an_agent_pane_never_mentions_one() {
    let mut row = open_pr(7, 193);
    row.workspace_exists = false;
    let model = single(row);
    let said = body(&model, 110, 40);
    assert!(said.contains("remove worktree and branch"), "{said}");
    assert!(!said.contains("agent pane"), "{said}");
}

fn run_row(number: u64, state: CleanupRowState) -> CleanupRunRow {
    CleanupRunRow {
        task_id: Uuid::from_u128(u128::from(number)),
        number,
        slot: Some(0),
        inspected: None,
        state,
    }
}

fn finished_run(rows: Vec<CleanupRunRow>, bulk: Option<(usize, usize)>) -> CleanupRun {
    CleanupRun {
        job: tsk_tui::dispatch::CleanupJob::new(0),
        rows,
        bulk,
        refused: Vec::new(),
        settled: true,
        waiting: Vec::new(),
        plan: Vec::new(),
        start_deadline: Instant::now(),
        started: true,
        merged: Vec::new(),
        unmerged: Vec::new(),
    }
}

#[test]
fn h_the_finished_card_says_what_happened_and_reports_the_remote_branch_truthfully() {
    let cleaned = CleanupRowState::Cleaned { branch_kept: None };
    for (remote_left, note) in [
        (Some(false), "remote branch deleted by GitHub"),
        (Some(true), "remote branch kept"),
    ] {
        let mut model = single(open_pr(7, 193));
        let mut run = finished_run(vec![run_row(7, cleaned.clone())], None);
        run.merged.push(MergedPr {
            identifier: "T7".into(),
            pr: "PR #193 (squash)".into(),
            remote_left,
        });
        model.begin_cleanup_run(run);
        assert_says(
            &model,
            &[
                &format!("Done T7 · merged PR #193 (squash) · cleaned · {note}"),
                "esc close",
            ],
        );
    }
    let mut model = single(row(7));
    model.begin_cleanup_run(finished_run(
        vec![run_row(
            7,
            CleanupRowState::Cleaned {
                branch_kept: Some(("not merged".into(), "not merged into main".into())),
            },
        )],
        None,
    ));
    assert_says(&model, &["Done T7 · cleaned · branch kept (not merged)"]);
    let mut model = single(row(7));
    model.begin_cleanup_run(finished_run(
        vec![run_row(
            7,
            CleanupRowState::kept(&tsk_tui::dispatch::CleanupError::FilesInUse),
        )],
        None,
    ));
    assert_says(&model, &["Cleanup refused: files in use"]);
    // `m` went nothing on: the card only closes.
    for (reason, line) in [
        (
            "merge failed: base branch policy",
            "Merge failed: base branch policy · nothing changed",
        ),
        (
            "PR #193 is queued, not merged yet",
            "PR #193 is queued, not merged yet · nothing changed",
        ),
    ] {
        let mut model = single(open_pr(7, 193));
        model.cleanup_prompt_mut().unwrap().settled = Some(vec![("T7".into(), reason.into())]);
        assert_says(&model, &[line, "esc close"]);
        assert!(options(&model).is_empty());
    }
}

/// T7 open PR, T8 already merged, T9 not merged, T10 uncommitted.
fn four_kinds() -> BoardModel {
    bulk(
        vec![
            open_pr(7, 193),
            merged(8, 190),
            row(9),
            dirty(row(10), &["src/ui/input.rs", "src/app.rs", "x"]),
        ],
        &[],
    )
}

#[test]
fn the_marked_set_card_shows_where_each_task_stands_and_names_who_each_option_touches() {
    let model = four_kinds();
    assert_says(
        &model,
        &[
            "T7 PR #193 is still open",
            "T8 PR #190 merged into main ✓",
            "T9 not merged · no pull request",
            "T10 3 uncommitted files · src/ui/input.rs, src/app.rs, …",
            "▸ m squash-merge PR #193 (T7) · mark T7 and T8 done · remove their worktrees, branches and agent panes · T9 and T10 unchanged",
            "y don't merge · mark T7, T8 and T9 done · remove their worktrees and agent panes · delete T8's branch · keep T7's and T9's branches · T10 unchanged",
            "n mark all 4 done · keep everything",
            "esc cancel",
        ],
    );
    let said = body(&model, 110, 40);
    assert!(!said.contains("/tmp/tsk-t7"), "no paths on the set's card");
    assert!(bold_row(&model, 110, "m  squash"));

    let model = bulk(
        (1..=8).map(|number| merged(number, number + 100)).collect(),
        &[],
    );
    let said = body(&model, 110, 40);
    assert!(said.contains("mark all 8 done"), "{said}");
    assert!(said.contains("delete the branches of 8 tasks"), "{said}");
}

#[test]
fn a_card_taller_than_the_frame_drops_its_blank_rows_before_it_scrolls() {
    let model = single(open_pr(7, 193));
    let inner = |height: u16| {
        rows(&model, 110, height)
            .iter()
            .filter(|row| row.contains('│'))
            .map(|row| {
                let start = row.find('│').unwrap() + '│'.len_utf8();
                let end = row.rfind('│').unwrap();
                row[start..end].trim().to_string()
            })
            .collect::<Vec<_>>()
    };
    let sections = |rows: &[String]| {
        let first = rows.iter().position(|row| row.contains("PR #193")).unwrap();
        let last = rows
            .iter()
            .position(|row| row.contains("esc cancel"))
            .unwrap();
        rows[first..=last]
            .iter()
            .filter(|row| row.is_empty())
            .count()
    };
    assert_eq!(sections(&inner(40)), 3, "one blank between each section");
    // The shortest frame that shows the whole card without scrolling does it without blanks.
    let fits = (8..40)
        .find(|&height| !rows(&model, 110, height).join("\n").contains('▼'))
        .expect("some height fits");
    let packed = inner(fits);
    assert_eq!(
        sections(&packed),
        0,
        "blanks dropped:\n{}",
        packed.join("\n")
    );
    let scrolled = rows(&model, 110, fits - 1).join("\n");
    assert!(
        scrolled.contains('▼'),
        "past the drop it scrolls:\n{scrolled}"
    );
}

fn click(
    model: &BoardModel,
    hits: &QueueHitMap,
    target: QueueHitTarget,
    row: usize,
) -> Option<BoardIntent> {
    let area = hits
        .regions
        .iter()
        .filter(|hit| hit.target == target)
        .nth(row)
        .unwrap_or_else(|| panic!("{target:?} row {row} painted"))
        .area;
    map_board_mouse(
        model,
        hits,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + area.width / 2,
            row: area.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    )
}

#[test]
fn every_row_of_an_option_line_is_its_target_and_the_esc_line_cancels() {
    let model = single(open_pr(7, 193));
    let area = Rect::new(0, 0, 40, 40);
    let hits = board_hit_map(area, &model);
    for index in 0..3 {
        let target = QueueHitTarget::CleanupOption(index);
        let wrapped = hits
            .regions
            .iter()
            .filter(|hit| hit.target == target)
            .count();
        assert!(wrapped >= 2, "option {index} wraps at 40 columns");
        for row in 0..wrapped {
            assert_eq!(
                click(&model, &hits, target, row),
                Some(BoardIntent::CleanupClickOption(index)),
                "option {index} row {row}"
            );
        }
    }
    let esc = hits
        .regions
        .iter()
        .filter(|hit| hit.target == QueueHitTarget::ModalClose)
        .count();
    assert_eq!(esc, 2, "`[x]` and the esc line");
    assert_eq!(
        click(&model, &hits, QueueHitTarget::ModalClose, 1),
        Some(BoardIntent::CancelCleanup)
    );
}

#[test]
fn a_single_click_selects_a_double_click_runs_and_two_lines_run_neither() {
    let mut model = single(open_pr(7, 193));
    let prompt = model.cleanup_prompt_mut().unwrap();
    let now = Instant::now();
    assert_eq!(prompt.click_option(1, now, ROW_DOUBLE_CLICK_WINDOW), None);
    assert_eq!(
        prompt.selected,
        Some(CleanupChoice::Clean),
        "selected, not run"
    );
    assert_eq!(
        prompt.click_option(1, now + Duration::from_millis(100), ROW_DOUBLE_CLICK_WINDOW),
        Some(CleanupChoice::Clean)
    );
    // A double click across two lines: each click only selects.
    assert_eq!(prompt.click_option(0, now, ROW_DOUBLE_CLICK_WINDOW), None);
    assert_eq!(
        prompt.click_option(2, now + Duration::from_millis(100), ROW_DOUBLE_CLICK_WINDOW),
        None
    );
    assert_eq!(prompt.selected, Some(CleanupChoice::Keep));
    // Too slow is two single clicks.
    assert_eq!(
        prompt.click_option(
            2,
            now + ROW_DOUBLE_CLICK_WINDOW * 3,
            ROW_DOUBLE_CLICK_WINDOW
        ),
        None
    );
}

#[test]
fn arrows_move_the_cursor_and_stop_at_the_ends() {
    let mut domain = DomainState::new();
    let mut model = single(open_pr(7, 193));
    let selected = |model: &BoardModel| {
        let prompt = model.cleanup_prompt().unwrap();
        prompt.selected_index(&prompt.options())
    };
    assert_eq!(selected(&model), Some(0), "starts on the bold `m`");
    let mut step = |model: &mut BoardModel, intent| {
        apply_intent(&mut domain, model, intent, None).expect("step");
    };
    step(&mut model, BoardIntent::CleanupSelectUp);
    assert_eq!(selected(&model), Some(0), "no wrap at the top");
    for expected in [1, 2, 2] {
        step(&mut model, BoardIntent::CleanupSelectDown);
        assert_eq!(selected(&model), Some(expected));
    }
    assert!(body(&model, 110, 40).contains("▸ n"), "the cursor moved");
    let prompt = model.cleanup_prompt().unwrap();
    assert_eq!(
        prompt.enter_choice(prompt.opened_at + CLEANUP_ENTER_GUARD),
        Some(CleanupChoice::Keep)
    );
    assert_eq!(
        prompt.enter_choice(prompt.opened_at + CLEANUP_ENTER_GUARD / 2),
        None,
        "an Enter right after the card opened is ignored"
    );
}

#[test]
fn page_keys_scroll_the_set_and_arrows_keep_the_selected_option_in_view() {
    let mut domain = DomainState::new();
    let mut model = bulk((1..=12).map(row).collect(), &[]);
    let height = 12;
    let first = rows(&model, 78, height).join("\n");
    assert!(
        first.contains("▸ ") && first.contains('▲'),
        "the card opens scrolled to the ▸ option, so Enter never runs one out of sight:\n{first}"
    );
    for _ in 0..20 {
        apply_intent(&mut domain, &mut model, BoardIntent::CleanupScrollUp, None).unwrap();
    }
    let top = rows(&model, 78, height).join("\n");
    assert!(
        top.contains("T1 ") && !top.contains('▲'),
        "page keys reach the first task:\n{top}"
    );
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CleanupScrollDown,
        None,
    )
    .unwrap();
    let paged = rows(&model, 78, height).join("\n");
    assert!(
        !paged.contains("T1  "),
        "page keys scroll the rows:\n{paged}"
    );
    for _ in 0..20 {
        apply_intent(&mut domain, &mut model, BoardIntent::CleanupScrollUp, None).unwrap();
    }
    rows(&model, 78, height);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::CleanupSelectDown,
        None,
    )
    .unwrap();
    let selected = rows(&model, 78, height).join("\n");
    assert!(
        selected.contains("▸ n"),
        "the selected option is in view:\n{selected}"
    );
    // Paged back to the top, then the frame shrinks: the ▸ option comes back into view.
    for _ in 0..20 {
        apply_intent(&mut domain, &mut model, BoardIntent::CleanupScrollUp, None).unwrap();
    }
    assert!(!rows(&model, 78, height).join("\n").contains("▸ "));
    model.follow_cleanup_selection();
    let resized = rows(&model, 70, height - 2).join("\n");
    assert!(
        resized.contains("▸ n"),
        "a resize scrolls the ▸ option back into view:\n{resized}"
    );
}
