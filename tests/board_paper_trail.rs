//! The task page's PAPER TRAIL: who did what, newest first, the latest five until `a` shows
//! them all, and closed blocks and review rounds that expand in place.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
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
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
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
fn the_page_shows_the_latest_five_and_a_shows_every_entry() {
    let (mut domain, mut model, _) = worked_task();
    let latest = trail(&rows(&model, 100, 60));
    assert!(latest[0].ends_with("a all"), "{latest:#?}");
    assert_eq!(
        latest.len(),
        1 + 5 + 1,
        "heading, five entries, + N earlier: {latest:#?}"
    );
    assert!(latest[1].starts_with("review → done · you"), "{latest:#?}");
    assert!(
        latest[2].starts_with("review round 2 · approved · you") && latest[2].ends_with('▸'),
        "{latest:#?}"
    );
    assert!(
        latest[6].starts_with("+ ") && latest[6].ends_with("earlier"),
        "{latest:#?}"
    );

    press(
        &mut domain,
        &mut model,
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    );
    let all = trail(&rows(&model, 100, 60));
    assert!(all[0].ends_with("a latest"), "{all:#?}");
    assert!(!all.iter().any(|row| row.ends_with("earlier")), "{all:#?}");
    assert_eq!(
        all.last()
            .map(String::as_str)
            .map(|row| row.starts_with("created · you")),
        Some(true)
    );
    assert!(
        all.iter()
            .any(|row| row.starts_with("2 steps checked · you")),
        "{all:#?}"
    );
    assert!(
        all.iter()
            .any(|row| row.starts_with("open → blocked · @claude")),
        "{all:#?}"
    );
    assert!(
        all.iter()
            .any(|row| row.starts_with("blocked on you · need creds · 1 reply · you")),
        "{all:#?}"
    );

    press(
        &mut domain,
        &mut model,
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    );
    assert_eq!(
        trail(&rows(&model, 100, 60)),
        latest,
        "a folds back to the latest five"
    );
}

#[test]
fn tab_from_add_step_reaches_the_records_and_enter_expands_one_in_place() {
    let (mut domain, mut model, _) = worked_task();
    press(
        &mut domain,
        &mut model,
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    );
    // Two steps, then `+ step`, then the newest closed record.
    for _ in 0..4 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
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

    // Shift+Tab from the newest record climbs back to `+ step`.
    for _ in 0..3 {
        press(&mut domain, &mut model, KeyCode::Tab, KeyModifiers::NONE);
    }
    assert_eq!(model.block_target(), newest);
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
}

#[test]
fn a_click_on_earlier_shows_every_entry_and_a_click_on_a_record_expands_it() {
    let (mut domain, mut model, _) = worked_task();
    let area = Rect::new(0, 0, 100, 60);
    rows(&model, 100, 60);
    let hits = board_hit_map(area, &model);
    let earlier = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::TrailHeading)
        .expect("+ N earlier is clickable");
    let click = crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: earlier.area.x + 2,
        row: earlier.area.y,
        modifiers: KeyModifiers::NONE,
    };
    let intent = map_board_mouse(&model, &hits, click).expect("click maps");
    assert_eq!(intent, BoardIntent::ToggleTrail);
    apply_intent(&mut domain, &mut model, intent, None).expect("apply");
    assert!(trail(&rows(&model, 100, 60))[0].ends_with("a latest"));

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
        .any(|row| row.ends_with('▾')));
}

#[test]
fn the_trail_wraps_without_truncating_at_40_and_120_columns() {
    let (mut domain, mut model, _) = worked_task();
    press(
        &mut domain,
        &mut model,
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    );
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
