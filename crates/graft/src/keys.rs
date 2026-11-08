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
    AllTabs,
    Settings,
    Refresh,
    Console,
    WrapLines,
    WholeFile,
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

const fn shortcut(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const CTRL: Modifiers = Modifiers::COMMAND;
const CTRL_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

const fn ctrl(key: Key) -> Option<KeyboardShortcut> {
    Some(shortcut(CTRL, key))
}

const fn ctrl_shift(key: Key) -> Option<KeyboardShortcut> {
    Some(shortcut(CTRL_SHIFT, key))
}

const fn alt(key: Key) -> Option<KeyboardShortcut> {
    Some(shortcut(Modifiers::ALT, key))
}

const fn plain(key: Key) -> Option<KeyboardShortcut> {
    Some(shortcut(Modifiers::NONE, key))
}

/// A command and everything about its shortcut.
struct Spec {
    command: Command,
    /// Stable name in the settings file.
    id: &'static str,
    label: &'static str,
    scope: Scope,
    default: Option<KeyboardShortcut>,
    /// Extra keys that always work too and can't be changed.
    aliases: &'static [KeyboardShortcut],
}

const fn spec(
    command: Command,
    id: &'static str,
    label: &'static str,
    scope: Scope,
    default: Option<KeyboardShortcut>,
) -> Spec {
    Spec {
        command,
        id,
        label,
        scope,
        default,
        aliases: &[],
    }
}

const fn with_aliases(spec: Spec, aliases: &'static [KeyboardShortcut]) -> Spec {
    Spec { aliases, ..spec }
}

use Command as C;
use Scope::{FileStatus, General, Review};

/// In the order of [`Command`].
const COMMANDS: [Spec; 21] = [
    spec(C::Search, "search", "Search", General, ctrl(Key::K)),
    spec(
        C::NewTab,
        "new-tab",
        "Open a repository (new tab)",
        General,
        ctrl(Key::T),
    ),
    spec(
        C::OpenFolder,
        "open-folder",
        "Open a folder",
        General,
        ctrl(Key::O),
    ),
    spec(
        C::CloseTab,
        "close-tab",
        "Close the tab",
        General,
        ctrl(Key::W),
    ),
    with_aliases(
        spec(C::NextTab, "next-tab", "Next tab", General, ctrl(Key::Tab)),
        &[shortcut(CTRL, Key::PageDown)],
    ),
    with_aliases(
        spec(
            C::PreviousTab,
            "previous-tab",
            "Previous tab",
            General,
            ctrl_shift(Key::Tab),
        ),
        &[shortcut(CTRL, Key::PageUp)],
    ),
    spec(
        C::AllTabs,
        "all-tabs",
        "Find an open tab",
        General,
        ctrl_shift(Key::A),
    ),
    spec(
        C::Settings,
        "settings",
        "Settings",
        General,
        ctrl(Key::Comma),
    ),
    with_aliases(
        spec(C::Refresh, "refresh", "Refresh", General, plain(Key::F5)),
        &[shortcut(CTRL, Key::R)],
    ),
    with_aliases(
        spec(
            C::Console,
            "console",
            "Show or hide the git console",
            General,
            ctrl(Key::J),
        ),
        &[shortcut(CTRL, Key::Backtick)],
    ),
    spec(
        C::WrapLines,
        "wrap-lines",
        "Wrap long lines in diffs",
        General,
        alt(Key::Z),
    ),
    spec(
        C::WholeFile,
        "whole-file",
        "Show whole files in diffs",
        General,
        alt(Key::F),
    ),
    spec(
        C::StageFile,
        "stage-file",
        "Stage or unstage the selected file",
        FileStatus,
        plain(Key::Space),
    ),
    spec(
        C::StageAll,
        "stage-all",
        "Stage all",
        FileStatus,
        ctrl(Key::S),
    ),
    spec(
        C::UnstageAll,
        "unstage-all",
        "Unstage all",
        FileStatus,
        ctrl_shift(Key::S),
    ),
    spec(
        C::DiscardFile,
        "discard-file",
        "Discard the selected file's changes",
        FileStatus,
        plain(Key::Delete),
    ),
    spec(
        C::NextFile,
        "next-file",
        "Next file",
        FileStatus,
        plain(Key::ArrowDown),
    ),
    spec(
        C::PreviousFile,
        "previous-file",
        "Previous file",
        FileStatus,
        plain(Key::ArrowUp),
    ),
    spec(C::Commit, "commit", "Commit", FileStatus, ctrl(Key::Enter)),
    spec(
        C::MarkViewed,
        "mark-viewed",
        "Mark the file viewed",
        Review,
        plain(Key::V),
    ),
    spec(
        C::SaveComment,
        "save-comment",
        "Save a review comment",
        Review,
        ctrl(Key::Enter),
    ),
];

/// Ctrl+1 to Ctrl+9: the first nine tabs. Fixed, like the aliases.
const GO_TO_TAB: [(KeyboardShortcut, &str); 9] = [
    (shortcut(CTRL, Key::Num1), "Go to tab 1"),
    (shortcut(CTRL, Key::Num2), "Go to tab 2"),
    (shortcut(CTRL, Key::Num3), "Go to tab 3"),
    (shortcut(CTRL, Key::Num4), "Go to tab 4"),
    (shortcut(CTRL, Key::Num5), "Go to tab 5"),
    (shortcut(CTRL, Key::Num6), "Go to tab 6"),
    (shortcut(CTRL, Key::Num7), "Go to tab 7"),
    (shortcut(CTRL, Key::Num8), "Go to tab 8"),
    (shortcut(CTRL, Key::Num9), "Go to tab 9"),
];

impl Command {
    pub const ALL: [Command; COMMANDS.len()] = {
        let mut all = [Command::Search; COMMANDS.len()];
        let mut i = 0;
        while i < all.len() {
            all[i] = COMMANDS[i].command;
            i += 1;
        }
        all
    };

    fn spec(self) -> &'static Spec {
        &COMMANDS[self as usize]
    }

    pub fn id(self) -> &'static str {
        self.spec().id
    }

    pub fn label(self) -> &'static str {
        self.spec().label
    }

    pub fn scope(self) -> Scope {
        self.spec().scope
    }

    pub fn default_shortcut(self) -> Option<KeyboardShortcut> {
        self.spec().default
    }

    fn from_id(id: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.id() == id)
    }

    /// Used while typing, so a shortcut without Ctrl or Alt would only ever
    /// type text.
    fn while_typing(self) -> bool {
        matches!(self, Command::Commit | Command::SaveComment)
    }

    /// Holding the keys repeats only moving around, never an action.
    fn repeats(self) -> bool {
        matches!(
            self,
            Command::NextFile | Command::PreviousFile | Command::NextTab | Command::PreviousTab
        )
    }
}

/// Shortcuts that can't be changed, next to the configurable ones: the
/// aliases and Go to tab.
fn fixed() -> impl Iterator<Item = (KeyboardShortcut, &'static str)> {
    COMMANDS
        .iter()
        .flat_map(|spec| spec.aliases.iter().map(|alias| (*alias, spec.label)))
        .chain(GO_TO_TAB)
}

/// Nothing fires behind an open dialog.
fn behind_dialog(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.top_modal_layer()).is_some()
}

/// Whether plain keys (arrows, Escape, ...) may act on the screen: no dialog
/// is open and no widget, such as a text field, has the keyboard.
pub fn free(ctx: &egui::Context) -> bool {
    !behind_dialog(ctx) && !ctx.egui_wants_keyboard_input()
}

/// Was `key` pressed (or held) this frame while keys are [`free`]? For the
/// fixed keys that move around a list or close something.
pub fn plain_key(ctx: &egui::Context, key: Key) -> bool {
    free(ctx) && ctx.input(|i| i.key_pressed(key))
}

/// The tab whose Go to tab shortcut was pressed, among the first `count`.
pub fn go_to_tab(ctx: &egui::Context, count: usize) -> Option<usize> {
    if behind_dialog(ctx) {
        return None;
    }
    GO_TO_TAB
        .iter()
        .take(count)
        .position(|(shortcut, _)| ctx.input_mut(|i| consume_exact(i, *shortcut, false)))
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
        let mut keymap = Self { changed };
        // A default that came with an update must not take a key the user
        // already gave to another action: that action keeps it.
        for command in Command::ALL {
            let Some(default) = command.default_shortcut() else {
                continue;
            };
            let taken = Command::ALL.into_iter().any(|other| {
                other != command
                    && other.scope().overlaps(command.scope())
                    && keymap.changed.get(other.id()) == Some(&Some(default))
            });
            if taken && !keymap.changed.contains_key(command.id()) {
                keymap.changed.insert(command.id(), None);
            }
        }
        keymap
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

    /// The action, configurable or fixed, whose shortcut would clash with
    /// `shortcut` for `command`.
    pub fn conflict(&self, command: Command, shortcut: KeyboardShortcut) -> Option<&'static str> {
        let configurable = Command::ALL.into_iter().find(|other| {
            *other != command
                && other.scope().overlaps(command.scope())
                && self.get(*other) == Some(shortcut)
        });
        configurable.map(Command::label).or_else(|| {
            fixed()
                .find(|(fixed, _)| *fixed == shortcut)
                .map(|(_, label)| label)
        })
    }

    /// Why `shortcut` can't be `command`'s, if it can't.
    pub fn refusal(&self, command: Command, shortcut: KeyboardShortcut) -> Option<String> {
        let m = shortcut.modifiers;
        if command.while_typing() && !(m.command || m.ctrl || m.alt) {
            return Some(format!(
                "“{}” is used while typing, so its shortcut needs Ctrl or Alt.",
                command.label()
            ));
        }
        self.conflict(command, shortcut).map(|other| {
            format!(
                "{} is already used for “{other}”. Change that first.",
                format(shortcut)
            )
        })
    }

    /// Was `command`'s shortcut, or one of its aliases, pressed? Consumes
    /// the key press. Nothing fires behind an open dialog. Shortcuts without
    /// Ctrl or Alt (except F1, F2, …) never fire while a text field has the
    /// keyboard, and Space and Enter go to a button that has it.
    pub fn pressed(&self, ctx: &egui::Context, command: Command) -> bool {
        if behind_dialog(ctx) {
            return false;
        }
        let repeat = command.repeats();
        let alias = || {
            command
                .spec()
                .aliases
                .iter()
                .any(|alias| ctx.input_mut(|i| consume_exact(i, *alias, repeat)))
        };
        let Some(shortcut) = self.get(command) else {
            return alias();
        };
        let m = shortcut.modifiers;
        if !(m.command || m.ctrl || m.alt) {
            let key = shortcut.logical_key;
            let typing = ctx.text_edit_focused() && !is_function_key(key);
            let on_button =
                matches!(key, Key::Space | Key::Enter) && ctx.memory(|m| m.focused()).is_some();
            if typing || on_button {
                return alias();
            }
        }
        ctx.input_mut(|i| consume_exact(i, shortcut, repeat)) || alias()
    }

    /// The shortcut as the user reads it, e.g. "Ctrl+S".
    pub fn text(&self, command: Command) -> Option<String> {
        self.get(command).map(format)
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

/// F1 to F35.
fn is_function_key(key: Key) -> bool {
    let name = key.name();
    name.len() > 1 && name.starts_with('F') && name[1..].bytes().all(|b| b.is_ascii_digit())
}

/// Like egui's `consume_shortcut`, but Shift must match too: Ctrl+S must not
/// fire on Ctrl+Shift+S. Held keys count only if `repeat`.
fn consume_exact(input: &mut InputState, shortcut: KeyboardShortcut, repeat: bool) -> bool {
    let mut hit = false;
    input.events.retain(|event| {
        let matches = matches!(
            event,
            Event::Key { key, pressed: true, modifiers, repeat: held, .. }
                if *key == shortcut.logical_key
                    && modifiers.matches_exact(shortcut.modifiers)
                    && (repeat || !held)
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
        // A general shortcut clashes everywhere, fixed ones too.
        assert_eq!(
            keys.conflict(Command::StageAll, ctrl(Key::K).unwrap()),
            Some("Search")
        );
        assert_eq!(
            keys.conflict(Command::Commit, ctrl(Key::R).unwrap()),
            Some("Refresh")
        );
        assert_eq!(
            keys.conflict(Command::Search, ctrl(Key::Num1).unwrap()),
            Some("Go to tab 1")
        );
        for (shortcut, label) in fixed() {
            assert!(
                Command::ALL
                    .iter()
                    .all(|c| c.default_shortcut() != Some(shortcut)),
                "{label}"
            );
        }
    }

    #[test]
    fn the_table_is_in_command_order() {
        for (i, spec) in COMMANDS.iter().enumerate() {
            assert_eq!(spec.command as usize, i, "{}", spec.id);
        }
    }

    #[test]
    fn aliases_are_taken() {
        assert_eq!(
            Keymap::default().conflict(Command::Search, ctrl(Key::PageUp).unwrap()),
            Some("Previous tab")
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
            Some(Command::UnstageAll.label())
        );
    }

    #[test]
    fn shortcuts_that_could_never_fire_are_refused() {
        let keys = Keymap::default();
        let enter = plain(Key::Enter).unwrap();
        assert!(keys.refusal(Command::SaveComment, enter).is_some());
        assert!(
            keys.refusal(Command::Commit, plain(Key::C).unwrap())
                .is_some()
        );
        assert_eq!(keys.refusal(Command::Commit, ctrl(Key::M).unwrap()), None);
        assert_eq!(
            keys.refusal(Command::StageAll, plain(Key::A).unwrap()),
            None
        );
        assert_eq!(
            keys.refusal(Command::Refresh, ctrl(Key::S).unwrap())
                .unwrap(),
            "Ctrl+S is already used for “Stage all”. Change that first."
        );
    }

    #[test]
    fn held_keys_repeat_only_moving_around() {
        let repeating: Vec<Command> = Command::ALL.into_iter().filter(|c| c.repeats()).collect();
        assert_eq!(
            repeating,
            [
                Command::NextTab,
                Command::PreviousTab,
                Command::NextFile,
                Command::PreviousFile
            ]
        );

        let held = |key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: Modifiers::NONE,
        };
        let mut input = InputState::default();
        input.events = vec![held(Key::Space), held(Key::ArrowDown)];
        let consumed = |input: &mut InputState, command: Command| {
            consume_exact(
                input,
                command.default_shortcut().unwrap(),
                command.repeats(),
            )
        };
        assert!(!consumed(&mut input, Command::StageFile));
        assert!(consumed(&mut input, Command::NextFile));
        input.events = vec![Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        assert!(consumed(&mut input, Command::StageFile), "a fresh press");
    }

    #[test]
    fn function_keys_are_recognised() {
        assert!(is_function_key(Key::F5) && is_function_key(Key::F12));
        assert!(!is_function_key(Key::F) && !is_function_key(Key::Space));
    }

    #[test]
    fn a_new_default_leaves_a_key_the_user_took() {
        let saved = BTreeMap::from([("unstage-all".to_owned(), "Ctrl+Shift+A".to_owned())]);
        let keys = Keymap::from_saved(&saved);
        assert_eq!(keys.get(Command::UnstageAll), parse("Ctrl+Shift+A"));
        assert_eq!(keys.get(Command::AllTabs), None);
        assert_eq!(
            keys.get(Command::Search),
            Command::Search.default_shortcut()
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
