//! Blocks on the board: the block card, block-aware sections and rows, and the task page's
//! BLOCKED section with its reply box.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::domain::{
    BlockDraft, BlockOn, DomainState, HumanStatus, ProvenanceOrigin, TaskScope, OWNER,
};
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{
    apply_intent, board_intent_may_persist, draw_board, BlockTarget, BoardInputMode, BoardModel,
    IntentOutcome,
};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::queue::SectionKind;
use uuid::Uuid;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Desk tasks saved through a temp store, so each carries its `T` number.
fn numbered_domain(titles: &[&str]) -> (DomainState, Vec<Uuid>) {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tsk-board-blocks-{nanos}-{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut domain = DomainState::new();
    let ids = titles
        .iter()
        .map(|title| {
            domain
                .create(
                    *title,
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("create")
        })
        .collect();
    let store = TaskStore::new(&dir);
    store.save(&domain).expect("save");
    let domain = store.load().expect("load");
    let _ = std::fs::remove_dir_all(&dir);
    (domain, ids)
}

/// The size of the top undo entry when it is one batch.
fn last_batch_len(domain: &DomainState) -> Option<usize> {
    let value = serde_json::to_value(domain).expect("state");
    value["undo_stack"].as_array()?.last()?["batch"]["entries"]
        .as_array()
        .map(Vec::len)
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// Map one key through the board keymap for the model's current mode and apply it.
fn press(domain: &mut DomainState, model: &mut BoardModel, event: KeyEvent) -> IntentOutcome {
    let intent = map_key(model.input_mode(), event).expect("mapped key");
    let outcome = apply_intent(domain, model, intent, None).expect("apply");
    if outcome == IntentOutcome::Persist {
        model.sync_from_domain(domain);
    }
    outcome
}

fn type_text(domain: &mut DomainState, model: &mut BoardModel, text: &str) {
    for character in text.chars() {
        press(
            domain,
            model,
            key(KeyCode::Char(character), KeyModifiers::NONE),
        );
    }
}

fn select(domain: &mut DomainState, model: &mut BoardModel, id: Uuid) {
    let index = model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .expect("task visible");
    apply_intent(domain, model, BoardIntent::SelectIndex(index), None).expect("select");
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

fn section_ids(model: &BoardModel, kind: SectionKind) -> Vec<Uuid> {
    model
        .queue_view()
        .sections
        .iter()
        .filter(|section| section.kind == kind)
        .flat_map(|section| section.task_ids.clone())
        .collect()
}

#[test]
fn ctrl_b_card_blocks_on_another_task_which_rides_in_motion_until_that_task_is_done() {
    let (mut domain, ids) = numbered_domain(&["api", "client"]);
    let (api, client) = (ids[0], ids[1]);
    domain.set_status(api, HumanStatus::Started).unwrap();
    domain.set_status(client, HumanStatus::Started).unwrap();
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, client);

    let intent = map_key(
        model.input_mode(),
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    )
    .expect("ctrl+b");
    assert!(board_intent_may_persist(&model, &intent));
    assert_eq!(
        apply_intent(&mut domain, &mut model, intent, None).unwrap(),
        IntentOutcome::None,
        "blocking asks first"
    );
    assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
    let card = rows(&model, 80, 24).join("\n");
    assert!(card.contains("block T2"), "{card}");
    assert!(card.contains("‹ you · task · other ›"), "{card}");
    assert!(
        card.contains("enter block · tab next field · esc cancel"),
        "{card}"
    );

    type_text(&mut domain, &mut model, "needs the endpoint");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Tab, KeyModifiers::NONE),
    );
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Right, KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "T2");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::BlockCard,
        "T2 is itself"
    );
    assert!(rows(&model, 80, 24)
        .join("\n")
        .contains("T2 cannot wait on itself"));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Backspace, KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "1");
    assert_eq!(
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Enter, KeyModifiers::NONE)
        ),
        IntentOutcome::Persist
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let block = domain.get(client).unwrap().block.clone().expect("block");
    assert_eq!(block.why.as_deref(), Some("needs the endpoint"));
    assert_eq!(block.on, BlockOn::Task(1));
    assert_eq!(block.by, OWNER);
    assert_eq!(last_batch_len(&domain), Some(1));

    assert!(section_ids(&model, SectionKind::InMotion).contains(&client));
    let board = rows(&model, 80, 24);
    let row = board
        .iter()
        .find(|row| row.contains("client"))
        .expect("client row");
    assert!(row.contains("□"), "{row}");
    assert!(row.trim_end().ends_with("on T1"), "{row}");

    domain.complete(api).unwrap();
    model.sync_from_domain(&domain);
    assert!(section_ids(&model, SectionKind::NeedsYou).contains(&client));
    assert_eq!(domain.get(client).unwrap().status, HumanStatus::Blocked);
    assert_eq!(model.selected_id(), Some(client));
    apply_intent(&mut domain, &mut model, BoardIntent::PeekDetail, None).unwrap();
    let board = rows(&model, 80, 24).join("\n");
    assert!(board.contains("T1 done, unblock?"), "{board}");
    assert!(board.contains("why  needs the endpoint"), "{board}");
}

#[test]
fn an_empty_card_blocks_with_no_reason_and_esc_changes_nothing() {
    let (mut domain, ids) = numbered_domain(&["one"]);
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, ids[0]);
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    type_text(&mut domain, &mut model, "draft");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Esc, KeyModifiers::NONE),
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(domain.get(ids[0]).unwrap().status, HumanStatus::Open);

    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    let task = domain.get(ids[0]).unwrap();
    assert_eq!(task.status, HumanStatus::Blocked);
    assert_eq!(task.block.as_ref().unwrap().why, None);

    // ctrl+b on a blocked task still unblocks to ready, closing the block.
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    let task = domain.get(ids[0]).unwrap();
    assert_eq!(task.status, HumanStatus::Ready);
    assert!(task.block.is_none());
    assert_eq!(task.past_blocks.len(), 1);
}

#[test]
fn a_marked_set_gets_one_card_one_batch_and_one_undo() {
    let (mut domain, ids) = numbered_domain(&["a", "b", "c"]);
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).unwrap();
    for id in &ids[..2] {
        select(&mut domain, &mut model, *id);
        apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).unwrap();
    }
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    assert!(rows(&model, 80, 24).join("\n").contains("block 2 tasks"));
    assert_eq!(model.marked_count(), 2, "marks stay while the card is open");
    type_text(&mut domain, &mut model, "waiting on design");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    for id in &ids[..2] {
        let task = domain.get(*id).unwrap();
        assert_eq!(task.status, HumanStatus::Blocked);
        assert_eq!(
            task.block.as_ref().unwrap().why.as_deref(),
            Some("waiting on design")
        );
    }
    assert_eq!(domain.get(ids[2]).unwrap().status, HumanStatus::Open);
    assert_eq!(model.marked_count(), 0);
    assert_eq!(last_batch_len(&domain), Some(2));

    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('u'), KeyModifiers::CONTROL),
    );
    for id in &ids[..2] {
        let task = domain.get(*id).unwrap();
        assert_eq!(task.status, HumanStatus::Open);
        assert!(task.block.is_none() && task.past_blocks.is_empty());
    }
}

fn blocked_page() -> (DomainState, BoardModel, Uuid) {
    let (mut domain, ids) = numbered_domain(&["pick a database"]);
    let id = ids[0];
    domain
        .block(
            id,
            BlockDraft::from_input(
                Some("Which database?"),
                Some("a decision"),
                &["postgres".into(), "sqlite".into()],
                BlockOn::You,
            )
            .unwrap(),
            "claude",
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, id);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).unwrap();
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    (domain, model, id)
}

#[test]
fn the_page_leads_with_the_blocked_section_and_a_rule_above_the_notes() {
    let (_domain, model, _) = blocked_page();
    let page = rows(&model, 80, 24);
    let heading = page
        .iter()
        .position(|row| row.contains("BLOCKED · on you · @claude"))
        .unwrap_or_else(|| panic!("{}", page.join("\n")));
    assert!(
        page[heading].trim_end().ends_with("r reply"),
        "{}",
        page[heading]
    );
    let joined = page.join("\n");
    assert!(joined.contains("why    Which database?"), "{joined}");
    assert!(joined.contains("needs  a decision"), "{joined}");
    assert!(joined.contains("○ postgres"), "{joined}");
    let rule = heading
        + page[heading..]
            .iter()
            .position(|row| row.trim().chars().all(|c| c == '─') && row.trim().chars().count() > 60)
            .unwrap_or_else(|| panic!("{joined}"));
    let notes = page
        .iter()
        .position(|row| row.contains("no notes yet"))
        .unwrap_or_else(|| panic!("{joined}"));
    assert!(heading < rule && rule < notes, "{joined}");
}

#[test]
fn tab_reaches_an_option_enter_prefills_the_reply_and_shift_enter_answers() {
    let (mut domain, mut model, id) = blocked_page();
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Tab, KeyModifiers::NONE),
    );
    assert_eq!(model.block_target(), Some(BlockTarget::Heading));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Tab, KeyModifiers::NONE),
    );
    assert_eq!(model.block_target(), Some(BlockTarget::Option(0)));
    assert!(rows(&model, 80, 24)
        .iter()
        .any(|row| row.contains("▸ ○ postgres")));
    assert!(model.block_option_selected());
    apply_intent(&mut domain, &mut model, BoardIntent::ReplyWithOption, None).unwrap();
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert_eq!(model.reply_draft(), Some("postgres"));
    type_text(&mut domain, &mut model, ", please");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "thanks");
    assert_eq!(model.reply_draft(), Some("postgres, please\nthanks"));
    assert!(rows(&model, 80, 24).join("\n").contains("shift+enter save"));

    let intent = map_key(model.input_mode(), key(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
    assert!(board_intent_may_persist(&model, &intent));
    assert_eq!(
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Enter, KeyModifiers::SHIFT)
        ),
        IntentOutcome::Persist
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    let block = domain.get(id).unwrap().block.clone().unwrap();
    assert_eq!(block.replies[0].text, "postgres, please\nthanks");
    assert_eq!(block.replies[0].by, OWNER);
    assert!(block.answered());
    assert_eq!(domain.get(id).unwrap().status, HumanStatus::Blocked);
    let page = rows(&model, 80, 24).join("\n");
    assert!(page.contains("└ you"), "{page}");

    press(
        &mut domain,
        &mut model,
        key(KeyCode::Esc, KeyModifiers::NONE),
    );
    let board = rows(&model, 80, 24);
    let row = board
        .iter()
        .find(|row| row.contains("pick a database"))
        .unwrap();
    assert!(row.trim_end().ends_with("answered"), "{row}");
}

#[test]
fn an_unanswered_agent_block_shows_who_asks_on_the_row() {
    let (_domain, mut model, _) = blocked_page();
    let mut domain = _domain;
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Esc, KeyModifiers::NONE),
    );
    let board = rows(&model, 80, 24);
    let row = board
        .iter()
        .find(|row| row.contains("pick a database"))
        .unwrap();
    assert!(row.contains("■"), "{row}");
    assert!(row.trim_end().ends_with("@claude ?"), "{row}");
}

#[test]
fn ctrl_s_in_the_reply_box_saves_and_unblocks_to_ready() {
    let (mut domain, mut model, id) = blocked_page();
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('r'), KeyModifiers::NONE),
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    // An empty reply refuses in the box and keeps it open.
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert!(rows(&model, 80, 24)
        .join("\n")
        .contains("type a reply first"));
    type_text(&mut domain, &mut model, "go with sqlite");
    assert_eq!(
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        ),
        IntentOutcome::Persist
    );
    let task = domain.get(id).unwrap();
    assert_eq!(task.status, HumanStatus::Ready);
    assert!(task.block.is_none());
    assert_eq!(task.past_blocks[0].replies[0].text, "go with sqlite");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    let page = rows(&model, 80, 24).join("\n");
    assert!(!page.contains("BLOCKED"), "{page}");
}

#[test]
fn ctrl_x_soft_deletes_your_reply_and_refuses_an_agents_never_the_task() {
    let (mut domain, mut model, id) = blocked_page();
    domain.reply(id, "more context", "claude").unwrap();
    domain.reply(id, "noted", OWNER).unwrap();
    model.sync_from_domain(&domain);
    // Heading, two options, then the agent's reply and yours.
    for _ in 0..4 {
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Tab, KeyModifiers::NONE),
        );
    }
    assert_eq!(model.block_target(), Some(BlockTarget::Reply(0)));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('x'), KeyModifiers::CONTROL),
    );
    assert!(!domain.get(id).unwrap().soft_deleted);
    assert!(!domain.get(id).unwrap().block.as_ref().unwrap().replies[0].deleted);
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "agent replies are frozen"
    );

    press(
        &mut domain,
        &mut model,
        key(KeyCode::Tab, KeyModifiers::NONE),
    );
    assert_eq!(model.block_target(), Some(BlockTarget::Reply(1)));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert_eq!(model.reply_draft(), Some("noted"));
    type_text(&mut domain, &mut model, " twice");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    let reply = &domain.get(id).unwrap().block.as_ref().unwrap().replies[1];
    assert_eq!(reply.text, "noted twice");
    assert!(reply.edited);

    assert_eq!(model.block_target(), Some(BlockTarget::Reply(1)));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('x'), KeyModifiers::CONTROL),
    );
    let task = domain.get(id).unwrap();
    assert!(!task.soft_deleted, "the task is never the target");
    assert!(task.block.as_ref().unwrap().replies[1].deleted);
    assert!(rows(&model, 80, 30).join("\n").contains("deleted"));
}

#[test]
fn ctrl_e_on_the_heading_edits_why_and_keeps_the_options() {
    let (mut domain, mut model, id) = blocked_page();
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Tab, KeyModifiers::NONE),
    );
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );
    assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
    assert!(rows(&model, 80, 24).join("\n").contains("edit block T1"));
    type_text(&mut domain, &mut model, " Really?");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    let block = domain.get(id).unwrap().block.clone().unwrap();
    assert_eq!(block.why.as_deref(), Some("Which database? Really?"));
    assert_eq!(block.options.len(), 2, "the card has no options field");
    assert!(block.edited_at.is_some());
    assert_eq!(block.by, "claude", "an agent's block stays the agent's");
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn shift_tab_from_the_first_step_climbs_back_into_the_section() {
    let (mut domain, mut model, id) = blocked_page();
    domain.add_step(id, "first").unwrap();
    model.sync_from_domain(&domain);
    for _ in 0..4 {
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Tab, KeyModifiers::NONE),
        );
    }
    assert_eq!(
        model.block_target(),
        None,
        "past the two options: the first step"
    );
    press(
        &mut domain,
        &mut model,
        key(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    assert_eq!(model.block_target(), Some(BlockTarget::Option(1)));
}

#[test]
fn a_cancelled_failed_reply_save_keeps_the_box_and_its_text() {
    let (mut domain, mut model, _) = blocked_page();
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('r'), KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "keep me");
    let intent = map_key(model.input_mode(), key(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
    let baseline = domain.clone();
    assert_eq!(
        apply_intent(&mut domain, &mut model, intent, None).unwrap(),
        IntentOutcome::Persist
    );
    // The save fails and is cancelled: the baseline comes back and the box stays put.
    model.begin_save_recovery("disk full");
    let mut domain = baseline;
    model.sync_from_domain(&domain);
    model.end_save_recovery(tsk_tui::ui::board::SaveResolution::Cancelled);
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert_eq!(model.reply_draft(), Some("keep me"));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "a retry lands"
    );
}

#[test]
fn reply_and_unblock_saves_through_the_locked_store_as_one_change() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "tsk-board-blocks-save-{nanos}-{}",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let store = TaskStore::new(&dir);
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "db",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    domain
        .block(id, BlockDraft::default(), "claude")
        .expect("block");
    store.save(&domain).expect("save blocked");
    let mut domain = store.load().expect("load");
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, id);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).unwrap();

    for (unblock, text) in [(false, "first"), (true, "second")] {
        press(
            &mut domain,
            &mut model,
            key(KeyCode::Char('r'), KeyModifiers::NONE),
        );
        type_text(&mut domain, &mut model, text);
        let chord = if unblock {
            key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        } else {
            key(KeyCode::Enter, KeyModifiers::SHIFT)
        };
        let intent = map_key(model.input_mode(), chord).unwrap();
        assert_eq!(
            apply_intent(&mut domain, &mut model, intent, None).unwrap(),
            IntentOutcome::Persist
        );
        // The board's locked merge-save: the task must still be based on the disk revision.
        store
            .reload_merge_save(&mut domain)
            .unwrap_or_else(|error| panic!("{text}: {error}"));
        domain = store.load().expect("reload");
        model.sync_from_domain(&domain);
    }
    let task = domain.get(id).unwrap();
    assert_eq!(task.status, HumanStatus::Ready);
    let texts: Vec<_> = task.past_blocks[0]
        .replies
        .iter()
        .map(|reply| reply.text.as_str())
        .collect();
    assert_eq!(texts, ["first", "second"]);
    let _ = std::fs::remove_dir_all(&dir);
}
