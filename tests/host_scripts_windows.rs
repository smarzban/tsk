//! Host Adapter launcher contract (Windows).
//! Scripts are PowerShell open/focus helpers; tests are path + text contracts
//! (no live herdr). Manual: second open-board focuses the same Tasks board.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use tsk_tui::app::MODE_ENV;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn open_board_path() -> PathBuf {
    repo_root().join("scripts/open-board.ps1")
}

fn open_capture_path() -> PathBuf {
    repo_root().join("scripts/open-capture.ps1")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

struct Launcher {
    root: PathBuf,
    stub: PathBuf,
}

impl Launcher {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "tsk-host-launcher-windows-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("state")).expect("temp root");
        fs::write(
            root.join("existing.json"),
            r#"{"result":{"panes":[{"pane_id":"w0:p2","workspace_id":"w0","tab_id":"w0:t2","label":"tsk"}]}}"#,
        )
        .expect("existing panes fixture");
        fs::write(root.join("empty.json"), r#"{"result":{"panes":[]}}"#)
            .expect("empty panes fixture");
        let stub = root.join("herdr.ps1");
        fs::write(
            &stub,
            r#"Add-Content -LiteralPath (Join-Path $env:STUB_ROOT 'calls.log') -Value ($args -join ' ')
if ($args.Count -ge 2 -and $args[0] -eq 'pane' -and $args[1] -eq 'list') {
  if (Test-Path -LiteralPath (Join-Path $env:STUB_ROOT 'list-failure')) {
    & $env:ComSpec /c exit 7
    return
  }
  if (Test-Path -LiteralPath (Join-Path $env:STUB_ROOT 'existing')) {
    Get-Content -LiteralPath (Join-Path $env:STUB_ROOT 'existing.json') -Raw
  } else {
    Get-Content -LiteralPath (Join-Path $env:STUB_ROOT 'empty.json') -Raw
  }
}
& $env:ComSpec /c exit 0
"#,
        )
        .expect("stub");
        Self { root, stub }
    }

    fn command(&self, script: &Path, mode: &str) -> Command {
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(script)
            .env("HERDR_BIN_PATH", &self.stub)
            .env("TSK_BIN", env!("CARGO_BIN_EXE_tsk"))
            .env("TSK_STATE_DIR", self.root.join("state"))
            .env("STUB_ROOT", &self.root)
            .env("STUB_MODE", mode);
        command
    }

    fn run(&self, mode: &str) -> std::process::Output {
        if mode == "existing" || mode == "list-failure" {
            fs::write(self.root.join(mode), b"").expect("stub mode marker");
        }
        self.command(&open_board_path(), mode)
            .env("HERDR_WORKSPACE_ID", "w0")
            .env("HERDR_PANE_ID", "w0:p1")
            .env("HERDR_TAB_ID", "w0:t1")
            .output()
            .expect("PowerShell launcher")
    }

    fn run_capture(&self) -> std::process::Output {
        self.command(&open_capture_path(), "capture")
            .output()
            .expect("PowerShell capture launcher")
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

/// Non-comment, non-empty lines of a PowerShell script (strip `# ...` full-line comments).
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

#[test]
fn open_board_script_exists() {
    assert!(
        open_board_path().is_file(),
        "expected launcher script at {}",
        open_board_path().display()
    );
}

#[test]
fn open_capture_script_exists() {
    assert!(
        open_capture_path().is_file(),
        "expected launcher script at {}",
        open_capture_path().display()
    );
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
}

#[test]
fn open_board_focuses_an_existing_board_using_native_exit_status() {
    let launcher = Launcher::new();
    let output = launcher.run("existing");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        launcher.calls().replace("\r\n", "\n"),
        "pane list --workspace w0\ntab focus w0:t2\nplugin pane focus w0:p2\n"
    );
}

#[test]
fn open_board_refuses_missing_host_context_without_calling_herdr() {
    let launcher = Launcher::new();
    let output = launcher
        .command(&open_board_path(), "absent")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_TAB_ID")
        .output()
        .expect("PowerShell launcher");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("without a Herdr workspace, tab and pane")
    );
    assert!(launcher.calls().is_empty());
}

#[test]
fn open_board_refuses_a_failed_pane_list_without_opening_a_duplicate() {
    let launcher = Launcher::new();
    let output = launcher.run("list-failure");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not list panes"));
    assert_eq!(
        launcher.calls().replace("\r\n", "\n"),
        "pane list --workspace w0\n"
    );
}

#[test]
fn open_board_opens_in_the_invoking_tab_when_absent() {
    let launcher = Launcher::new();
    let output = launcher.run("absent");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = launcher.calls().replace("\r\n", "\n");
    assert!(
        calls.starts_with("pane list --workspace w0\ntab focus w0:t1\n"),
        "{calls}"
    );
    let open = calls
        .lines()
        .find(|line| line.starts_with("plugin pane open "))
        .expect("open call");
    assert!(open.contains("--target-pane w0:p1"), "{open}");
    assert!(open.contains("--placement split"), "{open}");
    assert!(!open.contains("--workspace"), "{open}");
}

#[test]
fn open_capture_executes_the_popup_command() {
    let launcher = Launcher::new();
    let output = launcher.run_capture();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = launcher.calls().replace("\r\n", "\n");
    assert!(calls.contains("plugin pane open"), "{calls}");
    for argument in [
        "--plugin herdr-tsk",
        "--entrypoint board",
        "--placement popup",
        "--width 80",
        "--height 15",
        "--focus",
        "--env TSK_MODE=capture",
    ] {
        assert!(calls.contains(argument), "missing {argument:?}: {calls}");
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
        active.contains(&format!("--env {}=capture", MODE_ENV))
            || active.contains(&format!("--env '{}=capture'", MODE_ENV)),
        "the popup must still launch the binary in the capture session mode"
    );
}

#[test]
fn quick_capture_injects_the_mode_env_var_the_binary_reads() {
    let text = read(&open_capture_path());
    assert!(
        text.contains(&format!("--env {}=capture", MODE_ENV))
            || text.contains(&format!("--env '{}=capture'", MODE_ENV)),
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
    let open_board = action_section(&manifest, "open-board-windows");
    assert!(
        open_board.contains(r#"command = ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/open-board.ps1"]"#),
        "open-board-windows must use stock Windows PowerShell with the generated script"
    );
    let quick_capture = action_section(&manifest, "quick-capture-windows");
    assert!(
        quick_capture.contains(r#"command = ["powershell.exe", "-NoLogo", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/open-capture.ps1"]"#),
        "quick-capture-windows must use stock Windows PowerShell with the generated script"
    );
    assert!(
        !manifest.contains("[[events]]"),
        "must not declare [[events]] (explicit over ambient)"
    );
}
