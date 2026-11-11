//! App-wide preferences, saved with the rest of Graft's state.

use graft_core::slash::default_protected;
use graft_core::worktree::DEFAULT_COPY_PATTERNS;
use serde::{Deserialize, Serialize};

use crate::fonts::{FontChoice, Fonts};
use crate::keys::Keymap;
use crate::sidebar_order::SidebarOrder;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Command that opens a folder or file in the editor, e.g. `code`.
    pub ide_command: String,
    /// Fetch every open repository this often; 0 = never.
    pub auto_fetch_minutes: u32,
    /// Branch names Slash never proposes (comma-separated, `*` matches anything).
    pub protected: String,
    /// Ignored files copied into new worktrees.
    pub copy_patterns: String,
    /// Ask GitHub for a newer Graft when it starts.
    pub check_updates: bool,
    /// Long lines in diffs wrap instead of scrolling sideways.
    pub wrap_lines: bool,
    /// Diffs show every line of a file, not just the changes with context.
    pub whole_file: bool,
    /// Review lists the files under their folders instead of flat.
    pub review_folders: bool,
    /// Keyboard shortcuts; only the ones changed from the defaults are saved.
    pub keys: Keymap,
    /// Font for everything but code; the built-in Inter by default.
    pub ui_font: FontChoice,
    /// Font for diffs, commit hashes and the console; JetBrains Mono by default.
    pub code_font: FontChoice,
    /// Zoom in percent, on top of the Windows display scale.
    pub zoom: u32,
    /// "Push to <remote>" in the New tag dialog, as last chosen.
    pub push_new_tags: bool,
    /// The order of WORKTREES, BRANCHES, REMOTES and TAGS in the sidebar.
    pub sidebar_order: SidebarOrder,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            ide_command: "code".to_owned(),
            auto_fetch_minutes: 10,
            protected: default_protected(),
            copy_patterns: DEFAULT_COPY_PATTERNS.to_owned(),
            check_updates: true,
            wrap_lines: false,
            whole_file: false,
            review_folders: false,
            keys: Keymap::default(),
            ui_font: FontChoice::default(),
            code_font: FontChoice::default(),
            zoom: 100,
            push_new_tags: false,
            sidebar_order: SidebarOrder::default(),
        }
    }
}

/// The zoom range, in percent.
pub const ZOOM_MIN: u32 = 75;
pub const ZOOM_MAX: u32 = 150;
/// The largest zoom as egui takes it.
pub fn max_zoom_factor() -> f32 {
    ZOOM_MAX as f32 / 100.0
}

/// How much one Zoom in or Zoom out changes it.
const ZOOM_STEP: u32 = 10;

impl Settings {
    pub fn fonts(&self) -> Fonts {
        Fonts {
            ui: self.ui_font.clone(),
            code: self.code_font.clone(),
        }
    }

    pub fn set_fonts(&mut self, fonts: Fonts) {
        self.ui_font = fonts.ui;
        self.code_font = fonts.code;
    }

    /// The zoom as egui takes it; a saved value out of range is clamped.
    pub fn zoom_factor(&self) -> f32 {
        self.zoom.clamp(ZOOM_MIN, ZOOM_MAX) as f32 / 100.0
    }

    /// One step bigger (`in_`) or smaller, to the next multiple of the step.
    pub fn zoom_step(&mut self, in_: bool) {
        let zoom = self.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let next = if in_ {
            (zoom / ZOOM_STEP + 1) * ZOOM_STEP
        } else {
            zoom.div_ceil(ZOOM_STEP).saturating_sub(1) * ZOOM_STEP
        };
        self.zoom = next.clamp(ZOOM_MIN, ZOOM_MAX);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the Windows app mode (Settings > Personalization > Colors).
    #[default]
    System,
    Dark,
    Light,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Match Windows",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Dark => egui::ThemePreference::Dark,
            Self::Light => egui::ThemePreference::Light,
        }
    }
}

/// Where clones go when the user has not picked a folder yet.
pub fn default_clone_parent() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|home| std::path::PathBuf::from(home).join("source").join("repos"))
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\dev"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stepped(zoom: u32, in_: bool) -> u32 {
        let mut settings = Settings {
            zoom,
            ..Settings::default()
        };
        settings.zoom_step(in_);
        settings.zoom
    }

    #[test]
    fn zoom_steps_land_on_multiples_of_ten_and_stay_in_range() {
        assert_eq!(stepped(100, true), 110);
        assert_eq!(stepped(100, false), 90);
        assert_eq!(stepped(105, true), 110);
        assert_eq!(stepped(105, false), 100);
        assert_eq!(stepped(ZOOM_MAX, true), ZOOM_MAX);
        assert_eq!(stepped(ZOOM_MIN, false), ZOOM_MIN);
        assert_eq!(stepped(999, false), 140);
        assert_eq!(stepped(80, false), ZOOM_MIN);
    }

    #[test]
    fn settings_saved_by_an_older_graft_get_the_new_defaults() {
        let old: Settings =
            serde_json::from_str(r#"{"theme":"Dark","ide_command":"code"}"#).unwrap();
        assert_eq!(old.zoom, 100);
        assert!(old.ui_font.is_built_in() && old.code_font.is_built_in());
        assert!(
            !old.push_new_tags,
            "the New tag dialog starts without pushing"
        );
        assert!(
            old.sidebar_order.is_default(),
            "the sidebar keeps its usual order"
        );
    }
}
