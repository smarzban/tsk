//! Host Adapter launcher contract.
//! Scripts are bash open/focus helpers; tests are path + mode + text contracts
//! (no live herdr). Manual: second open-board focuses the same Tasks board.
#![cfg(unix)]

use std::fs;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use tsk_tui::app::MODE_ENV;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn open_board_path() -> PathBuf {
    repo_root().join("scripts/open-board.sh")
}

fn open_capture_path() -> PathBuf {
    repo_root().join("scripts/open-capture.sh")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

struct Launcher {
    root: PathBuf,
}

impl Launcher {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tsk-host-launcher-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("state")).expect("temp root");
        let stub = root.join("herdr");
        fs::write(
            &stub,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$STUB_ROOT/calls.log"
if [ "$1" = pane ] && [ "$2" = list ]; then
  [ "${STUB_MODE:-}" != list-failure ] || exit 1
  if [ "${STUB_MODE:-}" = missing-tab ]; then
    printf '%s' '{"result":{"panes":[{"pane_id":"w0:p2","label":"tsk"}]}}'
    exit 0
  fi
  if [ "$3" = --workspace ] && [ "$4" = w0 ]; then
    if [ "${STUB_MODE:-}" = existing ] || [ "${STUB_MODE:-}" = focus-failure ] || [ "${STUB_MODE:-}" = tab-focus-failure ] || [ -f "$STUB_ROOT/opened" ]; then
      printf '%s' '{"result":{"panes":[{"pane_id":"w0:p2","workspace_id":"w0","tab_id":"w0:t2","label":"tsk"}]}}'
    else
      printf '%s' '{"result":{"panes":[]}}'
    fi
  else
    printf '%s' '{"result":{"panes":[{"pane_id":"w9:p1","workspace_id":"w9","tab_id":"w9:t1","label":"tsk"},{"pane_id":"w0:p2","workspace_id":"w0","tab_id":"w0:t2","label":"tsk"}]}}'
  fi
elif [ "$1" = tab ] && [ "$2" = focus ]; then
  [ "${STUB_MODE:-}" != tab-focus-failure ] || exit 1
  printf '%s' "$3" > "$STUB_ROOT/visible-tab"
elif [ "$1" = plugin ] && [ "$2" = pane ] && [ "$3" = focus ]; then
  [ "${STUB_MODE:-}" != focus-failure ] || exit 1
elif [ "$1" = plugin ] && [ "$2" = pane ] && [ "$3" = open ]; then
  touch "$STUB_ROOT/opened"
  printf '%s' 'w0:t1' > "$STUB_ROOT/opened-tab"
  printf '%s' "${HERDR_PLUGIN_CONTEXT_JSON:-}" > "$STUB_ROOT/open-context.json"
fi
"#,
        )
        .expect("stub");
        let mut permissions = fs::metadata(&stub).expect("stub metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&stub, permissions).expect("stub executable");
        Self { root }
    }

    fn run(&self, mode: &str, workspace: Option<&str>) -> std::process::Output {
        self.run_with_pane(mode, workspace, Some("w0:p1"))
    }

    fn run_with_pane(
        &self,
        mode: &str,
        workspace: Option<&str>,
        pane: Option<&str>,
    ) -> std::process::Output {
        self.run_with_tab(mode, workspace, pane, Some("w0:t1"))
    }

    fn run_with_tab(
        &self,
        mode: &str,
        workspace: Option<&str>,
        pane: Option<&str>,
        tab: Option<&str>,
    ) -> std::process::Output {
        let mut command = Command::new("bash");
        command
            .arg(open_board_path())
            .env("HERDR_BIN_PATH", self.root.join("herdr"))
            .env("TSK_BIN", env!("CARGO_BIN_EXE_tsk"))
            .env("TSK_STATE_DIR", self.root.join("state"))
            .env("STUB_ROOT", &self.root)
            .env("STUB_MODE", mode)
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_TAB_ID")
            .env(
                "HERDR_PLUGIN_CONTEXT_JSON",
                r#"{"focused_pane_cwd":"/tmp"}"#,
            )
            .env_remove("HERDR_WORKSPACE_ID");
        if let Some(workspace) = workspace {
            command.env("HERDR_WORKSPACE_ID", workspace);
        }
        if let Some(pane) = pane {
            command.env("HERDR_PANE_ID", pane);
        }
        if let Some(tab) = tab {
            command.env("HERDR_TAB_ID", tab);
        }
        command.output().expect("launcher")
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.root.join("calls.log")).unwrap_or_default()
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn open_board_focuses_current_workspace_board_even_in_another_tab() {
    let launcher = Launcher::new();
    let output = launcher.run("existing", Some("w0"));
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        launcher.calls(),
        "pane list --workspace w0\ntab focus w0:t2\nplugin pane focus w0:p2\n"
    );
}

#[test]
fn open_board_opens_locally_when_only_other_workspaces_have_boards() {
    let launcher = Launcher::new();
    let output = launcher.run("absent", Some("w0"));
    assert!(output.status.success(), "{output:?}");
    let calls = launcher.calls();
    assert!(calls.starts_with("pane list --workspace w0\n"), "{calls}");
    assert!(calls.contains("plugin pane open "), "{calls}");
    assert!(calls.contains("--workspace w0"), "{calls}");
    assert!(calls.contains("--placement split"), "{calls}");
    assert!(calls.contains("--focus"), "{calls}");
    let open = calls
        .lines()
        .find(|line| line.starts_with("plugin pane open "))
        .expect("open call");
    assert!(open.contains("--target-pane w0:p1"), "{open}");
    assert!(
        !open.contains("--workspace"),
        "split placement rejects workspace_id: {open}"
    );
    assert!(!calls.contains("plugin pane focus"), "{calls}");
    assert_eq!(
        read(&launcher.root.join("open-context.json")),
        r#"{"focused_pane_cwd":"/tmp"}"#
    );
    // The next invocation finds the local pane, never creates a second one.
    let output = launcher.run("absent", Some("w0"));
    assert!(output.status.success(), "{output:?}");
    let calls = launcher.calls();
    assert_eq!(calls.matches("plugin pane open ").count(), 1, "{calls}");
    assert!(calls.ends_with("plugin pane focus w0:p2\n"), "{calls}");
}

#[test]
fn open_board_missing_tab_refuses_without_creating_a_duplicate() {
    let launcher = Launcher::new();
    let output = launcher.run("missing-tab", Some("w0"));
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(launcher.calls(), "pane list --workspace w0\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("board tab"));
}

#[test]
fn open_board_tab_focus_failure_does_not_focus_or_duplicate_the_board() {
    let launcher = Launcher::new();
    let output = launcher.run("tab-focus-failure", Some("w0"));
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        launcher.calls(),
        "pane list --workspace w0\ntab focus w0:t2\n"
    );
}

#[test]
fn open_board_stale_focus_falls_back_to_a_visible_board_in_the_invoking_tab() {
    let launcher = Launcher::new();
    let output = launcher.run("focus-failure", Some("w0"));
    assert!(output.status.success(), "{output:?}");
    let calls = launcher.calls();
    assert_eq!(
        read(&launcher.root.join("visible-tab")),
        read(&launcher.root.join("opened-tab")),
        "the replacement must be visible, not merely focused on the server"
    );
    assert!(
        calls.contains("plugin pane focus w0:p2\ntab focus w0:t1\nplugin pane open "),
        "{calls}"
    );
    let open = calls
        .lines()
        .find(|line| line.starts_with("plugin pane open "))
        .expect("open call");
    assert!(open.contains("--target-pane w0:p1"), "{open}");
    assert!(!open.contains("--workspace"), "{open}");
}

#[test]
fn open_board_refuses_missing_workspace_without_global_lookup() {
    for workspace in [None, Some("")] {
        let launcher = Launcher::new();
        let output = launcher.run("existing", workspace);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("workspace"));
        assert!(launcher.calls().is_empty());
    }
}

#[test]
fn open_board_refuses_missing_pane_without_using_host_focus() {
    for pane in [None, Some("")] {
        let launcher = Launcher::new();
        let output = launcher.run_with_pane("absent", Some("w0"), pane);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("pane"));
        assert!(launcher.calls().is_empty());
    }
}

#[test]
fn open_board_refuses_missing_invoking_tab_before_navigation() {
    for tab in [None, Some("")] {
        let launcher = Launcher::new();
        let output = launcher.run_with_tab("focus-failure", Some("w0"), Some("w0:p1"), tab);
        assert!(!output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("tab"));
        assert!(launcher.calls().is_empty());
    }
}

#[test]
fn open_board_list_failure_does_not_create_a_duplicate() {
    let launcher = Launcher::new();
    let output = launcher.run("list-failure", Some("w0"));
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(launcher.calls(), "pane list --workspace w0\n");
}

/// Non-comment, non-empty lines of a shell script (strip `# ...` full-line comments).
fn active_script_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Slice the `[[actions]]` table whose `id = "..."` matches `action_id`.
/// Bounds: from that table header through the line before the next `[[...]]` (or EOF).
/// Comments above the header are excluded so path mentions in prose cannot green the assert.
fn action_section<'a>(text: &'a str, action_id: &str) -> &'a str {
    let id_line = format!(r#"id = "{action_id}""#);
    let id_pos = text
        .find(&id_line)
        .unwrap_or_else(|| panic!("manifest must declare action id {action_id}"));
    let table_start = text[..id_pos]
        .rfind("[[actions]]")
        .unwrap_or_else(|| panic!("action id {action_id} must sit under an [[actions]] table"));
    let after_header = &text[table_start + "[[actions]]".len()..];
    let end = after_header.find("\n[[").unwrap_or(after_header.len());
    &text[table_start..table_start + "[[actions]]".len() + end]
}

fn assert_executable(path: &Path) {
    assert!(
        path.is_file(),
        "expected launcher script at {}",
        path.display()
    );
    let mode = fs::metadata(path)
        .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
        .permissions()
        .mode();
    assert!(
        mode & 0o111 != 0,
        "{} must be executable (mode {mode:#o}); chmod +x and keep git filemode 100755",
        path.display()
    );

    // Prefer git's recorded mode when the tree is tracked (CI / fresh clone).
    let rel = path
        .strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    let out = Command::new("git")
        .args(["ls-files", "-s", "--", &rel])
        .current_dir(repo_root())
        .output()
        .expect("git ls-files");
    if out.status.success() {
        let line = String::from_utf8_lossy(&out.stdout);
        let line = line.trim();
        if !line.is_empty() {
            assert!(
                line.starts_with("100755 "),
                "git mode for {rel} must be 100755 (executable), got: {line}"
            );
        }
    }
}

#[test]
fn open_board_script_exists_and_is_executable() {
    assert_executable(&open_board_path());
}

#[test]
fn open_capture_script_exists_and_is_executable() {
    assert_executable(&open_capture_path());
}

#[test]
fn open_board_uses_herdr_cli_and_board_entrypoint() {
    let text = read(&open_board_path());
    assert!(
        text.contains("HERDR_BIN_PATH"),
        "open-board must use HERDR_BIN_PATH"
    );
    assert!(
        text.contains("plugin pane open"),
        "open-board must call herdr plugin pane open"
    );
    assert!(
        text.contains("board"),
        "open-board must reference the board entrypoint id"
    );
    assert!(
        text.contains("tsk"),
        "open-board must reference the tsk board title/label"
    );
    assert!(
        text.contains("split"),
        "open-board must open with split placement"
    );
}

#[test]
fn open_board_has_idempotent_focus_list_logic() {
    let text = read(&open_board_path());
    let active = active_script_lines(&text).join("\n");

    // Contract: list existing panes, then focus if board already present.
    // Require these tokens on non-comment lines so a comment-only script cannot green.
    assert!(
        active.contains("pane list") || active.contains("plugin pane list"),
        "open-board active lines must list panes to find an existing board"
    );
    assert!(
        active.contains("focus") || active.contains("plugin pane focus"),
        "open-board active lines must focus an existing board pane when found"
    );
    assert!(
        active.contains("--find-board-pane"),
        "open-board must pipe pane list through tsk --find-board-pane (no python3)"
    );
    assert!(
        active.contains("target/release/tsk") || active.contains("TSK_BIN"),
        "open-board must locate plugin binary relative to script (or TSK_BIN)"
    );

    // No bare python3 dependency for focus logic.
    for line in active_script_lines(&text) {
        assert!(
            !line.contains("python3"),
            "open-board must not depend on python3 for focus logic; found: {line}"
        );
    }
}

#[test]
fn open_capture_uses_herdr_cli_and_capture_path() {
    let text = read(&open_capture_path());
    assert!(
        text.contains("HERDR_BIN_PATH"),
        "open-capture must use HERDR_BIN_PATH"
    );
    assert!(
        text.contains("plugin pane open"),
        "open-capture must call herdr plugin pane open"
    );
    assert!(
        text.contains("capture") || text.contains("overlay"),
        "open-capture must open capture mode or a popup/overlay surface"
    );
    assert!(
        text.contains("tsk") || text.contains("board"),
        "open-capture must target this plugin / board entrypoint"
    );
}

#[test]
fn open_capture_opens_a_sized_popup_for_the_capture_session() {
    let text = read(&open_capture_path());
    let active = active_script_lines(&text).join("\n");
    assert!(
        active.contains("--placement popup"),
        "open-capture must open a popup pane; active lines: {active}"
    );
    assert!(
        !text.contains("overlay"),
        "open-capture must not use the retired overlay placement: {text}"
    );
    assert!(
        active.contains("--width") && active.contains("--height"),
        "a popup needs explicit --width/--height sized for the task page: {active}"
    );
    assert!(
        active.contains("--width 80") && active.contains("--height 15"),
        "the popup is 80x15 (operable for the task page): {active}"
    );
    assert!(
        active.contains("--focus"),
        "the popup must take focus when quick capture is invoked: {active}"
    );
    assert!(
        active.contains(&format!("--env {}=capture", MODE_ENV)),
        "the popup must still launch the binary in the capture session mode"
    );
}

#[test]
fn quick_capture_injects_the_mode_env_var_the_binary_reads() {
    let text = read(&open_capture_path());
    assert!(
        text.contains(&format!("--env {}=capture", MODE_ENV)),
        "open-capture must inject --env {MODE_ENV}=capture: the binary reads \
         app::MODE_ENV, and any other variable name silently opens the full board instead"
    );
    assert!(
        !text.contains("HERDR_TASKS_"),
        "open-capture must not carry legacy HERDR_TASKS_* variable names"
    );
}

#[test]
fn manifest_actions_point_at_launcher_scripts() {
    let manifest = read(&repo_root().join("herdr-plugin.toml"));
    let open_board = action_section(&manifest, "open-board");
    assert!(
        open_board.contains(r#"command = ["bash", "scripts/open-board.sh"]"#),
        "open-board action command must be [\"bash\", \"scripts/open-board.sh\"] \
         (not merely a path mention in a comment or elsewhere in the file)"
    );
    let quick_capture = action_section(&manifest, "quick-capture");
    assert!(
        quick_capture.contains(r#"command = ["bash", "scripts/open-capture.sh"]"#),
        "quick-capture action command must be [\"bash\", \"scripts/open-capture.sh\"] \
         (not merely a path mention in a comment or elsewhere in the file)"
    );
    assert!(
        !manifest.contains("[[events]]"),
        "must not declare [[events]] (explicit over ambient)"
    );
}
