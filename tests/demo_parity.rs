//! App-side reference states for site/parity. These are TestBackend frames, not terminal screenshots.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tsk_tui::{
    domain::DomainState,
    ui::{
        apply_intent, draw_board,
        input::{map_key, route_responsive_key, ResponsiveKeyRoute},
        queue::NavTab,
        BoardIntent, BoardModel,
    },
};

fn key(
    state: &mut DomainState,
    model: &mut BoardModel,
    code: KeyCode,
    mods: KeyModifiers,
    width: u16,
) {
    let event = KeyEvent::new(code, mods);
    let intent = match route_responsive_key(
        model.input_mode(),
        model.wide_stage(),
        tsk_tui::ui::tier::resolve_responsive(width, 24, model.wide_stage()).presentation,
        event,
    ) {
        ResponsiveKeyRoute::Intent(intent) => Some(intent),
        ResponsiveKeyRoute::Inert => None,
        ResponsiveKeyRoute::Surface => map_key(model.input_mode(), event),
    };
    if let Some(intent) = intent {
        apply_intent(state, model, intent, None).unwrap();
    }
}
fn capture(model: &BoardModel, width: u16, _name: &str) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
    terminal
        .draw(|f| {
            draw_board(f, model);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let text = (0..24)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    text
}
fn export_reference(text: &str, width: u16, name: &str) {
    if let Ok(dir) = std::env::var("TSK_PARITY_OUTPUT") {
        fs::create_dir_all(&dir).unwrap();
        let dir = PathBuf::from(dir);
        fs::write(dir.join(format!("app-{width}-{name}.txt")), text).unwrap();
        if name == "page" {
            // Step content and frame-edge scrollbar chrome are separate contracts.
            // Export both, rather than treating a painted thumb as part of a step.
            let scrollbar_rows: Vec<_> = text
                .lines()
                .enumerate()
                .filter_map(|(row, line)| line.ends_with('▌').then_some(row))
                .collect();
            fs::write(
                dir.join(format!("page-scrollbar-{width}.json")),
                serde_json::to_string_pretty(&scrollbar_rows).unwrap(),
            )
            .unwrap();
            let lines: Vec<_> = text
                .lines()
                .skip_while(|line| !line.contains("✓ Check"))
                .take_while(|line| !line.contains("+ step"))
                .map(|line| {
                    line.chars()
                        .take(width.saturating_sub(1) as usize)
                        .skip(4)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect();
            fs::write(
                dir.join(format!("steps-{width}.json")),
                serde_json::to_string_pretty(&lines).unwrap(),
            )
            .unwrap();
        }
        if name == "project-peek-or-split" && width < 110 {
            write_json(
                &dir,
                &format!("peek-{width}.json"),
                &under_row(text, "T12 ", 0),
            );
        }
        if name.starts_with("preview-") {
            // The project preview is the right column: drop the index column and its rule.
            let column = text
                .lines()
                .find(|line| line.contains("T12 "))
                .and_then(|line| line.chars().position(|c| c == '│'))
                .expect("preview divider")
                + 1;
            write_json(
                &dir,
                &format!("{name}-{width}.json"),
                &under_row(text, "T12 ", column),
            );
        }
        if name == "initial" {
            // The blocked row's live line, at the fixture's pinned clock.
            write_json(
                &dir,
                &format!("live-{width}.json"),
                &under_row(text, "T12 ", 0),
            );
            let lines: Vec<_> = text
                .lines()
                .skip_while(|line| !line.contains("T13 "))
                .take_while(|line| {
                    let body = line.trim_start();
                    !body.is_empty()
                        && (line.contains("T13")
                            || (!body.starts_with('○') && !body.starts_with('▸')))
                })
                .map(|line| {
                    line.chars()
                        .skip(8)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect();
            fs::write(
                dir.join(format!("title-{width}.json")),
                serde_json::to_string_pretty(&lines).unwrap(),
            )
            .unwrap();
        }
    }
}

/// The fixture's clock, matching the browser harness's `now`, so painted ages agree.
fn fixture_clock() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_200)
}

fn fixture_board() -> (DomainState, BoardModel) {
    let state: DomainState =
        serde_json::from_str(include_str!("fixtures/demo-parity/store.json")).unwrap();
    let mut model = BoardModel::from_domain(&state, Some(PathBuf::from("/tmp/tsk-parity")));
    model.set_clock(Some(fixture_clock()));
    (state, model)
}

/// The lines painted under the first row containing `row`, from column `skip` on: its live
/// line or its open peek, up to the peek's `└` footer or the first line of anything else.
fn under_row(text: &str, row: &str, skip: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines().skip_while(|line| !line.contains(row)).skip(1) {
        let line: String = line.chars().skip(skip).collect();
        let line = line.trim_end().trim_end_matches(['▌', '█']).trim_end();
        let line = if skip > 0 {
            line.strip_prefix(' ').unwrap_or(line)
        } else {
            line
        };
        if line.starts_with("    │ ") || line.starts_with("    └") {
            out.push(line.to_string());
            // A peek ends at its `└` footer; a live line goes on to its continuations.
            if line.starts_with("    └") && out.iter().any(|line| line.starts_with("    │ ")) {
                break;
            }
        } else if line.starts_with("       ") && !line.trim().is_empty() && !out.is_empty() {
            out.push(line.to_string());
        } else {
            break;
        }
    }
    out
}

fn write_json(dir: &Path, name: &str, lines: &[String]) {
    fs::write(dir.join(name), serde_json::to_string_pretty(lines).unwrap()).unwrap();
}

#[test]
fn shared_fixture_first_flow_and_peek_references() {
    fixture_flow(false);
}

#[test]
#[ignore = "regenerates browser reference artifacts"]
fn regenerate_parity_references() {
    fixture_flow(true);
}

fn fixture_flow(export: bool) {
    let capture = |model: &BoardModel, width: u16, name: &str| {
        let text = capture(model, width, name);
        if export {
            export_reference(&text, width, name);
        }
        text
    };
    for width in [40, 78, 109, 110] {
        let (mut state, mut model) = fixture_board();
        apply_intent(
            &mut state,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Desk),
            None,
        )
        .unwrap();
        assert!(!capture(&model, width, "initial").contains("IN MOTION"));
        key(
            &mut state,
            &mut model,
            KeyCode::Right,
            KeyModifiers::NONE,
            width,
        );
        let peek = capture(&model, width, "project-peek-or-split");
        if width < 110 {
            // A recorded block's peek: live line, why, Decide; no notes, no collapsed line.
            assert!(peek.contains("└─ #release · tsk-parity"), "{peek}");
            assert!(peek.contains("│ @claude blocked on you · 1m"), "{peek}");
            assert!(peek.contains("│ Decide: Keep it · Rewrite"), "{peek}");
            assert!(!peek.contains("Confirm the wording"), "{peek}");
            assert!(!peek.contains("└─ @claude"), "{peek}");
        }
        key(
            &mut state,
            &mut model,
            KeyCode::Left,
            KeyModifiers::NONE,
            width,
        );
        let closed = capture(&model, width, "peek-closed");
        assert!(
            closed.contains("└─ @claude blocked on you · 1m"),
            "{closed}"
        );
        key(
            &mut state,
            &mut model,
            KeyCode::Down,
            KeyModifiers::NONE,
            width,
        );
        key(
            &mut state,
            &mut model,
            KeyCode::Char('s'),
            KeyModifiers::CONTROL,
            width,
        );
        assert!(capture(&model, width, "started").contains("IN MOTION"));
        key(
            &mut state,
            &mut model,
            KeyCode::Enter,
            KeyModifiers::NONE,
            width,
        );
        let page = capture(&model, width, "page");
        assert!(page.contains("plain note"));
        assert!(page.contains("steps 1/2"));
        apply_intent(&mut state, &mut model, BoardIntent::FormFocusNext, None).unwrap();
        // The app keyboard boundary resolves Enter on a stored step to ToggleStep.
        apply_intent(&mut state, &mut model, BoardIntent::ToggleStep, None).unwrap();
        assert!(capture(&model, width, "step-toggled").contains("steps 2/2"));
        let task = state
            .tasks()
            .iter()
            .find(|task| task.number == Some(13))
            .unwrap();
        assert_eq!(task.status, tsk_tui::domain::HumanStatus::Started);
        apply_intent(&mut state, &mut model, BoardIntent::BeginAddStep, None).unwrap();
        for ch in "New step".chars() {
            apply_intent(&mut state, &mut model, BoardIntent::EditInsert(ch), None).unwrap();
        }
        key(
            &mut state,
            &mut model,
            KeyCode::Enter,
            KeyModifiers::NONE,
            width,
        );
        assert_eq!(
            state
                .tasks()
                .iter()
                .find(|task| task.number == Some(13))
                .unwrap()
                .steps
                .len(),
            3
        );
        capture(&model, width, "step-added");
        key(
            &mut state,
            &mut model,
            KeyCode::Esc,
            KeyModifiers::NONE,
            width,
        );
        key(
            &mut state,
            &mut model,
            KeyCode::Esc,
            KeyModifiers::NONE,
            width,
        );
        assert!(capture(&model, width, "back").contains("IN MOTION"));
        if width < 110 {
            apply_intent(
                &mut state,
                &mut model,
                BoardIntent::SelectNavTab(NavTab::ProjectBoard),
                None,
            )
            .unwrap();
            key(
                &mut state,
                &mut model,
                KeyCode::Down,
                KeyModifiers::NONE,
                width,
            );
            key(
                &mut state,
                &mut model,
                KeyCode::Right,
                KeyModifiers::NONE,
                width,
            );
            assert!(capture(&model, width, "thread-peek").contains("└─ #release"));
            key(
                &mut state,
                &mut model,
                KeyCode::Down,
                KeyModifiers::NONE,
                width,
            );
            key(
                &mut state,
                &mut model,
                KeyCode::Right,
                KeyModifiers::NONE,
                width,
            );
            assert!(capture(&model, width, "unlabeled-peek").contains("└─ tsk-parity"));
        }
    }
    // The project preview paints the same live line and block peek as the board.
    for width in [110, 130] {
        let (mut state, mut model) = fixture_board();
        apply_intent(
            &mut state,
            &mut model,
            BoardIntent::SelectNavTab(NavTab::Projects),
            None,
        )
        .unwrap();
        // Each capture paints the frame, which records the wide presentation keys route by.
        capture(&model, width, "projects");
        for code in [KeyCode::Down, KeyCode::Right, KeyCode::Right] {
            key(&mut state, &mut model, code, KeyModifiers::NONE, width);
            capture(&model, width, "projects");
        }
        let live = capture(&model, width, "preview-live");
        assert!(live.contains("└─ @claude blocked on you · 1m"), "{live}");
        apply_intent(
            &mut state,
            model.input_target_mut(),
            BoardIntent::PeekDetail,
            None,
        )
        .unwrap();
        let peek = capture(&model, width, "preview-peek");
        assert!(peek.contains("│ Decide: Keep it · Rewrite"), "{peek}");
        assert!(!peek.contains("Confirm the wording"), "{peek}");
        assert!(!peek.contains("└─ @claude"), "{peek}");
        apply_intent(
            &mut state,
            model.input_target_mut(),
            BoardIntent::CollapseDetail,
            None,
        )
        .unwrap();
        let closed = capture(&model, width, "preview-closed");
        assert!(
            closed.contains("└─ @claude blocked on you · 1m"),
            "{closed}"
        );
    }
}

#[test]
fn selection_uses_arrow_and_underlined_tab_without_reverse_fill() {
    use ratatui::style::Modifier;
    let (mut state, mut model) = fixture_board();
    apply_intent(
        &mut state,
        &mut model,
        BoardIntent::SelectNavTab(NavTab::Desk),
        None,
    )
    .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(78, 24)).unwrap();
    terminal
        .draw(|f| {
            draw_board(f, &model);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(Modifier::REVERSED)),
        "board and tabs must not use reverse fill"
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "d" && cell.modifier.contains(Modifier::UNDERLINED)),
        "active desk tab must be underlined"
    );
    let frame = capture(&model, 78, "selection-style");
    assert!(
        frame.contains("▸ ■ T12"),
        "selection arrow must retain status and number: {frame}"
    );
    let tab_row = buffer
        .content
        .chunks(78)
        .find(|row| {
            row.iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
                .contains("desk")
        })
        .unwrap();
    let text = tab_row.iter().map(|cell| cell.symbol()).collect::<String>();
    for (label, active) in [("desk", true), ("tsk-parity", false), ("projects", false)] {
        let start = text[..text.find(label).unwrap()].chars().count();
        for cell in &tab_row[start..start + label.len()] {
            assert_eq!(
                cell.modifier.contains(Modifier::UNDERLINED),
                active,
                "{label}"
            );
            assert_eq!(cell.modifier.contains(Modifier::BOLD), active, "{label}");
            assert_eq!(cell.modifier.contains(Modifier::DIM), !active, "{label}");
        }
    }
    key(
        &mut state,
        &mut model,
        KeyCode::Down,
        KeyModifiers::NONE,
        78,
    );
    terminal
        .draw(|f| {
            draw_board(f, &model);
        })
        .unwrap();
    assert!(capture(&model, 78, "wrapped-selection").contains("▸ ○ T13"));
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(Modifier::REVERSED)),
        "wrapped selected rows must not use reverse fill"
    );
}
