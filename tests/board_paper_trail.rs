//! The task page's PAPER TRAIL: collapsed and dim on every open, `g` (or Enter or a click on its
//! heading) shows every entry newest first, and closed blocks and review rounds expand in place.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::domain::{
    acting_as, BlockDraft, BlockOn, DomainState, HumanStatus, ProvenanceOrigin, ReviewDraft,
    TaskScope,
};
use tsk_tui::ui::board::{apply_intent, board_hit_map, draw_board, BlockTarget, BoardModel};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::mouse::map_board_mouse;
use tsk_tui::ui::render::QueueHitTarget;
use uuid::Uuid;

fn press(domain: &mut DomainState, model: &mut BoardModel, code: KeyCode, modifiers: KeyModifiers) {
    let intent = map_key(model.input_mode(), KeyEvent::new(code, modifiers)).expect("mapped key");
    apply_intent(domain, model, intent, None).expect("apply");
}

/// `Enter` on a selected record: the board resolves it at its keyboard boundary, where the
/// selection is in reach (covered in `app.rs`), to this intent.
fn enter(domain: &mut DomainState, model: &mut BoardModel) {
    apply_intent(domain, model, BoardIntent::ToggleTrailRecord(None), None).expect("apply");
}

fn rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    styled_rows(model, width, height)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// Each painted row, and whether every visible character on it is dim.
fn styled_rows(model: &BoardModel, width: u16, height: u16) -> Vec<(String, bool)> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    tsk_tui::ui::render::assert_buffer_mono(buffer);
    (0..height)
        .map(|y| {
            let text = (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>();
            // The gutter and the scrollbar column are chrome.
            let dim = (2..width.saturating_sub(2))
                .map(|x| &buffer[(x, y)])
                .filter(|cell| !cell.symbol().trim().is_empty())
                .all(|cell| cell.modifier.contains(Modifier::DIM));
            (text, dim)
        })
        .collect()
}

fn toggle(domain: &mut DomainState, model: &mut BoardModel) {
    press(domain, model, KeyCode::Char('g'), KeyModifiers::NONE);
}

/// A task taken through steps, a block answered by you, a review sent back, and a second round
/// approved: more than five entries, two of them closed records.
fn worked_task() -> (DomainState, BoardModel, Uuid) {
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Ship the trail",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let first = domain.add_step(id, "write it").expect("step");
    let second = domain.add_step(id, "test it").expect("step");
    domain.toggle_step(id, first).expect("check");
    domain.toggle_step(id, second).expect("check");
    acting_as("claude", || {
        domain
            .block(
                id,
                BlockDraft::from_input(Some("need creds"), None, &[], BlockOn::You).expect("draft"),
                "claude",
            )
            .expect("block");
    });
    domain.reply(id, "use the vault", "you").expect("reply");
    domain
        .set_status(id, HumanStatus::Started)
        .expect("unblock");
    for round in ["first pass", "second pass"] {
        acting_as("claude", || {
            domain
                .review(
                    id,
                    ReviewDraft::from_input(
                        Some(round),
                        &["tests pass".into()],
                        None,
                        BlockOn::You,
                    )
                    .expect("draft"),
                    "claude",
                )
                .expect("review");
        });
        if round == "first pass" {
            domain
                .set_status(id, HumanStatus::Started)
                .expect("send back");
        }
    }
    domain.complete(id).expect("approve");
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer, None).ok();
    let index = model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .expect("task visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    (domain, model, id)
}

#[test]
fn the_wide_task_column_beside_the_board_paints_the_trail_too() {
    let (mut domain, mut model, _) = worked_task();
    // Back to the board, then the details column beside it.
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");
    apply_intent(&mut domain, &mut model, BoardIntent::StageRight, None).expect("open column");
    let page = rows(&model, 130, 40);
    assert!(
        page.iter().any(|row| row.contains("DONE"))
            && page.iter().any(|row| row.contains("PAPER TRAIL")),
        "{}",
        page.join("\n")
    );
}

fn trail(page: &[String]) -> Vec<String> {
    let start = page
        .iter()
        .position(|row| row.contains("PAPER TRAIL"))
        .unwrap_or_else(|| panic!("{}", page.join("\n")));
    page[start..]
        .iter()
        .map(|row| row.trim_end_matches(['▌', ' ']).trim().to_string())
        .take_while(|row| !row.is_empty() && !row.starts_with('⎇'))
        .collect()
}

#[test]
fn the_trail_opens_collapsed_and_dim_and_g_shows_every_entry() {
    let (mut domain, mut model, id) = worked_task();
    let count = tsk_tui::activity::paper_trail(domain.get(id).expect("task")).len();
    assert!(count > 5, "{count}");
    let collapsed = trail(&rows(&model, 100, 60));
    assert_eq!(collapsed, [format!("PAPER TRAIL · {count} ▸")]);

    toggle(&mut domain, &mut model);
    let page = styled_rows(&model, 100, 80);
    let start = page
        .iter()
        .position(|(row, _)| row.contains("PAPER TRAIL"))
        .expect("heading");
    let all = trail(&page.iter().map(|(row, _)| row.clone()).collect::<Vec<_>>());
    assert_eq!(all[0], format!("PAPER TRAIL · {count} ▾"));
    assert_eq!(all.len(), 1 + count, "every entry: {all:#?}");
    assert!(
        page[start..start + all.len()].iter().all(|(_, dim)| *dim),
        "all dim, records too: {all:#?}"
    );
    assert!(all[1].starts_with("review → done · you"), "{all:#?}");
    assert!(
        all[2].starts_with("review round 2 · approved · you") && all[2].ends_with('▸'),
        "{all:#?}"
    );
    assert!(all.last().is_some_and(|row| row.starts_with("created · you")));
    for entry in [
        "2 steps checked · you",
        "open → blocked · @claude",
        "blocked on you · need creds · 1 reply · you",
    ] {
        assert!(all.iter().any(|row| row.starts_with(entry)), "{entry}: {all:#?}");
    }
    assert!(!all.iter().any(|row| row.ends_with("earlier")), "{all:#?}");

    toggle(&mut domain, &mut model);
    assert_eq!(trail(&rows(&model, 100, 60)), collapsed, "g collapses it");

    // The expanded state lasts while you stay on the task and resets when a page opens again.
    toggle(&mut domain, &mut model);
    apply_intent(&mut domain, &mut model, BoardIntent::CloseLayer, None).expect("close page");
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("reopen");
    assert_eq!(trail(&rows(&model, 100, 60)), collapsed, "collapsed again");
}

#[test]
fn tab_from_add_step_reaches_the_heading_then_the_records_and_enter_expands_one_in_place() {
    let (mut domain, mut model, _) = worked_task();
    // Two steps, then `+ step`, then the collapsed heading.
    for _ in 0..4 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
    assert_eq!(model.block_target(), Some(BlockTarget::TrailHeading));
    assert!(
        rows(&model, 100, 60)
            .iter()
            .any(|row| row.contains("▸ PAPER TRAIL")),
        "the heading paints the selection"
    );
    // Collapsed, Tab past the heading wraps to the first step.
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(model.block_target(), None);
    assert!(rows(&model, 100, 80)
        .iter()
        .any(|row| row.contains("▸ ✓ write it")));
    for _ in 0..3 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
    assert_eq!(model.block_target(), Some(BlockTarget::TrailHeading));
    // Enter on the heading (resolved at the keyboard boundary to this intent) expands it.
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleTrail, None).expect("expand");
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    let newest = model.block_target();
    assert!(matches!(newest, Some(BlockTarget::Trail(_))), "{newest:?}");
    enter(&mut domain, &mut model);
    let expanded = trail(&rows(&model, 100, 60));
    let round = expanded
        .iter()
        .position(|row| row.starts_with("▸ review round 2 · approved") && row.ends_with('▾'))
        .unwrap_or_else(|| panic!("{expanded:#?}"));
    assert!(
        expanded[round + 1].starts_with("set by @claude"),
        "{expanded:#?}"
    );
    assert!(
        expanded.iter().any(|row| row == "done   second pass"),
        "{expanded:#?}"
    );
    assert!(
        expanded.iter().any(|row| row == "○ tests pass"),
        "{expanded:#?}"
    );

    // Tab walks on through the older records, then wraps to the steps.
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    enter(&mut domain, &mut model);
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    enter(&mut domain, &mut model);
    let all_open = trail(&rows(&model, 100, 80));
    assert!(
        all_open.iter().any(|row| row == "why    need creds"),
        "{all_open:#?}"
    );
    assert!(
        all_open
            .iter()
            .any(|row| row.starts_with("└ you") && row.ends_with("use the vault")),
        "{all_open:#?}"
    );
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(
        model.block_target(),
        None,
        "Tab past the oldest record leaves the trail"
    );
    assert!(
        rows(&model, 100, 80)
            .iter()
            .any(|row| row.contains("▸ ✓ write it")),
        "and wraps to the first step"
    );

    // Shift+Tab from the heading climbs back to `+ step`.
    for _ in 0..3 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
    assert_eq!(model.block_target(), Some(BlockTarget::TrailHeading));
    press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(model.block_target(), newest);
    press(
        &mut domain,
        &mut model,
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    );
    assert_eq!(model.block_target(), Some(BlockTarget::TrailHeading));
    press(
        &mut domain,
        &mut model,
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    );
    assert_eq!(model.block_target(), None);
    assert!(
        rows(&model, 100, 80)
            .iter()
            .any(|row| row.contains("▸ + step")),
        "Shift+Tab lands on + step"
    );

    // Collapsing with a record selected hands the selection to the heading.
    for _ in 0..2 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
    assert_eq!(model.block_target(), newest);
    toggle(&mut domain, &mut model);
    assert_eq!(model.block_target(), Some(BlockTarget::TrailHeading));
}

#[test]
fn a_click_on_the_heading_expands_the_trail_and_a_click_on_a_record_expands_it() {
    let (mut domain, mut model, _) = worked_task();
    let area = Rect::new(0, 0, 100, 60);
    rows(&model, 100, 60);
    let hits = board_hit_map(area, &model);
    let heading = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::TrailHeading)
        .expect("the heading is clickable");
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: heading.area.x + 2,
        row: heading.area.y,
        modifiers: KeyModifiers::NONE,
    };
    let intent = map_board_mouse(&model, &hits, click).expect("click maps");
    assert_eq!(intent, BoardIntent::ToggleTrail);
    apply_intent(&mut domain, &mut model, intent, None).expect("apply");
    assert!(trail(&rows(&model, 100, 60))[0].ends_with('▾'));

    let hits = board_hit_map(area, &model);
    let record = hits
        .regions
        .iter()
        .find_map(|hit| match hit.target {
            QueueHitTarget::TrailRecord(index) => Some((index, hit.area)),
            _ => None,
        })
        .expect("a record is clickable");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleTrailRecord(Some(record.0)),
        None,
    )
    .expect("apply");
    assert_eq!(model.block_target(), Some(BlockTarget::Trail(record.0)));
    assert!(trail(&rows(&model, 100, 60))
        .iter()
        .skip(1)
        .any(|row| row.ends_with('▾')));
}

#[test]
fn the_trail_wraps_without_truncating_at_40_and_120_columns() {
    let (mut domain, mut model, _) = worked_task();
    toggle(&mut domain, &mut model);
    for width in [40u16, 120] {
        let page = rows(&model, width, 80);
        let entries = trail(&page);
        assert!(
            !entries.iter().any(|row| row.contains('…')),
            "{width}: {entries:#?}"
        );
        let joined = entries.join(" ");
        assert!(joined.contains("need creds"), "{width}: {entries:#?}");
        assert!(joined.contains("created · you"), "{width}: {entries:#?}");
        assert!(
            !page.iter().any(|row| row.contains(" ago")),
            "{width}: the footer no longer shows dates\n{}",
            page.join("\n")
        );
    }
}

/// A 32-character profile name makes the `└ @<name> 2m  ` lead wider than a 40-column trail:
/// the lead takes its own row and the reply text still shows, under a bounded indent.
#[test]
fn a_long_agent_lead_wraps_onto_its_own_row_and_keeps_the_reply_text_at_40_columns() {
    let agent = "a-very-long-agent-profile-name-x";
    assert_eq!(agent.len(), 32);
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "Long names",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    acting_as(agent, || {
        domain
            .block(
                id,
                BlockDraft::from_input(Some("need creds"), None, &[], BlockOn::You).expect("draft"),
                agent,
            )
            .expect("block");
        domain
            .reply(id, "the reply text must stay visible", agent)
            .expect("reply");
    });
    domain
        .set_status(id, HumanStatus::Started)
        .expect("unblock");
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).expect("open page");
    toggle(&mut domain, &mut model);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ToggleTrailRecord(Some(0)),
        None,
    )
    .expect("expand");
    let page = rows(&model, 40, 60);
    let entries = trail(&page);
    let lead = entries
        .iter()
        .position(|row| row == "└")
        .unwrap_or_else(|| panic!("the lead takes its own row: {entries:#?}"));
    assert!(
        entries[lead + 1].starts_with("@a-very-long-agent"),
        "{entries:#?}"
    );
    assert_eq!(
        entries[lead + 3..lead + 5].join(" "),
        "the reply text must stay visible",
        "{}",
        page.join("\n")
    );
    let text_row = page
        .iter()
        .find(|row| row.trim_start().starts_with("the reply text"))
        .expect("reply text row");
    let indent = text_row.len() - text_row.trim_start().len();
    assert!(
        indent <= 8,
        "the text continues under a bounded indent ({indent}):\n{}",
        page.join("\n")
    );
}
