//! Due date CLI: `tsk add --due`, a plan item's `due`, `tsk edit --due/--clear-due`, `list --sort due`.
//! Black box on purpose: only argv in, stdout/stderr/exit and the store file out.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tsk_tui::cli::{run_with, CliOutput};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Board(PathBuf);

impl Board {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-due-{label}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).expect("create state dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Run `tsk <args> --state-dir <board>`.
    fn run(&self, args: &[&str]) -> CliOutput {
        let mut argv: Vec<String> = vec!["tsk".into()];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        argv.push("--state-dir".into());
        argv.push(self.0.to_string_lossy().into_owned());
        run_with(argv, Cursor::new(Vec::<u8>::new()), true)
    }

    fn add(&self, title: &str, extra: &[&str]) -> CliOutput {
        let mut args = vec!["add", "-t", title, "--desk"];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    /// Every desk task as `list --json` prints it, in the order requested.
    fn rows(&self, extra: &[&str]) -> Vec<Value> {
        let mut args = vec!["list", "--desk", "--json"];
        args.extend_from_slice(extra);
        let output = self.run(&args);
        assert_eq!(output.code, 0, "{output:?}");
        serde_json::from_str(&output.stdout).expect("list --json is an array")
    }

    fn row(&self, title: &str) -> Value {
        self.rows(&[])
            .into_iter()
            .find(|row| row["title"] == title)
            .unwrap_or_else(|| panic!("no task titled {title}"))
    }

    fn store_bytes(&self) -> Vec<u8> {
        fs::read(self.0.join("tsk.json")).unwrap_or_default()
    }
}

impl Drop for Board {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn due_round_trips_and_touches_neither_status_nor_after() {
    let board = Board::new("round-trip");
    assert_eq!(board.add("dated", &["--due=2026-03-15"]).code, 0);
    assert_eq!(board.add("undated", &[]).code, 0);

    let dated = board.row("dated");
    assert_eq!(dated["due"], "2026-03-15");
    assert_eq!(dated["status"], "open");
    assert_eq!(dated["after"], serde_json::json!([]));
    assert!(
        board.row("undated").get("due").is_none(),
        "an undated row carries no due key"
    );

    // Space-separated form, change, then clear.
    assert_eq!(board.run(&["edit", "T2", "--due", "2026-04-01"]).code, 0);
    assert_eq!(board.row("undated")["due"], "2026-04-01");
    assert_eq!(board.run(&["edit", "T2", "--due=2026-05-09"]).code, 0);
    assert_eq!(board.row("undated")["due"], "2026-05-09");
    assert_eq!(board.run(&["edit", "T2", "--clear-due"]).code, 0);
    assert!(board.row("undated").get("due").is_none());
    assert_eq!(board.row("undated")["status"], "open");
}

#[test]
fn repeating_an_edit_or_clearing_an_absent_due_writes_nothing() {
    let board = Board::new("noop");
    assert_eq!(board.add("task", &["--due=2026-03-15"]).code, 0);
    let before = board.store_bytes();
    assert_eq!(board.run(&["edit", "T1", "--due=2026-03-15"]).code, 0);
    assert_eq!(board.store_bytes(), before, "same due: store untouched");

    assert_eq!(board.run(&["edit", "T1", "--clear-due"]).code, 0);
    let cleared = board.store_bytes();
    assert_eq!(board.run(&["edit", "T1", "--clear-due"]).code, 0);
    assert_eq!(board.store_bytes(), cleared, "no due to clear: untouched");
}

#[test]
fn only_a_real_yyyy_mm_dd_day_is_accepted() {
    let board = Board::new("days");
    let refused = [
        "2026-02-30",
        "2026-13-01",
        "2026-00-10",
        "2026-04-31",
        "2026-01-00",
        "2027-02-29",
        "2100-02-29",
        "0000-01-01",
        "tomorrow",
        "2026-2-3",
        "26-02-03",
        "",
        " 2026-03-15",
        "2026-03-15 ",
        "2026-03-15T10:00",
        "2026-03-15 10:00",
        "2026-03-15Z",
        "2026-03-15+02:00",
        "２０２６-03-15",
    ];
    assert_eq!(board.add("base", &["--due=2026-05-05"]).code, 0);
    for bad in refused {
        let flag = format!("--due={bad}");
        let before = board.store_bytes();
        let added = board.add("new", &[&flag]);
        assert_eq!(added.code, 1, "add {bad:?}: {added:?}");
        assert!(
            added.stderr.contains("invalid-due"),
            "add {bad:?}: {added:?}"
        );
        let edited = board.run(&["edit", "T1", &flag]);
        assert_eq!(edited.code, 1, "edit {bad:?}: {edited:?}");
        assert!(
            edited.stderr.contains("invalid-due"),
            "edit {bad:?}: {edited:?}"
        );
        assert_eq!(
            board.store_bytes(),
            before,
            "{bad:?} must not touch the store"
        );
    }
    assert_eq!(board.row("base")["due"], "2026-05-05");

    // Month ends (30 and 31 days), leap days: divisible by 4, and the century rule (2000 yes,
    // 2100 no).
    for good in [
        "2026-04-30",
        "2026-11-30",
        "2026-01-31",
        "2028-02-29",
        "2000-02-29",
        "0001-01-01",
        "9999-12-31",
    ] {
        let title = format!("good-{good}");
        let added = board.add(&title, &[&format!("--due={good}")]);
        assert_eq!(added.code, 0, "{good}: {added:?}");
        assert_eq!(board.row(&title)["due"], good);
    }
}

#[test]
fn due_and_clear_due_together_are_a_usage_error() {
    let board = Board::new("conflict");
    assert_eq!(board.add("task", &["--due=2026-03-15"]).code, 0);
    let output = board.run(&["edit", "T1", "--due=2026-04-01", "--clear-due"]);
    assert_eq!(output.code, 2, "{output:?}");
    assert_eq!(board.row("task")["due"], "2026-03-15");
}

#[test]
fn plan_items_take_due_and_a_bad_one_fails_alone() {
    let board = Board::new("plan");
    let dir = board.path().join("plan.json");
    fs::write(
        &dir,
        r#"[
            {"title":"planned","project":null,"due":"2026-06-01"},
            {"title":"nulled","project":null,"due":null},
            {"title":"planbad","project":null,"due":"2026-02-30"},
            {"title":"plantype","project":null,"due":20260601}
        ]"#,
    )
    .expect("write plan");
    let output = board.run(&["add", "--file", &dir.to_string_lossy()]);
    assert_eq!(output.code, 1, "{output:?}");
    let result: Value = serde_json::from_str(&output.stdout).expect("plan result");
    let failed = result["failed"].as_array().expect("failed list");
    assert_eq!(failed.len(), 2, "{result}");
    assert_eq!(
        (failed[0]["i"].clone(), failed[0]["code"].clone()),
        (2.into(), "invalid-due".into())
    );
    assert_eq!(
        (failed[1]["i"].clone(), failed[1]["code"].clone()),
        (3.into(), "invalid-item".into())
    );
    assert_eq!(result["created"].as_array().expect("created").len(), 2);

    assert_eq!(board.row("planned")["due"], "2026-06-01");
    assert!(board.row("nulled").get("due").is_none());
    assert_eq!(board.rows(&[]).len(), 2, "bad items are not created");
}

#[test]
fn adding_the_same_title_with_the_same_due_is_one_task_and_a_new_due_is_another() {
    let board = Board::new("idempotent");
    assert_eq!(board.add("same", &["--due=2026-03-15"]).code, 0);
    let again = board.add("same", &["--due=2026-03-15"]);
    assert_eq!(again.code, 0, "{again:?}");
    assert_eq!(board.rows(&[]).len(), 1, "repeat is the existing task");
    assert_eq!(board.add("same", &["--due=2026-03-16"]).code, 0);
    assert_eq!(
        board.rows(&[]).len(),
        2,
        "a different due is a different task"
    );
}

#[test]
fn sort_due_puts_the_earliest_first_and_undated_last() {
    let board = Board::new("sort");
    for (title, due) in [
        ("may", Some("2026-05-01")),
        ("mar", Some("2026-03-01")),
        ("nodue", None),
        ("apr", Some("2026-04-01")),
        ("last-year", Some("2025-12-31")),
    ] {
        let flag = due.map(|day| format!("--due={day}"));
        let extra: Vec<&str> = flag.as_deref().into_iter().collect();
        assert_eq!(board.add(title, &extra).code, 0);
    }
    let titles = |rows: Vec<Value>| -> Vec<String> {
        rows.iter()
            .map(|row| row["title"].as_str().expect("title").to_string())
            .collect()
    };
    assert_eq!(
        titles(board.rows(&["--sort", "due"])),
        ["last-year", "mar", "apr", "may", "nodue"]
    );
    let unknown = board.run(&["list", "--desk", "--sort", "dued"]);
    assert_eq!(unknown.code, 2);
    assert!(unknown.stderr.contains("due"), "{unknown:?}");
}

#[test]
fn the_human_listing_and_the_task_detail_show_the_due_day() {
    let board = Board::new("text");
    assert_eq!(board.add("dated", &["--due=2026-03-15"]).code, 0);
    assert_eq!(board.add("plain", &[]).code, 0);

    let list = board.run(&["list", "--desk"]);
    let dated_line = list
        .stdout
        .lines()
        .find(|line| line.contains("dated"))
        .expect("dated row");
    assert!(dated_line.contains("due 2026-03-15"), "{}", list.stdout);
    let plain_line = list
        .stdout
        .lines()
        .find(|line| line.contains("plain"))
        .expect("plain row");
    assert!(!plain_line.contains("due"), "{}", list.stdout);

    let detail = board.run(&["list", "T1"]);
    assert!(
        detail.stdout.contains("due 2026-03-15"),
        "{}",
        detail.stdout
    );
    let direct = board.run(&["list", "T1", "--json"]);
    let rows: Vec<Value> = serde_json::from_str(&direct.stdout).expect("direct json");
    assert_eq!(rows[0]["due"], "2026-03-15");
}

#[test]
fn changing_due_is_a_dated_edited_event_naming_the_field() {
    let board = Board::new("activity");
    assert_eq!(board.add("task", &[]).code, 0);
    assert_eq!(board.run(&["edit", "T1", "--due=2026-03-15"]).code, 0);
    let direct = board.run(&["list", "T1", "--json"]);
    let rows: Vec<Value> = serde_json::from_str(&direct.stdout).expect("direct json");
    let newest = &rows[0]["activity"][0];
    assert_eq!(newest["kind"], "edited", "{newest}");
    assert_eq!(newest["detail"]["fields"], serde_json::json!(["due"]));
}

#[test]
fn add_takes_the_due_day_as_a_separate_argument_too() {
    let board = Board::new("spaced");
    assert_eq!(board.add("spaced", &["--due", "2026-07-04"]).code, 0);
    assert_eq!(board.row("spaced")["due"], "2026-07-04");
    let missing = board.add("nodate", &["--due"]);
    assert_eq!(
        missing.code, 2,
        "a bare --due is a usage error: {missing:?}"
    );
    assert_eq!(board.rows(&[]).len(), 1);
}

#[test]
fn the_human_paper_trail_says_due_edited() {
    let board = Board::new("trail");
    assert_eq!(board.add("task", &[]).code, 0);
    assert_eq!(board.run(&["edit", "T1", "--due=2026-03-15"]).code, 0);
    let detail = board.run(&["list", "T1"]);
    assert!(detail.stdout.contains("due edited"), "{}", detail.stdout);
}

fn titles(rows: Vec<Value>) -> Vec<String> {
    rows.iter()
        .map(|row| row["title"].as_str().expect("title").to_string())
        .collect()
}

#[test]
fn tasks_due_the_same_day_keep_their_recent_order() {
    let board = Board::new("ties");
    for (title, due) in [
        ("first", "2026-03-15"),
        ("second", "2026-03-15"),
        ("earlier", "2026-03-01"),
        ("third", "2026-03-15"),
    ] {
        assert_eq!(board.add(title, &[&format!("--due={due}")]).code, 0);
    }
    let recent: Vec<String> = titles(board.rows(&[]))
        .into_iter()
        .filter(|title| title != "earlier")
        .collect();
    let by_due = titles(board.rows(&["--sort", "due"]));
    assert_eq!(by_due[0], "earlier");
    assert_eq!(by_due[1..], recent[..]);
}

#[test]
fn grouped_rows_keep_the_due_day_whichever_dimension_the_group_names() {
    let board = Board::new("grouped");
    let added = board.add(
        "dana task",
        &["--assignee", "@dana", "--thread", "t1", "--due=2026-03-15"],
    );
    assert_eq!(added.code, 0, "{added:?}");
    for group in ["assignee", "thread", "none"] {
        let list = board.run(&["list", "--desk", "--group", group]);
        assert_eq!(list.code, 0, "{list:?}");
        let row = list
            .stdout
            .lines()
            .find(|line| line.contains("dana task"))
            .unwrap_or_else(|| panic!("no row under --group {group}: {}", list.stdout));
        assert!(row.contains("due 2026-03-15"), "--group {group}: {row}");
    }
}

#[test]
fn the_store_has_no_due_key_for_an_undated_or_cleared_task() {
    let board = Board::new("no-key");
    assert_eq!(board.add("plain", &[]).code, 0);
    assert_eq!(board.add("cleared", &["--due=2026-03-15"]).code, 0);
    assert_eq!(board.run(&["edit", "T2", "--clear-due"]).code, 0);
    assert_eq!(board.add("dated", &["--due=2026-03-16"]).code, 0);
    let document: Value = serde_json::from_slice(&board.store_bytes()).expect("store json");
    let tasks = document["tasks"].as_array().expect("tasks");
    for task in tasks {
        let has_key = task.get("due").is_some();
        assert_eq!(has_key, task["title"] == "dated", "{task}");
    }
    assert_eq!(document["format_version"], 16);
}
