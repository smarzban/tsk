use std::path::PathBuf;

use tsk_tui::context::InvocationSnapshot;
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::scope::{resolve_project_path, ProjectResolveError};
use tsk_tui::ui::board::{apply_intent, BoardModel, IntentOutcome};
use tsk_tui::ui::input::BoardIntent;

fn snapshot() -> InvocationSnapshot {
    InvocationSnapshot {
        default_scope: TaskScope::Project {
            path: "/repos/default".into(),
        },
        this_repo: Some(PathBuf::from("/repos/tsk-board")),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    }
}

fn create_project(domain: &mut DomainState, path: &str) {
    domain
        .create(
            "known project",
            None,
            TaskScope::Project { path: path.into() },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create fixture project");
}

fn board_quick_add_scope(
    domain: &mut DomainState,
    model: &mut BoardModel,
    snapshot: &InvocationSnapshot,
    title: &str,
) -> TaskScope {
    assert_eq!(
        apply_intent(domain, model, BoardIntent::OpenCapture, Some(snapshot),)
            .expect("open quick add"),
        IntentOutcome::None
    );
    for character in title.chars() {
        apply_intent(domain, model, BoardIntent::QuickAddInsert(character), None)
            .expect("type quick-add title");
    }
    assert_eq!(
        apply_intent(domain, model, BoardIntent::QuickAddSave, None).expect("save quick add"),
        IntentOutcome::Persist
    );
    model.sync_from_domain(domain);
    domain.tasks().last().expect("saved task").scope.clone()
}

#[test]
fn invocation_directory_candidate_participates_in_basename_resolution() {
    let invocation = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let basename = invocation
        .file_name()
        .expect("manifest directory basename")
        .to_string_lossy();
    // Use the same separator as the invocation path so the stored and invocation
    // paths are distinguishable (different parents) but not ambiguous on Windows.
    let parent = invocation
        .parent()
        .expect("manifest parent")
        .to_string_lossy();
    let stored = format!("{parent}/repos/{basename}");
    let mut domain = DomainState::new();
    create_project(&mut domain, &stored);
    let snapshot = InvocationSnapshot {
        default_scope: TaskScope::Global,
        this_repo: Some(invocation.clone()),
        title_prefill: None,
        provenance: ProvenanceOrigin::Capture,
    };

    let mut expected = vec![stored, invocation.to_string_lossy().into_owned()];
    expected.sort();
    assert_eq!(
        resolve_project_path(&basename, &domain, Some(&snapshot)),
        Err(ProjectResolveError::Ambiguous(expected))
    );
    assert_eq!(
        resolve_project_path(&invocation.to_string_lossy(), &domain, Some(&snapshot)),
        Ok(invocation.to_string_lossy().into_owned()),
        "the invocation directory remains addressable by its exact path"
    );
}

#[test]
fn shared_resolver_and_board_quick_add_agree_on_fixtures() {
    let mut domain = DomainState::new();
    create_project(&mut domain, "/repos/other");
    create_project(&mut domain, "/repos/normal");
    create_project(&mut domain, "/repos/ghost");
    let ghost = domain.tasks().last().expect("ghost task").id;
    domain.soft_delete(ghost).expect("soft delete ghost");
    let snapshot = snapshot();
    let mut model = BoardModel::from_domain(&domain, snapshot.this_repo.clone());

    for (token, title, expected) in [
        ("normal", "normal path !p normal", "/repos/normal"),
        ("ghost", "soft deleted path !p ghost", "/repos/ghost"),
        (
            "TSK-Board",
            "snapshot repo !p TSK-Board",
            "/repos/tsk-board",
        ),
    ] {
        assert_eq!(
            resolve_project_path(token, &domain, Some(&snapshot)),
            Ok(expected.into()),
            "shared resolver fixture {token:?}"
        );
        assert_eq!(
            board_quick_add_scope(&mut domain, &mut model, &snapshot, title),
            TaskScope::Project {
                path: expected.into(),
            },
            "board quick-add fixture {token:?}"
        );
    }

    let apply_source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ui/board/apply.rs"
    ))
    .expect("read board apply source");
    assert!(
        !apply_source.contains("fn resolve_quick_add_project_path"),
        "board apply must delegate to the shared resolver instead of retaining a private matcher"
    );
}
