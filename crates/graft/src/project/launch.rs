//! Starting other programs from a project: a file's default program,
//! Explorer, a terminal, the IDE. Each start runs in a job (it looks at the
//! disk, and the IDE command is waited for); a failure becomes a notice.

use std::io;
use std::path::PathBuf;

use super::Project;
use crate::launch;

impl Project {
    /// Run `start` in the background; if it fails, say `failed` and why.
    fn launch(&self, failed: String, start: impl FnOnce() -> io::Result<()> + Send + 'static) {
        self.jobs.spawn_long(
            move |_| start(),
            move |p, result| {
                if let Err(e) = result {
                    p.notices.push(format!("{failed}: {e}"));
                }
            },
        );
    }

    /// Open a file of the selected worktree with its default program.
    pub(super) fn open_path(&mut self, path: &str) {
        let full = self.nav.worktree.join(path);
        let ide = self.settings.ide_command.clone();
        let failed = format!("Could not open {}", full.display());
        self.launch(failed, move || launch::open_path(&full, &ide));
    }

    /// Show a file of the selected worktree in Explorer.
    pub(super) fn reveal_path(&mut self, path: &str) {
        let full = self.nav.worktree.join(path);
        self.launch("Could not open Explorer".to_owned(), move || {
            launch::reveal(&full)
        });
    }

    pub(super) fn open_folder(&mut self, dir: PathBuf) {
        let failed = format!("Could not open {}", dir.display());
        self.launch(failed, move || launch::open_folder(&dir));
    }

    pub(super) fn open_terminal(&mut self, dir: PathBuf) {
        let failed = format!("Could not open a terminal in {}", dir.display());
        self.launch(failed, move || launch::open_terminal(&dir));
    }

    pub(super) fn open_in_ide(&mut self, path: PathBuf) {
        let ide = self.settings.ide_command.clone();
        let failed = format!("Could not open {}", path.display());
        self.launch(failed, move || launch::open_in_ide(&path, &ide));
    }
}
