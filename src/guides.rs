use std::collections::BTreeSet;

use crate::delivery;
use crate::domain::{HumanStatus, TaskScope};
use crate::store::TaskStore;

#[derive(Debug, Clone, Copy)]
pub struct Guide {
    pub catalog_id: &'static str,
    pub title: &'static str,
    pub notes: &'static str,
    pub status: HumanStatus,
    pub steps: &'static [(&'static str, bool)],
}

pub const CATALOG: [Guide; 4] = [
    Guide {
        catalog_id: "guide.welcome",
        title: "Welcome to tsk, start here",
        notes: "Hi. This is your board. Every task you make lives here. Take a minute with this one.\n\
                \n\
                **Three tabs, always**\n\
                - `1` is your desk. It shows anything that needs you or is in motion, from every project, plus the ready and open tasks that belong to no project.\n\
                - `2` is one project. Press `p` to pick it.\n\
                - `3` lists every project. `Enter` on a row opens that project's board. On a terminal 110 columns or wider, moving the cursor previews the project beside the list and `→` makes that preview live.\n\
                \n\
                **Three sections, decided by status**\n\
                - **NEEDS YOU** holds `blocked` and `review` tasks. Something is waiting on you. This task is in review, so it sits here until you act on it.\n\
                - **IN MOTION** holds `started` tasks. Work you are doing right now.\n\
                - **ON DECK** holds picked `ready` tasks first, then the expanded `inbox` of `open` tasks. New tasks land in the inbox.\n\
                - `done` tasks leave the board for the drawer. Press `d` to open it, `d` again to close it.\n\
                \n\
                **Moving around**\n\
                - `j` and `k` or the arrows move the selection. `Enter` opens a task, `Esc` comes back.\n\
                - `/` searches whatever the tab shows. `Enter` pins the filter, `Esc` clears it.\n\
                - Status keys use Ctrl, so a stray letter never changes anything. `ctrl+s` starts, `ctrl+n` picks a task for ready, `ctrl+o` sends it to the inbox, `ctrl+b` blocks, `ctrl+r` marks review or takes it back to ready, and `ctrl+d` finishes.\n\
                - `ctrl+u` undoes the last change.",
        status: HumanStatus::Review,
        steps: &[
            ("Open this task with Enter", true),
            ("Press Esc, then 1, 2, and 3 to visit each tab", false),
            ("Press ctrl+r and watch this task move to ON DECK", false),
        ],
    },
    Guide {
        catalog_id: "guide.tasks",
        title: "Make a task, open it, check off steps",
        notes: "A task holds a title, notes (the description), and steps. You set its status yourself. Optionally put it on a thread to group related work inside a project, and file it under a project (or leave it on your desk).\n\
                \n\
                **Make one**\n\
                - Press `+`, type a title, press `Enter`. It lands in the ON DECK inbox as `open`.\n\
                - Press `ctrl+n` when you pick it for the ready queue.\n\
                - Press `Tab` instead of `Enter` to open the full page first, where you can add notes, steps, a thread, and a project before you save.\n\
                - Type `!p name` in the title to file it under a project the board already knows, by its folder name. A full path to an existing folder, like `!p ~/code/widget`, adds a new one. Bare `!p` keeps it on your desk.\n\
                - Type `!t release` in the title to tag it with the thread `release`. Threads group related work inside one project, and the project board can filter by them.\n\
                \n\
                **Read one**\n\
                - `→` peeks at the notes under the task. `←` or `Esc` closes the peek.\n\
                - `Enter` opens the full page. `Esc` comes back.\n\
                - On a terminal 110 columns or wider there is no peek. `→` slides the task page open beside the board, `→` again gives it more room, and `←` slides it back.\n\
                \n\
                **Change one**\n\
                - On the page, `ctrl+e` edits the title, then Tab moves to notes. The palette can also open notes editing. `shift+enter` saves.\n\
                - `Enter` on a step checks it. The `+ step` row at the bottom adds one.\n\
                - Steps never finish a task. Check every step below and this task stays `started` until you press `ctrl+d`. You decide when work is done.",
        status: HumanStatus::Started,
        steps: &[
            ("Press Enter on this step to check it", false),
            ("Check this one too", false),
            ("Notice the task is still started", false),
            ("Press + and add a task of your own", false),
        ],
    },
    Guide {
        catalog_id: "guide.cli-agents",
        title: "The command line, and how agents use it",
        notes: "Everything on this board is also a command. Open another terminal and try these.\n\
                \n\
                ```\n\
                tsk add -t \"Try the CLI\"\n\
                tsk list\n\
                tsk status T1 start\n\
                tsk steps T1 add \"First step\"\n\
                tsk edit T1 --notes \"Written from the CLI\"\n\
                ```\n\
                \n\
                Every task gets a `T` number, and that number is how each command finds it. Use the number `tsk list` prints for your new task in place of `T1`.\n\
                \n\
                `tsk list` shows the tasks of the project you are standing in, or your desk outside a repository. Add `--all` for every project, `--open` for the inbox, `--ready` for picked tasks, or `--json` for output another program can read.\n\
                \n\
                `tsk --help` lists every command and status. `tsk help list` explains one command: its flags, examples, and exit codes.\n\
                \n\
                **Let an agent work the board**\n\
                Run `tsk setup` once: it finds the coding agents on your machine (Claude, Pi, Cursor, Codex, and others) and offers to install the tsk skill for each. `tsk setup claude` targets one. Then ask the agent in plain words. \"Add a task for each failing test.\" \"Mark T7 as review.\" `tsk guide` prints the same instructions if you want to read them yourself.\n\
                \n\
                **Back up your board**\n\
                Your tasks live on this machine only. Run `tsk backup setup`: with the GitHub CLI (`gh`) logged in, it makes a private `tsk-backup` repository and pushes a copy after every save. For another git host, `tsk backup add <git-url>`. `tsk restore` brings a copy back.",
        status: HumanStatus::Ready,
        steps: &[
            ("Run tsk add -t \"Try the CLI\" in another terminal", false),
            ("Watch it appear in the ON DECK inbox", false),
            ("Run tsk status T<n> start on it", false),
        ],
    },
    Guide {
        catalog_id: "guide.wrap-up",
        title: "That's the tour, clear these when you're ready",
        notes: "You know the tabs, the sections, and how a task moves between them. That is most of tsk.\n\
                \n\
                **Clear these starter tasks**\n\
                Any of these works. A cleared starter task never comes back.\n\
                - `ctrl+d` marks it done. Done tasks wait in the drawer. `d` opens and closes it.\n\
                - To clear several at once: `Shift+M`, then `space` or `shift+↑`/`shift+↓` to select them, then one `ctrl+d`.\n\
                - `ctrl+f` archives it. Archived tasks fold into a group at the end of the drawer.\n\
                - `ctrl+x` twice deletes it. `ctrl+u` brings it back if you change your mind.\n\
                \n\
                **When you forget a key**\n\
                Press `?`. Every key of every screen is on one card. The full guide is at https://gettsk.sh/docs\n\
                \n\
                **Say hello**\n\
                - Something broke, or something is missing? Tell us at https://github.com/smarzban/tsk/issues\n\
                - A star at https://github.com/smarzban/tsk helps other people find tsk.\n\
                - Say hi at https://x.com/smarzbanX",
        status: HumanStatus::Open,
        steps: &[
            ("Press ctrl+d on this task", false),
            ("Press d and find it in the drawer", false),
        ],
    },
];

/// Seed every guide this state dir has not delivered, then record the whole catalog as
/// delivered. Returns how many notice rows were created.
///
/// The record is read and written under the store's exclusive lock beside the state
/// transition, so this process's stale record cannot overwrite a concurrent process's
/// dismissal or watermark write. Marks only ever accumulate, so a record already naming
/// every catalog id skips the lock entirely.
pub fn seed_on_open(store: &TaskStore) -> Result<usize, String> {
    if CATALOG.iter().all(|guide| {
        delivery::load(store.path())
            .guides
            .contains(guide.catalog_id)
    }) {
        return Ok(0);
    }
    store.locked_transition_with_delivery(|state, record| {
        let pending: Vec<&Guide> = CATALOG
            .iter()
            .filter(|guide| !record.guides.contains(guide.catalog_id))
            .collect();
        if pending.is_empty() {
            return Ok((0, false, false));
        }
        let present: BTreeSet<String> = state
            .tasks()
            .iter()
            .filter_map(|task| task.notice.as_ref())
            .map(|notice| notice.catalog_id.clone())
            .collect();
        let mut created = 0;
        for guide in pending
            .iter()
            .filter(|guide| !present.contains(guide.catalog_id))
        {
            state
                .create_notice(
                    guide.catalog_id,
                    guide.title,
                    Some(guide.notes.to_string()),
                    guide.status,
                    TaskScope::Global,
                    guide
                        .steps
                        .iter()
                        .map(|(text, done)| (text.to_string(), *done))
                        .collect(),
                )
                .map_err(|error| error.to_string())?;
            created += 1;
        }
        record
            .guides
            .extend(pending.iter().map(|guide| guide.catalog_id.to_string()));
        Ok((created, created > 0, true))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::domain::{DomainState, ProvenanceOrigin, Task};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_store(label: &str) -> TaskStore {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "tsk-guides-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        TaskStore::new(dir)
    }

    fn notices(state: &DomainState) -> Vec<&Task> {
        state
            .tasks()
            .iter()
            .filter(|task| task.is_notice())
            .collect()
    }

    fn catalog_ids() -> BTreeSet<String> {
        CATALOG
            .iter()
            .map(|guide| guide.catalog_id.to_string())
            .collect()
    }

    fn replace_store_with_ordinary_task_only(store: &TaskStore) {
        let mut forgetting = DomainState::new();
        forgetting
            .create(
                "unrelated",
                None,
                TaskScope::Global,
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("task");
        store.save(&forgetting).expect("save");
    }

    #[test]
    fn first_open_seeds_four_desk_notices_and_a_second_open_adds_none() {
        let store = temp_store("empty");
        assert_eq!(seed_on_open(&store), Ok(4));
        let state = store.load().expect("load");
        let seeded = notices(&state);
        assert_eq!(
            seeded
                .iter()
                .map(|task| task.board_identifier())
                .collect::<Vec<_>>(),
            (1..=4).map(|n| Some(format!("N{n}"))).collect::<Vec<_>>()
        );
        assert!(seeded.iter().all(|task| task.scope == TaskScope::Global));
        assert!(seeded.iter().all(|task| task.number.is_none()));
        let start = seeded[0];
        assert_eq!(start.title, "Welcome to tsk, start here");
        assert_eq!(start.status, HumanStatus::Review);
        assert_eq!(
            start.steps.iter().map(|step| step.done).collect::<Vec<_>>(),
            vec![true, false, false]
        );
        assert!(start
            .notes
            .as_deref()
            .is_some_and(|notes| notes.contains("NEEDS YOU")));
        assert_eq!(delivery::load(store.path()).guides, catalog_ids());

        assert_eq!(seed_on_open(&store), Ok(0));
        assert_eq!(notices(&store.load().expect("reload")).len(), 4);
        let _ = fs::remove_dir_all(store.path());
    }

    #[test]
    fn an_existing_store_with_ordinary_tasks_gains_guides_and_keeps_its_t_numbers() {
        let store = temp_store("existing");
        let mut state = DomainState::new();
        for title in ["first", "second"] {
            state
                .create(
                    title,
                    None,
                    TaskScope::Global,
                    ProvenanceOrigin::Manual,
                    None,
                )
                .expect("task");
        }
        store.save(&state).expect("save");

        assert_eq!(seed_on_open(&store), Ok(4));
        let state = store.load().expect("load");
        let ordinary: Vec<Option<u64>> = state
            .tasks()
            .iter()
            .filter(|task| !task.is_notice())
            .map(|task| task.number)
            .collect();
        assert_eq!(ordinary, vec![Some(1), Some(2)]);
        assert_eq!(state.next_task_number, 3);
        assert_eq!(notices(&state).len(), 4);
        let _ = fs::remove_dir_all(store.path());
    }

    #[test]
    fn a_dismissed_guide_is_not_seeded_again_even_after_it_leaves_the_store() {
        let store = temp_store("dismiss");
        assert_eq!(seed_on_open(&store), Ok(4));
        let mut state = store.load().expect("load");
        let ids: Vec<uuid::Uuid> = notices(&state).iter().map(|task| task.id).collect();
        state.complete(ids[0]).expect("complete");
        state.archive_task(ids[1]).expect("archive");
        state.soft_delete(ids[2]).expect("delete");
        store.save(&state).expect("save");
        delivery::record_dismissed_notices(&store, state.tasks()).expect("record");

        replace_store_with_ordinary_task_only(&store);
        assert_eq!(seed_on_open(&store), Ok(0));
        assert!(notices(&store.load().expect("reload")).is_empty());
        let _ = fs::remove_dir_all(store.path());
    }

    #[test]
    fn a_lost_delivery_record_is_rebuilt_from_the_store_without_duplicates() {
        let store = temp_store("crash");
        assert_eq!(seed_on_open(&store), Ok(4));
        fs::remove_file(store.path().join(delivery::DELIVERY_FILE)).expect("simulate crash");

        assert_eq!(seed_on_open(&store), Ok(0));
        let state = store.load().expect("load");
        let ids: BTreeSet<String> = notices(&state)
            .iter()
            .map(|task| task.notice.as_ref().expect("notice").catalog_id.clone())
            .collect();
        assert_eq!(notices(&state).len(), 4, "no catalog id is created twice");
        assert_eq!(ids, catalog_ids());
        assert_eq!(delivery::load(store.path()).guides, catalog_ids());
        let _ = fs::remove_dir_all(store.path());
    }

    #[test]
    fn a_new_catalog_row_reaches_a_store_that_already_has_the_others() {
        let store = temp_store("partial");
        let mut record = delivery::DeliveryDocument::default();
        record.guides.extend(
            CATALOG
                .iter()
                .skip(1)
                .map(|guide| guide.catalog_id.to_string()),
        );
        delivery::save(store.path(), &record).expect("save record");

        assert_eq!(seed_on_open(&store), Ok(1));
        let state = store.load().expect("load");
        let seeded = notices(&state);
        assert_eq!(seeded.len(), 1);
        assert_eq!(
            seeded[0].notice.as_ref().map(|n| n.catalog_id.as_str()),
            Some(CATALOG[0].catalog_id)
        );
        assert_eq!(delivery::load(store.path()).guides, catalog_ids());
        let _ = fs::remove_dir_all(store.path());
    }

    #[test]
    fn the_cli_guide_points_at_backups() {
        assert!(CATALOG
            .iter()
            .any(|guide| guide.notes.contains("tsk backup setup")
                && guide.notes.contains("tsk restore")));
    }

    #[test]
    fn catalog_rows_have_unique_ids_and_paintable_text() {
        let ids: BTreeSet<&str> = CATALOG.iter().map(|guide| guide.catalog_id).collect();
        assert_eq!(ids.len(), CATALOG.len());
        for guide in &CATALOG {
            assert!(
                guide.catalog_id.starts_with("guide."),
                "{}",
                guide.catalog_id
            );
            assert!(!guide.notes.ends_with('\n'), "{}", guide.catalog_id);
            assert!(
                guide.steps.iter().all(|(text, _)| !text.trim().is_empty()),
                "{}",
                guide.catalog_id
            );
        }
    }
}
