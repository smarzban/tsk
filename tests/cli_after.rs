//! `after` on the CLI: `edit --after`, add plans, `list --json`, `status started --force`, and a
//! done that starts what waited on it. Uses temp dirs only.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::HumanStatus;
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsk-cli-after-{label}-{nanos}-{seq}"));
        fs::create_dir_all(dir.join("state")).expect("create temp state dir");
        Temp(dir)
    }

    fn state(&self) -> PathBuf {
        self.0.join("state")
    }

    /// A project directory (not a git repository) for cross-project tasks.
    fn project(&self, name: &str) -> String {
        let path = self.0.join(name);
        fs::create_dir_all(&path).expect("project dir");
        path.to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> CliOutput {
        let mut command: Vec<String> = vec!["tsk".into()];
        command.extend(args.iter().map(|arg| (*arg).to_string()));
        command.push("--state-dir".into());
        command.push(self.state().to_string_lossy().into_owned());
        run_with(command, Cursor::new(Vec::<u8>::new()), true)
    }

    /// Add one task in `scope` (`--desk` or a project path) and return its number.
    fn add(&self, title: &str, scope: Option<&str>) -> u64 {
        let output = match scope {
            Some(project) => self.run(&["add", "-t", title, "-p", project]),
            None => self.run(&["add", "-t", title, "--desk"]),
        };
        assert_eq!(output.code, 0, "{}", output.stderr);
        let state = TaskStore::new(self.state()).load().expect("load");
        state
            .tasks()
            .iter()
            .find(|task| task.title == title)
            .and_then(|task| task.number)
            .expect("numbered")
    }

    fn json(&self, number: u64) -> serde_json::Value {
        let output = self.run(&["list", &format!("T{number}"), "--json"]);
        assert_eq!(output.code, 0, "{}", output.stderr);
        serde_json::from_str::<serde_json::Value>(&output.stdout).expect("json")[0].clone()
    }

    fn status(&self, number: u64) -> HumanStatus {
        TaskStore::new(self.state())
            .load()
            .expect("load")
            .tasks()
            .iter()
            .find(|task| task.number == Some(number))
            .expect("task")
            .status
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn edit_after_sets_and_clears_and_list_json_shows_after_and_before() {
    let temp = Temp::new("edit");
    let app = temp.project("app");
    let first = temp.add("first", Some(&app));
    let second = temp.add("second", None);
    let third = temp.add("third", Some(&app));

    let output = temp.run(&[
        "edit",
        &format!("T{third}"),
        "--after",
        &format!("T{first}"),
        "--after",
        &second.to_string(),
    ]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    let row = temp.json(third);
    assert_eq!(
        row["after"],
        serde_json::json!([
            {"task": first, "done": false},
            {"task": second, "done": false},
        ])
    );
    assert_eq!(row["before"], serde_json::json!([]));
    assert_eq!(temp.json(first)["before"], serde_json::json!([third]));
    let all: serde_json::Value =
        serde_json::from_str(&temp.run(&["list", "--all", "--json"]).stdout).expect("json");
    let listed = all
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["number"] == third)
        .expect("row");
    assert_eq!(
        listed["after"][0]["task"], first,
        "filtered rows carry after"
    );

    let human = temp.run(&["list", &format!("T{third}")]);
    assert!(
        human.stdout.contains(&format!("after T{first}, T{second}")),
        "{}",
        human.stdout
    );

    assert_eq!(temp.run(&["status", &format!("T{first}"), "done"]).code, 0);
    assert_eq!(
        temp.json(third)["after"][0],
        serde_json::json!({"task": first, "done": true})
    );

    let output = temp.run(&["edit", &format!("T{third}"), "--clear-after"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(temp.json(third)["after"], serde_json::json!([]));
}

#[test]
fn edit_after_refuses_itself_unknown_done_and_loops() {
    let temp = Temp::new("refuse");
    let one = temp.add("one", None);
    let two = temp.add("two", None);
    let done = temp.add("done already", None);
    assert_eq!(temp.run(&["status", &format!("T{done}"), "done"]).code, 0);

    for (after, code) in [
        (one, "invalid-after"),
        (99, "invalid-after"),
        (done, "invalid-after"),
    ] {
        let output = temp.run(&["edit", &format!("T{one}"), "--after", &after.to_string()]);
        assert_eq!(output.code, 1, "{after}");
        assert!(
            output.stderr.starts_with(&format!("tsk edit: {code}: ")),
            "{}",
            output.stderr
        );
    }
    assert_eq!(
        temp.run(&["edit", &format!("T{two}"), "--after", &one.to_string()])
            .code,
        0
    );
    let output = temp.run(&["edit", &format!("T{one}"), "--after", &two.to_string()]);
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        format!("tsk edit: after-loop: T{two} already runs after T{one}\n")
    );
    assert_eq!(temp.json(one)["after"], serde_json::json!([]));
}

#[test]
fn add_plans_accept_after_and_refuse_unknown_or_done_items_alone() {
    let temp = Temp::new("plan");
    let first = temp.add("first", None);
    let plan = format!(
        r#"[{{"title": "waits", "project": null, "after": [{first}]}},
            {{"title": "waits too", "project": null, "after": ["T{first}"]}},
            {{"title": "bad", "project": null, "after": [404]}},
            {{"title": "worse", "project": null, "after": "T1"}}]"#
    );
    let output = run_with(
        vec![
            "tsk".into(),
            "add".into(),
            "--state-dir".into(),
            temp.state().to_string_lossy().into_owned(),
        ],
        Cursor::new(plan.into_bytes()),
        false,
    );
    assert_eq!(output.code, 1, "{}", output.stdout);
    let result: serde_json::Value = serde_json::from_str(&output.stdout).expect("json");
    assert_eq!(result["created"].as_array().expect("created").len(), 2);
    assert_eq!(result["failed"][0]["code"], "invalid-after");
    assert_eq!(result["failed"][1]["code"], "invalid-item");
    let waits = result["created"][0]["number"].as_u64().expect("number");
    assert_eq!(temp.json(waits)["after"][0]["task"], first);
    assert_eq!(
        temp.json(first)["before"].as_array().expect("before").len(),
        2
    );
}

#[test]
fn starting_a_waiting_task_refuses_unless_forced() {
    let temp = Temp::new("force");
    let first = temp.add("first", None);
    let second = temp.add("second", None);
    assert_eq!(
        temp.run(&["edit", &format!("T{second}"), "--after", &first.to_string()])
            .code,
        0
    );
    assert_eq!(
        temp.run(&["status", &format!("T{first}"), "started"]).code,
        0
    );
    let output = temp.run(&["status", &format!("T{second}"), "started"]);
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        format!(
            "tsk status: after-not-done: T{second} runs after T{first} (started); --force starts it anyway\n"
        )
    );
    assert_eq!(temp.status(second), HumanStatus::Open);
    let output = temp.run(&["status", &format!("T{second}"), "started", "--force"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(temp.status(second), HumanStatus::Started);
}

/// A chain across a project and the desk: the done of the last prerequisite starts the waiting
/// ready task and records why; a waiting task in open does not move.
#[test]
fn a_done_starts_ready_tasks_it_released_across_projects() {
    let temp = Temp::new("release");
    let app = temp.project("app");
    let api = temp.project("api");
    let first = temp.add("first", Some(&app));
    let second = temp.add("second", Some(&api));
    let third = temp.add("third", None);
    let fourth = temp.add("fourth", Some(&app));
    for (task, after) in [(second, first), (third, second), (fourth, first)] {
        assert_eq!(
            temp.run(&["edit", &format!("T{task}"), "--after", &after.to_string()])
                .code,
            0
        );
    }
    assert_eq!(
        temp.run(&["status", &format!("T{second}"), "ready"]).code,
        0
    );

    let output = temp.run(&["status", &format!("T{first}"), "done"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        output.stdout,
        format!("status T{first} done first\nT{second} started · T{first} done\n"),
    );
    assert_eq!(temp.status(second), HumanStatus::Started);
    assert_eq!(temp.status(third), HumanStatus::Open, "still waits on T2");
    assert_eq!(temp.status(fourth), HumanStatus::Open, "open does not move");
    let activity = &temp.json(second)["activity"][0];
    assert_eq!(activity["kind"], "status_set");
    assert_eq!(activity["detail"]["after"], first);
    let plain = temp.run(&["list", &format!("T{second}")]);
    assert!(
        plain.stdout.contains(&format!("started · after T{first}")),
        "{}",
        plain.stdout
    );

    // Review is not done: nothing starts.
    assert_eq!(temp.run(&["status", &format!("T{third}"), "ready"]).code, 0);
    let output = temp.run(&["status", &format!("T{second}"), "review"]);
    assert_eq!(output.stdout, format!("status T{second} review second\n"));
    assert_eq!(temp.status(third), HumanStatus::Ready);
    // A repeat done is idempotent and releases nothing again.
    let output = temp.run(&["status", &format!("T{first}"), "done"]);
    assert_eq!(output.stdout, format!("status T{first} done first\n"));
}
