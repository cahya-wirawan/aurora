//! Keyboard shortcuts: a small, platform-agnostic chord vocabulary and a
//! chord -> command registry. PLAN.md M1.8's "keyboard shortcuts"
//! deliverable, keeping the same "abstract steps, not `winit` types"
//! seam [`crate::FocusManager`]/[`crate::hit_test`] already hold —
//! translating a real `winit::event::KeyEvent`/`ModifiersState` into a
//! [`KeyChord`] is `aurora-app`'s job, not this crate's.
//!
//! [`ShortcutRegistry`] is generic over the command it dispatches to —
//! this crate has no idea what "Undo" or "Toggle Command Palette" mean,
//! the same "knows nothing about documents or layers" boundary the rest
//! of this crate keeps; `aurora-app` instantiates it with its own
//! command type.

use std::collections::HashMap;
use std::fmt;

/// Which modifier keys were held down alongside a [`KeyChord`]'s own
/// [`Key`]. Four flat booleans, not a bitflags type — there are exactly
/// four modifier families every mainstream platform recognises (`Ctrl`,
/// `Shift`, `Alt`/`Option`, `Super`/`Cmd`/`Meta`), and a caller building
/// one by hand (tests, `aurora-app`) reads more plainly as struct fields
/// than as bit constants.
// Four flat bools, not a state machine or paired two-variant enums --
// these are four genuinely independent, simultaneously-settable
// modifier keys (a chord can hold all four down together), which is
// exactly the shape `bool` fields express directly and an enum would not.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    /// The `Cmd` key on macOS, the `Super`/`Windows` key elsewhere.
    pub meta: bool,
}

impl Modifiers {
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_none(self) -> bool {
        self == Self::default()
    }
}

/// The non-character key a [`KeyChord`] can name — a small, deliberately
/// partial vocabulary (not `winit::keyboard::NamedKey`'s own ~60-variant
/// set) covering only what a shortcut or command-palette binding
/// plausibly needs: navigation, activation, and the function-key row.
/// `#[non_exhaustive]`: more variants will be added as real shortcuts
/// need them, matching [`crate::WidgetError`]'s own growth policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NamedKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl fmt::Display for NamedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Enter => "Enter",
            Self::Escape => "Esc",
            Self::Tab => "Tab",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Space => "Space",
            Self::ArrowUp => "Up",
            Self::ArrowDown => "Down",
            Self::ArrowLeft => "Left",
            Self::ArrowRight => "Right",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F6 => "F6",
            Self::F7 => "F7",
            Self::F8 => "F8",
            Self::F9 => "F9",
            Self::F10 => "F10",
            Self::F11 => "F11",
            Self::F12 => "F12",
        };
        f.write_str(label)
    }
}

/// The key a [`KeyChord`] names, on top of its own [`Modifiers`].
///
/// **Case convention**: [`Key::Character`] is always lowercase — both
/// [`KeyChord::parse`] and `aurora-app`'s own winit-event translation
/// normalise via `to_ascii_lowercase` at construction, so a shortcut
/// written `"Ctrl+P"` and a physical `Shift`-less `p` keystroke compare
/// equal without this type's own `Eq` needing to know about case at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Character(char),
    Named(NamedKey),
}

/// One key combination — [`Modifiers`] plus a [`Key`]. The unit a
/// [`ShortcutRegistry`] binds a command to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl KeyChord {
    #[must_use]
    pub fn new(modifiers: Modifiers, key: Key) -> Self {
        Self { modifiers, key }
    }

    /// Parses a human-authored shortcut string like `"Ctrl+Shift+P"` or
    /// `"F1"` — tokens separated by `+`, case-insensitive, whitespace
    /// around a token ignored. The last token is the key; every token
    /// before it must be a recognised modifier spelling: `Ctrl`/
    /// `Control`, `Shift`, `Alt`/`Option`, or `Cmd`/`Command`/`Meta`/
    /// `Super`/`Win`/`Windows` (all four map to [`Modifiers::meta`] —
    /// this type doesn't try to guess a platform's own primary-modifier
    /// convention; a caller registering cross-platform defaults picks
    /// the spelling that reads naturally and both mean the same
    /// [`Modifiers::meta`] bit).
    ///
    /// # Errors
    ///
    /// Returns [`ParseChordError::Empty`] for an empty (or all-whitespace)
    /// string, [`ParseChordError::UnknownModifier`] for an unrecognised
    /// modifier token, or [`ParseChordError::UnknownKey`] for a key token
    /// that's neither a single character nor a recognised [`NamedKey`]
    /// spelling.
    pub fn parse(source: &str) -> Result<Self, ParseChordError> {
        let tokens: Vec<&str> = source
            .split('+')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .collect();
        let Some((&key_token, modifier_tokens)) = tokens.split_last() else {
            return Err(ParseChordError::Empty);
        };

        let mut modifiers = Modifiers::default();
        for token in modifier_tokens {
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => modifiers.control = true,
                "shift" => modifiers.shift = true,
                "alt" | "option" => modifiers.alt = true,
                "cmd" | "command" | "meta" | "super" | "win" | "windows" => modifiers.meta = true,
                other => return Err(ParseChordError::UnknownModifier(other.to_owned())),
            }
        }

        Ok(Self {
            modifiers,
            key: parse_key(key_token)?,
        })
    }
}

fn parse_key(token: &str) -> Result<Key, ParseChordError> {
    let lower = token.to_ascii_lowercase();
    let named = match lower.as_str() {
        "enter" | "return" => Some(NamedKey::Enter),
        "escape" | "esc" => Some(NamedKey::Escape),
        "tab" => Some(NamedKey::Tab),
        "backspace" => Some(NamedKey::Backspace),
        "delete" | "del" => Some(NamedKey::Delete),
        "space" => Some(NamedKey::Space),
        "up" | "arrowup" => Some(NamedKey::ArrowUp),
        "down" | "arrowdown" => Some(NamedKey::ArrowDown),
        "left" | "arrowleft" => Some(NamedKey::ArrowLeft),
        "right" | "arrowright" => Some(NamedKey::ArrowRight),
        "f1" => Some(NamedKey::F1),
        "f2" => Some(NamedKey::F2),
        "f3" => Some(NamedKey::F3),
        "f4" => Some(NamedKey::F4),
        "f5" => Some(NamedKey::F5),
        "f6" => Some(NamedKey::F6),
        "f7" => Some(NamedKey::F7),
        "f8" => Some(NamedKey::F8),
        "f9" => Some(NamedKey::F9),
        "f10" => Some(NamedKey::F10),
        "f11" => Some(NamedKey::F11),
        "f12" => Some(NamedKey::F12),
        _ => None,
    };
    if let Some(named) = named {
        return Ok(Key::Named(named));
    }

    let mut chars = lower.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(Key::Character(ch)),
        _ => Err(ParseChordError::UnknownKey(token.to_owned())),
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.control {
            write!(f, "Ctrl+")?;
        }
        if self.modifiers.alt {
            write!(f, "Alt+")?;
        }
        if self.modifiers.shift {
            write!(f, "Shift+")?;
        }
        if self.modifiers.meta {
            write!(f, "Cmd+")?;
        }
        match self.key {
            Key::Character(ch) => write!(f, "{}", ch.to_ascii_uppercase()),
            Key::Named(named) => write!(f, "{named}"),
        }
    }
}

/// [`KeyChord::parse`]'s own error type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseChordError {
    #[error("shortcut string is empty")]
    Empty,
    #[error("unknown modifier {0:?}")]
    UnknownModifier(String),
    #[error("unknown key {0:?}")]
    UnknownKey(String),
}

/// Maps [`KeyChord`]s to a caller-defined command id `T` — generic so
/// this crate stays free of any application's own command vocabulary
/// (`aurora-app` instantiates this with its own command enum).
#[derive(Debug, Clone)]
pub struct ShortcutRegistry<T> {
    bindings: HashMap<KeyChord, T>,
}

impl<T> ShortcutRegistry<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
        }
    }

    /// Binds `chord` to `command`.
    ///
    /// # Errors
    ///
    /// Returns [`ShortcutConflict`] (carrying the chord's existing
    /// command) if `chord` is already bound to something else — silently
    /// overwriting a shortcut would make the previous binding disappear
    /// with no signal, exactly the bug a caller registering a growing
    /// command list is likely to hit by accident. Nothing changes when
    /// this happens.
    pub fn bind(&mut self, chord: KeyChord, command: T) -> Result<(), ShortcutConflict<T>>
    where
        T: Clone,
    {
        if let Some(existing) = self.bindings.get(&chord) {
            return Err(ShortcutConflict {
                chord,
                existing: existing.clone(),
            });
        }
        self.bindings.insert(chord, command);
        Ok(())
    }

    #[must_use]
    pub fn resolve(&self, chord: KeyChord) -> Option<&T> {
        self.bindings.get(&chord)
    }

    /// Removes `chord`'s binding, if any, returning the command it used
    /// to map to.
    pub fn unbind(&mut self, chord: KeyChord) -> Option<T> {
        self.bindings.remove(&chord)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl<T> Default for ShortcutRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// [`ShortcutRegistry::bind`]'s own error: `chord` was already bound to
/// `existing`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{chord} is already bound")]
pub struct ShortcutConflict<T> {
    pub chord: KeyChord,
    pub existing: T,
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyChord, Modifiers, NamedKey, ParseChordError, ShortcutRegistry};

    fn chord(modifiers: Modifiers, key: Key) -> KeyChord {
        KeyChord::new(modifiers, key)
    }

    // -- KeyChord::parse --

    #[test]
    fn parse_a_plain_character_key() {
        assert_eq!(
            KeyChord::parse("p"),
            Ok(chord(Modifiers::none(), Key::Character('p')))
        );
    }

    #[test]
    fn parse_is_case_insensitive_for_both_modifiers_and_the_key() {
        assert_eq!(
            KeyChord::parse("ctrl+shift+P"),
            KeyChord::parse("Ctrl+Shift+p")
        );
    }

    #[test]
    fn parse_a_full_chord_with_every_modifier() {
        let parsed = match KeyChord::parse("Ctrl+Alt+Shift+Cmd+K") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(
            parsed,
            chord(
                Modifiers {
                    control: true,
                    shift: true,
                    alt: true,
                    meta: true,
                },
                Key::Character('k')
            )
        );
    }

    #[test]
    fn parse_recognises_alternate_modifier_spellings() {
        assert_eq!(KeyChord::parse("Control+A"), KeyChord::parse("Ctrl+A"));
        assert_eq!(KeyChord::parse("Option+A"), KeyChord::parse("Alt+A"));
        assert_eq!(KeyChord::parse("Command+A"), KeyChord::parse("Cmd+A"));
        assert_eq!(KeyChord::parse("Super+A"), KeyChord::parse("Cmd+A"));
        assert_eq!(KeyChord::parse("Windows+A"), KeyChord::parse("Cmd+A"));
    }

    #[test]
    fn parse_a_named_key() {
        assert_eq!(
            KeyChord::parse("Ctrl+Shift+P"),
            Ok(chord(
                Modifiers {
                    control: true,
                    shift: true,
                    alt: false,
                    meta: false,
                },
                Key::Character('p')
            ))
        );
        assert_eq!(
            KeyChord::parse("Escape"),
            Ok(chord(Modifiers::none(), Key::Named(NamedKey::Escape)))
        );
        assert_eq!(
            KeyChord::parse("F5"),
            Ok(chord(Modifiers::none(), Key::Named(NamedKey::F5)))
        );
    }

    #[test]
    fn parse_ignores_surrounding_whitespace_around_tokens() {
        assert_eq!(KeyChord::parse(" Ctrl + P "), KeyChord::parse("Ctrl+P"));
    }

    #[test]
    fn parse_rejects_an_empty_string() {
        assert_eq!(KeyChord::parse(""), Err(ParseChordError::Empty));
        assert_eq!(KeyChord::parse("   "), Err(ParseChordError::Empty));
    }

    #[test]
    fn parse_rejects_an_unknown_modifier() {
        match KeyChord::parse("Fn+P") {
            Err(ParseChordError::UnknownModifier(token)) => assert_eq!(token, "fn"),
            other => unreachable!("expected UnknownModifier, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_an_unknown_key() {
        match KeyChord::parse("Ctrl+Nonsense") {
            Err(ParseChordError::UnknownKey(token)) => assert_eq!(token, "Nonsense"),
            other => unreachable!("expected UnknownKey, got {other:?}"),
        }
    }

    // -- Display --

    #[test]
    fn display_renders_the_canonical_modifier_order_and_uppercase_character() {
        let parsed = match KeyChord::parse("shift+cmd+alt+ctrl+p") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(parsed.to_string(), "Ctrl+Alt+Shift+Cmd+P");
    }

    #[test]
    fn display_renders_a_named_key_with_no_modifiers() {
        let parsed = match KeyChord::parse("Escape") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(parsed.to_string(), "Esc");
    }

    // -- ShortcutRegistry --

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Command {
        Save,
        Quit,
    }

    #[test]
    fn fresh_registry_is_empty() {
        let registry: ShortcutRegistry<Command> = ShortcutRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn bind_then_resolve_finds_the_bound_command() {
        let mut registry = ShortcutRegistry::new();
        let save = match KeyChord::parse("Ctrl+S") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = registry.bind(save, Command::Save) {
            unreachable!("{err:?}");
        }
        assert_eq!(registry.resolve(save), Some(&Command::Save));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn resolve_returns_none_for_an_unbound_chord() {
        let registry: ShortcutRegistry<Command> = ShortcutRegistry::new();
        let unbound = match KeyChord::parse("Ctrl+S") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        assert_eq!(registry.resolve(unbound), None);
    }

    #[test]
    fn bind_rejects_a_chord_already_bound_to_something_else() {
        let mut registry = ShortcutRegistry::new();
        let chord = match KeyChord::parse("Ctrl+Q") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = registry.bind(chord, Command::Save) {
            unreachable!("{err:?}");
        }
        match registry.bind(chord, Command::Quit) {
            Err(conflict) => {
                assert_eq!(conflict.chord, chord);
                assert_eq!(conflict.existing, Command::Save);
            }
            Ok(()) => unreachable!("expected a conflict"),
        }
        // The original binding must survive an attempted overwrite.
        assert_eq!(registry.resolve(chord), Some(&Command::Save));
    }

    #[test]
    fn unbind_removes_a_binding_and_returns_the_old_command() {
        let mut registry = ShortcutRegistry::new();
        let chord = match KeyChord::parse("Ctrl+Q") {
            Ok(chord) => chord,
            Err(err) => unreachable!("{err:?}"),
        };
        if let Err(err) = registry.bind(chord, Command::Quit) {
            unreachable!("{err:?}");
        }
        assert_eq!(registry.unbind(chord), Some(Command::Quit));
        assert_eq!(registry.resolve(chord), None);
        assert!(registry.is_empty());
    }
}
