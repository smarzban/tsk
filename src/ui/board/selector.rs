//! The option selector every decision card shares (cleanup, relaunch, start anyway, the bulk
//! start card, the launch card): one line per option starting with its key, a `▸` cursor that
//! `↑`/`↓` move and `Enter` runs, letter keys that run their option directly, a single click
//! that selects and a double click that runs. The card says what its options are; this module
//! owns the cursor, the early-`Enter` guard and the double click, so every card behaves alike.

use std::time::{Duration, Instant};

use crate::ui::input::BoardIntent;
use crate::ui::render::CleanupCardLine;

/// An `Enter` this soon after a card opened is the tail of the keypress that opened it.
pub const CARD_ENTER_GUARD: Duration = Duration::from_millis(300);

/// One choice of a card, by its key.
pub trait CardChoice: Copy + Eq {
    /// The letter that runs it, painted at the start of its line.
    fn key(self) -> &'static str;
    /// The intent that carries it out.
    fn intent(self) -> BoardIntent;
}

/// One option line: its choice, what it does, and whether it is the card's likely or safe
/// choice (bold, and where the cursor starts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardOption<C> {
    pub choice: C,
    pub text: String,
    pub bold: bool,
}

/// The cursor over a card's options, its open time and its last single click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionSelector<C> {
    /// The option under the `▸` cursor; `None` rests on the card's bold option.
    pub selected: Option<C>,
    /// When the card opened: an `Enter` this soon after is the tail of a double tap.
    pub opened_at: Instant,
    /// The last single click on an option line, for its double click.
    pub option_click: Option<(Instant, C)>,
}

impl<C: CardChoice> Default for OptionSelector<C> {
    fn default() -> Self {
        Self {
            selected: None,
            opened_at: Instant::now(),
            option_click: None,
        }
    }
}

impl<C: CardChoice> OptionSelector<C> {
    /// The option under the cursor: the chosen one while it is still offered, else the bold
    /// one, else the first.
    pub fn selected_index(&self, options: &[CardOption<C>]) -> Option<usize> {
        self.selected
            .and_then(|choice| options.iter().position(|option| option.choice == choice))
            .or_else(|| options.iter().position(|option| option.bold))
            .or(if options.is_empty() { None } else { Some(0) })
    }

    /// Move the cursor one option up or down, stopping at the ends.
    pub fn step(&mut self, options: &[CardOption<C>], down: bool) -> Option<usize> {
        let index = self.selected_index(options)?;
        let next = if down {
            (index + 1).min(options.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        self.selected = Some(options[next].choice);
        Some(next)
    }

    /// `Enter`: the selected option, unless it lands within [`CARD_ENTER_GUARD`] of the card
    /// opening.
    pub fn enter(&self, options: &[CardOption<C>], now: Instant) -> Option<C> {
        if now.duration_since(self.opened_at) < CARD_ENTER_GUARD {
            return None;
        }
        self.selected_index(options)
            .map(|index| options[index].choice)
    }

    /// A click on option `index`: the first selects it, a second on the same option within
    /// `window` runs it.
    pub fn click(
        &mut self,
        options: &[CardOption<C>],
        index: usize,
        now: Instant,
        window: Duration,
    ) -> Option<C> {
        let choice = options.get(index)?.choice;
        let double = self
            .option_click
            .is_some_and(|(at, last)| last == choice && now.duration_since(at) <= window);
        self.selected = Some(choice);
        if double {
            self.option_click = None;
            Some(choice)
        } else {
            self.option_click = Some((now, choice));
            None
        }
    }

    /// The card's option lines, the `▸` on the selected one.
    pub fn lines(&self, options: &[CardOption<C>]) -> Vec<CleanupCardLine> {
        let selected = self.selected_index(options);
        options
            .iter()
            .enumerate()
            .map(|(index, option)| CleanupCardLine::Option {
                index,
                key: option.choice.key(),
                text: option.text.clone(),
                bold: option.bold,
                selected: selected == Some(index),
            })
            .collect()
    }
}

/// A card with a selector: what its options are and which of its keys act now.
pub trait OptionCard {
    type Choice: CardChoice;

    fn options(&self) -> Vec<CardOption<Self::Choice>>;
    fn selector(&self) -> &OptionSelector<Self::Choice>;
    fn selector_mut(&mut self) -> &mut OptionSelector<Self::Choice>;
    /// The card's first visible row and whether the painter scrolls to the `▸` option.
    fn scroll_follow(&mut self) -> (&mut usize, &mut bool);

    /// Whether `choice`'s key acts now: by default, whether the card offers it.
    fn accepts(&self, choice: Self::Choice) -> bool {
        self.options().iter().any(|option| option.choice == choice)
    }

    /// `Enter`, a click on an option line, or a letter key (`key_choice`) becomes the intent
    /// it runs. `None`: nothing runs (inside the guard, a first click, a key not offered).
    fn resolve(
        &mut self,
        intent: BoardIntent,
        key_choice: Option<Self::Choice>,
        now: Instant,
    ) -> Option<BoardIntent> {
        let options = self.options();
        let choice = match intent {
            BoardIntent::CleanupRunSelected => self.selector().enter(&options, now)?,
            BoardIntent::CleanupClickOption(index) => {
                self.selector_mut()
                    .click(&options, index, now, super::ROW_DOUBLE_CLICK_WINDOW)?
            }
            _ => key_choice?,
        };
        self.accepts(choice).then(|| choice.intent())
    }
}

/// A card's cursor without its choice type, for the scrolling that keeps it in view.
pub trait CardCursor {
    /// The selected option's index.
    fn selected_option(&self) -> Option<usize>;
    /// `↑`/`↓`: the newly selected option's index.
    fn step_option(&mut self, down: bool) -> Option<usize>;
    fn scroll_follow(&mut self) -> (&mut usize, &mut bool);
}

impl<T: OptionCard> CardCursor for T {
    fn selected_option(&self) -> Option<usize> {
        self.selector().selected_index(&self.options())
    }

    fn step_option(&mut self, down: bool) -> Option<usize> {
        let options = self.options();
        self.selector_mut().step(&options, down)
    }

    fn scroll_follow(&mut self) -> (&mut usize, &mut bool) {
        OptionCard::scroll_follow(self)
    }
}
