use super::*;
use std::{
    cell::RefCell,
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = env::temp_dir().join(format!(
            "tsk-setup-rex-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> PathBuf {
        self.0.join("rex/init.lua")
    }
    fn managed(&self) -> PathBuf {
        self.0.join("rex/tsk.lua")
    }
    fn write_init(&self, text: &str) {
        fs::create_dir_all(self.0.join("rex")).unwrap();
        fs::write(self.config(), text).unwrap();
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A fake `rex`: the keymap is the user's `bindings` plus every `rex.bind` line of the
/// managed file, minus keys it unbinds; `config check` fails on `error_marker`.
struct Fake {
    revision: u64,
    user: Vec<(&'static str, &'static str, &'static str)>,
    /// Bindings loaded after tsk.lua: on top of every stack, and out of reach of its unbinds.
    late: Vec<(&'static str, &'static str, &'static str)>,
    served: Option<PathBuf>,
    error_marker: Option<&'static str>,
    calls: RefCell<Vec<String>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            revision: MIN_REX_API_REVISION,
            user: Vec::new(),
            late: Vec::new(),
            served: None,
            error_marker: None,
            calls: RefCell::new(Vec::new()),
        }
    }
    fn keymap(&self, dir: &Path) -> String {
        let managed = fs::read_to_string(dir.join(MANAGED)).unwrap_or_default();
        let unbound: Vec<String> = managed
            .lines()
            .filter_map(|l| l.strip_prefix("rex.unbind(\""))
            .map(|l| l.trim_end_matches("\")").to_string())
            .collect();
        let mut bindings: Vec<serde_json::Value> = self
            .late
            .iter()
            .map(|(key, action, source)| {
                serde_json::json!({
                    "keys": key.split('>').collect::<Vec<_>>(),
                    "action": action,
                    "source": source,
                })
            })
            .collect();
        bindings.extend(
            managed
                .lines()
                .filter_map(|l| l.strip_prefix("rex.bind(\""))
                .map(|l| {
                    let (key, rest) = l.split_once("\", \"").unwrap();
                    serde_json::json!({
                        "keys": key.split('>').collect::<Vec<_>>(),
                        "action": rest.trim_end_matches("\")"),
                        "source": "tsk.lua:1",
                    })
                }),
        );
        for (key, action, source) in &self.user {
            if !unbound.iter().any(|u| u == key) {
                bindings.push(serde_json::json!({
                    "keys": key.split('>').collect::<Vec<_>>(),
                    "action": action,
                    "source": source,
                }));
            }
        }
        let mut json = serde_json::json!({ "bindings": bindings, "diagnostics": [] });
        if let Some(served) = &self.served {
            json["config"] = serde_json::json!({ "path": served });
        }
        json.to_string()
    }
    fn host<'a>(&'a self, dir: &'a Path) -> impl FnMut(&[&str]) -> io::Result<RexOutput> + 'a {
        move |args: &[&str]| {
            self.calls.borrow_mut().push(args.join(" "));
            let ok = |stdout: String| {
                Ok(RexOutput {
                    ok: true,
                    stdout,
                    stderr: String::new(),
                })
            };
            match args {
                ["version"] => ok(format!(
                    "rex 0.0.1\napi: revision {} (supports 0 to {})\n",
                    self.revision, self.revision
                )),
                ["keymap", "--json"] => ok(self.keymap(dir)),
                ["config", "check", "--json", path] => {
                    // The merged config is init.lua plus the module it requires.
                    let mut text = fs::read_to_string(path).unwrap_or_default();
                    if path.ends_with("init.lua") {
                        text.push_str(&fs::read_to_string(dir.join(MANAGED)).unwrap_or_default());
                    }
                    let failed = self.error_marker.is_some_and(|m| text.contains(m));
                    let diagnostics = if failed {
                        serde_json::json!([{ "severity": "error", "source": "init.lua:1", "message": "boom" }])
                    } else {
                        serde_json::json!([])
                    };
                    Ok(RexOutput {
                        ok: !failed,
                        stdout: serde_json::json!({ "diagnostics": diagnostics }).to_string(),
                        stderr: String::new(),
                    })
                }
                ["config", "reload"] => ok("{}".into()),
                other => panic!("unexpected rex call {other:?}"),
            }
        }
    }
}

fn run(
    temp: &Temp,
    fake: &Fake,
    options: Options,
    interactive: bool,
    answers: &str,
) -> (io::Result<RexSetupResult>, String) {
    let mut reader = io::Cursor::new(answers.as_bytes().to_vec());
    let mut prompt = Vec::new();
    let dir = temp.0.join("rex");
    let result = run_at(
        &temp.config(),
        "/opt/tsk \"bin\"/tsk",
        &options,
        &mut reader,
        &mut prompt,
        interactive,
        &mut fake.host(&dir),
    );
    (result, String::from_utf8(prompt).unwrap())
}

fn keys(board: &str, capture: &str) -> Options {
    Options {
        board_key: Some(board.into()),
        capture_key: Some(capture.into()),
    }
}

#[test]
fn fresh_setup_creates_both_files_with_the_default_keys() {
    let temp = Temp::new();
    let fake = Fake::new();
    let (result, prompt) = run(&temp, &fake, Options::default(), false, "");
    let result = result.unwrap();
    assert!(prompt.is_empty());
    assert!(result.backup.is_none());
    assert_eq!(
        result.shortcuts,
        vec![
            ("alt+t".to_string(), "board"),
            ("alt+a".to_string(), "quick capture")
        ]
    );
    let init = fs::read_to_string(temp.config()).unwrap();
    assert!(requires_tsk(&init));
    let managed = fs::read_to_string(temp.managed()).unwrap();
    assert!(managed.starts_with(MARKER));
    assert!(managed.contains("local TSK_BIN = \"/opt/tsk \\\"bin\\\"/tsk\"\n"));
    assert!(managed.contains("rex.bind(\"alt+t\", \"tsk_board\")\n"));
    assert!(managed.contains("rex.bind(\"alt+a\", \"tsk_capture\")\n"));
    assert!(fake.calls.borrow().contains(&"config reload".to_string()));
}

#[test]
fn rerun_keeps_one_require_and_rewrites_nothing() {
    let temp = Temp::new();
    temp.write_init("-- mine\nrex.bind(\"alt+q\", \"pane.split\")\n");
    let fake = Fake::new();
    let first = run(&temp, &fake, Options::default(), false, "").0.unwrap();
    let backup = first.backup.expect("an existing init.lua is backed up");
    assert_eq!(
        fs::read_to_string(backup).unwrap(),
        "-- mine\nrex.bind(\"alt+q\", \"pane.split\")\n"
    );
    let init = fs::read_to_string(temp.config()).unwrap();
    assert!(init.starts_with("-- mine\nrex.bind(\"alt+q\", \"pane.split\")\n\nrequire(\"tsk\")"));
    let managed = fs::read_to_string(temp.managed()).unwrap();

    let second = run(&temp, &fake, Options::default(), false, "").0.unwrap();
    assert!(second.backup.is_none());
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), init);
    assert_eq!(fs::read_to_string(temp.managed()).unwrap(), managed);
    assert_eq!(init.matches("require(\"tsk\")").count(), 1);
}

#[test]
fn a_require_the_user_already_wrote_is_kept() {
    let temp = Temp::new();
    let source = "require 'tsk'\nrex.bind(\"alt+q\", \"pane.split\")\n";
    temp.write_init(source);
    let result = run(&temp, &Fake::new(), Options::default(), false, "")
        .0
        .unwrap();
    assert!(result.backup.is_none());
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), source);
}

#[test]
fn a_taken_key_refuses_noninteractively_before_any_write() {
    let temp = Temp::new();
    temp.write_init("-- mine\n");
    let mut fake = Fake::new();
    fake.user = vec![("alt+t", "pane.send_key", "init.lua:2")];
    let (result, prompt) = run(&temp, &fake, Options::default(), false, "");
    let message = result.err().unwrap().to_string();
    assert!(message.contains("alt+t is already bound"), "{message}");
    assert!(message.contains("no changes made"), "{message}");
    assert!(prompt.is_empty());
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), "-- mine\n");
    assert!(!temp.managed().exists());
    assert!(!fake.calls.borrow().contains(&"config reload".to_string()));
}

#[test]
fn a_longer_or_shorter_sequence_on_the_same_chord_is_a_conflict() {
    for (theirs, ours) in [("alt+t>x", "alt+t"), ("ctrl+b", "ctrl+b>t")] {
        let temp = Temp::new();
        let mut fake = Fake::new();
        fake.user = vec![(theirs, "pane.split", "init.lua:1")];
        let error = run(&temp, &fake, keys(ours, "alt+a"), false, "")
            .0
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("already bound"),
            "{theirs} {ours}"
        );
    }
}

#[test]
fn a_declined_conflict_leaves_the_key_unbound_and_a_rerun_keeps_it_so() {
    let temp = Temp::new();
    let mut fake = Fake::new();
    fake.user = vec![("alt+t", "pane.send_key", "init.lua:2")];
    let (result, prompt) = run(&temp, &fake, Options::default(), true, "n\n");
    let result = result.unwrap();
    assert!(
        prompt.contains("alt+t -> pane.send_key (init.lua:2)"),
        "{prompt}"
    );
    assert!(prompt.contains("Replace the existing alt+t binding? [y/N]"));
    assert!(result.declined_conflicts);
    assert_eq!(
        result.shortcuts,
        vec![("alt+a".to_string(), "quick capture")]
    );
    let managed = fs::read_to_string(temp.managed()).unwrap();
    assert!(managed.contains("-- board-key: none\n"));
    assert!(!managed.contains("\"tsk_board\")\n"));
    // `tsk update` reruns setup with no flags and no terminal: no refusal, no prompt.
    let (again, prompt) = run(&temp, &fake, Options::default(), false, "");
    again.unwrap();
    assert!(prompt.is_empty());
    assert_eq!(fs::read_to_string(temp.managed()).unwrap(), managed);
}

#[test]
fn an_accepted_conflict_unbinds_the_users_key_in_tsk_lua_only() {
    let temp = Temp::new();
    temp.write_init("rex.bind(\"alt+t\", \"pane.send_key\")\n");
    let mut fake = Fake::new();
    fake.user = vec![("alt+t", "pane.send_key", "init.lua:1")];
    let result = run(&temp, &fake, Options::default(), true, "y\n")
        .0
        .unwrap();
    assert!(!result.declined_conflicts);
    let managed = fs::read_to_string(temp.managed()).unwrap();
    assert!(managed.contains("rex.unbind(\"alt+t\")\nrex.bind(\"alt+t\", \"tsk_board\")\n"));
    assert!(fs::read_to_string(temp.config())
        .unwrap()
        .starts_with("rex.bind(\"alt+t\", \"pane.send_key\")\n"));
}

#[test]
fn key_flags_are_recorded_and_kept_by_a_rerun_without_flags() {
    let temp = Temp::new();
    let fake = Fake::new();
    run(&temp, &fake, keys("ctrl+b>t", "none"), false, "")
        .0
        .unwrap();
    let managed = fs::read_to_string(temp.managed()).unwrap();
    assert!(managed.contains("-- board-key: ctrl+b>t\n-- capture-key: none\n"));
    let again = run(&temp, &fake, Options::default(), false, "").0.unwrap();
    assert_eq!(again.shortcuts, vec![("ctrl+b>t".to_string(), "board")]);
    assert_eq!(fs::read_to_string(temp.managed()).unwrap(), managed);
}

#[test]
fn an_action_the_user_bound_themselves_gets_no_default_key() {
    let temp = Temp::new();
    let mut fake = Fake::new();
    fake.user = vec![("alt+z>t", BOARD_ACTION, "init.lua:9")];
    let result = run(&temp, &fake, Options::default(), false, "").0.unwrap();
    let managed = fs::read_to_string(temp.managed()).unwrap();
    assert!(!managed.contains("rex.bind(\"alt+t\""));
    assert!(managed.contains("rex.bind(\"alt+a\", \"tsk_capture\")"));
    assert!(result.shortcuts.contains(&("alt+z>t".to_string(), "board")));
}

#[test]
fn overlapping_board_and_capture_keys_are_refused() {
    let temp = Temp::new();
    let error = run(&temp, &Fake::new(), keys("alt+t", "alt+t>a"), false, "")
        .0
        .err()
        .unwrap();
    assert!(error.to_string().contains("overlap"));
    assert!(!temp.0.join("rex").exists());
}

#[test]
fn malformed_key_flags_are_refused_before_rex_runs() {
    for bad in ["", "alt+t>", "alt t", "alt+\"t", ">t"] {
        let temp = Temp::new();
        let fake = Fake::new();
        let error = run(&temp, &fake, keys(bad, "alt+a"), false, "")
            .0
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("--board-key"),
            "{bad:?}: {error}"
        );
        assert!(fake.calls.borrow().is_empty());
    }
}

#[test]
fn an_unmanaged_tsk_lua_is_never_overwritten() {
    let temp = Temp::new();
    temp.write_init("");
    fs::write(temp.managed(), "-- my own tsk module\n").unwrap();
    let error = run(&temp, &Fake::new(), Options::default(), false, "")
        .0
        .err()
        .unwrap();
    assert!(error.to_string().contains("was not written by tsk"));
    assert_eq!(
        fs::read_to_string(temp.managed()).unwrap(),
        "-- my own tsk module\n"
    );
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), "");
}

#[test]
fn an_old_rex_is_refused_before_any_write() {
    let temp = Temp::new();
    let mut fake = Fake::new();
    fake.revision = MIN_REX_API_REVISION - 1;
    let error = run(&temp, &fake, Options::default(), false, "")
        .0
        .err()
        .unwrap();
    assert!(error.to_string().contains("Update Rex"), "{error}");
    assert!(!temp.0.join("rex").exists());
}

#[test]
fn a_user_config_with_errors_is_refused_before_any_write() {
    let temp = Temp::new();
    temp.write_init("BROKEN\n");
    let mut fake = Fake::new();
    fake.error_marker = Some("BROKEN");
    let error = run(&temp, &fake, Options::default(), false, "")
        .0
        .err()
        .unwrap();
    assert!(error.to_string().contains("the Rex config has errors"));
    assert!(!temp.managed().exists());
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), "BROKEN\n");
}

#[test]
fn a_merged_config_rex_rejects_is_rolled_back() {
    let temp = Temp::new();
    temp.write_init("-- mine\n");
    let mut fake = Fake::new();
    // Only the merged config (init.lua with the require line) trips the check.
    fake.error_marker = Some("-- mine\n\nrequire(\"tsk\")");
    let error = run(&temp, &fake, Options::default(), false, "")
        .0
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("previous Rex config was restored"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(temp.config()).unwrap(), "-- mine\n");
    assert!(!temp.managed().exists());
    let leftovers: Vec<_> = fs::read_dir(temp.0.join("rex"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "init.lua" && name != ".tsk-setup.lock")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(!fake.calls.borrow().contains(&"config reload".to_string()));
}

#[test]
fn a_server_reading_another_config_is_refused() {
    let temp = Temp::new();
    let mut fake = Fake::new();
    fake.served = Some(temp.0.join("elsewhere/init.lua"));
    let error = run(&temp, &fake, Options::default(), false, "")
        .0
        .err()
        .unwrap();
    assert!(error.to_string().contains("the running Rex server reads"));
    assert!(!temp.0.join("rex").exists());
}

#[test]
fn require_detection_ignores_comments() {
    assert!(requires_tsk("require(\"tsk\")\n"));
    assert!(requires_tsk("  require 'tsk' -- keys\n"));
    assert!(requires_tsk("require ( \"tsk\" )\n"));
    assert!(!requires_tsk("-- require(\"tsk\")\n"));
    assert!(!requires_tsk("require(\"tsk2\")\n"));
}

#[test]
fn lua_strings_escape_quotes_backslashes_and_control_bytes() {
    assert_eq!(lua_string("a\"b\\c\nd"), "\"a\\\"b\\\\c\\010d\"");
}

#[test]
fn rex_version_parsing_reads_the_api_revision() {
    require_min_rex("rex 0.0.1\napi: revision 10 (supports 0 to 10)\n").unwrap();
    require_min_rex("api: revision 11\n").unwrap();
    assert!(require_min_rex("api: revision 9 (supports 0 to 9)\n").is_err());
    assert!(require_min_rex("rex 0.0.1\n").is_err());
}

#[test]
fn tsks_own_require_line_moves_back_to_the_end() {
    let source = format!("-- mine\n{REQUIRE_LINE}\nrequire(\"later\")\n");
    assert_eq!(
        with_require(Some(&source)),
        format!("-- mine\nrequire(\"later\")\n\n{REQUIRE_LINE}\n")
    );
    let settled = with_require(Some(&source));
    assert_eq!(with_require(Some(&settled)), settled);
    // A require the user wrote stays where they put it.
    let theirs = "require(\"tsk\")\nrequire(\"later\")\n";
    assert_eq!(with_require(Some(theirs)), theirs);
}

#[test]
fn a_key_a_later_binding_still_takes_is_reported_not_claimed() {
    let temp = Temp::new();
    let mut fake = Fake::new();
    fake.late = vec![("alt+t", "pane.send_key", "mine.lua:1")];
    let result = run(&temp, &fake, Options::default(), true, "y\n")
        .0
        .unwrap();
    assert_eq!(
        result.shortcuts,
        vec![("alt+a".to_string(), "quick capture")]
    );
    assert_eq!(
        result.shadowed,
        vec![("alt+t".to_string(), "board", "mine.lua:1".to_string())]
    );
}
