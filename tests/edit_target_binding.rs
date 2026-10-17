//! A concurrent store writer must not redirect a field edit that is still open.
//!
//! Proved the binding, the refusal, and the selection clamp at the unit level by
//! mutating `BoardModel` directly (a swap, a direct `sync_from_domain` call). This is the
//! end-to-end proof: it merges a `TaskStore` whose on-disk snapshot a second writer changes
//! while the edit sits open, the same shape as the idle loop's store revalidation, then
//! confirms and checks the write landed on the bound task and nowhere else.
//!
//! The two cases below are not interchangeable and do not carry equal weight against a
//! binding-vs-selection regression.'s `clamp_selection` glues `selected_id()` to
//! `edit_target` for as long as the bound task stays visible, unconditionally. The
//! added-ahead case (below) never removes the bound task from view, so `selected_id()` and
//! `edit_target` are provably identical at the moment `ConfirmEdit` fires no matter which one
//! `confirm_edit` reads; it cannot fail on that regression (checked: it still passes with
//! `confirm_edit` reverted to read `model.selected_id()`). Only the removed-task case, where
//! the bound task itself drops out of the visible list and the clamp falls back to whatever
//! took its old slot, can actually separate the two reads, and it is the one that fails under
//! that revert. That asymmetry is a consequence of the clamp design, not a gap in this
//! test: there is no way to construct an "added-ahead, bound task stays visible" scenario where
//! the two targets diverge.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::app::{
    apply_board_intent_with_save_recovery, confirm_edit_refusal_against_the_record,
    refresh_before_mutation, revalidate_board_from_store, BoardSaveContext, StoreWatch,
};
use tsk_tui::domain::{DomainError, DomainState, ProvenanceOrigin, TaskScope};
use tsk_tui::save_recovery::SaveRecovery;
use tsk_tui::store::TaskStore;
use tsk_tui::ui::board::{apply_intent, BoardInputMode, BoardModel, IntentOutcome};
use tsk_tui::ui::capture::CaptureField;
use tsk_tui::ui::input::{map_key, BoardIntent};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const THIS_REPO: &str = "/repos/app";

fn scope() -> TaskScope {
    TaskScope::Project {
        path: THIS_REPO.into(),
    }
}

fn temp_state_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-edit-target-binding-{tag}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Drive the production idle-merge path against a concurrent store write.
fn merge_disk_into_board(store: &TaskStore, domain: &mut DomainState, model: &mut BoardModel) {
    let mut watch = StoreWatch::new();
    let recovery = SaveRecovery::new();
    assert!(
        revalidate_board_from_store(store, domain, model, &mut watch, &recovery),
        "concurrent store write must merge through revalidate_board_from_store"
    );
}

/// case 1: a task **added** to the visible order ahead of the bound task.
///
/// A brand-new task can never land ahead of one this board already knows about: the real merge
/// (`DomainState::merge_tasks_from_disk`) only appends tasks it has not seen before, always after
/// whatever is already local. The genuine way a poll puts something ahead of an open edit is a
/// task this board already holds, currently filtered out, that a second writer brings back into
/// view: created before the bound task, so it resumes that earlier position the moment it is
/// visible again. Here Zebra is created (and completed, so Open hides it) before Alpha, the bound
/// task; a second writer reopens Zebra while the edit sits open, and the real
/// `merge_disk_into_board` must merge that in and put Zebra ahead of Alpha in the visible list.
///
/// What this proves: the merge (`DomainState::merge_tasks_from_disk`) and the `clamp_selection`
/// "glue to the bound task" rule both hold up end to end through the disk merge, and
/// no other task's data is disturbed by the reorder.
///
/// What this does **not** prove: a binding-vs-selection regression in `confirm_edit`. Alpha
/// never leaves the visible list here, so's clamp keeps `selected_id()` glued to
/// `edit_target` throughout, and the two are identical at `ConfirmEdit` regardless of which one
/// `confirm_edit` reads. Checked directly: this test still passes with `confirm_edit` reverted
/// to read `model.selected_id()` instead of `model.edit_target`. The removed-task case below is
/// the one that guards the redirect; this case cannot substitute for it.
#[test]
fn attention_cycle_adds_a_task_ahead_of_the_bound_task_and_confirm_still_lands_on_it() {
    let dir = temp_state_dir("add-ahead");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut domain = DomainState::new();
    let zebra = domain
        .create(
            "Zebra",
            Some("z notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create zebra");
    domain.complete(zebra).expect("complete zebra");
    let alpha = domain
        .create(
            "Alpha",
            Some("a notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create alpha");
    store.save(&domain).expect("save initial snapshot");

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    assert_eq!(
        model.selected_id(),
        Some(alpha),
        "Zebra is done, so Alpha is the only visible task at open"
    );

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("begin title edit");
    assert_eq!(model.edit_buffer(), "Alpha");
    for _ in 0.."Alpha".len() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditBackspace, None)
            .expect("backspace draft");
    }
    for character in "Alpha renamed".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("insert draft");
    }

    // The second writer: another session reopens Zebra while this edit sits open, exactly the
    // kind of concurrent change the idle loop's poll must merge without disturbing the edit.
    let mut other = store.load().expect("load for concurrent writer");
    other.reopen(zebra).expect("reopen zebra concurrently");
    store.save(&other).expect("save concurrent reopen");

    merge_disk_into_board(&store, &mut domain, &mut model);

    assert_eq!(
        model
            .visible_tasks()
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![zebra, alpha],
        "Zebra must now sit ahead of Alpha in the visible order"
    );
    assert_eq!(
        model.selected_id(),
        Some(alpha),
        "the reclamp keeps the highlight on the bound task even though it moved"
    );

    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm edit");
    assert_eq!(outcome, IntentOutcome::Persist);

    assert_eq!(domain.get(alpha).unwrap().title, "Alpha renamed");
    let zebra_task = domain.get(zebra).unwrap();
    assert_eq!(zebra_task.title, "Zebra", "Zebra's title must be untouched");
    assert_eq!(
        zebra_task.notes.as_deref(),
        Some("z notes"),
        "Zebra's notes must be untouched"
    );
    assert_eq!(zebra_task.scope, scope(), "Zebra's scope must be untouched");
}

/// case 2: a task **removed** from the visible order, specifically the bound task itself.
///
/// This is the one construction that can actually diverge `selected_id()` from the binding: the
/// selection clamp keeps `selected_id()` glued to the bound task for as long as it stays
/// visible, so any reorder that leaves the bound task on screen can never separate the two --
/// confirming through either would land in the same place. The gap only opens when the bound
/// task itself drops out of the visible list: the clamp falls back to the old numeric index,
/// which now names whatever task slid into it, not the one the edit is bound to. A second writer
/// completes Alpha (the bound task) while its notes edit sits open; Bravo slides into index 0 in
/// Alpha's place. Confirm must still land on Alpha, not on Bravo.
#[test]
fn attention_cycle_removes_the_bound_task_and_confirm_still_lands_on_it_not_the_task_that_took_its_slot(
) {
    let dir = temp_state_dir("removed");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut domain = DomainState::new();
    let alpha = domain
        .create(
            "Alpha",
            Some("a notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create alpha");
    let bravo = domain
        .create(
            "Bravo",
            Some("b notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create bravo");
    store.save(&domain).expect("save initial snapshot");

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    // Queue order is updated_at desc; pin Alpha explicitly for the bind under test.
    let visible = model.visible_ids();
    assert!(visible.contains(&alpha) && visible.contains(&bravo));
    let alpha_idx = visible
        .iter()
        .position(|&id| id == alpha)
        .expect("alpha visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(alpha_idx),
        None,
    )
    .expect("select alpha");
    assert_eq!(model.selected_id(), Some(alpha));

    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditNotes, None)
        .expect("begin notes edit");
    assert_eq!(model.edit_buffer(), "a notes");
    for _ in 0.."a notes".len() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditBackspace, None)
            .expect("backspace draft");
    }
    for character in "a notes updated".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("insert draft");
    }

    // The second writer: another session completes Alpha while this notes edit sits open.
    // Alpha stays on disk (not soft-deleted), just out of the Open lens this board is on.
    let mut other = store.load().expect("load for concurrent writer");
    other.complete(alpha).expect("complete alpha concurrently");
    store.save(&other).expect("save concurrent completion");

    merge_disk_into_board(&store, &mut domain, &mut model);

    assert_eq!(
        model
            .visible_tasks()
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![bravo],
        "Alpha must have left the visible list"
    );
    assert_eq!(
        model.selected_id(),
        Some(bravo),
        "the numeric clamp fell back onto whatever now sits at Alpha's old index -- proof the \
         divergence this test needs is real, not that confirm already agrees with the clamp"
    );

    let outcome = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("confirm edit");
    assert_eq!(outcome, IntentOutcome::Persist);

    let alpha_task = domain.get(alpha).unwrap();
    assert_eq!(
        alpha_task.notes.as_deref(),
        Some("a notes updated"),
        "the draft must land on Alpha, the bound task, even though it is no longer visible"
    );
    assert_eq!(alpha_task.title, "Alpha", "Alpha's title must be untouched");
    assert_eq!(alpha_task.scope, scope(), "Alpha's scope must be untouched");

    let bravo_task = domain.get(bravo).unwrap();
    assert_eq!(
        bravo_task.title, "Bravo",
        "Bravo must not receive the draft meant for Alpha"
    );
    assert_eq!(
        bravo_task.notes.as_deref(),
        Some("b notes"),
        "Bravo's notes must be untouched"
    );
    assert_eq!(bravo_task.scope, scope(), "Bravo's scope must be untouched");
}

/// Shared setup for the two cross-actor cases below: a store holding Alpha (the task the edit
/// binds to) and Bravo (the bystander that must survive both cases untouched), a board with a
/// Title edit open on Alpha carrying the draft `Alpha RENAMED`, and the cursor parked off the end
/// so "unchanged" is a real claim.
///
/// Returns the store, the board's domain and model, the two ids, and the draft and cursor as they
/// stood the moment the edit was left open.
#[allow(clippy::type_complexity)]
fn board_with_an_open_edit_and_a_bystander(
    tag: &str,
) -> (
    TempDirGuard,
    TaskStore,
    DomainState,
    BoardModel,
    uuid::Uuid,
    uuid::Uuid,
    String,
    usize,
) {
    let dir = temp_state_dir(tag);
    // The guard rides back out with the fixture: dropping it here would delete the store the
    // caller is about to use.
    let guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    // The board's own state: Alpha is the task the edit will bind to, Bravo the bystander that
    // must come through both the refusal and the later success untouched.
    let mut domain = DomainState::new();
    let alpha = domain
        .create(
            "Alpha",
            Some("a notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create Alpha");
    let bravo = domain
        .create(
            "Bravo",
            Some("b notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create Bravo");
    store.save(&domain).expect("seed the store");

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    // Queue order is updated_at desc within ON DECK; select Alpha by id, not by open seed.
    let visible = model.visible_ids();
    let alpha_idx = visible
        .iter()
        .position(|&id| id == alpha)
        .expect("alpha visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(alpha_idx),
        None,
    )
    .expect("select alpha");
    assert_eq!(model.selected_id(), Some(alpha), "Alpha must be selected");

    // Open a Title edit on Alpha and type a draft, through the real intent path.
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open the title edit");
    for character in " RENAMED".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type into the draft");
    }
    // Leave the cursor off the end so "unchanged" is a real claim after the refusal.
    apply_intent(&mut domain, &mut model, BoardIntent::EditMoveLeft, None)
        .expect("move the cursor");
    let draft_before = model.edit_buffer().to_string();
    let cursor_before = model.edit_cursor();
    assert_eq!(draft_before, "Alpha RENAMED");

    // A second actor soft-deletes Alpha and persists it. No disk merge runs: this board's
    // in-memory copy of Alpha is still alive and still says `soft_deleted == false`.
    let mut other_actor = store.load().expect("second actor loads the store");
    other_actor
        .soft_delete(alpha)
        .expect("second actor deletes");
    store
        .reload_merge_save(&mut other_actor)
        .expect("second actor persists the delete");
    assert!(
        !domain.get(alpha).expect("Alpha still local").soft_deleted,
        "precondition: this board must not already know about the delete, \
         or the test proves nothing about freshness"
    );

    (
        guard,
        store,
        domain,
        model,
        alpha,
        bravo,
        draft_before,
        cursor_before,
    )
}

/// the availability of the bound task is judged against the **fresh durable state**, not
/// against whatever the last store revalidation happened to leave in memory.
///
/// Resolved decision 8 (2026-07-27): a soft-deleted task is not editable, and staleness is not an
/// excuse for editing one. The hazard here is narrower than the poll-driven one above and the poll
/// cannot close it: a second actor soft-deletes the bound task and **persists** it, and the user
/// confirms **before any poll runs**. Nothing in memory knows about the delete yet, so without the
/// pre-mutation refresh the confirm sails past `confirm_edit`'s soft-delete guard and writes to a
/// task that is deleted on disk.
///
/// The two production steps under test are the two `handle_board_intent` performs, in its order:
/// [`refresh_before_mutation`] against the freshly loaded baseline, then
/// [`apply_board_intent_with_save_recovery`] over the same intent. The only glue not exercised
/// here is the terminal-bound event loop that calls them, which no integration test can reach.
///
/// Falsified: with `ConfirmEdit` removed from `board_intent_needs_fresh_state` and nothing else
/// changed, this fails on the refusal assertion below rather than compiling out.
#[test]
fn a_persisted_cross_actor_soft_delete_refuses_the_confirm_and_keeps_the_draft() {
    let (_guard, store, mut domain, mut model, alpha, bravo, draft_before, cursor_before) =
        board_with_an_open_edit_and_a_bystander("cross-actor-soft-delete");

    // ---: confirm, exactly as the board loop does it. -------------------------------
    let refused = confirm_through_the_board_loop(&store, &mut domain, &mut model);
    assert_eq!(
        refused,
        Err(DomainError::SoftDeleted(alpha)),
        "the confirm must refuse against the fresh durable state"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditTitle,
        "the edit must stay open through the refusal"
    );
    assert_eq!(
        model.edit_buffer(),
        draft_before,
        "the draft must survive the refusal byte for byte"
    );
    assert_eq!(
        model.edit_cursor(),
        cursor_before,
        "the cursor must not jump on the refusal"
    );

    let alpha_after_refusal = domain.get(alpha).expect("Alpha is still a record");
    assert_eq!(
        alpha_after_refusal.title, "Alpha",
        "a refused edit must not write the draft"
    );
    assert!(
        !alpha_after_refusal.soft_deleted,
        "the verdict must READ the record, not merge it: merging here would hand \
         merge_for_save a base this board never earned and silently overwrite a concurrent \
         same-task edit (see a_concurrent_edit_to_the_bound_task_still_reaches_the_save_conflict)"
    );
    let on_disk = store.load().expect("reload after the refusal");
    assert_eq!(
        on_disk.get(alpha).expect("Alpha on disk").title,
        "Alpha",
        "a refused edit must not reach the durable record either"
    );
    assert_eq!(
        on_disk.get(bravo).expect("Bravo on disk").title,
        "Bravo",
        "the bystander must be untouched by the refusal"
    );
}

/// a task the deletion refused becomes editable again once it is **explicitly restored**,
/// through the edit session that was still open.
///
/// Decision 8 puts the restore in the user's hands: the refusal is not a dead end and it is not
/// self-healing either. This drives the whole arc the criterion describes — refuse, restore,
/// confirm — because "a restored task confirms normally" is only meaningful about a session that
/// has already been refused once. Re-running the refusal here rather than sharing state with the
/// case above keeps each test a complete story that fails on its own terms.
#[test]
fn a_restored_task_accepts_the_draft_the_deletion_refused() {
    let (_guard, store, mut domain, mut model, alpha, bravo, draft_before, cursor_before) =
        board_with_an_open_edit_and_a_bystander("cross-actor-restore");

    // The refusal covers, replayed here so the restore acts on a session that has
    // genuinely been refused rather than on a fresh one.
    let refused = confirm_through_the_board_loop(&store, &mut domain, &mut model);
    assert_eq!(
        refused,
        Err(DomainError::SoftDeleted(alpha)),
        "precondition: the delete must refuse before the restore is meaningful"
    );
    assert_eq!(
        model.edit_buffer(),
        draft_before,
        "the draft survives the refusal"
    );
    assert_eq!(
        model.edit_cursor(),
        cursor_before,
        "the cursor survives it too"
    );

    let mut restorer = store.load().expect("second actor loads again");
    restorer
        .restore(alpha)
        .expect("second actor restores Alpha");
    store
        .reload_merge_save(&mut restorer)
        .expect("second actor persists the restore");

    // The restore is refused no longer — check that first, on the still-stale board, because it
    // is the half decision 8 owns: the deleted verdict is gone the moment the record says the
    // task is back, with no poll and no reopen needed.
    assert_eq!(
        confirm_edit_refusal_against_the_record(&store.load().expect("load the record"), &model),
        None,
        "an explicitly restored task must stop being refused as deleted, immediately"
    );

    // Then the board catches up the way it always does, through its own idle poll, and the
    // confirm writes. The poll is what refreshes local revisions; decision 8 deliberately does
    // not, because merging here would defeat same-task conflict detection (see
    // `a_concurrent_edit_to_the_bound_task_still_reaches_the_save_conflict`). Without the poll
    // this board's copy is older than the delete-and-restore round trip left on disk, and the
    // confirm collides at `merge_for_save` exactly as it did before — pre-existing behavior
    // this feature neither introduces nor removes.
    merge_disk_into_board(&store, &mut domain, &mut model);
    assert_eq!(
        model.edit_buffer(),
        draft_before,
        "the poll must not disturb the open session it runs underneath"
    );

    let accepted = confirm_through_the_board_loop(&store, &mut domain, &mut model);
    assert_eq!(
        accepted,
        Ok(IntentOutcome::Persisted),
        "a restored task must confirm normally, with no second refusal and no reopen"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "a successful confirm must return to the saved task page"
    );

    let alpha_final = store
        .load()
        .expect("reload after the success")
        .get(alpha)
        .cloned()
        .expect("Alpha on disk");
    assert_eq!(
        alpha_final.title, "Alpha RENAMED",
        "the draft the deletion refused must land once the task is restored"
    );
    assert!(
        !alpha_final.soft_deleted,
        "Alpha must be restored, not resurrected by the edit"
    );
    assert_eq!(
        alpha_final.notes.as_deref(),
        Some("a notes"),
        "a title edit must not disturb the notes"
    );
    assert_eq!(
        alpha_final.scope,
        scope(),
        "Alpha's scope must be untouched"
    );

    let bravo_final = store
        .load()
        .expect("reload after the success")
        .get(bravo)
        .cloned()
        .expect("Bravo on disk");
    assert_eq!(bravo_final.title, "Bravo", "the bystander's title");
    assert_eq!(
        bravo_final.notes.as_deref(),
        Some("b notes"),
        "the bystander's notes"
    );
    assert_eq!(bravo_final.scope, scope(), "the bystander's scope");
}

/// One confirmation, driven through the two production steps `handle_board_intent` performs in
/// its own order: load the durable baseline, [`refresh_before_mutation`] it into local state, then
/// apply the intent through [`apply_board_intent_with_save_recovery`] with that same baseline.
///
/// Returning the typed result rather than asserting inside keeps both call sites above readable,
/// and keeps the refusal and the success on exactly the same path — if the two diverged, the
/// half would no longer be evidence about the path.
fn confirm_through_the_board_loop(
    store: &TaskStore,
    domain: &mut DomainState,
    model: &mut BoardModel,
) -> Result<IntentOutcome, DomainError> {
    let baseline = store.load().expect("load the durable baseline");
    refresh_before_mutation(&BoardIntent::ConfirmEdit, &baseline, domain, model);

    let mut recovery = SaveRecovery::new();
    apply_board_intent_with_save_recovery(
        domain,
        model,
        &mut recovery,
        BoardSaveContext {
            baseline,
            intent: BoardIntent::ConfirmEdit,
            snapshot: None,
        },
        |state| {
            store
                .reload_merge_save(state)
                .map_err(|error| error.to_string())
        },
    )
}

/// The guard on the *other* half of decision 8: consulting the durable record must not become
/// merging from it.
///
/// This is a regression test with a real history. The first implementation of added
/// `ConfirmEdit` to the pre-mutation **merge** set, which reads as the natural way to "judge
/// against fresh durable state". It also silently broke same-task conflict detection:
/// `merge_tasks_from_disk` replaces the local task wholesale, so the `edit` that follows records
/// a merge base taken from the disk revision, `merge_for_save` finds a base that matches, and the
/// other actor's concurrent edit is overwritten with no conflict and no save recovery. Measured
/// before and after: with the merge, this scenario ended `Ok(Persisted)` with the other actor's
/// title gone; without it, the confirm does not persist and the durable title survives.
///
/// So the availability verdict reads the record and never merges it, and this test fails if that
/// ever regresses — which no other test would catch, because every and assertion is
/// equally satisfied by the broken version.
#[test]
fn a_concurrent_edit_to_the_bound_task_still_reaches_the_save_conflict() {
    let (_guard, store, mut domain, mut model, alpha, _bravo, draft_before, _cursor) =
        board_with_an_open_edit_and_a_bystander("concurrent-same-task-edit");

    // The fixture stages a delete; this case is about a concurrent *edit*, so put Alpha back.
    let mut restorer = store.load().expect("load to restore");
    restorer.restore(alpha).expect("restore Alpha");
    store
        .reload_merge_save(&mut restorer)
        .expect("persist the restore");

    // A second actor edits the same task's title and persists it, while this board's edit sits
    // open on its own stale copy.
    let mut other_actor = store.load().expect("second actor loads");
    let current = other_actor.get(alpha).cloned().expect("Alpha on disk");
    other_actor
        .edit(
            alpha,
            "Alpha from the other actor",
            current.notes.clone(),
            current.scope.clone(),
            current.thread.clone(),
        )
        .expect("second actor edits the same task");
    store
        .reload_merge_save(&mut other_actor)
        .expect("second actor persists its edit");

    let outcome = confirm_through_the_board_loop(&store, &mut domain, &mut model);

    assert_ne!(
        outcome,
        Ok(IntentOutcome::Persisted),
        "a concurrent same-task edit must not be silently overwritten by this confirm"
    );
    let on_disk = store.load().expect("reload after the confirm");
    assert_eq!(
        on_disk.get(alpha).expect("Alpha on disk").title,
        "Alpha from the other actor",
        "the other actor's edit must survive: the collision belongs to merge_for_save, and \
         consulting the record must not have handed it a merge base this board never earned"
    );
    assert_ne!(
        on_disk.get(alpha).expect("Alpha on disk").title,
        draft_before,
        "this board's draft must not have landed"
    );
}

/// N3 /: the same soft-delete-then-restore arc [`a_persisted_cross_actor_soft_delete_refuses_the_confirm_and_keeps_the_draft`]
/// and [`a_restored_task_accepts_the_draft_the_deletion_refused`] already prove, but opened
/// through the actual new-surface entry point instead of a hand-built `BoardIntent`.
///
/// Every case above opens its edit by constructing `BoardIntent::BeginEditTitle` /
/// `BeginEditNotes` directly and passing it to `apply_intent`. That proves the reducer and the
/// app-loop boundary honor the bind; it does not prove the bind survives the layer this board's
/// own key map sits in front of the reducer -- `map_key`, the same translation the terminal loop
/// and the queue board's `e` binding go through before a keypress ever becomes a `BoardIntent`.
/// This test opens and types through `map_key` end to end, then drives the refusal and the
/// eventual success through [`confirm_through_the_board_loop`] -- [`refresh_before_mutation`]
/// then [`apply_board_intent_with_save_recovery`], the same two steps `handle_board_intent`
/// performs in `src/app.rs` -- so's bound-task guarantee is proven from the real key
/// entry point through to the durable write, not just from an intent constructed mid-air.
///
/// A real `merge_disk_into_board` now runs between the persisted soft delete and the refusal, so
/// `selected_id` genuinely clamps onto Bravo while `edit_target` still names Alpha at the moment
/// this test asserts the refusal names Alpha, not whatever the clamp resolves to -- a real
/// falsifier for that half. It is not one for the eventual *success* half: `reanchor_selection`
/// re-pins `selected_id` onto `edit_target` the instant the bound task is visible again
/// (`src/ui/board.rs`), and nothing clears `confirm_edit`'s own soft-deleted guard on the local
/// `domain` without a poll that performs exactly that re-pin in the same call -- there is no
/// production sequence that merges the restore into `domain` without also reanchoring the
/// selection onto Alpha. So once Alpha is restored and visible, `selected_id` and `edit_target`
/// are equal by construction, not by coincidence, and this test's second confirm cannot exercise
/// a divergent read no matter which field `confirm_edit` consults. Checked by mutating
/// `src/ui/board.rs`'s `model.edit_target` to `model.selected_id()` in a scratch copy of the
/// tree: this test still passes (the refusal-arm divergence is real but short-circuits before
/// reaching the mutated line, and the restore-arm never diverges at all); only
/// [`attention_cycle_removes_the_bound_task_and_confirm_still_lands_on_it_not_the_task_that_took_its_slot`]
/// fails, because it is the one shape where the bound task stays permanently out of view instead
/// of coming back, so the pin-back above never fires and a successful confirm can actually read
/// the wrong id. That test is the falsifier for the redirect on a successful confirm; this one is
/// the falsifier for the redirect on a refusal, both exercised through the real key map's typed
/// path in the cases above it.
///
/// A bind naming a task this board's domain has genuinely never held (the third leg the review
/// named) cannot be constructed from outside this crate: `edit_target` has no public writer by
/// design (`BoardModel::edit_target` is a private field with a read-only accessor, and "Open is
/// still the only writer" is a documented invariant at its definition), and no legitimate write
/// path ever drops a task this board already created -- `reload_merge_save` only adds tasks it
/// has not seen, it never removes ones it has (`reload_merge_save_keeps_disk_and_local_tasks`,
/// `src/store.rs`). `confirm_edit_refuses_when_the_bound_task_is_gone` in `src/ui/board.rs`
/// already covers that shape at the unit level, by writing `model.edit_target` directly from
/// inside the module; there is no equivalent black-box construction for this file to add.
#[test]
fn a_soft_deleted_bind_opened_through_the_real_key_map_refuses_then_confirms_after_restore() {
    let dir = temp_state_dir("new-surface-entry");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut domain = DomainState::new();
    let alpha = domain
        .create(
            "Alpha",
            Some("a notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create alpha");
    let bravo = domain
        .create(
            "Bravo",
            Some("b notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create bravo");
    store.save(&domain).expect("seed the store");

    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let visible = model.visible_ids();
    let alpha_idx = visible
        .iter()
        .position(|&id| id == alpha)
        .expect("alpha visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(alpha_idx),
        None,
    )
    .expect("select alpha");
    assert_eq!(model.selected_id(), Some(alpha));

    // The actual new-surface entry: the `e` key through `map_key`, exactly as the terminal
    // loop and the queue board's key map route it, not a hand-built `BoardIntent`.
    let open_title = map_key(
        BoardInputMode::Normal,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
    )
    .expect("e opens a title edit");
    assert_eq!(open_title, BoardIntent::BeginEditTitle);
    apply_intent(&mut domain, &mut model, open_title, None).expect("open the title edit");
    assert_eq!(model.edit_buffer(), "Alpha");

    for character in " RENAMED".chars() {
        let typed = map_key(
            BoardInputMode::EditTitle,
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
        )
        .expect("typing maps to an edit intent");
        apply_intent(&mut domain, &mut model, typed, None).expect("type into the draft");
    }
    let draft_before = model.edit_buffer().to_string();
    assert_eq!(draft_before, "Alpha RENAMED");

    // A second actor soft-deletes the bound task and persists it before any poll runs.
    let mut other_actor = store.load().expect("second actor loads the store");
    other_actor
        .soft_delete(alpha)
        .expect("second actor deletes");
    store
        .reload_merge_save(&mut other_actor)
        .expect("second actor persists the delete");

    // A poll runs before the confirm, exactly as the terminal loop's background refresh would.
    // This must move `selected_id()` off Alpha (the numeric clamp lands on Bravo, the only task
    // left) while `edit_target` -- the bind -- still names Alpha, so the assertions below are a
    // real oracle on which one confirm actually reads.
    merge_disk_into_board(&store, &mut domain, &mut model);
    assert_eq!(
        model
            .visible_tasks()
            .iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![bravo],
        "Alpha must have left the visible list once the poll picks up the persisted soft delete"
    );
    assert_eq!(
        model.selected_id(),
        Some(bravo),
        "the numeric clamp must move off Alpha's old slot onto Bravo -- proof selected_id() has \
         actually diverged from the bind, not merely that confirm already agrees with it"
    );
    assert_eq!(
        model.edit_target(),
        Some(alpha),
        "the bind itself must still name Alpha; only the selection may move under the open edit"
    );

    let confirm = map_key(
        BoardInputMode::EditTitle,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
    )
    .expect("Shift+Enter confirms a title edit");
    assert_eq!(confirm, BoardIntent::ConfirmEdit);

    let refused = confirm_through_the_board_loop(&store, &mut domain, &mut model);
    assert_eq!(
        refused,
        Err(DomainError::SoftDeleted(alpha)),
        "a bind opened through the real key map must refuse against fresh durable state exactly \
         like a hand-built intent does"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::EditTitle,
        "the edit opened by the real key map must stay open through the refusal"
    );
    assert_eq!(
        model.edit_buffer(),
        draft_before,
        "the draft survives the refusal byte for byte"
    );

    // Restored by a second actor, the same bind must confirm normally -- proving the whole arc
    // (real key open -> refuse -> restore -> real key confirm) closes end to end, not just its
    // refusal half.
    //
    // A poll must run here: `confirm_edit`'s own soft-deleted guard reads the *local* `domain`,
    // not the freshly loaded baseline, and nothing except `merge_tasks_from_disk` (always paired
    // with `model.sync_from_domain`, in both `merge_disk_into_board` and `refresh_before_mutation`
    // -- there is no production path that calls one without the other) clears that flag. That
    // pairing is also why `selected_id()` cannot stay diverged from `edit_target` through this
    // second confirm: `reanchor_selection` re-pins the selection onto `edit_target` the instant
    // the bound task is visible again (`src/ui/board.rs`, the `if let Some(bound) =
    // self.edit_target` arm before the plain clamp runs), so once Alpha is back the two always
    // agree here by construction. The redirect-mutation falsifier for a *successful* confirm
    // needs the bound task to stay permanently out of view instead of coming back --
    // `attention_cycle_removes_the_bound_task_and_confirm_still_lands_on_it_not_the_task_that_took_its_slot`
    // above is that shape, at the reducer entry point.
    let mut restorer = store.load().expect("second actor loads again");
    restorer
        .restore(alpha)
        .expect("second actor restores Alpha");
    store
        .reload_merge_save(&mut restorer)
        .expect("second actor persists the restore");
    merge_disk_into_board(&store, &mut domain, &mut model);
    assert_eq!(
        model.edit_buffer(),
        draft_before,
        "the poll must not disturb the open session it runs underneath"
    );
    assert_eq!(
        model.selected_id(),
        Some(alpha),
        "reanchor re-pins the selection onto the bound task once it is visible again"
    );

    let accepted = confirm_through_the_board_loop(&store, &mut domain, &mut model);
    assert_eq!(
        accepted,
        Ok(IntentOutcome::Persisted),
        "the same bind must confirm normally once restored, with no second refusal"
    );
    assert_eq!(
        model.input_mode(),
        BoardInputMode::TaskPage,
        "a successful confirm must return to the saved task page"
    );

    let alpha_final = store
        .load()
        .expect("reload after the success")
        .get(alpha)
        .cloned()
        .expect("Alpha on disk");
    assert_eq!(
        alpha_final.title, "Alpha RENAMED",
        "the draft opened by the real key map must land"
    );
    assert!(
        !alpha_final.soft_deleted,
        "Alpha must be restored, not resurrected by the edit"
    );

    let bravo_final = store
        .load()
        .expect("reload after the success")
        .get(bravo)
        .cloned()
        .expect("Bravo on disk");
    assert_eq!(
        bravo_final.title, "Bravo",
        "the bystander must be untouched throughout"
    );
    assert_eq!(
        bravo_final.scope,
        scope(),
        "the bystander's scope must be untouched"
    );
}

/// /: a scope dropdown belongs to the same immutable task form as title and Notes.
/// A background sync may reorder the queue, but it cannot redirect the pending dropdown choice
/// or the later atomic form save to the newly selected task.
#[test]
fn board_edit_preserves_existing_thread() {
    let dir = temp_state_dir("thread-preserved");
    let _guard = TempDirGuard(dir.clone());
    let store = TaskStore::new(&dir);

    let mut seeded = DomainState::new();
    let id = seeded
        .create(
            "Threaded task",
            Some("original notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create threaded task");
    seeded
        .edit(
            id,
            "Threaded task",
            Some("original notes".into()),
            scope(),
            Some("release-2026".into()),
        )
        .expect("attach thread");
    store.save(&seeded).expect("persist threaded task");

    let mut domain = store.load().expect("load threaded task");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    let index = model
        .visible_ids()
        .iter()
        .position(|&visible_id| visible_id == id)
        .expect("threaded task visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(index),
        None,
    )
    .expect("select threaded task");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditTitle, None)
        .expect("open board title edit");
    for _ in 0.."Threaded task".len() {
        apply_intent(&mut domain, &mut model, BoardIntent::EditBackspace, None)
            .expect("clear title draft");
    }
    for character in "Renamed threaded task".chars() {
        apply_intent(
            &mut domain,
            &mut model,
            BoardIntent::EditInsert(character),
            None,
        )
        .expect("type title draft");
    }

    assert_eq!(
        apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None,)
            .expect("confirm board title edit"),
        IntentOutcome::Persist
    );
    let task = domain.get(id).expect("threaded task after board edit");
    assert_eq!(task.title, "Renamed threaded task");
    assert_eq!(task.thread.as_deref(), Some("release-2026"));
}

#[test]
fn background_sync_cannot_redirect_a_bound_task_form_while_its_scope_dropdown_is_open() {
    let mut domain = DomainState::new();
    let alpha = domain
        .create(
            "Alpha",
            Some("alpha notes".into()),
            scope(),
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create Alpha");
    let bravo = domain
        .create(
            "Bravo",
            Some("bravo notes".into()),
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create Bravo");
    let mut model = BoardModel::from_domain(&domain, Some(PathBuf::from(THIS_REPO)));
    model.set_selected_project(Some(PathBuf::from(THIS_REPO)));
    let alpha_index = model
        .visible_ids()
        .iter()
        .position(|&id| id == alpha)
        .expect("Alpha visible");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::SelectIndex(alpha_index),
        None,
    )
    .expect("select Alpha");
    apply_intent(&mut domain, &mut model, BoardIntent::BeginEditScope, None)
        .expect("open Alpha form at Scope");
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::OpenFormDropdown(CaptureField::Scope),
        None,
    )
    .expect("open scope dropdown");
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);
    assert_eq!(model.edit_target(), Some(alpha));

    // The same refresh shape the idle loop uses: Bravo moves ahead in queue order.
    domain
        .set_status(bravo, tsk_tui::domain::HumanStatus::Started)
        .expect("move Bravo");
    model.sync_from_domain(&domain);
    assert_eq!(model.edit_target(), Some(alpha));
    assert_eq!(model.input_mode(), BoardInputMode::FormDropdown);

    for _ in 0..model.form_scope_options().len() {
        if model.form_scope_dropdown_choice() == Some(&TaskScope::Global) {
            break;
        }
        apply_intent(&mut domain, &mut model, BoardIntent::FormDropdownNext, None)
            .expect("move scope selection");
    }
    apply_intent(
        &mut domain,
        &mut model,
        BoardIntent::ConfirmFormDropdown,
        None,
    )
    .expect("apply scope selection");
    let saved = apply_intent(&mut domain, &mut model, BoardIntent::ConfirmEdit, None)
        .expect("save bound task form");
    assert_eq!(saved, IntentOutcome::Persist);
    assert_eq!(domain.get(alpha).expect("Alpha").scope, TaskScope::Global);
    assert_eq!(
        domain.get(bravo).expect("Bravo").scope,
        TaskScope::Global,
        "the refreshed selection's task must not be edited"
    );
}
