//! Wide stage slider: chrome (T2), stage routing (T3), mouse (T4), dirty drafts (T5).

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::Terminal;
use tsk_tui::domain::{DomainState, HumanStatus, Notice, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{map_key, route_responsive_key, ResponsiveKeyRoute};
use tsk_tui::ui::mouse::{
    left_click, map_board_mouse, map_responsive_board_mouse, wide_mouse_focus_intent,
};
use tsk_tui::ui::render::{assert_buffer_mono, QueueHitMap, QueueHitTarget};
use tsk_tui::ui::tier::{
    resolve_responsive, FocusedSurface, ResponsivePresentation, Tier, WideStage,
    STANDARD_VERB_BAR_ENTRY_BUDGET, WIDE_SPLIT_MIN_WIDTH,
};

use tsk_tui::ui::{
    apply_intent, draw_board, BoardInputMode, BoardIntent, BoardModel, IntentOutcome,
};

const REPO: &str = "/repos/tsk";

const T12_NOTES: &str = "Rework the wide split so the task page reads as a **detail pane**, not a boxed clone.\n\n- keep board unboxed\n- decide separator\n- verb bar ownership\n\n```\nresolve_responsive(w, h, focus)\n```";
const PROJECT_A: &str = "/repos/alpha";
const PROJECT_B: &str = "/repos/beta";

/// The brief's reference fixture: T12 started with markdown notes in this repo, T15 ready on
/// the desk. The model carries the numbers; the domain shares the ids.
fn fixture() -> (DomainState, BoardModel) {
    fixture_with_titles("Frame the wide task view", "Renew domain")
}

fn projects_fixture() -> (DomainState, BoardModel) {
    let mut domain = DomainState::new();
    domain
        .create(
            "alpha task",
            Some("alpha notes".to_string()),
            TaskScope::Project {
                path: PROJECT_A.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create alpha task");
    let beta = domain
        .create(
            "beta task",
            Some("beta notes".to_string()),
            TaskScope::Project {
                path: PROJECT_B.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create beta task");
    domain
        .edit(
            beta,
            "beta task",
            Some("beta notes".to_string()),
            TaskScope::Project {
                path: PROJECT_B.to_string(),
            },
            Some("release".to_string()),
        )
        .expect("thread beta task");
    let mut model = BoardModel::from_domain(&domain, None);
    go(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
    );
    (domain, model)
}

fn fixture_with_titles(first: &str, second: &str) -> (DomainState, BoardModel) {
    let mut domain = DomainState::new();
    let started = domain
        .create(
            first,
            Some(T12_NOTES.to_string()),
            TaskScope::Project {
                path: REPO.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create T12");
    domain
        .set_status(started, HumanStatus::Started)
        .expect("start T12");
    let ready = domain
        .create(
            second,
            Some("renewal notes".to_string()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create T15");
    domain
        .set_status(ready, HumanStatus::Ready)
        .expect("ready T15");
    let mut tasks = domain.tasks().to_vec();
    tasks[0].number = Some(12);
    tasks[1].number = Some(15);
    let model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    (domain, model)
}

fn go(domain: &mut DomainState, model: &mut BoardModel, intent: BoardIntent) {
    apply_intent(domain, model, intent, None).expect("apply intent");
}

fn to_stage(domain: &mut DomainState, model: &mut BoardModel, stage: WideStage) {
    let order = [
        WideStage::FullBoard,
        WideStage::Split,
        WideStage::Rail,
        WideStage::FullTask,
    ];
    let index = |stage: WideStage| order.iter().position(|&s| s == stage).unwrap();
    let (mut cur, want) = (index(model.wide_stage()), index(stage));
    while cur < want {
        go(domain, model, BoardIntent::StageRight);
        cur += 1;
    }
    while cur > want {
        go(domain, model, BoardIntent::StageLeft);
        cur -= 1;
    }
    assert_eq!(model.wide_stage(), stage);
}

const STAGES: [WideStage; 4] = [
    WideStage::FullBoard,
    WideStage::Split,
    WideStage::Rail,
    WideStage::FullTask,
];

fn render_buffer(model: &BoardModel, width: u16, height: u16) -> (Buffer, QueueHitMap) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    let mut hits = QueueHitMap::default();
    terminal
        .draw(|frame| hits = draw_board(frame, model))
        .expect("draw board");
    (terminal.backend().buffer().clone(), hits)
}

fn rows_of(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// Render and assert the frame is mono at every width, wide or not.
fn render(model: &BoardModel, width: u16, height: u16) -> (Vec<String>, QueueHitMap) {
    let (buffer, hits) = render_buffer(model, width, height);
    assert_buffer_mono(&buffer);
    (rows_of(&buffer), hits)
}

fn region_text(rows: &[String], area: Rect) -> String {
    rows.iter()
        .skip(area.y as usize)
        .take(area.height as usize)
        .map(|row| {
            row.chars()
                .skip(area.x as usize)
                .take(area.width as usize)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn column_text(rows: &[String], column: Rect, y: u16) -> String {
    rows[y as usize]
        .chars()
        .skip(column.x as usize)
        .take(column.width as usize)
        .collect()
}

fn footer_rows(height: u16) -> (u16, u16, u16) {
    (height - 3, height - 2, height - 1)
}

fn inside(area: Rect, hit: Rect) -> bool {
    hit.x >= area.x
        && hit.y >= area.y
        && hit.x.saturating_add(hit.width) <= area.x.saturating_add(area.width)
        && hit.y.saturating_add(hit.height) <= area.y.saturating_add(area.height)
}

// ---------------------------------------------------------------------------
// T2 chrome
// ---------------------------------------------------------------------------

#[test]
fn selecting_a_project_row_stays_in_the_index_until_stage_right() {
    let (mut domain, mut model) = projects_fixture();
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert!(model.right_seat().is_none());

    go(&mut domain, &mut model, BoardIntent::SelectNext);

    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert!(model.right_seat().is_none());
    assert_eq!(
        model.selected_project_row().map(|row| row.path),
        Some(PROJECT_B.to_string())
    );

    go(&mut domain, &mut model, BoardIntent::StageRight);

    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(
        model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(|path| path.to_string_lossy().into_owned()),
        Some(PROJECT_B.to_string())
    );
}

#[test]
fn clicking_a_project_row_stays_in_the_index_until_stage_right() {
    let area = Rect::new(0, 0, 110, 30);
    let (mut domain, mut model) = projects_fixture();
    let (_, hits) = render(&model, area.width, area.height);
    let row = hits
        .regions
        .iter()
        .find(|hit| matches!(hit.target, QueueHitTarget::ProjectRow(1)))
        .expect("second project row hit")
        .area;
    let intent = map_responsive_board_mouse(&model, &hits, area, left_click(row.x, row.y))
        .expect("project row click maps");

    go(&mut domain, &mut model, intent);

    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert!(model.right_seat().is_none());
    go(&mut domain, &mut model, BoardIntent::StageRight);

    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(
        model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(|path| path.to_string_lossy().into_owned()),
        Some(PROJECT_B.to_string())
    );
}

#[test]
fn projects_preview_stages_bind_a_nested_project_board_and_keep_the_index_cursor() {
    let (mut domain, mut model) = projects_fixture();
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Split);
    let first_path = model
        .selected_project_row()
        .expect("projects cursor")
        .path
        .clone();
    assert_eq!(
        model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(|path| path.to_string_lossy().into_owned()),
        Some(first_path)
    );
    let (split_rows, split_hits) = render(&model, 110, 30);
    let split = resolve_responsive(110, 30, WideStage::Split);
    assert!(split_rows.iter().any(|row| row.contains("alpha task")));
    assert!(
        column_text(&split_rows, split.task_content(), 1).contains("alpha"),
        "the split preview should name the selected project in its top chrome"
    );
    assert!(split_hits
        .regions
        .iter()
        .any(|hit| matches!(hit.target, QueueHitTarget::Task(_))));
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert!(model.right_seat().is_some());
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);

    let (rows, hits) = render(&model, 110, 30);
    let rail = resolve_responsive(110, 30, WideStage::Rail);
    assert!(rows
        .iter()
        .any(|row| row.contains("alpha task") || row.contains("beta task")));
    assert!(
        column_text(&rows, rail.task_content(), 1).contains("alpha"),
        "the rail preview should keep the selected project name in its top chrome"
    );
    assert!(hits
        .regions
        .iter()
        .any(|hit| matches!(hit.target, QueueHitTarget::ProjectRow(_))));
    assert!(hits
        .regions
        .iter()
        .any(|hit| matches!(hit.target, QueueHitTarget::Task(_))));
}

#[test]
fn projects_preview_stage_left_walks_back_without_reaching_full_task() {
    let (mut domain, mut model) = projects_fixture();
    go(&mut domain, &mut model, BoardIntent::StageRight);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::StageLeft);
    assert_eq!(model.wide_stage(), WideStage::Split);
    go(&mut domain, &mut model, BoardIntent::StageLeft);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
}

#[test]
fn projects_preview_mouse_focuses_rail_and_rail_rows_return_to_split() {
    let area = Rect::new(0, 0, 110, 30);
    let (mut domain, mut model) = projects_fixture();
    go(&mut domain, &mut model, BoardIntent::StageRight);
    let geometry = resolve_responsive(area.width, area.height, WideStage::Split);
    let (_, hits) = render(&model, area.width, area.height);
    let task = hits
        .regions
        .iter()
        .find(|hit| {
            inside(geometry.task, hit.area) && matches!(hit.target, QueueHitTarget::Task(_))
        })
        .expect("project preview task hit")
        .area;
    let click = left_click(task.x, task.y);
    assert_eq!(
        map_responsive_board_mouse(&model, &hits, area, click),
        None,
        "the preview is inert before focus moves right"
    );
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, area, click),
        Some(BoardIntent::StageRight)
    );
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert!(model.project_right_seat_focused());

    let current = model
        .selected_project_row()
        .expect("selected project")
        .path
        .clone();
    let rail = resolve_responsive(area.width, area.height, WideStage::Rail);
    let (_, hits) = render(&model, area.width, area.height);
    let row = hits
        .regions
        .iter()
        .find(|hit| {
            inside(rail.board, hit.area)
                && matches!(hit.target, QueueHitTarget::ProjectRow(index) if {
                    model
                        .project_rows()
                        .into_iter()
                        .nth(index)
                        .is_some_and(|row| row.path != current)
                })
        })
        .expect("another project row hit")
        .area;
    let intent = map_responsive_board_mouse(&model, &hits, area, left_click(row.x, row.y))
        .expect("rail project row maps");
    assert!(matches!(intent, BoardIntent::SelectProjectRow(_)));
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert!(!model.project_right_seat_focused());
    assert_ne!(
        model
            .right_seat()
            .and_then(BoardModel::active_project)
            .map(|path| path.to_string_lossy().into_owned()),
        Some(current)
    );
}

#[test]
fn projects_preview_cursor_rebinds_split_and_thread_view_drops_it() {
    let (mut domain, mut model) = projects_fixture();
    go(&mut domain, &mut model, BoardIntent::StageRight);
    let before = model
        .right_seat()
        .and_then(BoardModel::active_project)
        .map(|path| path.to_path_buf());
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    let after = model
        .right_seat()
        .and_then(BoardModel::active_project)
        .map(|path| path.to_path_buf());
    assert_ne!(before, after);
    go(&mut domain, &mut model, BoardIntent::OpenProjectsViewPicker);
    go(&mut domain, &mut model, BoardIntent::ListPickerNext);
    go(&mut domain, &mut model, BoardIntent::ConfirmListPicker);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert!(model.right_seat().is_none());
}

#[test]
fn projects_preview_cursor_does_not_change_remembered_project_until_enter() {
    let (mut domain, mut model) = projects_fixture();
    model.set_selected_project(Some(PathBuf::from(PROJECT_A)));
    go(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
    );
    go(&mut domain, &mut model, BoardIntent::StageRight);
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    assert_eq!(
        model.selected_project(),
        Some(std::path::Path::new(PROJECT_A))
    );
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    assert_eq!(
        model.active_project(),
        Some(std::path::Path::new(PROJECT_B))
    );
    assert_eq!(
        model.selected_project(),
        Some(std::path::Path::new(PROJECT_B))
    );
}

#[test]
fn wide_frames_are_mono_at_every_stage_width_and_height() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        for (width, height) in [
            (WIDE_SPLIT_MIN_WIDTH, 24),
            (111, 24),
            (130, 24),
            (130, 10),
            (157, 26),
            (200, 40),
        ] {
            let (buffer, _) = render_buffer(&model, width, height);
            assert_buffer_mono(&buffer);
        }
    }
}

#[test]
fn wide_paints_exactly_one_footer_rule_row_at_every_stage() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let (rows, hits) = render(&model, 130, 24);
        let (rule_y, status_y, verb_y) = footer_rows(24);
        let rule_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.chars().all(|c| c == '─'))
            .map(|(y, _)| y)
            .collect();
        // The shared footer paints exactly one full-width rule row, at `rule_y`.
        let footer_rules: Vec<usize> = rule_rows
            .iter()
            .copied()
            .filter(|&y| y == rule_y as usize)
            .collect();
        assert_eq!(
            footer_rules,
            vec![rule_y as usize],
            "{stage:?}: exactly one footer rule row"
        );
        // Any other full-width dash row is a task-column header underline: that exists
        // only in stage F, where the column spans the whole frame, and only on row 2.
        let header_underlines: Vec<usize> = rule_rows
            .into_iter()
            .filter(|&y| y != rule_y as usize)
            .collect();
        let expected_underlines = if stage == WideStage::FullTask {
            vec![2usize]
        } else {
            vec![]
        };
        assert_eq!(
            header_underlines, expected_underlines,
            "{stage:?}: header underline rows"
        );
        assert!(
            rows[status_y as usize].starts_with(" desk"),
            "{stage:?}: footer status names the desk lens: {}",
            rows[status_y as usize]
        );
        let verb_row = &rows[verb_y as usize];
        assert!(
            verb_row.starts_with(' '),
            "{stage:?}: verb bar keeps its leading space"
        );
        assert!(verb_row.contains("ctrl+d done"), "{stage:?}: {verb_row}");
        let ctrl_rows = rows.iter().filter(|row| row.contains("ctrl+")).count();
        assert_eq!(ctrl_rows, 1, "{stage:?}: exactly one verb bar");
        let verb_hits: Vec<&tsk_tui::ui::render::QueueHit> = hits
            .regions
            .iter()
            .filter(|hit| matches!(hit.target, QueueHitTarget::Verb(_)))
            .collect();
        assert!(!verb_hits.is_empty(), "{stage:?}: verb hits exist");
        assert!(
            verb_hits.iter().all(|hit| hit.area.y == verb_y),
            "{stage:?}: every verb hit sits on the one verb row"
        );
    }
}

#[test]
fn stage_zero_and_full_task_render_the_standard_tier_at_130x24() {
    let (mut domain, mut model) = fixture();
    let geometry = resolve_responsive(130, 24, WideStage::FullBoard);
    assert_eq!(geometry.density, Tier::Standard);
    let (rows, hits) = render(&model, 130, 24);
    assert!(rows[0].trim().is_empty(), "row 0 blank");
    assert!(
        rows[1].contains("desk  ·  tsk ▾  ·  projects"),
        "selector row"
    );
    assert!(rows[2].trim().is_empty(), "blank above IN MOTION");
    assert!(rows[3].starts_with(" IN MOTION ─"));
    assert!(rows[4].trim().is_empty(), "blank between header and rows");
    assert!(rows[5].starts_with("▸ ● T12 Frame the wide task view"));
    assert!(!rows.iter().any(|row| row.contains("└─ tsk")));
    assert!(rows[6].trim().is_empty());
    assert!(rows[7].starts_with(" ON DECK · desk ─"));
    assert!(rows[9].starts_with("  ○ T15 Renew domain"));
    let verbs = hits
        .regions
        .iter()
        .filter(|hit| matches!(hit.target, QueueHitTarget::Verb(_)))
        .count();
    // The bar is a fixed shape (open · status verbs · help): a started row paints
    // six entries at every tier, well inside the standard budget.
    assert_eq!(verbs, 6, "started row bar: {verbs}");
    assert!(verbs <= usize::from(STANDARD_VERB_BAR_ENTRY_BUDGET));

    to_stage(&mut domain, &mut model, WideStage::FullTask);
    assert_eq!(
        resolve_responsive(130, 24, WideStage::FullTask).density,
        Tier::Standard
    );
    let (rows, _) = render(&model, 130, 24);
    assert!(rows[0].trim().is_empty(), "F keeps the blank row");
    assert!(rows[1].starts_with(" ● T12 Frame the wide task view"));
    assert!(rows[1].trim_end().ends_with("started · tsk"));
    assert!(
        rows[2].chars().all(|c| c == '─'),
        "the rule sits under the title"
    );
    assert!(
        rows[3].contains("Rework the wide split"),
        "body starts under the rule"
    );
}

#[test]
fn task_column_header_replaces_the_in_pane_header_with_stage_weight() {
    for stage in [WideStage::Split, WideStage::Rail, WideStage::FullTask] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 24, stage);
        let column = geometry.task_content();
        let (buffer, _) = render_buffer(&model, 130, 24);
        let rows = rows_of(&buffer);
        let header = column_text(&rows, column, 1);
        assert!(
            header.starts_with(" ● T12 Frame the wide task view"),
            "{stage:?}: {header}"
        );
        assert!(
            header.trim_end().ends_with("started · tsk"),
            "{stage:?}: {header}"
        );
        assert!(
            column_text(&rows, column, 2).chars().all(|c| c == '─'),
            "{stage:?}: the dash rule sits under the title"
        );
        let body = region_text(&rows, Rect::new(column.x, 3, column.width, 18));
        assert!(
            !body.contains("● T12"),
            "{stage:?}: in-pane header must not paint"
        );
        assert!(
            !body
                .lines()
                .any(|line| line.trim().chars().all(|c| c == '─') && !line.trim().is_empty()),
            "{stage:?}: no in-pane divider"
        );
        assert!(column_text(&rows, column, 3).contains("Rework the wide split"));
        let title_x = column.x + 7;
        let title_cell = &buffer[(title_x, 1)];
        if stage == WideStage::Split {
            assert!(title_cell.modifier.contains(Modifier::DIM), "A header dim");
            assert!(
                !title_cell.modifier.contains(Modifier::BOLD),
                "A header not bold"
            );
        } else {
            assert!(
                title_cell.modifier.contains(Modifier::BOLD),
                "{stage:?} header bold"
            );
            assert!(!title_cell.modifier.contains(Modifier::DIM));
        }
    }
}

#[test]
fn stage_a_board_has_no_attribution_column() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (rows, _) = render(&model, 130, 24);
    let row = column_text(&rows, geometry.board, 5);
    assert!(row.starts_with("▸ ● T12 Frame"), "{row}");
    assert!(!rows.iter().any(|row| row.contains("└─ tsk")));
    assert_eq!(rows[5].chars().nth(geometry.rule.x as usize), Some('│'));
}

#[test]
fn rail_wraps_titles_with_indent_four_and_dims_every_cell() {
    let long = "A deliberately long rail title that cannot fit thirty two columns";
    let (mut domain, mut model) = fixture_with_titles(long, "Renew domain");
    go(&mut domain, &mut model, BoardIntent::ToggleDoneDrawer);
    assert!(model.drawer_open());
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (buffer, _) = render_buffer(&model, 130, 24);
    let rows = rows_of(&buffer);
    let rail = geometry.board;
    let (rule_y, _, _) = footer_rows(24);
    let rail_rows: Vec<String> = (0..rule_y).map(|y| column_text(&rows, rail, y)).collect();
    let first = rail_rows
        .iter()
        .position(|row| row.starts_with("▸ ● T12 "))
        .expect("selected rail row retains the status beside its selection arrow");
    let continuations: Vec<&String> = rail_rows[first + 1..]
        .iter()
        .take_while(|row| !row.trim().is_empty())
        .collect();
    assert!(
        continuations.len() >= 2,
        "long title wraps over several rows"
    );
    for row in &continuations {
        assert!(row.starts_with("    "), "continuation indent 4: {row:?}");
        assert!(!row.starts_with("     "), "exactly four cells: {row:?}");
    }
    let painted: String = std::iter::once(&rail_rows[first])
        .chain(continuations.iter().copied())
        .map(|row| row.trim().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    for word in long.split(' ') {
        assert!(painted.contains(word), "never truncated: {painted}");
    }
    assert!(!painted.contains('…'));
    for y in 0..rule_y {
        assert_eq!(
            buffer[(rail.width, y)].symbol(),
            "│",
            "rule column at row {y}"
        );
        if y != 1 {
            assert_eq!(
                buffer[(rail.width - 1, y)].symbol(),
                " ",
                "no glyph touches the rule at row {y}"
            );
        }
        for x in 0..rail.width {
            assert!(
                buffer[(x, y)].modifier.contains(Modifier::DIM),
                "rail cell ({x},{y}) must be dim"
            );
        }
    }
    let rail_text = rail_rows.join("\n");
    assert!(!rail_text.contains("0s"), "rail drops the meta column");
    assert!(!rail_text.contains("DONE"), "rail drops the done drawer");
    assert!(rail_text.contains(" IN MOTION ─"));
    assert!(rail_text.contains(" desk ─"));
    assert!(rail_text.contains("desk  ·  tsk"));
}

#[test]
fn status_row_crumb_and_keys_follow_the_stage() {
    let expectations = [
        (WideStage::FullBoard, None, "→ task pane"),
        (WideStage::Split, Some("board ▸ task"), "→ task · ← close"),
        (
            WideStage::Rail,
            Some("board ◂ task"),
            "← board · → full page",
        ),
        (WideStage::FullTask, None, "← rail"),
    ];
    for (stage, crumb, keys) in expectations {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let (rows, _) = render(&model, 130, 24);
        let (_, status_y, _) = footer_rows(24);
        let status = &rows[status_y as usize];
        assert!(status.trim_end().ends_with(keys), "{stage:?}: {status}");
        match crumb {
            Some(crumb) => assert!(
                status.contains(&format!("{crumb}    {keys}")),
                "{stage:?}: {status}"
            ),
            None => assert!(!status.contains('▸') && !status.contains('◂'), "{stage:?}"),
        }
        for other in ["board ▸ task", "board ◂ task"] {
            if crumb != Some(other) {
                assert!(!status.contains(other), "{stage:?}: {status}");
            }
        }
    }
}

#[test]
fn status_row_refusal_wins_over_the_crumb_then_the_keys() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    let (_, status_y, _) = footer_rows(24);
    let (rows, _) = render(&model, 130, 24);
    assert!(rows[status_y as usize].contains("board ▸ task"));

    // 130 cols: ` x…x` (1 + 100) leaves 29 cells, room for `→ task · ← close` (16) but not
    // for the crumb-plus-keys pair (32).
    model.set_message("x".repeat(100));
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(status.contains(&"x".repeat(100)));
    assert!(
        !status.contains("board ▸ task"),
        "crumb drops first: {status}"
    );
    assert!(
        status.contains("→ task · ← close"),
        "keys survive: {status}"
    );

    model.set_message("y".repeat(120));
    let (rows, _) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(status.contains(&"y".repeat(120)));
    assert!(
        !status.contains("→ task"),
        "keys drop after the crumb: {status}"
    );
}

#[test]
fn stage_a_board_meta_column_sizes_to_the_widest_meta() {
    // A long title must fit on one row once the meta budget shrinks to the widest meta
    // actually painted (plus a small gap) instead of the narrow board's fixed reserve.
    let mut domain = DomainState::new();
    let id = domain
        .create(
            "wide board meta column sizing",
            Some("notes".to_string()),
            TaskScope::Project {
                path: REPO.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create long-title task");
    // Started puts the task in IN MOTION, where its meta paints `tsk · 0s`.
    domain
        .set_status(id, HumanStatus::Started)
        .expect("start task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO)));
    to_stage(&mut domain, &mut model, WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (rows, _) = render(&model, 130, 24);
    let board = region_text(&rows, geometry.board);
    assert!(
        board
            .lines()
            .any(|line| line.contains("wide board meta column sizing")),
        "the 29-cell title must paint on a single row when the meta is `tsk · 0s`:\n{board}"
    );

    // A 60-cell title needs a wider column to fit; at 200 cols the sized-down meta gives it
    // the room, whereas the fixed 28-cell reserve would still wrap it.
    // Arithmetic at 200 cols: board column floor(200*2/5)=80, meta budget max(8+3,8)=11,
    // title width 80-11=69, row lead (no identifier) 4, so title room = 69-4 = 65 >= 60.
    let base = "a sixty character title that needs the resized meta column to fit";
    let long: String = base.chars().take(60).collect();
    assert_eq!(long.chars().count(), 60, "fixture title must be 60 cells");
    let mut domain = DomainState::new();
    let id = domain
        .create(
            long.clone(),
            Some("notes".to_string()),
            TaskScope::Project {
                path: REPO.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create 60-char title task");
    domain
        .set_status(id, HumanStatus::Started)
        .expect("start task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(REPO)));
    to_stage(&mut domain, &mut model, WideStage::Split);
    let geometry = resolve_responsive(200, 24, WideStage::Split);
    let (rows, _) = render(&model, 200, 24);
    let board = region_text(&rows, geometry.board);
    assert!(
        board.lines().any(|line| line.contains(long.as_str())),
        "the 60-cell title must paint on a single row at 200 cols:\n{board}"
    );
}

#[test]
fn footer_crumb_returns_to_stage_keys_when_the_session_is_clean_and_parked() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_editing(), "the edit session stays open");
    assert!(!model.task_session_dirty(), "the draft is clean");
    assert!(
        model.open_field_edit().is_none(),
        "no field editor is active"
    );
    let (rows, _) = render(&model, 130, 24);
    let (_, status_y, _) = footer_rows(24);
    let status = &rows[status_y as usize];
    assert!(
        status.contains("← board · → full page"),
        "a clean parked session shows the stage keys: {status}"
    );
    assert!(!status.contains("shift+enter save"), "{status}");

    // A dirty parked draft keeps the save/cancel keys even without an active editor.
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_session_dirty(), "the draft is now dirty");
    assert!(model.open_field_edit().is_none());
    let (rows, hits) = render(&model, 130, 24);
    let status = &rows[status_y as usize];
    assert!(
        !status.contains("← board") && status.contains("board ◂ task"),
        "a dirty parked draft drops the stage keys but keeps the crumb: {status}"
    );
    // The save keys live on the verb row, not the status row.
    let (_, _, verb_y) = footer_rows(24);
    assert!(
        rows[verb_y as usize].contains("shift+enter save")
            && rows[verb_y as usize].contains("esc cancel"),
        "the verb row carries save / cancel: {}",
        rows[verb_y as usize]
    );
    let _ = hits;
}

#[test]
fn status_row_shows_editor_keys_while_an_editor_is_active() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    let (rows, _) = render(&model, 130, 24);
    let (_, status_y, verb_y) = footer_rows(24);
    assert!(
        rows[verb_y as usize].contains("shift+enter save")
            && rows[verb_y as usize].contains("esc cancel"),
        "{}",
        rows[verb_y as usize]
    );
    assert!(!rows[status_y as usize].contains("← board"));
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let header = column_text(&rows, geometry.task_content(), 1);
    assert!(header.trim_end().ends_with("editing title"), "{header}");
}

#[test]
fn empty_pane_paints_no_task_header_and_is_inert() {
    let mut model = BoardModel::from_tasks(Vec::new(), Some(PathBuf::from(REPO)));
    let mut domain = DomainState::new();
    assert_eq!(model.selected_id(), None);
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(
        model.wide_stage(),
        WideStage::FullBoard,
        "stage A needs a selection"
    );
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);

    // Stage A reached with a task, which then disappears: stay in A, paint the empty pane.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    model.sync_from_domain(&DomainState::new());
    assert_eq!(model.selected_id(), None);
    assert_eq!(model.wide_stage(), WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (rows, hits) = render(&model, 130, 24);
    let column = geometry.task_content();
    let header = column_text(&rows, column, 1);
    assert!(header.starts_with(" no task"), "{header}");
    assert!(
        column_text(&rows, column, 2).trim().starts_with('─'),
        "{header}"
    );
    assert!(column_text(&rows, column, 3).contains("select a task to preview it here"));
    assert!(
        hits.regions
            .iter()
            .all(|hit| !inside(column_text_area(column, 24), hit.area)),
        "empty pane exposes no hits"
    );
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Split, "G needs a selection");
}

fn column_text_area(column: Rect, height: u16) -> Rect {
    Rect::new(column.x, 0, column.width, height - 3)
}

#[test]
fn wide_hits_stay_inside_their_column_or_the_footer() {
    for stage in STAGES {
        for (width, height) in [(110, 24), (130, 24), (157, 26), (240, 60)] {
            let (mut domain, mut model) = fixture();
            to_stage(&mut domain, &mut model, stage);
            let geometry = resolve_responsive(width, height, stage);
            let (_, hits) = render(&model, width, height);
            let footer = Rect::new(0, height - 3, width, 3);
            assert_eq!(
                hits.footer,
                Some(footer),
                "{stage:?} {width}x{height}: painted footer"
            );
            let board = Rect::new(0, 0, geometry.board.width, height - 3);
            let task = Rect::new(
                geometry.task_content().x,
                0,
                geometry.task_content().width,
                height - 3,
            );
            for area in hits
                .regions
                .iter()
                .map(|hit| hit.area)
                .chain(hits.copyable.iter().copied())
            {
                assert!(
                    inside(footer, area) || inside(board, area) || inside(task, area),
                    "{stage:?} {width}x{height}: {area:?} escapes board {board:?}, task {task:?}, footer {footer:?}"
                );
                assert!(
                    !(geometry.rule.width > 0
                        && area.x <= geometry.rule.x
                        && area.x + area.width > geometry.rule.x
                        && area.y < height - 3),
                    "{stage:?} {width}x{height}: {area:?} crosses the rule column"
                );
            }
            if geometry.task.width > 0 && geometry.board.width > 0 {
                assert!(
                    hits.regions.iter().any(|hit| inside(board, hit.area)),
                    "{stage:?}: board column keeps row hits"
                );
            }
        }
    }
}

#[test]
fn stage_a_preview_paints_controls_for_the_focus_router_only() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (_, hits) = render(&model, 130, 24);
    let column = column_text_area(geometry.task_content(), 24);
    let number = hits
        .regions
        .iter()
        .find(|hit| inside(column, hit.area) && matches!(hit.target, QueueHitTarget::TaskNumber(_)))
        .expect("the T<number> prefix keeps its copy hit");
    assert_eq!(
        number.area,
        Rect::new(geometry.task_content().x + 3, 1, 3, 1)
    );
    let control = hits
        .regions
        .iter()
        .find(|hit| inside(column, hit.area) && matches!(hit.target, QueueHitTarget::StepAdd))
        .expect("preview controls exist for the focus router");
    // The board-focused mouse map never dispatches them...
    assert_eq!(
        click_map(&model, &hits, control.area.x, control.area.y),
        None,
        "preview controls are inert to the board-focused router"
    );
    // ...only the focus router reads them, sliding the stage first.
    assert_eq!(
        wide_mouse_focus_intent(
            &model,
            &hits,
            AREA_130,
            left_click(control.area.x, control.area.y)
        ),
        Some(BoardIntent::StageRight)
    );
}

#[test]
fn narrow_board_is_unchanged_by_the_stage_model() {
    let (mut domain, mut model) = fixture();
    let (rows, _) = render(&model, 109, 24);
    assert!(
        rows[1].contains("desk  ·  tsk ▾  ·  projects"),
        "selector: {:?}",
        rows[1]
    );
    assert!(rows[5].starts_with("▸ ● T12 Frame the wide task view"));
    assert!(
        !rows[22].contains("→ pane"),
        "no crumb below the wide threshold"
    );
    go(&mut domain, &mut model, BoardIntent::PeekDetail);
    let (rows, _) = render(&model, 109, 24);
    assert!(
        rows.join("\n").contains("│ Rework the wide split"),
        "peek still works narrow"
    );
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
}

#[test]
fn title_edit_window_matches_the_painted_header_room_with_a_long_identifier() {
    // A 7-char identifier (T123456): the draft must be windowed to exactly the cells the
    // painter gives the title, so a long draft never truncates the caret with an ellipsis.
    let (mut domain, model) = fixture();
    let mut tasks = domain.tasks().to_vec();
    let id = model.selected_id().expect("selection");
    for task in &mut tasks {
        if task.id == id {
            task.number = Some(123456);
        }
    }
    let mut model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    assert_eq!(model.selected_id(), Some(id));
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(
        &mut domain,
        &mut model,
        BoardIntent::EditInsertText("x".repeat(200)),
    );
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (rows, _) = render(&model, 130, 24);
    let header = column_text(&rows, geometry.task_content(), 1);
    assert!(header.contains("T123456"), "{header}");
    assert!(header.contains('x'), "the windowed draft paints: {header}");
    assert!(
        !header.contains('…'),
        "the caret window must not overflow the painted title room: {header}"
    );
    assert!(header.trim_end().ends_with("editing title"), "{header}");
}

// ---------------------------------------------------------------------------
// T3 stage routing
// ---------------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Route one bare key through the stage router, deriving the presentation from the
/// model's own stage at a wide frame instead of assuming it.
fn wide_key(model: &BoardModel, code: KeyCode) -> Option<BoardIntent> {
    let presentation = resolve_responsive(130, 24, model.wide_stage()).presentation;
    route_key(model, presentation, code)
}

fn route_key(
    model: &BoardModel,
    presentation: ResponsivePresentation,
    code: KeyCode,
) -> Option<BoardIntent> {
    match route_responsive_key(
        model.input_mode(),
        model.wide_stage(),
        presentation,
        key(code),
    ) {
        ResponsiveKeyRoute::Intent(intent) => Some(intent),
        ResponsiveKeyRoute::Inert => None,
        ResponsiveKeyRoute::Surface => map_key(model.input_mode(), key(code)),
    }
}

/// Press one key at a wide width: map it, apply it when it maps to something.
fn press(domain: &mut DomainState, model: &mut BoardModel, code: KeyCode) -> Option<BoardIntent> {
    let intent = wide_key(model, code);
    if let Some(intent) = intent.clone() {
        go(domain, model, intent);
    }
    intent
}

#[test]
fn stage_zero_keys_slide_open_select_and_stay_put() {
    let (mut domain, mut model) = fixture();
    let first = model.selected_id();
    assert_eq!(wide_key(&model, KeyCode::Left), None, "← is inert in 0");
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CloseLayer)
    );
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('j')),
        Some(BoardIntent::SelectNext)
    );
    assert_ne!(model.selected_id(), first);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert_eq!(press(&mut domain, &mut model, KeyCode::Tab), None);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Right),
        Some(BoardIntent::StageRight)
    );
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(model.focused_surface(), FocusedSurface::Board);

    let (mut domain, mut model) = fixture();
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    assert_eq!(model.stage_origin(), Some(WideStage::FullBoard));
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn stage_a_keys_slide_both_ways_open_and_retarget_the_pane() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    let first = model.selected_id().expect("selection");
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CloseLayer)
    );
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('j')),
        Some(BoardIntent::SelectNext)
    );
    let second = model.selected_id().expect("moved selection");
    assert_ne!(second, first);
    assert_eq!(model.wide_stage(), WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (rows, _) = render(&model, 130, 24);
    assert!(column_text(&rows, geometry.task_content(), 1).contains("T15 Renew domain"));
    assert_eq!(press(&mut domain, &mut model, KeyCode::Tab), None);
    assert_eq!(
        model.wide_stage(),
        WideStage::Split,
        "Tab is not a stage key"
    );

    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Left),
        Some(BoardIntent::StageLeft)
    );
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    to_stage(&mut domain, &mut model, WideStage::Split);
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Right),
        Some(BoardIntent::StageRight)
    );
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(model.focused_surface(), FocusedSurface::Task);
    assert_eq!(model.edit_target(), Some(second));
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);

    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    assert_eq!(model.stage_origin(), Some(WideStage::Split));
}

#[test]
fn stage_g_keys_slide_open_close_and_navigate_the_page() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let bound = model.edit_target().expect("bound page");
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Char('j')),
        Some(BoardIntent::PageScrollDown)
    );
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(
        model.selected_id(),
        Some(bound),
        "j navigates the page, not the rail"
    );
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Right),
        Some(BoardIntent::StageRight)
    );
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    assert_eq!(model.stage_origin(), Some(WideStage::Rail));

    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let bound = model.edit_target().expect("bound page");
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Left),
        Some(BoardIntent::StageLeft)
    );
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(model.focused_surface(), FocusedSurface::Board);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.edit_target(), Some(bound), "the pane stays bound");

    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Esc),
        Some(BoardIntent::CloseLayer)
    );
    assert_eq!(model.wide_stage(), WideStage::Split);

    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let bound = model.edit_target().expect("bound page");
    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    assert_eq!(model.stage_origin(), Some(WideStage::Rail));
    assert_eq!(model.edit_target(), Some(bound));
}

#[test]
fn stage_f_keys_return_to_the_rail_or_the_origin() {
    for origin in [WideStage::FullBoard, WideStage::Split, WideStage::Rail] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, origin);
        press(&mut domain, &mut model, KeyCode::Enter);
        assert_eq!(model.wide_stage(), WideStage::FullTask, "{origin:?}");
        assert_eq!(
            wide_key(&model, KeyCode::Right),
            None,
            "{origin:?}: → inert in F"
        );
        assert_eq!(
            press(&mut domain, &mut model, KeyCode::Char('k')),
            Some(BoardIntent::PageScrollUp)
        );
        assert_eq!(model.wide_stage(), WideStage::FullTask);
        assert_eq!(
            press(&mut domain, &mut model, KeyCode::Esc),
            Some(BoardIntent::CloseLayer)
        );
        assert_eq!(model.wide_stage(), origin, "Esc returns to {origin:?}");
        assert_eq!(model.stage_origin(), None, "origin memory clears");
        assert_eq!(
            model.focused_surface(),
            origin.focused_surface(),
            "{origin:?}"
        );
        if origin == WideStage::FullBoard {
            assert_eq!(
                model.edit_target(),
                None,
                "back on the bare board the page closes"
            );
        } else {
            assert!(
                model.edit_target().is_some(),
                "{origin:?} keeps the page session"
            );
        }
    }

    for origin in [WideStage::FullBoard, WideStage::Split, WideStage::Rail] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, origin);
        press(&mut domain, &mut model, KeyCode::Enter);
        assert_eq!(
            press(&mut domain, &mut model, KeyCode::Left),
            Some(BoardIntent::StageLeft)
        );
        assert_eq!(
            model.wide_stage(),
            WideStage::Rail,
            "← from F always goes to G"
        );
        assert_eq!(model.stage_origin(), None);
        assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    }
}

#[test]
fn enter_in_stage_f_closes_the_page_like_the_single_pane_page() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    press(&mut domain, &mut model, KeyCode::Enter);
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    press(&mut domain, &mut model, KeyCode::Enter);
    assert_eq!(
        model.wide_stage(),
        WideStage::Split,
        "Enter toggles the page shut"
    );
    assert_eq!(model.stage_origin(), None);
}

#[test]
fn stage_keys_need_a_selection() {
    let mut domain = DomainState::new();
    let mut model = BoardModel::from_tasks(Vec::new(), Some(PathBuf::from(REPO)));
    for code in [KeyCode::Right, KeyCode::Enter] {
        press(&mut domain, &mut model, code);
        assert_eq!(model.wide_stage(), WideStage::FullBoard, "{code:?}");
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
    }
    assert_eq!(model.edit_target(), None);
}

#[test]
fn stage_keys_fall_through_to_editor_semantics_while_editing() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert_eq!(
        wide_key(&model, KeyCode::Left),
        Some(BoardIntent::EditMoveLeft)
    );
    assert_eq!(
        wide_key(&model, KeyCode::Right),
        Some(BoardIntent::EditMoveRight)
    );
    for character in ['h', 'l'] {
        assert_eq!(
            wide_key(&model, KeyCode::Char(character)),
            Some(BoardIntent::EditInsert(character)),
            "{character} remains text in a wide editor"
        );
    }
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CancelEdit)
    );
    // An edit session in view mode (add target selected) keeps the page's own Esc.
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_editing());
    assert_eq!(
        wide_key(&model, KeyCode::Left),
        Some(BoardIntent::StageLeft),
        "a parked session slides; the pane stays bound"
    );
    assert_eq!(
        wide_key(&model, KeyCode::Esc),
        Some(BoardIntent::CloseLayer)
    );
    press(&mut domain, &mut model, KeyCode::Esc);
    assert!(!model.task_editing(), "Esc cancels the session first");
    assert_eq!(
        model.wide_stage(),
        WideStage::Rail,
        "and leaves the stage alone"
    );
}

#[test]
fn narrow_routes_are_unchanged_by_the_slider() {
    let (mut domain, mut model) = fixture();
    let narrow =
        |model: &BoardModel, code| route_key(model, ResponsivePresentation::SingleBoard, code);
    assert_eq!(
        narrow(&model, KeyCode::Right),
        Some(BoardIntent::PeekDetail)
    );
    assert_eq!(
        narrow(&model, KeyCode::Left),
        Some(BoardIntent::CollapseDetail)
    );
    assert_eq!(
        narrow(&model, KeyCode::Enter),
        Some(BoardIntent::OpenTaskPage)
    );
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    let page =
        |model: &BoardModel, code| route_key(model, ResponsivePresentation::SingleTask, code);
    assert_eq!(page(&model, KeyCode::Left), None);
    assert_eq!(page(&model, KeyCode::Right), None);
    assert_eq!(page(&model, KeyCode::Esc), Some(BoardIntent::CloseLayer));
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.edit_target(), None);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
}

#[test]
fn shrinking_and_growing_keeps_every_stage_and_its_session() {
    let long = "long notes ".repeat(400);
    for stage in STAGES {
        let mut domain = DomainState::new();
        domain
            .create(
                "resize survivor",
                Some(long.clone()),
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create survivor");
        domain
            .create(
                "other row",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create other");
        let mut model = BoardModel::from_domain(&domain, None);
        let survivor = domain
            .tasks()
            .iter()
            .find(|task| task.title == "resize survivor")
            .expect("survivor")
            .id;
        let index = model
            .visible_ids()
            .iter()
            .position(|&id| id == survivor)
            .expect("survivor row");
        go(&mut domain, &mut model, BoardIntent::SelectIndex(index));
        to_stage(&mut domain, &mut model, stage);
        let _ = render(&model, 130, 24);
        if stage.focused_surface() == FocusedSurface::Task {
            go(&mut domain, &mut model, BoardIntent::PageWheelScrollDown);
            assert!(
                model.page_scroll() > 0,
                "{stage:?}: fixture exercises page scroll"
            );
        }
        let before = (
            model.wide_stage(),
            model.focused_surface(),
            model.selected_id(),
            model.edit_target(),
            model.page_scroll(),
            model.input_mode(),
        );

        let (narrow_rows, _) = render(&model, 109, 24);
        let narrow = resolve_responsive(109, 24, stage);
        match stage.focused_surface() {
            FocusedSurface::Board => {
                assert_eq!(narrow.presentation, ResponsivePresentation::SingleBoard);
                assert!(
                    narrow_rows[1].contains("desk  ·  select project"),
                    "{stage:?}"
                );
            }
            FocusedSurface::Task => {
                assert_eq!(narrow.presentation, ResponsivePresentation::SingleTask);
                assert!(
                    !narrow_rows.join("\n").contains("other row"),
                    "{stage:?}: the page fills the frame"
                );
            }
        }
        assert_eq!(
            (
                model.wide_stage(),
                model.focused_surface(),
                model.selected_id(),
                model.edit_target(),
                model.page_scroll(),
                model.input_mode(),
            ),
            before,
            "{stage:?}: shrink keeps the stage and session"
        );

        let (wide_rows, _) = render(&model, 130, 24);
        assert_eq!(
            (
                model.wide_stage(),
                model.focused_surface(),
                model.selected_id(),
                model.edit_target(),
                model.page_scroll(),
                model.input_mode(),
            ),
            before,
            "{stage:?}: grow keeps the stage and session"
        );
        let geometry = resolve_responsive(130, 24, stage);
        if geometry.task.width > 0 {
            assert!(
                column_text(&wide_rows, geometry.task_content(), 1).contains("resize survivor"),
                "{stage:?}"
            );
        }
        if geometry.board.width > 0 {
            assert!(
                region_text(&wide_rows, geometry.board).contains("resize survivor"),
                "{stage:?}"
            );
        }
    }
}

#[test]
fn shrinking_during_an_edit_keeps_mode_draft_cursor_and_binding() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    go(&mut domain, &mut model, BoardIntent::EditMoveLeft);
    let before = (
        model.wide_stage(),
        model.input_mode(),
        model.edit_target(),
        model.edit_buffer().to_string(),
        model.edit_cursor(),
    );
    assert!(model.task_session_dirty());
    let (narrow_rows, _) = render(&model, 109, 24);
    assert!(narrow_rows.join("\n").contains(&before.3));
    let _ = render(&model, 130, 24);
    assert_eq!(
        (
            model.wide_stage(),
            model.input_mode(),
            model.edit_target(),
            model.edit_buffer().to_string(),
            model.edit_cursor(),
        ),
        before
    );
}

#[test]
fn repeated_threshold_crossings_keep_every_stage_live() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        for width in [109u16, 110].into_iter().cycle().take(20) {
            let (rows, hits) = render(&model, width, 24);
            assert!(!rows.is_empty());
            assert!(!hits.regions.is_empty(), "{stage:?} at {width}");
        }
        assert_eq!(model.wide_stage(), stage);
    }
}

#[test]
fn stage_changes_never_mutate_the_domain() {
    let (mut domain, mut model) = fixture();
    let before = domain.clone();
    for intent in [
        BoardIntent::StageRight,
        BoardIntent::StageRight,
        BoardIntent::StageRight,
        BoardIntent::StageLeft,
        BoardIntent::StageLeft,
        BoardIntent::OpenTaskPage,
        BoardIntent::CloseLayer,
        BoardIntent::StageLeft,
    ] {
        let outcome = apply_intent(&mut domain, &mut model, intent, None).expect("stage intent");
        assert_eq!(outcome, IntentOutcome::None);
    }
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert_eq!(domain.tasks(), before.tasks());
    for width in [109u16, 110, 130, 109] {
        let _ = render(&model, width, 24);
    }
    assert_eq!(domain.tasks(), before.tasks());
}

// ---------------------------------------------------------------------------
// T4 mouse
// ---------------------------------------------------------------------------

const AREA_130: Rect = Rect {
    x: 0,
    y: 0,
    width: 130,
    height: 24,
};

fn row_hit(hits: &QueueHitMap, column: Rect, predicate: impl Fn(uuid::Uuid) -> bool) -> Rect {
    hits.regions
        .iter()
        .find(|hit| {
            inside(column_text_area(column, 24), hit.area)
                && matches!(hit.target, QueueHitTarget::Task(id) if predicate(id))
        })
        .map(|hit| hit.area)
        .expect("row hit")
}

fn click_map(model: &BoardModel, hits: &QueueHitMap, x: u16, y: u16) -> Option<BoardIntent> {
    map_responsive_board_mouse(model, hits, AREA_130, left_click(x, y))
}

#[test]
fn board_row_click_opens_or_retargets_split_without_peeking() {
    for stage in [WideStage::FullBoard, WideStage::Split] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let selected = model.selected_id();
        let geometry = resolve_responsive(130, 24, stage);
        let (rows, hits) = render(&model, 130, 24);
        let other = row_hit(&hits, geometry.board, |id| Some(id) != selected);
        assert_eq!(
            wide_mouse_focus_intent(&model, &hits, AREA_130, left_click(other.x, other.y)),
            None,
            "{stage:?}: a board row is not a task-column click"
        );
        let intent = click_map(&model, &hits, other.x, other.y).expect("row click maps");
        assert!(matches!(intent, BoardIntent::FocusBoardAndSelectIndex(_)));
        go(&mut domain, &mut model, intent);
        assert_ne!(model.selected_id(), selected);
        assert_eq!(model.wide_stage(), WideStage::Split, "{stage:?}");
        assert_eq!(model.focused_surface(), FocusedSurface::Board);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        assert_eq!(model.detail_open(), None, "no peek at wide widths");
        let (after, _) = render(&model, 130, 24);
        assert_ne!(rows, after);
        let split = resolve_responsive(130, 24, WideStage::Split);
        let header = column_text(&after, split.task_content(), 1);
        assert!(
            header.contains("T15 Renew domain"),
            "pane retargets: {header}"
        );
    }
}

#[test]
fn full_board_plain_click_marks_in_mark_mode_while_ctrl_click_keeps_the_wide_route() {
    let (mut domain, mut model) = fixture();
    let selected = model.selected_id();
    let geometry = resolve_responsive(130, 24, WideStage::FullBoard);
    let (_, hits) = render(&model, 130, 24);
    let row = row_hit(&hits, geometry.board, |id| Some(id) != selected);

    go(&mut domain, &mut model, BoardIntent::ToggleMarkMode);
    let mark = click_map(&model, &hits, row.x, row.y).expect("mark-mode row click");
    let BoardIntent::MarkToggleAt(index) = mark else {
        panic!("plain click should mark in mark mode: {mark:?}");
    };
    let marked = model.visible_ids()[index];
    go(&mut domain, &mut model, mark);
    assert!(model.marked_ids().contains(&marked));
    assert_eq!(model.wide_stage(), WideStage::FullBoard);

    let mut ctrl_click = left_click(row.x, row.y);
    ctrl_click.modifiers = KeyModifiers::CONTROL;
    assert!(matches!(
        map_responsive_board_mouse(&model, &hits, AREA_130, ctrl_click),
        Some(BoardIntent::FocusBoardAndSelectIndex(_))
    ));
}

#[test]
fn full_board_click_on_selected_task_opens_split() {
    let (mut domain, mut model) = fixture();
    let selected = model.selected_id().expect("selected task");
    let geometry = resolve_responsive(130, 24, WideStage::FullBoard);
    let (_, hits) = render(&model, 130, 24);
    let row = row_hit(&hits, geometry.board, |id| id == selected);
    let intent = click_map(&model, &hits, row.x, row.y).expect("row click");
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(model.selected_id(), Some(selected));
    assert_eq!(model.focused_surface(), FocusedSurface::Board);
    assert_eq!(model.detail_open(), None);
}

#[test]
fn stage_a_task_column_click_moves_to_g_then_dispatches_against_the_painted_frame() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (_, hits) = render(&model, 130, 24);
    let add = hits
        .regions
        .iter()
        .find(|hit| {
            inside(column_text_area(geometry.task_content(), 24), hit.area)
                && matches!(hit.target, QueueHitTarget::StepAdd)
        })
        .expect("preview add-step control")
        .area;
    let click = left_click(add.x, add.y);
    assert_eq!(
        click_map(&model, &hits, add.x, add.y),
        None,
        "board focus: inert"
    );
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        Some(BoardIntent::StageRight)
    );
    // Blank pane space slides too; the footer never does.
    let blank = left_click(geometry.task_content().x + 10, 18);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, blank),
        Some(BoardIntent::StageRight)
    );
    let footer = left_click(geometry.task_content().x + 10, 23);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, footer),
        None
    );

    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(
        click_map(&model, &hits, add.x, add.y),
        Some(BoardIntent::BeginAddStep),
        "the painted frame's control dispatches after the stage move"
    );
}

#[test]
fn stage_a_task_column_click_without_selection_is_inert() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    model.sync_from_domain(&DomainState::new());
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (_, hits) = render(&model, 130, 24);
    let click = left_click(geometry.task_content().x + 4, 4);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        None
    );
    assert_eq!(click_map(&model, &hits, click.column, click.row), None);
}

#[test]
fn left_side_click_moves_focus_left_from_the_rail() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let bound = model.edit_target().expect("bound page");
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (_, hits) = render(&model, 130, 24);

    // A rail row click retargets the pane and lands the board beside it.
    let other = row_hit(&hits, geometry.board, |id| id != bound);
    let intent = click_map(&model, &hits, other.x, other.y).expect("rail row maps");
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(model.focused_surface(), FocusedSurface::Board);
    assert_ne!(model.edit_target(), Some(bound));
    assert_eq!(model.edit_target(), model.selected_id());
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    let split = resolve_responsive(130, 24, WideStage::Split);
    let (rows, _) = render(&model, 130, 24);
    assert!(column_text(&rows, split.task_content(), 1).contains("T15 Renew domain"));

    // Back to G: blank rail space slides left too.
    to_stage(&mut domain, &mut model, WideStage::Rail);
    assert_eq!(
        click_map(&model, &hits, 2, 9),
        Some(BoardIntent::StageLeft),
        "blank rail space moves focus left"
    );
    // Rail furniture (tabs, headers) is part of the left side; the rule itself is inert.
    assert_eq!(
        click_map(&model, &hits, 2, 1),
        Some(BoardIntent::StageLeft),
        "rail tab"
    );
    assert_eq!(
        click_map(&model, &hits, geometry.rule.x, 5),
        None,
        "rule column"
    );
}

#[test]
fn row_double_click_opens_the_full_page_and_records_the_origin() {
    for stage in [WideStage::FullBoard, WideStage::Split, WideStage::Rail] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 24, stage);
        let (_, hits) = render(&model, 130, 24);
        let selected = model.selected_id();
        let row = row_hit(&hits, geometry.board, |id| Some(id) != selected);
        let intent = click_map(&model, &hits, row.x, row.y).expect("row click");
        go(&mut domain, &mut model, intent);
        let after_first = WideStage::Split;
        assert_eq!(model.wide_stage(), after_first, "{stage:?}: first click");
        let (_, hits) = render(&model, 130, 24);
        // Rail rows can reflow vertically when the board expands. Keep that existing
        // case task-based; full-board and split double-clicks use the same coordinates.
        let row = if stage == WideStage::Rail {
            let split = resolve_responsive(130, 24, WideStage::Split);
            row_hit(&hits, split.board, |id| Some(id) == model.selected_id())
        } else {
            row
        };
        let intent = click_map(&model, &hits, row.x, row.y).expect("second row click");
        go(&mut domain, &mut model, intent);
        assert_eq!(
            model.wide_stage(),
            WideStage::FullTask,
            "{stage:?}: double-click opens F"
        );
        assert_eq!(model.stage_origin(), Some(after_first), "{stage:?}");
        assert_eq!(model.edit_target(), model.selected_id());
        assert_ne!(model.selected_id(), selected);
    }
}

#[test]
fn footer_verbs_dispatch_for_the_focused_surface_in_every_stage() {
    for stage in STAGES {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let (rows, hits) = render(&model, 130, 24);
        let verb_row = &rows[23];
        let done_x = u16::try_from(verb_row.find("ctrl+d done").expect("done verb")).expect("x");
        let intent = click_map(&model, &hits, done_x, 23);
        assert_eq!(intent, Some(BoardIntent::Complete), "{stage:?}: {verb_row}");
        if stage.focused_surface() == FocusedSurface::Board {
            if let Some(add_x) = verb_row.find("+ add") {
                assert_eq!(
                    click_map(&model, &hits, u16::try_from(add_x).expect("x"), 23),
                    Some(BoardIntent::OpenCapture),
                    "{stage:?}: footer add verb routes"
                );
            }
        }
        let id = model.selected_id().expect("selection");
        go(&mut domain, &mut model, intent.expect("verb"));
        assert_eq!(
            domain.get(id).expect("task").status,
            HumanStatus::Done,
            "{stage:?}"
        );
    }
}

#[test]
fn projects_split_wheel_starts_from_the_painted_index_scroll() {
    let mut domain = DomainState::new();
    for index in 0..24 {
        domain
            .create(
                format!("project task {index}"),
                None,
                TaskScope::Project {
                    path: format!("/repos/project-{index:02}"),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("project task");
    }
    let mut model = BoardModel::from_domain(&domain, None);
    go(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(tsk_tui::ui::queue::NavTab::Projects),
    );
    go(&mut domain, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Split);

    for _ in 0..23 {
        go(&mut domain, &mut model, BoardIntent::SelectNext);
    }
    assert_eq!(model.project_rows().len(), 24);
    assert_eq!(model.projects_cursor(), 23);
    let (_, _) = render(&model, WIDE_SPLIT_MIN_WIDTH, 30);
    let painted_scroll = model.list_scroll();
    assert!(
        painted_scroll > 0,
        "selected project must follow the viewport"
    );

    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 2,
        row: 6,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        map_responsive_board_mouse(
            &model,
            &QueueHitMap::default(),
            Rect {
                x: 0,
                y: 0,
                width: WIDE_SPLIT_MIN_WIDTH,
                height: 30,
            },
            wheel
        ),
        Some(BoardIntent::ListScrollTo(painted_scroll + 1)),
        "wheel reads the renderer's painted index scroll"
    );
}

#[test]
fn wheel_scrolls_only_the_focused_column() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (_, hits) = render(&model, 130, 24);
    let wheel = |x, y| MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        map_responsive_board_mouse(
            &model,
            &hits,
            AREA_130,
            wheel(geometry.task_content().x + 5, 6)
        ),
        Some(BoardIntent::PageWheelScrollDown)
    );
    assert_eq!(
        map_responsive_board_mouse(&model, &hits, AREA_130, wheel(4, 6)),
        None,
        "the rail is not the focused column"
    );
    go(&mut domain, &mut model, BoardIntent::StageLeft);
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (_, hits) = render(&model, 130, 24);
    assert!(matches!(
        map_responsive_board_mouse(&model, &hits, AREA_130, wheel(4, 6)),
        Some(BoardIntent::ListScrollTo(_))
    ));
    assert_eq!(
        map_responsive_board_mouse(
            &model,
            &hits,
            AREA_130,
            wheel(geometry.task_content().x + 5, 6)
        ),
        None,
        "the preview is not the focused column"
    );
}

#[test]
fn help_and_palette_close_on_task_column_clicks_without_dispatching() {
    for (open, expected, mode) in [
        (
            BoardIntent::OpenHelp,
            BoardIntent::CloseHelp,
            BoardInputMode::Help,
        ),
        (
            BoardIntent::OpenCommandPalette,
            BoardIntent::CloseCommandSurface,
            BoardInputMode::Palette,
        ),
    ] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, WideStage::Split);
        go(&mut domain, &mut model, open);
        assert_eq!(model.input_mode(), mode);
        let geometry = resolve_responsive(130, 24, WideStage::Split);
        let (_, hits) = render(&model, 130, 24);
        let click = left_click(geometry.task_content().x + 6, 6);
        assert_eq!(
            wide_mouse_focus_intent(&model, &hits, AREA_130, click),
            None
        );
        assert_eq!(
            click_map(&model, &hits, click.column, click.row),
            Some(expected)
        );
        if mode == BoardInputMode::Help {
            let wheel = MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: geometry.task_content().x + 6,
                row: 6,
                modifiers: KeyModifiers::NONE,
            };
            assert_eq!(
                map_responsive_board_mouse(&model, &hits, AREA_130, wheel),
                Some(BoardIntent::HelpScrollDown),
                "Help owns wheel input across the centered card"
            );
        }
    }
}

#[test]
fn quick_add_ignores_task_column_clicks_and_keeps_its_draft() {
    let (mut domain, mut model) = fixture();
    let original = domain.tasks().len();
    to_stage(&mut domain, &mut model, WideStage::Split);
    go(&mut domain, &mut model, BoardIntent::OpenCapture);
    go(
        &mut domain,
        &mut model,
        BoardIntent::QuickAddInsertText("draft task".to_string()),
    );
    let draft = model.quick_add_title_value().to_string();
    let geometry = resolve_responsive(130, 24, WideStage::Split);
    let (rows, hits) = render(&model, 130, 24);
    assert!(
        rows.join("\n").contains("draft task"),
        "the footer paints the input"
    );
    let click = left_click(geometry.task_content().x + 6, 6);
    assert_eq!(
        wide_mouse_focus_intent(&model, &hits, AREA_130, click),
        None
    );
    let mapped = click_map(&model, &hits, click.column, click.row);
    if let Some(intent) = mapped.clone() {
        go(&mut domain, &mut model, intent);
    }
    assert_eq!(mapped, None);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    assert_eq!(model.quick_add_title_value(), draft);
    assert_eq!(domain.tasks().len(), original);
}

fn hit_signature(hits: &QueueHitMap, column: Rect) -> Vec<String> {
    let mut targets: Vec<String> = hits
        .regions
        .iter()
        .filter(|hit| inside(column, hit.area))
        .filter_map(|hit| match hit.target {
            QueueHitTarget::Step(index) => Some(format!("Step({index})")),
            QueueHitTarget::StepAdd => Some("StepAdd".to_string()),
            QueueHitTarget::FormNotes(_) => Some("FormNotes".to_string()),
            QueueHitTarget::FormThread => Some("FormThread".to_string()),
            QueueHitTarget::FormScopeOption(index) => Some(format!("FormScopeOption({index})")),
            QueueHitTarget::Verb(index) => Some(format!("Verb({index})")),
            _ => None,
        })
        .collect();
    targets.sort();
    targets.dedup();
    targets
}

fn first_hit(hits: &QueueHitMap, column: Rect, name: &str) -> Rect {
    hits.regions
        .iter()
        .find(|hit| {
            inside(column, hit.area)
                && match hit.target {
                    QueueHitTarget::Step(index) => format!("Step({index})") == name,
                    QueueHitTarget::StepAdd => name == "StepAdd",
                    QueueHitTarget::FormNotes(_) => name == "FormNotes",
                    QueueHitTarget::FormThread => name == "FormThread",
                    QueueHitTarget::FormScopeOption(index) => {
                        format!("FormScopeOption({index})") == name
                    }
                    QueueHitTarget::Verb(index) => format!("Verb({index})") == name,
                    _ => false,
                }
        })
        .map(|hit| hit.area)
        .expect("named hit")
}

fn session_signature(model: &BoardModel) -> impl PartialEq + std::fmt::Debug {
    (
        model.input_mode(),
        model.selected_id(),
        model.edit_target(),
        model.page_scroll(),
        model.step_cursor(),
        model.form_focus(),
        model.edit_buffer().to_string(),
        model.edit_cursor(),
        model.task_editing(),
    )
}

fn assert_same_outcome(
    domain: &DomainState,
    single: &BoardModel,
    wide: &BoardModel,
    intent: BoardIntent,
    label: &str,
) {
    let mut single_domain = domain.clone();
    let mut wide_domain = domain.clone();
    let mut single_model = single.clone();
    let mut wide_model = wide.clone();
    let single_outcome = apply_intent(&mut single_domain, &mut single_model, intent.clone(), None)
        .expect("single-pane action");
    let wide_outcome =
        apply_intent(&mut wide_domain, &mut wide_model, intent, None).expect("wide action");
    assert_eq!(single_outcome, wide_outcome, "{label}: outcome");
    assert_eq!(
        single_domain.tasks(),
        wide_domain.tasks(),
        "{label}: domain"
    );
    assert_eq!(
        session_signature(&single_model),
        session_signature(&wide_model),
        "{label}: session"
    );
}

/// The G and F task surfaces expose the same controls as the single-pane page, and every
/// shared control maps to the same intent with the same domain and session outcome.
#[test]
fn task_surface_controls_in_g_and_f_match_the_single_pane_page() {
    let (mut domain, mut base) = fixture();
    let id = base.selected_id().expect("selection");
    domain.add_step(id, "first parity step").expect("step");
    domain.add_step(id, "second parity step").expect("step");
    base.sync_from_domain(&domain);

    // The single-pane reference: the same task open as the full page below the threshold.
    let mut single = base.clone();
    go(&mut domain.clone(), &mut single, BoardIntent::OpenTaskPage);
    let single_area = Rect::new(0, 0, 100, 24);
    let single_column = Rect::new(0, 0, 100, 21);

    let edits: Vec<(&str, Vec<BoardIntent>)> = vec![
        ("task view", vec![]),
        ("title edit", vec![BoardIntent::BeginEditTitle]),
        ("notes edit", vec![BoardIntent::BeginEditNotes]),
        (
            "thread selection",
            vec![
                BoardIntent::BeginEditTitle,
                BoardIntent::FocusFormField(CaptureField::Thread),
            ],
        ),
        (
            "step edit",
            vec![BoardIntent::BeginEditTitle, BoardIntent::SelectStep(0)],
        ),
    ];
    for stage in [WideStage::Rail, WideStage::FullTask] {
        for (label, steps) in &edits {
            let label = format!("{stage:?} {label}");
            let mut single_model = single.clone();
            let mut wide_model = base.clone();
            let mut wide_domain = domain.clone();
            to_stage(&mut wide_domain, &mut wide_model, stage);
            for intent in steps {
                go(&mut domain.clone(), &mut single_model, intent.clone());
                go(&mut wide_domain, &mut wide_model, intent.clone());
            }
            assert_eq!(
                single_model.input_mode(),
                wide_model.input_mode(),
                "{label}"
            );
            let (_, single_hits) = render(&single_model, single_area.width, single_area.height);
            let (_, wide_hits) = render(&wide_model, 130, 24);
            let geometry = resolve_responsive(130, 24, stage);
            let wide_column = Rect::new(
                geometry.task_content().x,
                0,
                geometry.task_content().width,
                21,
            );
            let single_targets = hit_signature(&single_hits, single_column);
            let wide_targets = hit_signature(&wide_hits, wide_column);
            assert_eq!(
                single_targets, wide_targets,
                "{label}: task-surface controls"
            );
            assert!(
                single_targets.iter().any(|t| t.starts_with("Step(")),
                "{label}: steps painted"
            );
            for name in &single_targets {
                let single_rect = first_hit(&single_hits, single_column, name);
                let wide_rect = first_hit(&wide_hits, wide_column, name);
                let single_intent = map_board_mouse(
                    &single_model,
                    &single_hits,
                    left_click(single_rect.x, single_rect.y),
                );
                let wide_intent = map_responsive_board_mouse(
                    &wide_model,
                    &wide_hits,
                    AREA_130,
                    left_click(wide_rect.x, wide_rect.y),
                );
                assert_eq!(single_intent, wide_intent, "{label}: {name}");
                if let Some(intent) = single_intent {
                    assert_same_outcome(
                        &domain,
                        &single_model,
                        &wide_model,
                        intent,
                        &format!("{label}: {name}"),
                    );
                }
            }
            // Footer verbs paint the same legend and map the same way.
            let (single_rows, _) = render(&single_model, single_area.width, single_area.height);
            let (wide_rows, _) = render(&wide_model, 130, 24);
            assert_eq!(
                single_rows[23].trim_end(),
                wide_rows[23].trim_end(),
                "{label}: verb bar"
            );
            // Keyboard parity: the page's own keys map identically through the wide router.
            for code in [
                KeyCode::Char('x'),
                KeyCode::Enter,
                KeyCode::Down,
                KeyCode::Tab,
            ] {
                let single_intent = map_key(single_model.input_mode(), key(code));
                let wide_intent = route_key(&wide_model, ResponsivePresentation::WideSplit, code);
                assert_eq!(single_intent, wide_intent, "{label}: {code:?}");
            }
            for code in [KeyCode::Char('e'), KeyCode::Char('d')] {
                let ctrl = KeyEvent::new(code, KeyModifiers::CONTROL);
                assert_eq!(
                    map_key(single_model.input_mode(), ctrl),
                    match route_responsive_key(
                        wide_model.input_mode(),
                        wide_model.wide_stage(),
                        ResponsivePresentation::WideSplit,
                        ctrl,
                    ) {
                        ResponsiveKeyRoute::Intent(intent) => Some(intent),
                        ResponsiveKeyRoute::Inert => None,
                        ResponsiveKeyRoute::Surface => {
                            map_key(wide_model.input_mode(), ctrl)
                        }
                    },
                    "{label}: ctrl+{code:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// T5 dirty drafts across stages
// ---------------------------------------------------------------------------

/// Stage G with a dirty title draft parked in the page's edit session (view mode).
fn dirty_session_in_g() -> (DomainState, BoardModel) {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_editing());
    assert!(model.task_session_dirty());
    (domain, model)
}

/// Stage G with the title editor open and dirty.
fn dirty_editor_in_g() -> (DomainState, BoardModel) {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert!(model.task_session_dirty());
    (domain, model)
}

/// The title row of the task column header; the state slot is its right end.
fn header_title_row(model: &BoardModel, stage: WideStage) -> String {
    let geometry = resolve_responsive(130, 24, stage);
    let (rows, _) = render(model, 130, 24);
    column_text(&rows, geometry.task_content(), 1)
        .trim_end()
        .to_string()
}

#[test]
fn dirty_draft_slides_left_to_a_and_right_back_to_g_untouched() {
    let (mut domain, mut model) = dirty_session_in_g();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    assert!(header_title_row(&model, WideStage::Rail).ends_with("unsaved"));

    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Left),
        Some(BoardIntent::StageLeft),
        "← from G is allowed with a dirty draft"
    );
    assert_eq!(model.wide_stage(), WideStage::Split);
    assert_eq!(model.input_mode(), BoardInputMode::Normal);
    assert_eq!(model.edit_target(), Some(bound), "the pane stays bound");
    assert_eq!(model.selected_id(), Some(bound));
    assert!(header_title_row(&model, WideStage::Split).ends_with("unsaved"));
    assert!(model.message().is_none(), "no refusal for a stage move");

    assert_eq!(
        press(&mut domain, &mut model, KeyCode::Right),
        Some(BoardIntent::StageRight)
    );
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(model.edit_target(), Some(bound));
    assert_eq!(model.edit_buffer(), draft, "the draft is untouched");
    assert!(model.task_editing());
    assert!(model.task_session_dirty());
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
}

#[test]
fn dirty_draft_refuses_keyboard_retarget_in_stage_a() {
    let (mut domain, mut model) = dirty_session_in_g();
    press(&mut domain, &mut model, KeyCode::Left);
    assert_eq!(model.wide_stage(), WideStage::Split);
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    for code in [KeyCode::Char('j'), KeyCode::Char('k')] {
        let intent = press(&mut domain, &mut model, code).expect("select key");
        assert!(matches!(
            intent,
            BoardIntent::SelectNext | BoardIntent::SelectPrev
        ));
        assert_eq!(
            model.selected_id(),
            Some(bound),
            "{code:?}: selection refused"
        );
        assert_eq!(model.edit_target(), Some(bound));
        assert_eq!(model.edit_buffer(), draft);
        assert_eq!(model.wide_stage(), WideStage::Split);
        assert!(model
            .message()
            .is_some_and(|message| message.contains("save or cancel")));
    }
    let (rows, _) = render(&model, 130, 24);
    assert!(
        rows[22].contains("save or cancel"),
        "refusal paints on the status row"
    );
}

#[test]
fn dirty_draft_refuses_rail_row_click_and_keeps_the_editor() {
    let (mut domain, mut model) = dirty_editor_in_g();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (_, hits) = render(&model, 130, 24);
    let other = row_hit(&hits, geometry.board, |id| id != bound);
    let intent = click_map(&model, &hits, other.x, other.y)
        .expect("a rail row stays an explicit retarget attempt");
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(model.selected_id(), Some(bound));
    assert_eq!(model.edit_target(), Some(bound));
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    assert_eq!(model.edit_buffer(), draft);
    assert!(model
        .message()
        .is_some_and(|message| message.contains("save or cancel")));

    // The same bound row is not a retarget at all.
    let same = row_hit(&hits, geometry.board, |id| id == bound);
    assert_eq!(click_map(&model, &hits, same.x, same.y), None);
}

#[test]
fn dirty_refusal_clears_after_save_and_the_pane_retargets_again() {
    let (mut domain, mut model) = dirty_editor_in_g();
    let bound = model.edit_target().expect("bound");
    let draft = model.edit_buffer().to_string();
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    assert!(
        model.message().is_some(),
        "SelectNext in an editor is a refused retarget"
    );
    assert!(header_title_row(&model, WideStage::Rail).ends_with("editing title"));

    go(&mut domain, &mut model, BoardIntent::ConfirmEdit);
    assert!(model.message().is_none());
    assert!(!model.task_session_dirty());
    assert_eq!(domain.get(bound).expect("saved").title, draft);
    assert!(header_title_row(&model, WideStage::Rail).ends_with("started · tsk"));

    press(&mut domain, &mut model, KeyCode::Left);
    assert_eq!(model.wide_stage(), WideStage::Split);
    press(&mut domain, &mut model, KeyCode::Char('j'));
    assert_ne!(
        model.selected_id(),
        Some(bound),
        "retarget works after the save"
    );
    assert_eq!(model.edit_target(), model.selected_id());
}

#[test]
fn dirty_refusal_clears_after_cancel() {
    let (mut domain, mut model) = dirty_editor_in_g();
    let bound = model.edit_target().expect("bound");
    let saved = domain.get(bound).expect("bound").title.clone();
    go(&mut domain, &mut model, BoardIntent::SelectNext);
    assert!(model.message().is_some());
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    assert!(model.message().is_none());
    assert!(!model.task_session_dirty());
    assert_eq!(model.edit_buffer(), saved);
    assert!(header_title_row(&model, WideStage::Rail).ends_with("started · tsk"));
    press(&mut domain, &mut model, KeyCode::Left);
    press(&mut domain, &mut model, KeyCode::Char('j'));
    assert_ne!(model.selected_id(), Some(bound));
}

#[test]
fn clean_editor_rail_row_click_retargets_and_moves_focus_left() {
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert!(!model.task_session_dirty());
    let bound = model.edit_target().expect("bound");
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (_, hits) = render(&model, 130, 24);
    let other = row_hit(&hits, geometry.board, |id| id != bound);
    let intent = click_map(&model, &hits, other.x, other.y).expect("clean retarget");
    go(&mut domain, &mut model, intent);
    assert_eq!(model.wide_stage(), WideStage::Split, "focus moves left");
    assert_ne!(model.edit_target(), Some(bound));
    assert_eq!(model.edit_target(), model.selected_id());
    assert_eq!(model.input_mode(), BoardInputMode::Normal, "the page parks");
    assert!(model.message().is_none());
}

#[test]
fn changed_thread_and_scope_drafts_refuse_retarget_and_paint_unsaved() {
    // Thread.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Thread),
    );
    go(&mut domain, &mut model, BoardIntent::ToggleThreadEditing);
    go(&mut domain, &mut model, BoardIntent::EditInsert('x'));
    assert!(header_title_row(&model, WideStage::Rail).ends_with("editing thread"));
    let bound = model.edit_target().expect("bound");
    let other = model
        .visible_ids()
        .iter()
        .position(|&id| id != bound)
        .expect("other row");
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusBoardAndSelectIndex(other),
    );
    assert_eq!(model.edit_target(), Some(bound));
    assert_eq!(model.selected_id(), Some(bound));
    assert!(model
        .message()
        .is_some_and(|message| message.contains("save or cancel")));

    // Scope, then back to view mode: the header slot reads `unsaved`.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Scope),
    );
    let original = model.form_scope().cloned();
    go(&mut domain, &mut model, BoardIntent::FormCycleScope);
    assert_ne!(model.form_scope(), original.as_ref());
    assert!(header_title_row(&model, WideStage::Rail).ends_with("editing scope"));
    let bound = model.edit_target().expect("bound");
    let other = model
        .visible_ids()
        .iter()
        .position(|&id| id != bound)
        .expect("other row");
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusBoardAndSelectIndex(other),
    );
    assert_eq!(model.edit_target(), Some(bound));
    assert!(model.message().is_some());
    for _ in 0..8 {
        if model.input_mode() == BoardInputMode::TaskPage {
            break;
        }
        go(&mut domain, &mut model, BoardIntent::FormFocusNext);
    }
    assert_eq!(model.input_mode(), BoardInputMode::TaskPage);
    assert!(model.task_session_dirty());
    assert!(header_title_row(&model, WideStage::Rail).ends_with("unsaved"));
}

#[test]
fn dirty_scope_dropdown_same_bound_row_click_keeps_the_visible_editor() {
    let (mut domain, mut model) = dirty_editor_in_g();
    let draft = model.edit_buffer().to_string();
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Scope),
    );
    go(&mut domain, &mut model, BoardIntent::OpenFormScopeDropdown);
    assert_eq!(model.input_mode(), BoardInputMode::FormScopeDropdown);
    let bound = model.edit_target().expect("bound");
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (_, hits) = render(&model, 130, 24);
    let row = row_hit(&hits, geometry.board, |id| id == bound);
    let mapped = click_map(&model, &hits, row.x, row.y);
    if let Some(intent) = mapped.clone() {
        go(&mut domain, &mut model, intent);
    }
    assert_eq!(mapped, None);
    assert_eq!(model.input_mode(), BoardInputMode::FormScopeDropdown);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    go(
        &mut domain,
        &mut model,
        BoardIntent::CancelFormScopeDropdown,
    );
    go(
        &mut domain,
        &mut model,
        BoardIntent::FocusFormField(CaptureField::Title),
    );
    assert_eq!(model.edit_buffer(), draft);
}

#[test]
fn board_focused_field_edit_enters_the_task_stage() {
    // From A, Ctrl+E enters G with the editor live in the column.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Split);
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(model.input_mode(), BoardInputMode::EditTitle);
    go(&mut domain, &mut model, BoardIntent::EditInsert('!'));
    let geometry = resolve_responsive(130, 24, WideStage::Rail);
    let (rows, _) = render(&model, 130, 24);
    let header = column_text(&rows, geometry.task_content(), 1);
    assert!(header.contains(model.edit_buffer()), "{header}");
    assert!(header.trim_end().ends_with("editing title"));

    // From 0, Ctrl+E opens the full page with the bare board as its origin.
    let (mut domain, mut model) = fixture();
    go(&mut domain, &mut model, BoardIntent::BeginEditTitle);
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    assert_eq!(model.stage_origin(), Some(WideStage::FullBoard));
    go(&mut domain, &mut model, BoardIntent::CancelEdit);
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(
        model.wide_stage(),
        WideStage::FullTask,
        "first Esc ends the session"
    );
    assert!(!model.task_editing());
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
    assert_eq!(model.edit_target(), None);
}

// Review-panel fixes on `main..b625955`.

#[test]
fn task_column_header_number_click_copies_in_g_and_f() {
    // F-1/F-2: the column pushed `FormTitle` after `TaskNumber`, so the full-row title hit
    // shadowed the copy hit (`hit_at` searches newest first). The single-pane page pushes the
    // other way round; the column must match it.
    for stage in [WideStage::Rail, WideStage::FullTask] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let area = Rect::new(0, 0, 130, 24);
        let (_, hits) = render_buffer(&model, 130, 24);
        let column = resolve_responsive(130, 24, stage).task;
        let number = hits
            .regions
            .iter()
            .find(|hit| {
                matches!(hit.target, QueueHitTarget::TaskNumber(_))
                    && column.contains(hit.area.as_position())
            })
            .expect("header number hit")
            .area;
        let intent =
            map_responsive_board_mouse(&model, &hits, area, left_click(number.x, number.y));
        assert_eq!(
            intent,
            Some(BoardIntent::CopyTaskNumber(model.selected_id().unwrap())),
            "{stage:?}: clicking T<n> in the column header copies it"
        );
    }
}

/// The wide column derives its header identifier from the task, not a stored literal:
/// a notice must paint `N<n>` in both task stages and keep its click-to-copy hit, or
/// every seeded guide loses its header id when the slider is wide.
#[test]
fn wide_task_column_header_derives_the_notice_n_identifier_and_its_copy_hit() {
    let mut domain = DomainState::new();
    let notice = domain
        .create_notice(
            "guide.wide",
            "Wide guide header",
            None,
            HumanStatus::Ready,
            TaskScope::Global,
            Vec::new(),
        )
        .expect("seed notice");
    // The model carries the persisted N number; the domain shares the id.
    let mut tasks = domain.tasks().to_vec();
    tasks[0].notice = Some(Notice {
        catalog_id: "guide.wide".into(),
        number: Some(2),
    });
    let model = BoardModel::from_tasks(tasks, Some(PathBuf::from(REPO)));
    assert_eq!(model.selected_id(), Some(notice));

    for stage in [WideStage::Rail, WideStage::FullTask] {
        let (mut domain, mut model) = (domain.clone(), model.clone());
        to_stage(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 24, stage);
        let (rows, hits) = render(&model, 130, 24);
        let header = column_text(&rows, geometry.task_content(), 1);
        assert!(
            header.contains("N2 Wide guide header"),
            "{stage:?}: header must derive the notice identifier: {header}"
        );
        let identifier = hits
            .regions
            .iter()
            .find(|hit| {
                matches!(hit.target, QueueHitTarget::TaskNumber(id) if id == notice)
                    && geometry.task.contains(hit.area.as_position())
            })
            .expect("header identifier copy hit")
            .area;
        let painted: String = rows[identifier.y as usize]
            .chars()
            .skip(identifier.x as usize)
            .take(identifier.width as usize)
            .collect();
        assert_eq!(painted, "N2", "{stage:?}: the hit covers the identifier");
        let intent = map_responsive_board_mouse(
            &model,
            &hits,
            Rect::new(0, 0, 130, 24),
            left_click(identifier.x, identifier.y),
        );
        assert_eq!(
            intent,
            Some(BoardIntent::CopyTaskNumber(notice)),
            "{stage:?}: clicking N<n> in the column header copies it"
        );
    }
}

#[test]
fn rail_without_a_selection_cannot_slide_into_full_task() {
    // F-5/F-7: a disk merge can remove the task shown in G. `→` must then stay in G (AC-21:
    // A, G and F need a selected task), not open an empty F.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let mut empty = DomainState::new();
    model.sync_from_domain(&empty);
    assert_eq!(model.selected_id(), None);
    assert_eq!(model.wide_stage(), WideStage::Rail);

    go(&mut empty, &mut model, BoardIntent::StageRight);
    assert_eq!(model.wide_stage(), WideStage::Rail);
    assert_eq!(model.stage_origin(), None);
}

#[test]
fn soft_delete_from_a_task_stage_returns_the_board_to_a_board_stage() {
    // F-9: deleting the page's own task cleared the form and dropped to Normal mode while the
    // stage stayed task-owned, which left `←`/`→` inert on an empty pane.
    for (stage, expected) in [
        (WideStage::Rail, WideStage::Split),
        (WideStage::FullTask, WideStage::Split),
    ] {
        let (mut domain, mut model) = fixture();
        to_stage(&mut domain, &mut model, stage);
        let deleted = model.selected_id().unwrap();
        go(&mut domain, &mut model, BoardIntent::SoftDelete);
        go(&mut domain, &mut model, BoardIntent::SoftDelete);
        assert!(!model.visible_ids().contains(&deleted));
        assert_eq!(
            model.wide_stage(),
            expected,
            "{stage:?} lands on a board stage"
        );
        assert_eq!(model.focused_surface(), FocusedSurface::Board);
        assert_eq!(model.input_mode(), BoardInputMode::Normal);
        // The slider still answers.
        let route = route_responsive_key(
            model.input_mode(),
            model.wide_stage(),
            ResponsivePresentation::WideSplit,
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        );
        assert_eq!(route, ResponsiveKeyRoute::Intent(BoardIntent::StageLeft));
    }

    // From F entered off the full board, deletion returns to the full board.
    let (mut domain, mut model) = fixture();
    go(&mut domain, &mut model, BoardIntent::OpenTaskPage);
    assert_eq!(model.wide_stage(), WideStage::FullTask);
    go(&mut domain, &mut model, BoardIntent::SoftDelete);
    go(&mut domain, &mut model, BoardIntent::SoftDelete);
    assert_eq!(model.wide_stage(), WideStage::FullBoard);
}

#[test]
fn rail_cells_carry_no_bold() {
    // F-3: the rail's BOLD strip used `set_style(add_modifier)`, which cannot remove a bit.
    let (mut domain, mut model) = fixture();
    to_stage(&mut domain, &mut model, WideStage::Rail);
    let (buffer, _) = render_buffer(&model, 130, 24);
    let rail = resolve_responsive(130, 24, WideStage::Rail).board;
    let footer_top = tsk_tui::ui::tier::resolve(130, 24)
        .rule_row
        .expect("footer rule");
    for y in rail.top()..footer_top {
        for x in rail.left()..rail.right() {
            let cell = &buffer[(x, y)];
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "rail cell ({x},{y}) {:?} is bold",
                cell.symbol()
            );
        }
    }
}

#[test]
fn collapsed_wide_board_titles_fill_the_space_previously_reserved_for_attribution() {
    for stage in [WideStage::FullBoard, WideStage::Split] {
        let (mut domain, mut model) = fixture_with_titles(&"X".repeat(180), "second");
        to_stage(&mut domain, &mut model, stage);
        let geometry = resolve_responsive(130, 40, stage);
        let (rows, _) = render(&model, 130, 40);
        let line = (0..rows.len())
            .map(|y| column_text(&rows, geometry.board, y as u16))
            .find(|line| line.contains("T12 X"))
            .unwrap();
        assert!(!line.contains("tsk"), "no row-end attribution: {line}");
        assert_eq!(
            line.trim_end().chars().count(),
            geometry.board.width as usize - 2,
            "title reaches two-cell margin: {line}"
        );
    }
}

/// Regression: at wide widths `Tab` on the quick-add line used to expand into a draft that
/// the slider never painted (the wide column paints only task forms), so the board stayed
/// on screen while keys went to an invisible page.
#[test]
fn expanded_quick_add_draft_paints_at_wide_widths_and_esc_returns_to_the_line() {
    let (mut domain, mut model) = fixture();
    go(&mut domain, &mut model, BoardIntent::OpenCapture);
    for c in "buy milk".chars() {
        go(&mut domain, &mut model, BoardIntent::QuickAddInsert(c));
    }
    go(&mut domain, &mut model, BoardIntent::ExpandQuickAdd);
    assert_eq!(model.input_mode(), BoardInputMode::EditNotes);
    for width in [110u16, 140, 200] {
        let (buffer, _) = render_buffer(&model, width, 30);
        let rows = rows_of(&buffer);
        let text = rows.join("\n");
        assert!(
            text.contains("buy milk"),
            "{width} cols: draft title missing\n{text}"
        );
        assert!(
            text.contains("shift+enter save"),
            "{width} cols: draft verb bar missing\n{text}"
        );
        assert!(
            !text.contains("IN MOTION"),
            "{width} cols: board still painted\n{text}"
        );
    }
    go(&mut domain, &mut model, BoardIntent::CloseLayer);
    assert_eq!(model.input_mode(), BoardInputMode::QuickAdd);
    let (buffer, _) = render_buffer(&model, 140, 30);
    let text = rows_of(&buffer).join("\n");
    assert!(text.contains("IN MOTION"), "board should be back\n{text}");
    assert!(
        text.contains("buy milk"),
        "quick-add line should keep the title\n{text}"
    );
}
