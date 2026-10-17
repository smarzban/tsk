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

fn assert_sections_in_order(output: &str, headings: &[&str]) {
    let mut previous = 0;
    for heading in headings {
        let position = output
            .find(heading)
            .unwrap_or_else(|| panic!("help should contain {heading:?}"));
        assert!(
            position >= previous,
            "{heading:?} should follow the prior section"
        );
        previous = position;
    }
    assert!(
        output.contains("Exit:"),
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
        "Output",
        "Values",
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
        "Scope",
        "Filters",
        "Output",
        "Values",
        "Examples:",
        "Exit:"
    ]
);
verb_help_test!(
    status_help_has_reference_sections,
    ["status", "--help"],
    [
        "usage:",
        "Values",
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
        "Values",
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
        "Values",
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
        "Values",
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
        "Actions",
        "Values",
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
        "Values",
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
        "Values",
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
        "Values",
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
        "Scope",
        "Values",
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

#[test]
fn every_command_help_purpose_is_a_capitalized_sentence() {
    for args in [
        &["add", "--help"][..],
        &["list", "--help"],
        &["status", "--help"],
        &["edit", "--help"],
        &["steps", "--help"],
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
