use egui::{CentralPanel, Frame, Margin, Panel};
use graft_core::RepoSnapshot;

use crate::theme::color;
use crate::{fonts, theme, ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Workspace,
    Review,
    Stashes,
}

/// What the Workspace tab shows in its main area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceView {
    FileStatus,
    History,
}

/// Where the user is. The toolbar, tabs and sidebar change it; the main area reads it.
#[derive(Clone, Debug)]
pub struct Nav {
    pub tab: Tab,
    pub view: WorkspaceView,
    /// Index into [`RepoSnapshot::worktrees`].
    pub worktree: usize,
}

impl Nav {
    /// A dirty worktree opens File status, a clean one opens History.
    pub fn open_worktree(&mut self, index: usize, dirty: bool) {
        self.worktree = index;
        self.tab = Tab::Workspace;
        self.view = if dirty {
            WorkspaceView::FileStatus
        } else {
            WorkspaceView::History
        };
    }

    pub fn open_file_status(&mut self) {
        self.tab = Tab::Workspace;
        self.view = WorkspaceView::FileStatus;
    }

    pub fn open_history(&mut self) {
        self.tab = Tab::Workspace;
        self.view = WorkspaceView::History;
    }
}

pub struct GraftApp {
    repo: RepoSnapshot,
    nav: Nav,
    search: String,
    window_title: String,
}

impl GraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        fonts::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);

        // Until step 2 can open a repository, the shell shows sample data.
        let repo = graft_core::demo::snapshot();
        let mut nav = Nav {
            tab: Tab::Workspace,
            view: WorkspaceView::History,
            worktree: 0,
        };
        if let Some(index) = repo.worktrees.iter().position(|wt| wt.is_main) {
            nav.open_worktree(index, repo.worktrees[index].dirty > 0);
        }

        Self {
            repo,
            nav,
            search: String::new(),
            window_title: String::new(),
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.repo.worktrees.get(self.nav.worktree) {
            Some(wt) => format!("{} — {}", self.repo.name, wt.path.display()),
            None => "Graft".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

impl eframe::App for GraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.sync_window_title(ui.ctx());

        let bar = |margin| Frame::new().fill(color::BAR_BG).inner_margin(margin);

        Panel::top("toolbar")
            .frame(bar(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui::toolbar::show(ui, &self.repo, &mut self.nav, &mut self.search)
            });

        Panel::top("tabs")
            .frame(bar(Margin::symmetric(12, 0)))
            .show(ui, |ui| ui::tabs::show(ui, &self.repo, &mut self.nav));

        Panel::left("sidebar")
            .resizable(true)
            .default_size(250.0)
            .size_range(200.0..=420.0)
            .frame(
                Frame::new()
                    .fill(color::SIDEBAR_BG)
                    .inner_margin(Margin::symmetric(8, 12)),
            )
            .show(ui, |ui| ui::sidebar::show(ui, &self.repo, &mut self.nav));

        CentralPanel::no_frame()
            .frame(Frame::new().fill(color::APP_BG))
            .show(ui, |ui| ui::main_area::show(ui, &self.repo, &self.nav));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        color::APP_BG.to_normalized_gamma_f32()
    }
}
