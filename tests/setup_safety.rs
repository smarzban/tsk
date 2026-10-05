#![cfg(unix)]
#[path = "support/setup_host.rs"]
mod fixture;
use fixture::Host;
use std::{
    fs,
    os::unix::{fs::symlink, process::CommandExt},
    time::{Duration, Instant},
};
fn host() -> Host {
    Host::new(env!("CARGO_BIN_EXE_tsk"))
}
fn ok(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn scalar_and_single_element_builtin_are_removed() {
    for binding in ["'prefix+t'", "['prefix+t']"] {
        let updated =
            tsk_tui::setup::edit_bindings(&format!("[keys]\nnew_tab={binding}\n"), true, |_, _| {
                Ok(true)
            })
            .unwrap();
        assert!(!updated.contains("new_tab"));
    }
}
#[test]
fn setup_uses_cli_harness() {
    let output =
        tsk_tui::cli::run_with(["tsk", "setup", "herdr", "--help"], std::io::empty(), false);
    assert_eq!(output.code, 0);
    assert!(output.stdout.contains("Values"));
    assert!(output.stdout.contains("herdr"));
    assert!(output.stdout.contains("Exit:"));
}
#[test]
fn a_gone_registration_is_reported_on_stderr_even_when_the_relink_fails() {
    let gone_notice = |h: &Host| {
        let gone = h.root.join("leaked-rehearsal-root");
        fs::write(
            h.root.join("registry"),
            format!(
                "{{\"result\":{{\"plugins\":[{{\"plugin_id\":\"herdr-tsk\",\"plugin_root\":\"{}\"}}]}}}}",
                gone.display()
            ),
        )
        .unwrap();
        format!(
            "previous registration at {} is gone, re-registering\n",
            gone.display()
        )
    };

    let h = host();
    let expected = gone_notice(&h);
    let output = h.run("");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), expected);
    assert!(!String::from_utf8(output.stdout).unwrap().contains("gone"));
    assert!(h.linked().exists());

    // The notice is written before `plugin link`, so a refused relink still shows it.
    let h = host();
    let expected = gone_notice(&h);
    let output = h.run("link-fail");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with(&expected), "{stderr}");
    assert!(stderr.contains("refused"), "{stderr}");

    // A live registration says nothing.
    let h = host();
    let first = ok(h.run(""));
    assert!(!first.contains("gone"));
    let output = h.run("");
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn bare_path_install_materializes_capture_and_version_without_printing_the_root() {
    let h = host();
    let output = ok(h.run(""));
    let root = h.linked();
    assert!(
        !output.contains(root.to_str().unwrap()),
        "the content-addressed root is not user-facing on success: {output}"
    );
    assert!(output.contains("Shortcuts:      prefix+t board, prefix+a quick capture"));
    assert_eq!(
        fs::read(root.join("scripts/open-capture.sh")).unwrap(),
        include_bytes!("../scripts/open-capture.sh")
    );
    let capture = std::process::Command::new("bash")
        .arg(root.join("scripts/open-capture.sh"))
        .env("HERDR_BIN_PATH", h.bin.join("herdr"))
        .env("FIXTURE", &h.root)
        .output()
        .unwrap();
    assert!(capture.status.success());
    assert!(h.calls().contains("plugin pane open --plugin herdr-tsk --entrypoint board --placement popup --width 80 --height 15 --focus --env TSK_MODE=capture"));
    let doc = fs::read_to_string(root.join("herdr-plugin.toml"))
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let actions = doc["actions"].as_array_of_tables().unwrap();
    assert_eq!(actions.len(), 2);
    for (id, script) in [
        ("open-board", "scripts/open-board.sh"),
        ("quick-capture", "scripts/open-capture.sh"),
    ] {
        let action = actions
            .iter()
            .find(|a| a["id"].as_str() == Some(id))
            .unwrap();
        let command = action["command"].as_array().unwrap();
        assert_eq!(command.len(), 2);
        assert_eq!(command.get(0).unwrap().as_str(), Some("bash"));
        assert_eq!(command.get(1).unwrap().as_str(), Some(script));
    }
    assert_eq!(doc["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    let (major, minor, patch) = tsk_tui::setup::MIN_HERDR_VERSION;
    assert_eq!(
        doc["min_herdr_version"].as_str(),
        Some(format!("{major}.{minor}.{patch}").as_str())
    );
    assert_eq!(
        doc["panes"].as_array_of_tables().unwrap().get(0).unwrap()["command"][0].as_str(),
        h.bin.join("tsk").to_str()
    );
}
#[test]
fn rejected_native_config_leaves_config_untouched_and_never_links() {
    let h = host();
    fs::write(&h.config, "# original\n").unwrap();
    let output = h.run("invalid");
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&h.config).unwrap(), "# original\n");
    assert!(!h.calls().contains("plugin link"));
    assert!(!output.stderr.contains(&0x1b));
    assert!(!output.stderr.contains(&7));
    let checked = fs::read_to_string(h.root.join("checked")).unwrap();
    assert_ne!(checked, h.config.to_str().unwrap());
    assert!(!std::path::Path::new(&checked).exists());
}
#[test]
fn old_herdr_is_refused_before_config_check_with_a_readable_message() {
    let h = host();
    fs::write(&h.config, "# original\n").unwrap();
    let output = h.command().env("HERDR_VERSION", "0.6.8").output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.trim(),
        "tsk setup: herdr 0.6.8 found; tsk needs 0.9.0 or newer. Update Herdr, then run tsk setup herdr again"
    );
    assert_eq!(fs::read_to_string(&h.config).unwrap(), "# original\n");
    assert!(!h.calls().contains("config check"));
    assert!(!h.calls().contains("plugin link"));
    assert!(!h.root.join("checked").exists());
}
#[test]
fn supported_windows_preview_reaches_config_check_and_plugin_link() {
    let h = host();
    let output = h
        .command()
        .env("HERDR_VERSION", "0.9.0-preview.2026-09-16-2c29fb29e302")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = h.calls();
    assert!(calls.contains("config check"), "{calls}");
    assert!(calls.contains("plugin link"), "{calls}");
}

#[test]
fn herdr_stderr_in_a_setup_failure_keeps_its_line_breaks() {
    let h = host();
    let output = h.run("invalid");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("\\u{000a}"), "{stderr}");
    assert!(
        stderr.contains("invalid config\n  second diagnostic line"),
        "Herdr's second line is a real line: {stderr}"
    );
    assert!(
        stderr.contains("\\u{001b}"),
        "escape sequences stay escaped: {stderr}"
    );
}
#[test]
fn noninteractive_conflict_has_zero_host_calls() {
    let h = host();
    fs::write(&h.config, "[keys]\nnew_tab='prefix+t'\n").unwrap();
    assert!(!h.run("").status.success());
    assert_eq!(h.calls(), "");
    assert_eq!(fs::read_dir(h.config.parent().unwrap()).unwrap().count(), 1);
}
#[test]
fn replacement_backs_up_original_and_renames_new_document() {
    use std::os::unix::fs::MetadataExt;
    let h = host();
    let original = "# preserved\n[ui]\nmouse_capture=true\n";
    fs::write(&h.config, original).unwrap();
    let inode = fs::metadata(&h.config).unwrap().ino();
    let output = ok(h.run(""));
    assert_ne!(inode, fs::metadata(&h.config).unwrap().ino());
    let backups: Vec<_> = fs::read_dir(h.config.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.toml.tsk-backup-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
    assert!(output.contains(backups[0].to_str().unwrap()));
    let updated = fs::read_to_string(&h.config).unwrap();
    assert!(updated.contains(original.trim()));
    assert!(updated.contains("herdr-tsk.quick-capture"));
}
#[test]
fn backup_name_is_a_timestamp_not_a_uuid() {
    let h = host();
    fs::write(&h.config, "# original\n").unwrap();
    let output = ok(h.run(""));
    let backups: Vec<String> = fs::read_dir(h.config.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("config.toml.tsk-backup-"))
        .collect();
    assert_eq!(backups.len(), 1, "{output}");
    let stamp = backups[0]
        .strip_prefix("config.toml.tsk-backup-")
        .unwrap()
        .to_owned();
    let parts: Vec<&str> = stamp.split('-').collect();
    assert!(parts.len() == 2 || parts.len() == 3, "{stamp}");
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    assert!(parts[0].len() == 8 && digits(parts[0]), "{stamp}");
    assert!(parts[1].len() == 6 && digits(parts[1]), "{stamp}");
    assert!(parts.get(2).is_none_or(|extra| digits(extra)), "{stamp}");
    assert!(output.contains(
        h.config
            .parent()
            .unwrap()
            .join(&backups[0])
            .to_str()
            .unwrap()
    ));
}

#[test]
fn two_setups_keep_distinct_backups_instead_of_overwriting() {
    let h = host();
    fs::write(&h.config, "# first\n").unwrap();
    ok(h.run(""));
    fs::write(&h.config, "# second\n").unwrap();
    ok(h.run(""));
    let mut backups: Vec<String> = fs::read_dir(h.config.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("config.toml.tsk-backup-"))
        .collect();
    backups.sort();
    assert_eq!(backups.len(), 2, "{backups:?}");
    assert_ne!(backups[0], backups[1]);
}

#[test]
fn argv0_mismatch_is_refused_without_registration() {
    let h = host();
    let mut command = h.command();
    command.arg0("herdr");
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not match"));
    assert_eq!(h.calls(), "");
}
#[test]
fn symlink_config_and_parent_and_assets_are_refused() {
    for which in [
        "config", "parent", "asset", "scripts", "root", "base", "lock",
    ] {
        let h = host();
        let outside = h.root.join("outside");
        match which {
            "config" => {
                fs::write(outside.join("file"), "# target").unwrap();
                symlink(outside.join("file"), &h.config).unwrap();
            }
            "parent" => {
                fs::remove_dir(h.config.parent().unwrap()).unwrap();
                symlink(&outside, h.config.parent().unwrap()).unwrap();
            }
            "lock" => {
                fs::write(outside.join("file"), "# target").unwrap();
                symlink(
                    outside.join("file"),
                    h.config.parent().unwrap().join(".tsk-setup.lock"),
                )
                .unwrap();
            }
            _ => {
                ok(h.run(""));
                let root = h.linked();
                let target = match which {
                    "asset" => root.join("scripts/open-capture.sh"),
                    "scripts" => root.join("scripts"),
                    "root" => root.clone(),
                    _ => root.parent().unwrap().to_path_buf(),
                };
                if target.is_dir() {
                    fs::remove_dir_all(&target).unwrap();
                    symlink(&outside, &target).unwrap();
                } else {
                    fs::remove_file(&target).unwrap();
                    fs::write(outside.join("file"), "# target").unwrap();
                    symlink(outside.join("file"), &target).unwrap();
                }
            }
        }
        let output = h.run("");
        assert!(!output.status.success(), "{which}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("symlink"),
            "{which}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!outside.join("config.toml").exists());
        if outside.join("file").exists() {
            assert_eq!(
                fs::read_to_string(outside.join("file")).unwrap(),
                "# target"
            );
        }
    }
}
#[test]
fn changing_parent_during_validation_cannot_redirect_writes_or_cleanup() {
    let h = host();
    let output = h.run("swap");
    assert!(!output.status.success());
    assert!(!h.calls().contains("plugin link"));
    assert_eq!(fs::read_dir(h.root.join("outside")).unwrap().count(), 0);
    assert!(!h.root.join("moved/config.toml").exists());
}
#[test]
fn config_changed_during_host_calls_is_not_overwritten() {
    for scenario in ["change", "change-link"] {
        let h = host();
        fs::write(&h.config, "# original\n").unwrap();
        let output = h.run(scenario);
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(&h.config).unwrap(), "# external edit\n");
        if scenario == "change" {
            assert!(!h.calls().contains("plugin link"));
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("plugin registered"));
        }
    }
}
#[test]
fn contention_refuses_but_killed_owner_does_not_leave_a_stale_lock() {
    let h = host();
    let mut child = h
        .command()
        .env("SCENARIO", "block")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !h.root.join("waiting").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let refused = h.run("");
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("lock"));
    child.kill().unwrap();
    child.wait().unwrap();
    fs::write(h.root.join("release"), "").unwrap();
    ok(h.run(""));
}
#[test]
fn modified_asset_error_names_file() {
    let h = host();
    ok(h.run(""));
    let path = h.linked().join("scripts/open-board.sh");
    fs::write(&path, "broken").unwrap();
    let output = h.run("");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(path.to_str().unwrap()));
}

#[test]
fn setup_resolves_default_config_paths_and_ignores_empty_environment_values() {
    for (override_value, xdg_value, home_value, expected) in [
        (
            None,
            Some("xdg"),
            Some("home"),
            Some("xdg/herdr/config.toml"),
        ),
        (
            Some(""),
            Some("xdg"),
            Some("home"),
            Some("xdg/herdr/config.toml"),
        ),
        (
            None,
            None,
            Some("home"),
            Some("home/.config/herdr/config.toml"),
        ),
        (
            Some(""),
            Some(""),
            Some("home"),
            Some("home/.config/herdr/config.toml"),
        ),
        (
            Some(""),
            Some("xdg"),
            Some(""),
            Some("xdg/herdr/config.toml"),
        ),
        (Some(""), Some(""), Some(""), None),
    ] {
        let h = host();
        let mut command = h.command();
        command
            .current_dir(&h.root)
            .env_remove("HERDR_CONFIG_PATH")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("HOME");
        for (key, value) in [
            ("HERDR_CONFIG_PATH", override_value),
            ("XDG_CONFIG_HOME", xdg_value),
            ("HOME", home_value),
        ] {
            if let Some(value) = value {
                command.env(
                    key,
                    if value.is_empty() {
                        String::new()
                    } else {
                        h.root.join(value).display().to_string()
                    },
                );
            }
        }
        let output = command.output().unwrap();
        if let Some(expected) = expected {
            ok(output);
            assert!(h.root.join(expected).is_file(), "{expected}");
        } else {
            assert!(!output.status.success());
            assert_eq!(h.calls(), "");
        }
        assert!(!h.root.join("herdr/config.toml").exists());
    }
}
#[test]
fn failed_link_leaves_no_config_backup() {
    let h = host();
    fs::write(&h.config, "# original\n").unwrap();
    assert!(!h.run("link-fail").status.success());
    for entry in fs::read_dir(h.config.parent().unwrap()).unwrap() {
        assert!(!entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("backup"));
    }
    assert_eq!(fs::read_to_string(&h.config).unwrap(), "# original\n");
}
