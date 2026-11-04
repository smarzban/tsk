//! `tsk setup rex` end to end against a stub `rex`: the installed binary path, the probe,
//! reruns, and a noninteractive conflict.
#![cfg(unix)]
use std::{
    fs,
    os::unix::{fs::symlink, process::CommandExt},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// Logs each call. `keymap` reports the bindings `tsk.lua` declares plus `$FIXTURE/user`
/// (one JSON binding object per line); `config check` passes unless `$SCENARIO` is `old`.
const STUB: &str = r#"#!/bin/sh
[ "$1" = --autostart=false ] && shift
printf '%s\n' "$*" >> "$FIXTURE/calls"
case "$1 $2" in
  'version ')
    if [ "$SCENARIO" = old ]; then rev=9; else rev=10; fi
    printf 'rex 0.0.1\napi: revision %s (supports 0 to %s)\n' "$rev" "$rev";;
  'keymap --json')
    sep=
    printf '{"bindings":['
    if [ -f "$FIXTURE/cfg/rex/tsk.lua" ]; then
      grep '^rex.bind(' "$FIXTURE/cfg/rex/tsk.lua" | sed 's/^rex.bind("\([^"]*\)", "\([^"]*\)")$/{"keys":["\1"],"action":"\2","source":"tsk.lua:1"}/' > "$FIXTURE/ours"
      for line in $(cat "$FIXTURE/ours"); do printf '%s%s' "$sep" "$line"; sep=,; done
    fi
    if [ -f "$FIXTURE/user" ]; then
      for line in $(cat "$FIXTURE/user"); do printf '%s%s' "$sep" "$line"; sep=,; done
    fi
    printf '],"diagnostics":[],"config":{"path":"%s"}}\n' "$REX_CONFIG";;
  'config check')
    printf '{"diagnostics":[]}\n';;
  'config reload')
    printf '{"generation":2}\n';;
  *) echo "unexpected: $*" >&2; exit 2;;
esac
"#;

struct Rex {
    root: PathBuf,
}

impl Rex {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "tsk-setup-rex-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("bin")).unwrap();
        symlink(env!("CARGO_BIN_EXE_tsk"), root.join("bin/tsk")).unwrap();
        crate::stub::write_stub(&root.join("bin/rex"), STUB, 0o755);
        Self { root }
    }
    fn config(&self) -> PathBuf {
        self.root.join("cfg/rex/init.lua")
    }
    fn run(&self, scenario: &str, args: &[&str]) -> Output {
        Command::new(self.root.join("bin/tsk"))
            .arg0("tsk")
            .args(["setup", "rex"])
            .args(args)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("REX_CONFIG", self.config())
            .env("FIXTURE", &self.root)
            .env("SCENARIO", scenario)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
    fn calls(&self) -> String {
        fs::read_to_string(self.root.join("calls")).unwrap_or_default()
    }
}

impl Drop for Rex {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn setup_writes_the_keys_reloads_and_the_probe_reports_bound() {
    let rex = Rex::new();
    let probe = rex.run("", &["--check"]);
    assert_eq!(String::from_utf8_lossy(&probe.stdout), "unbound\n");
    assert!(rex.calls().is_empty(), "the probe never calls rex");

    let first = rex.run("", &[]);
    assert!(first.status.success(), "{}", text(&first));
    let stdout = String::from_utf8_lossy(&first.stdout);
    assert!(
        stdout.contains("Shortcuts:      alt+t board, alt+a quick capture"),
        "{stdout}"
    );
    let managed = fs::read_to_string(rex.root.join("cfg/rex/tsk.lua")).unwrap();
    // The managed file launches the invoked path, not a PATH lookup.
    let bin = rex.root.join("bin/tsk");
    assert!(
        managed.contains(&format!("local TSK_BIN = \"{}\"\n", bin.display())),
        "{managed}"
    );
    assert!(rex.calls().contains("config reload"));

    let probe = rex.run("", &["--check"]);
    assert_eq!(String::from_utf8_lossy(&probe.stdout), "bound\n");

    let init = fs::read_to_string(rex.config()).unwrap();
    let again = rex.run("", &[]);
    assert!(again.status.success(), "{}", text(&again));
    assert_eq!(fs::read_to_string(rex.config()).unwrap(), init);
    assert_eq!(
        fs::read_to_string(rex.root.join("cfg/rex/tsk.lua")).unwrap(),
        managed
    );
}

#[test]
fn a_taken_key_refuses_without_a_terminal_and_writes_nothing() {
    let rex = Rex::new();
    fs::write(
        rex.root.join("user"),
        "{\"keys\":[\"alt+t\"],\"action\":\"pane.split\",\"source\":\"init.lua:3\"}\n",
    )
    .unwrap();
    let output = rex.run("", &[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("alt+t is already bound in the Rex config: alt+t -> pane.split (init.lua:3)"),
        "{stderr}"
    );
    assert!(!rex.root.join("cfg").exists());
    assert!(!rex.calls().contains("config reload"));

    let other = rex.run("", &["--board-key", "alt+y"]);
    assert!(other.status.success(), "{}", text(&other));
    assert!(String::from_utf8_lossy(&other.stdout).contains("alt+y board"));
}

#[test]
fn an_old_rex_is_refused_with_a_readable_message() {
    let rex = Rex::new();
    let output = rex.run("old", &[]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Rex API revision 9 found; tsk needs revision 10 or newer"),
        "{stderr}"
    );
    assert!(!rex.root.join("cfg").exists());
}
