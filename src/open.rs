//! Open an artifact with the host's system opener.
//!
//! The target is always passed as one argument, never through a shell built from task text.
//! Starting the opener is fire-and-forget: a failure becomes a status message, never a crash.

use std::process::Command;

/// Launch the system opener for `target`, returning a short reason on failure.
pub fn open_target(target: &str) -> Result<(), String> {
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("cmd", vec!["/C", "start", "", target])
    } else if cfg!(target_os = "macos") {
        ("open", vec![target])
    } else {
        ("xdg-open", vec![target])
    };
    Command::new(program)
        .args(args)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot open {target}: {error}"))
}
