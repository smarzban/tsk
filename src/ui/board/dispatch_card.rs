//! The start cards' options: the relaunch card, the start-anyway card and the bulk start card
//! over a marked set. Each option line says what its key does; the painter, the keys and the
//! mouse all read the same list through the shared selector.

use super::cleanup_card::tasks;
use super::model::DispatchPrompt;
use super::selector::{CardChoice, CardOption, OptionCard, OptionSelector};
use crate::ui::input::BoardIntent;

/// One of a start card's choices, by its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchChoice {
    /// `y`: launch (or relaunch), or start anyway.
    Launch,
    /// `n`, relaunch card only: start the task, leave the agent stopped.
    StartOnly,
}

impl CardChoice for DispatchChoice {
    fn key(self) -> &'static str {
        match self {
            Self::Launch => "y",
            Self::StartOnly => "n",
        }
    }

    fn intent(self) -> BoardIntent {
        match self {
            Self::Launch => BoardIntent::ConfirmDispatch,
            Self::StartOnly => BoardIntent::StartWithoutRelaunch,
        }
    }
}

impl DispatchPrompt {
    /// The card's options, in key order.
    pub fn options(&self) -> Vec<CardOption<DispatchChoice>> {
        let option = |choice, text: String, bold| CardOption { choice, text, bold };
        if let Some(relaunch) = &self.relaunch {
            let number = relaunch.number;
            let assignee = &relaunch.assignee;
            // Starting a dispatched task whose agent is gone most likely means its agent.
            return vec![
                option(
                    DispatchChoice::Launch,
                    format!("relaunch @{assignee} in the same worktree · start T{number}"),
                    true,
                ),
                option(
                    DispatchChoice::StartOnly,
                    format!("start T{number} · don't relaunch @{assignee}"),
                    false,
                ),
            ];
        }
        if self.start_anyway.is_some() {
            return vec![option(
                DispatchChoice::Launch,
                "start now, without waiting".into(),
                true,
            )];
        }
        let launch = self
            .launch
            .iter()
            .map(|eligible| format!("T{}", eligible.number))
            .collect::<Vec<_>>();
        let start_only = self
            .start_only
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let skipped = self
            .skipped
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let mut parts = Vec::new();
        if !launch.is_empty() {
            parts.push(format!("dispatch {}", tasks(&launch)));
        }
        if !start_only.is_empty() {
            parts.push(if launch.is_empty() {
                format!("start {}", tasks(&start_only))
            } else {
                format!("start {} without launching", tasks(&start_only))
            });
        }
        if parts.is_empty() {
            return Vec::new();
        }
        if !skipped.is_empty() {
            parts.push(format!("{} not started", tasks(&skipped)));
        }
        vec![option(DispatchChoice::Launch, parts.join(" · "), true)]
    }
}

impl OptionCard for DispatchPrompt {
    type Choice = DispatchChoice;

    fn options(&self) -> Vec<CardOption<DispatchChoice>> {
        DispatchPrompt::options(self)
    }

    fn selector(&self) -> &OptionSelector<DispatchChoice> {
        &self.selector
    }

    fn selector_mut(&mut self) -> &mut OptionSelector<DispatchChoice> {
        &mut self.selector
    }

    fn scroll_follow(&mut self) -> (&mut usize, &mut bool) {
        (&mut self.scroll, &mut self.follow)
    }
}
