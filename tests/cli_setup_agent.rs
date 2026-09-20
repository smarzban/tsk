use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tsk_tui::cli::run_with;

/// The plugin action IDs are platform-specific: `-windows` suffix on Windows,
/// bare on Unix. `BINDINGS` in `src/setup.rs` mirrors this split.
#[cfg(unix)]
const OPEN_BOARD_ACTION: &str = "herdr-tsk.open-board";
#[cfg(unix)]
const QUICK_CAPTURE_ACTION: &str = "herdr-tsk.quick-capture";
#[cfg(windows)]
const OPEN_BOARD_ACTION: &str = "herdr-tsk.open-board-windows";
#[cfg(windows)]
const QUICK_CAPTURE_ACTION: &str = "herdr-tsk.quick-capture-windows";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsk-agent-site-setup-{label}-{nanos}-{seq}"));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn cli(args: &[&str]) -> tsk_tui::cli::CliOutput {
    run_with(args.iter().copied(), Cursor::new(Vec::<u8>::new()), true)
}

fn cli_non_tty(args: &[&str]) -> tsk_tui::cli::CliOutput {
    run_with(args.iter().copied(), Cursor::new(Vec::<u8>::new()), false)
}

fn skill_source() -> &'static str {
    include_str!("../skills/tsk-cli/SKILL.md")
}

/// Join path components with the native separator so assertions match `PathBuf::display()`
/// output on both Unix (`/`) and Windows (`\`).
fn native_path(components: &[&str]) -> String {
    let mut path = PathBuf::new();
    for component in components {
        path.push(component);
    }
    path.display().to_string()
}

struct OmpEnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl OmpEnvGuard {
    fn cleared() -> Self {
        let keys = [
            "OMP_PROFILE",
            "PI_PROFILE",
            "PI_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
        ];
        let values = keys
            .into_iter()
            .map(|key| {
                let value = std::env::var_os(key);
                std::env::remove_var(key);
                (key, value)
            })
            .collect();
        Self(values)
    }
}

impl Drop for OmpEnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn skill_dir_writes_full_skill_prints_path_exits_0() {
    let _lock = env_lock();
    let root = temp_dir("write");
    let skill_dir = root.join("skills");
    let output = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    let written = skill_dir.join("tsk-cli").join("SKILL.md");
    assert_eq!(output.code, 0);
    assert!(output.stderr.is_empty());
    assert!(
        output.stdout.contains(written.to_str().expect("utf-8")),
        "stdout should print the written path, got {:?}",
        output.stdout
    );
    assert_eq!(
        fs::read_to_string(&written).expect("read written skill"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn equal_version_without_force_is_skill_exists_and_leaves_file() {
    let _lock = env_lock();
    let root = temp_dir("exists");
    let skill_dir = root.join("skills");
    let first = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    assert_eq!(first.code, 0);
    let written = skill_dir.join("tsk-cli/SKILL.md");
    let second = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    assert_eq!(second.code, 1);
    assert!(
        second.stderr.contains("skill-exists"),
        "stderr should name skill-exists, got {:?}",
        second.stderr
    );
    assert_eq!(
        fs::read_to_string(&written).expect("unchanged"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn missing_or_mismatched_version_updates_without_force() {
    let _lock = env_lock();
    let root = temp_dir("update");
    let skill_dir = root.join("skills");
    let written = skill_dir.join("tsk-cli/SKILL.md");
    fs::create_dir_all(written.parent().expect("parent")).expect("mkdir");
    fs::write(&written, "stale-marker\n").expect("stamp missing version");
    let missing = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    assert_eq!(missing.code, 0, "{missing:?}");
    assert_eq!(
        fs::read_to_string(&written).expect("updated"),
        skill_source()
    );

    let outdated =
        "---\nname: tsk-cli\ndescription: stale\nversion: 0.0.1\n---\n\nstale body\n".to_string();
    fs::write(&written, &outdated).expect("stamp old version");
    let updated = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    assert_eq!(updated.code, 0, "{updated:?}");
    assert_eq!(
        fs::read_to_string(&written).expect("updated again"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn force_overwrites() {
    let _lock = env_lock();
    let root = temp_dir("force");
    let skill_dir = root.join("skills");
    let written = skill_dir.join("tsk-cli/SKILL.md");
    fs::create_dir_all(written.parent().expect("parent")).expect("mkdir");
    fs::write(&written, "stale-marker\n").expect("stamp file");
    let output = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
        "--force",
    ]);
    assert_eq!(output.code, 0);
    assert_eq!(
        fs::read_to_string(&written).expect("overwritten"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bare_setup_non_tty_lists_targets_and_writes_nothing() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("list");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home");
    let previous_home = std::env::var_os("HOME");
    let previous_xdg = std::env::var_os("XDG_CONFIG_HOME");
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", root.join("xdg"));
    let output = cli_non_tty(&["tsk", "setup"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_xdg {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    assert_eq!(output.code, 0);
    for token in [
        "herdr",
        "claude",
        "pi",
        "omp",
        "codex",
        "cursor",
        "grok",
        "opencode",
        "--skill-dir",
    ] {
        assert!(
            output.stdout.contains(token),
            "bare setup should list {token}, got {:?}",
            output.stdout
        );
    }
    assert!(
        output.stdout.contains(&native_path(&[".grok", "skills"])),
        "bare setup should advertise the grok skills dir, got {:?}",
        output.stdout
    );
    assert!(
        output
            .stdout
            .contains(&native_path(&[".config", "opencode", "skills"])),
        "bare setup should advertise the opencode skills dir, got {:?}",
        output.stdout
    );
    assert!(
        output
            .stdout
            .contains(&native_path(&[".omp", "agent", "skills"])),
        "bare setup should advertise the omp skills dir, got {:?}",
        output.stdout
    );
    std::env::set_var("OMP_PROFILE", "research");
    let profiled = cli_non_tty(&["tsk", "setup"]);
    assert_eq!(profiled.code, 0, "{profiled:?}");
    assert!(
        profiled.stdout.contains(&native_path(&[
            ".omp", "profiles", "research", "agent", "skills", "tsk-cli", "SKILL.md"
        ])),
        "the listing should resolve the active OMP profile: {profiled:?}"
    );
    assert!(
        !home.join(".claude").exists(),
        "bare setup must not write agent skills"
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn userprofile_only_drives_windows_skill_install_and_listing() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("userprofile-only");
    let profile = root.join("profile");
    fs::create_dir_all(&profile).expect("profile");
    let previous_home = std::env::var_os("HOME");
    let previous_profile = std::env::var_os("USERPROFILE");
    std::env::remove_var("HOME");
    std::env::set_var("USERPROFILE", &profile);

    let listed = cli_non_tty(&["tsk", "setup"]);
    let installed = cli(&["tsk", "setup", "claude"]);

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("USERPROFILE", value),
        None => std::env::remove_var("USERPROFILE"),
    }

    let skill = profile.join(".claude/skills/tsk-cli/SKILL.md");
    assert_eq!(listed.code, 0, "{listed:?}");
    assert!(
        listed
            .stdout
            .replace('\\', "/")
            .contains(&skill.display().to_string().replace('\\', "/")),
        "{listed:?}"
    );
    assert_eq!(installed.code, 0, "{installed:?}");
    assert_eq!(
        fs::read_to_string(&skill).expect("installed skill"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn two_targets_or_herdr_plus_skill_dir_are_usage() {
    let _lock = env_lock();
    let root = temp_dir("usage");
    let skill_dir = root.join("skills");
    let two = cli(&["tsk", "setup", "claude", "pi"]);
    assert_eq!(two.code, 2);
    let mixed = cli(&[
        "tsk",
        "setup",
        "herdr",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
    ]);
    assert_eq!(mixed.code, 2);
    assert!(!skill_dir.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn json_emits_written_exists_or_listed() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("json");
    let skill_dir = root.join("skills");
    let path = skill_dir.to_str().expect("utf-8");
    let written = cli(&["tsk", "setup", "--skill-dir", path, "--json"]);
    assert_eq!(written.code, 0);
    let written_value: serde_json::Value =
        serde_json::from_str(written.stdout.trim()).expect("written json");
    assert_eq!(written_value["outcome"], "written");
    assert!(written_value["path"].as_str().is_some());
    assert!(written_value["target"].as_str().is_some());

    let exists = cli(&["tsk", "setup", "--skill-dir", path, "--json"]);
    assert_eq!(exists.code, 1);
    let exists_value: serde_json::Value =
        serde_json::from_str(exists.stdout.trim()).expect("exists json");
    assert_eq!(exists_value["outcome"], "exists");

    let detected = cli(&["tsk", "setup", "--json"]);
    assert_eq!(detected.code, 0);
    let detected_value: serde_json::Value =
        serde_json::from_str(detected.stdout.trim()).expect("detected json");
    assert_eq!(detected_value["outcome"], "detected");
    assert!(detected_value["embedded_skill_version"].as_str().is_some());
    assert!(detected_value["agents"].as_array().is_some());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn setup_herdr_help_uses_the_shared_reference() {
    let help = cli(&["tsk", "setup", "herdr", "--help"]);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("Values"));
    assert!(help.stdout.contains("herdr"));
    assert!(help.stdout.contains("Exit:"));
    let bad = cli(&["tsk", "setup", "herdr", "--force"]);
    assert_eq!(bad.code, 2);
}

#[test]
fn empty_skill_dir_is_usage_and_writes_nothing() {
    let _lock = env_lock();
    let root = temp_dir("empty-dir");
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("chdir temp");
    let space = cli(&["tsk", "setup", "--skill-dir", ""]);
    let equals = cli(&["tsk", "setup", "--skill-dir="]);
    std::env::set_current_dir(&previous).expect("restore cwd");
    assert_eq!(space.code, 2, "{space:?}");
    assert_eq!(equals.code, 2, "{equals:?}");
    assert!(
        !root.join("tsk-cli").exists(),
        "empty --skill-dir must not write into cwd"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn named_agent_targets_write_under_home() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("named");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home");
    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);
    let cases = [
        ("claude", ".claude/skills"),
        ("pi", ".pi/agent/skills"),
        ("omp", ".omp/agent/skills"),
        ("cursor", ".cursor/skills"),
        ("codex", ".agents/skills"),
        ("grok", ".grok/skills"),
        ("opencode", ".config/opencode/skills"),
    ];
    for (name, suffix) in cases {
        let dest = home.join(suffix).join("tsk-cli/SKILL.md");
        let output = cli(&["tsk", "setup", name, "--force"]);
        assert_eq!(output.code, 0, "{name}: {output:?}");
        assert_eq!(
            fs::read_to_string(&dest).expect("written skill"),
            skill_source(),
            "{name} path"
        );
    }
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn omp_target_resolves_profiles_and_directory_overrides() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    // Keep the duplicated absolute-path case below clear of legacy MAX_PATH limits.
    let root = temp_dir("omp");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home");
    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);

    let default = cli(&["tsk", "setup", "omp"]);
    assert_eq!(default.code, 0, "{default:?}");
    let default_skill = home.join(".omp/agent/skills/tsk-cli/SKILL.md");
    assert_eq!(
        fs::read_to_string(&default_skill).expect("default skill"),
        skill_source()
    );
    let current = cli(&["tsk", "setup", "omp"]);
    assert_eq!(
        current.code, 1,
        "matching version stays current: {current:?}"
    );
    fs::write(&default_skill, "stale without frontmatter\n").expect("stale skill");
    let updated = cli(&["tsk", "setup", "omp"]);
    assert_eq!(updated.code, 0, "outdated skill updates: {updated:?}");
    assert_eq!(
        fs::read_to_string(&default_skill).expect("updated skill"),
        skill_source()
    );

    std::env::set_var("OMP_PROFILE", "  research  ");
    let named = cli(&["tsk", "setup", "omp"]);
    assert_eq!(named.code, 0, "named profile: {named:?}");
    assert!(home
        .join(".omp/profiles/research/agent/skills/tsk-cli/SKILL.md")
        .exists());

    std::env::set_var("OMP_PROFILE", "");
    std::env::set_var("PI_PROFILE", "legacy-ignored");
    let empty_wins = cli(&["tsk", "setup", "omp", "--force"]);
    assert_eq!(empty_wins.code, 0, "empty OMP profile wins: {empty_wins:?}");
    assert!(!home.join(".omp/profiles/legacy-ignored").exists());
    std::env::set_var("OMP_PROFILE", "   ");
    let whitespace_wins = cli(&["tsk", "setup", "omp", "--force"]);
    assert_eq!(
        whitespace_wins.code, 0,
        "whitespace OMP profile wins: {whitespace_wins:?}"
    );
    assert!(!home.join(".omp/profiles/legacy-ignored").exists());
    std::env::set_var(
        "PI_CODING_AGENT_DIR",
        home.join(".omp/profiles/legacy-ignored/agent"),
    );
    let inherited_override = cli(&["tsk", "setup", "omp", "--force"]);
    assert_eq!(
        inherited_override.code, 0,
        "profile-derived override is suppressed: {inherited_override:?}"
    );
    assert!(
        !home
            .join(".omp/profiles/legacy-ignored/agent/skills/tsk-cli/SKILL.md")
            .exists(),
        "an inherited profile agent dir must not move the explicit default profile"
    );

    std::env::remove_var("OMP_PROFILE");
    let legacy = cli(&["tsk", "setup", "omp"]);
    assert_eq!(legacy.code, 0, "legacy PI profile: {legacy:?}");
    assert!(home
        .join(".omp/profiles/legacy-ignored/agent/skills/tsk-cli/SKILL.md")
        .exists());

    std::env::set_var("OMP_PROFILE", "custom");
    std::env::set_var("PI_CONFIG_DIR", ".config/omp-test");
    std::env::set_var("PI_CODING_AGENT_DIR", root.join("ignored-agent-dir"));
    let configured_named = cli(&["tsk", "setup", "omp"]);
    assert_eq!(
        configured_named.code, 0,
        "configured profile: {configured_named:?}"
    );
    assert!(
        home.join(".config/omp-test/profiles/custom/agent/skills/tsk-cli/SKILL.md")
            .exists(),
        "named profiles use PI_CONFIG_DIR"
    );
    assert!(
        !root
            .join("ignored-agent-dir/skills/tsk-cli/SKILL.md")
            .exists(),
        "named profiles ignore PI_CODING_AGENT_DIR"
    );

    std::env::remove_var("PI_CODING_AGENT_DIR");
    let absolute_config = root.join("absolute-config");
    std::env::set_var("PI_CONFIG_DIR", &absolute_config);
    std::env::set_var("OMP_PROFILE", "absolute");
    let absolute_config_output = cli(&["tsk", "setup", "omp"]);
    assert_eq!(
        absolute_config_output.code, 0,
        "absolute-looking config dir: {absolute_config_output:?}"
    );
    let home_relative_config = absolute_config
        .components()
        .filter_map(|component| match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => None,
            std::path::Component::CurDir => None,
            std::path::Component::ParentDir => Some("..".into()),
            std::path::Component::Normal(part) => Some(part.to_owned()),
        })
        .collect::<std::path::PathBuf>();
    assert!(
        home.join(&home_relative_config)
            .join("profiles")
            .join("absolute")
            .join("agent")
            .join("skills")
            .join("tsk-cli")
            .join("SKILL.md")
            .exists(),
        "PI_CONFIG_DIR follows OMP's home-relative path.join semantics"
    );
    assert!(
        !absolute_config
            .join("profiles/absolute/agent/skills/tsk-cli/SKILL.md")
            .exists(),
        "an absolute PI_CONFIG_DIR must not escape HOME"
    );

    std::env::set_var("PI_CONFIG_DIR", ".default-omp-test");
    std::env::set_var("OMP_PROFILE", "default");
    let configured_default_root = cli(&["tsk", "setup", "omp"]);
    assert_eq!(
        configured_default_root.code, 0,
        "configured default root: {configured_default_root:?}"
    );
    assert!(
        home.join(".default-omp-test/agent/skills/tsk-cli/SKILL.md")
            .exists(),
        "the default profile honors PI_CONFIG_DIR without an agent override"
    );

    std::env::set_var("PI_CONFIG_DIR", ".config/omp-test");
    std::env::set_var("PI_CODING_AGENT_DIR", root.join("ignored-agent-dir"));
    std::env::set_var("OMP_PROFILE", "default");
    let configured_default = cli(&["tsk", "setup", "omp"]);
    assert_eq!(
        configured_default.code, 0,
        "configured default: {configured_default:?}"
    );
    assert!(
        root.join("ignored-agent-dir/skills/tsk-cli/SKILL.md")
            .exists(),
        "the default profile honors PI_CODING_AGENT_DIR"
    );

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn omp_relative_agent_override_is_lexically_normalized() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("omp-relative-agent-dir");
    let home = root.join("home");
    let work = root.join("work");
    let outside_nested = root.join("outside/nested");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&work).expect("work");
    fs::create_dir_all(&outside_nested).expect("outside");
    std::os::unix::fs::symlink(&outside_nested, work.join("link")).expect("directory symlink");
    let previous_home = std::env::var_os("HOME");
    let previous_cwd = std::env::current_dir().expect("cwd");
    std::env::set_var("HOME", &home);
    std::env::set_var("PI_CODING_AGENT_DIR", "link/..");
    std::env::set_current_dir(&work).expect("enter work dir");

    let output = cli(&["tsk", "setup", "omp"]);
    std::env::set_current_dir(previous_cwd).expect("restore cwd");
    assert_eq!(output.code, 0, "{output:?}");
    assert!(
        work.join("skills/tsk-cli/SKILL.md").exists(),
        "path.resolve normalizes the override before filesystem traversal"
    );
    assert!(
        !root.join("outside/skills/tsk-cli/SKILL.md").exists(),
        "the symlink must not redirect a parent traversal"
    );

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn omp_target_rejects_invalid_profiles_without_writing() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("omp-invalid-profile");
    let home = root.join("home");
    fs::create_dir_all(&home).expect("home");
    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);

    let too_long = "a".repeat(65);
    for profile in ["../escape", "Upper", "name.", "con", too_long.as_str()] {
        std::env::set_var("OMP_PROFILE", profile);
        let output = cli(&["tsk", "setup", "omp"]);
        assert_eq!(output.code, 1, "profile {profile:?}: {output:?}");
        assert!(
            output.stderr.contains("invalid OMP profile"),
            "profile {profile:?}: {output:?}"
        );
    }
    assert!(
        !root.join("escape/agent/skills/tsk-cli/SKILL.md").exists(),
        "a profile cannot escape the OMP config root"
    );

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_omp_profile_does_not_block_other_agent_detection() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("omp-invalid-detection");
    let home = root.join("home");
    let empty_bin = root.join("empty-bin");
    fs::create_dir_all(home.join(".claude")).expect("Claude marker");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", &empty_bin);
    std::env::set_var("OMP_PROFILE", "Invalid/Profile");

    let detected = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(detected.code, 0, "{detected:?}");
    assert_eq!(detected.stdout.trim(), "claude");
    let installed = cli_non_tty(&["tsk", "setup", "agents", "--yes"]);
    assert_eq!(installed.code, 0, "{installed:?}");
    assert!(home.join(".claude/skills/tsk-cli/SKILL.md").exists());
    let direct = cli_non_tty(&["tsk", "setup", "omp"]);
    assert_eq!(
        direct.code, 1,
        "explicit OMP setup still refuses: {direct:?}"
    );
    assert!(direct.stderr.contains("invalid OMP profile"), "{direct:?}");

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn omp_detection_uses_configured_and_overridden_roots() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("omp-detection-roots");
    let home = root.join("home");
    let empty_bin = root.join("empty-bin");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", &empty_bin);

    std::env::set_var("PI_CONFIG_DIR", ".custom-omp");
    fs::create_dir_all(home.join(".custom-omp")).expect("custom config root");
    let configured = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(configured.code, 0, "{configured:?}");
    assert_eq!(configured.stdout.trim(), "omp");

    fs::remove_dir_all(home.join(".custom-omp")).expect("remove config root");
    std::env::remove_var("PI_CONFIG_DIR");
    let agent_dir = root.join("active-agent");
    fs::create_dir_all(&agent_dir).expect("active agent dir");
    std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir);
    let overridden = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(overridden.code, 0, "{overridden:?}");
    assert_eq!(overridden.stdout.trim(), "omp");

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn omp_detection_and_batch_install_use_the_active_profile() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("omp-detection");
    let home = root.join("home");
    let empty_bin = root.join("empty-bin");
    fs::create_dir_all(home.join(".omp")).expect("OMP config root");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", &empty_bin);
    std::env::set_var("OMP_PROFILE", "research");

    let detected = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(detected.code, 0, "{detected:?}");
    assert_eq!(detected.stdout.trim(), "omp");
    let installed = cli_non_tty(&["tsk", "setup", "agents", "--yes"]);
    assert_eq!(installed.code, 0, "{installed:?}");
    let skill = home.join(".omp/profiles/research/agent/skills/tsk-cli/SKILL.md");
    assert_eq!(
        fs::read_to_string(&skill).expect("batch skill"),
        skill_source()
    );

    let report = cli_non_tty(&["tsk", "setup", "--json"]);
    assert_eq!(report.code, 0, "{report:?}");
    let report: serde_json::Value =
        serde_json::from_str(report.stdout.trim()).expect("detection report");
    let omp = report["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["id"] == "omp")
        .expect("OMP row");
    assert_eq!(omp["state"], "current");
    assert_eq!(
        omp["skill_path"],
        skill.display().to_string(),
        "detection reports the profile-scoped destination"
    );

    fs::remove_dir_all(home.join(".omp")).expect("remove config marker");
    std::env::remove_var("OMP_PROFILE");
    let omp_bin = empty_bin.join("omp");
    fs::write(&omp_bin, "#!/bin/sh\n").expect("stub OMP binary");
    let non_executable = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(non_executable.code, 0, "{non_executable:?}");
    assert!(
        non_executable.stdout.trim().is_empty(),
        "a non-executable file on PATH is not an installed agent: {non_executable:?}"
    );
    fs::remove_file(&omp_bin).expect("remove non-executable stub");
    let real_bin = root.join("real-bin");
    fs::create_dir_all(&real_bin).expect("real bin");
    let real_omp = real_bin.join("omp-real");
    fs::write(&real_omp, "#!/bin/sh\n").expect("real OMP binary");
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&real_omp).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&real_omp, permissions).expect("chmod");
    std::os::unix::fs::symlink(&real_omp, &omp_bin).expect("OMP PATH symlink");
    let path_detected = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(path_detected.code, 0, "{path_detected:?}");
    assert_eq!(path_detected.stdout.trim(), "omp");

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn force_refuses_a_symlinked_skill_file_and_directory() {
    let _lock = env_lock();
    let root = temp_dir("symlink");
    let skill_dir = root.join("skills");
    let outside = root.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    let planted = outside.join("SKILL.md");
    fs::write(&planted, "do-not-clobber\n").expect("plant");

    fs::create_dir_all(skill_dir.join("tsk-cli")).expect("skill folder");
    std::os::unix::fs::symlink(&planted, skill_dir.join("tsk-cli/SKILL.md")).expect("link file");
    let file = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
        "--force",
    ]);
    assert_eq!(file.code, 1, "{file:?}");
    assert!(
        file.stderr.contains("refusing symlink"),
        "stderr should refuse the file symlink, got {:?}",
        file.stderr
    );
    assert_eq!(
        fs::read_to_string(&planted).expect("untouched file"),
        "do-not-clobber\n"
    );

    let _ = fs::remove_dir_all(skill_dir.join("tsk-cli"));
    std::os::unix::fs::symlink(&outside, skill_dir.join("tsk-cli")).expect("link dir");
    let dir = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skill_dir.to_str().expect("utf-8"),
        "--force",
    ]);
    assert_eq!(dir.code, 1, "{dir:?}");
    assert!(
        dir.stderr.contains("refusing symlink"),
        "stderr should refuse the directory symlink, got {:?}",
        dir.stderr
    );
    assert_eq!(
        fs::read_to_string(&planted).expect("untouched dir target"),
        "do-not-clobber\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn detected_ids_prints_space_separated_agents() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("ids");
    let home = root.join("home");
    fs::create_dir_all(home.join(".cursor")).expect("cursor");
    fs::create_dir_all(home.join(".claude")).expect("claude");
    fs::create_dir_all(home.join(".config/opencode")).expect("opencode");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let output = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 0, "{output:?}");
    let ids: Vec<&str> = output.stdout.split_whitespace().collect();
    assert!(ids.contains(&"cursor"), "{ids:?}");
    assert!(ids.contains(&"claude"), "{ids:?}");
    assert!(ids.contains(&"opencode"), "{ids:?}");
    assert!(!ids.contains(&"grok"), "{ids:?}");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn opencode_detects_via_path_binary() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("opencode-path");
    let home = root.join("home");
    let bin = root.join("bin");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&bin).expect("bin");
    fs::write(bin.join("opencode"), "#!/bin/sh\n").expect("stub");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", &bin);
    let non_executable = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    assert_eq!(non_executable.code, 0, "{non_executable:?}");
    assert_eq!(
        non_executable.stdout.trim(),
        "opencode",
        "existing agents keep regular-file PATH detection"
    );
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(bin.join("opencode"))
        .expect("meta")
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(bin.join("opencode"), perms).expect("chmod");
    let output = cli_non_tty(&["tsk", "setup", "--detected-ids"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 0, "{output:?}");
    let ids: Vec<&str> = output.stdout.split_whitespace().collect();
    assert_eq!(ids, vec!["opencode"], "{ids:?}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn agents_yes_installs_without_asking() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("agents-yes");
    let home = root.join("home");
    fs::create_dir_all(home.join(".claude")).expect("claude");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let output = cli_non_tty(&["tsk", "setup", "agents", "--yes"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 0, "{output:?}");
    assert_eq!(
        fs::read_to_string(home.join(".claude/skills/tsk-cli/SKILL.md")).expect("written"),
        skill_source()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn agents_yes_force_rewrites_a_current_skill() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("agents-yes-force");
    let home = root.join("home");
    let skill = home.join(".claude/skills/tsk-cli/SKILL.md");
    let cursor_skill = home.join(".cursor/skills/tsk-cli/SKILL.md");
    for path in [&skill, &cursor_skill] {
        fs::create_dir_all(path.parent().expect("skill dir")).expect("skills dir");
    }
    // Same frontmatter version as the embedded skill, different body: the version check
    // alone would call these current and skip them.
    let stale = skill_source().replacen("# tsk", "# stale body", 1);
    assert_ne!(stale, skill_source());
    fs::write(&skill, &stale).expect("seed stale claude skill");
    fs::write(&cursor_skill, &stale).expect("seed stale cursor skill");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let skipped = cli_non_tty(&["tsk", "setup", "agents", "--yes", "--json"]);
    let forced = cli_non_tty(&["tsk", "setup", "agents", "--yes", "--force", "--json"]);
    let interactive_force = cli_non_tty(&["tsk", "setup", "agents", "--force"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(skipped.code, 0, "{skipped:?}");
    assert!(
        skipped
            .stdout
            .contains("\"skipped_current\":[\"claude\",\"cursor\"]"),
        "matching versions are skipped without --force: {}",
        skipped.stdout
    );
    assert_eq!(forced.code, 0, "{forced:?}");
    for id in ["claude", "cursor"] {
        assert!(
            forced
                .stdout
                .contains(&format!("\"id\":\"{id}\",\"kind\":\"updated\"")),
            "--force rewrites every matching version: {}",
            forced.stdout
        );
    }
    assert!(
        forced.stdout.contains("\"skipped_current\":[]"),
        "{}",
        forced.stdout
    );
    for path in [&skill, &cursor_skill] {
        assert_eq!(fs::read_to_string(path).expect("rewritten"), skill_source());
    }
    assert_eq!(
        interactive_force.code, 2,
        "agents --force without --yes is a usage error: {interactive_force:?}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn agents_yes_json_is_machine_readable() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("agents-yes-json");
    let home = root.join("home");
    fs::create_dir_all(home.join(".claude")).expect("claude");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let output = cli_non_tty(&["tsk", "setup", "agents", "--yes", "--json"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 0, "{output:?}");
    let payload: serde_json::Value =
        serde_json::from_str(output.stdout.trim()).expect("batch json");
    assert_eq!(payload["outcome"], "batch");
    assert_eq!(payload["applied"][0]["id"], "claude");
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
#[cfg(unix)]
fn agents_yes_reports_blocked_skill_roots() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("agents-blocked");
    let home = root.join("home");
    let outside = root.join("outside");
    fs::create_dir_all(home.join(".claude")).expect("claude");
    fs::create_dir_all(&outside).expect("outside");
    std::os::unix::fs::symlink(&outside, home.join(".claude/skills")).expect("skill link");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let output = cli_non_tty(&["tsk", "setup", "agents", "--yes", "--json"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 1, "{output:?}");
    assert!(output.stderr.contains("blocked agent skill roots: claude"));
    let payload: serde_json::Value =
        serde_json::from_str(output.stdout.trim()).expect("batch json");
    assert_eq!(payload["blocked"], serde_json::json!(["claude"]));
    assert!(!outside.join("tsk-cli/SKILL.md").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bare_setup_non_tty_with_detected_agents_prints_guidance_without_writing() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("bare-nontty");
    let home = root.join("home");
    fs::create_dir_all(home.join(".cursor")).expect("cursor");
    let previous_home = std::env::var_os("HOME");
    let previous_path = std::env::var_os("PATH");
    std::env::set_var("HOME", &home);
    std::env::set_var("PATH", root.join("empty-bin"));
    fs::create_dir_all(root.join("empty-bin")).expect("empty bin");
    let output = cli_non_tty(&["tsk", "setup"]);
    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match previous_path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
    assert_eq!(output.code, 0, "{output:?}");
    assert!(
        !home.join(".cursor/skills/tsk-cli/SKILL.md").exists(),
        "non-TTY bare setup must not write skills"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(unix)]
fn skill_states_probe_lists_each_detected_agent_with_state_version_and_path() {
    let _lock = env_lock();
    let _omp_env = OmpEnvGuard::cleared();
    let root = temp_dir("skill-states");
    let home = root.join("home");
    fs::create_dir_all(home.join(".claude")).expect("Claude marker");
    fs::create_dir_all(home.join(".codex")).expect("Codex marker");
    fs::create_dir_all(home.join(".agents/skills/tsk-cli")).expect("Codex skill folder");
    fs::write(
        home.join(".agents/skills/tsk-cli/SKILL.md"),
        "---\nname: tsk-cli\nversion: 0.0.1\n---\nold\n",
    )
    .expect("stale codex skill");
    fs::create_dir_all(home.join(".cursor/skills/tsk-cli")).expect("Cursor skill folder");
    fs::write(
        home.join(".cursor/skills/tsk-cli/SKILL.md"),
        format!(
            "---\nname: tsk-cli\nversion: {}\n---\ncurrent\n",
            tsk_tui::setup_agent::embedded_skill_version()
        ),
    )
    .expect("current cursor skill");
    fs::create_dir_all(home.join(".grok/skills")).expect("Grok skills root");
    fs::create_dir_all(root.join("elsewhere\ttab")).expect("symlink target with a tab");
    std::os::unix::fs::symlink(
        root.join("elsewhere\ttab"),
        home.join(".grok/skills/tsk-cli"),
    )
    .expect("blocked grok skill folder");
    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);

    let probe = cli_non_tty(&["tsk", "setup", "--skill-states"]);
    assert_eq!(probe.code, 0, "{probe:?}");
    let lines: Vec<Vec<&str>> = probe
        .stdout
        .lines()
        .map(|line| line.split('\t').collect())
        .collect();
    assert_eq!(lines[0][0], "embedded");
    assert_eq!(lines[0][1], tsk_tui::setup_agent::embedded_skill_version());
    let claude = lines.iter().find(|l| l[0] == "claude").expect("claude row");
    assert_eq!(claude[1], "missing");
    assert_eq!(claude[2], "-");
    assert!(
        claude[3].ends_with(".claude/skills/tsk-cli/SKILL.md"),
        "{claude:?}"
    );
    let codex = lines.iter().find(|l| l[0] == "codex").expect("codex row");
    assert_eq!(codex[1], "outdated");
    assert_eq!(codex[2], "0.0.1");
    let cursor = lines.iter().find(|l| l[0] == "cursor").expect("cursor row");
    assert_eq!(cursor[1], "current");
    assert_eq!(cursor[2], tsk_tui::setup_agent::embedded_skill_version());
    let grok = lines.iter().find(|l| l[0] == "grok").expect("grok row");
    assert_eq!(grok[1], "blocked-symlink");
    assert_eq!(
        grok.len(),
        4,
        "a control character in a path must not add a field"
    );
    assert!(
        lines.iter().all(|l| l.len() == 2 || l.len() == 4),
        "{lines:?}"
    );

    let both = cli_non_tty(&["tsk", "setup", "--skill-states", "--detected-ids"]);
    assert_eq!(both.code, 2, "{both:?}");
    let stray_check = cli_non_tty(&["tsk", "setup", "--check"]);
    assert_eq!(stray_check.code, 2, "{stray_check:?}");

    match previous_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn herdr_check_reports_bound_only_when_both_commands_are_in_the_config() {
    let _lock = env_lock();
    let root = temp_dir("herdr-check");
    let config = root.join("herdr/config.toml");
    fs::create_dir_all(config.parent().unwrap()).expect("config dir");
    let previous = std::env::var_os("HERDR_CONFIG_PATH");
    std::env::set_var("HERDR_CONFIG_PATH", &config);

    let missing = cli_non_tty(&["tsk", "setup", "herdr", "--check"]);
    assert_eq!(
        (missing.code, missing.stdout.trim()),
        (0, "unbound"),
        "{missing:?}"
    );

    fs::write(
        &config,
        format!(
            "[[keys.command]]\nkey = 'prefix+b'\ntype = 'plugin_action'\ncommand = '{}'\n[[keys.command]]\nkey = 'prefix+a'\ntype = 'plugin_action'\ncommand = '{}'\n",
            OPEN_BOARD_ACTION, QUICK_CAPTURE_ACTION
        ),
    )
    .expect("write config");
    let bound = cli_non_tty(&["tsk", "setup", "herdr", "--check"]);
    assert_eq!((bound.code, bound.stdout.trim()), (0, "bound"), "{bound:?}");

    fs::write(&config, "[keys]\nprefix = 'ctrl+b'\n").expect("write config");
    let unbound = cli_non_tty(&["tsk", "setup", "herdr", "--check"]);
    assert_eq!(
        (unbound.code, unbound.stdout.trim()),
        (0, "unbound"),
        "{unbound:?}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = root.join("outside.toml");
        fs::write(
            &outside,
            format!(
                "[[keys.command]]\nkey = 'prefix+b'\ntype = 'plugin_action'\ncommand = '{}'\n[[keys.command]]\nkey = 'prefix+a'\ntype = 'plugin_action'\ncommand = '{}'\n",
                OPEN_BOARD_ACTION, QUICK_CAPTURE_ACTION
            ),
        )
        .expect("outside config");
        fs::remove_file(&config).expect("remove config");
        symlink(&outside, &config).expect("linked config");
        let linked = cli_non_tty(&["tsk", "setup", "herdr", "--check"]);
        assert_ne!(
            linked.code, 0,
            "a linked final config must be refused: {linked:?}"
        );
        assert!(linked.stdout.trim().is_empty(), "{linked:?}");
    }

    match previous {
        Some(value) => std::env::set_var("HERDR_CONFIG_PATH", value),
        None => std::env::remove_var("HERDR_CONFIG_PATH"),
    }
    let _ = fs::remove_dir_all(root);
}

/// The skill is replaced by staging beside it and renaming: an update leaves no staging
/// file behind, a stale staging file from a killed run is cleared, and a symlink planted at
/// the staging name is refused rather than written through.
#[test]
#[cfg(unix)]
fn skill_updates_stage_and_rename_without_following_a_planted_symlink() {
    let _lock = env_lock();
    let root = temp_dir("stage-replace");
    let skills = root.join("skills");
    let folder = skills.join("tsk-cli");
    fs::create_dir_all(&folder).expect("skill folder");
    fs::write(
        folder.join("SKILL.md"),
        "---\nname: tsk-cli\nversion: 0.0.1\n---\nold\n",
    )
    .expect("stale skill");
    // A leftover from an interrupted earlier run, at the exact name this pid would pick.
    let stale = folder.join(format!(".SKILL.md.tmp.{}", std::process::id()));
    fs::write(&stale, "half-written").expect("stale staging file");

    let updated = cli(&["tsk", "setup", "--skill-dir", skills.to_str().unwrap()]);
    assert_eq!(updated.code, 0, "{updated:?}");
    let installed = fs::read_to_string(folder.join("SKILL.md")).expect("installed skill");
    assert!(
        installed.contains(&format!(
            "version: {}",
            tsk_tui::setup_agent::embedded_skill_version()
        )),
        "{installed}"
    );
    let leftovers: Vec<_> = fs::read_dir(&folder)
        .expect("skill folder")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // Now plant a symlink at the next staging name: the write must refuse, and the live
    // skill and the symlink's target must both be untouched.
    let target = root.join("elsewhere");
    fs::write(&target, "do not touch").expect("symlink target");
    let planted = folder.join(format!(".SKILL.md.tmp.{}", std::process::id()));
    std::os::unix::fs::symlink(&target, &planted).expect("planted symlink");
    let before = fs::read_to_string(folder.join("SKILL.md")).expect("live skill");
    let refused = cli(&[
        "tsk",
        "setup",
        "--skill-dir",
        skills.to_str().unwrap(),
        "--force",
    ]);
    assert_ne!(refused.code, 0, "{refused:?}");
    assert!(refused.stderr.contains("symlink"), "{refused:?}");
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "do not touch",
        "the write went through the planted symlink"
    );
    assert_eq!(fs::read_to_string(folder.join("SKILL.md")).unwrap(), before);
    assert!(
        planted.symlink_metadata().is_ok(),
        "the planted link is not ours to delete"
    );
    let _ = fs::remove_dir_all(root);
}
