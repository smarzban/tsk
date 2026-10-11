//! The launch card: the board opened in an archived project's directory. `y` unarchives it,
//! `n` keeps it archived and sends quick-add to the desk for the session; the cursor starts on
//! `n`, which changes nothing on disk.

use std::path::PathBuf;

use super::selector::{CardChoice, CardOption, OptionCard, OptionSelector};
use crate::ui::input::BoardIntent;

/// One of the launch card's choices, by its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchChoice {
    /// `y`: unarchive the project.
    Unarchive,
    /// `n` (and `Esc`): keep it archived.
    KeepArchived,
}

impl CardChoice for LaunchChoice {
    fn key(self) -> &'static str {
        match self {
            Self::Unarchive => "y",
            Self::KeepArchived => "n",
        }
    }

    fn intent(self) -> BoardIntent {
        match self {
            Self::Unarchive => BoardIntent::LaunchUnarchive,
            Self::KeepArchived => BoardIntent::LaunchKeepArchived,
        }
    }
}

/// The launch card while it is up: the archived project it names and its selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPrompt {
    pub path: PathBuf,
    pub scroll: usize,
    pub follow: bool,
    pub selector: OptionSelector<LaunchChoice>,
}

impl LaunchPrompt {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            scroll: 0,
            follow: true,
            selector: OptionSelector::default(),
        }
    }

    /// The project's directory name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .map_or_else(|| self.path.to_string_lossy().into_owned(), str::to_string)
    }

    pub fn options(&self) -> Vec<CardOption<LaunchChoice>> {
        let name = self.name();
        vec![
            CardOption {
                choice: LaunchChoice::Unarchive,
                text: format!("unarchive {name} · quick-add goes to it"),
                bold: false,
            },
            CardOption {
                choice: LaunchChoice::KeepArchived,
                text: format!("keep {name} archived · quick-add goes to your desk this session"),
                bold: true,
            },
        ]
    }
}

impl OptionCard for LaunchPrompt {
    type Choice = LaunchChoice;

    fn options(&self) -> Vec<CardOption<LaunchChoice>> {
        LaunchPrompt::options(self)
    }

    fn selector(&self) -> &OptionSelector<LaunchChoice> {
        &self.selector
    }

    fn selector_mut(&mut self) -> &mut OptionSelector<LaunchChoice> {
        &mut self.selector
    }

    fn scroll_follow(&mut self) -> (&mut usize, &mut bool) {
        (&mut self.scroll, &mut self.follow)
    }
}
