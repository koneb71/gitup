//! Keyboard bindings, and the rules that keep them from shadowing each other.
//!
//! egui matches modifiers *logically*: a binding for `⌘F` also fires on `⇧⌘F`,
//! because the pattern only requires the modifiers it names to be present. That
//! makes the order shortcuts are checked in load-bearing, and getting it wrong
//! is silent — the more specific chord simply never runs.
//!
//! Rather than rely on whoever edits the handler remembering that,
//! [`Keymap::triggered`] sorts by specificity and checks the most specific
//! first. Bindings can then be listed, remapped, and reordered freely.

use egui::{Key, Modifiers};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Something the user can bind a key to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    CommandPalette,
    OpenRepository,
    Settings,
    Refresh,
    Search,
    ToggleTheme,
    Fetch,
    Pull,
    Push,
    StageAll,
    Commit,
    DraftMessage,
    Stash,
    NewBranch,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::CommandPalette => "Command palette",
            Self::OpenRepository => "Open repository",
            Self::Settings => "Settings",
            Self::Refresh => "Refresh",
            Self::Search => "Search history",
            Self::ToggleTheme => "Toggle light/dark theme",
            Self::Fetch => "Fetch all remotes",
            Self::Pull => "Pull",
            Self::Push => "Push",
            Self::StageAll => "Stage all changes",
            Self::Commit => "Commit",
            Self::DraftMessage => "Draft a commit message",
            Self::Stash => "Stash changes",
            Self::NewBranch => "New branch",
        }
    }

    /// Whether the action only makes sense with a repository open.
    pub fn needs_repository(self) -> bool {
        !matches!(
            self,
            Self::OpenRepository | Self::Settings | Self::ToggleTheme
        )
    }

    /// Every action, in the order they are worth learning.
    pub fn all() -> [Self; 14] {
        [
            Self::CommandPalette,
            Self::OpenRepository,
            Self::Search,
            Self::Refresh,
            Self::Settings,
            Self::Commit,
            Self::StageAll,
            Self::DraftMessage,
            Self::Stash,
            Self::NewBranch,
            Self::Fetch,
            Self::Pull,
            Self::Push,
            Self::ToggleTheme,
        ]
    }
}

/// A key with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: Key,
}

impl Chord {
    pub const fn cmd(key: Key) -> Self {
        Self {
            command: true,
            shift: false,
            alt: false,
            key,
        }
    }

    pub const fn cmd_shift(key: Key) -> Self {
        Self {
            command: true,
            shift: true,
            alt: false,
            key,
        }
    }

    /// A key pressed on its own.
    pub const fn plain(key: Key) -> Self {
        Self {
            command: false,
            shift: false,
            alt: false,
            key,
        }
    }

    /// How many modifiers this chord requires.
    ///
    /// Used to order matching: the chord with more modifiers has to be tested
    /// first, or a less specific one swallows it.
    fn specificity(self) -> u8 {
        u8::from(self.command) + u8::from(self.shift) + u8::from(self.alt)
    }

    fn modifiers(self) -> Modifiers {
        let mut modifiers = Modifiers::NONE;
        if self.command {
            modifiers |= Modifiers::COMMAND;
        }
        if self.shift {
            modifiers |= Modifiers::SHIFT;
        }
        if self.alt {
            modifiers |= Modifiers::ALT;
        }
        modifiers
    }

    /// The form shown to the user, in this platform's convention.
    pub fn display(self) -> String {
        self.display_in(ChordStyle::native())
    }

    /// As [`display`](Self::display), for a named convention rather than the
    /// running platform's. Exists so both spellings can be tested anywhere.
    pub fn display_in(self, style: ChordStyle) -> String {
        match style {
            ChordStyle::Symbols => {
                let mut text = String::new();
                // Apple's documented order: control, option, shift, command.
                if self.alt {
                    text.push('⌥');
                }
                if self.shift {
                    text.push('⇧');
                }
                if self.command {
                    text.push('⌘');
                }
                text.push_str(key_symbol(self.key));
                text
            }
            ChordStyle::Words => {
                let mut parts: Vec<&str> = Vec::new();
                // `Modifiers::COMMAND` is Ctrl everywhere but macOS, so this
                // names the key the user will actually press, not the one the
                // binding is stored under.
                if self.command {
                    parts.push("Ctrl");
                }
                if self.alt {
                    parts.push("Alt");
                }
                if self.shift {
                    parts.push("Shift");
                }
                parts.push(key_word(self.key));
                parts.join("+")
            }
        }
    }

    /// The form stored in the settings file: lowercase, `+`-separated, stable.
    pub fn serialize(self) -> String {
        let mut parts = Vec::new();
        if self.alt {
            parts.push("alt".to_owned());
        }
        if self.shift {
            parts.push("shift".to_owned());
        }
        if self.command {
            parts.push("cmd".to_owned());
        }
        parts.push(self.key.name().to_owned());
        parts.join("+")
    }

    /// Parse the stored form. Unknown modifiers and keys make the whole chord
    /// invalid rather than silently binding something else.
    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Self {
            command: false,
            shift: false,
            alt: false,
            key: Key::A,
        };
        let mut key = None;

        for part in text.split('+') {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "" => continue,
                "cmd" | "command" | "ctrl" | "control" => chord.command = true,
                "shift" => chord.shift = true,
                "alt" | "option" | "opt" => chord.alt = true,
                _ => {
                    if key.is_some() {
                        // Two keys in one chord is not a chord.
                        return None;
                    }
                    key = Key::from_name(part);
                    key?;
                }
            }
        }

        chord.key = key?;
        Some(chord)
    }
}

/// How key chords are written down.
///
/// macOS spells them with the glyphs printed on the keys and no separator;
/// Windows and the Linux desktops spell them as words joined by `+`. Writing
/// `⌘O` on Windows would name a key that keyboard does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordStyle {
    /// `⇧⌘P`
    Symbols,
    /// `Ctrl+Shift+P`
    Words,
}

impl ChordStyle {
    /// The convention of the platform this build is running on.
    pub const fn native() -> Self {
        if cfg!(target_os = "macos") {
            Self::Symbols
        } else {
            Self::Words
        }
    }
}

/// A friendlier symbol than `Key::name` for the keys that have one.
fn key_symbol(key: Key) -> &'static str {
    match key {
        Key::Enter => "↵",
        Key::Escape => "⎋",
        Key::Backspace => "⌫",
        Key::Delete => "⌦",
        Key::Tab => "⇥",
        Key::Space => "Space",
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::Semicolon => ";",
        Key::Backslash => "\\",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        Key::Minus => "-",
        Key::Equals => "=",
        other => other.name(),
    }
}

/// The spelled-out name for keys whose glyph would be unfamiliar off macOS.
///
/// Punctuation is left alone: `,` reads the same everywhere.
fn key_word(key: Key) -> &'static str {
    match key {
        Key::Enter => "Enter",
        Key::Escape => "Esc",
        Key::Backspace => "Backspace",
        Key::Delete => "Del",
        Key::Tab => "Tab",
        Key::Space => "Space",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        other => key_symbol(other),
    }
}

impl Serialize for Chord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&Chord::serialize(*self))
    }
}

impl<'de> Deserialize<'de> for Chord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Chord::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not a key chord: {text:?}")))
    }
}

/// The set of bindings in force.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keymap {
    bindings: BTreeMap<Action, Chord>,
}

impl Default for Keymap {
    fn default() -> Self {
        use Action as A;
        let mut bindings = BTreeMap::new();
        bindings.insert(A::CommandPalette, Chord::cmd(Key::K));
        bindings.insert(A::OpenRepository, Chord::cmd(Key::O));
        bindings.insert(A::Settings, Chord::cmd(Key::Comma));
        bindings.insert(A::Refresh, Chord::cmd(Key::R));
        bindings.insert(A::Search, Chord::cmd(Key::F));
        bindings.insert(A::Commit, Chord::cmd(Key::Enter));
        bindings.insert(A::StageAll, Chord::cmd_shift(Key::A));
        bindings.insert(A::DraftMessage, Chord::cmd_shift(Key::G));
        bindings.insert(A::Stash, Chord::cmd_shift(Key::S));
        bindings.insert(A::NewBranch, Chord::cmd_shift(Key::B));
        bindings.insert(A::Fetch, Chord::cmd_shift(Key::F));
        bindings.insert(A::Pull, Chord::cmd_shift(Key::L));
        bindings.insert(A::Push, Chord::cmd_shift(Key::P));
        bindings.insert(A::ToggleTheme, Chord::cmd_shift(Key::T));
        Self { bindings }
    }
}

impl Keymap {
    pub fn chord(&self, action: Action) -> Option<Chord> {
        self.bindings.get(&action).copied()
    }

    /// Bind `action` to `chord`, replacing whatever it had.
    pub fn set(&mut self, action: Action, chord: Chord) {
        self.bindings.insert(action, chord);
    }

    /// Leave `action` unbound.
    pub fn clear(&mut self, action: Action) {
        self.bindings.remove(&action);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Actions sharing a chord with `action`.
    ///
    /// Only exact duplicates count. `⌘F` and `⇧⌘F` are not a conflict —
    /// specificity ordering resolves them — and reporting them as one would
    /// train people to ignore the warning.
    pub fn conflicting(&self, action: Action) -> Vec<Action> {
        let Some(chord) = self.chord(action) else {
            return Vec::new();
        };
        self.bindings
            .iter()
            .filter(|(other, other_chord)| **other != action && **other_chord == chord)
            .map(|(other, _)| *other)
            .collect()
    }

    /// True when any chord is bound twice.
    pub fn has_conflicts(&self) -> bool {
        Action::all()
            .into_iter()
            .any(|action| !self.conflicting(action).is_empty())
    }

    /// Which action the current input triggers, if any.
    ///
    /// Consumes the key, so this returns at most one action per frame and the
    /// key does not also reach a text field.
    pub fn triggered(&self, ctx: &egui::Context, repository_open: bool) -> Option<Action> {
        // Most modifiers first: see the module comment.
        let mut candidates: Vec<(Action, Chord)> = self
            .bindings
            .iter()
            .map(|(action, chord)| (*action, *chord))
            .filter(|(action, _)| repository_open || !action.needs_repository())
            .collect();
        candidates.sort_by(|a, b| {
            b.1.specificity()
                .cmp(&a.1.specificity())
                .then_with(|| a.0.cmp(&b.0))
        });

        for (action, chord) in candidates {
            if ctx.input_mut(|i| i.consume_key(chord.modifiers(), chord.key)) {
                return Some(action);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_round_trip_through_their_stored_form() {
        for chord in [
            Chord::cmd(Key::K),
            Chord::cmd_shift(Key::F),
            Chord {
                command: false,
                shift: false,
                alt: true,
                key: Key::Enter,
            },
            Chord {
                command: true,
                shift: true,
                alt: true,
                key: Key::Comma,
            },
        ] {
            let text = chord.serialize();
            assert_eq!(Chord::parse(&text), Some(chord), "round trip of {text}");
        }
    }

    #[test]
    fn stored_forms_are_readable_and_stable() {
        assert_eq!(Chord::cmd(Key::K).serialize(), "cmd+K");
        assert_eq!(Chord::cmd_shift(Key::F).serialize(), "shift+cmd+F");
    }

    #[test]
    fn parsing_accepts_the_names_people_type() {
        assert_eq!(Chord::parse("cmd+K"), Some(Chord::cmd(Key::K)));
        assert_eq!(Chord::parse("Command+k"), Some(Chord::cmd(Key::K)));
        assert_eq!(Chord::parse("ctrl+K"), Some(Chord::cmd(Key::K)));
        assert_eq!(Chord::parse("shift+cmd+f"), Some(Chord::cmd_shift(Key::F)));
    }

    #[test]
    fn nonsense_is_rejected_rather_than_guessed_at() {
        assert_eq!(Chord::parse(""), None);
        assert_eq!(Chord::parse("cmd"), None, "a chord needs a key");
        assert_eq!(Chord::parse("cmd+notakey"), None);
        assert_eq!(Chord::parse("cmd+A+B"), None, "two keys is not a chord");
    }

    #[test]
    fn display_uses_the_symbols_on_the_keys() {
        // Spelled out rather than left to `display()`, which follows whichever
        // platform the tests happen to be running on.
        let mac = |c: Chord| c.display_in(ChordStyle::Symbols);
        assert_eq!(mac(Chord::cmd(Key::K)), "⌘K");
        assert_eq!(mac(Chord::cmd_shift(Key::P)), "⇧⌘P");
        assert_eq!(mac(Chord::cmd(Key::Comma)), "⌘,");
        assert_eq!(mac(Chord::cmd(Key::Enter)), "⌘↵");
    }

    #[test]
    fn chords_are_spelled_out_away_from_a_mac() {
        // `Modifiers::COMMAND` is Ctrl on Windows and Linux, so that is the key
        // the label has to name — a keyboard there has no ⌘ on it.
        let pc = |c: Chord| c.display_in(ChordStyle::Words);
        assert_eq!(pc(Chord::cmd(Key::K)), "Ctrl+K");
        assert_eq!(pc(Chord::cmd_shift(Key::P)), "Ctrl+Shift+P");
        assert_eq!(pc(Chord::cmd(Key::Comma)), "Ctrl+,");
        assert_eq!(pc(Chord::cmd(Key::Enter)), "Ctrl+Enter");
        assert_eq!(pc(Chord::plain(Key::Escape)), "Esc");
        assert_eq!(pc(Chord::plain(Key::ArrowUp)), "Up");
        assert_eq!(
            pc(Chord {
                command: true,
                shift: true,
                alt: true,
                key: Key::D,
            }),
            "Ctrl+Alt+Shift+D"
        );
    }

    #[test]
    fn every_binding_has_a_label_on_both_conventions() {
        // A chord that rendered to nothing but modifiers would be a shortcut
        // the user cannot read, and it would only show up on one platform.
        let keymap = Keymap::default();
        for action in Action::all() {
            let Some(chord) = keymap.chord(action) else {
                continue;
            };
            for style in [ChordStyle::Symbols, ChordStyle::Words] {
                let text = chord.display_in(style);
                assert!(!text.is_empty(), "{action:?} has no label");
                assert!(
                    !text.ends_with('+'),
                    "{action:?} renders as {text:?}, which names no key"
                );
            }
        }
    }

    #[test]
    fn the_default_keymap_has_no_conflicts() {
        let keymap = Keymap::default();
        for action in Action::all() {
            assert!(
                keymap.conflicting(action).is_empty(),
                "{action:?} shares a chord with {:?}",
                keymap.conflicting(action)
            );
        }
        assert!(!keymap.has_conflicts());
    }

    #[test]
    fn every_action_is_bound_by_default() {
        let keymap = Keymap::default();
        for action in Action::all() {
            assert!(keymap.chord(action).is_some(), "{action:?} is unbound");
        }
    }

    #[test]
    fn duplicate_chords_are_reported_both_ways() {
        let mut keymap = Keymap::default();
        keymap.set(Action::Pull, Chord::cmd_shift(Key::T));
        assert_eq!(keymap.conflicting(Action::Pull), vec![Action::ToggleTheme]);
        assert_eq!(keymap.conflicting(Action::ToggleTheme), vec![Action::Pull]);
        assert!(keymap.has_conflicts());
    }

    #[test]
    fn a_more_specific_chord_is_not_a_conflict() {
        // ⌘F and ⇧⌘F coexist; specificity ordering keeps them apart.
        let keymap = Keymap::default();
        assert_eq!(keymap.chord(Action::Search), Some(Chord::cmd(Key::F)));
        assert_eq!(keymap.chord(Action::Fetch), Some(Chord::cmd_shift(Key::F)));
        assert!(keymap.conflicting(Action::Search).is_empty());
    }

    #[test]
    fn specificity_orders_shift_chords_ahead_of_plain_ones() {
        assert!(Chord::cmd_shift(Key::F).specificity() > Chord::cmd(Key::F).specificity());
    }

    #[test]
    fn clearing_and_resetting_work() {
        let mut keymap = Keymap::default();
        keymap.clear(Action::Push);
        assert_eq!(keymap.chord(Action::Push), None);
        keymap.reset();
        assert!(keymap.chord(Action::Push).is_some());
    }

    #[test]
    fn actions_that_need_no_repository_are_marked() {
        assert!(!Action::OpenRepository.needs_repository());
        assert!(!Action::Settings.needs_repository());
        assert!(Action::Push.needs_repository());
        assert!(Action::Commit.needs_repository());
    }

    #[test]
    fn a_keymap_survives_serialization() {
        let mut keymap = Keymap::default();
        keymap.set(Action::Push, Chord::cmd(Key::U));
        let text = toml::to_string(&keymap).expect("serialize");
        let restored: Keymap = toml::from_str(&text).expect("deserialize");
        assert_eq!(restored, keymap);
        assert_eq!(restored.chord(Action::Push), Some(Chord::cmd(Key::U)));
    }
}
