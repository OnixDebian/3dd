//! The intent layer: keyboard events are mapped to [`Action`]s, decoupling key
//! bindings from state mutation. Kept minimal for the skeleton; later phases add
//! camera/selection/theme intents here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A high-level input intent produced from a raw key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Quit the application.
    Quit,
    /// No actionable intent for this key.
    None,
}

impl Action {
    /// Map a raw key event to an [`Action`].
    ///
    /// `q`, `Esc` and `Ctrl-C` all request quit; everything else is `None`.
    pub fn from_key(key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            _ => Action::None,
        }
    }
}
