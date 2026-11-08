//! The screens. They read the app's state and report what the user did as
//! [`Actions`]. The only state they change themselves is what is typed into
//! forms and dialogs, and egui's memory (scroll positions, open menus).

pub mod about;
mod branch_menu;
mod can;
pub mod confirm;
pub mod conflicts;
pub mod console;
pub mod dialog;
pub mod diff_view;
pub mod file_history;
mod file_row;
pub mod file_status;
pub mod group_dialog;
pub mod history;
pub mod icons;
pub mod main_area;
pub mod new_worktree;
pub mod project;
pub mod project_tabs;
pub mod review;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod slash;
pub mod start;
pub mod stashes;
pub mod tabs;
pub mod toasts;
pub mod toolbar;
pub mod widgets;
pub mod window_frame;

use crate::project::Action;

/// Something the user did that the app handles, not the shown project.
#[derive(Clone, Debug)]
pub enum AppAction {
    /// Close the shown project's tab.
    Close,
    /// Switch to another open project and apply `actions` there (a search
    /// hit in another project).
    InProject {
        project: usize,
        actions: Vec<Action>,
    },
    /// Show whole files in diffs, or only the changes (an app setting).
    ToggleWholeFile,
}

/// What the user did this frame.
#[derive(Default)]
pub struct Actions {
    pub project: Vec<Action>,
    pub app: Vec<AppAction>,
}

impl Actions {
    pub fn push(&mut self, action: Action) {
        self.project.push(action);
    }

    pub fn app(&mut self, action: AppAction) {
        self.app.push(action);
    }

    pub fn extend(&mut self, actions: impl IntoIterator<Item = Action>) {
        self.project.extend(actions);
    }
}
