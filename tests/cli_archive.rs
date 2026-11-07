//! Archive CLI: `tsk archive T<n>` / `tsk unarchive T<n>` (AC-29, AC-31).
//! Uses temp dirs only; never writes real plugin state.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::{run_with, CliOutput};
use tsk_tui::domain::{DomainState, ProvenanceOrigin, TaskEventKind, TaskScope};
use tsk_tui::store::TaskStore;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-cli-archive-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp state dir");
    dir
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn cli(args: Vec<String>) -> CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn add_task(dir: &Path, title: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        "add".into(),
        "-t".into(),
        title.into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn archive(dir: &Path, verb: &str, task: &str) -> CliOutput {
    cli(vec![
        "tsk".into(),
        verb.into(),
        task.into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ])
}

fn archived_flag_and_events(dir: &Path) -> (bool, usize) {
    let state = TaskStore::new(dir).load().expect("load store");
    let task = state.tasks().first().expect("seeded task");
    (
        task.archived,
        task.history
            .iter()
            .filter(|event| event.kind == TaskEventKind::Archived)
            .count(),
    )
}

#[test]
fn archive_and_unarchive_by_number_exit_0_and_repeat_is_idempotent() {
    let dir = temp_state_dir("idempotent");
    let _guard = TempDirGuard(dir.clone());
    let added = add_task(&dir, "file me");
    assert_eq!(added.code, 0, "{:?}", added.stderr);

    let first = archive(&dir, "archive", "T1");
    assert_eq!(first.code, 0, "{:?}", first.stderr);
    assert_eq!(first.stdout, "archived T1 file me\n");
    let (flag, events) = archived_flag_and_events(&dir);
    assert!(flag);
    assert_eq!(events, 1, "exactly one archived event after one archive");

    // Repeat prints the same and stays exit 0 with no second event.
    let second = archive(&dir, "archive", "T1");
    assert_eq!(second.code, 0, "{:?}", second.stderr);
    assert_eq!(second.stdout, "archived T1 file me\n");
    let (flag, events) = archived_flag_and_events(&dir);
    assert!(flag);
    assert_eq!(events, 1, "a repeat archive writes no second event");

    let un = archive(&dir, "unarchive", "T1");
    assert_eq!(un.code, 0, "{:?}", un.stderr);
    assert_eq!(un.stdout, "unarchived T1 file me\n");
    let un2 = archive(&dir, "unarchive", "T1");
    assert_eq!(un2.code, 0);
    assert_eq!(un2.stdout, "unarchived T1 file me\n");
    let (flag, events) = archived_flag_and_events(&dir);
    assert!(!flag);
    assert_eq!(
        events, 1,
        "unarchive never touches the archived event count"
    );
}

#[test]
fn archive_and_unarchive_refusals_include_stable_codes_and_human_messages() {
    let dir = temp_state_dir("refusals");
    let _guard = TempDirGuard(dir.clone());
    let added = add_task(&dir, "delete me later");
    assert_eq!(added.code, 0);

    for verb in ["archive", "unarchive"] {
        let unknown = archive(&dir, verb, "T99");
        assert_eq!(unknown.code, 1, "{:?}", unknown.stdout);
        assert!(
            unknown.stderr.contains("unknown-task"),
            "{verb}: {:?}",
            unknown.stderr
        );
        assert!(
            unknown.stderr.contains("T99 is not on the board"),
            "{verb}: {:?}",
            unknown.stderr
        );
    }

    let deleted = cli(vec![
        "tsk".into(),
        "add".into(),
        "-t".into(),
        "gone".into(),
        "--desk".into(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(deleted.code, 0);
    let state = TaskStore::new(&dir).load().expect("load");
    let gone_id = state
        .tasks()
        .iter()
        .find(|task| task.title == "gone")
        .expect("gone task")
        .id;
    let mut writable = TaskStore::new(&dir).load().expect("load");
    writable.soft_delete(gone_id).expect("soft delete");
    TaskStore::new(&dir)
        .save(&writable)
        .expect("persist the soft delete");

    for verb in ["archive", "unarchive"] {
        let rejected = archive(&dir, verb, "T2");
        assert_eq!(rejected.code, 1, "{verb}: {:?}", rejected.stdout);
        assert!(
            rejected.stderr.contains("soft-deleted-task"),
            "{verb}: {:?}",
            rejected.stderr
        );
        assert!(
            rejected.stderr.contains("T2 is deleted"),
            "{verb}: {:?}",
            rejected.stderr
        );
    }
}

#[test]
fn project_archive_and_unarchive_resolve_basename_and_path_exit_0_and_are_idempotent() {
    let dir = temp_state_dir("project-verbs");
    let _guard = TempDirGuard(dir.clone());
    let project_path = dir.join("widget");
    std::fs::create_dir(&project_path).expect("create project directory");
    let project_text = project_path.to_string_lossy().into_owned();
    let added = cli(vec![
        "tsk".into(),
        "add".into(),
        "-t".into(),
        "widget task".into(),
        "-p".into(),
        project_text.clone(),
        "--state-dir".into(),
        dir.to_string_lossy().into_owned(),
    ]);
    assert_eq!(added.code, 0, "{:?}", added.stderr);

    let store = || TaskStore::new(&dir).load().expect("load store");
    let project = |verb: &str, name: &str| {
        cli(vec![
            "tsk".into(),
            "project".into(),
            verb.into(),
            name.into(),
            "--state-dir".into(),
            dir.to_string_lossy().into_owned(),
        ])
    };

    // Basename, case-insensitive.
    let out = project("archive", "widget");
    assert_eq!(out.code, 0, "{:?}", out.stderr);
    assert_eq!(out.stdout, "archived project widget\n");
    assert!(store().is_project_archived(&project_text));

    // Idempotent repeat.
    let again = project("archive", "widget");
    assert_eq!(again.code, 0);
    assert_eq!(again.stdout, "archived project widget\n");
    assert_eq!(store().projects().len(), 1, "exactly one record");

    // Case-insensitive basename is the same project.
    let upper = project("archive", "WIDGET");
    assert_eq!(upper.code, 0, "{:?}", upper.stderr);
    assert_eq!(store().projects().len(), 1);

    // Absolute path addresses the same project.
    let verbatim = project("archive", &project_text);
    assert_eq!(verbatim.code, 0, "{:?}", verbatim.stderr);
    assert_eq!(store().projects().len(), 1);

    // Unarchive removes the record.
    let un = project("unarchive", "widget");
    assert_eq!(un.code, 0, "{:?}", un.stderr);
    assert_eq!(un.stdout, "unarchived project widget\n");
    assert!(
        !store().is_project_archived(&project_text),
        "the record is gone"
    );
    assert_eq!(store().projects().len(), 0);

    // An unknown name exits 1.
    let unknown = project("archive", "nothing-here");
    assert_eq!(unknown.code, 1, "{:?}", unknown.stdout);
    assert!(
        unknown
            .stderr
            .contains("no project named nothing-here has tasks"),
        "{:?}",
        unknown.stderr
    );
}

#[test]
fn project_archive_basename_ignores_an_outside_git_invocation_candidate() {
    let dir = temp_state_dir("project-outside-invocation");
    let _guard = TempDirGuard(dir.clone());
    let outside = dir.join("outside").join("api");
    std::fs::create_dir_all(&outside).expect("create outside-Git invocation directory");
    let mut state = DomainState::new();
    state
        .create(
            "stored api task",
            None,
            TaskScope::Project {
                path: "/projects/api".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("create stored project fixture");
    TaskStore::new(&dir).save(&state).expect("save fixture");
    let context = serde_json::json!({"focused_pane_cwd": outside}).to_string();
    let binary = std::env::var("CARGO_BIN_EXE_tsk").expect("Cargo must provide the tsk binary");

    let output = Command::new(binary)
        .env("HERDR_PLUGIN_CONTEXT_JSON", context)
        .args([
            "project",
            "archive",
            "api",
            "--state-dir",
            &dir.to_string_lossy(),
        ])
        .output()
        .expect("run project archive");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(TaskStore::new(&dir)
        .load()
        .expect("load archived state")
        .is_project_archived("/projects/api"));
}

#[test]
fn project_verbs_keep_addressing_removed_and_legacy_relative_project_paths() {
    let dir = temp_state_dir("project-legacy-paths");
    let _guard = TempDirGuard(dir.clone());
    let removed = dir.join("removed-project").to_string_lossy().into_owned();
    let relative = "legacy/relative-project".to_string();
    let mut state = DomainState::new();
    for (title, path) in [
        ("removed", removed.as_str()),
        ("relative", relative.as_str()),
    ] {
        state
            .create(
                title,
                None,
                TaskScope::Project { path: path.into() },
                ProvenanceOrigin::Manual,
                None,
            )
            .expect("create project fixture");
    }
    TaskStore::new(&dir).save(&state).expect("save fixtures");

    let project = |verb: &str, name: &str| {
        cli(vec![
            "tsk".into(),
            "project".into(),
            verb.into(),
            name.into(),
            "--state-dir".into(),
            dir.to_string_lossy().into_owned(),
        ])
    };
    for path in [&removed, &relative] {
        let archived = project("archive", path);
        assert_eq!(archived.code, 0, "{archived:?}");
        let unarchived = project("unarchive", path);
        assert_eq!(unarchived.code, 0, "{unarchived:?}");
    }
}

#[test]
fn unarchive_refusals_name_the_unarchive_verb() {
    let dir = temp_state_dir("verb-refusal");
    let _guard = TempDirGuard(dir.clone());
    let added = add_task(&dir, "verb check");
    assert_eq!(added.code, 0);

    let unknown = archive(&dir, "unarchive", "T99");
    assert_eq!(unknown.code, 1);
    assert!(
        unknown
            .stderr
            .starts_with("tsk unarchive: unknown-task: T99 is not on the board"),
        "{:?}",
        unknown.stderr
    );
}

#[test]
fn project_archive_refuses_a_basename_shared_by_two_projects() {
    let dir = temp_state_dir("project-ambiguous");
    let _guard = TempDirGuard(dir.clone());
    let mut paths = Vec::new();
    for site in ["site-a", "site-b"] {
        let project = dir.join(site).join("wordpress").join("wp-content");
        fs::create_dir_all(&project).expect("create project directory");
        let path = project.to_string_lossy().into_owned();
        let added = cli(vec![
            "tsk".into(),
            "add".into(),
            "-t".into(),
            format!("{site} task"),
            "-p".into(),
            path.clone(),
            "--state-dir".into(),
            dir.to_string_lossy().into_owned(),
        ]);
        assert_eq!(added.code, 0, "{:?}", added.stderr);
        paths.push(path);
    }
    let project = |verb: &str, name: &str| {
        cli(vec![
            "tsk".into(),
            "project".into(),
            verb.into(),
            name.into(),
            "--state-dir".into(),
            dir.to_string_lossy().into_owned(),
        ])
    };

    for verb in ["archive", "unarchive"] {
        let out = project(verb, "wp-content");
        assert_eq!(out.code, 1, "{verb}: {out:?}");
        assert!(out.stdout.is_empty(), "{verb}: {:?}", out.stdout);
        assert_eq!(
            out.stderr,
            format!(
                "tsk project: project wp-content is ambiguous: {}, {}\n",
                paths[0], paths[1]
            ),
            "{verb}"
        );
    }
    let store = TaskStore::new(&dir).load().expect("load store");
    assert!(store.projects().is_empty(), "nothing archived");

    // A full path still addresses one of them.
    let one = project("archive", &paths[0]);
    assert_eq!(one.code, 0, "{one:?}");
    assert_eq!(one.stdout, "archived project wp-content\n");
}

/// `tsk project title`: a title names the project for `-p`, the listings, and the project
/// verbs; a second project cannot take it in any case; `--clear` gives the folder name back.
#[test]
fn project_title_sets_resolves_lists_refuses_a_duplicate_and_clears() {
    let dir = temp_state_dir("project-title");
    let _guard = TempDirGuard(dir.clone());
    let state_dir = dir.to_string_lossy().into_owned();
    let mut paths = Vec::new();
    for site in ["site-a", "site-b"] {
        let project = dir.join(site).join("wordpress").join("wp-content");
        fs::create_dir_all(&project).expect("create project directory");
        let path = project.to_string_lossy().into_owned();
        let added = cli(vec![
            "tsk".into(),
            "add".into(),
            "-t".into(),
            format!("{site} task"),
            "-p".into(),
            path.clone(),
            "--state-dir".into(),
            state_dir.clone(),
        ]);
        assert_eq!(added.code, 0, "{:?}", added.stderr);
        paths.push(path);
    }
    let run = |args: &[&str]| {
        let mut argv: Vec<String> = vec!["tsk".into()];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv.extend(["--state-dir".to_string(), state_dir.clone()]);
        cli(argv)
    };

    let titled = run(&["project", "title", &paths[0], "Site A"]);
    assert_eq!(titled.code, 0, "{titled:?}");
    assert_eq!(
        titled.stdout,
        format!("titled project Site A ({})\n", paths[0])
    );
    // Repeating it is safe and changes nothing.
    assert_eq!(run(&["project", "title", &paths[0], "Site A"]).code, 0);

    // Titles are unique across projects, ignoring case; the refusal names the holder.
    let taken = run(&["project", "title", &paths[1], "site a"]);
    assert_eq!(taken.code, 1, "{taken:?}");
    assert_eq!(
        taken.stderr,
        format!(
            "tsk project: title-taken: title site a is used by {}\n",
            paths[0]
        )
    );
    let invalid = run(&["project", "title", &paths[1], "desk"]);
    assert_eq!(invalid.code, 1, "{invalid:?}");
    assert!(
        invalid.stderr.starts_with("tsk project: invalid-title: "),
        "{invalid:?}"
    );

    // -p takes the title, ignoring case.
    let listed = run(&["list", "-p", "site A", "--json"]);
    assert_eq!(listed.code, 0, "{listed:?}");
    let rows: serde_json::Value = serde_json::from_str(&listed.stdout).expect("json");
    assert_eq!(rows.as_array().expect("rows").len(), 1);
    assert_eq!(rows[0]["title"], "site-a task");
    assert_eq!(rows[0]["project"], paths[0].as_str());
    assert_eq!(rows[0]["project_title"], "Site A");

    // An untitled project's rows carry no project_title.
    let all = run(&["list", "--all", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&all.stdout).expect("json");
    let site_b = rows
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["title"] == "site-b task")
        .expect("site-b row");
    assert!(site_b.get("project_title").is_none(), "{site_b}");

    // Grouped text names the titled project by its title; the basename is no longer shared
    // on screen, so the other project keeps its folder name.
    let grouped = run(&["list", "--all", "--group", "project"]);
    assert_eq!(grouped.code, 0, "{grouped:?}");
    assert!(grouped.stdout.contains("  Site A\n"), "{}", grouped.stdout);
    assert!(
        grouped.stdout.contains("  wp-content\n"),
        "{}",
        grouped.stdout
    );

    // The basename both share still refuses, listing the titled one as `Title (path)`.
    let ambiguous = run(&["project", "archive", "wp-content"]);
    assert_eq!(ambiguous.code, 1, "{ambiguous:?}");
    assert_eq!(
        ambiguous.stderr,
        format!(
            "tsk project: project wp-content is ambiguous: Site A ({}), {}\n",
            paths[0], paths[1]
        )
    );
    // The project verbs take the title too.
    let archived = run(&["project", "archive", "Site A"]);
    assert_eq!(archived.stdout, "archived project Site A\n");
    assert_eq!(run(&["project", "unarchive", "site a"]).code, 0);

    let cleared = run(&["project", "title", "Site A", "--clear"]);
    assert_eq!(cleared.code, 0, "{cleared:?}");
    assert_eq!(
        cleared.stdout,
        format!("cleared title of project wp-content ({})\n", paths[0])
    );
    let store = TaskStore::new(&dir).load().expect("load store");
    assert!(store.projects().is_empty(), "no record is left behind");
}

#[test]
fn project_title_usage_errors_exit_2_and_an_unknown_project_refuses() {
    let dir = temp_state_dir("project-title-usage");
    let _guard = TempDirGuard(dir.clone());
    let run = |args: &[&str]| {
        let mut argv: Vec<String> = vec!["tsk".into()];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv.extend([
            "--state-dir".to_string(),
            dir.to_string_lossy().into_owned(),
        ]);
        cli(argv)
    };
    for args in [
        &["project", "title", "atlas"][..],
        &["project", "title", "atlas", "Atlas", "--clear"][..],
        &["project", "archive", "atlas", "--clear"][..],
        &["project", "title"][..],
    ] {
        let out = run(args);
        assert_eq!(out.code, 2, "{args:?}: {out:?}");
    }
    let unknown = run(&["project", "title", "atlas", "Atlas"]);
    assert_eq!(unknown.code, 1, "{unknown:?}");
    assert_eq!(
        unknown.stderr,
        "tsk project: unknown-project: no project named atlas has tasks\n"
    );
}

/// A titled project names itself in `tsk add --json` (both outcomes), in the `--all` scope
/// headings, and in `--sort project` order, which sorts by the shown name.
#[test]
fn project_title_reaches_add_json_scope_headings_and_project_sort() {
    let dir = temp_state_dir("project-title-add");
    let _guard = TempDirGuard(dir.clone());
    let state_dir = dir.to_string_lossy().into_owned();
    let zeta = dir.join("zeta");
    let beta = dir.join("beta");
    for project in [&zeta, &beta] {
        fs::create_dir_all(project).expect("create project directory");
    }
    let (zeta, beta) = (
        zeta.to_string_lossy().into_owned(),
        beta.to_string_lossy().into_owned(),
    );
    let run = |args: &[&str]| {
        let mut argv: Vec<String> = vec!["tsk".into()];
        argv.extend(args.iter().map(|arg| arg.to_string()));
        argv.extend(["--state-dir".to_string(), state_dir.clone()]);
        cli(argv)
    };
    assert_eq!(run(&["add", "-t", "in beta", "-p", &beta]).code, 0);
    assert_eq!(run(&["add", "-t", "seed zeta", "-p", &zeta]).code, 0);
    assert_eq!(run(&["project", "title", &zeta, "Alpha"]).code, 0);

    for outcome in ["created", "existing"] {
        let added = run(&["add", "-t", "in alpha", "-p", "alpha", "--json"]);
        assert_eq!(added.code, 0, "{added:?}");
        let value: serde_json::Value = serde_json::from_str(&added.stdout).expect("json");
        assert_eq!(value["outcome"], outcome);
        assert_eq!(value["project"], zeta.as_str());
        assert_eq!(value["project_title"], "Alpha", "{outcome}");
    }
    let untitled = run(&["add", "-t", "in beta", "-p", &beta, "--json"]);
    let value: serde_json::Value = serde_json::from_str(&untitled.stdout).expect("json");
    assert!(value.get("project_title").is_none(), "{value}");

    let all = run(&["list", "--all"]);
    assert_eq!(all.code, 0, "{all:?}");
    assert!(all.stdout.contains("  Alpha\n"), "{}", all.stdout);
    assert!(all.stdout.contains("  beta\n"), "{}", all.stdout);
    assert!(!all.stdout.contains("  zeta\n"), "{}", all.stdout);

    // By folder, beta sorts before zeta; by shown name, Alpha comes first.
    let sorted = run(&["list", "--all", "--sort", "project"]);
    assert_eq!(sorted.code, 0, "{sorted:?}");
    let alpha_at = sorted.stdout.find("in alpha").expect("alpha row");
    let beta_at = sorted.stdout.find("in beta").expect("beta row");
    assert!(alpha_at < beta_at, "{}", sorted.stdout);
}
