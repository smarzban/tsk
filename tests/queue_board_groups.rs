//! Group by, sort by, fold, and the desk filter (#175, #158): keys, pickers, folding,
//! group marks, and the selection / mark invariants they must keep.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, BoardInputMode, BoardModel, FilterTab, ListPickerKind,
};
use tsk_tui::ui::input::{map_key, BoardIntent};
use tsk_tui::ui::mouse::{left_click, map_board_mouse};
use tsk_tui::ui::queue::{is_group_header_id, GroupBy, NavTab, SectionKind, SortBy};
use tsk_tui::ui::render::QueueHitTarget;
use uuid::Uuid;

const ALPHA: &str = "/repos/alpha";
const BETA: &str = "/repos/beta";

fn project(path: &str) -> TaskScope {
    TaskScope::Project {
        path: path.to_string(),
    }
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn add(
    domain: &mut DomainState,
    title: &str,
    scope: TaskScope,
    thread: Option<&str>,
    status: HumanStatus,
) -> Uuid {
    let id = domain
        .create(
            title,
            None,
            scope,
            ProvenanceOrigin::Manual,
            thread.map(str::to_string),
        )
        .expect("create");
    if status != HumanStatus::Open {
        domain.set_status(id, status).expect("status");
    }
    id
}

/// Started work in two projects and the desk, plus one blocked task.
struct Board {
    domain: DomainState,
    model: BoardModel,
    alpha_one: Uuid,
    alpha_two: Uuid,
    beta: Uuid,
    desk: Uuid,
    blocked: Uuid,
}

fn board() -> Board {
    let mut domain = DomainState::new();
    let alpha_one = add(
        &mut domain,
        "alpha one",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Started,
    );
    let beta = add(
        &mut domain,
        "beta work",
        project(BETA),
        Some("ship"),
        HumanStatus::Started,
    );
    let desk = add(
        &mut domain,
        "desk chore",
        TaskScope::Global,
        None,
        HumanStatus::Started,
    );
    let alpha_two = add(
        &mut domain,
        "alpha two",
        project(ALPHA),
        None,
        HumanStatus::Started,
    );
    let blocked = add(
        &mut domain,
        "blocked thing",
        project(BETA),
        None,
        HumanStatus::Blocked,
    );
    let mut model = BoardModel::from_domain(&domain, None);
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectNavTab(NavTab::Desk),
        None,
    )
    .expect("desk");
    Board {
        domain,
        model,
        alpha_one,
        alpha_two,
        beta,
        desk,
        blocked,
    }
}

impl Board {
    fn apply(&mut self, intent: BoardIntent) {
        apply_intent(&mut self.domain, &mut self.model, intent, None).expect("apply");
    }

    fn key(&mut self, key: KeyEvent) {
        let intent = map_key(self.model.input_mode(), key).expect("bound key");
        self.apply(intent);
    }

    fn pick(&mut self, open: BoardIntent, label: &str) {
        self.apply(open);
        assert_eq!(self.model.input_mode(), BoardInputMode::ListPicker);
        let index = self
            .model
            .visible_list_picker_options()
            .iter()
            .position(|(_, option)| option.label == label)
            .unwrap_or_else(|| panic!("picker offers {label}"));
        self.apply(BoardIntent::SelectListOption(index));
        assert_eq!(self.model.input_mode(), BoardInputMode::Normal);
    }

    fn picker_labels(&self) -> Vec<String> {
        self.model
            .visible_list_picker_options()
            .into_iter()
            .map(|(_, option)| option.label)
            .collect()
    }

    /// Move the cursor onto `id` with the arrows (a repeated row click would open it).
    fn select(&mut self, id: Uuid) {
        assert!(self.model.visible_ids().contains(&id), "row visible");
        for _ in 0..self.model.visible_ids().len() {
            if self.model.selected_id() == Some(id) {
                return;
            }
            self.apply(BoardIntent::SelectNext);
        }
        assert_eq!(self.model.selected_id(), Some(id));
    }

    fn motion_groups(&self) -> Vec<(String, Vec<Uuid>, Uuid)> {
        self.model
            .queue_view()
            .sections
            .into_iter()
            .find(|section| section.kind == SectionKind::InMotion)
            .expect("in motion")
            .groups
            .into_iter()
            .map(|group| (group.label, group.task_ids, group.header_id))
            .collect()
    }
}

#[test]
fn g_and_s_open_pickers_whose_choices_follow_the_surface() {
    let mut board = board();
    board.key(press(KeyCode::Char('g')));
    assert_eq!(
        board.model.list_picker_kind(),
        Some(ListPickerKind::GroupBy)
    );
    assert_eq!(board.picker_labels(), ["none", "project", "thread"]);
    board.apply(BoardIntent::CancelListPicker);
    board.key(press(KeyCode::Char('s')));
    assert_eq!(board.model.list_picker_kind(), Some(ListPickerKind::SortBy));
    assert_eq!(
        board.picker_labels(),
        ["recent", "project", "thread", "title"]
    );
    board.apply(BoardIntent::CancelListPicker);

    // A project board never offers its own dimension.
    board.model.set_selected_project(Some(PathBuf::from(ALPHA)));
    board.key(press(KeyCode::Char('g')));
    assert_eq!(board.picker_labels(), ["none", "thread"]);
    board.apply(BoardIntent::CancelListPicker);
    board.key(press(KeyCode::Char('s')));
    assert_eq!(board.picker_labels(), ["recent", "thread", "title"]);
}

#[test]
fn grouping_adds_hottest_first_sub_groups_and_the_header_shows_the_layout() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let groups = board.motion_groups();
    let labels: Vec<&str> = groups.iter().map(|group| group.0.as_str()).collect();
    assert_eq!(labels, ["alpha", "desk", "beta"], "newest change leads");
    assert_eq!(groups[0].1, [board.alpha_two, board.alpha_one]);
    assert_eq!(
        board.model.layout().label().as_deref(),
        Some("group: project")
    );

    board.pick(BoardIntent::OpenSortPicker, "title");
    assert_eq!(
        board.motion_groups()[0].1,
        [board.alpha_one, board.alpha_two]
    );
    assert_eq!(
        board.model.layout().label().as_deref(),
        Some("group: project · sort: title")
    );
    assert_eq!(board.model.layout().sort, SortBy::Title);

    // Grouped by project, rows stop repeating the project; the thread stays.
    assert_eq!(board.model.row_label_dimensions(), (true, false));
}

#[test]
fn f_folds_the_cursor_group_and_arrows_only_land_on_folded_headers() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let (_, alpha_rows, alpha_header) = board.motion_groups()[0].clone();
    assert!(
        !board.model.visible_ids().contains(&alpha_header),
        "an unfolded sub-header is skipped by the arrows"
    );

    board.select(board.alpha_one);
    board.key(press(KeyCode::Char('f')));
    let visible = board.model.visible_ids();
    assert!(visible.contains(&alpha_header));
    assert!(alpha_rows.iter().all(|id| !visible.contains(id)));
    assert!(
        board.model.group_header_selected(),
        "the cursor rests on the header"
    );
    assert_eq!(board.model.selected_id(), None, "a header is never a task");

    // A task verb on the header refuses rather than acting on a hidden row.
    board.apply(BoardIntent::Complete);
    assert!(board
        .domain
        .tasks()
        .iter()
        .all(|task| task.status != HumanStatus::Done));

    // Enter on the folded header unfolds it and lands on its first row.
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.selected_id(), Some(alpha_rows[0]));
    assert!(!board.model.visible_ids().contains(&alpha_header));

    // Outside any group, f has nothing to fold.
    board.pick(BoardIntent::OpenGroupPicker, "none");
    board.select(board.blocked);
    board.key(press(KeyCode::Char('f')));
    assert_eq!(board.model.message(), Some("nothing to fold here"));
}

#[test]
fn shift_f_folds_every_sub_group_then_unfolds_them_all() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.select(board.desk);
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    let visible = board.model.visible_ids();
    assert!(
        visible.iter().all(|id| is_group_header_id(*id)),
        "only sub-headers remain: {visible:?}"
    );
    let desk_header = board.motion_groups()[1].2;
    assert_eq!(
        board.model.visible_ids()[board.model.selected_index().expect("selection")],
        desk_header,
        "the cursor follows its row into that row's own header"
    );
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    assert!(board.model.visible_ids().contains(&board.beta));
    assert!(board
        .model
        .visible_ids()
        .iter()
        .all(|id| !is_group_header_id(*id)));
}

#[test]
fn a_click_on_a_sub_header_folds_that_group() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let (_, rows, header) = board.motion_groups()[1].clone();
    let area = Rect::new(0, 0, 80, 24);
    let hits = board_hit_map(area, &board.model);
    let region = hits
        .regions
        .iter()
        .find(|hit| hit.target == QueueHitTarget::GroupHeader(header))
        .expect("the sub-header is clickable");
    let intent = map_board_mouse(
        &board.model,
        &hits,
        left_click(region.area.x + 2, region.area.y),
    )
    .expect("click maps");
    assert_eq!(intent, BoardIntent::ToggleGroupFold(header));
    board.apply(intent);
    assert!(rows
        .iter()
        .all(|id| !board.model.visible_ids().contains(id)));
    assert!(board.model.group_header_selected());
}

#[test]
fn shift_m_on_a_folded_sub_header_marks_the_whole_group_as_one_set() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.select(board.alpha_one);
    board.key(press(KeyCode::Char('f')));
    assert!(board.model.group_header_selected());

    board.key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT));
    assert!(board.model.mark_mode_active());
    let marked: Vec<Uuid> = board.model.marked_ids().iter().copied().collect();
    let mut expected = vec![board.alpha_one, board.alpha_two];
    expected.sort();
    assert_eq!(marked, expected);
    let visible = board.model.visible_ids();
    assert!(
        marked.iter().all(|id| visible.contains(id)),
        "marks only rest on painted rows: the group unfolds"
    );

    // One verb acts on the whole set.
    board.apply(BoardIntent::Complete);
    for id in [board.alpha_one, board.alpha_two] {
        assert_eq!(
            board.domain.get(id).expect("task").status,
            HumanStatus::Done
        );
    }
    assert_eq!(
        board.domain.get(board.beta).expect("task").status,
        HumanStatus::Started
    );
}

#[test]
fn marks_clear_when_the_group_sort_or_filter_changes_or_a_group_folds() {
    let mut board = board();
    let mark = |board: &mut Board| {
        board.apply(BoardIntent::ToggleMarkMode);
        board.select(board.beta);
        board.apply(BoardIntent::MarkToggle);
        assert_eq!(board.model.marked_count(), 1);
    };
    mark(&mut board);
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    assert_eq!(board.model.marked_count(), 0, "group change");
    mark(&mut board);
    board.pick(BoardIntent::OpenSortPicker, "title");
    assert_eq!(board.model.marked_count(), 0, "sort change");
    mark(&mut board);
    board.key(press(KeyCode::Char('f')));
    assert_eq!(board.model.marked_count(), 0, "fold");
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    mark(&mut board);
    board.apply(BoardIntent::OpenThreadFilterPicker);
    board.apply(BoardIntent::SelectListOption(1));
    assert_eq!(board.model.marked_count(), 0, "filter change");
}

#[test]
fn t_on_the_desk_opens_a_tabbed_filter_whose_choices_combine() {
    let mut board = board();
    board.key(press(KeyCode::Char('t')));
    assert_eq!(
        board.model.list_picker_kind(),
        Some(ListPickerKind::DeskFilter)
    );
    assert_eq!(board.model.list_picker_tab(), Some(FilterTab::Threads));
    assert_eq!(
        board.picker_labels(),
        ["All threads", "#ship", "Without a thread"]
    );
    board.key(press(KeyCode::Right));
    assert_eq!(board.model.list_picker_tab(), Some(FilterTab::Projects));
    assert_eq!(
        board.picker_labels(),
        ["All projects", "desk", "alpha", "beta"]
    );
    let alpha = board
        .picker_labels()
        .iter()
        .position(|label| label == "alpha")
        .expect("alpha");
    board.apply(BoardIntent::SelectListOption(alpha));
    assert_eq!(board.model.desk_filter().label(), "alpha");
    let listed: Vec<Uuid> = board.model.visible_ids();
    assert_eq!(listed, [board.alpha_two, board.alpha_one]);

    board.pick(BoardIntent::OpenThreadFilterPicker, "#ship");
    assert_eq!(board.model.desk_filter().label(), "alpha #ship");
    assert_eq!(board.model.visible_ids(), [board.alpha_one]);
    assert_eq!(
        board.model.row_label_dimensions(),
        (false, false),
        "both filtered dimensions leave the row labels"
    );

    // Search narrows inside the filter and the grouping.
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    board.pick(BoardIntent::OpenThreadFilterPicker, "All threads");
    board.apply(BoardIntent::FocusSearch);
    for character in "two".chars() {
        board.apply(BoardIntent::SearchQueryInsert(character));
    }
    assert_eq!(board.model.visible_ids(), [board.alpha_two]);
    let _ = board.desk;
}

#[test]
fn group_fold_and_layout_state_is_per_surface() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.model.set_selected_project(Some(PathBuf::from(ALPHA)));
    assert_eq!(
        board.model.layout().group,
        GroupBy::None,
        "project boards start ungrouped"
    );
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    board.apply(BoardIntent::SelectNavTab(NavTab::Desk));
    assert_eq!(
        board.model.layout().group,
        GroupBy::Project,
        "the desk kept its own"
    );
}
