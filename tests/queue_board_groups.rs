//! Group by, sort by, fold, and the desk filter (#175, #158): keys, pickers, folding,
//! group marks, and the selection / mark invariants they must keep.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{
    apply_intent, board_hit_map, draw_board, BoardInputMode, BoardModel, FilterTab, ListPickerKind,
    ProjectsView,
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

    fn rows(&self, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                let _ = draw_board(frame, &self.model);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    fn click(&mut self, target: QueueHitTarget) {
        let hits = board_hit_map(Rect::new(0, 0, 80, 24), &self.model);
        let region = hits
            .regions
            .iter()
            .find(|hit| hit.target == target)
            .unwrap_or_else(|| panic!("{target:?} is painted and clickable"));
        let intent = map_board_mouse(
            &self.model,
            &hits,
            left_click(region.area.x + region.area.width / 2, region.area.y),
        )
        .expect("click maps");
        self.apply(intent);
    }

    /// Move the cursor onto any visible row, a header included, with the arrows.
    fn select_row(&mut self, id: Uuid) {
        assert!(self.model.visible_ids().contains(&id), "row visible");
        let at = |model: &BoardModel| model.selected_index().map(|i| model.visible_ids()[i]);
        for _ in 0..self.model.visible_ids().len() {
            if at(&self.model) == Some(id) {
                return;
            }
            self.apply(BoardIntent::SelectNext);
        }
        for _ in 0..self.model.visible_ids().len() {
            if at(&self.model) == Some(id) {
                return;
            }
            self.apply(BoardIntent::SelectPrev);
        }
        assert_eq!(at(&self.model), Some(id));
    }

    /// The visible task rows, sub-headers left out.
    fn visible_tasks(&self) -> Vec<Uuid> {
        self.model
            .visible_ids()
            .into_iter()
            .filter(|id| !is_group_header_id(*id))
            .collect()
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

    // Grouped by project, grouped rows stop repeating the project; the thread stays.
    let rows = board.rows(80, 24);
    let row = rows
        .iter()
        .find(|row| row.contains("alpha one"))
        .expect("row");
    assert!(row.trim_end().ends_with("#ship"), "{row}");
}

#[test]
fn f_folds_the_cursor_group_and_the_arrows_stop_on_every_sub_header() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let (_, alpha_rows, alpha_header) = board.motion_groups()[0].clone();
    let visible = board.model.visible_ids();
    let at = visible
        .iter()
        .position(|id| *id == alpha_header)
        .expect("an unfolded sub-header is a stop for the arrows");
    assert_eq!(visible[at + 1], alpha_rows[0], "its rows follow it");

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

    // f again unfolds it and lands on its first row; the header stays a stop.
    board.key(press(KeyCode::Char('f')));
    assert_eq!(board.model.selected_id(), Some(alpha_rows[0]));
    assert!(board.model.visible_ids().contains(&alpha_header));

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
    let visible = board.visible_tasks();
    for id in [board.alpha_one, board.alpha_two, board.beta, board.desk] {
        assert!(visible.contains(&id), "every row is back");
    }
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
    assert_eq!(board.visible_tasks(), [board.alpha_two]);
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

#[test]
fn layout_commands_stay_off_an_open_task_page() {
    let mut board = board();
    board.select(board.beta);
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    board.apply(BoardIntent::OpenCommandPalette);
    let labels: Vec<&str> = board
        .model
        .visible_commands()
        .iter()
        .map(|command| command.label)
        .collect();
    for absent in ["group by…", "sort by…", "fold / unfold group"] {
        assert!(
            !labels.contains(&absent),
            "{absent} offered over a page: {labels:?}"
        );
    }
    board.apply(BoardIntent::CloseCommandSurface);
    board.apply(BoardIntent::OpenGroupPicker);
    assert_eq!(board.model.input_mode(), BoardInputMode::TaskPage);
    assert_eq!(board.model.list_picker_kind(), None);
}

#[test]
fn shift_m_on_a_sub_header_marks_its_group_even_inside_multi_select() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.select(board.alpha_one);
    board.key(press(KeyCode::Char('f')));
    let alpha_header = board.motion_groups()[0].2;
    // Multi-select on: mark beta's row, then arrow back to alpha's folded header.
    board.select(board.beta);
    board.apply(BoardIntent::ToggleMarkMode);
    board.apply(BoardIntent::MarkToggle);
    assert!(board.model.visible_ids().contains(&alpha_header));
    while board.model.visible_ids()[board.model.selected_index().expect("cursor")] != alpha_header {
        board.apply(BoardIntent::SelectPrev);
    }
    assert!(board.model.mark_mode_active());
    let shift_m = KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT);
    let intent = map_key(board.model.input_mode(), shift_m).expect("bound");
    board.apply(intent);
    assert!(board.model.mark_mode_active(), "multi-select stays on");
    let mut expected = vec![board.alpha_one, board.alpha_two, board.beta];
    expected.sort();
    let marked: Vec<Uuid> = board.model.marked_ids().iter().copied().collect();
    assert_eq!(marked, expected, "the group joins the existing set");
    assert!(
        board.model.visible_ids().contains(&board.alpha_one),
        "and unfolds"
    );
}

#[test]
fn ungrouped_sections_keep_the_grouped_dimension_on_their_labels() {
    let mut board = board();
    let inbox = add(
        &mut board.domain,
        "alpha inbox item",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Open,
    );
    let done = add(
        &mut board.domain,
        "alpha done item",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Done,
    );
    let _ = (inbox, done);
    board.model.sync_from_domain(&board.domain);
    board.model.set_selected_project(Some(PathBuf::from(ALPHA)));
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    board.apply(BoardIntent::ToggleDoneDrawer);
    let rows = board.rows(80, 40);
    let done = rows
        .iter()
        .find(|row| row.contains("alpha done item"))
        .expect("drawer row");
    assert!(done.trim_end().ends_with("#ship"), "{done}");
    for title in ["alpha one", "alpha inbox item"] {
        let grouped = rows.iter().find(|row| row.contains(title)).expect(title);
        assert!(
            !grouped.contains("#ship"),
            "a grouped row drops it: {grouped}"
        );
    }
}

/// An open project task under a thread, on the alpha board grouped by thread.
fn grouped_inbox() -> (Board, Uuid, Uuid, Uuid) {
    let mut board = board();
    let ship_one = add(
        &mut board.domain,
        "inbox ship one",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Open,
    );
    let ship_two = add(
        &mut board.domain,
        "inbox ship two",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Open,
    );
    let loose = add(
        &mut board.domain,
        "inbox loose",
        project(ALPHA),
        None,
        HumanStatus::Open,
    );
    board.model.sync_from_domain(&board.domain);
    board.model.set_selected_project(Some(PathBuf::from(ALPHA)));
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    (board, ship_one, ship_two, loose)
}

fn inbox_groups(board: &Board) -> Vec<(String, Vec<Uuid>, Uuid)> {
    board
        .model
        .queue_view()
        .sections
        .into_iter()
        .find(|section| section.kind == SectionKind::Inbox)
        .expect("inbox")
        .groups
        .into_iter()
        .map(|group| (group.label, group.task_ids, group.header_id))
        .collect()
}

#[test]
fn the_inbox_groups_under_its_own_fold_and_paints_sub_headers() {
    let (board, ship_one, ship_two, loose) = grouped_inbox();
    let groups = inbox_groups(&board);
    let labels: Vec<&str> = groups.iter().map(|group| group.0.as_str()).collect();
    assert_eq!(labels, ["no thread", "#ship"], "hottest first");
    assert_eq!(groups[0].1, [loose]);
    let mut ship = groups[1].1.clone();
    ship.sort();
    let mut expected = vec![ship_one, ship_two];
    expected.sort();
    assert_eq!(ship, expected);

    let rows = board.rows(80, 40);
    let inbox = rows
        .iter()
        .position(|row| row.contains("inbox ·"))
        .expect("inbox header");
    let ship_header = rows
        .iter()
        .rposition(|row| row.contains("#ship ·"))
        .expect("an inbox sub-header");
    assert!(ship_header > inbox, "{}", rows.join("\n"));
}

#[test]
fn f_shift_f_and_shift_m_work_on_inbox_sub_headers() {
    let (mut board, ship_one, ship_two, loose) = grouped_inbox();
    let ship_header = inbox_groups(&board)[1].2;

    // f inside an inbox sub-group folds that sub-group, not the whole inbox.
    board.select(ship_one);
    board.key(press(KeyCode::Char('f')));
    let visible = board.model.visible_ids();
    assert!(visible.contains(&ship_header));
    assert!(!visible.contains(&ship_one) && !visible.contains(&ship_two));
    assert!(visible.contains(&loose), "the inbox itself stays open");

    // Shift+M on the folded inbox sub-header marks the whole group.
    board.key(KeyEvent::new(KeyCode::Char('M'), KeyModifiers::SHIFT));
    let mut marked: Vec<Uuid> = board.model.marked_ids().iter().copied().collect();
    marked.sort();
    let mut expected = vec![ship_one, ship_two];
    expected.sort();
    assert_eq!(marked, expected);
    board.apply(BoardIntent::ToggleMarkMode);

    // Shift+F folds every sub-group and the inbox; a hidden row lands on the inbox header.
    board.select(loose);
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    let visible = board.model.visible_ids();
    assert!(visible.contains(&tsk_tui::ui::queue::INBOX_HEADER_ROW_ID));
    assert!(!visible.contains(&ship_header), "under the folded inbox");
    let index = board.model.selected_index().expect("selection");
    assert_eq!(visible[index], tsk_tui::ui::queue::INBOX_HEADER_ROW_ID);

    // Shift+F again unfolds everything, inbox sub-groups included.
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    let visible = board.model.visible_ids();
    assert!([ship_one, ship_two, loose]
        .iter()
        .all(|id| visible.contains(id)));
}

#[test]
fn archived_rows_carry_the_same_label_as_drawer_rows() {
    let mut board = board();
    let filed = add(
        &mut board.domain,
        "filed",
        project(ALPHA),
        Some("ship"),
        HumanStatus::Done,
    );
    board.domain.archive_task(filed).expect("archive");
    board.model.sync_from_domain(&board.domain);
    board.apply(BoardIntent::ToggleDoneDrawer);
    board.apply(BoardIntent::ToggleArchivedGroup);
    let rows = board.rows(80, 40);
    let row = rows
        .iter()
        .find(|row| row.contains("filed"))
        .expect("archived row");
    assert!(row.trim_end().ends_with("#ship · alpha"), "{row}");
}

#[test]
fn project_groups_merge_spellings_of_the_same_repository() {
    let mut board = board();
    let slashed = add(
        &mut board.domain,
        "alpha via slash",
        project("/repos/alpha/"),
        None,
        HumanStatus::Started,
    );
    board.model.sync_from_domain(&board.domain);
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let alpha: Vec<_> = board
        .motion_groups()
        .into_iter()
        .filter(|group| group.0 == "alpha")
        .collect();
    assert_eq!(alpha.len(), 1, "one project, one group");
    assert!(alpha[0].1.contains(&slashed) && alpha[0].1.contains(&board.alpha_one));
}

#[test]
fn clicking_the_layout_control_and_the_desk_filter_tabs() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.click(QueueHitTarget::NavGroup);
    assert_eq!(
        board.model.list_picker_kind(),
        Some(ListPickerKind::GroupBy)
    );
    board.apply(BoardIntent::CancelListPicker);

    board.click(QueueHitTarget::NavChip);
    assert_eq!(
        board.model.list_picker_kind(),
        Some(ListPickerKind::DeskFilter)
    );
    board.click(QueueHitTarget::PickerTab(
        tsk_tui::ui::board::PickerTab::Archived,
    ));
    assert_eq!(board.model.list_picker_tab(), Some(FilterTab::Projects));
    let beta = board
        .picker_labels()
        .iter()
        .position(|label| label == "beta")
        .expect("beta");
    board.click(QueueHitTarget::ListPickerOption(beta));
    assert_eq!(board.model.desk_filter().label(), "beta");
    board.click(QueueHitTarget::NavChip);
    board.click(QueueHitTarget::PickerTab(
        tsk_tui::ui::board::PickerTab::Main,
    ));
    assert_eq!(board.model.list_picker_tab(), Some(FilterTab::Threads));
    board.click(QueueHitTarget::ListPickerOption(1));
    assert_eq!(board.model.desk_filter().label(), "beta #ship");
    assert_eq!(board.visible_tasks(), [board.beta]);

    // The header paints the active filter like a project board's control.
    let rows = board.rows(80, 24);
    assert!(
        rows.iter().take(4).any(|row| row.contains("beta #ship ▾")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn a_board_whose_every_row_is_folded_still_seeds_a_header() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT));
    board.apply(BoardIntent::SelectNavTab(NavTab::Projects));
    board.apply(BoardIntent::SelectNavTab(NavTab::Desk));
    let index = board
        .model
        .selected_index()
        .expect("the cursor rests on a folded header, not on nothing");
    assert!(is_group_header_id(board.model.visible_ids()[index]));
}

#[test]
fn merged_tasks_never_move_the_tab_or_pin_a_hidden_row_under_groups() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.select(board.beta);
    // A task from disk that becomes the hottest group reorders groups, not the cursor.
    add(
        &mut board.domain,
        "gamma from disk",
        project("/repos/gamma"),
        None,
        HumanStatus::Started,
    );
    board.model.sync_from_domain(&board.domain);
    assert_eq!(board.model.nav_tab(), NavTab::Desk);
    assert_eq!(board.model.selected_id(), Some(board.beta));
    assert_eq!(board.motion_groups()[0].0, "gamma");

    // A merged change that moves the selected row into a folded group re-pins the cursor
    // on a painted row.
    board.select(board.alpha_one);
    board.key(press(KeyCode::Char('f')));
    board.select(board.desk);
    board
        .domain
        .edit(board.desk, "desk chore", None, project(ALPHA), None)
        .expect("merged edit");
    board.model.sync_from_domain(&board.domain);
    let index = board.model.selected_index().expect("a selection survives");
    let id = board.model.visible_ids()[index];
    assert_ne!(id, board.desk, "the hidden row is not pinned");
    assert_eq!(board.model.nav_tab(), NavTab::Desk);
}

#[test]
fn every_task_list_header_shows_group_and_sort_controls_that_open_their_pickers() {
    let mut board = board();
    // The desk, a project board, and the cross-project thread View.
    for surface in ["desk", "project", "thread view"] {
        match surface {
            "project" => board.model.set_selected_project(Some(PathBuf::from(ALPHA))),
            "thread view" => {
                board.apply(BoardIntent::SelectNavTab(NavTab::Projects));
                board.pick(BoardIntent::OpenProjectsViewPicker, "#ship");
            }
            _ => {}
        }
        let rows = board.rows(110, 24);
        let header = rows.iter().take(4).cloned().collect::<Vec<_>>().join("\n");
        assert!(
            header.contains("group: none ▾") && header.contains("sort: recent ▾"),
            "{surface}:\n{header}"
        );
        board.click(QueueHitTarget::NavGroup);
        assert_eq!(
            board.model.list_picker_kind(),
            Some(ListPickerKind::GroupBy),
            "{surface}"
        );
        board.apply(BoardIntent::CancelListPicker);
        board.click(QueueHitTarget::NavSort);
        assert_eq!(
            board.model.list_picker_kind(),
            Some(ListPickerKind::SortBy),
            "{surface}"
        );
        board.apply(BoardIntent::CancelListPicker);
    }
}

#[test]
fn narrow_headers_shorten_the_controls_and_drop_only_idle_ones() {
    let mut board = board();
    // 76 columns: the pair shortens to its keys beside the tabs.
    let rows = board.rows(76, 24);
    assert!(
        rows[1].contains("g:none ▾") && rows[1].contains("s:recent ▾"),
        "{}",
        rows.join("\n")
    );

    // 40 columns: idle controls never cost a row; the project board keeps its filter
    // beside the tabs exactly as before.
    board.model.set_selected_project(Some(PathBuf::from(ALPHA)));
    let rows = board.rows(40, 10);
    let all = rows.join("\n");
    assert!(!all.contains("g:") && !all.contains("group:"), "{all}");
    assert!(rows[1].contains("all ▾"), "{all}");

    // A layout that says something takes its own row under the tabs.
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    let rows = board.rows(40, 10);
    assert!(
        rows[3].contains("g:thread ▾") || rows[3].contains("group: thread ▾"),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn defaults_drop_before_a_chosen_group_when_a_long_filter_crowds_the_header() {
    let mut board = board();
    add(
        &mut board.domain,
        "candidate work",
        project(ALPHA),
        Some("release-candidate-x"),
        HumanStatus::Started,
    );
    board.model.sync_from_domain(&board.domain);
    board.pick(BoardIntent::OpenGroupPicker, "project");
    board.apply(BoardIntent::OpenThreadFilterPicker);
    let index = board
        .picker_labels()
        .iter()
        .position(|label| label.starts_with("#release-candidate-x"))
        .expect("the thread is offered");
    board.apply(BoardIntent::SelectListOption(index));
    assert_eq!(board.model.desk_filter().label(), "#release-candidate-x");

    // 45 columns: the controls take their own row; the default sort drops and the
    // chosen group keeps its full words beside the filter.
    let rows = board.rows(45, 24);
    let controls = &rows[3];
    assert!(
        controls.contains("group: project ▾") && controls.contains("#release-candidate-x ▾"),
        "{}",
        rows.join("\n")
    );
    assert!(!controls.contains("sort"), "{controls}");

    // 40 columns: the chosen group shortens to its key rather than vanishing while the
    // default sort stays.
    let rows = board.rows(40, 10);
    let controls = &rows[3];
    assert!(
        controls.contains("g:project ▾") && controls.contains("#release-candidate-x ▾"),
        "{}",
        rows.join("\n")
    );
    assert!(!controls.contains("s:"), "{controls}");
}

#[test]
fn enter_on_a_project_sub_header_opens_that_projects_board() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let (label, _, alpha_header) = board.motion_groups()[0].clone();
    assert_eq!(label, "alpha");
    board.select_row(alpha_header);
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.nav_tab(), NavTab::ProjectBoard);
    assert_eq!(
        board.model.active_project(),
        Some(std::path::Path::new(ALPHA))
    );
    assert_eq!(board.model.input_mode(), BoardInputMode::Normal);

    // The desk's own sub-header opens the desk: here, it stays put.
    board.apply(BoardIntent::SelectNavTab(NavTab::Desk));
    let desk_header = board.motion_groups()[1].2;
    board.select_row(desk_header);
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.nav_tab(), NavTab::Desk);
    assert!(
        !board.model.layout().folded.contains(&desk_header),
        "Enter never folds a sub-header"
    );
}

#[test]
fn enter_on_a_thread_sub_header_opens_its_thread_view_and_no_thread_opens_nothing() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    let groups = board.motion_groups();
    let ship = groups
        .iter()
        .find(|group| group.0 == "#ship")
        .expect("#ship")
        .2;
    let loose = groups
        .iter()
        .find(|group| group.0 == "no thread")
        .expect("no thread")
        .2;
    board.select_row(loose);
    board.key(press(KeyCode::Enter));
    assert_eq!(
        board.model.nav_tab(),
        NavTab::Desk,
        "no thread opens nothing"
    );
    assert!(!board.model.layout().folded.contains(&loose));

    board.select_row(ship);
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.nav_tab(), NavTab::Projects);
    assert_eq!(
        board.model.projects_view(),
        &ProjectsView::Thread("ship".into())
    );
    let visible = board.visible_tasks();
    assert!(visible.contains(&board.alpha_one) && visible.contains(&board.beta));
}

#[test]
fn enter_on_the_inbox_header_does_nothing() {
    let (mut board, ship_one, _, _) = grouped_inbox();
    board.select_row(tsk_tui::ui::queue::INBOX_HEADER_ROW_ID);
    board.key(press(KeyCode::Enter));
    assert!(board.model.inbox_header_selected());
    assert!(board.model.visible_ids().contains(&ship_one), "no fold");
    assert_eq!(board.model.input_mode(), BoardInputMode::Normal);
}

#[test]
fn arrows_fold_and_unfold_sub_headers_and_the_inbox_in_place() {
    let (mut board, ship_one, _, _) = grouped_inbox();
    let ship = inbox_groups(&board)
        .into_iter()
        .find(|group| group.0 == "#ship")
        .expect("#ship")
        .2;
    board.select_row(ship);
    board.apply(BoardIntent::FoldHeader);
    assert!(!board.model.visible_ids().contains(&ship_one));
    assert!(
        board.model.group_header_selected(),
        "← leaves the cursor on it"
    );
    board.apply(BoardIntent::UnfoldHeader);
    assert!(board.model.visible_ids().contains(&ship_one));
    assert!(
        board.model.group_header_selected(),
        "→ leaves the cursor on it"
    );

    board.select_row(tsk_tui::ui::queue::INBOX_HEADER_ROW_ID);
    board.apply(BoardIntent::FoldHeader);
    assert!(!board.model.visible_ids().contains(&ship));
    board.apply(BoardIntent::FoldHeader);
    assert!(
        board.model.inbox_header_selected(),
        "a second ← changes nothing"
    );
    board.apply(BoardIntent::UnfoldHeader);
    assert!(board.model.visible_ids().contains(&ship));
}

#[test]
fn a_double_click_on_a_sub_header_opens_its_board_and_leaves_the_fold_alone() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let alpha_header = board.motion_groups()[0].2;
    board.click(QueueHitTarget::GroupHeader(alpha_header));
    assert!(
        board.model.layout().folded.contains(&alpha_header),
        "a single click folds"
    );
    board.click(QueueHitTarget::GroupHeader(alpha_header));
    assert_eq!(board.model.nav_tab(), NavTab::ProjectBoard);
    assert_eq!(
        board.model.active_project(),
        Some(std::path::Path::new(ALPHA))
    );
    board.apply(BoardIntent::SelectNavTab(NavTab::Desk));
    assert!(
        !board.model.layout().folded.contains(&alpha_header),
        "the double-click put the fold back before leaving"
    );
}

#[test]
fn a_sub_header_verb_bar_offers_open_and_its_fold_arrow() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "thread");
    let groups = board.motion_groups();
    let ship = groups.iter().find(|g| g.0 == "#ship").expect("#ship").2;
    let loose = groups
        .iter()
        .find(|g| g.0 == "no thread")
        .expect("no thread")
        .2;
    let verbs = |board: &Board| {
        tsk_tui::ui::board::board_verb_items(&board.model)
            .iter()
            .map(|entry| (entry.key, entry.label))
            .collect::<Vec<_>>()
    };
    board.select_row(ship);
    assert_eq!(
        verbs(&board),
        [
            ("enter", "open"),
            ("←", "fold"),
            ("+", "add"),
            ("?", "help")
        ]
    );
    // The bar's arrow seat folds on a click, like the key.
    let index = 1;
    board.click(QueueHitTarget::Verb(index));
    assert!(board.model.layout().folded.contains(&ship));
    assert_eq!(verbs(&board)[1], ("→", "unfold"));
    // `no thread` opens nothing, so its bar has no Enter.
    board.select_row(loose);
    assert_eq!(verbs(&board)[0], ("←", "fold"));
}

#[test]
fn enter_on_the_desk_sub_header_on_the_desk_keeps_search_and_marks() {
    let mut board = board();
    board.pick(BoardIntent::OpenGroupPicker, "project");
    let desk_header = board
        .motion_groups()
        .into_iter()
        .find(|group| group.0 == "desk")
        .expect("desk group")
        .2;
    board.select_row(desk_header);
    let bar: Vec<(&str, &str)> = tsk_tui::ui::board::board_verb_items(&board.model)
        .iter()
        .map(|entry| (entry.key, entry.label))
        .collect();
    assert_eq!(
        bar,
        [("←", "fold"), ("+", "add"), ("?", "help")],
        "the bar never offers an Enter that does nothing"
    );
    board.apply(BoardIntent::FocusSearch);
    for character in "o".chars() {
        board.apply(BoardIntent::SearchQueryInsert(character));
    }
    board.apply(BoardIntent::PinSearch);
    board.select(board.desk);
    board.apply(BoardIntent::ToggleMarkMode);
    board.apply(BoardIntent::MarkToggle);
    assert_eq!(board.model.marked_ids().len(), 1);
    board.select_row(desk_header);
    board.key(press(KeyCode::Enter));
    assert_eq!(board.model.nav_tab(), NavTab::Desk);
    assert_eq!(board.model.search_query(), "o", "the pinned search stays");
    assert_eq!(board.model.marked_ids().len(), 1, "the marks stay");
}
