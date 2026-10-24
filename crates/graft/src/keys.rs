//! Keyboard shortcuts: one list of the actions that have one, with their
//! default keys. Key handling, tooltips and the Settings page all read from
//! it, and the user's changes are saved in [`Settings::keys`].
//!
//! [`Settings::keys`]: crate::settings::Settings::keys

use std::collections::BTreeMap;

use egui::{Event, InputState, Key, KeyboardShortcut, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Search,
    NewTab,
    OpenFolder,
    CloseTab,
    NextTab,
    PreviousTab,
    Settings,
    Refresh,
    Console,
    StageFile,
    StageAll,
    UnstageAll,
    DiscardFile,
    NextFile,
    PreviousFile,
    Commit,
    MarkViewed,
    SaveComment,
}

/// Where a shortcut works. Shortcuts may repeat across File status and
/// Review, but not with a general one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    General,
    FileStatus,
    Review,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::General => "General",
            Scope::FileStatus => "File status",
            Scope::Review => "Review",
        }
    }

    fn overlaps(self, other: Scope) -> bool {
        self == other || self == Scope::General || other == Scope::General
    }
}

const fn ctrl(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::COMMAND, key))
}

const fn ctrl_shift(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(
        Modifiers {
            alt: false,
            ctrl: false,
            shift: true,
            mac_cmd: false,
            command: true,
        },
        key,
    ))
}

const fn plain(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::NONE, key))
}

impl Command {
    pub const ALL: [Command; 18] = [
        Command::Search,
        Command::NewTab,
        Command::OpenFolder,
        Command::CloseTab,
        Command::NextTab,
        Command::PreviousTab,
        Command::Settings,
        Command::Refresh,
        Command::Console,
        Command::StageFile,
        Command::StageAll,
        Command::UnstageAll,
        Command::DiscardFile,
        Command::NextFile,
        Command::PreviousFile,
        Command::Commit,
        Command::MarkViewed,
        Command::SaveComment,
    ];

    /// Stable name in the settings file.
    pub fn id(self) -> &'static str {
        match self {
            Command::Search => "search",
            Command::NewTab => "new-tab",
            Command::OpenFolder => "open-folder",
            Command::CloseTab => "close-tab",
            Command::NextTab => "next-tab",
            Command::PreviousTab => "previous-tab",
            Command::Settings => "settings",
            Command::Refresh => "refresh",
            Command::Console => "console",
            Command::StageFile => "stage-file",
            Command::StageAll => "stage-all",
            Command::UnstageAll => "unstage-all",
            Command::DiscardFile => "discard-file",
            Command::NextFile => "next-file",
            Command::PreviousFile => "previous-file",
            Command::Commit => "commit",
            Command::MarkViewed => "mark-viewed",
            Command::SaveComment => "save-comment",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Command::Search => "Search",
            Command::NewTab => "Open a repository (new tab)",
            Command::OpenFolder => "Open a folder",
            Command::CloseTab => "Close the tab",
            Command::NextTab => "Next tab",
            Command::PreviousTab => "Previous tab",
            Command::Settings => "Settings",
            Command::Refresh => "Refresh",
            Command::Console => "Show or hide the git console",
            Command::StageFile => "Stage or unstage the selected file",
            Command::StageAll => "Stage all",
            Command::UnstageAll => "Unstage all",
            Command::DiscardFile => "Discard the selected file's changes",
            Command::NextFile => "Next file",
            Command::PreviousFile => "Previous file",
            Command::Commit => "Commit",
            Command::MarkViewed => "Mark the file viewed",
            Command::SaveComment => "Save a review comment",
        }
    }

    pub fn scope(self) -> Scope {
        match self {
            Command::StageFile
            | Command::StageAll
            | Command::UnstageAll
            | Command::DiscardFile
            | Command::NextFile
            | Command::PreviousFile
            | Command::Commit => Scope::FileStatus,
            Command::MarkViewed | Command::SaveComment => Scope::Review,
            _ => Scope::General,
        }
    }

    pub fn default_shortcut(self) -> Option<KeyboardShortcut> {
        match self {
            Command::Search => ctrl(Key::K),
            Command::NewTab => ctrl(Key::T),
            Command::OpenFolder => ctrl(Key::O),
            Command::CloseTab => ctrl(Key::W),
            Command::NextTab => ctrl(Key::Tab),
            Command::PreviousTab => ctrl_shift(Key::Tab),
            Command::Settings => ctrl(Key::Comma),
            Command::Refresh => plain(Key::F5),
            Command::Console => ctrl(Key::J),
            Command::StageFile => plain(Key::Space),
            Command::StageAll => ctrl(Key::S),
            Command::UnstageAll => ctrl_shift(Key::S),
            Command::DiscardFile => plain(Key::Delete),
            Command::NextFile => plain(Key::ArrowDown),
            Command::PreviousFile => plain(Key::ArrowUp),
            Command::Commit => ctrl(Key::Enter),
            Command::MarkViewed => plain(Key::V),
            Command::SaveComment => ctrl(Key::Enter),
        }
    }

    fn from_id(id: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// The shortcuts in use: the defaults with the user's changes on top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keymap {
    changed: BTreeMap<&'static str, Option<KeyboardShortcut>>,
}

impl Keymap {
    /// From the saved changes: action id → shortcut text ("" = none).
    /// Unknown actions and unreadable shortcuts are skipped.
    pub fn from_saved(saved: &BTreeMap<String, String>) -> Self {
        let changed = saved
            .iter()
            .filter_map(|(id, text)| {
                let command = Command::from_id(id)?;
                let shortcut = if text.trim().is_empty() {
                    None
                } else {
                    Some(parse(text)?)
                };
                Some((command.id(), shortcut))
            })
            .collect();
        Self { changed }
    }

    /// What to save: only the shortcuts that differ from the defaults, so
    /// new defaults still reach the user.
    pub fn to_saved(&self) -> BTreeMap<String, String> {
        Command::ALL
            .into_iter()
            .filter(|c| self.get(*c) != c.default_shortcut())
            .map(|c| {
                (
                    c.id().to_owned(),
                    self.get(c).map(format).unwrap_or_default(),
                )
            })
            .collect()
    }

    pub fn get(&self, command: Command) -> Option<KeyboardShortcut> {
        match self.changed.get(command.id()) {
            Some(shortcut) => *shortcut,
            None => command.default_shortcut(),
        }
    }

    pub fn set(&mut self, command: Command, shortcut: Option<KeyboardShortcut>) {
        if shortcut == command.default_shortcut() {
            self.changed.remove(command.id());
        } else {
            self.changed.insert(command.id(), shortcut);
        }
    }

    /// Another action whose shortcut would clash with `shortcut` for `command`.
    pub fn conflict(&self, command: Command, shortcut: KeyboardShortcut) -> Option<Command> {
        Command::ALL.into_iter().find(|other| {
            *other != command
                && other.scope().overlaps(command.scope())
                && self.get(*other) == Some(shortcut)
        })
    }

    /// Was `command`'s shortcut pressed? Consumes the key press. Nothing
    /// fires behind an open dialog, and shortcuts without Ctrl or Alt never
    /// fire while a text field has the keyboard.
    pub fn pressed(&self, ctx: &egui::Context, command: Command) -> bool {
        let Some(shortcut) = self.get(command) else {
            return false;
        };
        if ctx.memory(|m| m.top_modal_layer()).is_some() {
            return false;
        }
        let m = shortcut.modifiers;
        if !(m.command || m.ctrl || m.alt) && ctx.text_edit_focused() {
            return false;
        }
        ctx.input_mut(|i| consume_exact(i, shortcut))
    }

    /// `text` with the shortcut, for a tooltip: "Stage all  (Ctrl+S)".
    pub fn tip(&self, text: &str, command: Command) -> String {
        match self.get(command) {
            Some(shortcut) => format!("{text}  ({})", format(shortcut)),
            None => text.to_owned(),
        }
    }
}

impl serde::Serialize for Keymap {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_saved().serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Keymap {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_saved(&BTreeMap::deserialize(deserializer)?))
    }
}

/// Like egui's `consume_shortcut`, but Shift must match too: Ctrl+S must not
/// fire on Ctrl+Shift+S.
fn consume_exact(input: &mut InputState, shortcut: KeyboardShortcut) -> bool {
    let mut hit = false;
    input.events.retain(|event| {
        let matches = matches!(
            event,
            Event::Key { key, pressed: true, modifiers, .. }
                if *key == shortcut.logical_key && modifiers.matches_exact(shortcut.modifiers)
        );
        if matches && !hit {
            hit = true;
            return false;
        }
        true
    });
    hit
}

/// "Ctrl+Shift+S", "Space", "F5".
pub fn format(shortcut: KeyboardShortcut) -> String {
    let m = shortcut.modifiers;
    let mut parts = Vec::new();
    if m.command || m.ctrl {
        parts.push("Ctrl");
    }
    if m.alt {
        parts.push("Alt");
    }
    if m.shift {
        parts.push("Shift");
    }
    let key = match shortcut.logical_key {
        Key::Comma => ",",
        Key::Backtick => "`",
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        other => other.name(),
    };
    parts.push(key);
    parts.join("+")
}

/// The reverse of [`format`].
pub fn parse(text: &str) -> Option<KeyboardShortcut> {
    let mut modifiers = Modifiers::NONE;
    let (mods, key) = match text.trim().rsplit_once('+') {
        // "Ctrl++" would be Ctrl and the plus key.
        Some((mods, "")) => (mods.strip_suffix('+')?, "+"),
        Some((mods, key)) => (mods, key),
        None => ("", text.trim()),
    };
    for part in mods.split('+').filter(|p| !p.is_empty()) {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= Modifiers::COMMAND,
            "alt" => modifiers |= Modifiers::ALT,
            "shift" => modifiers |= Modifiers::SHIFT,
            _ => return None,
        }
    }
    let key = Key::from_name(key.trim())?;
    Some(KeyboardShortcut::new(modifiers, key))
}

/// The shortcut pressed this frame, for recording a new one in Settings:
/// the first key press that isn't a lone modifier.
pub fn recorded(input: &InputState) -> Option<KeyboardShortcut> {
    input.events.iter().find_map(|event| match event {
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            let mut modifiers = *modifiers;
            // Windows reports Ctrl as both; keep one spelling.
            modifiers.command |= modifiers.ctrl;
            modifiers.ctrl = false;
            modifiers.mac_cmd = false;
            Some(KeyboardShortcut::new(modifiers, *key))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_read_and_write_alike() {
        for command in Command::ALL {
            let shortcut = command.default_shortcut().unwrap();
            assert_eq!(parse(&format(shortcut)), Some(shortcut), "{command:?}");
        }
        assert_eq!(format(ctrl_shift(Key::S).unwrap()), "Ctrl+Shift+S");
        assert_eq!(format(ctrl(Key::Comma).unwrap()), "Ctrl+,");
        assert_eq!(parse("ctrl + shift + s"), ctrl_shift(Key::S));
        assert_eq!(parse("Hyper+S"), None);
    }

    #[test]
    fn only_changes_are_saved() {
        let mut keys = Keymap::default();
        assert!(keys.to_saved().is_empty());
        keys.set(Command::StageAll, plain(Key::A));
        keys.set(Command::Refresh, None);
        let saved = keys.to_saved();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved["stage-all"], "A");
        assert_eq!(saved["refresh"], "");
        let back = Keymap::from_saved(&saved);
        assert_eq!(back, keys);
        assert_eq!(back.get(Command::Refresh), None);
        // Setting a default again forgets the change.
        keys.set(Command::StageAll, Command::StageAll.default_shortcut());
        assert_eq!(keys.to_saved().len(), 1);
    }

    #[test]
    fn defaults_do_not_clash() {
        let keys = Keymap::default();
        for command in Command::ALL {
            let shortcut = command.default_shortcut().unwrap();
            assert_eq!(keys.conflict(command, shortcut), None, "{command:?}");
        }
        // Commit and saving a comment share Ctrl+Enter in different places.
        assert_eq!(keys.get(Command::Commit), keys.get(Command::SaveComment));
        // A general shortcut clashes everywhere.
        assert_eq!(
            keys.conflict(Command::StageAll, ctrl(Key::K).unwrap()),
            Some(Command::Search)
        );
    }

    #[test]
    fn a_recorded_ctrl_shortcut_matches_the_default() {
        let mut input = InputState::default();
        // What Windows sends for Ctrl+Shift+S: Ctrl as both ctrl and command.
        input.events.push(Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl: true,
                command: true,
                shift: true,
                ..Modifiers::NONE
            },
        });
        let shortcut = recorded(&input).unwrap();
        assert_eq!(Some(shortcut), Command::UnstageAll.default_shortcut());
        assert_eq!(format(shortcut), "Ctrl+Shift+S");
        let keys = Keymap::default();
        assert_eq!(
            keys.conflict(Command::StageAll, shortcut),
            Some(Command::UnstageAll)
        );
    }

    #[test]
    fn unknown_saved_entries_are_skipped() {
        let saved = BTreeMap::from([
            ("no-such-action".to_owned(), "Ctrl+X".to_owned()),
            ("stage-all".to_owned(), "Ctrl+Nope".to_owned()),
        ]);
        assert_eq!(Keymap::from_saved(&saved), Keymap::default());
    }
}
