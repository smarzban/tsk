//! Task Store: load/save survives reload.
//! Uses temp dirs only; never writes real plugin state.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, Step, TaskEventKind, TaskScope};
use tsk_tui::store::TaskStore;

fn temp_state_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-store-persist-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn platform_nanos(nanos: u32) -> u32 {
    #[cfg(windows)]
    {
        return nanos / 100 * 100;
    }
    #[cfg(not(windows))]
    nanos
}

fn platform_v5_fixture() -> Vec<u8> {
    #[cfg(not(windows))]
    return include_bytes!("fixtures/current_store_v5.json").to_vec();
    #[cfg(windows)]
    {
        // Round-trip through the persisted type, not Value's sorted map. SystemTime applies
        // Windows' 100 ns precision while DomainState preserves the canonical field order.
        let state: DomainState =
            serde_json::from_str(include_str!("fixtures/current_store_v5.json"))
                .expect("v5 fixture");
        serde_json::to_vec_pretty(&state).expect("encode platform fixture")
    }
}

fn current_store_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/current_store_v1.json"))
        .expect("current store fixture is valid JSON")
}

fn assert_store_document_refused_without_rewrite(document: serde_json::Value) {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let bytes = serde_json::to_vec_pretty(&document).expect("encode test document");
    fs::write(dir.join("tsk.json"), &bytes).expect("install test document");

    TaskStore::new(&dir)
        .load()
        .expect_err("removed wire shape must be refused");
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read refused document"),
        bytes,
        "a refused document must not be rewritten"
    );
}

#[test]
fn literal_current_v1_fixture_pins_the_complete_store_wire_shape() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v1.json"),
    )
    .expect("install current fixture");

    let mut state = TaskStore::new(&dir).load().expect("load current fixture");
    assert_eq!(state.format_version(), 5, "the v1 fixture loads migrated");
    assert!(state.projects().is_empty());
    assert_eq!(state.next_task_number, 8);
    assert_eq!(state.next_notice_number, 1);
    let task = state.tasks().first().expect("fixture task");
    assert_eq!(task.id.to_string(), "11111111-1111-4111-8111-111111111111");
    assert_eq!(task.number, Some(7));
    assert_eq!(
        task.revision.to_string(),
        "22222222-2222-4222-8222-222222222222"
    );
    assert_eq!(task.title, "Pinned wire task");
    assert_eq!(task.notes.as_deref(), Some("Line one\nLine two"));
    assert_eq!(task.thread.as_deref(), Some("release-2026"));
    assert_eq!(task.status, HumanStatus::Done);
    assert!(!task.archived);
    assert_eq!(
        task.scope,
        TaskScope::Project {
            path: "/tmp/example-project".into()
        }
    );
    assert_eq!(task.provenance, ProvenanceOrigin::Selection);
    assert_eq!(
        task.history
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            TaskEventKind::Created,
            TaskEventKind::StepAdded,
            TaskEventKind::Completed
        ]
    );
    assert_eq!(task.steps.len(), 1);
    assert_eq!(task.steps[0].text, "Pinned step");
    assert!(task.steps[0].done);
    assert!(!task.soft_deleted);
    assert_eq!(
        task.created_at
            .duration_since(UNIX_EPOCH)
            .expect("created after epoch"),
        std::time::Duration::new(1_700_000_000, platform_nanos(123_456_789))
    );
    assert_eq!(
        task.updated_at
            .duration_since(UNIX_EPOCH)
            .expect("updated after epoch"),
        std::time::Duration::new(1_700_000_100, platform_nanos(987_654_321))
    );

    state.undo().expect("fixture undo entry is current");
    assert_eq!(
        state
            .tasks()
            .first()
            .expect("fixture task after undo")
            .status,
        HumanStatus::Open
    );
}

#[test]
fn v1_document_loads_through_the_chain_and_first_save_leaves_tsk_json_v1_beside_the_live_file() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v1.json"),
    )
    .expect("install v1 fixture");
    let store = TaskStore::new(&dir);

    let state = store.load().expect("v1 document loads through the chain");
    assert_eq!(state.format_version(), 5);
    assert!(state.projects().is_empty(), "v1 has no archived projects");
    assert!(state.tasks().iter().all(|task| !task.archived));

    store.save(&state).expect("first save after migration");
    assert_eq!(
        fs::read(dir.join("tsk.json.v1")).expect("read version backup"),
        include_bytes!("fixtures/current_store_v1.json").as_slice(),
        "the pre-migration document is backed up byte-identically"
    );
    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(live["format_version"], 5);
    assert_eq!(live["projects"], serde_json::json!({}));
    assert_eq!(live["next_notice_number"], 1);
}

#[test]
fn v2_document_loads_with_notice_counter_one_and_first_save_leaves_tsk_json_v2_beside_the_live_file(
) {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v2.json"),
    )
    .expect("install v2 fixture");
    let store = TaskStore::new(&dir);

    let state = store.load().expect("v2 document loads through the chain");
    assert_eq!(state.format_version(), 5);
    assert_eq!(state.next_notice_number, 1);
    assert!(state.tasks().iter().all(|task| !task.is_notice()));
    assert_eq!(state.tasks()[0].board_identifier().as_deref(), Some("T7"));

    store.save(&state).expect("first save after migration");
    assert_eq!(
        fs::read(dir.join("tsk.json.v2")).expect("read version backup"),
        include_bytes!("fixtures/current_store_v2.json").as_slice(),
        "the pre-migration document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v5_fixture().as_slice(),
        "a migrated v2 document saves as the canonical platform v5 wire"
    );
}

#[test]
fn v3_document_loads_ready_as_open_and_first_save_leaves_tsk_json_v3_beside_the_live_file() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/current_store_v3.json"))
            .expect("v3 fixture is valid JSON");
    document["tasks"][0]["status"] = serde_json::json!("ready");
    let seeded = serde_json::to_vec_pretty(&document).expect("encode v3 with ready");
    fs::write(dir.join("tsk.json"), &seeded).expect("install v3 ready document");
    let store = TaskStore::new(&dir);

    let state = store
        .load()
        .expect("v3 document with ready tasks loads through the chain");
    assert_eq!(state.format_version(), 5);
    let task = state.tasks().first().expect("fixture task");
    assert_eq!(task.status, HumanStatus::Open, "ready migrates to open");
    assert_eq!(
        task.history
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            TaskEventKind::Created,
            TaskEventKind::StepAdded,
            TaskEventKind::Completed
        ],
        "migration must not append a history event"
    );

    store.save(&state).expect("first save after migration");
    assert_eq!(
        fs::read(dir.join("tsk.json.v3")).expect("read version backup"),
        seeded.as_slice(),
        "the pre-migration document is backed up byte-identically"
    );
    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(live["format_version"], 5);
    assert_eq!(live["tasks"][0]["status"], "open");
}

#[test]
fn v3_document_loads_through_the_chain_and_first_save_leaves_tsk_json_v3_beside_the_live_file() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v3.json"),
    )
    .expect("install v3 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v3 fixture");
    assert_eq!(loaded.format_version(), 5);
    assert!(loaded.projects().is_empty());
    assert_eq!(loaded.next_notice_number, 1);
    assert_eq!(
        loaded.tasks().first().expect("fixture task").status,
        HumanStatus::Done
    );

    store.save(&loaded).expect("save loaded state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v3")).expect("read version backup"),
        include_bytes!("fixtures/current_store_v3.json").as_slice(),
        "the pre-migration document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v5_fixture().as_slice(),
        "a migrated v3 document saves as the canonical platform v5 wire"
    );
}

#[test]
fn v4_document_migrates_to_v5_and_keeps_its_original_backup() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v4.json"),
    )
    .expect("install v4 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load v4 fixture");
    assert_eq!(loaded.format_version(), 5);
    assert!(loaded.projects().is_empty());
    assert_eq!(loaded.next_notice_number, 1);

    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v4")).expect("read version backup"),
        include_bytes!("fixtures/current_store_v4.json").as_slice(),
        "the v4 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v5_fixture().as_slice(),
        "a migrated v4 document saves as the canonical platform v5 wire"
    );
}

#[test]
fn literal_current_v5_fixture_round_trips_with_platform_timestamp_precision() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v5.json"),
    )
    .expect("install current v5 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v5 fixture");
    assert_eq!(loaded.format_version(), 5);
    assert!(loaded.projects().is_empty());
    assert_eq!(loaded.next_notice_number, 1);

    store.save(&loaded).expect("save loaded state");
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read resaved live document"),
        platform_v5_fixture().as_slice(),
        "an unchanged v5 state must serialize with the platform's timestamp precision"
    );
}

#[test]
fn v5_batch_undo_round_trips_and_still_reverts_the_whole_completion() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let first = state
        .create(
            "first",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create first");
    let second = state
        .create(
            "second",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create second");
    state
        .complete_batch(&[first, second])
        .expect("complete batch");
    store.save(&state).expect("save batch undo");

    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(live["format_version"], 5);
    assert!(live["undo_stack"][0].get("batch").is_some());

    let mut loaded = store.load().expect("load batch undo");
    loaded.undo().expect("undo reloaded batch");
    assert!([first, second]
        .into_iter()
        .all(|id| loaded.get(id).expect("task").status == HumanStatus::Open));
}

#[test]
fn notice_round_trips_with_its_n_number_and_no_t_number() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let notice = store
        .locked_transition(|state| {
            state
                .create_notice(
                    "welcome",
                    "Welcome",
                    Some("hello".into()),
                    HumanStatus::Ready,
                    TaskScope::Global,
                    vec![("first".into(), true)],
                )
                .map_err(|error| error.to_string())
        })
        .expect("persist notice");

    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(document["next_notice_number"], 2);
    assert_eq!(document["next_task_number"], 1);
    assert_eq!(
        document["tasks"][0]["notice"],
        serde_json::json!({ "catalog_id": "welcome", "number": 1 })
    );
    assert!(document["tasks"][0].get("number").is_none());

    let loaded = store.load().expect("reload");
    let task = loaded.get(notice).expect("notice survives the reload");
    assert_eq!(task.board_identifier().as_deref(), Some("N1"));
    assert_eq!(task.number, None);
    assert_eq!(task.steps[0].text, "first");
    assert!(task.steps[0].done, "a pre-checked step survives the reload");
}

#[test]
fn every_removed_wire_token_is_refused_at_the_store_boundary() {
    for status in ["todo", "doing"] {
        let mut document = current_store_fixture();
        document["tasks"][0]["status"] = serde_json::json!(status);
        assert_store_document_refused_without_rewrite(document);
    }

    for kind in [
        "checklist_item_added",
        "checklist_item_checked",
        "checklist_item_unchecked",
        "checklist_item_renamed",
        "checklist_item_removed",
        "parked",
        "agent_linked",
        "agent_unlinked",
        "dispatched",
    ] {
        let mut document = current_store_fixture();
        document["tasks"][0]["history"][0]["kind"] = serde_json::json!(kind);
        assert_store_document_refused_without_rewrite(document);
    }

    for field in ["checklist", "capsule", "agent_meta", "last_observed"] {
        let mut document = current_store_fixture();
        document["tasks"][0][field] = serde_json::Value::Null;
        assert_store_document_refused_without_rewrite(document);
    }

    let mut document = current_store_fixture();
    document["active_attempts"] = serde_json::json!([]);
    assert_store_document_refused_without_rewrite(document);
}

#[test]
fn thread_round_trips_through_save_and_load() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let id = state
        .create(
            "Threaded task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    state
        .edit(
            id,
            "Threaded task",
            None,
            TaskScope::Global,
            Some("release-2026".into()),
        )
        .expect("attach thread");

    store.save(&state).expect("save threaded task");
    let loaded = store.load().expect("reload threaded task");
    assert_eq!(
        loaded.get(id).and_then(|task| task.thread.as_deref()),
        Some("release-2026")
    );
}

#[test]
fn edit_with_thread_persists_through_one_locked_merge() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    let id = seed
        .create(
            "Merge thread",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    store.save(&seed).expect("seed task");

    let mut local = store.load().expect("load local task");
    local
        .edit(
            id,
            "Merge thread",
            None,
            TaskScope::Global,
            Some("release-2026".into()),
        )
        .expect("edit thread");
    store
        .reload_merge_save(&mut local)
        .expect("one locked merge persists thread edit");

    assert_eq!(
        local.get(id).and_then(|task| task.thread.as_deref()),
        Some("release-2026")
    );
    assert_eq!(
        store
            .load()
            .expect("reload merged state")
            .get(id)
            .and_then(|task| task.thread.as_deref()),
        Some("release-2026")
    );
}

#[test]
fn conflicting_concurrent_thread_edits_reject_via_revision_guard() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    let id = seed
        .create(
            "Conflicting threads",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    store.save(&seed).expect("seed task");

    let mut local = store.load().expect("load local writer");
    let mut concurrent = store.load().expect("load concurrent writer");
    local
        .edit(
            id,
            "Conflicting threads",
            None,
            TaskScope::Global,
            Some("local".into()),
        )
        .expect("stage local thread edit");
    concurrent
        .edit(
            id,
            "Conflicting threads",
            None,
            TaskScope::Global,
            Some("concurrent".into()),
        )
        .expect("stage concurrent thread edit");
    store.save(&concurrent).expect("save concurrent edit");

    let error = store
        .reload_merge_save(&mut local)
        .expect_err("divergent thread edits must conflict");
    assert!(error.to_string().contains("changed during save"));
    assert_eq!(
        store
            .load()
            .expect("reload durable concurrent edit")
            .get(id)
            .and_then(|task| task.thread.as_deref()),
        Some("concurrent")
    );
}

#[test]
fn steps_round_trip_preserves_identity_flags_and_order() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut state = DomainState::new();
    let id = state
        .create(
            "Steps round trip",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    let first = state.add_step(id, "First").expect("add first");
    let second = state.add_step(id, "Second").expect("add second");
    let third = state.add_step(id, "Third").expect("add third");
    state.toggle_step(id, first).expect("toggle first done");
    state.toggle_step(id, third).expect("toggle third done");

    store.save(&state).expect("save steps state");

    let loaded = store.load().expect("reload steps state");
    let task = loaded.get(id).expect("task survives reload");
    assert_eq!(
        task.steps,
        vec![
            Step {
                id: first,
                text: "First".into(),
                done: true,
            },
            Step {
                id: second,
                text: "Second".into(),
                done: false,
            },
            Step {
                id: third,
                text: "Third".into(),
                done: true,
            },
        ],
        "identity, text, done flag, and order must round-trip unchanged"
    );
}

#[test]
fn save_then_load_preserves_task_id_title_status_and_events() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());

    let mut state = DomainState::new();
    let id = state
        .create(
            "Fix flake",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");

    {
        let store = TaskStore::new(&dir);
        store.save(&state).expect("save domain state");
    } // drop store

    let store = TaskStore::new(&dir);
    let loaded = store.load().expect("load after drop");
    let task = loaded.get(id).expect("task present after reload");
    assert_eq!(task.id, id);
    assert_eq!(task.title, "Fix flake");
    assert_eq!(task.status, HumanStatus::Open);
    assert!(
        task.history
            .iter()
            .any(|e| e.kind == TaskEventKind::Created),
        "Created event must survive reload"
    );
}

#[test]
fn two_writers_merge_save_do_not_lose_tasks() {
    // Simulate: store has task A; second writer with B-only local merge-saves; both remain.
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut writer_a = DomainState::new();
    let id_a = writer_a
        .create(
            "Task A",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create A");
    store.save(&writer_a).expect("writer A save");

    let mut writer_b = DomainState::new();
    let id_b = writer_b
        .create(
            "Task B",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Capture,
            None,
        )
        .expect("create B");
    store
        .reload_merge_save(&mut writer_b)
        .expect("writer B merge-save");

    assert!(writer_b.get(id_a).is_some(), "A must survive merge");
    assert!(writer_b.get(id_b).is_some(), "B must survive merge");

    let loaded = store.load().expect("final load");
    assert_eq!(loaded.tasks().len(), 2);
    assert!(loaded.get(id_a).is_some());
    assert!(loaded.get(id_b).is_some());
}

#[test]
fn stale_undo_refuses_newer_task_revision_without_popping_newer_state() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut stale_writer = DomainState::new();
    let id = stale_writer
        .create(
            "Concurrent task",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create task");
    stale_writer.complete(id).expect("complete task");
    store.save(&stale_writer).expect("save undoable action");
    // A successful durable save clears the transient merge intent on reload. The stale
    // writer now represents an unchanged board snapshot carrying its guarded Undo entry.
    stale_writer = store.load().expect("reload persisted undoable action");

    let mut newer_writer = store.load().expect("load concurrent writer");
    std::thread::sleep(std::time::Duration::from_millis(5));
    newer_writer
        .edit(
            id,
            "Changed concurrently",
            Some("newer notes".into()),
            TaskScope::Global,
            None,
        )
        .expect("newer semantic mutation");
    store.save(&newer_writer).expect("save newer task revision");

    store
        .reload_merge_save(&mut stale_writer)
        .expect("merge newer durable state");
    let newer_task = stale_writer.get(id).expect("merged task").clone();

    let error = stale_writer.undo().expect_err("stale undo must be refused");
    assert!(error.to_string().contains("changed since"));
    assert_eq!(stale_writer.get(id), Some(&newer_task));

    let repeated = stale_writer
        .undo()
        .expect_err("refused undo entry remains guarded");
    assert_eq!(repeated, error);
    assert_eq!(stale_writer.get(id), Some(&newer_task));
}

#[test]
fn reload_merge_save_keeps_numbers_and_counter_across_another_writer() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut local = DomainState::new();
    let mine = local
        .create(
            "mine",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create mine");
    store.reload_merge_save(&mut local).expect("persist mine");
    let mine_number = local
        .get(mine)
        .and_then(|task| task.number)
        .expect("synced number");

    store
        .locked_transition(|state| {
            state
                .create(
                    "theirs",
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .map_err(|error| error.to_string())
        })
        .expect("another writer persists");
    store
        .locked_transition(|state| {
            // Gaps are valid: a merge must retain a counter ahead of every assigned number.
            state.next_task_number = 10;
            Ok(())
        })
        .expect("advance counter");

    local
        .set_status(mine, HumanStatus::Started)
        .expect("change mine");
    store.reload_merge_save(&mut local).expect("persist status");
    assert_eq!(
        local.get(mine).and_then(|task| task.number),
        Some(mine_number)
    );
    let after_status = store.load().expect("load");
    assert_eq!(
        after_status.get(mine).and_then(|task| task.number),
        Some(mine_number)
    );
    assert_eq!(
        after_status.next_task_number, 10,
        "merge must retain the disk counter"
    );

    let later = local
        .create(
            "later",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create later");
    store.reload_merge_save(&mut local).expect("persist later");
    assert_eq!(local.get(later).and_then(|task| task.number), Some(10));
}

#[test]
fn overlapping_creates_under_lock_receive_distinct_numbers() {
    const WRITERS: usize = 8;
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let barrier = Arc::new(Barrier::new(WRITERS));
    let ids = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let store = store.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    store.locked_transition(|state| {
                        state
                            .create(
                                format!("writer {writer}"),
                                None,
                                TaskScope::Global,
                                ProvenanceOrigin::Manual,
                                None,
                            )
                            .map_err(|error| error.to_string())
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("writer thread").expect("create"))
            .collect::<Vec<_>>()
    });
    let state = store.load().expect("load");
    let numbers: HashSet<u64> = ids
        .iter()
        .map(|&id| state.get(id).and_then(|task| task.number).expect("number"))
        .collect();
    assert_eq!(state.tasks().len(), WRITERS);
    assert_eq!(numbers.len(), WRITERS);
}

#[test]
fn archive_flag_survives_save_and_load() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let kept = state
        .create(
            "Kept open",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create kept task");
    let filed = state
        .create(
            "Filed away",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create filed task");
    state.archive_task(filed).expect("archive one task");

    store.save(&state).expect("save archived state");
    let loaded = store.load().expect("reload archived state");
    let filed_task = loaded.get(filed).expect("archived task survives reload");
    assert!(filed_task.archived);
    assert_eq!(filed_task.status, HumanStatus::Open, "status is untouched");
    assert!(
        !loaded
            .get(kept)
            .expect("kept task survives reload")
            .archived
    );

    // An unarchived task serialises without an archived key.
    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    let tasks = document["tasks"].as_array().expect("tasks array");
    assert_eq!(tasks.len(), 2);
    assert!(!tasks[0]
        .as_object()
        .expect("task object")
        .contains_key("archived"));
    assert_eq!(tasks[1]["archived"], serde_json::json!(true));
}

#[test]
fn reload_merge_save_keeps_a_sibling_writers_project_record_and_applies_the_local_intent() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    for path in ["/repos/p", "/repos/q"] {
        seed.create(
            "seed",
            None,
            TaskScope::Project {
                path: path.to_string(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create seed task");
    }
    store.save(&seed).expect("seed two projects");

    // Writer A's snapshot is stale: taken before writer B archives Q on disk.
    let mut a = store.load().expect("stale writer A");
    let mut b = store.load().expect("writer B");
    b.archive_project("/repos/q").expect("B archives Q");
    store.save(&b).expect("B persists the Q record");

    // A archives P against its stale (empty) map, then merge-saves: the disk map
    // replaces A's, A's own intent is re-applied, and B's record survives.
    a.archive_project("/repos/p").expect("A archives P");
    store.reload_merge_save(&mut a).expect("A merge-save");
    let disk = store.load().expect("reload");
    assert_eq!(
        disk.archived_projects(),
        std::collections::BTreeSet::from(["/repos/p".to_string(), "/repos/q".to_string()]),
        "both records are on disk after the merge"
    );

    // A unarchives P through a locked merge: a plain union would resurrect it, the
    // recorded intent must win.
    a.unarchive_project("/repos/p").expect("A unarchives P");
    store
        .reload_merge_save(&mut a)
        .expect("A merge-save unarchive");
    let disk = store.load().expect("reload");
    assert_eq!(
        disk.archived_projects(),
        std::collections::BTreeSet::from(["/repos/q".to_string()]),
        "only Q remains on disk"
    );
}

#[test]
fn noncurrent_store_is_refused_without_rewriting_the_file() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let document = serde_json::json!({ "format_version": 6, "next_task_number": 2, "tasks": [], "undo_stack": [] });
    let bytes = serde_json::to_vec_pretty(&document).expect("json");
    fs::write(dir.join("tsk.json"), &bytes).expect("write noncurrent store");
    let error = TaskStore::new(&dir)
        .save(&DomainState::new())
        .expect_err("current writer refuses noncurrent store");
    assert!(error.to_string().contains("expected 5"));
    assert_eq!(fs::read(dir.join("tsk.json")).expect("read"), bytes);
}

#[test]
fn v1_migration_strips_defensive_archived_keys() {
    // A v1 binary never wrote an archived key; the chain removes one defensively so
    // "a v1 document has no archived tasks" holds literally after the load.
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/current_store_v1.json"))
            .expect("v1 fixture is valid JSON");
    assert_eq!(document["format_version"], 1);
    document["tasks"][0]["archived"] = serde_json::json!(true);

    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        serde_json::to_vec_pretty(&document).expect("encode v1 with archived key"),
    )
    .expect("install document");

    let state = TaskStore::new(&dir)
        .load()
        .expect("load v1 through the chain");
    assert_eq!(state.format_version(), 5);
    let task = state.tasks().first().expect("fixture task");
    assert!(
        !task.archived,
        "migration must strip the defensive archived key"
    );
    assert_eq!(task.status, HumanStatus::Done, "status is untouched");
}
