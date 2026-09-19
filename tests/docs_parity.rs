//! The product docs under `site/` describe what ships. These checks read the shipped
//! Markdown and compare it with the code, so a table row can no longer promise an action
//! the board does not offer (the palette listed a `Set done` that never existed).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::ui::board::{BoardInputMode, BoardModel};
use tsk_tui::ui::input::{map_key, BoardIntent};

fn docs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("site/src/content/docs/docs")
}

fn read_doc(name: &str) -> String {
    let path = docs_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The rows of the first Markdown table after `heading`, as trimmed cell vectors, header
/// and separator excluded.
fn table_after(doc: &str, heading: &str) -> Vec<Vec<String>> {
    let start = doc
        .find(heading)
        .unwrap_or_else(|| panic!("heading {heading:?} missing"));
    let mut rows = Vec::new();
    let mut in_table = false;
    for line in doc[start + heading.len()..].lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') {
            in_table = true;
            let cells: Vec<String> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect();
            if cells
                .iter()
                .all(|cell| cell.chars().all(|c| c == '-' || c == ':'))
            {
                continue;
            }
            rows.push(cells);
        } else if in_table {
            break;
        }
    }
    assert!(rows.len() > 1, "table after {heading:?} has no rows");
    rows.remove(0);
    rows
}

/// Expand one "Available actions" cell into the palette labels it promises.
fn promised_labels(cell: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for part in cell.split(',') {
        let part = part.trim().to_lowercase();
        if part == "set assignee" {
            labels.push(part);
        } else if let Some(rest) = part.strip_prefix("set ") {
            // "Set open/ready/started/blocked/review" is five status commands.
            for status in rest.split('/') {
                labels.push(format!("set status: {}", status.trim()));
            }
        } else {
            labels.push(part);
        }
    }
    labels
}

fn selected_model() -> BoardModel {
    let mut domain = DomainState::new();
    domain
        .create(
            "palette witness",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("seed task");
    let model = BoardModel::from_tasks(domain.tasks().to_vec(), None);
    assert!(model.selected_id().is_some(), "a task must be selected");
    model
}

fn labels(model: &BoardModel) -> Vec<String> {
    model
        .available_commands()
        .iter()
        .map(|command| command.label.to_lowercase())
        .collect()
}

#[test]
fn board_md_palette_table_matches_the_palette_catalog() {
    let doc = read_doc("board.md");
    let rows = table_after(&doc, "## Palette");

    // Three board states, one per documented condition. "Always" is checked against an
    // empty board, so a command that quietly became selection-gated fails here.
    let empty = BoardModel::from_tasks(Vec::new(), None);
    assert!(empty.selected_id().is_none());
    let always = labels(&empty);

    let mut model = selected_model();
    let with_selection = labels(&model);

    model.begin_save_recovery("disk full");
    let recovery = labels(&model);

    let mut documented: Vec<(String, &str)> = Vec::new();
    for row in &rows {
        let (actions, when) = (&row[0], &row[1]);
        let available = match when.as_str() {
            "Always" => &always,
            "A task is selected" => &with_selection,
            "A save has failed" => &recovery,
            other => panic!("unknown palette condition {other:?} in board.md"),
        };
        for label in promised_labels(actions) {
            assert!(
                available.contains(&label),
                "board.md promises palette action {label:?} ({when}), the catalog offers {available:?}"
            );
            documented.push((label, when.as_str()));
        }
    }

    // The conditions are exact: an "Always" command really is offered with nothing
    // selected, and a selection-only command really is absent from the empty board.
    for (label, when) in &documented {
        match *when {
            "A task is selected" => assert!(
                !always.contains(label),
                "{label:?} is documented as selection-only but the empty board offers it"
            ),
            "Always" => assert!(
                with_selection.contains(label),
                "{label:?} is documented as always available but vanishes with a selection"
            ),
            _ => {}
        }
    }

    // And the other way: every command the palette offers, in every state, is documented
    // under a condition that covers that state.
    let covers = |label: &String, states: &[&str]| {
        documented
            .iter()
            .any(|(doc_label, when)| doc_label == label && states.contains(when))
    };
    for label in &always {
        assert!(
            covers(label, &["Always"]),
            "the empty board offers {label:?} but board.md does not list it as Always"
        );
    }
    for label in &with_selection {
        assert!(
            covers(label, &["Always", "A task is selected"]),
            "a selected board offers {label:?} but board.md's table does not list it"
        );
    }
    for label in &recovery {
        assert!(
            covers(label, &["A save has failed"]),
            "save recovery offers {label:?} but board.md does not list it under a failed save"
        );
    }
}

/// The intent each documented Board-table action names. A row whose action is not listed
/// here fails the test, so a new documented chord must be classified, not skipped.
fn expected_intent(action: &str) -> Option<BoardIntent> {
    let action = action.to_lowercase();
    Some(if action.starts_with("edit title") {
        BoardIntent::BeginEditTitle
    } else if action.starts_with("start") {
        BoardIntent::PrimaryVerb
    } else if action.starts_with("set ready") {
        BoardIntent::SetStatus(HumanStatus::Ready)
    } else if action.starts_with("set open") {
        BoardIntent::Reopen
    } else if action.starts_with("mark done") {
        BoardIntent::Complete
    } else if action.starts_with("toggle blocked") {
        BoardIntent::ToggleBlock
    } else if action.starts_with("toggle review") {
        BoardIntent::ToggleReview
    } else if action.starts_with("delete") {
        BoardIntent::SoftDelete
    } else if action.starts_with("undo") {
        BoardIntent::Undo
    } else if action.starts_with("archive") {
        BoardIntent::File
    } else if action.starts_with("quit") {
        BoardIntent::Quit
    } else {
        return None;
    })
}

#[test]
fn keys_md_board_table_matches_the_normal_mode_keymap() {
    let doc = read_doc("keys.md");
    let rows = table_after(&doc, "## Board");
    let mut checked = 0;
    for row in rows {
        let (action, keys) = (&row[0], &row[1]);
        // Single ctrl chords are checked here against the action the row names; the
        // rest of the table is exercised by tests/v1_keymap_guard.rs.
        let Some(letter) = keys
            .split(" or ")
            .next()
            .and_then(|first| first.trim().strip_prefix("`ctrl+"))
            .and_then(|rest| rest.strip_suffix('`'))
            .filter(|letter| letter.len() == 1)
        else {
            continue;
        };
        let expected = expected_intent(action)
            .unwrap_or_else(|| panic!("keys.md row {action:?} ({keys}) is not classified"));
        let intent = map_key(
            BoardInputMode::Normal,
            KeyEvent::new(
                KeyCode::Char(letter.chars().next().unwrap()),
                KeyModifiers::CONTROL,
            ),
        );
        assert_eq!(
            intent,
            Some(expected),
            "keys.md says {keys} {action:?}, normal mode maps it to {intent:?}"
        );
        checked += 1;
    }
    assert!(
        checked >= 8,
        "expected the ctrl verbs in keys.md, checked {checked}"
    );
}

/// keys.md's editing prose: in the step editor `ctrl+d` and `ctrl+o` address the task,
/// `ctrl+x` stages the step's removal (no confirm press inside the editor) and `ctrl+a`
/// adds a step.
#[test]
fn keys_md_step_editor_chords_match_the_edit_step_keymap() {
    let doc = read_doc("keys.md");
    assert!(
        doc.contains("In the step editor, `ctrl+d` and `ctrl+o` still address the task; `ctrl+x` removes the step being edited, staged until the task edit is saved; `ctrl+a` adds another step."),
        "keys.md step-editor sentence changed; update this test with the code it now claims"
    );
    let chord = |letter: char| {
        map_key(
            BoardInputMode::EditStep,
            KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL),
        )
    };
    assert_eq!(chord('d'), Some(BoardIntent::Complete));
    assert_eq!(chord('o'), Some(BoardIntent::Reopen));
    assert_eq!(chord('x'), Some(BoardIntent::SoftDelete));
    assert_eq!(chord('a'), Some(BoardIntent::BeginAddStep));
    assert_eq!(chord('n'), None, "ctrl+n is not a step-editor chord");
}
