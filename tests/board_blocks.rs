//! Blocks on the board: the block card, block-aware sections and rows, and the task page's
//! BLOCKED section with its reply box.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::app::{apply_board_intent_with_save_recovery, BoardSaveContext};
use tsk_tui::domain::{
    BlockDraft, BlockOn, DomainState, HumanStatus, ProvenanceOrigin, TaskScope, OWNER,
};
use tsk_tui::save_recovery::SaveRecovery;
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
    let at = board
        .iter()
        .position(|row| row.contains("client"))
        .expect("client row");
    assert!(board[at].contains("□"), "{}", board[at]);
    assert!(board[at].trim_end().ends_with("client"), "{}", board[at]);
    // The open peek leads with the live line's text, then the why.
    assert_eq!(board[at + 1].trim_end(), "    │ waiting on T1");
    assert_eq!(board[at + 2].trim_end(), "    │ needs the endpoint");

    domain.complete(api).unwrap();
    model.sync_from_domain(&domain);
    assert!(section_ids(&model, SectionKind::NeedsYou).contains(&client));
    assert_eq!(domain.get(client).unwrap().status, HumanStatus::Blocked);
    assert_eq!(model.selected_id(), Some(client));
    let board = rows(&model, 80, 24).join("\n");
    assert!(board.contains("│ T1 is done · unblock it · "), "{board}");
    assert!(board.contains("│ needs the endpoint"), "{board}");
    assert!(
        !board.contains("└─ T1 is done"),
        "the peek replaces the live line: {board}"
    );
    // Closing the peek brings the live line back.
    apply_intent(&mut domain, &mut model, BoardIntent::CollapseDetail, None).unwrap();
    let board = rows(&model, 80, 24).join("\n");
    assert!(
        board.contains("    └─ T1 is done · unblock it · "),
        "{board}"
    );
    assert!(!board.contains("needs the endpoint"), "{board}");
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

/// The page rows between the header rule and the footer, without the gutter's scrollbar.
fn page_body(page: &[String]) -> Vec<String> {
    page.iter()
        .map(|row| row.trim_end_matches(['▌', ' ']).to_string())
        .collect()
}

#[test]
fn the_page_leads_with_the_blocked_section_and_a_rule_above_the_notes() {
    let (_domain, model, _) = blocked_page();
    let page = page_body(&rows(&model, 80, 30));
    let joined = page.join("\n");
    let top = page
        .iter()
        .position(|row| row.starts_with("  @claude blocked on you · "))
        .unwrap_or_else(|| panic!("{joined}"));
    // Plain body text, no labels; numbered options in a fixed column; then the dim action line
    // as the last row before the rule.
    assert_eq!(
        page[top + 1..top + 8],
        [
            "",
            "  Which database?",
            "  a decision",
            "",
            "   1  postgres",
            "   2  sqlite",
            "",
        ],
        "{joined}"
    );
    assert_eq!(
        page[top + 8],
        "  1-2 choose · r reply · ctrl+s reply + unblock",
        "{joined}"
    );
    let rule = page[top + 9].trim();
    assert!(
        rule.chars().all(|c| c == '─') && rule.chars().count() > 60,
        "{joined}"
    );
    let notes = page
        .iter()
        .position(|row| row.contains("no notes yet"))
        .unwrap_or_else(|| panic!("{joined}"));
    assert!(top + 9 < notes, "{joined}");
    for gone in ["BLOCKED", "why ", "needs ", "○", "r reply ─"] {
        assert!(!joined.contains(gone), "{gone}:\n{joined}");
    }
    assert_eq!(model.block_target(), None, "a blocked page opens with nothing selected");
}

#[test]
fn number_keys_pick_an_option_and_open_the_reply_box_prefilled() {
    let (mut domain, mut model, id) = blocked_page();
    let three = map_key(model.input_mode(), key(KeyCode::Char('3'), KeyModifiers::NONE));
    assert_eq!(three, Some(BoardIntent::PickOption(2)));
    // No third option: inert, and nothing persists.
    assert!(!board_intent_may_persist(&model, &three.clone().unwrap()));
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('3'), KeyModifiers::NONE),
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('2'), KeyModifiers::NONE),
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert_eq!(model.reply_draft(), Some("sqlite"));
    // The box replaces the action line with its own keys.
    let page = page_body(&rows(&model, 80, 30)).join("\n");
    assert!(
        page.contains("  shift+enter save · ctrl+s save + unblock · esc cancel\n  ───"),
        "{page}"
    );
    assert!(!page.contains("1-2 choose"), "{page}");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    let block = domain.get(id).unwrap().block.clone().unwrap();
    assert_eq!(block.replies[0].text, "sqlite");
    // A digit in the box is text.
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('r'), KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "1");
    assert_eq!(model.reply_draft(), Some("1"));
}

#[test]
fn the_action_line_follows_the_cursor() {
    let (mut domain, mut model, id) = blocked_page();
    domain.add_step(id, "migrate").unwrap();
    model.sync_from_domain(&domain);
    let action = |model: &BoardModel| {
        let page = page_body(&rows(model, 90, 40));
        // The section's rule, after the header's.
        let rule = page
            .iter()
            .rposition(|row| row.starts_with("  ───"))
            .expect("rule");
        page[rule - 1].trim().to_string()
    };
    assert_eq!(action(&model), "1-2 choose · r reply · ctrl+s reply + unblock");
    press(&mut domain, &mut model, key(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(model.block_target(), Some(BlockTarget::Heading));
    assert_eq!(
        action(&model),
        "ctrl+e edit · 1-2 choose · r reply · ctrl+s reply + unblock"
    );
    press(&mut domain, &mut model, key(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(
        action(&model),
        "enter choose · 1-2 choose · r reply · ctrl+s reply + unblock"
    );
    for _ in 0..2 {
        press(&mut domain, &mut model, key(KeyCode::Tab, KeyModifiers::NONE));
    }
    assert!(model.stored_step_selected());
    assert_eq!(
        action(&model),
        "enter toggle step · 1-2 choose · r reply · ctrl+s reply + unblock"
    );
}

#[test]
fn the_thread_keeps_names_in_a_fixed_column_and_wraps_at_40_and_110() {
    let (mut domain, mut model, id) = blocked_page();
    tsk_tui::domain::acting_as("claude", || {
        domain
            .reply(id, "Recommend postgres; it is the smaller change.", "claude")
            .unwrap();
    });
    domain
        .reply(id, "Go with postgres, keep the worker running.", OWNER)
        .unwrap();
    model.sync_from_domain(&domain);
    let wide = page_body(&rows(&model, 110, 40));
    let claude = wide
        .iter()
        .find(|row| row.starts_with("  claude · "))
        .unwrap_or_else(|| panic!("{}", wide.join("\n")));
    let you = wide
        .iter()
        .find(|row| row.starts_with("  you · "))
        .unwrap_or_else(|| panic!("{}", wide.join("\n")));
    assert_eq!(
        claude.find("Recommend"),
        you.find("Go with"),
        "one column:\n{}",
        wide.join("\n")
    );
    assert!(!wide.iter().any(|row| row.contains('└') || row.contains("@claude ·")));

    let narrow = page_body(&rows(&model, 40, 60));
    let section: Vec<String> = narrow
        .iter()
        .take_while(|row| !row.contains("no notes yet"))
        .cloned()
        .collect();
    let joined = section.join(" ");
    assert!(!joined.contains('…'), "{joined}");
    let claude = section
        .iter()
        .find(|row| row.starts_with("  claude · "))
        .unwrap_or_else(|| panic!("{}", narrow.join("\n")));
    let you = section
        .iter()
        .find(|row| row.starts_with("  you · "))
        .unwrap_or_else(|| panic!("{}", narrow.join("\n")));
    assert_eq!(claude.find("Recommend"), you.find("Go with"));
    for text in [
        "Recommend postgres;",
        "change.",
        "running.",
        "1-2 choose · r reply",
        "ctrl+s reply + unblock",
    ] {
        assert!(joined.contains(text), "{text}:\n{}", narrow.join("\n"));
    }
}

#[test]
fn a_block_on_another_task_says_so_on_top_and_offers_ctrl_b() {
    let (mut domain, ids) = numbered_domain(&["wait for it", "the other one"]);
    domain
        .block(
            ids[0],
            BlockDraft::from_input(Some("needs the schema"), None, &[], BlockOn::Task(2)).unwrap(),
            OWNER,
        )
        .unwrap();
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, ids[0]);
    apply_intent(&mut domain, &mut model, BoardIntent::OpenTaskPage, None).unwrap();
    let page = page_body(&rows(&model, 80, 30));
    let joined = page.join("\n");
    assert!(joined.contains("  waiting on T2\n\n  needs the schema"), "{joined}");
    assert!(joined.contains("  r reply · ctrl+b unblock\n  ───"), "{joined}");
    assert!(!joined.contains("choose"), "{joined}");
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
        .any(|row| row.starts_with("▸  1  postgres")));
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
    assert!(page.contains("▸ you · 0s   postgres, please"), "{page}");

    press(
        &mut domain,
        &mut model,
        key(KeyCode::Esc, KeyModifiers::NONE),
    );
    // The block is still yours to clear: the live line does not change after you answer.
    let board = rows(&model, 80, 24);
    let at = board
        .iter()
        .position(|row| row.contains("pick a database"))
        .unwrap();
    assert!(
        board[at].trim_end().ends_with("pick a database"),
        "{}",
        board[at]
    );
    assert!(
        board[at + 1].starts_with("    └─ @claude blocked on you · "),
        "{}",
        board[at + 1]
    );
}

#[test]
fn an_agent_block_says_who_asks_on_the_live_line() {
    let (_domain, mut model, _) = blocked_page();
    let mut domain = _domain;
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Esc, KeyModifiers::NONE),
    );
    let board = rows(&model, 80, 24);
    let at = board
        .iter()
        .position(|row| row.contains("pick a database"))
        .unwrap();
    assert!(board[at].contains("■"), "{}", board[at]);
    assert!(
        board[at].trim_end().ends_with("pick a database"),
        "{}",
        board[at]
    );
    assert!(
        board[at + 1].starts_with("    └─ @claude blocked on you · "),
        "{}",
        board[at + 1]
    );
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

/// Apply one intent through the board's save boundary. `fail` decides whether this persist
/// call fails; `calls` counts every persist call.
fn apply_saving(
    domain: &mut DomainState,
    model: &mut BoardModel,
    recovery: &mut SaveRecovery<DomainState>,
    intent: BoardIntent,
    fail: bool,
    calls: &Cell<usize>,
) -> IntentOutcome {
    let baseline = domain.clone();
    apply_board_intent_with_save_recovery(
        domain,
        model,
        recovery,
        BoardSaveContext {
            baseline,
            intent,
            snapshot: None,
        },
        |_| {
            calls.set(calls.get() + 1);
            if fail {
                Err("disk full".to_string())
            } else {
                Ok(())
            }
        },
    )
    .expect("apply")
}

fn enter() -> BoardIntent {
    map_key(
        BoardInputMode::BlockCard,
        key(KeyCode::Enter, KeyModifiers::NONE),
    )
    .expect("enter")
}

fn marked_card(titles: &[&str], marked: usize) -> (DomainState, BoardModel, Vec<Uuid>) {
    let (mut domain, ids) = numbered_domain(titles);
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    apply_intent(&mut domain, &mut model, BoardIntent::ToggleMarkMode, None).unwrap();
    for id in &ids[..marked] {
        select(&mut domain, &mut model, *id);
        apply_intent(&mut domain, &mut model, BoardIntent::MarkToggle, None).unwrap();
    }
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('b'), KeyModifiers::CONTROL),
    );
    type_text(&mut domain, &mut model, "waiting on design");
    (domain, model, ids)
}

#[test]
fn a_cancelled_failed_card_save_keeps_the_card_its_why_and_the_marked_set() {
    let (mut domain, mut model, ids) = marked_card(&["a", "b", "c"], 2);
    let mut recovery = SaveRecovery::new();
    let calls = Cell::new(0);

    let outcome = apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        enter(),
        true,
        &calls,
    );
    assert_eq!(outcome, IntentOutcome::None);
    assert!(recovery.is_pending());
    assert!(
        model.block_card().is_some(),
        "the card outlives the failed save"
    );
    assert_eq!(model.marked_count(), 2);

    apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardIntent::CancelSave,
        false,
        &calls,
    );
    assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
    assert!(rows(&model, 80, 24)
        .join("\n")
        .contains("waiting on design"));
    assert_eq!(model.marked_count(), 2, "the marked set survives Cancel");
    assert!(ids
        .iter()
        .all(|id| domain.get(*id).unwrap().status == HumanStatus::Open));

    calls.set(0);
    let outcome = apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        enter(),
        false,
        &calls,
    );
    assert_eq!(outcome, IntentOutcome::Persisted);
    assert_eq!(calls.get(), 1, "one save for the whole set");
    for id in &ids[..2] {
        assert_eq!(domain.get(*id).unwrap().status, HumanStatus::Blocked);
    }
    assert_eq!(last_batch_len(&domain), Some(2));
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert!(model.block_card().is_none());
    assert_eq!(
        model.marked_count(),
        0,
        "the marks clear once the save lands"
    );
}

#[test]
fn a_retried_failed_card_save_blocks_the_whole_set_with_one_persist() {
    let (mut domain, mut model, ids) = marked_card(&["a", "b", "c"], 2);
    let mut recovery = SaveRecovery::new();
    let calls = Cell::new(0);
    apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        enter(),
        true,
        &calls,
    );
    assert!(recovery.is_pending());

    calls.set(0);
    let outcome = apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardIntent::RetrySave,
        false,
        &calls,
    );
    assert_eq!(outcome, IntentOutcome::Persisted);
    assert_eq!(calls.get(), 1);
    for id in &ids[..2] {
        let task = domain.get(*id).unwrap();
        assert_eq!(task.status, HumanStatus::Blocked);
        assert_eq!(
            task.block.as_ref().unwrap().why.as_deref(),
            Some("waiting on design")
        );
    }
    assert_eq!(domain.get(ids[2]).unwrap().status, HumanStatus::Open);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert!(model.block_card().is_none());
    assert_eq!(model.marked_count(), 0);
}

#[test]
fn a_cancelled_failed_edit_card_save_keeps_the_card_and_its_text() {
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
    type_text(&mut domain, &mut model, " Really?");
    let mut recovery = SaveRecovery::new();
    let calls = Cell::new(0);
    apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        enter(),
        true,
        &calls,
    );
    assert!(recovery.is_pending());
    apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        BoardIntent::CancelSave,
        false,
        &calls,
    );
    assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
    assert!(rows(&model, 80, 24)
        .join("\n")
        .contains("Which database? Really?"));
    assert_eq!(
        domain
            .get(id)
            .unwrap()
            .block
            .as_ref()
            .unwrap()
            .why
            .as_deref(),
        Some("Which database?")
    );

    calls.set(0);
    let outcome = apply_saving(
        &mut domain,
        &mut model,
        &mut recovery,
        enter(),
        false,
        &calls,
    );
    assert_eq!(outcome, IntentOutcome::Persisted);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        domain
            .get(id)
            .unwrap()
            .block
            .as_ref()
            .unwrap()
            .why
            .as_deref(),
        Some("Which database? Really?")
    );
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn a_reply_box_refuses_to_save_onto_a_block_that_replaced_its_own() {
    let (mut domain, mut model, id) = blocked_page();
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('r'), KeyModifiers::NONE),
    );
    type_text(&mut domain, &mut model, "postgres");
    // Another board or the CLI closes the block and opens a new one while the owner types.
    domain.set_status(id, HumanStatus::Ready).unwrap();
    domain
        .block(
            id,
            BlockDraft::from_input(Some("Which region?"), None, &[], BlockOn::You).unwrap(),
            "claude",
        )
        .unwrap();
    model.sync_from_domain(&domain);
    let before = domain.clone();

    let outcome = press(
        &mut domain,
        &mut model,
        key(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(
        serde_json::to_value(&domain).unwrap(),
        serde_json::to_value(&before).unwrap(),
        "nothing landed on the new block"
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    assert_eq!(model.reply_draft(), Some("postgres"), "the draft is kept");
    assert!(
        model
            .message()
            .is_some_and(|message| message.contains("closed or replaced")),
        "{:?}",
        model.message()
    );
}

#[test]
fn editing_a_reply_by_index_refuses_once_its_block_was_replaced() {
    let (mut domain, mut model, id) = blocked_page();
    domain.reply(id, "mine", OWNER).unwrap();
    model.sync_from_domain(&domain);
    // Heading, two options, then your reply.
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
        key(KeyCode::Char('e'), KeyModifiers::CONTROL),
    );
    assert_eq!(model.input_mode(), BoardInputMode::EditReply);
    type_text(&mut domain, &mut model, " edited");
    // The block is replaced by one whose first reply is a different answer of yours.
    domain.set_status(id, HumanStatus::Ready).unwrap();
    domain.block(id, BlockDraft::default(), "claude").unwrap();
    domain.reply(id, "another answer", OWNER).unwrap();
    model.sync_from_domain(&domain);

    let outcome = press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::SHIFT),
    );
    assert_eq!(outcome, IntentOutcome::None);
    let block = domain.get(id).unwrap().block.clone().unwrap();
    assert_eq!(block.replies[0].text, "another answer");
    assert!(!block.replies[0].edited);
    assert_eq!(model.reply_draft(), Some("mine edited"));
}

#[test]
fn palette_set_status_blocked_opens_the_block_card() {
    let (mut domain, ids) = numbered_domain(&["a"]);
    let mut model = BoardModel::from_domain(&domain, None);
    model.sync_from_domain(&domain);
    select(&mut domain, &mut model, ids[0]);
    let outcome = apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Blocked),
        None,
    )
    .unwrap();
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::BlockCard);
    assert_eq!(domain.get(ids[0]).unwrap().status, HumanStatus::Open);
    type_text(&mut domain, &mut model, "asked legal");
    press(
        &mut domain,
        &mut model,
        key(KeyCode::Enter, KeyModifiers::NONE),
    );
    let task = domain.get(ids[0]).unwrap();
    assert_eq!(task.status, HumanStatus::Blocked);
    assert_eq!(
        task.block.as_ref().unwrap().why.as_deref(),
        Some("asked legal")
    );

    // On a task already blocked it does nothing, never unblocks.
    let outcome = apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SetStatus(HumanStatus::Blocked),
        None,
    )
    .unwrap();
    assert_eq!(outcome, IntentOutcome::None);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(domain.get(ids[0]).unwrap().status, HumanStatus::Blocked);
}
