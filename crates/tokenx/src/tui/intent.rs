//! Typed user intent.
//!
//! Key bindings are decoded exactly once into this vocabulary. Capability
//! checks and state transitions consume the same value, so advertised and
//! executable behavior cannot drift into separate key tables.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::actions::Action;
use super::interaction::MoveCommand;
use super::model::{SortField, Tab};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Intent {
    Interrupt,
    Quit,
    Move(MoveCommand),
    PreviousTab,
    NextTab,
    Sort(SortField),
    OpenDetails,
    Back,
    ToggleView,
    Clients,
    GroupBy,
    Theme,
    Language,
    ToggleAutoRefresh,
    IncreaseRefreshInterval,
    DecreaseRefreshInterval,
    RefreshLocal,
    RefreshSubscription,
    Copy,
    Export,
    SelectTab(Tab),
    SelectGraphCell { week: usize, day: usize },
}

impl Intent {
    pub(crate) fn from_key(tab: Tab, key: KeyEvent) -> Option<Self> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Self::Interrupt);
        }

        match key.code {
            KeyCode::Char('q') => Some(Self::Quit),
            KeyCode::Up => Some(Self::Move(MoveCommand::Up)),
            KeyCode::Down => Some(Self::Move(MoveCommand::Down)),
            KeyCode::PageUp => Some(Self::Move(MoveCommand::PageUp)),
            KeyCode::PageDown => Some(Self::Move(MoveCommand::PageDown)),
            KeyCode::Home => Some(Self::Move(MoveCommand::Home)),
            KeyCode::End => Some(Self::Move(MoveCommand::End)),
            KeyCode::Left | KeyCode::BackTab => Some(Self::PreviousTab),
            KeyCode::Right | KeyCode::Tab => Some(Self::NextTab),
            KeyCode::Char('d') => Some(Self::Sort(SortField::Date)),
            KeyCode::Char('t') => Some(Self::Sort(SortField::Tokens)),
            KeyCode::Char('c') => Some(Self::Sort(SortField::Cost)),
            KeyCode::Enter => Some(Self::OpenDetails),
            KeyCode::Esc | KeyCode::Backspace => Some(Self::Back),
            KeyCode::Char('h') if tab == Tab::Overview => Some(Self::ToggleView),
            KeyCode::Char('v') if matches!(tab, Tab::Daily | Tab::Hourly) => Some(Self::ToggleView),
            KeyCode::Char('s') => Some(Self::Clients),
            KeyCode::Char('g') => Some(Self::GroupBy),
            KeyCode::Char('p') => Some(Self::Theme),
            KeyCode::Char('L') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(Self::Language)
            }
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some(Self::ToggleAutoRefresh)
            }
            KeyCode::Char('+') | KeyCode::Char('=') => Some(Self::IncreaseRefreshInterval),
            KeyCode::Char('-') => Some(Self::DecreaseRefreshInterval),
            KeyCode::Char('r') => Some(Self::RefreshLocal),
            KeyCode::Char('u') => Some(Self::RefreshSubscription),
            KeyCode::Char('y') => Some(Self::Copy),
            KeyCode::Char('e') => Some(Self::Export),
            _ => None,
        }
    }

    pub(crate) const fn action(self) -> Option<Action> {
        match self {
            Self::Interrupt | Self::Quit => Some(Action::Quit),
            Self::Move(_) => Some(Action::Scroll),
            Self::PreviousTab => Some(Action::PreviousTab),
            Self::NextTab => Some(Action::NextTab),
            Self::Sort(field) => Some(Action::Sort(field)),
            Self::OpenDetails => Some(Action::OpenDetails),
            Self::Back => Some(Action::Back),
            Self::ToggleView => Some(Action::ToggleView),
            Self::Clients => Some(Action::Clients),
            Self::GroupBy => Some(Action::GroupBy),
            Self::Theme => Some(Action::Theme),
            Self::Language => Some(Action::Language),
            Self::ToggleAutoRefresh => Some(Action::ToggleAutoRefresh),
            Self::IncreaseRefreshInterval => Some(Action::IncreaseRefreshInterval),
            Self::DecreaseRefreshInterval => Some(Action::DecreaseRefreshInterval),
            Self::RefreshLocal => Some(Action::RefreshLocal),
            Self::RefreshSubscription => Some(Action::RefreshSubscription),
            Self::Copy => Some(Action::Copy),
            Self::Export => Some(Action::Export),
            Self::SelectTab(_) | Self::SelectGraphCell { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uppercase_l_is_the_only_language_shortcut() {
        let uppercase = KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT);
        let lowercase = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);

        assert_eq!(
            Intent::from_key(Tab::Overview, uppercase),
            Some(Intent::Language)
        );
        assert_eq!(Intent::from_key(Tab::Overview, lowercase), None);
        assert_eq!(Intent::Language.action(), Some(Action::Language));
    }
}
