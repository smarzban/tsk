//! tsk binary entry.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use tsk_tui::cli::router::{route, Surface};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match route(&args, std::env::var(tsk_tui::app::MODE_ENV).ok().as_deref()) {
        Surface::FindBoardPane => find_board_main(false),
        Surface::FindBoardTab => find_board_main(true),
        Surface::GlobalHelp => {
            print!("{}", tsk_tui::cli::presenter::top_level_help());
            ExitCode::SUCCESS
        }
        Surface::Version => {
            println!("tsk {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Surface::Usage => usage_exit(),
        Surface::Update => update_main(&args),
        // Verbs that open the store need a home for it; setup, guide and help do not.
        Surface::Add
        | Surface::Steps
        | Surface::List
        | Surface::Status
        | Surface::Dispatch
        | Surface::Clean
        | Surface::Edit
        | Surface::Trash
        | Surface::Archive
        | Surface::Unarchive
        | Surface::Project => match tsk_tui::store::require_home_or_override(&args) {
            Ok(()) => headless_main(args),
            Err(message) => {
                eprintln!("tsk: {message}");
                ExitCode::from(1)
            }
        },
        Surface::Setup | Surface::Guide | Surface::Help => headless_main(args),
        Surface::Board | Surface::Capture => match tsk_tui::store::require_home_or_override(&args)
            .map_err(|message| message.into())
            .and_then(|()| tsk_tui::run(args))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("tsk: {err}");
                ExitCode::from(1)
            }
        },
    }
}

fn usage_exit() -> ExitCode {
    eprintln!("usage: tsk [capture] | <command> [args] | help [<command>] | --help | --version");
    ExitCode::from(2)
}

fn update_main(args: &[String]) -> ExitCode {
    // `update --help` is reference text like every other verb's: the headless runner owns it.
    if args.len() == 3 && args[2] == "--help" {
        return headless_main(args.to_vec());
    }
    if args.len() != 2 {
        eprintln!("usage: tsk update");
        return ExitCode::from(2);
    }

    match tsk_tui::cli::update::run() {
        Ok(tsk_tui::cli::update::UpdateOutcome::Homebrew) => {
            println!("tsk was installed with Homebrew. Run:\n  brew update && brew upgrade tsk");
            ExitCode::SUCCESS
        }
        Ok(tsk_tui::cli::update::UpdateOutcome::Installed) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tsk update: {error}");
            ExitCode::from(1)
        }
    }
}

fn headless_main(args: Vec<String>) -> ExitCode {
    let stdin = io::stdin();
    let stdin_is_tty = stdin.is_terminal();
    let terminal_width = io::stdout()
        .is_terminal()
        .then(|| crossterm::terminal::size().ok())
        .flatten()
        .map(|(columns, _)| usize::from(columns))
        .filter(|width| *width > 0);
    let output = tsk_tui::cli::run_with_terminal_width(args, stdin, stdin_is_tty, terminal_width);
    if io::stdout().write_all(output.stdout.as_bytes()).is_err() {
        return ExitCode::from(1);
    }
    if io::stderr().write_all(output.stderr.as_bytes()).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(output.code)
}

/// Read herdr `pane list` JSON; print the first Tasks pane or its tab, or exit 1.
fn find_board_main(tab: bool) -> ExitCode {
    let result = if tab {
        tsk_tui::board_pane::find_board_tab_from_stdin()
    } else {
        tsk_tui::find_board_pane_from_stdin()
    };
    match result {
        Ok(Some(id)) => {
            if writeln!(io::stdout(), "{id}").is_err() {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Ok(None) => ExitCode::from(1),
        Err(err) => {
            let flag = if tab {
                "--find-board-tab"
            } else {
                "--find-board-pane"
            };
            eprintln!("tsk {flag}: {err}");
            ExitCode::from(1)
        }
    }
}
