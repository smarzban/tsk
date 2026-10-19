use std::io::Cursor;
use std::process::Command;

use tsk_tui::cli::run_with;

fn help(args: &[&str]) -> tsk_tui::cli::CliOutput {
    run_with(args, Cursor::new(Vec::<u8>::new()), true)
}

fn executable(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_tsk"))
        .args(args)
        .output()
        .expect("run tsk")
}

/// Each heading is a whole line (`usage:` starts one), found in this order.
fn assert_sections_in_order(output: &str, headings: &[&str]) {
    let lines: Vec<&str> = output.lines().collect();
    let mut previous = 0;
    for heading in headings {
        let position = lines[previous..]
            .iter()
            .position(|line| {
                line == heading || (*heading == "usage:" && line.starts_with("usage: "))
            })
            .unwrap_or_else(|| panic!("help should contain {heading:?} after the prior section"));
        previous += position;
    }
    assert!(
        lines.contains(&"Exit:"),
        "help should contain an exit section"
    );
}

macro_rules! verb_help_test {
    ($name:ident, [$($arg:expr),+], [$($heading:expr),+]) => {
        #[test]
        fn $name() {
            let output = help(&["tsk", $($arg),+]);
            assert_eq!(output.code, 0);
            assert!(output.stderr.is_empty());
            assert_sections_in_order(&output.stdout, &[$($heading),+]);
        }
    };
}

verb_help_test!(
    add_help_has_reference_sections,
    ["add", "--help"],
    [
        "usage:",
        "Scope",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    list_help_has_reference_sections,
    ["list", "--help"],
    [
        "usage:",
        "Arguments",
        "Scope",
        "Filters",
        "Arrange",
        "Options",
        "Output",
        "Examples:",
        "Exit:"
    ]
);
verb_help_test!(
    status_help_has_reference_sections,
    ["status", "--help"],
    [
        "usage:",
        "Arguments",
        "Started",
        "Done",
        "Blocked",
        "Review",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    edit_help_has_reference_sections,
    ["edit", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    steps_help_has_reference_sections,
    ["steps", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Place",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    reply_help_has_reference_sections,
    ["reply", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    dispatch_help_has_reference_sections,
    ["dispatch", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    clean_help_has_reference_sections,
    ["clean", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    trash_help_has_reference_sections,
    ["trash", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    backup_help_has_reference_sections,
    ["backup", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    restore_help_has_reference_sections,
    ["restore", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    archive_help_has_reference_sections,
    ["archive", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    unarchive_help_has_reference_sections,
    ["unarchive", "--help"],
    [
        "usage:",
        "Arguments",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    project_help_has_reference_sections,
    ["project", "--help"],
    [
        "usage:",
        "Arguments",
        "Actions",
        "Options",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    setup_help_has_reference_sections,
    ["setup", "--help"],
    [
        "usage:",
        "Actions",
        "Options",
        "Output",
        "Examples:",
        "Refusals (exit 1):",
        "Exit:"
    ]
);
verb_help_test!(
    update_help_has_reference_sections,
    ["update", "--help"],
    ["usage:", "Examples:", "Refusals (exit 1):", "Exit:"]
);
verb_help_test!(
    guide_help_has_reference_sections,
    ["guide", "--help"],
    ["usage:", "Examples:", "Exit:"]
);

#[test]
fn help_without_an_operand_is_top_level_help() {
    let via_help = executable(&["help"]);
    let global = executable(&["--help"]);
    assert_eq!(via_help.status.code(), Some(0));
    assert!(via_help.stderr.is_empty());
    assert_eq!(via_help.stdout, global.stdout);
    assert_eq!(
        String::from_utf8(via_help.stdout).expect("UTF-8 help"),
        tsk_tui::cli::presenter::top_level_help()
    );
}

#[test]
fn archive_help_uses_verb_specific_exit_lines() {
    let archive = help(&["tsk", "archive", "--help"]);
    assert!(archive.stdout.contains("0 task archived, or already was"));
    assert!(!archive.stdout.contains("task unarchived"));

    let unarchive = help(&["tsk", "unarchive", "--help"]);
    assert!(unarchive
        .stdout
        .contains("0 task unarchived, or already was"));
    assert!(!unarchive.stdout.contains("archived flag set"));
}

#[test]
fn command_help_lines_have_no_trailing_whitespace() {
    for args in [
        &["--help"][..],
        &["add", "--help"],
        &["list", "--help"],
        &["status", "--help"],
        &["edit", "--help"],
        &["steps", "--help"],
        &["reply", "--help"],
        &["dispatch", "--help"],
        &["clean", "--help"],
        &["archive", "--help"],
        &["unarchive", "--help"],
        &["project", "--help"],
        &["trash", "--help"],
        &["backup", "--help"],
        &["restore", "--help"],
        &["setup", "--help"],
        &["update", "--help"],
        &["guide", "--help"],
    ] {
        let output = executable(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
        assert!(
            stdout.lines().all(|line| line == line.trim_end()),
            "{args:?} contains trailing whitespace:\n{stdout}"
        );
    }
}

/// Help is reference text at 80 columns, and an example copies as one command: a wrapped
/// example would paste as two broken lines.
#[test]
fn help_fits_80_columns_and_examples_never_wrap() {
    for command in [
        "",
        "add",
        "list",
        "status",
        "edit",
        "steps",
        "reply",
        "dispatch",
        "clean",
        "archive",
        "unarchive",
        "project",
        "trash",
        "backup",
        "restore",
        "setup",
        "update",
        "guide",
    ] {
        let args: Vec<&str> = ["tsk", "help", command]
            .into_iter()
            .filter(|arg| !arg.is_empty())
            .collect();
        let output = help(&args);
        assert_eq!(output.code, 0, "{args:?}");
        for line in output.stdout.lines() {
            assert!(
                line.chars().count() <= 80,
                "{args:?} help is wider than 80 columns: {line:?}"
            );
        }
        let examples = output
            .stdout
            .lines()
            .skip_while(|line| *line != "Examples:")
            .skip(1)
            .take_while(|line| !line.is_empty());
        for line in examples {
            assert!(
                line.starts_with("  tsk ") || line.starts_with("  cat "),
                "{args:?} example wraps mid-command: {line:?}"
            );
        }
    }
}

#[test]
fn every_command_help_purpose_is_a_capitalized_sentence() {
    for args in [
        &["add", "--help"][..],
        &["list", "--help"],
        &["status", "--help"],
        &["edit", "--help"],
        &["steps", "--help"],
        &["reply", "--help"],
        &["dispatch", "--help"],
        &["clean", "--help"],
        &["archive", "--help"],
        &["unarchive", "--help"],
        &["project", "--help"],
        &["trash", "--help"],
        &["backup", "--help"],
        &["restore", "--help"],
        &["setup", "--help"],
        &["update", "--help"],
        &["guide", "--help"],
    ] {
        let output = help(&["tsk", args[0], "--help"]);
        let purpose = output
            .stdout
            .split("\n\n")
            .nth(1)
            .expect("purpose after usage");
        assert!(
            purpose.chars().next().is_some_and(char::is_uppercase) && purpose.ends_with('.'),
            "{args:?} purpose should be a capitalized sentence: {purpose:?}"
        );
    }
}

#[test]
fn help_operand_matches_verb_help_byte_for_byte() {
    let via_help = executable(&["help", "add"]);
    let direct = executable(&["add", "--help"]);
    assert_eq!(via_help.status.code(), Some(0));
    assert!(via_help.stderr.is_empty());
    assert_eq!(via_help.stdout, direct.stdout);
}

#[test]
fn unknown_help_operand_is_a_usage_error() {
    let output = executable(&["help", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 usage error"),
        "tsk help: unknown command nope\nusage: tsk help [<command>]\n"
    );
}

#[test]
fn add_version_is_not_a_global_flag_after_the_verb() {
    let output = help(&["tsk", "add", "--version"]);
    assert_eq!(output.code, 2);
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with("tsk add:"));
}

#[test]
fn version_flags_print_the_package_version() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_tsk"))
            .arg(flag)
            .output()
            .expect("run version flag");
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).expect("UTF-8 version"),
            format!("tsk {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}
