//! T252: a footer never cuts an entry. Every verb bar and card legend, on every surface, at
//! 40, 58, 78 and 110 columns, paints whole `key label` entries in their declared order and
//! keeps its essentials; whatever does not fit is dropped whole.

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use tsk_tui::domain::{
    BlockDraft, BlockOn, DomainState, HumanStatus, ProvenanceOrigin, ReviewDraft, TaskScope,
};
use tsk_tui::ui::board::{apply_intent, board_verb_items, draw_board, BoardInputMode, BoardModel};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::BoardIntent;
use tsk_tui::ui::tier;

const REPO: &str = "/repos/app";
const WIDTHS: [u16; 4] = [40, 58, 78, 110];

/// One declared footer entry as painted: key, full label, the short label it may fall back
/// to, and whether it is essential.
#[derive(Debug, Clone)]
struct Entry {
    key: String,
    label: String,
    short: Option<&'static str>,
    essential: bool,
}

fn entry(key: &str, label: &str, essential: bool) -> Entry {
    Entry {
        key: key.to_string(),
        label: label.to_string(),
        short: None,
        essential,
    }
}

/// The board's own verb list for `model`, with the `ctrl+` the bar paints on mutating keys
/// when `prefix` is set (the board and the task page's view mode).
fn board_entries(model: &BoardModel, prefix: bool) -> Vec<Entry> {
    board_verb_items(model)
        .into_iter()
        .map(|verb| {
            let mutating = matches!(
                verb.key,
                "s" | "d" | "o" | "b" | "r" | "x" | "a" | "e" | "u" | "n" | "f" | "g" | "q"
            );
            Entry {
                key: if prefix && mutating {
                    format!("ctrl+{}", verb.key)
                } else {
                    verb.key.to_string()
                },
                label: verb.label.to_string(),
                short: match verb.label {
                    "save step" | "add step" => Some("step"),
                    "save all" => Some("save"),
                    _ => None,
                },
                essential: verb.shed == 0,
            }
        })
        .collect()
}

fn quick_add() -> Vec<Entry> {
    vec![
        entry("enter", "add", true),
        entry("tab", "notes", false),
        entry("esc", "close", true),
    ]
}

fn notes_form() -> Vec<Entry> {
    vec![
        entry("shift+enter", "save", true),
        entry("tab", "next", false),
        entry("esc", "cancel", true),
        entry("enter", "new line", false),
    ]
}

fn thread_form() -> Vec<Entry> {
    vec![
        entry("shift+enter", "save", true),
        entry("tab", "next", false),
        entry("esc", "cancel", true),
    ]
}

fn scope_form() -> Vec<Entry> {
    vec![
        entry("enter", "choose", true),
        entry("space", "cycle", false),
        entry("esc", "cancel", true),
    ]
}

fn assignee_form() -> Vec<Entry> {
    vec![
        entry("space/←→", "cycle", false),
        entry("enter", "choose", true),
        entry("esc", "cancel", true),
    ]
}

fn base_form() -> Vec<Entry> {
    vec![entry("enter", "choose", true), entry("esc", "cancel", true)]
}

fn title_form() -> Vec<Entry> {
    vec![
        entry("enter", "next", false),
        entry("shift+enter", "save", true),
        entry("esc", "cancel", true),
    ]
}

/// Every card legend, by the card that paints it.
fn card_legends() -> Vec<(&'static str, Vec<Entry>)> {
    vec![
        (
            "help",
            vec![
                entry("type", "search", false),
                entry("↑↓", "scroll", false),
                entry("esc", "clear / close", true),
            ],
        ),
        (
            "palette",
            vec![
                entry("↑↓", "move", false),
                entry("enter", "run", true),
                entry("esc", "close", true),
            ],
        ),
        (
            "project picker",
            vec![
                entry("↑↓", "move", false),
                entry("enter", "choose", false),
                entry("s", "sort: name A–Z", false),
                entry("ctrl+f", "archive", true),
                entry("esc", "close", true),
            ],
        ),
        (
            "archived tab",
            vec![
                entry("ctrl+u", "unarchive", true),
                entry("enter", "open", false),
                entry("s", "sort: name A–Z", false),
                entry("esc", "close", true),
            ],
        ),
        (
            "filter picker",
            vec![
                entry("tab", "switch", true),
                entry("↑↓", "move", false),
                entry("enter", "choose", false),
                entry("esc", "close", true),
            ],
        ),
        (
            "list picker",
            vec![
                entry("↑↓", "move", false),
                entry("enter", "choose", true),
                entry("esc", "close", true),
            ],
        ),
        (
            "block card",
            vec![
                entry("enter", "block", true),
                entry("tab", "next field", false),
                entry("esc", "cancel", true),
            ],
        ),
        (
            "review card",
            vec![
                entry("enter", "review", true),
                entry("shift+enter", "new check", false),
                entry("tab", "next field", false),
                entry("esc", "cancel", true),
            ],
        ),
    ]
}

/// Why `row` is not whole `entries`: a cut, an unknown or reordered piece, or a missing
/// essential. `None` when the row is a clean fit.
fn misfit(row: &str, entries: &[Entry], width: u16) -> Option<String> {
    let body = row.trim();
    if body.is_empty() {
        return Some("the footer is empty".to_string());
    }
    if body.contains('…') {
        return Some(format!("{body:?} cuts an entry"));
    }
    let mut next = 0;
    let mut shown = Vec::new();
    for piece in body.split(" · ") {
        let found = entries.iter().enumerate().skip(next).find(|(_, entry)| {
            piece == format!("{} {}", entry.key, entry.label)
                || entry
                    .short
                    .is_some_and(|short| piece == format!("{} {short}", entry.key))
        });
        match found {
            Some((index, _)) => {
                shown.push(index);
                next = index + 1;
            }
            None => return Some(format!("{piece:?} in {body:?} is not a whole entry")),
        }
    }
    // `: commands` is the one essential a row too narrow for every essential gives up.
    let essentials: Vec<&Entry> = entries.iter().filter(|entry| entry.essential).collect();
    let essentials_width = 1
        + essentials
            .iter()
            .map(|entry| entry.key.chars().count() + 1 + entry.label.chars().count())
            .sum::<usize>()
        + 3 * essentials.len().saturating_sub(1);
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !(entry.key == ":" && essentials_width > usize::from(width)))
        .find(|(index, entry)| entry.essential && !shown.contains(index))
        .map(|(_, entry)| format!("{body:?} drops essential {} {}", entry.key, entry.label))
}

fn rows(model: &BoardModel, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = draw_board(frame, model);
        })
        .expect("draw board");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

fn height(width: u16) -> u16 {
    if width >= 110 {
        30
    } else {
        24
    }
}

/// A started task with steps and notes and an open one, both in one project.
fn board() -> (DomainState, BoardModel) {
    let (domain, _) = board_domain();
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO)));
    (domain, model)
}

fn board_domain() -> (DomainState, uuid::Uuid) {
    let mut domain = DomainState::new();
    let project = TaskScope::Project {
        path: REPO.to_string(),
    };
    let started = domain
        .create(
            "Wire the footer fitter through every surface",
            Some("notes".to_string()),
            project.clone(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create started");
    domain.add_step(started, "first step").expect("add step");
    domain
        .set_status(started, HumanStatus::Started)
        .expect("start");
    domain
        .create(
            "An inbox task",
            None,
            project,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create open");
    (domain, started)
}

/// The board's project archived, the board still on its working lens.
fn archived_board() -> (DomainState, BoardModel) {
    let (mut domain, _) = board_domain();
    domain.archive_project(REPO).expect("archive project");
    let model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO)));
    (domain, model)
}

/// The board's project archived and opened read-only from the picker's archived tab.
fn archived_focus() -> (DomainState, BoardModel) {
    let (mut domain, mut model) = archived_board();
    for intent in [
        BoardIntent::OpenProjectSelector,
        BoardIntent::ProjectPickerSwitchTab,
        BoardIntent::ConfirmProjectChoice,
    ] {
        apply_intent(&mut domain, &mut model, intent, None).expect("open the archived focus");
    }
    assert!(model.focus_is_archived(), "scene setup: archived focus");
    (domain, model)
}

/// The started task blocked on you, with options, its page open.
fn blocked_page() -> (DomainState, BoardModel) {
    let (mut domain, id) = board_domain();
    let draft = BlockDraft::from_input(
        Some("Which database?"),
        Some("a decision"),
        &["postgres".into(), "sqlite".into()],
        BlockOn::You,
    )
    .expect("block draft");
    domain.block(id, draft, "claude").expect("block");
    page_of(domain, id)
}

/// The started task handed back for review with checks, its page open.
fn review_page() -> (DomainState, BoardModel) {
    let (mut domain, id) = board_domain();
    let draft = ReviewDraft::from_input(
        Some("wired the fitter"),
        &["footers at 40".into(), "legends in borders".into()],
        Some("merge"),
        BlockOn::You,
    )
    .expect("review draft");
    domain.review(id, draft, "claude").expect("review");
    page_of(domain, id)
}

fn page_of(mut domain: DomainState, id: uuid::Uuid) -> (DomainState, BoardModel) {
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO)));
    let index = model
        .visible_ids()
        .iter()
        .position(|visible| *visible == id)
        .expect("task visible");
    for intent in [BoardIntent::SelectIndex(index), BoardIntent::OpenTaskPage] {
        apply_intent(&mut domain, &mut model, intent, None).expect("open the page");
    }
    (domain, model)
}

fn drive(
    setup: fn() -> (DomainState, BoardModel),
    intents: &[BoardIntent],
) -> (DomainState, BoardModel) {
    let (mut domain, mut model) = setup();
    for intent in intents {
        apply_intent(&mut domain, &mut model, intent.clone(), None)
            .unwrap_or_else(|err| panic!("{intent:?}: {err:?}"));
    }
    (domain, model)
}

/// The verb bar (the frame's last row) of every bar-owning surface.
#[test]
fn every_verb_bar_paints_whole_entries_and_keeps_its_essentials_at_every_width() {
    type Setup = fn() -> (DomainState, BoardModel);
    type Expect = fn(&BoardModel) -> Vec<Entry>;
    let edit = |field| {
        vec![
            BoardIntent::OpenTaskPage,
            BoardIntent::BeginEditTitle,
            BoardIntent::FocusFormField(field),
        ]
    };
    type Scene = (
        &'static str,
        Setup,
        Vec<BoardIntent>,
        Option<BoardInputMode>,
        Expect,
    );
    let scenes: Vec<Scene> = vec![
        (
            "board",
            board,
            vec![],
            Some(BoardInputMode::Normal),
            |model| board_entries(model, true),
        ),
        (
            "board on an inbox row",
            board,
            vec![BoardIntent::SelectNext],
            Some(BoardInputMode::Normal),
            |model| board_entries(model, true),
        ),
        (
            "archived focus",
            archived_focus,
            vec![],
            Some(BoardInputMode::Normal),
            |model| board_entries(model, true),
        ),
        (
            "search",
            board,
            vec![BoardIntent::FocusSearch],
            Some(BoardInputMode::Search),
            |model| board_entries(model, false),
        ),
        (
            "task page",
            board,
            vec![BoardIntent::OpenTaskPage],
            Some(BoardInputMode::TaskPage),
            |model| board_entries(model, true),
        ),
        (
            "notes editor",
            board,
            vec![BoardIntent::OpenTaskPage, BoardIntent::BeginEditNotes],
            Some(BoardInputMode::EditNotes),
            |_| notes_form(),
        ),
        (
            "title editor",
            board,
            vec![BoardIntent::OpenTaskPage, BoardIntent::BeginEditTitle],
            Some(BoardInputMode::EditTitle),
            |_| title_form(),
        ),
        (
            "thread field",
            board,
            edit(CaptureField::Thread),
            None,
            |_| thread_form(),
        ),
        (
            "scope field",
            board,
            edit(CaptureField::Scope),
            None,
            |_| scope_form(),
        ),
        (
            "assignee field",
            board,
            edit(CaptureField::Assignee),
            None,
            |_| assignee_form(),
        ),
        ("base field", board, edit(CaptureField::Base), None, |_| {
            base_form()
        }),
        (
            "step editor",
            board,
            vec![BoardIntent::OpenTaskPage, BoardIntent::BeginAddStep],
            Some(BoardInputMode::EditStep),
            |model| board_entries(model, true),
        ),
        (
            "blocked reply editor",
            blocked_page,
            vec![BoardIntent::BeginReply],
            Some(BoardInputMode::EditReply),
            |model| board_entries(model, true),
        ),
        (
            "review feedback editor",
            review_page,
            vec![BoardIntent::BeginReply],
            Some(BoardInputMode::EditReply),
            |model| board_entries(model, true),
        ),
        (
            "quick-add",
            board,
            vec![BoardIntent::OpenCapture],
            Some(BoardInputMode::QuickAdd),
            |_| quick_add(),
        ),
        (
            "quick-add draft",
            board,
            vec![BoardIntent::OpenCapture, BoardIntent::ExpandQuickAdd],
            None,
            |_| notes_form(),
        ),
    ];
    let mut failures = Vec::new();
    for (name, setup, intents, mode, expect) in &scenes {
        let (_, model) = drive(*setup, intents);
        if let Some(mode) = mode {
            assert_eq!(model.input_mode(), *mode, "{name}: scene setup");
        }
        let entries = expect(&model);
        for width in WIDTHS {
            let painted = rows(&model, width, height(width));
            let bar = painted.last().expect("verb row");
            if let Some(why) = misfit(bar, &entries, width) {
                failures.push(format!("{name} at {width}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The legend row (under the card's `├─┤` rule) of every card that owns one.
#[test]
fn every_card_legend_paints_whole_entries_and_keeps_its_essentials_at_every_width() {
    type Setup = fn() -> (DomainState, BoardModel);
    let scenes: Vec<(&str, Setup, Vec<BoardIntent>)> = vec![
        ("help", board, vec![BoardIntent::OpenHelp]),
        ("palette", board, vec![BoardIntent::OpenCommandPalette]),
        (
            "project picker",
            board,
            vec![BoardIntent::OpenProjectSelector],
        ),
        (
            "archived tab",
            archived_board,
            vec![
                BoardIntent::OpenProjectSelector,
                BoardIntent::ProjectPickerSwitchTab,
            ],
        ),
        (
            "filter picker",
            board,
            vec![BoardIntent::OpenThreadFilterPicker],
        ),
        ("sort picker", board, vec![BoardIntent::OpenSortPicker]),
        ("block card", board, vec![BoardIntent::ToggleBlock]),
        ("review card", board, vec![BoardIntent::ToggleReview]),
    ];
    let legends = card_legends();
    let mut failures = Vec::new();
    for (name, setup, intents) in &scenes {
        let (_, model) = drive(*setup, intents);
        for width in WIDTHS {
            let painted = rows(&model, width, height(width));
            let Some(rule) = painted
                .iter()
                .position(|row| row.trim_start().starts_with('├'))
            else {
                // Compact Help spends its rows on the catalog and paints no legend at all.
                if !(*name == "help"
                    && tier::resolve(width, height(width)).tier == tier::Tier::Compact)
                {
                    failures.push(format!("{name} at {width}: no legend rule painted"));
                }
                continue;
            };
            // The card's edges are where its rule's corners sit.
            let rule_cells: Vec<char> = painted[rule].chars().collect();
            let left = rule_cells
                .iter()
                .position(|&c| c == '├')
                .expect("rule start");
            let right = rule_cells.iter().position(|&c| c == '┤').expect("rule end");
            let inner: String = painted[rule + 1]
                .chars()
                .skip(left + 1)
                .take(right - left - 1)
                .collect();
            let inner = inner.as_str();
            // The last entry keeps a blank cell off the right border.
            if !inner.ends_with(' ') {
                failures.push(format!("{name} at {width}: {inner:?} touches the border"));
            }
            // Each card paints its own legend; the sort picker uses the plain list legend.
            let want = if *name == "sort picker" {
                "list picker"
            } else {
                name
            };
            let (_, entries) = legends
                .iter()
                .find(|(legend, _)| *legend == want)
                .expect("a declared legend for every card");
            if let Some(why) = misfit(inner, entries, width) {
                failures.push(format!("{name} at {width}: {why}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
