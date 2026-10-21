//! Dispatch CLI: routing, parsing, stable refusal codes, and persistence.

#[cfg(unix)]
use crate::stub;
use std::fs;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};

use std::path::{Path, PathBuf};

use tsk_tui::cli::parser::TaskAddress;
use tsk_tui::cli::run_with;
use tsk_tui::cli::{clean, dispatch};
use tsk_tui::dispatch::{CleanupInspection, CreatedWorktree, DispatchHost, WorktreeCleanup};
use tsk_tui::domain::{DomainState, HumanStatus, ProvenanceOrigin, TaskScope};
use tsk_tui::store::TaskStore;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// A real host whose launchers and removal marks land in a fresh temp dir, never the real store.
fn system_host() -> tsk_tui::dispatch::SystemDispatchHost {
    tsk_tui::dispatch::SystemDispatchHost::in_state_dir(std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-host-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )))
}

#[derive(Default)]
struct FakeHost {
    inspection: Option<CleanupInspection>,
    removed: usize,
    /// A store and document a whole-store restore swaps in while the agent launches.
    restore_during_launch: Option<(PathBuf, Vec<u8>)>,
}

impl DispatchHost for FakeHost {
    fn is_git_repo(&mut self, _project: &Path) -> Result<bool, String> {
        Ok(true)
    }

    fn resolve_base(&mut self, _project: &Path) -> Result<String, String> {
        Ok("main".into())
    }

    fn resolve_base_choice(
        &mut self,
        _project: &Path,
        explicit: Option<&str>,
    ) -> Result<tsk_tui::git_base::ResolvedBase, String> {
        let reference = explicit.unwrap_or("main");
        Ok(tsk_tui::git_base::ResolvedBase {
            full_ref: format!("refs/heads/{reference}"),
            reference: reference.into(),
            commit: None,
            remote: None,
            warning: None,
        })
    }

    fn create_worktree(
        &mut self,
        _project: &Path,
        branch: &str,
        _base: Option<&str>,
        _label: &str,
    ) -> Result<CreatedWorktree, String> {
        Ok(CreatedWorktree {
            path: PathBuf::from("/tmp/T1-worktree"),
            branch: branch.into(),
            workspace_id: "workspace-1".into(),
            root_pane_id: "pane-1".into(),
        })
    }

    fn inspect_cleanup(
        &mut self,
        _project: &Path,
        _dispatch: &tsk_tui::domain::Dispatch,
        _in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        self.inspection
            .clone()
            .ok_or_else(|| "inspection missing".into())
    }

    fn remove_herdr_worktree(&mut self, _workspace_id: &str) -> Result<(), String> {
        self.removed += 1;
        Ok(())
    }

    fn delete_branch(&mut self, _project: &Path, _branch: &str) -> Result<(), String> {
        Ok(())
    }

    fn root_pane(
        &mut self,
        _workspace_id: &str,
    ) -> Result<String, tsk_tui::dispatch::RootPaneError> {
        Ok("pane-1".into())
    }

    fn run_in_pane(&mut self, _pane_id: &str, _command: &str) -> Result<(), String> {
        if let Some((dir, document)) = self.restore_during_launch.take() {
            TaskStore::new(dir)
                .restore_document(&document, std::time::SystemTime::now())
                .expect("restore while the agent launches");
        }
        Ok(())
    }

    fn launcher_path(&self, workspace_id: &str) -> Result<std::path::PathBuf, String> {
        Ok(format!("/tsk/launchers/dispatch-{workspace_id}.sh").into())
    }

    fn write_launcher(
        &mut self,
        workspace_id: &str,
        _script: &str,
    ) -> Result<std::path::PathBuf, String> {
        self.launcher_path(workspace_id)
    }
}

#[test]
fn dispatch_requires_a_task_and_accepts_again() {
    let missing = run_with(["tsk", "dispatch"], Cursor::new(Vec::<u8>::new()), true);
    assert_eq!(missing.code, 2);
    assert!(missing.stderr.contains("task number is required"));

    let help = run_with(
        ["tsk", "dispatch", "--help"],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(help.code, 0, "{}", help.stderr);
    assert!(help.stdout.contains("usage: tsk dispatch <task> [--again]"));
    assert!(help.stdout.contains("already-dispatched"));
}

#[test]
fn successful_cli_dispatch_persists_record_and_started_together() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-success-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("config.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    store.save(&state).expect("save");

    let result = dispatch::run_with_host(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        &mut FakeHost::default(),
    )
    .expect("dispatch");
    assert_eq!(result.number, 1);
    let saved = store.load().expect("reload");
    let task = saved.tasks().first().expect("task");
    assert_eq!(task.status, HumanStatus::Started);
    let record = task.dispatch.as_ref().expect("record");
    assert_eq!(record.herdr_workspace_id, "workspace-1");
    assert_eq!(record.base.as_deref(), Some("main"));
    assert_eq!(record.base_commit, None);
    assert_eq!(record.base_ref.as_deref(), Some("refs/heads/main"));
    let relaunched = dispatch::run_with_host(
        TaskAddress::Number(1),
        true,
        Some(dir.clone()),
        true,
        &mut FakeHost::default(),
    )
    .unwrap();
    assert_eq!(
        relaunched.record.base_ref.as_deref(),
        Some("refs/heads/main")
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

/// A restore that lands while `tsk dispatch` launches replaces the store the command loaded.
/// Its save must refuse rather than write back the task the restore removed.
#[test]
fn a_restore_during_a_dispatch_refuses_its_save_and_keeps_the_restored_store() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-restore-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("config.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "backed up",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let backup = fs::read(dir.join("tsk.json")).expect("backup");
    let mut state = store.load().expect("load");
    state
        .create_assigned(
            "created after the backup",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    store.reload_merge_save(&mut state).expect("save");

    let mut host = FakeHost {
        restore_during_launch: Some((dir.clone(), backup.clone())),
        ..FakeHost::default()
    };
    let error = dispatch::run_with_host(
        TaskAddress::Number(2),
        false,
        Some(dir.clone()),
        true,
        &mut host,
    )
    .expect_err("the restore replaced the store dispatch loaded");
    assert_eq!(error.code(), "store-error");
    let message = error.to_string();
    assert!(
        message.starts_with("the store was restored from a backup while this command ran"),
        "{message}"
    );
    assert!(
        message.ends_with("; launch undone: run it again"),
        "{message}"
    );
    let restored = store.load().expect("load");
    let titles: Vec<&str> = restored
        .tasks()
        .iter()
        .map(|task| task.title.as_str())
        .collect();
    assert_eq!(titles, ["backed up"], "the dispatch wrote nothing back");
    assert_eq!(restored.next_task_number, 3, "numbering stays past T2");
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn cli_dispatch_one_off_base_reaches_core_without_editing_task_base() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-base-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("config.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    store.save(&state).expect("save");

    let result = dispatch::run_with_host_base(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        Some("release"),
        &mut FakeHost::default(),
    )
    .expect("dispatch");
    assert_eq!(result.record.base.as_deref(), Some("release"));
    let loaded = store.load().expect("reload");
    let task = &loaded.tasks()[0];
    assert_eq!(task.base, None, "one-off override must not edit the task");
    assert_eq!(
        task.dispatch
            .as_ref()
            .and_then(|record| record.base.as_deref()),
        Some("release")
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn clean_cli_help_and_success_report_each_removed_resource() {
    let help = run_with(
        ["tsk", "clean", "--help"],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(help.code, 0, "{}", help.stderr);
    assert!(help
        .stdout
        .contains("usage: tsk clean <task> [--merge] [--json]"));
    for code in [
        "unknown-task",
        "not-dispatched",
        "already-cleaned",
        "dirty-worktree",
        "herdr-failed",
        "store-error",
        "gh-unavailable",
        "no-pr",
        "pr-not-open",
        "pr-base-mismatch",
        "merge-failed",
        "merge-config",
    ] {
        assert!(help.stdout.contains(code), "missing {code}");
    }

    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-clean-success-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(
        dir.join("config.toml"),
        "[agent.implementer]\ncommand = [\"runner\", \"{prompt}\"]\n",
    )
    .expect("profiles");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create_assigned(
            "assigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
            Some("implementer".into()),
        )
        .expect("task");
    let mut launch = FakeHost::default();
    store.save(&state).expect("save");
    dispatch::run_with_host(
        TaskAddress::Number(1),
        false,
        Some(dir.clone()),
        true,
        &mut launch,
    )
    .expect("dispatch");

    let mut host = FakeHost {
        inspection: Some(CleanupInspection {
            unreachable_remote: None,
            warning: None,
            base_available: true,
            worktree_exists: true,
            dirty: false,
            dirty_files: Vec::new(),
            checked_out_elsewhere: false,
            branch_merged: true,
            workspace_exists: true,
            target_matches: true,
        }),
        ..FakeHost::default()
    };
    let result = clean::run_with_host(TaskAddress::Number(1), Some(dir.clone()), true, &mut host)
        .expect("clean");
    assert_eq!(result.worktree, WorktreeCleanup::Removed);
    assert_eq!(host.removed, 1);
    assert!(
        store.load().unwrap().tasks()[0]
            .dispatch
            .as_ref()
            .unwrap()
            .cleaned
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn clean_cli_refusals_print_stable_codes() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-clean-refusal-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "plain",
            None,
            TaskScope::Global,
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let output = run_with(
        ["tsk", "clean", "T1", "--state-dir", dir.to_str().unwrap()],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk clean: not-dispatched: task has no dispatch to clean\n"
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn dispatch_refusals_print_stable_codes_and_persist_nothing() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-dispatch-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let store = TaskStore::new(&dir);
    let mut state = DomainState::new();
    state
        .create(
            "unassigned",
            None,
            TaskScope::Project {
                path: "/repos/app".into(),
            },
            ProvenanceOrigin::Manual,
            None,
        )
        .expect("task");
    store.save(&state).expect("save");
    let before = fs::read(dir.join("tsk.json")).expect("state bytes");

    let output = run_with(
        [
            "tsk",
            "dispatch",
            "T1",
            "--state-dir",
            dir.to_str().expect("utf-8 path"),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk dispatch: no-assignee: no agent assigned, use tsk edit T<n> --assignee <name>\n"
    );
    assert_eq!(fs::read(dir.join("tsk.json")).expect("after"), before);

    let unknown = run_with(
        [
            "tsk",
            "dispatch",
            "T9",
            "--state-dir",
            dir.to_str().expect("utf-8 path"),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(unknown.code, 1);
    assert!(unknown.stderr.starts_with("tsk dispatch: unknown-task:"));
    fs::remove_dir_all(dir).expect("cleanup");
}

/// The Windows launcher hands a hostile prompt and the profile's env to a real program
/// (`tsk.exe`, which stores the prompt as a task's notes) through Windows PowerShell 5.1
/// exactly: quotes, `$`, `%`, cmd metacharacters, a newline, backslashes, non-ASCII.
#[cfg(windows)]
#[test]
fn windows_launcher_delivers_argv_and_env_verbatim_through_powershell() {
    use tsk_tui::agents::{AgentProfile, RenderContext};

    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-launcher-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let state = dir.join("state dir");
    fs::create_dir_all(&state).expect("mkdir");
    let prompt = "say \"hi there\" and 'single' \\\"pre-escaped\\\"\nnew line $HOME %PATH% \
                  ^ & | < > `tick` C:\\path with space\\ caf\u{e9} \u{6771}\u{4eac} end\\";
    let profile = AgentProfile {
        command: vec![
            env!("CARGO_BIN_EXE_tsk").into(),
            "add".into(),
            "--desk".into(),
            "-t".into(),
            "{title}".into(),
            "-n".into(),
        ],
        prompt: Some(prompt.into()),
        env: [(
            "TSK_STATE_DIR".to_string(),
            state.to_string_lossy().into_owned(),
        )]
        .into(),
    };
    let rendered = profile.render(&RenderContext {
        number: 1,
        title: "a \"quoted\" title\\",
        notes: "",
        steps: "",
        worktree: "C:\\w",
        branch: "tsk/t1",
        base: "main",
    });
    let launcher = dir.join("launcher.ps1");
    fs::write(&launcher, rendered.powershell_script()).expect("launcher");

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&launcher)
        .output()
        .expect("powershell");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!launcher.exists(), "the launcher deletes itself");

    let tasks = TaskStore::new(&state).load().expect("store");
    let task = &tasks.tasks()[0];
    assert_eq!(task.title, "a \"quoted\" title\\");
    assert_eq!(task.notes.as_deref(), Some(prompt));
    fs::remove_dir_all(dir).expect("cleanup");
}

/// Run a rendered profile's Windows launcher with `bin` first on PATH, as the pane would.
#[cfg(windows)]
fn run_windows_launcher(
    dir: &Path,
    bin: &Path,
    profile: &tsk_tui::agents::AgentProfile,
    title: &str,
) -> std::process::Output {
    let rendered = profile.render(&tsk_tui::agents::RenderContext {
        number: 1,
        title,
        notes: "",
        steps: "",
        worktree: "C:\\w",
        branch: "tsk/t1",
        base: "main",
    });
    let launcher = dir.join(format!(
        "launcher-{}.ps1",
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&launcher, rendered.powershell_script()).expect("launcher");
    let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("PATH");
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&launcher)
        .env("PATH", path)
        .output()
        .expect("powershell");
    assert!(!launcher.exists(), "the launcher deletes itself");
    output
}

/// npm installs an agent as `agent.ps1` and `agent.cmd` shims that forward their arguments to a
/// native program. The launcher picks the PowerShell shim, and a hostile multiline prompt, an
/// empty argument, and trailing backslashes reach the native program (`tsk.exe`) intact.
#[cfg(windows)]
#[test]
fn windows_launcher_hands_arguments_through_an_npm_powershell_shim_intact() {
    use tsk_tui::agents::AgentProfile;

    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-shim-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let (bin, state) = (dir.join("bin"), dir.join("state"));
    fs::create_dir_all(&bin).expect("bin");
    fs::create_dir_all(&state).expect("state");
    let ran_batch = dir.join("ran-batch.txt");
    fs::write(
        bin.join("agent.ps1"),
        format!(
            "#!/usr/bin/env pwsh\r\n& \"{}\" $args\r\nexit $LASTEXITCODE\r\n",
            env!("CARGO_BIN_EXE_tsk")
        ),
    )
    .expect("ps1 shim");
    fs::write(
        bin.join("agent.cmd"),
        format!("@echo off\r\necho ran > \"{}\"\r\n", ran_batch.display()),
    )
    .expect("cmd shim");
    let env = [(
        "TSK_STATE_DIR".to_string(),
        state.to_string_lossy().into_owned(),
    )];
    let prompt = "say \"hi there\" & echo x | y\nnext line $HOME %PATH% `t` end\\";

    // The prompt as notes, after a title with a trailing backslash and a space.
    let notes = AgentProfile {
        command: ["agent", "add", "--desk", "-t", "{title}", "-n"]
            .map(String::from)
            .to_vec(),
        prompt: Some(prompt.into()),
        env: env.clone().into(),
    };
    let output = run_windows_launcher(&dir, &bin, &notes, "first title\\");
    assert!(output.status.success(), "{output:?}");
    // An empty argument: dropped, `-n` would take `-t` and the add would fail.
    let empty = AgentProfile {
        command: ["agent", "add", "--desk", "-n", "", "-t"]
            .map(String::from)
            .to_vec(),
        prompt: Some("second".into()),
        env: env.into(),
    };
    let output = run_windows_launcher(&dir, &bin, &empty, "unused");
    assert!(output.status.success(), "{output:?}");

    let tasks = TaskStore::new(&state).load().expect("store");
    let tasks = tasks.tasks();
    assert_eq!(tasks[0].title, "first title\\");
    assert_eq!(tasks[0].notes.as_deref(), Some(prompt));
    assert_eq!(tasks[1].title, "second");
    assert!(!ran_batch.exists(), "the .cmd shim never runs");
    fs::remove_dir_all(dir).expect("cleanup");
}

/// A profile that resolves only to a batch file would hand task text to cmd.exe, where a quote
/// in a title ends quoting and `&` runs a command. The launcher refuses and starts nothing.
#[cfg(windows)]
#[test]
fn windows_launcher_refuses_a_batch_only_program_before_cmd_sees_task_text() {
    use tsk_tui::agents::AgentProfile;

    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-batch-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let (ran, injected) = (dir.join("ran.txt"), dir.join("injected.txt"));
    fs::write(
        bin.join("batchonly.cmd"),
        format!("@echo off\r\necho ran > \"{}\"\r\n", ran.display()),
    )
    .expect("cmd");
    let profile = AgentProfile {
        command: vec!["batchonly".into()],
        prompt: Some("{title}".into()),
        env: Default::default(),
    };
    let title = format!("x\" & echo pwned > \"{}\" & rem \"", injected.display());

    let output = run_windows_launcher(&dir, &bin, &profile, &title);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tsk: not starting batchonly: it resolves only to the batch file"),
        "{stderr}"
    );
    assert!(!ran.exists(), "the batch file never starts");
    assert!(!injected.exists(), "task text never reaches cmd.exe");
    fs::remove_dir_all(dir).expect("cleanup");
}

/// Where Windows cleanup's in-use check moves a checkout for a moment.
fn checked_aside(worktree: &Path) -> PathBuf {
    let mut aside = worktree.as_os_str().to_owned();
    aside.push(".tsk-cleanup-check");
    PathBuf::from(aside)
}

/// A cleanup interrupted between the check's two renames leaves the checkout aside. The next
/// cleanup must put it back before it reads the worktree as missing and records it cleaned.
#[test]
fn cleanup_restores_a_checkout_left_aside_and_still_refuses_its_uncommitted_work() {
    let repo = CleanupRepo::new();
    fs::write(repo.worktree.join("work.txt"), "uncommitted").unwrap();
    fs::rename(&repo.worktree, checked_aside(&repo.worktree)).unwrap();
    let (mut state, id) = repo.state("base", None);

    let refused = tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut system_host())
        .expect_err("dirty work is never cleaned");
    assert_eq!(refused, tsk_tui::dispatch::CleanupError::DirtyWorktree);
    assert_eq!(
        fs::read_to_string(repo.worktree.join("work.txt")).unwrap(),
        "uncommitted"
    );
    assert!(!checked_aside(&repo.worktree).exists());
    let record = state.get(id).unwrap().dispatch.as_ref().unwrap();
    assert!(!record.cleaned, "the dispatch stays live");
    fs::remove_dir_all(&repo.root).unwrap();
}

#[test]
fn cleanup_restores_a_clean_checkout_left_aside_and_keeps_its_unmerged_branch() {
    let repo = CleanupRepo::new();
    repo.commit_in_worktree("unmerged work");
    fs::rename(&repo.worktree, checked_aside(&repo.worktree)).unwrap();

    let result = repo.clean("base", None);
    assert_eq!(
        result.worktree,
        WorktreeCleanup::Removed,
        "removed, not missing"
    );
    assert_eq!(
        result.branch_reason,
        Some(tsk_tui::dispatch::BranchRetentionReason::NotMerged)
    );
    assert!(!repo.worktree.exists() && !checked_aside(&repo.worktree).exists());
    assert_eq!(
        repo.git(&["branch", "--list", "tsk/t1-clean"]),
        "tsk/t1-clean"
    );
    fs::remove_dir_all(&repo.root).unwrap();
}

/// The removal mark of the dispatch `CleanupRepo` records (workspace `w1`).
fn removal_mark(repo: &CleanupRepo) -> PathBuf {
    repo.root
        .join("state")
        .join("cleanups")
        .join("dispatch-w1.removing")
}

/// A Windows removal git never finished (a board quit ended tsk, or git stopped midway) left
/// committed files deleted in a still-registered worktree. The next cleanup reports a partial
/// removal, never "uncommitted changes", and its non-destructive advice converges.
#[test]
fn an_unfinished_removal_reads_as_partly_removed_not_as_uncommitted_work() {
    use tsk_tui::dispatch::{CleanupError, DispatchHost, SystemDispatchHost};

    let repo = CleanupRepo::new();
    repo.commit_files(&[("tracked.txt", "x")]);
    let (mut state, id) = repo.state("base", None);
    let mut host = SystemDispatchHost::in_state_dir(repo.root.join("state"));

    // Without a started removal, a deleted tracked file is uncommitted work.
    fs::remove_file(repo.worktree.join("tracked.txt")).unwrap();
    assert_eq!(
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host),
        Err(CleanupError::DirtyWorktree)
    );

    host.mark_removal("w1", true);
    let refused = tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host)
        .expect_err("partly removed");
    assert_eq!(refused.code(), "partly-removed");
    assert!(
        refused.to_string().contains("restore . and clean again"),
        "{refused}"
    );
    assert!(!state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);

    // Restoring as told lets the next cleanup run normally and converge.
    repo.in_worktree(&["restore", "."]);
    let result =
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host).expect("cleaned");
    assert_eq!(result.worktree, WorktreeCleanup::Removed);
    assert!(state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);
    assert!(!removal_mark(&repo).exists());
    fs::remove_dir_all(&repo.root).unwrap();
}

/// With a started removal marked, any change git's removal cannot make (it deletes files
/// and never touches the index) is uncommitted work: refused as dirty, nothing deleted, the
/// dispatch live, and the mark cleared so it never vouches for later changes.
#[test]
fn a_marked_removal_never_reads_real_work_as_partly_removed() {
    use tsk_tui::dispatch::{CleanupError, DispatchHost, SystemDispatchHost};

    type Change = fn(&CleanupRepo);
    /// Each file afterwards: its content, or `None` when it is gone.
    type After = &'static [(&'static str, Option<&'static str>)];
    let cases: [(&str, Change, After); 4] = [
        (
            "modified",
            |repo| fs::write(repo.worktree.join("a.txt"), "edited").unwrap(),
            &[("a.txt", Some("edited")), ("b.txt", Some("b"))],
        ),
        (
            "untracked",
            |repo| fs::write(repo.worktree.join("new.txt"), "new").unwrap(),
            &[("new.txt", Some("new")), ("a.txt", Some("a"))],
        ),
        (
            "staged deletion",
            |repo| repo.in_worktree(&["rm", "-q", "a.txt"]),
            &[("a.txt", None), ("b.txt", Some("b"))],
        ),
        (
            "deletion and edit",
            |repo| {
                fs::remove_file(repo.worktree.join("a.txt")).unwrap();
                fs::write(repo.worktree.join("b.txt"), "edited").unwrap();
            },
            &[("a.txt", None), ("b.txt", Some("edited"))],
        ),
    ];
    for (label, change, after) in cases {
        let repo = CleanupRepo::new();
        repo.commit_files(&[("a.txt", "a"), ("b.txt", "b")]);
        let (mut state, id) = repo.state("base", None);
        let mut host = SystemDispatchHost::in_state_dir(repo.root.join("state"));
        host.mark_removal("w1", true);
        change(&repo);

        assert_eq!(
            tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host),
            Err(CleanupError::DirtyWorktree),
            "{label}"
        );
        for (file, content) in after {
            let found = fs::read_to_string(repo.worktree.join(file)).ok();
            assert_eq!(found.as_deref(), *content, "{label}: {file}");
        }
        assert!(
            !state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned,
            "{label}"
        );
        assert!(!removal_mark(&repo).exists(), "{label}: mark cleared");
        fs::remove_dir_all(&repo.root).unwrap();
    }
}

/// A mark left by a run that ended before git started (an older build, or a crash) proves
/// nothing: a deletion made afterwards is the user's work.
#[test]
fn a_stale_removal_mark_never_vouches_for_a_later_deletion() {
    use tsk_tui::dispatch::{CleanupError, SystemDispatchHost};

    let repo = CleanupRepo::new();
    repo.commit_files(&[("tracked.txt", "x")]);
    let (mut state, id) = repo.state("base", None);
    fs::create_dir_all(removal_mark(&repo).parent().unwrap()).unwrap();
    fs::write(removal_mark(&repo), "").unwrap();
    fs::remove_file(repo.worktree.join("tracked.txt")).unwrap();

    let mut host = SystemDispatchHost::in_state_dir(repo.root.join("state"));
    assert_eq!(
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host),
        Err(CleanupError::DirtyWorktree)
    );
    assert!(!state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);
    assert!(!removal_mark(&repo).exists(), "the stale mark is cleared");
    fs::remove_dir_all(&repo.root).unwrap();
}

/// The real host behind a Windows cleanup whose agent writes one more file as its workspace
/// closes, after the inspection found the worktree clean.
struct AgentWritesOnClose {
    inner: tsk_tui::dispatch::SystemDispatchHost,
    worktree: PathBuf,
}

impl DispatchHost for AgentWritesOnClose {
    fn is_git_repo(&mut self, project: &Path) -> Result<bool, String> {
        self.inner.is_git_repo(project)
    }
    fn create_worktree(
        &mut self,
        project: &Path,
        branch: &str,
        base: Option<&str>,
        label: &str,
    ) -> Result<CreatedWorktree, String> {
        self.inner.create_worktree(project, branch, base, label)
    }
    fn root_pane(
        &mut self,
        workspace_id: &str,
    ) -> Result<String, tsk_tui::dispatch::RootPaneError> {
        self.inner.root_pane(workspace_id)
    }
    fn run_in_pane(&mut self, pane_id: &str, command: &str) -> Result<(), String> {
        self.inner.run_in_pane(pane_id, command)
    }
    fn platform(&self) -> tsk_tui::dispatch::HostPlatform {
        tsk_tui::dispatch::HostPlatform::Windows
    }
    fn inspect_cleanup(
        &mut self,
        project: &Path,
        dispatch: &tsk_tui::domain::Dispatch,
        _: bool,
    ) -> Result<CleanupInspection, String> {
        let inspection = self.inner.inspect_cleanup(project, dispatch, false)?;
        Ok(CleanupInspection {
            workspace_exists: true,
            ..inspection
        })
    }
    fn close_herdr_workspace(&mut self, _: &str) -> Result<(), String> {
        fs::write(self.worktree.join("agent-notes.txt"), "written on exit").unwrap();
        Ok(())
    }
    fn worktree_dirty(&mut self, worktree: &Path) -> Result<bool, String> {
        self.inner.worktree_dirty(worktree)
    }
    fn remove_git_worktree(&mut self, project: &Path, worktree: &Path) -> Result<(), String> {
        self.inner.remove_git_worktree(project, worktree)
    }
    fn remove_marked_worktree(
        &mut self,
        project: &Path,
        worktree: &Path,
        workspace_id: &str,
    ) -> Result<(), String> {
        self.inner
            .remove_marked_worktree(project, worktree, workspace_id)
    }
    fn mark_removal(&mut self, workspace_id: &str, started: bool) {
        self.inner.mark_removal(workspace_id, started)
    }
    fn removal_state(
        &mut self,
        project: &Path,
        worktree: &Path,
    ) -> Result<tsk_tui::dispatch::RemovalState, String> {
        self.inner.removal_state(project, worktree)
    }
    fn interrupted_removal(
        &mut self,
        project: &Path,
        worktree: &Path,
        workspace_id: &str,
    ) -> Result<Option<tsk_tui::dispatch::RemovalState>, String> {
        self.inner
            .interrupted_removal(project, worktree, workspace_id)
    }
}

#[test]
fn work_written_after_the_inspection_is_refused_as_uncommitted_and_kept() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("tracked.txt", "x")]);
    let (mut state, id) = repo.state("base", None);
    let mut host = AgentWritesOnClose {
        inner: tsk_tui::dispatch::SystemDispatchHost::in_state_dir(repo.root.join("state")),
        worktree: repo.worktree.clone(),
    };
    assert_eq!(
        tsk_tui::dispatch::clean_with_host(&mut state, id, true, &mut host),
        Err(tsk_tui::dispatch::CleanupError::DirtyWorktree)
    );
    assert_eq!(
        fs::read_to_string(repo.worktree.join("agent-notes.txt")).unwrap(),
        "written on exit"
    );
    assert_eq!(
        fs::read_to_string(repo.worktree.join("tracked.txt")).unwrap(),
        "x"
    );
    assert!(!state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned);
    assert!(!removal_mark(&repo).exists());
    fs::remove_dir_all(&repo.root).unwrap();
}

struct CleanupRepo {
    root: PathBuf,
    project: PathBuf,
    worktree: PathBuf,
}

impl CleanupRepo {
    fn new() -> Self {
        tsk_tui::git_base::stretch_default_deadlines_for_tests();
        let root = std::env::temp_dir().join(format!(
            "tsk-cli-clean-git-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let project = root.join("repo");
        let worktree = root.join("worktree");
        fs::create_dir_all(&project).unwrap();
        let repo = Self {
            root,
            project,
            worktree,
        };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        repo.git(&["branch", "base"]);
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "tsk/t1-clean",
            repo.worktree.to_str().unwrap(),
            "base",
        ]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn state(&self, base: &str, remote: Option<&str>) -> (DomainState, uuid::Uuid) {
        let mut state = DomainState::new();
        let id = state
            .create(
                "cleanup",
                None,
                TaskScope::Project {
                    path: self.project.to_string_lossy().into(),
                },
                ProvenanceOrigin::Manual,
                None,
            )
            .unwrap();
        let store = TaskStore::new(self.root.join("state"));
        store.save(&state).unwrap();
        state = store.load().unwrap();
        state
            .record_dispatch(
                id,
                tsk_tui::domain::Dispatch {
                    argv: vec!["agent".into()],
                    worktree: self.worktree.to_string_lossy().into(),
                    branch: "tsk/t1-clean".into(),
                    base: Some(base.into()),
                    base_ref: Some(format!(
                        "refs/{}/{}",
                        if remote.is_some() { "remotes" } else { "heads" },
                        base
                    )),
                    base_commit: None,
                    base_remote: remote.map(str::to_string),
                    herdr_workspace_id: "w1".into(),
                    at: std::time::SystemTime::now(),
                    cleaned: false,
                },
            )
            .unwrap();
        (state, id)
    }

    /// `origin/main` cached locally at the task's start, with `origin` unreachable.
    fn offline_remote_base(&self) {
        self.git(&[
            "remote",
            "add",
            "origin",
            self.root.join("missing.git").to_str().unwrap(),
        ]);
        self.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    }

    fn commit_in_worktree(&self, message: &str) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.worktree)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                message,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    fn in_worktree(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.worktree)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }

    /// Commit `files` (name, content) on the task branch.
    fn commit_files(&self, files: &[(&str, &str)]) {
        for (name, content) in files {
            fs::write(self.worktree.join(name), content).unwrap();
            self.in_worktree(&["add", name]);
        }
        self.in_worktree(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "work",
        ]);
    }

    fn clean(&self, base: &str, remote: Option<&str>) -> tsk_tui::dispatch::CleanupResult {
        let (mut state, id) = self.state(base, remote);
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut system_host())
            .expect("clean worktree even when base vanished")
    }
}

impl Drop for CleanupRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn cleanup_removes_clean_worktree_when_local_base_vanished() {
    let repo = CleanupRepo::new();
    repo.git(&["branch", "-D", "base"]);
    let result = repo.clean("base", None);
    assert_eq!(result.worktree, WorktreeCleanup::Removed);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    assert!(!repo.worktree.exists());
    let output = tsk_tui::cli::presenter::cleaned(result, false);
    assert!(
        output.stdout.contains("base no longer available"),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains("squash-merged?"));
}

#[test]
fn cleanup_removes_clean_worktree_when_fetch_prunes_base() {
    let repo = CleanupRepo::new();
    let remote = repo.root.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "base"]);
    repo.git(&["config", "fetch.prune", "true"]);
    repo.git(&["push", "-q", "origin", ":base"]);
    // Keep a cached tracking ref until cleanup's fetch prunes it.
    repo.git(&["update-ref", "refs/remotes/origin/base", "base"]);
    let result = repo.clean("origin/base", Some("origin"));
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    let output = tsk_tui::cli::presenter::cleaned(result, true);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert!(value["branch"]["reason"]
        .as_str()
        .unwrap()
        .contains("base no longer available"));
}

#[test]
fn cleanup_offline_keeps_the_branch_and_says_why_in_human_and_json() {
    for merged in [true, false] {
        let repo = CleanupRepo::new();
        repo.offline_remote_base();
        if !merged {
            repo.commit_in_worktree("unmerged");
        }
        let result = repo.clean("origin/main", Some("origin"));
        // Cached ancestry says merged, but the remote may have dropped the task commits since:
        // only the clean worktree goes.
        assert_eq!(result.worktree, WorktreeCleanup::Removed);
        assert!(!repo.worktree.exists());
        assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
        assert_eq!(
            result.branch_reason,
            Some(tsk_tui::dispatch::BranchRetentionReason::RemoteUnreachable)
        );
        assert!(!repo.git(&["branch", "--list", "tsk/t1-clean"]).is_empty());
        let human = tsk_tui::cli::presenter::cleaned(result.clone(), false);
        assert!(
            human
                .stdout
                .contains("(kept, could not reach origin to confirm the merge)"),
            "{}",
            human.stdout
        );
        assert!(
            human.stdout.contains("warning: fetch failed (offline"),
            "{}",
            human.stdout
        );
        let json = tsk_tui::cli::presenter::cleaned(result, true);
        let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
        assert_eq!(value["branch"]["outcome"], "kept");
        assert_eq!(
            value["branch"]["reason"],
            "could not reach origin to confirm the merge"
        );
        assert!(value["warning"]
            .as_str()
            .unwrap()
            .contains("merged status not confirmed"));
    }
}

#[test]
fn status_done_clean_keeps_the_branch_when_the_remote_is_unreachable() {
    let repo = CleanupRepo::new();
    repo.offline_remote_base();
    let (state, _) = repo.state("origin/main", Some("origin"));
    let dir = repo.root.join("state");
    TaskStore::new(dir.clone()).save(&state).unwrap();
    let output = run_with(
        [
            "tsk",
            "status",
            "T1",
            "done",
            "--clean",
            "--state-dir",
            dir.to_str().unwrap(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(
        output
            .stdout
            .contains("(kept, could not reach origin to confirm the merge)"),
        "{}",
        output.stdout
    );
    assert!(!repo.worktree.exists());
    assert!(!repo.git(&["branch", "--list", "tsk/t1-clean"]).is_empty());
}

#[test]
fn status_done_clean_on_an_already_cleaned_dispatch_succeeds_with_nothing_to_clean() {
    let repo = CleanupRepo::new();
    let (mut state, id) = repo.state("base", None);
    state
        .record_dispatch_cleaned(id, tsk_tui::domain::CleanupOutcome::Removed)
        .unwrap();
    let dir = repo.root.join("state");
    TaskStore::new(dir.clone()).save(&state).unwrap();
    let output = run_with(
        [
            "tsk",
            "status",
            "T1",
            "done",
            "--clean",
            "--state-dir",
            dir.to_str().unwrap(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(output.stdout, "status T1 done cleanup\nnothing to clean\n");
    assert_eq!(output.stderr, "");
    // The explicit verb still refuses: there the user asked for cleanup specifically.
    let explicit = run_with(
        ["tsk", "clean", "T1", "--state-dir", dir.to_str().unwrap()],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(explicit.code, 1);
    assert_eq!(
        explicit.stderr,
        "tsk clean: already-cleaned: dispatch is already cleaned\n"
    );
}

#[test]
fn status_done_clean_still_refuses_a_dirty_worktree_after_done_persists() {
    let repo = CleanupRepo::new();
    let (state, _) = repo.state("base", None);
    let dir = repo.root.join("state");
    TaskStore::new(dir.clone()).save(&state).unwrap();
    fs::write(repo.worktree.join("wip.txt"), "unsaved").unwrap();
    let output = run_with(
        [
            "tsk",
            "status",
            "T1",
            "done",
            "--clean",
            "--state-dir",
            dir.to_str().unwrap(),
        ],
        Cursor::new(Vec::<u8>::new()),
        true,
    );
    assert_eq!(output.code, 1);
    assert!(
        output.stderr.starts_with("tsk status: dirty-worktree:"),
        "{}",
        output.stderr
    );
    assert!(repo.worktree.exists());
    let task = TaskStore::new(&dir).load().unwrap().tasks()[0].clone();
    assert_eq!(task.status, tsk_tui::domain::HumanStatus::Done);
}

#[test]
fn cleanup_inside_the_fetch_window_trusts_the_fresh_refs() {
    let repo = CleanupRepo::new();
    let remote = repo.root.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "main"]);
    tsk_tui::git_base::fetch_remote(&repo.project, "origin").unwrap();
    // The remote vanishes after a successful fetch: inside the window cleanup never fetches,
    // so the fresh refs confirm the merge and the branch goes.
    fs::remove_dir_all(&remote).unwrap();
    let result = repo.clean("origin/main", Some("origin"));
    assert_eq!(result.warning, None);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Removed);
    assert!(repo.git(&["branch", "--list", "tsk/t1-clean"]).is_empty());
}

#[test]
fn cleanup_with_a_local_only_base_deletes_a_merged_branch_offline() {
    let repo = CleanupRepo::new();
    // A remote that cannot be reached is irrelevant: the local base has no upstream.
    repo.git(&[
        "remote",
        "add",
        "origin",
        repo.root.join("missing.git").to_str().unwrap(),
    ]);
    let result = repo.clean("base", None);
    assert_eq!(result.warning, None);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Removed);
    assert!(!repo.worktree.exists());
}

#[test]
fn cleanup_checked_out_merged_branch_has_truthful_reason() {
    let repo = CleanupRepo::new();
    repo.git(&["checkout", "-q", "tsk/t1-clean", "--ignore-other-worktrees"]);
    let result = repo.clean("base", None);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    let human = tsk_tui::cli::presenter::cleaned(result.clone(), false);
    assert!(
        human.stdout.contains("checked out in another worktree"),
        "{}",
        human.stdout
    );
    assert!(!human.stdout.contains("squash-merged?"));
    let json = tsk_tui::cli::presenter::cleaned(result, true);
    assert!(!json.stdout.contains("squash-merged?"));
}

#[test]
fn cleanup_keeps_exact_local_namespace_after_adding_same_named_remote() {
    for merged in [false, true] {
        let repo = CleanupRepo::new();
        repo.git(&["branch", "integration/main", "base"]);
        let (mut state, id) = repo.state("integration/main", None);
        let mut record = state.get(id).unwrap().dispatch.clone().unwrap();
        record.base_ref = Some("refs/heads/integration/main".into());
        if merged {
            record.base = None; // The exact ref is sufficient, the display label is optional.
        }
        state.record_dispatch(id, record).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo.worktree)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "unmerged into local base",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        if merged {
            repo.git(&["update-ref", "refs/heads/integration/main", "tsk/t1-clean"]);
        }
        let remote = repo.root.join("integration.git");
        assert!(std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&remote)
            .status()
            .unwrap()
            .success());
        repo.git(&["remote", "add", "integration", remote.to_str().unwrap()]);
        repo.git(&[
            "push",
            "-q",
            "integration",
            if merged {
                "base:main"
            } else {
                "tsk/t1-clean:main"
            },
        ]);
        let result =
            tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut system_host()).unwrap();
        assert_eq!(
            result.branch,
            if merged {
                tsk_tui::dispatch::BranchCleanup::Removed
            } else {
                tsk_tui::dispatch::BranchCleanup::Kept
            }
        );
        assert_eq!(
            result.branch_reason,
            if merged {
                None
            } else {
                Some(tsk_tui::dispatch::BranchRetentionReason::NotMerged)
            }
        );
        assert_eq!(
            result.warning, None,
            "an exact local record must not fetch a newly added remote"
        );
        if !merged {
            repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
        }
    }
}

struct AdvancingCleanupHost;

impl DispatchHost for AdvancingCleanupHost {
    fn is_git_repo(&mut self, _: &Path) -> Result<bool, String> {
        Ok(true)
    }
    fn create_worktree(
        &mut self,
        _: &Path,
        _: &str,
        _: Option<&str>,
        _: &str,
    ) -> Result<CreatedWorktree, String> {
        unreachable!()
    }
    fn root_pane(&mut self, _: &str) -> Result<String, tsk_tui::dispatch::RootPaneError> {
        unreachable!()
    }
    fn run_in_pane(&mut self, _: &str, _: &str) -> Result<(), String> {
        unreachable!()
    }
    fn inspect_cleanup(
        &mut self,
        project: &Path,
        record: &tsk_tui::domain::Dispatch,
        in_herdr: bool,
    ) -> Result<CleanupInspection, String> {
        system_host().inspect_cleanup(project, record, in_herdr)
    }
    fn remove_git_worktree(&mut self, project: &Path, worktree: &Path) -> Result<(), String> {
        system_host().remove_git_worktree(project, worktree)?;
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "refs/heads/tsk/t1-clean",
                "-m",
                "advanced after inspection",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let tip = String::from_utf8(output.stdout).unwrap();
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(["update-ref", "refs/heads/tsk/t1-clean", tip.trim()])
            .output()
            .unwrap();
        assert!(output.status.success());
        Ok(())
    }
    fn delete_merged_branch(
        &mut self,
        project: &Path,
        branch: &str,
        base: &str,
    ) -> Result<tsk_tui::dispatch::BranchDeletion, String> {
        system_host().delete_merged_branch(project, branch, base)
    }
}

#[test]
fn cleanup_advanced_merged_branch_has_truthful_reason() {
    let repo = CleanupRepo::new();
    let (mut state, id) = repo.state("base", None);
    let result =
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut AdvancingCleanupHost)
            .unwrap();
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    assert_eq!(
        result.branch_reason,
        Some(tsk_tui::dispatch::BranchRetentionReason::LatestTipNotMerged)
    );
    for json in [true, false] {
        let output = tsk_tui::cli::presenter::cleaned(result.clone(), json);
        assert!(
            output.stdout.contains("changed since inspection"),
            "{}",
            output.stdout
        );
        assert!(!output.stdout.contains("squash-merged?"));
    }
    repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
}

// Cleanup's status and ancestry reads use a dedicated ~10s deadline (`cleanup_query` in
// src/dispatch.rs), longer than git_base's 250ms metadata default: a real checkout's full
// untracked-file status, or an ancestry walk, can legitimately run past 250ms. These tests
// exercise both edges: a stall well past the dedicated deadline still refuses (and bounds
// the wait), while a stall under it still completes.

#[cfg(unix)]
#[test]
fn cleanup_git_status_is_bounded_even_when_fsmonitor_stalls() {
    let repo = CleanupRepo::new();
    let hook = repo.root.join("slow-fsmonitor");
    // Longer than cleanup's ~10s deadline, so the deadline (not the hook) ends the wait.
    stub::write_stub(&hook, "#!/bin/sh\nsleep 13\nprintf 'token\\0'\n", 0o700);
    repo.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    let (mut state, id) = repo.state("base", None);
    let start = std::time::Instant::now();
    let result = tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut system_host());
    let elapsed = start.elapsed();
    assert!(result.is_err(), "stalled Git must time out: {result:?}");
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Git status timed out after 10s; cleanup refused before removal"));
    assert!(
        elapsed >= std::time::Duration::from_secs(9)
            && elapsed < std::time::Duration::from_secs(12),
        "expected cleanup's dedicated ~10s deadline, got {elapsed:?}"
    );
    // Refused before any removal: the worktree and its branch both survive.
    assert!(
        repo.worktree.exists(),
        "a refused cleanup must not remove the worktree"
    );
    repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
    assert!(
        !state.get(id).unwrap().dispatch.as_ref().unwrap().cleaned,
        "a refused cleanup must not mark the dispatch cleaned"
    );
}

#[cfg(unix)]
#[test]
fn cleanup_status_tolerates_a_slow_but_finishing_filesystem_watcher() {
    // Pin the shipped 250ms default so a cleanup query that slipped back onto it fails here.
    tsk_tui::git_base::exact_local_deadline_on_this_thread();
    let repo = CleanupRepo::new();
    let hook = repo.root.join("slow-fsmonitor-ok");
    // Longer than git_base's 250ms metadata default, well under cleanup's ~10s deadline.
    stub::write_stub(&hook, "#!/bin/sh\nsleep 1\nprintf 'token\\0'\n", 0o700);
    repo.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    let (state, id) = repo.state("base", None);
    let record = state.get(id).unwrap().dispatch.as_ref().unwrap();
    let start = std::time::Instant::now();
    let result = system_host().inspect_cleanup(&repo.project, record, false);
    let elapsed = start.elapsed();
    assert!(
        result.is_ok(),
        "a status read slower than 250ms must still succeed under the longer deadline: {result:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(1),
        "expected the hook's artificial delay to be observed, got {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "must still stay within cleanup's deadline, got {elapsed:?}"
    );
}

/// Run PATH-sensitive probes in their own test process, never the parallel shared harness.
#[cfg(unix)]
fn ancestry_probe_child(name: &str, sleep_secs: u64) -> bool {
    cleanup_query_probe_child(name, sleep_secs, "merge-base:*")
}

#[cfg(unix)]
fn cleanup_query_probe_child(name: &str, sleep_secs: u64, pattern: &str) -> bool {
    if std::env::var("TSK_CLEANUP_ANCESTRY_CHILD").ok().as_deref() == Some(name) {
        return false;
    }
    let output = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let real_git = String::from_utf8(output.stdout).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "tsk-slow-ancestry-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    let script = dir.join("git");
    stub::write_stub(&script, format!("#!/bin/sh\ncase \"$3:$4\" in {pattern}) sleep {sleep_secs};; esac\nexec \"{}\" \"$@\"\n", real_git.trim()), 0o700);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env("TSK_CLEANUP_ANCESTRY_CHILD", name)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

#[cfg(unix)]
#[test]
fn delete_merged_branch_retains_branch_when_late_ancestry_check_times_out() {
    if ancestry_probe_child(
        "cli_dispatch::delete_merged_branch_retains_branch_when_late_ancestry_check_times_out",
        11,
    ) {
        return;
    }
    let repo = CleanupRepo::new();
    system_host()
        .remove_git_worktree(&repo.project, &repo.worktree)
        .expect("remove worktree ahead of the late ancestry check");
    let start = std::time::Instant::now();
    let result =
        { system_host().delete_merged_branch(&repo.project, "tsk/t1-clean", "refs/heads/base") };
    let elapsed = start.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_secs(9)
            && elapsed < std::time::Duration::from_secs(13),
        "expected cleanup's dedicated ~10s deadline, got {elapsed:?}"
    );
    assert_eq!(
        result,
        Ok(tsk_tui::dispatch::BranchDeletion::Kept(
            tsk_tui::dispatch::BranchRetentionReason::AncestryCheckTimedOut
        )),
        "a late ancestry timeout must retain the branch with an honest reason, not NotMerged"
    );
    // Branch retained: it must still exist after the timed-out check.
    repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
}

#[cfg(unix)]
#[test]
fn delete_merged_branch_tolerates_a_slow_but_finishing_ancestry_check() {
    if ancestry_probe_child(
        "cli_dispatch::delete_merged_branch_tolerates_a_slow_but_finishing_ancestry_check",
        1,
    ) {
        return;
    }
    // Pin the shipped 250ms default so a cleanup query that slipped back onto it fails here.
    tsk_tui::git_base::exact_local_deadline_on_this_thread();
    let repo = CleanupRepo::new();
    system_host()
        .remove_git_worktree(&repo.project, &repo.worktree)
        .expect("remove worktree ahead of the ancestry check");
    let start = std::time::Instant::now();
    let result =
        { system_host().delete_merged_branch(&repo.project, "tsk/t1-clean", "refs/heads/base") };
    let elapsed = start.elapsed();
    assert_eq!(
        result,
        Ok(tsk_tui::dispatch::BranchDeletion::Removed),
        "an ancestry check slower than 250ms must still complete and delete a merged branch: {result:?}"
    );
    assert!(
        elapsed >= std::time::Duration::from_secs(1),
        "expected the wrapper's artificial delay to be observed, got {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "must still stay within cleanup's deadline, got {elapsed:?}"
    );
}

#[cfg(unix)]
#[test]
fn cleanup_inspection_tolerates_a_slow_but_finishing_ancestry_check() {
    if ancestry_probe_child(
        "cli_dispatch::cleanup_inspection_tolerates_a_slow_but_finishing_ancestry_check",
        1,
    ) {
        return;
    }
    // Pin the shipped 250ms default so a cleanup query that slipped back onto it fails here.
    tsk_tui::git_base::exact_local_deadline_on_this_thread();
    let repo = CleanupRepo::new();
    let (state, id) = repo.state("base", None);
    let start = std::time::Instant::now();
    let result = system_host()
        .inspect_cleanup(
            &repo.project,
            state.get(id).unwrap().dispatch.as_ref().unwrap(),
            false,
        )
        .unwrap();
    assert!(result.branch_merged);
    assert!(start.elapsed() >= std::time::Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn cleanup_tolerates_slow_worktree_listings_at_both_safety_checks() {
    if cleanup_query_probe_child(
        "cli_dispatch::cleanup_tolerates_slow_worktree_listings_at_both_safety_checks",
        1,
        "worktree:list",
    ) {
        return;
    }
    // Pin the shipped 250ms default so a cleanup query that slipped back onto it fails here.
    tsk_tui::git_base::exact_local_deadline_on_this_thread();
    let repo = CleanupRepo::new();
    let (mut state, id) = repo.state("base", None);
    let start = std::time::Instant::now();
    let result =
        tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut system_host()).unwrap();
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Removed);
    assert!(!repo.worktree.exists());
    assert!(
        start.elapsed() >= std::time::Duration::from_secs(2),
        "both cleanup listing calls must run through the slow wrapper"
    );
}

#[cfg(unix)]
#[test]
fn timed_out_final_worktree_listing_keeps_the_branch_with_a_clear_reason() {
    if cleanup_query_probe_child(
        "cli_dispatch::timed_out_final_worktree_listing_keeps_the_branch_with_a_clear_reason",
        11,
        "worktree:list",
    ) {
        return;
    }
    let repo = CleanupRepo::new();
    system_host()
        .remove_git_worktree(&repo.project, &repo.worktree)
        .unwrap();
    let result = system_host()
        .delete_merged_branch(&repo.project, "tsk/t1-clean", "refs/heads/base")
        .unwrap();
    let tsk_tui::dispatch::BranchDeletion::Kept(reason) = result else {
        panic!("timed-out listing must not delete the branch");
    };
    assert_eq!(
        reason,
        tsk_tui::dispatch::BranchRetentionReason::WorktreeListingTimedOut
    );
    assert!(reason
        .message(Some("base"), None)
        .contains("worktree listing timed out; branch retained"));
    repo.git(&["show-ref", "--verify", "refs/heads/tsk/t1-clean"]);
}

/// `tsk dispatch` hands naming to a detached `tsk --name-agent` helper, which keeps asking
/// Herdr through slow detection after the caller has moved on, then exits.
#[cfg(unix)]
#[test]
fn the_detached_naming_helper_names_a_slow_agent_on_its_own() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-name-agent-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = dir.join("calls");
    // Two undetected answers, then the rename lands.
    stub::write_stub(
        &bin.join("herdr"),
        format!(
            "#!/bin/sh\necho \"$*\" >> '{log}'\nif [ $(wc -l < '{log}') -lt 3 ]; then\n  echo '{{\"error\":{{\"code\":\"agent_not_found\",\"message\":\"agent target p1 not found\"}}}}' >&2\n  exit 1\nfi\necho '{{\"result\":{{}}}}'\n",
            log = log.display()
        ),
        0o700,
    );
    let naming = tsk_tui::dispatch::AgentNaming {
        pane_id: "p1".into(),
        name: "t12-claude".into(),
    };
    let mut command =
        tsk_tui::dispatch::agent_naming_command(Path::new(env!("CARGO_BIN_EXE_tsk")), &naming);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    command.env("PATH", path);
    let mut helper = command.spawn().expect("spawn helper");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        if let Some(status) = helper.try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "helper never finished"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(status.success());
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "agent rename p1 t12-claude\n".repeat(3)
    );
    fs::remove_dir_all(dir).unwrap();
}

/// A Herdr that never answers cannot hold the detached naming helper open: each rename is
/// bounded by what is left of the 30 s window, and the stalled herdr is killed and reaped.
#[cfg(unix)]
#[test]
fn the_naming_helper_exits_within_its_window_when_herdr_hangs() {
    let dir = std::env::temp_dir().join(format!(
        "tsk-cli-name-agent-hang-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let pid_file = dir.join("herdr.pid");
    stub::write_stub(
        &bin.join("herdr"),
        format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 600\n",
            pid_file.display()
        ),
        0o700,
    );
    let naming = tsk_tui::dispatch::AgentNaming {
        pane_id: "p1".into(),
        name: "t12-claude".into(),
    };
    let mut command =
        tsk_tui::dispatch::agent_naming_command(Path::new(env!("CARGO_BIN_EXE_tsk")), &naming);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    command.env("PATH", path);
    let started = std::time::Instant::now();
    let mut helper = command.spawn().expect("spawn helper");

    let deadline = started + std::time::Duration::from_secs(45);
    let status = loop {
        if let Some(status) = helper.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let herdr: Option<i32> = fs::read_to_string(&pid_file)
        .ok()
        .and_then(|pid| pid.trim().parse().ok());
    if status.is_none() {
        let _ = helper.kill();
        let _ = helper.wait();
    }
    // The stalled herdr must be gone too, whatever happened to the helper.
    let herdr_alive = herdr.is_some_and(|pid| {
        // SAFETY: signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        if alive {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        alive
    });
    let _ = fs::remove_dir_all(&dir);
    let status = status.expect("the helper outlived its 30 s window with herdr hung");
    assert!(status.success());
    assert!(herdr.is_some(), "the helper never ran herdr");
    assert!(!herdr_alive, "the hung herdr was left running");
    assert!(started.elapsed() < std::time::Duration::from_secs(40));
}

/// A stub `gh` for [`CleanupRepo`]: `pr view` answers `state` with `head` into `base`, and
/// `pr merge` exits `merge_exit`. Every call's arguments are logged, one line each.
#[cfg(unix)]
fn stub_gh(repo: &CleanupRepo, state: &str, head: &str, merge_exit: u8) -> (PathBuf, PathBuf) {
    stub_gh_with(repo, state, head, merge_exit, false, "")
}

/// A stateful stub `gh`: `pr view` reads the PR's state from a file, and a successful
/// `pr merge` runs `on_merge`, then marks the PR MERGED unless `queue` (a merge queue).
#[cfg(unix)]
fn stub_gh_with(
    repo: &CleanupRepo,
    state: &str,
    head: &str,
    merge_exit: u8,
    queue: bool,
    on_merge: &str,
) -> (PathBuf, PathBuf) {
    let gh = repo.root.join("gh");
    let log = repo.root.join("gh.log");
    let state_file = repo.root.join("gh.state");
    fs::write(&state_file, state).unwrap();
    let merged = if queue {
        String::new()
    } else {
        format!("echo MERGED > '{}'", state_file.display())
    };
    stub::write_stub(
        &gh,
        format!(
            "#!/bin/sh\necho \"$@\" >> '{log}'\ncase \"$1 $2\" in\n\
             'pr view') printf '{{\"number\":7,\"url\":\"https://github.com/o/r/pull/7\",\"state\":\"%s\",\"headRefOid\":\"{head}\",\"baseRefName\":\"base\"}}\\n' \"$(cat '{state}')\" ;;\n\
             'pr merge') if [ {merge_exit} != 0 ]; then printf 'X Pull request o/r#7 is not mergeable: the base branch policy prohibits the merge.\\n\\nTo have the pull request merged after all the requirements have been met, add the `--auto` flag.\\n' >&2; exit {merge_exit}; fi\n{on_merge}\n{merged} ;;\n\
             esac\n",
            log = log.display(),
            state = state_file.display(),
        ),
        0o700,
    );
    (gh, log)
}

#[cfg(unix)]
fn clean_with_gh(repo: &CleanupRepo, gh: &Path) -> tsk_tui::dispatch::CleanupResult {
    let (mut state, id) = repo.state("base", None);
    let mut host = system_host().with_gh(gh);
    tsk_tui::dispatch::clean_with_host(&mut state, id, false, &mut host).expect("cleaned")
}

#[cfg(unix)]
#[test]
fn a_squash_merged_branch_is_deleted_when_gh_confirms_its_exact_tip() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("feature.txt", "work\n")]);
    let tip = repo.git(&["rev-parse", "tsk/t1-clean"]);
    let (gh, log) = stub_gh(&repo, "MERGED", &tip, 0);
    let result = clean_with_gh(&repo, &gh);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Removed);
    assert_eq!(result.branch_reason, None);
    assert_eq!(
        repo.git(&["branch", "--list", "tsk/t1-clean"]),
        "",
        "the squash-merged branch is gone"
    );
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.starts_with("pr view tsk/t1-clean --json"), "{calls}");
}

#[cfg(unix)]
#[test]
fn a_merged_pr_whose_head_is_not_the_branch_tip_keeps_the_branch() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("feature.txt", "work\n")]);
    let merged = repo.git(&["rev-parse", "tsk/t1-clean"]);
    repo.commit_files(&[("later.txt", "after the merge\n")]);
    let (gh, _) = stub_gh(&repo, "MERGED", &merged, 0);
    let result = clean_with_gh(&repo, &gh);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    let output = tsk_tui::cli::presenter::cleaned(result, false);
    assert!(
        output
            .stdout
            .contains("PR merged but the branch has newer commits; delete by hand"),
        "{}",
        output.stdout
    );
    assert_ne!(repo.git(&["branch", "--list", "tsk/t1-clean"]), "");
}

#[cfg(unix)]
#[test]
fn an_open_pr_keeps_an_unmerged_branch_with_todays_reason() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("feature.txt", "work\n")]);
    let tip = repo.git(&["rev-parse", "tsk/t1-clean"]);
    let (gh, _) = stub_gh(&repo, "OPEN", &tip, 0);
    let result = clean_with_gh(&repo, &gh);
    assert_eq!(
        result.branch_reason,
        Some(tsk_tui::dispatch::BranchRetentionReason::NotMerged)
    );
    assert_ne!(repo.git(&["branch", "--list", "tsk/t1-clean"]), "");
}

#[cfg(unix)]
#[test]
fn merge_runs_gh_pr_merge_with_the_configured_method_and_refuses_without_changing_anything() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("feature.txt", "work\n")]);
    let tip = repo.git(&["rev-parse", "tsk/t1-clean"]);
    let (state, _) = repo.state("base", None);
    let state_dir = repo.root.join("state");
    let store = TaskStore::new(&state_dir);
    store.save(&state).unwrap();
    fs::write(
        state_dir.join("config.toml"),
        "[merge]\nmethod = \"squash\"\n",
    )
    .unwrap();
    let before = fs::read_to_string(state_dir.join("tsk.json")).unwrap();

    // gh refuses: the reason comes back, and the task and branch are untouched.
    let (gh, log) = stub_gh(&repo, "OPEN", &tip, 1);
    let refused = clean::merge_with_host(
        TaskAddress::Number(1),
        Some(state_dir.clone()),
        false,
        &mut system_host().with_gh(&gh),
    )
    .expect_err("gh refused the merge");
    assert_eq!(refused.code(), "merge-failed");
    // gh's whole message, every line, without the `X ` glyph gh leads it with.
    assert!(
        refused.message().contains(
            "Pull request o/r#7 is not mergeable: the base branch policy prohibits the merge. \
             To have the pull request merged after all the requirements have been met, add the \
             `--auto` flag."
        ) && !refused.message().contains("X Pull request"),
        "{}",
        refused.message()
    );
    assert_eq!(
        fs::read_to_string(state_dir.join("tsk.json")).unwrap(),
        before
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains(&format!(
            "pr merge tsk/t1-clean --squash --match-head-commit {tip}"
        )),
        "{calls}"
    );

    // gh merges.
    let (gh, _) = stub_gh(&repo, "OPEN", &tip, 0);
    let merged = clean::merge_with_host(
        TaskAddress::Number(1),
        Some(state_dir.clone()),
        false,
        &mut system_host().with_gh(&gh),
    )
    .expect("merged");
    assert_eq!(merged.outcome.pr.number, 7);
    assert_eq!(merged.outcome.method.name(), "squash");

    // An invalid method refuses before gh is asked anything.
    fs::write(state_dir.join("config.toml"), "[merge]\nmethod = \"ff\"\n").unwrap();
    fs::remove_file(&log).unwrap();
    let refused = clean::merge_with_host(
        TaskAddress::Number(1),
        Some(state_dir),
        false,
        &mut system_host().with_gh(&gh),
    )
    .expect_err("bad config");
    assert_eq!(refused.code(), "merge-config");
    assert!(!log.exists(), "gh never ran");
}

/// A dispatched task whose branch is pushed to a bare `origin`, recorded from `origin/base`
/// with its remote, so cleanup fetches and the merge's `ls-remote` runs against real git.
/// `[merge] method = "squash"`. Returns the state dir and the branch tip.
#[cfg(unix)]
fn merge_repo(repo: &CleanupRepo) -> (PathBuf, String) {
    repo.commit_files(&[("feature.txt", "work\n")]);
    let remote = repo.root.join("remote.git");
    let status = std::process::Command::new("git")
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(status.success());
    repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    repo.git(&["push", "-q", "origin", "base", "tsk/t1-clean"]);
    repo.git(&["fetch", "-q", "origin"]);
    let (state, _) = repo.state("origin/base", Some("origin"));
    let state_dir = repo.root.join("state");
    TaskStore::new(&state_dir).save(&state).unwrap();
    fs::write(
        state_dir.join("config.toml"),
        "[merge]\nmethod = \"squash\"\n",
    )
    .unwrap();
    (state_dir, repo.git(&["rev-parse", "tsk/t1-clean"]))
}

#[cfg(unix)]
fn task_on_disk(state_dir: &Path) -> tsk_tui::domain::Task {
    TaskStore::new(state_dir).load().unwrap().tasks()[0].clone()
}

#[cfg(unix)]
#[test]
fn clean_merge_json_merges_then_saves_done_then_cleans_and_reports_the_merge() {
    let repo = CleanupRepo::new();
    let (state_dir, tip) = merge_repo(&repo);
    // The stub snapshots the store at the merge: done must not be saved yet.
    let snapshot = repo.root.join("at-merge.json");
    let on_merge = format!(
        "cp '{}' '{}'",
        state_dir.join("tsk.json").display(),
        snapshot.display()
    );
    let (gh, log) = stub_gh_with(&repo, "OPEN", &tip, 0, false, &on_merge);
    let output = tsk_tui::cli::merge_done_clean_with_host(
        TaskAddress::Number(1),
        state_dir.clone(),
        "clean",
        true,
        false,
        &mut system_host().with_gh(&gh),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).expect("one JSON object");
    assert_eq!(value["merge"]["pr"], 7);
    assert_eq!(value["merge"]["method"], "squash");
    assert_eq!(
        value["merge"]["remote_branch"], "kept",
        "ls-remote found it on origin"
    );
    assert_eq!(value["status"], "done");
    assert_eq!(
        value["branch"]["outcome"], "removed",
        "the squash-merged branch went"
    );
    assert_eq!(value["worktree"]["outcome"], "removed");
    let before: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&snapshot).unwrap()).unwrap();
    assert_ne!(
        before["tasks"][0]["status"], "done",
        "merge ran before done"
    );
    let task = task_on_disk(&state_dir);
    assert_eq!(task.status, HumanStatus::Done);
    assert!(task.dispatch.as_ref().unwrap().cleaned);
    let calls = fs::read_to_string(&log).unwrap();
    let calls = calls.lines().collect::<Vec<_>>();
    assert!(calls[0].starts_with("pr view"), "{calls:?}");
    assert!(
        calls[1].starts_with("pr merge tsk/t1-clean --squash"),
        "{calls:?}"
    );
    assert!(
        calls[2].starts_with("pr view"),
        "the merge confirmed: {calls:?}"
    );
    assert!(!repo.worktree.exists());
}

#[cfg(unix)]
#[test]
fn status_done_merge_prints_the_merge_the_status_and_the_cleanup() {
    let repo = CleanupRepo::new();
    let (state_dir, tip) = merge_repo(&repo);
    let (gh, _) = stub_gh(&repo, "OPEN", &tip, 0);
    let output = tsk_tui::cli::merge_done_clean_with_host(
        TaskAddress::Number(1),
        state_dir.clone(),
        "status",
        false,
        false,
        &mut system_host().with_gh(&gh),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let lines = output.stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[0],
        "merged T1 pull request #7 (squash) · remote branch origin/tsk/t1-clean left in place"
    );
    assert_eq!(lines[1], "status T1 done cleanup");
    assert!(lines[2].starts_with("cleaned T1:"), "{lines:?}");
    assert_eq!(task_on_disk(&state_dir).status, HumanStatus::Done);
}

#[cfg(unix)]
#[test]
fn a_merge_whose_done_save_fails_exits_3_reports_the_merge_and_cleans_nothing() {
    let repo = CleanupRepo::new();
    let (state_dir, tip) = merge_repo(&repo);
    // The store goes read-only once GitHub merged: the done save after it fails.
    let on_merge = format!("chmod 555 '{}'", state_dir.display());
    let (gh, _) = stub_gh_with(&repo, "OPEN", &tip, 0, false, &on_merge);
    let output = tsk_tui::cli::merge_done_clean_with_host(
        TaskAddress::Number(1),
        state_dir.clone(),
        "clean",
        true,
        false,
        &mut system_host().with_gh(&gh),
    );
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.code, 3, "{}", output.stderr);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).expect("JSON still");
    assert_eq!(value["merge"]["pr"], 7);
    assert_eq!(value["status"], serde_json::Value::Null);
    assert_eq!(value["error"]["stage"], "done");
    assert_eq!(value["error"]["code"], "store-error");
    assert!(
        output.stderr.starts_with("tsk clean: "),
        "{}",
        output.stderr
    );
    assert_ne!(task_on_disk(&state_dir).status, HumanStatus::Done);
    assert!(repo.worktree.exists(), "nothing cleaned");
}

#[cfg(unix)]
#[test]
fn a_merge_gh_only_queued_refuses_and_leaves_the_task() {
    let repo = CleanupRepo::new();
    let (state_dir, tip) = merge_repo(&repo);
    let (gh, _) = stub_gh_with(&repo, "OPEN", &tip, 0, true, "");
    let output = tsk_tui::cli::merge_done_clean_with_host(
        TaskAddress::Number(1),
        state_dir.clone(),
        "status",
        false,
        false,
        &mut system_host().with_gh(&gh),
    );
    assert_eq!(output.code, 1);
    assert_eq!(
        output.stderr,
        "tsk status: merge-failed: queued or not merged yet: open\n"
    );
    assert_ne!(task_on_disk(&state_dir).status, HumanStatus::Done);
    assert!(repo.worktree.exists());
}

#[cfg(unix)]
#[test]
fn a_squash_merged_branch_checked_out_elsewhere_is_kept() {
    let repo = CleanupRepo::new();
    repo.commit_files(&[("feature.txt", "work\n")]);
    let tip = repo.git(&["rev-parse", "tsk/t1-clean"]);
    repo.git(&["checkout", "-q", "--ignore-other-worktrees", "tsk/t1-clean"]);
    let (gh, _) = stub_gh(&repo, "MERGED", &tip, 0);
    let result = clean_with_gh(&repo, &gh);
    assert_eq!(result.branch, tsk_tui::dispatch::BranchCleanup::Kept);
    assert_eq!(
        result.branch_reason,
        Some(tsk_tui::dispatch::BranchRetentionReason::CheckedOutElsewhere)
    );
    assert_eq!(
        repo.git(&["rev-parse", "tsk/t1-clean"]),
        tip,
        "the ref stays"
    );
}

#[cfg(unix)]
#[test]
fn a_merge_whose_cleanup_is_refused_still_reports_the_merge_and_the_done_in_json() {
    let repo = CleanupRepo::new();
    let (state_dir, tip) = merge_repo(&repo);
    // Work lands in the worktree while gh merges: the cleanup after it refuses.
    let on_merge = format!("echo late > '{}'", repo.worktree.join("late.txt").display());
    let (gh, _) = stub_gh_with(&repo, "OPEN", &tip, 0, false, &on_merge);
    let output = tsk_tui::cli::merge_done_clean_with_host(
        TaskAddress::Number(1),
        state_dir.clone(),
        "clean",
        true,
        false,
        &mut system_host().with_gh(&gh),
    );
    assert_eq!(output.code, 1, "{}", output.stderr);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).expect("JSON still");
    assert_eq!(value["merge"]["method"], "squash");
    assert_eq!(value["status"], "done");
    assert_eq!(value["error"]["stage"], "cleanup");
    assert_eq!(value["error"]["code"], "dirty-worktree");
    assert_eq!(
        value["error"]["message"],
        "worktree has uncommitted changes"
    );
    assert_eq!(task_on_disk(&state_dir).status, HumanStatus::Done);
    assert!(repo.worktree.exists());
}
