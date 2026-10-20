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
        nanos / 100 * 100
    }
    #[cfg(not(windows))]
    nanos
}

fn platform_v6_fixture() -> Vec<u8> {
    #[cfg(not(windows))]
    return include_bytes!("fixtures/current_store_v6.json").to_vec();
    #[cfg(windows)]
    {
        // Round-trip through the persisted type, not Value's sorted map. SystemTime applies
        // Windows' 100 ns precision while DomainState preserves the canonical field order.
        let state: DomainState =
            serde_json::from_str(include_str!("fixtures/current_store_v6.json"))
                .expect("v6 fixture");
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
    assert_eq!(state.format_version(), 6, "the v1 fixture loads migrated");
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
    assert_eq!(state.format_version(), 6);
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
    assert_eq!(live["format_version"], 6);
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
    assert_eq!(state.format_version(), 6);
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
        platform_v6_fixture().as_slice(),
        "a migrated v2 document saves as the canonical platform v6 wire"
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
    assert_eq!(state.format_version(), 6);
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
    assert_eq!(live["format_version"], 6);
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
    assert_eq!(loaded.format_version(), 6);
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
        platform_v6_fixture().as_slice(),
        "a migrated v3 document saves as the canonical platform v6 wire"
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
    assert_eq!(loaded.format_version(), 6);
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
        platform_v6_fixture().as_slice(),
        "a migrated v4 document saves as the canonical platform v6 wire"
    );
}

#[test]
fn literal_v5_fixture_migrates_to_canonical_v6() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v5.json"),
    )
    .expect("install current v5 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v5 fixture");
    assert_eq!(loaded.format_version(), 6);
    assert!(loaded.projects().is_empty());
    assert_eq!(loaded.next_notice_number, 1);
    assert_eq!(loaded.tasks()[0].assignee, None);

    store.save(&loaded).expect("save loaded state");
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read resaved live document"),
        platform_v6_fixture().as_slice(),
        "a v5 state must serialize as canonical platform v6"
    );
}

/// v6 is the current format: it opens with no migration, writes no version copy, and saves
/// byte for byte.
#[test]
fn literal_v6_fixture_is_current_and_round_trips() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v6 = platform_v6_fixture();
    fs::write(dir.join("tsk.json"), &v6).expect("install current v6 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v6 fixture");
    assert_eq!(loaded.format_version(), 6);
    assert_eq!(loaded.tasks()[0].block, None);
    assert!(loaded.tasks()[0].past_blocks.is_empty());

    store.save(&loaded).expect("save current state");
    assert!(
        !dir.join("tsk.json.v6").exists(),
        "a current document needs no version copy"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read live document"),
        v6,
        "a v6 state must serialize as canonical platform v6"
    );
    let reloaded = store.load().expect("reload v6");
    assert_eq!(
        reloaded.tasks(),
        loaded.tasks(),
        "v6 round-trips losslessly"
    );
}

#[test]
fn literal_dev_v7_fixture_opens_as_v6_and_backs_up() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v7 = include_bytes!("fixtures/current_store_v7.json");
    fs::write(dir.join("tsk.json"), v7).expect("install current v7 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v7 fixture");
    assert_eq!(loaded.format_version(), 6);
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v7")).expect("read version backup"),
        v7.as_slice(),
        "the v7 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v7 state must serialize as canonical platform v6"
    );
}

#[test]
fn literal_dev_v8_fixture_opens_as_v6_and_backs_up() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v8 = include_bytes!("fixtures/current_store_v8.json");
    fs::write(dir.join("tsk.json"), v8).expect("install current v8 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v8 fixture");
    assert_eq!(loaded.format_version(), 6);
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v8")).expect("read version backup"),
        v8.as_slice(),
        "the v8 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v8 state must serialize as canonical platform v6"
    );
}

#[test]
fn literal_dev_v9_fixture_opens_as_v6_and_backs_up() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v9 = include_bytes!("fixtures/current_store_v9.json");
    fs::write(dir.join("tsk.json"), v9).expect("install current v9 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v9 fixture");
    assert_eq!(loaded.format_version(), 6);
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v9")).expect("read version backup"),
        v9.as_slice(),
        "the v9 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v9 state must serialize as canonical platform v6"
    );
}

/// A v10 task runs after nothing: it migrates with an empty `after`, which stays off the wire,
/// and a link set after migration round-trips.
#[test]
fn literal_dev_v10_fixture_opens_as_v6_backs_up_and_gains_after() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v10 = include_bytes!("fixtures/current_store_v10.json");
    fs::write(dir.join("tsk.json"), v10).expect("install current v10 fixture");
    let store = TaskStore::new(&dir);

    let mut loaded = store.load().expect("load current v10 fixture");
    assert_eq!(loaded.format_version(), 6);
    assert!(loaded.tasks().iter().all(|task| task.after.is_empty()));
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v10")).expect("read version backup"),
        v10.as_slice(),
        "the v10 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v10 state must serialize as canonical platform v6"
    );

    let prerequisite = loaded
        .create(
            "prerequisite",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let waiting = loaded
        .create(
            "waiting",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    store.reload_merge_save(&mut loaded).expect("number them");
    let number = loaded
        .get(prerequisite)
        .and_then(|task| task.number)
        .expect("number");
    loaded.set_after(waiting, &[number]).expect("set after");
    store.reload_merge_save(&mut loaded).expect("save the link");
    let reloaded = store.load().expect("reload v6");
    assert_eq!(reloaded.get(waiting).expect("task").after, vec![number]);
}

#[test]
fn literal_dev_v11_fixture_opens_as_v6_and_backs_up() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v11 = include_bytes!("fixtures/current_store_v11.json");
    fs::write(dir.join("tsk.json"), v11).expect("install current v11 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v11 fixture");
    assert_eq!(loaded.format_version(), 6);
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v11")).expect("read version backup"),
        v11.as_slice(),
        "the v11 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v11 state must serialize as canonical platform v6"
    );
}

/// A v9 event carries kind and time only: it migrates without an author or detail, renders on
/// the paper trail without who or what, and a change after migration records both.
#[test]
fn v9_events_render_without_who_or_what_and_new_events_carry_both() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    fs::write(
        dir.join("tsk.json"),
        include_bytes!("fixtures/current_store_v9.json"),
    )
    .expect("install current v9 fixture");
    let store = TaskStore::new(&dir);

    let mut loaded = store.load().expect("load v9");
    let task = loaded.tasks()[0].clone();
    assert!(!task.history.is_empty());
    assert!(task
        .history
        .iter()
        .all(|event| event.by.is_none() && event.detail.is_none()));
    let trail = tsk_tui::activity::paper_trail(&task);
    assert!(trail.iter().all(|entry| entry.by.is_none()), "{trail:?}");
    assert_eq!(
        trail.last().map(|entry| entry.text.as_str()),
        Some("created")
    );

    let from = task.status;
    let to = if from == HumanStatus::Ready {
        HumanStatus::Open
    } else {
        HumanStatus::Ready
    };
    loaded.set_status(task.id, to).expect("status");
    store.save(&loaded).expect("save");
    let reloaded = store.load().expect("reload v6");
    let event = reloaded.tasks()[0].history.last().cloned().expect("event");
    assert_eq!(event.by.as_deref(), Some("you"));
    assert_eq!(
        event.detail,
        Some(tsk_tui::domain::EventDetail::status(from, to))
    );
}

/// A v8 task in review has no round: it loads as in review on you with nothing recorded, and
/// its first review round after migration is round 1.
#[test]
fn v8_review_task_without_a_round_migrates_and_reads_as_review() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let mut v8: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/current_store_v8.json")).expect("v8 json");
    v8["tasks"][0]["status"] = serde_json::json!("review");
    fs::write(
        dir.join("tsk.json"),
        serde_json::to_vec_pretty(&v8).expect("encode"),
    )
    .expect("install v8 review document");
    let store = TaskStore::new(&dir);

    let mut loaded = store.load().expect("load v8 review document");
    let id = loaded.tasks()[0].id;
    assert_eq!(loaded.tasks()[0].status, HumanStatus::Review);
    assert_eq!(loaded.tasks()[0].block, None);
    loaded
        .review(
            id,
            tsk_tui::domain::ReviewDraft::from_input(
                Some("built it"),
                &["tests pass".into()],
                None,
                tsk_tui::domain::BlockOn::You,
            )
            .expect("draft"),
            "claude",
        )
        .expect("review");
    store.save(&loaded).expect("save");
    let reloaded = store.load().expect("reload v6");
    let round = reloaded.tasks()[0].block.clone().expect("open round");
    assert!(round.is_review());
    assert_eq!(round.round, 1);
    assert_eq!(round.done.as_deref(), Some("built it"));
    assert_eq!(
        reloaded.tasks()[0].block,
        loaded.tasks()[0].block,
        "v6 round-trips the round losslessly"
    );
}

#[test]
fn v6_blocked_task_without_a_block_migrates_and_reads_as_blocked() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/current_store_v6.json")).expect("v6 json");
    document["tasks"][0]["status"] = serde_json::json!("blocked");
    fs::write(
        dir.join("tsk.json"),
        serde_json::to_vec_pretty(&document).expect("encode"),
    )
    .expect("install blocked v6");
    let loaded = TaskStore::new(&dir).load().expect("load blocked v6");
    assert_eq!(loaded.tasks()[0].status, HumanStatus::Blocked);
    assert_eq!(loaded.tasks()[0].block, None);
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
    assert_eq!(live["format_version"], 6);
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
    let document = serde_json::json!({ "format_version": 16, "next_task_number": 2, "tasks": [], "undo_stack": [] });
    let bytes = serde_json::to_vec_pretty(&document).expect("json");
    fs::write(dir.join("tsk.json"), &bytes).expect("write noncurrent store");
    let error = TaskStore::new(&dir)
        .save(&DomainState::new())
        .expect_err("current writer refuses noncurrent store");
    assert!(error.to_string().contains("expected 6"));
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
    assert_eq!(state.format_version(), 6);
    let task = state.tasks().first().expect("fixture task");
    assert!(
        !task.archived,
        "migration must strip the defensive archived key"
    );
    assert_eq!(task.status, HumanStatus::Done, "status is untouched");
}

/// A v10 store with everything optional populated (assignee, base, a live dispatch record, an
/// open block with options and a reply, two review rounds, authored and detailed events, steps,
/// an archived project, a batch undo entry) migrates losslessly: the first save writes it back
/// unchanged but for the version, and backs up the original byte-identically.
#[test]
fn populated_v10_fixture_round_trips_losslessly_to_v6() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v10 = include_str!("fixtures/populated_store_v10.json");
    fs::write(dir.join("tsk.json"), v10).expect("install populated v10 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load populated v10 fixture");
    assert_eq!(loaded.format_version(), 6);
    let task = loaded
        .tasks()
        .iter()
        .find(|task| task.number == Some(1))
        .expect("T1");
    assert!(task.dispatch.is_some() && task.assignee.is_some() && task.base.is_some());
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read_to_string(dir.join("tsk.json.v10")).expect("read version backup"),
        v10,
        "the v10 document is backed up byte-identically"
    );
    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.join("tsk.json")).expect("read migrated live document"),
    )
    .expect("json");
    let mut expected: serde_json::Value = serde_json::from_str(v10).expect("json");
    expected["format_version"] = 6.into();
    with_complete_previous_open(&mut expected);
    assert_eq!(
        saved, expected,
        "nothing but the version and each done's undo status change"
    );
}

/// A pre-v13 done's undo entry gains the status it restores, `open`: where its undo always went.
fn with_complete_previous_open(document: &mut serde_json::Value) {
    fn walk(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::Object(complete)) = map.get_mut("complete") {
                    let id = complete.remove("id").expect("complete id");
                    let revision = complete
                        .remove("expected_revision")
                        .expect("complete revision");
                    complete.insert("id".into(), id);
                    complete.insert("previous".into(), "open".into());
                    complete.insert("expected_revision".into(), revision);
                }
                map.values_mut().for_each(walk);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut document["undo_stack"]);
}

/// A v11 store's replies carry no `sent` marker: they migrate as not yet delivered, so the
/// record's unblock or send-back delivers them, and the first save writes the document back
/// unchanged but for the version after backing up the original byte-identically.
#[test]
fn v11_replies_open_as_v6_as_not_yet_sent() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/populated_store_v10.json")).expect("json");
    document["format_version"] = 11.into();
    let v11 = serde_json::to_string_pretty(&document).expect("encode v11");
    assert!(
        !v11.contains("\"sent\""),
        "a v11 document has no sent marker"
    );
    fs::write(dir.join("tsk.json"), &v11).expect("install v11 document");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load v11");
    assert_eq!(loaded.format_version(), 6);
    let replies: Vec<_> = loaded
        .tasks()
        .iter()
        .filter_map(|task| task.block.as_ref())
        .flat_map(|block| block.replies.iter())
        .collect();
    assert!(!replies.is_empty(), "the fixture holds a reply");
    assert!(replies.iter().all(|reply| !reply.sent));
    let block = loaded
        .tasks()
        .iter()
        .find_map(|task| {
            task.block
                .as_ref()
                .filter(|block| !block.replies.is_empty())
        })
        .expect("open block with replies");
    let owner_replies = block
        .replies
        .iter()
        .filter(|reply| reply.is_owner() && !reply.deleted)
        .count();
    assert_eq!(block.owner_entries().len(), owner_replies);

    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read_to_string(dir.join("tsk.json.v11")).expect("read version backup"),
        v11,
        "the v11 document is backed up byte-identically"
    );
    let saved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(dir.join("tsk.json")).expect("read migrated live document"),
    )
    .expect("json");
    let mut expected = document;
    expected["format_version"] = 6.into();
    with_complete_previous_open(&mut expected);
    assert_eq!(
        saved, expected,
        "nothing but the version and each done's undo status change"
    );
}

/// A v12 done's undo entry records no previous status: it migrates as `open`, so undo goes
/// back to the inbox as it did before, and the first save writes the canonical v6 wire after
/// backing up the original byte-identically.
#[test]
fn literal_dev_v12_fixture_opens_as_v6_and_its_done_undoes_to_open() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v12 = include_bytes!("fixtures/current_store_v12.json");
    assert!(
        !String::from_utf8_lossy(v12).contains("\"previous\""),
        "a v12 done's undo entry has no previous status"
    );
    fs::write(dir.join("tsk.json"), v12).expect("install current v12 fixture");
    let store = TaskStore::new(&dir);

    let loaded = store.load().expect("load current v12 fixture");
    assert_eq!(loaded.format_version(), 6);
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v12")).expect("read version backup"),
        v12.as_slice(),
        "the v12 document is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v12 state must serialize as canonical platform v6"
    );

    let mut reloaded = store.load().expect("reload v6");
    let id = reloaded.tasks()[0].id;
    assert_eq!(
        reloaded.get(id).expect("fixture task").status,
        HumanStatus::Done
    );
    reloaded.undo().expect("undo the migrated done");
    assert_eq!(
        reloaded.get(id).expect("fixture task").status,
        HumanStatus::Open,
        "a pre-v13 done undoes to open"
    );
}

/// A v13 assignee and its assignment undo carry no kind: both read as an agent profile, the
/// only kind there was, so a migrated task still dispatches. The first save writes v6 with no
/// kind keys at all, and a person assignee round-trips with its kind.
#[test]
fn v13_assignees_migrate_as_agents_and_a_person_round_trips() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let mut v13: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/current_store_v13.json")).expect("v13 json");
    v13["tasks"][0]["assignee"] = serde_json::json!("claude");
    v13["undo_stack"] = serde_json::json!([{
        "assign": {
            "id": "11111111-1111-4111-8111-111111111111",
            "previous": "pi",
            "expected_revision": "22222222-2222-4222-8222-222222222222"
        }
    }]);
    fs::write(
        dir.join("tsk.json"),
        serde_json::to_vec_pretty(&v13).expect("encode"),
    )
    .expect("install v13 document");
    let store = TaskStore::new(&dir);

    let mut loaded = store.load().expect("load v13");
    let id = loaded.tasks()[0].id;
    assert_eq!(
        loaded.get(id).expect("task").assignee_kind,
        tsk_tui::domain::AssigneeKind::Agent
    );
    assert_eq!(
        loaded.get(id).expect("task").agent_assignee(),
        Some("claude")
    );
    store.save(&loaded).expect("save v6");
    let live = fs::read_to_string(dir.join("tsk.json")).expect("read live");
    assert!(!live.contains("assignee_kind"), "{live}");
    assert!(!live.contains("previous_kind"), "{live}");

    loaded.undo().expect("undo the migrated assignment");
    assert_eq!(
        loaded.get(id).expect("task").assigned(),
        Some(tsk_tui::domain::Assignee::agent("pi"))
    );

    loaded
        .assign(id, Some(tsk_tui::domain::Assignee::person("dana")))
        .expect("assign a person");
    store.save(&loaded).expect("save person");
    let reloaded = store.load().expect("reload");
    assert_eq!(
        reloaded.get(id).expect("task").assigned(),
        Some(tsk_tui::domain::Assignee::person("dana"))
    );
    assert_eq!(reloaded.get(id).expect("task").agent_assignee(), None);
}

/// A v14 document hides no name: it loads as v6, backs up byte-identically, and saves as the
/// canonical v6 wire. A hidden name then round-trips, and its undo survives the reload.
#[test]
fn literal_dev_v14_fixture_opens_as_v6_and_a_hidden_name_round_trips() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let v14 = include_bytes!("fixtures/current_store_v14.json");
    fs::write(dir.join("tsk.json"), v14).expect("install v14 fixture");
    let store = TaskStore::new(&dir);

    let mut loaded = store.load().expect("load v14");
    assert_eq!(loaded.format_version(), 6);
    assert!(loaded.hidden_assignees().is_empty());
    store.save(&loaded).expect("save migrated state");
    assert_eq!(
        fs::read(dir.join("tsk.json.v14")).expect("read version backup"),
        v14.as_slice(),
        "the v14 original is backed up byte-identically"
    );
    assert_eq!(
        fs::read(dir.join("tsk.json")).expect("read migrated live document"),
        platform_v6_fixture().as_slice(),
        "a v14 state must serialize as canonical platform v6"
    );

    assert!(loaded.hide_assignee("dnaa"));
    store.save(&loaded).expect("save the hidden name");
    let live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(live["hidden_assignees"], serde_json::json!(["dnaa"]));
    let top = live["undo_stack"]
        .as_array()
        .and_then(|stack| stack.last())
        .expect("an undo entry");
    assert_eq!(top["hide_assignee"]["name"], "dnaa");

    let mut reloaded = store.load().expect("reload v6");
    assert!(reloaded.hidden_assignees().contains("dnaa"));
    reloaded.undo().expect("undo the hide");
    assert!(reloaded.hidden_assignees().is_empty());
}

/// Two writers: a hide and an unhide this process has not saved yet survive a merge with a
/// disk copy another writer saved meanwhile, and the other writer's hide is kept.
#[test]
fn reload_merge_save_keeps_hidden_names_from_both_writers() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    let id = seed
        .create(
            "Seed",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create seed task");
    store.save(&seed).expect("seed");

    let mut a = store.load().expect("stale writer A");
    let mut b = store.load().expect("writer B");
    b.hide_assignee("sam");
    store.save(&b).expect("B hides sam");

    a.hide_assignee("dana");
    store.reload_merge_save(&mut a).expect("A merge-save");
    let disk = store.load().expect("reload");
    assert_eq!(
        disk.hidden_assignees(),
        &std::collections::BTreeSet::from(["dana".to_string(), "sam".to_string()]),
        "both hides are on disk"
    );

    // A assigns sam again, which unhides it: a plain union with disk would bring it back.
    a.assign(id, Some(tsk_tui::domain::Assignee::person("sam")))
        .expect("assign sam");
    store
        .reload_merge_save(&mut a)
        .expect("A merge-save unhide");
    let disk = store.load().expect("reload");
    assert_eq!(
        disk.hidden_assignees(),
        &std::collections::BTreeSet::from(["dana".to_string()]),
        "the unhide wins over the disk copy"
    );
}

/// Assigning a removed name puts it back on the list inside the assignment's undo: one `ctrl+u`
/// unassigns and hides it again, and the next undoes the removal, across saves. Assigning a
/// name the task already has only shows it again, as its own undo entry; with the name shown
/// already, it records nothing.
#[test]
fn assigning_a_removed_name_shows_it_inside_the_assignment_undo() {
    use tsk_tui::domain::Assignee;
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let kept = state
        .create(
            "Kept",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    let fresh = state
        .create(
            "Fresh",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    state
        .assign(kept, Some(Assignee::person("dana")))
        .expect("assign dana");
    assert!(state.hide_assignee("dana"));
    store.reload_merge_save(&mut state).expect("save the hide");

    state
        .assign(fresh, Some(Assignee::person("dana")))
        .expect("assign dana again");
    assert!(state.hidden_assignees().is_empty());
    store
        .reload_merge_save(&mut state)
        .expect("save the assignment");
    let mut state = store.load().expect("reload");
    state.undo().expect("undo the assignment");
    assert_eq!(state.get(fresh).expect("task").assignee, None);
    assert!(state.hidden_assignees().contains("dana"), "hidden again");
    store.reload_merge_save(&mut state).expect("save the undo");
    let mut state = store.load().expect("reload");
    state.undo().expect("undo the removal");
    assert!(
        state.hidden_assignees().is_empty(),
        "the removal undoes next"
    );

    // The task already has the name: only the list changes, and undo hides it again.
    assert!(state.hide_assignee("dana"));
    let before = undo_top(&state);
    assert!(state
        .assign(kept, Some(Assignee::person("dana")))
        .expect("same name"));
    assert!(state.hidden_assignees().is_empty());
    state.undo().expect("undo the re-show");
    assert!(state.hidden_assignees().contains("dana"));
    assert_eq!(undo_top(&state), before, "nothing else undone");
    state.undo().expect("undo the removal");
    let top = undo_top(&state);
    assert!(!state
        .assign(kept, Some(Assignee::person("dana")))
        .expect("nothing to do"));
    assert_eq!(undo_top(&state), top, "no entry for no change");
}

/// An undone removal stays undone through a save: the disk copy of its entry is not merged
/// back, so a later `ctrl+u` has nothing to replay.
#[test]
fn an_undone_removal_is_not_merged_back_from_disk() {
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    assert!(state.hide_assignee("dnaa"));
    store.reload_merge_save(&mut state).expect("save the hide");
    state.undo().expect("undo the hide");
    store.reload_merge_save(&mut state).expect("save the undo");
    assert_eq!(undo_top(&state), serde_json::Value::Null);
    state.undo().expect("nothing to undo");
    assert!(state.hidden_assignees().is_empty());
    assert_eq!(
        undo_top(&store.load().expect("reload")),
        serde_json::Value::Null
    );
}

/// Every way of assigning a person puts a removed name back: a new task, the CLI edit, and the
/// task page's edit.
#[test]
fn every_assignment_path_shows_a_removed_name() {
    use tsk_tui::domain::Assignee;
    let dana = || Some(Assignee::person("dana"));
    let mut state = DomainState::new();
    assert!(state.hide_assignee("dana"));
    state
        .create_configured(
            "New",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
            dana(),
            None,
        )
        .expect("create");
    assert!(state.hidden_assignees().is_empty(), "create");

    let id = state
        .create(
            "Edited",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    assert!(state.hide_assignee("dana"));
    state
        .edit_with_assignee_and_base(id, "Edited", None, TaskScope::Global, None, dana(), None)
        .expect("cli edit");
    assert!(state.hidden_assignees().is_empty(), "cli edit");

    let id = state
        .create(
            "Paged",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    assert!(state.hide_assignee("dana"));
    state
        .edit_with_step_changes_and_base(
            id,
            "Paged",
            None,
            TaskScope::Global,
            None,
            dana(),
            None,
            &[],
            &[],
            &[],
            None,
        )
        .expect("page edit");
    assert!(state.hidden_assignees().is_empty(), "page edit");
}

/// Two boards: one removes a name and saves, the other (loaded before) assigns that name and
/// merge-saves. The assignment wins: the name is back on the list on disk.
#[test]
fn an_assignment_merged_after_another_writers_removal_shows_the_name() {
    use tsk_tui::domain::Assignee;
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    let id = seed
        .create(
            "Seed",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    store.save(&seed).expect("seed");

    let mut a = store.load().expect("writer A");
    let mut b = store.load().expect("writer B");
    assert!(b.hide_assignee("dana"));
    store.save(&b).expect("B hides dana");

    a.assign(id, Some(Assignee::person("dana")))
        .expect("A assigns dana");
    store.reload_merge_save(&mut a).expect("A merge-save");
    assert!(
        store.load().expect("reload").hidden_assignees().is_empty(),
        "the assignment shows the name again"
    );
}

/// The top undo entry as stored, or null.
fn undo_top(state: &DomainState) -> serde_json::Value {
    serde_json::to_value(state).expect("state serializes")["undo_stack"]
        .as_array()
        .and_then(|stack| stack.last().cloned())
        .unwrap_or_default()
}

/// Removing a name again after an assignment re-showed it keeps that assignment undoable
/// through a save: `ctrl+u` undoes the removal, then the assignment.
#[test]
fn removing_a_reshown_name_keeps_the_assignment_undoable_across_a_save() {
    use tsk_tui::domain::Assignee;
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let id = state
        .create(
            "T5",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    assert!(state.hide_assignee("dnaa"));
    store.reload_merge_save(&mut state).expect("save the hide");
    state
        .assign(id, Some(Assignee::person("dnaa")))
        .expect("assign dnaa");
    assert!(state.hide_assignee("dnaa"), "removed again");
    store.reload_merge_save(&mut state).expect("save");
    let mut state = store.load().expect("reload");

    state.undo().expect("undo the second removal");
    assert!(state.hidden_assignees().is_empty());
    assert_eq!(
        state.get(id).expect("task").assigned(),
        Some(Assignee::person("dnaa"))
    );
    state.undo().expect("undo the assignment");
    assert_eq!(state.get(id).expect("task").assignee, None, "unassigned");
}

/// Editing a task that already carries a removed name, from the task page or `tsk edit
/// --title`, leaves the name off the list: only a change to that name shows it.
#[test]
fn editing_a_task_that_keeps_a_removed_name_leaves_it_hidden() {
    use tsk_tui::domain::Assignee;
    let dnaa = || Some(Assignee::person("dnaa"));
    let mut state = DomainState::new();
    let id = state
        .create(
            "T11",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    state.assign(id, dnaa()).expect("assign");
    assert!(state.hide_assignee("dnaa"));
    state
        .edit_with_step_changes_and_base(
            id,
            "T11",
            Some("new notes".into()),
            TaskScope::Global,
            None,
            dnaa(),
            None,
            &[],
            &[],
            &[],
            None,
        )
        .expect("page edit");
    assert!(state.hidden_assignees().contains("dnaa"), "page edit");
    state
        .edit_with_assignee_and_base(
            id,
            "T11 renamed",
            None,
            TaskScope::Global,
            None,
            dnaa(),
            None,
        )
        .expect("cli edit");
    assert!(state.hidden_assignees().contains("dnaa"), "cli edit");
}

/// Another writer removing the name while this one's assignment re-showed it keeps that
/// assignment's undo on disk: its stale name step does not keep the merge from taking it.
#[test]
fn a_concurrent_removal_keeps_the_other_writers_assignment_undo() {
    use tsk_tui::domain::Assignee;
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut seed = DomainState::new();
    let id = seed
        .create(
            "T5",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create");
    assert!(seed.hide_assignee("dnaa"));
    store.save(&seed).expect("seed");

    let mut a = store.load().expect("writer A");
    let mut b = store.load().expect("writer B");
    a.assign(id, Some(Assignee::person("dnaa")))
        .expect("A assigns dnaa");
    store.reload_merge_save(&mut a).expect("A saves");
    b.undo().expect("B undoes the seed hide");
    assert!(b.hide_assignee("dnaa"), "B removes it again");
    store.reload_merge_save(&mut b).expect("B merge-saves");

    let mut disk = store.load().expect("reload");
    disk.undo().expect("undo B's removal");
    disk.undo().expect("undo A's assignment");
    assert_eq!(
        disk.get(id).expect("task").assignee,
        None,
        "A's assignment undone"
    );
}

/// A store a dispatch dev build wrote at v15 (an agent and a person assignee, a hidden name,
/// an `after` link, a review round with a sent reply, a batch done on the undo stack) opens as
/// v6 unchanged. Its first save writes v6, keeps the original as `tsk.json.v15`, and the batch
/// still undoes as one.
#[test]
fn a_populated_dev_v15_store_opens_as_v6_and_keeps_tsk_json_v15() {
    use tsk_tui::domain::{Assignee, BlockOn, ReviewDraft, OWNER};
    let dir = temp_state_dir();
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    let mut create = |title: &str| {
        state
            .create(
                title,
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create")
    };
    let (first, waiting, reviewed, done_a, done_b) = (
        create("prerequisite"),
        create("waiting"),
        create("reviewed"),
        create("done a"),
        create("done b"),
    );
    store.reload_merge_save(&mut state).expect("number them");
    let number = state.get(first).and_then(|task| task.number).expect("T1");
    state.set_after(waiting, &[number]).expect("after");
    state
        .assign(waiting, Some(Assignee::person("dana")))
        .expect("assign a person");
    state
        .assign(reviewed, Some(Assignee::agent("claude")))
        .expect("assign an agent");
    let draft = ReviewDraft::from_input(
        Some("built it"),
        &["tests pass".into()],
        Some("merge"),
        BlockOn::Agent("pi".into()),
    )
    .expect("draft");
    state.review(reviewed, draft, "claude").expect("review");
    let reply = state.reply(reviewed, "looks good", OWNER).expect("reply");
    let record = state.get(reviewed).unwrap().block.as_ref().unwrap().key();
    assert!(state
        .mark_reply_sent(reviewed, &record, reply, "looks good")
        .expect("mark sent"));
    assert!(state.hide_assignee("dana"));
    state.complete_batch(&[done_a, done_b]).expect("batch done");
    store.save(&state).expect("save");
    let as_v6 = store.load().expect("load the v6 save");

    let mut document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read"))
            .expect("json");
    document["format_version"] = 15.into();
    let v15 = serde_json::to_vec_pretty(&document).expect("encode v15");
    let text = String::from_utf8_lossy(&v15);
    for key in [
        "\"assignee_kind\"",
        "\"hidden_assignees\"",
        "\"after\"",
        "\"sent\": true",
        "\"batch\"",
        "\"review\"",
    ] {
        assert!(text.contains(key), "the v15 document carries {key}");
    }
    fs::write(dir.join("tsk.json"), &v15).expect("install dev v15");

    let mut loaded = store.load().expect("a dev v15 store opens");
    assert_eq!(loaded.format_version(), 6);
    assert_eq!(loaded.tasks(), as_v6.tasks(), "opens with no transform");
    assert!(loaded.hidden_assignees().contains("dana"));
    store.save(&loaded).expect("first save");
    assert_eq!(
        fs::read(dir.join("tsk.json.v15")).expect("read version copy"),
        v15,
        "the dev original is kept byte-identically"
    );
    let mut live: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("tsk.json")).expect("read live"))
            .expect("json");
    assert_eq!(live["format_version"], 6);
    live["format_version"] = 15.into();
    assert_eq!(live, document, "nothing but the version changes");

    loaded.undo().expect("undo the batch");
    for id in [done_a, done_b] {
        assert_ne!(loaded.get(id).expect("task").status, HumanStatus::Done);
    }
}

/// Every dev version opens as v6 and keeps its own copy; the first version past the dev range
/// is refused without a write.
#[test]
fn dev_v7_and_v11_open_as_v6_and_v16_is_refused() {
    for version in [7, 11] {
        let dir = temp_state_dir();
        let _guard = TempDirGuard(dir.clone());
        let mut document: serde_json::Value =
            serde_json::from_slice(&platform_v6_fixture()).expect("json");
        document["format_version"] = version.into();
        let bytes = serde_json::to_vec_pretty(&document).expect("encode");
        fs::write(dir.join("tsk.json"), &bytes).expect("install");
        let store = TaskStore::new(&dir);
        let loaded = store.load().expect("a dev store opens");
        assert_eq!(loaded.format_version(), 6);
        store.save(&loaded).expect("save");
        assert_eq!(
            fs::read(dir.join(format!("tsk.json.v{version}"))).expect("version copy"),
            bytes
        );
        assert_eq!(
            fs::read(dir.join("tsk.json")).expect("live"),
            platform_v6_fixture()
        );
    }
    let mut document: serde_json::Value =
        serde_json::from_slice(&platform_v6_fixture()).expect("json");
    document["format_version"] = 16.into();
    assert_store_document_refused_without_rewrite(document);
}
