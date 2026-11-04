//! `tsk setup rex`: the board and quick-capture actions and keys in the Rex config.
//!
//! tsk owns one file, `tsk.lua`, beside the Rex `init.lua`, and adds one `require("tsk")`
//! line to the end of `init.lua` so its bindings stack over the user's. Every decision
//! (version, conflicts, key notation) is made before the first write; the merged config is
//! checked with `rex config check` after the write and both files are restored if Rex
//! rejects it. The keys in use are recorded in the managed file's header, so a rerun without
//! key flags (`tsk update`) keeps them.
use super::{backup_name_for, confirm, error, installed_binary, resolve_config, Dir, TempFile};
use std::{
    env,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub const BOARD_ACTION: &str = "tsk_board";
pub const CAPTURE_ACTION: &str = "tsk_capture";
pub const DEFAULT_BOARD_KEY: &str = "alt+t";
pub const DEFAULT_CAPTURE_KEY: &str = "alt+a";
/// The lowest Rex server API revision (`rex version`) the managed actions were built and
/// tested against: `rex.action`, `rex.session.new_layer`, and terminal block calls.
pub const MIN_REX_API_REVISION: u64 = 10;

const MANAGED: &str = "tsk.lua";
const MARKER: &str = "-- Managed by tsk setup rex.";
const REQUIRE_LINE: &str = "require(\"tsk\") -- tsk setup rex: board and quick-capture keys";
const BOARD_KEY_HEADER: &str = "-- board-key: ";
const CAPTURE_KEY_HEADER: &str = "-- capture-key: ";
const NONE: &str = "none";

/// Output of one `rex` call. `config check` reports on stdout and exits 1 on errors, so the
/// caller needs stdout whatever the status.
pub struct RexOutput {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Options {
    pub board_key: Option<String>,
    pub capture_key: Option<String>,
}

pub struct RexSetupResult {
    pub config: PathBuf,
    pub backup: Option<PathBuf>,
    pub declined_conflicts: bool,
    /// The keys each tsk action is on after the reload, `(keys, label)`: only keys where it
    /// is the binding Rex tries first.
    pub shortcuts: Vec<(String, &'static str)>,
    /// Keys setup bound that a later binding of the user's still takes, `(key, label, source)`.
    pub shadowed: Vec<(String, &'static str, String)>,
}

/// `REX_CONFIG`, else `rex/init.lua` under `XDG_CONFIG_HOME` or `~/.config`.
fn config_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("REX_CONFIG").filter(|v| !v.is_empty()) {
        return super::absolute(path.into());
    }
    let root = env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .ok_or_else(|| error("HOME or XDG_CONFIG_HOME is required"))?;
    super::absolute(root.join("rex").join("init.lua"))
}

fn rex(args: &[&str]) -> io::Result<RexOutput> {
    let output = Command::new("rex")
        .arg("--autostart=false")
        .args(args)
        .output()
        .map_err(|e| error(format!("could not run rex: {e}")))?;
    Ok(RexOutput {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn quoted(output: &RexOutput) -> String {
    format!("{}{}", output.stdout, output.stderr)
        .lines()
        .map(crate::ui::terminal_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn rex_ok(
    host: &mut impl FnMut(&[&str]) -> io::Result<RexOutput>,
    args: &[&str],
) -> io::Result<String> {
    let output = host(args)?;
    if !output.ok {
        return Err(error(format!(
            "rex {} failed: {}",
            args.join(" "),
            quoted(&output)
        )));
    }
    Ok(output.stdout)
}

/// `rex version` prints `api: revision N (supports 0 to N)`.
pub fn require_min_rex(version_output: &str) -> io::Result<()> {
    let revision = version_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("api: revision "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| {
            error(format!(
                "could not read the Rex API revision from `rex version` ({})",
                version_output.trim()
            ))
        })?;
    if revision < MIN_REX_API_REVISION {
        return Err(error(format!(
            "Rex API revision {revision} found; tsk needs revision {MIN_REX_API_REVISION} or newer. Update Rex, then run tsk setup rex again"
        )));
    }
    Ok(())
}

/// A key in Rex notation (`alt+t`, `ctrl+b>t`) as its sequence of chords, or `None` for
/// `none`. Rex validates the chord names themselves when the managed file is checked.
fn parse_key(raw: &str, flag: &str) -> io::Result<Option<Vec<String>>> {
    let key = raw.trim();
    if key.eq_ignore_ascii_case(NONE) {
        return Ok(None);
    }
    let bad = key.is_empty()
        || key
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '\\'));
    let chords: Vec<String> = key.split('>').map(|c| c.to_ascii_lowercase()).collect();
    if bad || chords.iter().any(String::is_empty) {
        return Err(error(format!(
            "{flag} {}: expected a Rex key such as alt+t or ctrl+b>t, or none",
            crate::ui::terminal_text(raw)
        )));
    }
    Ok(Some(chords))
}

fn key_text(chords: &[String]) -> String {
    chords.join(">")
}

/// The keys a managed file records in its header: `(board, capture)`, each `None` when
/// the header says `none` or is missing.
fn recorded_keys(managed: &str) -> (Option<String>, Option<String>) {
    let find = |prefix: &str| {
        managed
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string)
    };
    (find(BOARD_KEY_HEADER), find(CAPTURE_KEY_HEADER))
}

fn is_managed(text: &str) -> bool {
    text.starts_with(MARKER)
}

/// Whether a live (uncommented) line of `init.lua` requires the `tsk` module.
pub fn requires_tsk(init: &str) -> bool {
    init.lines().any(|line| {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("require") else {
            return false;
        };
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('(').unwrap_or(rest).trim_start();
        rest.starts_with("\"tsk\"") || rest.starts_with("'tsk'")
    })
}

/// A Lua string literal for `text`: quotes, backslashes, and control bytes escaped.
fn lua_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Binding {
    keys: Vec<String>,
    action: String,
    source: String,
}

struct Keymap {
    bindings: Vec<Binding>,
    config: Option<PathBuf>,
}

fn parse_keymap(text: &str) -> io::Result<Keymap> {
    let json: serde_json::Value =
        serde_json::from_str(text).map_err(|e| error(format!("invalid rex keymap: {e}")))?;
    let bindings = json["bindings"]
        .as_array()
        .ok_or_else(|| error("rex keymap is missing bindings"))?
        .iter()
        .map(|b| Binding {
            keys: b["keys"]
                .as_array()
                .map(|keys| {
                    keys.iter()
                        .filter_map(|k| k.as_str().map(str::to_ascii_lowercase))
                        .collect()
                })
                .unwrap_or_default(),
            action: b["action"].as_str().unwrap_or_default().to_string(),
            source: b["source"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    let config = json
        .pointer("/config/path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    Ok(Keymap { bindings, config })
}

fn ours(binding: &Binding) -> bool {
    binding.source.starts_with("tsk.lua:")
        || binding.action == BOARD_ACTION
        || binding.action == CAPTURE_ACTION
}

fn prefix_related(a: &[String], b: &[String]) -> bool {
    let n = a.len().min(b.len());
    n > 0 && a[..n] == b[..n]
}

/// Error diagnostics in `rex config check --json` output, ignoring those from `skip`.
fn check_errors(text: &str, skip: Option<&str>) -> io::Result<Vec<String>> {
    let json: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| error(format!("invalid rex config check output: {e}")))?;
    Ok(json["diagnostics"]
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|d| d["severity"].as_str() == Some("error"))
                .filter(|d| {
                    skip.is_none_or(|prefix| {
                        !d["source"].as_str().unwrap_or_default().starts_with(prefix)
                    })
                })
                .map(|d| {
                    crate::ui::terminal_text(&format!(
                        "{}: {}",
                        d["source"].as_str().unwrap_or("?"),
                        d["message"].as_str().unwrap_or("?")
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
}

fn config_check(
    host: &mut impl FnMut(&[&str]) -> io::Result<RexOutput>,
    path: &Path,
    skip: Option<&str>,
) -> io::Result<Vec<String>> {
    let path = path
        .to_str()
        .ok_or_else(|| error("Rex config path must be UTF-8"))?;
    let output = host(&["config", "check", "--json", path])?;
    if output.stdout.trim().is_empty() {
        return Err(error(format!(
            "rex config check failed: {}",
            quoted(&output)
        )));
    }
    check_errors(&output.stdout, skip)
}

/// The managed file: marker, recorded keys, the binary path, the actions, then the keys.
fn managed_file(
    binary: &str,
    board: Option<&[String]>,
    capture: Option<&[String]>,
    unbind: &[String],
) -> String {
    let record = |k: Option<&[String]>| k.map_or_else(|| NONE.to_string(), key_text);
    let mut out = format!(
        "{MARKER} Rerun `tsk setup rex` to update it; edits here are overwritten.\n\
         {BOARD_KEY_HEADER}{}\n\
         {CAPTURE_KEY_HEADER}{}\n\n\
         -- Absolute path: Rex's login shell may not have the install directory on PATH.\n\
         local TSK_BIN = {}\n\n",
        record(board),
        record(capture),
        lua_string(binary)
    );
    out.push_str(include_str!("rex.lua"));
    out.push('\n');
    for key in unbind {
        out.push_str(&format!("rex.unbind({})\n", lua_string(key)));
    }
    for (key, action) in [(board, BOARD_ACTION), (capture, CAPTURE_ACTION)] {
        if let Some(key) = key {
            out.push_str(&format!(
                "rex.bind({}, {})\n",
                lua_string(&key_text(key)),
                lua_string(action)
            ));
        }
    }
    out
}

/// `init.lua` with tsk's require line last, so its bindings stack over the user's. tsk's own
/// marked line moves to the end when the user added lines after it; a require the user
/// wrote themselves stays where it is.
fn with_require(init: Option<&str>) -> String {
    if let Some(text) = init {
        let last = text.lines().rev().find(|l| !l.trim().is_empty());
        if last != Some(REQUIRE_LINE) && text.lines().any(|l| l == REQUIRE_LINE) {
            let kept: Vec<&str> = text.lines().filter(|l| *l != REQUIRE_LINE).collect();
            return with_require(Some(&format!("{}\n", kept.join("\n").trim_end())));
        }
    }
    match init {
        Some(text) if requires_tsk(text) => text.to_string(),
        Some("") => format!("{REQUIRE_LINE}\n"),
        Some(text) if text.ends_with('\n') => format!("{text}\n{REQUIRE_LINE}\n"),
        Some(text) => format!("{text}\n\n{REQUIRE_LINE}\n"),
        None => format!(
            "-- Rex config. tsk setup rex added the line below; it loads tsk.lua beside this file.\n{REQUIRE_LINE}\n"
        ),
    }
}

/// `tsk setup rex --check`: the managed file is in place and `init.lua` loads it. Reads
/// files only; never calls Rex.
pub fn rex_setup_present() -> io::Result<bool> {
    if cfg!(windows) {
        return Ok(false);
    }
    let config = config_path()?;
    let (dir, init) = resolve_config(&config)?;
    let read = |path: &Path| match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    };
    let managed = read(&dir.join(MANAGED))?;
    let init = read(&init)?;
    Ok(managed.as_deref().is_some_and(is_managed) && init.as_deref().is_some_and(requires_tsk))
}

pub fn run(
    options: &Options,
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    interactive: bool,
) -> io::Result<RexSetupResult> {
    if cfg!(windows) {
        return Err(error(
            "Rex runs on macOS and Linux; tsk setup rex is not available on Windows",
        ));
    }
    let binary = installed_binary()?;
    let binary = binary
        .to_str()
        .ok_or_else(|| error("installed binary path must be UTF-8"))?
        .to_string();
    run_at(
        &config_path()?,
        &binary,
        options,
        reader,
        writer,
        interactive,
        &mut rex,
    )
}

/// Pick the key for one action. A flag wins; a rerun keeps the recorded key; a fresh setup
/// leaves an action the user already bound in their own config alone; otherwise the default.
fn wanted_key(
    flag: Option<&str>,
    flag_name: &str,
    recorded: Option<Option<String>>,
    action: &str,
    default: &str,
    keymap: &Keymap,
) -> io::Result<Option<Vec<String>>> {
    if let Some(flag) = flag {
        return parse_key(flag, flag_name);
    }
    if let Some(recorded) = recorded {
        return match recorded {
            Some(key) => parse_key(&key, flag_name),
            None => Ok(None),
        };
    }
    let user_bound = keymap
        .bindings
        .iter()
        .any(|b| b.action == action && !b.source.starts_with("tsk.lua:"));
    if user_bound {
        return Ok(None);
    }
    parse_key(default, flag_name)
}

pub(crate) fn run_at(
    config: &Path,
    binary: &str,
    options: &Options,
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    interactive: bool,
    host: &mut impl FnMut(&[&str]) -> io::Result<RexOutput>,
) -> io::Result<RexSetupResult> {
    // Key notation first: a typo is a usage mistake, not a Rex problem.
    for (flag, name) in [
        (&options.board_key, "--board-key"),
        (&options.capture_key, "--capture-key"),
    ] {
        if let Some(flag) = flag {
            parse_key(flag, name)?;
        }
    }
    require_min_rex(&rex_ok(host, &["version"])?)?;

    let (dir_path, init_path) = resolve_config(config)?;
    let init_dir_path = init_path
        .parent()
        .ok_or_else(|| error("Rex config has no parent directory"))?
        .to_path_buf();
    let init_name = PathBuf::from(
        init_path
            .file_name()
            .ok_or_else(|| error("Rex config has no filename"))?,
    );
    let managed_name = Path::new(MANAGED);
    let open = |path: &Path| match Dir::open(path, false) {
        Ok(dir) => Ok(Some(dir)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    };
    let existing_dir = open(&dir_path)?;
    let existing_init_dir = if init_dir_path == dir_path {
        None
    } else {
        open(&init_dir_path)?
    };
    let read_init = |dir: Option<&Dir>| -> io::Result<Option<String>> {
        match dir {
            Some(dir) => dir.read(&init_name),
            None => Ok(None),
        }
    };
    let before_init = if init_dir_path == dir_path {
        read_init(existing_dir.as_ref())?
    } else {
        read_init(existing_init_dir.as_ref())?
    };
    let before_managed = match &existing_dir {
        Some(dir) => dir.read(managed_name)?,
        None => None,
    };
    if let Some(text) = &before_managed {
        if !is_managed(text) {
            return Err(error(format!(
                "{} exists and was not written by tsk; rename it, then run tsk setup rex again (no changes made)",
                dir_path.join(MANAGED).display()
            )));
        }
    }
    // The user's config must load cleanly before tsk adds to it. Errors in the managed file
    // are tsk's own and are replaced below.
    if before_init.is_some() {
        let errors = config_check(host, &init_path, Some("tsk.lua:"))?;
        if !errors.is_empty() {
            return Err(error(format!(
                "the Rex config has errors; fix them, then run tsk setup rex again (no changes made):\n{}",
                errors.join("\n")
            )));
        }
    }

    let keymap = parse_keymap(&rex_ok(host, &["keymap", "--json"])?)?;
    if let Some(served) = &keymap.config {
        let same = |a: &Path, b: &Path| {
            a == b
                || matches!(
                    (std::fs::canonicalize(a), std::fs::canonicalize(b)),
                    (Ok(x), Ok(y)) if x == y
                )
        };
        if !same(served, config) && !same(served, &init_path) {
            return Err(error(format!(
                "the running Rex server reads {}, but tsk setup rex edits {}; set REX_CONFIG to the server's config, then run it again (no changes made)",
                served.display(),
                config.display()
            )));
        }
    }

    let recorded = before_managed.as_deref().map(recorded_keys);
    let board = wanted_key(
        options.board_key.as_deref(),
        "--board-key",
        recorded.as_ref().map(|r| r.0.clone()),
        BOARD_ACTION,
        DEFAULT_BOARD_KEY,
        &keymap,
    )?;
    let capture = wanted_key(
        options.capture_key.as_deref(),
        "--capture-key",
        recorded.as_ref().map(|r| r.1.clone()),
        CAPTURE_ACTION,
        DEFAULT_CAPTURE_KEY,
        &keymap,
    )?;
    if let (Some(b), Some(c)) = (&board, &capture) {
        if prefix_related(b, c) {
            return Err(error(format!(
                "the board key {} and the capture key {} overlap; pick two different keys (no changes made)",
                key_text(b),
                key_text(c)
            )));
        }
    }

    // Conflicts: a binding of the user's on the same key, or on a key that starts with it
    // or that it starts with. Never replaced without a yes.
    let mut declined = false;
    let mut unbind: Vec<String> = Vec::new();
    let mut resolve = |key: Option<Vec<String>>| -> io::Result<Option<Vec<String>>> {
        let Some(key) = key else { return Ok(None) };
        let clashes: Vec<&Binding> = keymap
            .bindings
            .iter()
            .filter(|b| !ours(b) && prefix_related(&b.keys, &key))
            .collect();
        if clashes.is_empty() {
            return Ok(Some(key));
        }
        let text = key_text(&key);
        let detail: String = clashes
            .iter()
            .map(|b| {
                format!(
                    "{} -> {} ({})\n",
                    key_text(&b.keys),
                    b.action,
                    if b.source.is_empty() { "?" } else { &b.source }
                )
            })
            .collect();
        if !interactive {
            return Err(error(format!(
                "{text} is already bound in the Rex config: {}; rerun tsk setup rex in an interactive terminal to replace it, or pick another key with --board-key or --capture-key (no changes made)",
                detail.trim_end().replace('\n', ", ")
            )));
        }
        if !confirm(reader, writer, &text, &detail)? {
            declined = true;
            return Ok(None);
        }
        // A shorter or equal key of the user's would swallow ours: take it away. A longer
        // one is shadowed by ours and needs nothing.
        for clash in clashes {
            if clash.keys.len() <= key.len() {
                let theirs = key_text(&clash.keys);
                if !unbind.contains(&theirs) {
                    unbind.push(theirs);
                }
            }
        }
        Ok(Some(key))
    };
    let board = resolve(board)?;
    let capture = resolve(capture)?;

    let managed = managed_file(binary, board.as_deref(), capture.as_deref(), &unbind);
    let init = with_require(before_init.as_deref());

    // Writes start here. Directories are created only now.
    let dir = match existing_dir {
        Some(dir) => dir,
        None => Dir::open(&dir_path, true)?,
    };
    let init_dir_owned = if init_dir_path == dir_path {
        None
    } else {
        Some(match existing_init_dir {
            Some(d) => d,
            None => Dir::open(&init_dir_path, true)?,
        })
    };
    let init_dir = init_dir_owned.as_ref().unwrap_or(&dir);
    dir.validate()?;
    init_dir.validate()?;
    let _lock = super::SetupLock(dir.lock()?);

    // Rex checks the managed file on its own first, so a bad key names the flag's file.
    let mut staged_managed = TempFile {
        dir: &dir,
        name: format!(".tsk-rex-{}.lua", uuid::Uuid::new_v4()).into(),
    };
    dir.write_new(&staged_managed.name, &managed)?;
    let errors = config_check(host, &dir.path.join(&staged_managed.name), None)?;
    if !errors.is_empty() {
        return Err(error(format!(
            "Rex rejected the tsk keys (no changes made): {}",
            // The staged file's name and line mean nothing to the user; the message does.
            errors
                .iter()
                .map(|e| e
                    .split_once(": ")
                    .map_or(e.as_str(), |(_, message)| message))
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let unchanged = || -> io::Result<()> {
        dir.validate()?;
        init_dir.validate()?;
        if init_dir.read(&init_name)? != before_init || dir.read(managed_name)? != before_managed {
            return Err(error("Rex config changed during setup; retry"));
        }
        Ok(())
    };
    unchanged()?;

    let init_changed = before_init.as_deref() != Some(init.as_str());
    let mut staged_init = TempFile {
        dir: init_dir,
        name: format!(".tsk-rex-init-{}.lua", uuid::Uuid::new_v4()).into(),
    };
    let mut staged_backup = None;
    let mut backup = None;
    if init_changed {
        init_dir.write_new(&staged_init.name, &init)?;
        if let Some(original) = &before_init {
            let staged = TempFile {
                dir: init_dir,
                name: format!(".tsk-rex-backup-{}.tmp", uuid::Uuid::new_v4()).into(),
            };
            init_dir.write_new(&staged.name, original)?;
            staged_backup = Some(staged);
            let name = backup_name_for(
                &init_name.to_string_lossy(),
                std::time::SystemTime::now(),
                &mut |candidate| init_dir.exists(Path::new(candidate)),
            )?;
            backup = Some(init_dir.path.join(name));
        }
    }
    unchanged()?;

    // Restore both files to what they were if anything after the first rename fails.
    let restore = |reason: String| -> io::Error {
        let mut problems = Vec::new();
        let put_back = |d: &Dir, name: &Path, text: &Option<String>| -> io::Result<()> {
            match text {
                Some(text) => {
                    let tmp = PathBuf::from(format!(".tsk-rex-restore-{}", uuid::Uuid::new_v4()));
                    d.write_new(&tmp, text)?;
                    d.rename(&tmp, name)
                }
                None => match d.remove(name, false) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                },
            }
        };
        if let Err(e) = put_back(&dir, managed_name, &before_managed) {
            problems.push(format!("{}: {e}", dir.path.join(MANAGED).display()));
        }
        if init_changed {
            if let Err(e) = put_back(init_dir, &init_name, &before_init) {
                problems.push(format!("{}: {e}", init_path.display()));
            }
        }
        if problems.is_empty() {
            error(format!("{reason}; the previous Rex config was restored"))
        } else {
            error(format!(
                "{reason}; restoring the previous Rex config failed: {}",
                problems.join("; ")
            ))
        }
    };

    dir.rename(&staged_managed.name, managed_name)?;
    staged_managed.preserve();
    if init_changed {
        if let Err(e) = init_dir.rename(&staged_init.name, &init_name) {
            return Err(restore(format!("config replacement failed: {e}")));
        }
    }
    staged_init.preserve();
    let errors = match config_check(host, &init_path, None) {
        Ok(errors) => errors,
        Err(e) => return Err(restore(e.to_string())),
    };
    if !errors.is_empty() {
        return Err(restore(format!(
            "Rex rejected the merged config ({})",
            errors.join("; ")
        )));
    }
    if let (Some(mut staged), Some(final_path)) = (staged_backup.take(), backup.as_ref()) {
        if let Err(e) = init_dir.rename(&staged.name, Path::new(final_path.file_name().unwrap())) {
            let pending = init_dir.path.join(&staged.name);
            staged.preserve();
            return Err(error(format!(
                "config saved, backup promotion failed: {e}; inspect {} and {}",
                pending.display(),
                final_path.display()
            )));
        }
    }
    rex_ok(host, &["config", "reload"]).map_err(|e| {
        error(format!(
            "Rex config written, but the reload failed: {e}\nRun rex config reload, or restart Rex, to apply the keys"
        ))
    })?;
    let after = parse_keymap(&rex_ok(host, &["keymap", "--json"])?)?;
    // The keymap lists each key's stack top first: a key counts only where tsk is on top.
    let top = |keys: &[String]| after.bindings.iter().find(|b| b.keys == keys);
    let labels = [(BOARD_ACTION, "board"), (CAPTURE_ACTION, "quick capture")];
    let shortcuts = labels
        .into_iter()
        .filter_map(|(action, label)| {
            let keys: Vec<String> = after
                .bindings
                .iter()
                .filter(|b| b.action == action)
                .filter(|b| top(&b.keys).is_some_and(|t| t.action == action))
                .map(|b| key_text(&b.keys))
                .collect();
            (!keys.is_empty()).then(|| (keys.join(" / "), label))
        })
        .collect();
    let shadowed = [(board, labels[0]), (capture, labels[1])]
        .into_iter()
        .filter_map(|(key, (action, label))| {
            let key = key?;
            let winner = top(&key)?;
            (winner.action != action).then(|| (key_text(&key), label, winner.source.clone()))
        })
        .collect();
    Ok(RexSetupResult {
        config: init_path,
        backup,
        declined_conflicts: declined,
        shortcuts,
        shadowed,
    })
}

#[cfg(test)]
mod tests;
