//! Positional command router.

/// The process surface selected from the binary argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Board,
    Capture,
    Add,
    Steps,
    List,
    Status,
    Dispatch,
    Edit,
    Trash,
    Archive,
    Unarchive,
    Project,
    Setup,
    Update,
    Guide,
    FindBoardPane,
    FindBoardTab,
    GlobalHelp,
    Help,
    Version,
    Usage,
}

/// Select a process surface from argv-style arguments, including argv0.
///
/// Global help and internal launcher flags are recognized only before the first
/// positional argument. A capture-mode environment value applies when there is no
/// subcommand or global flag.
pub fn route<S: AsRef<str>>(
    args: impl IntoIterator<Item = S>,
    capture_env: Option<&str>,
) -> Surface {
    let mut args = args.into_iter();
    let _argv0 = args.next();
    let mut global = None;

    for arg in args {
        let arg = arg.as_ref();
        if arg.starts_with('-') {
            let surface = match arg {
                "--find-board-pane" => Surface::FindBoardPane,
                "--find-board-tab" => Surface::FindBoardTab,
                "--help" => Surface::GlobalHelp,
                "--version" | "-V" => Surface::Version,
                _ => return Surface::Usage,
            };
            if global.replace(surface).is_some() {
                return Surface::Usage;
            }
            continue;
        }

        if global.is_some() {
            return Surface::Usage;
        }
        return match arg {
            "capture" => Surface::Capture,
            "add" => Surface::Add,
            "steps" => Surface::Steps,
            "list" => Surface::List,
            "status" => Surface::Status,
            "dispatch" => Surface::Dispatch,
            "edit" => Surface::Edit,
            "trash" => Surface::Trash,
            "archive" => Surface::Archive,
            "unarchive" => Surface::Unarchive,
            "project" => Surface::Project,
            "setup" => Surface::Setup,
            "update" => Surface::Update,
            "guide" => Surface::Guide,
            "help" => Surface::Help,
            _ => Surface::Usage,
        };
    }

    match global {
        Some(surface) => surface,
        None if capture_env.is_some_and(|value| value.eq_ignore_ascii_case("capture")) => {
            Surface::Capture
        }
        None => Surface::Board,
    }
}

#[cfg(test)]
mod tests {
    use super::{route, Surface};

    #[test]
    fn add_dash_t_capture_stays_add() {
        assert_eq!(route(["tsk", "add", "-t", "capture"], None), Surface::Add);
    }

    #[test]
    fn tokens_after_add_do_not_select_a_surface() {
        assert_eq!(
            route(["tsk", "add", "-t", "--find-board-pane"], None),
            Surface::Add
        );
    }

    #[test]
    fn unknown_positional_is_usage() {
        assert_eq!(route(["tsk", "foo"], None), Surface::Usage);
    }

    #[test]
    fn unknown_pre_positional_flag_is_usage_even_with_capture_env() {
        assert_eq!(
            route(["tsk", "--something"], Some("capture")),
            Surface::Usage
        );
    }

    #[test]
    fn find_board_pane_with_extra_argument_is_usage() {
        assert_eq!(
            route(["tsk", "--find-board-pane", "extra"], None),
            Surface::Usage
        );
    }

    #[test]
    fn find_board_tab_is_a_hidden_global_surface() {
        assert_eq!(
            route(["tsk", "--find-board-tab"], Some("capture")),
            Surface::FindBoardTab
        );
        assert_eq!(
            route(["tsk", "--find-board-tab", "extra"], None),
            Surface::Usage
        );
    }

    #[test]
    fn subcommand_wins_over_capture_env() {
        assert_eq!(route(["tsk", "add"], Some("capture")), Surface::Add);
    }

    #[test]
    fn steps_positional_selects_steps_surface() {
        assert_eq!(
            route(["tsk", "steps", "id", "toggle", "abc"], None),
            Surface::Steps
        );
    }

    #[test]
    fn trash_positional_selects_trash_surface() {
        assert_eq!(
            route(["tsk", "trash", "restore", "T3"], None),
            Surface::Trash
        );
    }

    #[test]
    fn status_positional_selects_status_surface() {
        assert_eq!(
            route(["tsk", "status", "T3", "started"], None),
            Surface::Status
        );
    }

    #[test]
    fn edit_positional_selects_edit_surface() {
        assert_eq!(
            route(["tsk", "edit", "T3", "--title", "new"], None),
            Surface::Edit
        );
    }

    #[test]
    fn capture_env_selects_capture_without_subcommand() {
        assert_eq!(route(["tsk"], Some("capture")), Surface::Capture);
    }

    #[test]
    fn no_args_selects_board() {
        assert_eq!(route(["tsk"], None), Surface::Board);
    }

    #[test]
    fn guide_selects_guide_surface() {
        assert_eq!(route(["tsk", "guide"], None), Surface::Guide);
    }

    #[test]
    fn update_selects_update_surface() {
        assert_eq!(route(["tsk", "update"], None), Surface::Update);
    }

    #[test]
    fn help_and_version_are_global_surfaces() {
        assert_eq!(route(["tsk", "help", "add"], None), Surface::Help);
        assert_eq!(route(["tsk", "--version"], None), Surface::Version);
        assert_eq!(route(["tsk", "-V"], None), Surface::Version);
        assert_eq!(route(["tsk", "add", "--version"], None), Surface::Add);
    }
}
