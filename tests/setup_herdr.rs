use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use tsk_tui::setup::edit_bindings;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "tsk-setup-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

/// The plugin action IDs are platform-specific: `-windows` suffix on Windows,
/// bare on Unix. `BINDINGS` in `src/setup.rs` mirrors this split.
#[cfg(unix)]
const OPEN_BOARD: &str = "herdr-tsk.open-board";
#[cfg(unix)]
const QUICK_CAPTURE: &str = "herdr-tsk.quick-capture";
#[cfg(windows)]
const OPEN_BOARD: &str = "herdr-tsk.open-board-windows";
#[cfg(windows)]
const QUICK_CAPTURE: &str = "herdr-tsk.quick-capture-windows";

#[test]
fn creates_bindings_once_and_preserves_other_settings() {
    let source =
        "# my settings\n[ui]\nmouse_capture = true\n[keys]\nprefix = 'alt+z' # custom prefix\n";
    let first = edit_bindings(source, false, |_, _| panic!("no conflicts")).unwrap();
    assert!(first.contains("# my settings"));
    assert!(first.contains("prefix = 'alt+z' # custom prefix"));
    assert!(first.contains(OPEN_BOARD));
    assert!(first.contains(QUICK_CAPTURE));
    assert_eq!(
        first,
        edit_bindings(&first, false, |_, _| panic!("idempotent")).unwrap()
    );
}
#[test]
fn conflict_requires_terminal_and_decline_preserves_original_binding() {
    let source = "[[keys.command]]\nkey = 'prefix+t'\ntype = 'shell'\ncommand = 'my-tool'\n";
    assert!(edit_bindings(source, false, |_, _| panic!("must not prompt")).is_err());
    let declined = edit_bindings(source, true, |key, _| {
        assert_eq!(key, "prefix+t");
        Ok(false)
    })
    .unwrap();
    assert!(declined.contains("my-tool"));
    assert!(!declined.contains(OPEN_BOARD));
    assert!(declined.contains(QUICK_CAPTURE));
    let accepted = edit_bindings(source, true, |_, _| Ok(true)).unwrap();
    assert!(!accepted.contains("my-tool"));
    assert!(accepted.contains(OPEN_BOARD));
}
#[test]
fn builtin_and_multi_binding_conflicts_preserve_unrelated_keys() {
    let source = "[keys]\nnew_tab = ['prefix+t', 'prefix+c']\ncommand = [{key = ['prefix+a', 'prefix+f'], type = 'shell', command = 'other'}]\n";
    let mut prompts = 0;
    let updated = edit_bindings(source, true, |_, _| {
        prompts += 1;
        Ok(true)
    })
    .unwrap();
    assert_eq!(prompts, 2);
    let doc = updated.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(doc["keys"]["new_tab"].as_array().unwrap().len(), 1);
    assert_eq!(doc["keys"]["new_tab"][0].as_str(), Some("prefix+c"));
    assert!(updated.contains("prefix+f"));
    assert!(updated.contains("other"));
}
#[test]
fn noninteractive_conflict_preserves_config_bytes() {
    let root = temp();
    let config = root.join("config.toml");
    let source = "[keys]\nnew_tab = 'prefix+t'\n";
    fs::write(&config, source).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr"])
        .env("HERDR_CONFIG_PATH", &config)
        .env("XDG_CONFIG_HOME", &root)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read_to_string(&config).unwrap(), source);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn setup_help_is_headless_and_bad_arguments_are_usage_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("Values"));
    assert!(help.contains("herdr"));
    assert!(help.contains("Exit:"));
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr", "--force"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// On Windows, `tsk setup herdr` must read/write keybindings at
/// `%APPDATA%\herdr\config.toml`, the path Herdr actually reads -- not
/// `%USERPROFILE%\.config\herdr\config.toml` (the Unix path). This test
/// verifies `tsk setup herdr --check` finds a config at the APPDATA path.
#[cfg(windows)]
#[test]
fn setup_reads_config_from_appdata_not_userprofile_config() {
    let root = temp();
    let appdata = root.join("appdata");
    let config_dir = appdata.join("herdr");
    let config = config_dir.join("config.toml");
    fs::create_dir_all(&config_dir).unwrap();

    // Write a config with the Windows action IDs bound.
    fs::write(
        &config,
        "[keys]
\n[[keys.command]]
\nkey = 'prefix+t'
\ntype = 'plugin_action'
\ncommand = 'herdr-tsk.open-board-windows'
\n[[keys.command]]
\nkey = 'prefix+a'
\ntype = 'plugin_action'
\ncommand = 'herdr-tsk.quick-capture-windows'
",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr", "--check"])
        .env("APPDATA", &appdata)
        .env_remove("HERDR_CONFIG_PATH")
        .env("XDG_CONFIG_HOME", root.join("xdg-must-not-win"))
        .env_remove("USERPROFILE")
        .env_remove("HOME")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "bound",
        "--check must find the config at %APPDATA%/herdr/config.toml and report bound"
    );

    // Remove the config and verify --check reports unbound (confirming it was
    // reading from APPDATA, not somewhere else).
    fs::remove_file(&config).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr", "--check"])
        .env("APPDATA", &appdata)
        .env_remove("HERDR_CONFIG_PATH")
        .env("XDG_CONFIG_HOME", root.join("xdg-must-not-win"))
        .env_remove("USERPROFILE")
        .env_remove("HOME")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "unbound",
        "--check must report unbound when the config is missing from APPDATA"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn setup_falls_back_to_userprofile_roaming_when_appdata_is_missing() {
    let root = temp();
    let config_dir = root.join("AppData").join("Roaming").join("herdr");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[keys]\n\n[[keys.command]]\nkeys = 't'\ntype = 'plugin_action'\ncommand = 'herdr-tsk.open-board-windows'\n\n[[keys.command]]\nkeys = 'a'\ntype = 'plugin_action'\ncommand = 'herdr-tsk.quick-capture-windows'\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr", "--check"])
        .env_remove("APPDATA")
        .env("USERPROFILE", &root)
        .env_remove("HERDR_CONFIG_PATH")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HOME")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "bound");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn installed_symlink_is_shared_with_plugin_and_rerun_does_not_duplicate_assets() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = temp();
    let bin = root.join("bin with spaces");
    fs::create_dir(&bin).unwrap();
    let installed = bin.join("tsk");
    symlink(env!("CARGO_BIN_EXE_tsk"), &installed).unwrap();
    let host = bin.join("herdr");
    fs::write(&host, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'herdr 0.9.0'; exit 0; fi\nif [ \"$1 $2\" = 'plugin list' ]; then if [ -f \"$SETUP_LINK\" ]; then printf '{\"result\":{\"plugins\":[{\"plugin_id\":\"herdr-tsk\",\"plugin_root\":\"%s\"}]}}' \"$(cat \"$SETUP_LINK\")\"; else printf '{\"result\":{\"plugins\":[]}}'; fi; exit 0; fi\nif [ \"$1 $2\" = 'plugin link' ]; then printf '%s' \"$3\" > \"$SETUP_LINK\"; fi\nexit 0\n").unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let config = root.join("herdr/config.toml");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let run = || {
        Command::new(&installed)
            .args(["setup", "herdr"])
            .env("PATH", &path)
            .env("HERDR_CONFIG_PATH", &config)
            .env("SETUP_LINK", root.join("linked"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let first = fs::read_to_string(&config).unwrap();
    let linked = fs::read_to_string(root.join("linked")).unwrap();
    let manifest = fs::read_to_string(PathBuf::from(&linked).join("herdr-plugin.toml"))
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    assert!(manifest.get("build").is_none());
    assert_eq!(
        manifest["panes"]
            .as_array_of_tables()
            .unwrap()
            .get(0)
            .unwrap()["command"][0]
            .as_str(),
        installed.to_str()
    );
    let script = fs::read_to_string(PathBuf::from(&linked).join("scripts/open-board.sh")).unwrap();
    assert!(script.contains(&format!("plugin_bin='{}'", installed.display())));
    assert!(!script.contains("plugin_bin=\"${TSK_BIN:"));
    assert!(run().status.success());
    assert_eq!(fs::read_to_string(&config).unwrap(), first);
    assert_eq!(fs::read_to_string(root.join("linked")).unwrap(), linked);
    assert_eq!(
        fs::read_dir(root.join("herdr/tsk-plugins"))
            .unwrap()
            .count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn failed_host_registration_keeps_original_config_and_releases_lock() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp();
    let host = root.join("herdr");
    fs::write(
        &host,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'herdr 0.9.0'; exit 0; fi\nif [ \"$1 $2\" = 'plugin list' ]; then printf '{\"result\":{\"plugins\":[]}}'; exit 0; fi\nif [ \"$1 $2\" = 'plugin link' ]; then echo refused >&2; exit 1; fi\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let config = root.join("config.toml");
    let original = "# keep this\n[ui]\nmouse_capture = true\n";
    fs::write(&config, original).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(["setup", "herdr"])
        .env(
            "PATH",
            format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
        )
        .env("HERDR_CONFIG_PATH", &config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("refused"));
    assert_eq!(fs::read_to_string(&config).unwrap(), original);
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".tsk-setup.lock"))
        .unwrap()
        .try_lock()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn whitespace_in_chord_is_detected_but_shifted_uppercase_is_not_replaced() {
    assert!(edit_bindings("[keys]\nnew_tab='prefix+ t'\n", false, |_, _| panic!()).is_err());
    let updated = edit_bindings("[keys]\nnew_tab='prefix+T'\n", false, |_, _| panic!()).unwrap();
    assert!(updated.contains("new_tab='prefix+T'"));
}

#[test]
fn an_action_bound_on_a_custom_key_is_kept_and_no_default_chord_is_added() {
    // The user moved the board to prefix+b; a rerun (or `tsk update`) must not add prefix+t.
    let source = format!(
        "[[keys.command]]\nkey = 'prefix+b'\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD
    );
    let edited = edit_bindings(&source, false, |_, _| panic!("no conflicts")).unwrap();
    let doc = edited.parse::<toml_edit::DocumentMut>().unwrap();
    let commands = doc["keys"]["command"].as_array_of_tables().unwrap();
    let boards: Vec<_> = commands
        .iter()
        .filter(|t| t["command"].as_str() == Some(OPEN_BOARD))
        .collect();
    assert_eq!(boards.len(), 1, "{edited}");
    assert_eq!(boards[0]["key"].as_str(), Some("prefix+b"));
    assert!(edited.contains(QUICK_CAPTURE), "{edited}");
    assert!(!edited.contains("'prefix+t'"), "{edited}");
}

#[test]
fn commands_bound_matches_both_plugin_commands_on_any_key() {
    use tsk_tui::setup::commands_bound;
    let both = format!(
        "[[keys.command]]\nkey = 'prefix+b'\ntype = 'plugin_action'\ncommand = '{}'\n[[keys.command]]\nkey = 'prefix+q'\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD, QUICK_CAPTURE
    );
    assert!(commands_bound(&both));
    let inline = format!(
        "[keys]\ncommand = [{{key = 'prefix+t', type = 'plugin_action', command = '{}'}}, {{key = 'prefix+a', type = 'plugin_action', command = '{}'}}]\n",
        OPEN_BOARD, QUICK_CAPTURE
    );
    assert!(commands_bound(&inline));
    let one = format!(
        "[[keys.command]]\nkey = 'prefix+t'\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD
    );
    assert!(!commands_bound(&one));
    let wrong_type = format!(
        "[[keys.command]]\nkey = 'prefix+t'\ntype = 'shell'\ncommand = '{}'\n[[keys.command]]\nkey = 'prefix+a'\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD, QUICK_CAPTURE
    );
    assert!(!commands_bound(&wrong_type));
    assert!(!commands_bound(""));
    assert!(!commands_bound("not = [toml"));
}

#[test]
fn a_keyless_or_empty_key_plugin_action_does_not_count_as_bound() {
    use tsk_tui::setup::commands_bound;
    let keyless = format!(
        "[[keys.command]]\ntype = 'plugin_action'\ncommand = '{}'\n[[keys.command]]\nkey = []\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD, QUICK_CAPTURE
    );
    assert!(!commands_bound(&keyless));
    let edited = edit_bindings(&keyless, false, |_, _| panic!("no conflicts")).unwrap();
    let doc = edited.parse::<toml_edit::DocumentMut>().unwrap();
    let keys: Vec<Option<&str>> = doc["keys"]["command"]
        .as_array_of_tables()
        .unwrap()
        .iter()
        .map(|t| t.get("key").and_then(|k| k.as_str()))
        .collect();
    assert!(keys.contains(&Some("prefix+t")), "{edited}");
    assert!(keys.contains(&Some("prefix+a")), "{edited}");
}

#[test]
fn a_builtin_shadowing_the_default_chord_is_still_offered_for_repair() {
    // The plugin action sits on prefix+t, but a builtin also claims prefix+t: the action is
    // bound on its default key, so the custom-key skip must not hide the conflict.
    let source = format!(
        "[keys]\nnew_tab = ['prefix+t']\n[[keys.command]]\nkey = 'prefix+t'\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD
    );
    assert!(edit_bindings(&source, false, |_, _| panic!("must not prompt")).is_err());
    let mut asked = Vec::new();
    let repaired = edit_bindings(&source, true, |key, _| {
        asked.push(key.to_string());
        Ok(true)
    })
    .unwrap();
    assert_eq!(asked, vec!["prefix+t".to_string()]);
    assert!(!repaired.contains("new_tab"), "{repaired}");
}

#[test]
fn bound_shortcuts_report_the_keys_each_command_is_on() {
    use tsk_tui::setup::bound_shortcuts;
    let source = format!(
        "[[keys.command]]\nkey = ['prefix+b', 'prefix+t']\ntype = 'plugin_action'\ncommand = '{}'\n",
        OPEN_BOARD
    );
    assert_eq!(
        bound_shortcuts(&source),
        vec![("prefix+b / prefix+t".to_string(), "board")]
    );
    let edited = edit_bindings(&source, false, |_, _| panic!("no conflicts")).unwrap();
    assert_eq!(
        bound_shortcuts(&edited),
        vec![
            ("prefix+b / prefix+t".to_string(), "board"),
            ("prefix+a".to_string(), "quick capture"),
        ]
    );
    assert!(bound_shortcuts("").is_empty());
}
