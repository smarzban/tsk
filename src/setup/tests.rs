use super::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "tsk-setup-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let output = Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("run mklink");
    assert!(
        output.status.success(),
        "mklink failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn windows_setup_rejects_a_junction_ancestor_before_creating_directories() {
    let temp = Temp::new();
    let target = temp.0.join("outside");
    let link = temp.0.join("redirect");
    fs::create_dir(&target).unwrap();
    junction(&link, &target);

    let result = Dir::open(&link.join("herdr"), true);

    assert!(result.is_err(), "a junction ancestor must be refused");
    assert!(!target.join("herdr").exists());
    fs::remove_dir(&link).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_generated_assets_are_filtered_and_pin_the_installed_binary() {
    let binary = Path::new(r"C:\Program Files\O'Brien\tsk.exe");
    let assets = managed_assets(binary, "9.8.7").unwrap();
    let manifest = assets
        .iter()
        .find(|(name, _)| *name == "herdr-plugin.toml")
        .unwrap()
        .1
        .parse::<DocumentMut>()
        .unwrap();
    let actions = manifest["actions"].as_array_of_tables().unwrap();
    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions.get(0).unwrap()["id"].as_str(),
        Some("open-board-windows")
    );
    assert_eq!(
        actions.get(1).unwrap()["id"].as_str(),
        Some("quick-capture-windows")
    );
    assert!(actions.iter().all(|action| {
        action["platforms"].as_array().is_some_and(|platforms| {
            platforms.len() == 1
                && platforms.get(0).and_then(|entry| entry.as_str()) == Some("windows")
        })
    }));
    let pane = manifest["panes"]
        .as_array_of_tables()
        .unwrap()
        .get(0)
        .unwrap();
    assert_eq!(
        pane["command"]
            .as_array()
            .unwrap()
            .get(0)
            .and_then(|entry| entry.as_str()),
        binary.to_str()
    );

    let board = &assets
        .iter()
        .find(|(name, _)| *name == "scripts/open-board.ps1")
        .unwrap()
        .1;
    assert!(board.contains(r"$pluginBin = 'C:\Program Files\O''Brien\tsk.exe'"));
    assert!(!board.contains("$env:TSK_BIN"));
    assert!(!board.contains(r"target\release\tsk.exe"));
}

#[cfg(windows)]
#[test]
fn windows_bare_tsk_path_lookup_resolves_the_exe_suffix() {
    let temp = Temp::new();
    let executable = temp.0.join("tsk.exe");
    fs::write(&executable, b"fixture").unwrap();

    assert_eq!(
        resolve_installed_binary(Path::new("tsk"), temp.0.as_os_str(), &executable).unwrap(),
        executable
    );
}

#[cfg(windows)]
#[test]
fn windows_setup_rejects_a_reparse_point_file_before_reading_it() {
    use std::os::windows::fs::symlink_file;

    let temp = Temp::new();
    let outside = temp.0.join("outside.toml");
    let linked = temp.0.join("config.toml");
    fs::write(&outside, "outside").unwrap();
    if let Err(error) = symlink_file(&outside, &linked) {
        if error.kind() == io::ErrorKind::PermissionDenied {
            eprintln!("symlink privilege unavailable; reparse-file assertion skipped");
            return;
        }
        panic!("create file symlink: {error}");
    }
    let dir = Dir::open(&temp.0, false).unwrap();
    assert!(dir.read(Path::new("config.toml")).is_err());
    assert!(dir.exists(Path::new("config.toml")).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
}

#[test]
fn real_confirmation_parser_accepts_only_yes_and_rejects_eof() {
    for (answer, expected) in [
        (" y \n", true),
        ("YES\n", true),
        ("n\n", false),
        ("\n", false),
        ("sure\n", false),
    ] {
        let mut output = Vec::new();
        let mut input = io::Cursor::new(answer);
        assert_eq!(
            confirm(
                &mut input,
                &mut output,
                "prefix+t",
                "bad\x1b]52;c;evil\x07\nnext"
            )
            .unwrap(),
            expected
        );
        assert!(!output.contains(&0x1b));
        assert!(!output.contains(&7));
        assert!(String::from_utf8(output).unwrap().contains("[y/N]"));
    }
    assert!(confirm(
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        "prefix+t",
        "existing"
    )
    .unwrap_err()
    .to_string()
    .contains("no changes"));
}
#[test]
fn upgrade_replaces_one_registration_and_removes_only_the_intact_old_root() {
    let temp = Temp::new();
    let config = temp.0.join("config.toml");
    let registry = RefCell::new(BTreeMap::<String, PathBuf>::new());
    let mut host = |args: &[&str], _: &Path| -> io::Result<String> {
        if args == ["--version"] {
            return Ok("herdr 0.9.0\n".into());
        }
        if args.starts_with(&["plugin", "link"]) {
            registry
                .borrow_mut()
                .insert("herdr-tsk".into(), args[2].into());
        }
        Ok(serde_json::json!({"result":{"plugins":registry.borrow().iter().map(|(id,root)|serde_json::json!({"plugin_id":id,"plugin_root":root})).collect::<Vec<_>>()}}).to_string())
    };
    let a = run_at(
        &config,
        "0.5.0",
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        false,
        &mut host,
    )
    .unwrap();
    let b = run_at(
        &config,
        "0.5.1",
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        false,
        &mut host,
    )
    .unwrap();
    assert_ne!(a.root, b.root);
    assert!(!a.root.exists());
    assert!(b.root.exists());
    assert_eq!(registry.borrow().len(), 1);
    assert_eq!(registry.borrow()["herdr-tsk"], b.root);
    assert_eq!(fs::read_dir(b.root.parent().unwrap()).unwrap().count(), 1);
    let doc = fs::read_to_string(b.root.join("herdr-plugin.toml"))
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    assert_eq!(doc["version"].as_str(), Some("0.5.1"));
    let again = run_at(
        &config,
        "0.5.1",
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        false,
        &mut host,
    )
    .unwrap();
    assert_eq!(again.root, b.root);
}
#[test]
fn a_registration_whose_root_is_gone_is_reported_before_relinking() {
    let temp = Temp::new();
    let config = temp.0.join("config.toml");
    let gone = temp.0.join("leaked-rehearsal-root");
    let registered = RefCell::new(Some(gone.clone()));
    let mut host = |args: &[&str], _: &Path| -> io::Result<String> {
        if args == ["--version"] {
            return Ok("herdr 0.9.0\n".into());
        }
        if args.starts_with(&["plugin", "link"]) {
            *registered.borrow_mut() = Some(args[2].into());
        }
        Ok(serde_json::json!({"result":{"plugins":registered.borrow().iter().map(|p|serde_json::json!({"plugin_id":"herdr-tsk","plugin_root":p})).collect::<Vec<_>>()}}).to_string())
    };
    let mut notices = Vec::new();
    let result = run_at(
        &config,
        "0.5.0",
        &mut io::Cursor::new(""),
        &mut notices,
        false,
        &mut host,
    )
    .unwrap();
    let notices = String::from_utf8(notices).unwrap();
    assert_eq!(
        notices,
        format!(
            "previous registration at {} is gone, re-registering\n",
            gone.display()
        )
    );
    assert_eq!(*registered.borrow(), Some(result.root.clone()));

    // A live registration says nothing.
    let mut quiet = Vec::new();
    run_at(
        &config,
        "0.5.0",
        &mut io::Cursor::new(""),
        &mut quiet,
        false,
        &mut host,
    )
    .unwrap();
    assert!(quiet.is_empty(), "{:?}", String::from_utf8_lossy(&quiet));
}
#[test]
fn unsuccessful_upgrade_keeps_previous_registration_and_assets() {
    let temp = Temp::new();
    let config = temp.0.join("config.toml");
    let old = RefCell::new(None::<PathBuf>);
    let fail = std::cell::Cell::new(false);
    let mut host = |args: &[&str], _: &Path| -> io::Result<String> {
        if args == ["--version"] {
            return Ok("herdr 0.9.0\n".into());
        }
        if args.starts_with(&["plugin", "link"]) {
            if fail.get() {
                return Err(error("link failed"));
            }
            *old.borrow_mut() = Some(args[2].into());
        }
        Ok(serde_json::json!({"result":{"plugins":old.borrow().iter().map(|p|serde_json::json!({"plugin_id":"herdr-tsk","plugin_root":p})).collect::<Vec<_>>()}}).to_string())
    };
    let a = run_at(
        &config,
        "0.5.0",
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        false,
        &mut host,
    )
    .unwrap();
    let before = fs::read(&config).unwrap();
    fail.set(true);
    assert!(run_at(
        &config,
        "0.5.1",
        &mut io::Cursor::new(""),
        &mut Vec::new(),
        false,
        &mut host
    )
    .is_err());
    assert!(a.root.exists());
    assert_eq!(*old.borrow(), Some(a.root));
    assert_eq!(fs::read(&config).unwrap(), before);
}
#[cfg(unix)]
#[test]
fn descriptor_writes_and_cleanup_stay_pinned_after_parent_swap() {
    use std::os::unix::fs::symlink;
    let temp = Temp::new();
    let dir = Dir::open(&temp.0.join("config"), true).unwrap();
    let outside = temp.0.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::rename(&dir.path, temp.0.join("moved")).unwrap();
    symlink(&outside, &dir.path).unwrap();
    assert!(dir.validate().is_err());
    dir.write_new(Path::new("staged"), "ours").unwrap();
    dir.rename(Path::new("staged"), Path::new("config.toml"))
        .unwrap();
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(
        fs::read_to_string(temp.0.join("moved/config.toml")).unwrap(),
        "ours"
    );
    dir.remove(Path::new("config.toml"), false).unwrap();
    assert!(!temp.0.join("moved/config.toml").exists());
}

#[test]
fn cleanup_keeps_external_checkout_and_tolerates_missing_roots() {
    let temp = Temp::new();
    let base = Dir::open(&temp.0.join("tsk-plugins"), true).unwrap();
    let current = base.child(Path::new("current"), true).unwrap();
    let checkout = temp.0.join("checkout");
    fs::create_dir(&checkout).unwrap();
    fs::write(checkout.join("README.md"), "source checkout").unwrap();
    for old in [
        &checkout,
        &temp.0.join("missing/checkout"),
        &base.path.join("removed"),
    ] {
        cleanup_old(&base, &current, old).unwrap();
    }
    assert_eq!(
        fs::read_to_string(checkout.join("README.md")).unwrap(),
        "source checkout"
    );
    assert!(current.path.exists());
}
#[test]
fn cleanup_preserves_modified_or_incomplete_managed_roots() {
    for missing in [false, true] {
        let temp = Temp::new();
        let config = temp.0.join("config.toml");
        let registry = RefCell::new(None::<PathBuf>);
        let mut host = |args: &[&str], _: &Path| -> io::Result<String> {
            if args == ["--version"] {
                return Ok("herdr 0.9.0\n".into());
            }
            if args.starts_with(&["plugin", "link"]) {
                *registry.borrow_mut() = Some(args[2].into());
            }
            Ok(serde_json::json!({"result":{"plugins":registry.borrow().iter().map(|p|serde_json::json!({"plugin_id":"herdr-tsk","plugin_root":p})).collect::<Vec<_>>()}}).to_string())
        };
        let a = run_at(
            &config,
            "0.5.0",
            &mut io::Cursor::new(""),
            &mut Vec::new(),
            false,
            &mut host,
        )
        .unwrap();
        #[cfg(unix)]
        let file = a.root.join("scripts/open-capture.sh");
        #[cfg(windows)]
        let file = a.root.join("scripts/open-capture.ps1");
        if missing {
            fs::remove_file(&file).unwrap();
        } else {
            fs::write(&file, "user edit").unwrap();
        }
        let error = run_at(
            &config,
            "0.5.1",
            &mut io::Cursor::new(""),
            &mut Vec::new(),
            false,
            &mut host,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains(if missing {
            "stale asset missing"
        } else {
            "stale plugin root was modified"
        }));
        assert!(error.contains(a.root.to_str().unwrap()));
        assert!(a.root.join("herdr-plugin.toml").exists());
        #[cfg(unix)]
        assert!(a.root.join("scripts/open-board.sh").exists());
        #[cfg(windows)]
        assert!(a.root.join("scripts/open-board.ps1").exists());
        if !missing {
            assert_eq!(fs::read_to_string(file).unwrap(), "user edit");
        }
    }
}

#[test]
fn asset_root_name_has_a_fixed_byte_encoding_and_known_digest() {
    assert_eq!(
        asset_root_name(&[("a", "hello".into()), ("notes", "world\n".into())]),
        "2e96f2c494750a92"
    );
    assert_ne!(
        asset_root_name(&[("ab", "c".into())]),
        asset_root_name(&[("a", "bc".into())])
    );
}

#[test]
fn setup_replaces_source_checkout_and_missing_registrations_without_deleting_source() {
    for missing in [false, true] {
        let temp = Temp::new();
        let config = temp.0.join("config.toml");
        let checkout = temp.0.join("source-checkout");
        if !missing {
            fs::create_dir(&checkout).unwrap();
            fs::write(checkout.join("README.md"), "source").unwrap();
        }
        let registered = RefCell::new(checkout.clone());
        let mut host = |args: &[&str], _: &Path| -> io::Result<String> {
            if args == ["--version"] {
                return Ok("herdr 0.9.0\n".into());
            }
            if args.starts_with(&["plugin", "link"]) {
                *registered.borrow_mut() = args[2].into();
            }
            Ok(serde_json::json!({"result":{"plugins":[{"plugin_id":"herdr-tsk","plugin_root":*registered.borrow()}]}}).to_string())
        };
        let result = run_at(
            &config,
            "0.5.0",
            &mut io::Cursor::new(""),
            &mut Vec::new(),
            false,
            &mut host,
        )
        .unwrap();
        assert_eq!(*registered.borrow(), result.root);
        if !missing {
            assert_eq!(
                fs::read_to_string(checkout.join("README.md")).unwrap(),
                "source"
            );
        }
        assert!(config.exists());
    }
}

#[test]
fn backup_name_is_utc_timestamp_and_suffixes_collisions() {
    let known = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_789_223_422);
    assert_eq!(utc_timestamp(known), "20260912-143022");
    let leap_day_end =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(951_868_799);
    assert_eq!(utc_timestamp(leap_day_end), "20000229-235959");
    let year_end =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_735_603_200);
    assert_eq!(utc_timestamp(year_end), "20241231-000000");
    assert_eq!(
        utc_timestamp(std::time::SystemTime::UNIX_EPOCH),
        "19700101-000000"
    );
    let name = backup_file_name(known, &mut |_| Ok(false)).unwrap();
    assert_eq!(name, "config.toml.tsk-backup-20260912-143022");
    let mut taken = std::collections::BTreeSet::from([name.clone()]);
    let collided = backup_file_name(known, &mut |candidate: &str| {
        Ok(!taken.insert(candidate.to_owned()))
    })
    .unwrap();
    assert_eq!(collided, "config.toml.tsk-backup-20260912-143022-1");
    let twice =
        backup_file_name(known, &mut |candidate: &str| Ok(taken.contains(candidate))).unwrap();
    assert_eq!(twice, "config.toml.tsk-backup-20260912-143022-2");
}

#[test]
fn generated_backup_name_matches_the_documented_pattern() {
    let name = backup_file_name(std::time::SystemTime::now(), &mut |_| Ok(false)).unwrap();
    let stamp = name
        .strip_prefix("config.toml.tsk-backup-")
        .expect("prefix");
    let parts: Vec<&str> = stamp.split('-').collect();
    assert!(parts.len() == 2 || parts.len() == 3, "{stamp}");
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    assert!(parts[0].len() == 8 && digits(parts[0]), "{stamp}");
    assert!(parts[1].len() == 6 && digits(parts[1]), "{stamp}");
    assert!(parts.get(2).is_none_or(|extra| digits(extra)), "{stamp}");
}

#[test]
fn old_herdr_is_refused_before_any_write_with_an_actionable_message() {
    assert!(require_min_herdr("herdr 0.9.0\n").is_ok());
    assert!(require_min_herdr("herdr 1.2.0-beta.1").is_ok());
    assert!(require_min_herdr("herdr 0.10.0").is_ok());
    assert!(require_min_herdr("herdr 0.9.0+build.7").is_ok());
    assert!(require_min_herdr("herdr 0.9.0+build-7").is_ok());
    assert!(require_min_herdr("herdr 1.0.0+20130313144700-sha").is_ok());
    let err = require_min_herdr("herdr 0.6.8\n").unwrap_err().to_string();
    assert_eq!(
        err,
        "herdr 0.6.8 found; tsk needs 0.9.0 or newer. Update Herdr, then run tsk setup herdr again"
    );
    // A pre-release of the minimum is not the minimum.
    assert!(require_min_herdr("herdr 0.9.0-beta.2")
        .unwrap_err()
        .to_string()
        .starts_with("herdr 0.9.0-beta.2 found"));
    // Only the version after the word `herdr` counts, never a stray semver in a banner.
    assert!(require_min_herdr("warning: helper 1.2.3\nherdr 0.6.8")
        .unwrap_err()
        .to_string()
        .starts_with("herdr 0.6.8 found"));
    for garbage in [
        "something 1.2.3",
        "herdr",
        "herdr abc",
        "herdr 1.2",
        "herdr 1.2.3.4",
        "",
    ] {
        let unreadable = require_min_herdr(garbage).unwrap_err().to_string();
        assert!(
            unreadable.starts_with("could not read the Herdr version"),
            "{garbage:?}: {unreadable}"
        );
    }
}
