//! Paths the Windows way: comparing, showing and naming folders, including
//! ones inside WSL (`\\wsl.localhost\<distro>\...`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A path git printed (forward slashes), the Windows way.
pub fn native(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

/// Compare paths the way Windows does: case-insensitive, either slash, no
/// trailing separator, `\\wsl$` and `\\wsl.localhost` alike.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| match wsl_parts(p) {
        Some(parts) => format!(r"\\wsl\{}{}", parts.distro, parts.linux)
            .trim_end_matches('/')
            .to_lowercase(),
        None => p
            .to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase(),
    };
    norm(a) == norm(b)
}

/// A path for showing: `/home/x/app (Ubuntu)` for one inside WSL.
pub fn display(path: &Path) -> String {
    match wsl_parts(path) {
        Some(parts) => format!("{} ({})", parts.linux, parts.distro),
        None => path.display().to_string(),
    }
}

/// The last part of `path`, e.g. a repository's or worktree's folder name;
/// the whole path (as shown) when it has none, like a drive root.
pub fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| display(path))
}

/// Whether `rel` under `root` passes through a symlink or junction (or is
/// one). Writing there could land outside the repository.
pub(crate) fn through_link(root: &Path, rel: &Path) -> bool {
    let mut at = root.to_path_buf();
    rel.components().any(|part| {
        at.push(part);
        std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// Start `cmd` without flashing a console window (Graft is a GUI app, so
/// every console program it starts would otherwise open one).
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// A path into WSL, taken apart.
pub(crate) struct WslParts {
    /// `wsl.localhost` or `wsl$`, as written.
    pub host: String,
    pub distro: String,
    /// The Linux path, `/rest`.
    pub linux: String,
}

/// `\\wsl.localhost\<distro>\rest` (or `\\wsl$\...`, either slash, with or
/// without `\\?\UNC\`) into its parts.
pub(crate) fn wsl_parts(path: &Path) -> Option<WslParts> {
    let text = path.to_string_lossy().replace('/', "\\");
    let unc = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        rest
    } else {
        text.strip_prefix(r"\\")?
    };
    let mut parts = unc.splitn(3, '\\');
    let host = parts.next()?;
    if !host.eq_ignore_ascii_case("wsl.localhost") && !host.eq_ignore_ascii_case("wsl$") {
        return None;
    }
    let distro = parts.next().filter(|d| !d.is_empty())?;
    let rest = parts.next().unwrap_or("").trim_end_matches('\\');
    Some(WslParts {
        host: host.to_owned(),
        distro: distro.to_owned(),
        linux: format!("/{}", rest.replace('\\', "/")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_wsl_names_are_the_same_folder() {
        assert!(same_path(
            Path::new(r"\\wsl$\Ubuntu\home\app"),
            Path::new(r"\\wsl.localhost\Ubuntu\home\app\")
        ));
        assert!(same_path(
            Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\home\app"),
            Path::new("//wsl.localhost/ubuntu/home/app")
        ));
        assert!(!same_path(
            Path::new(r"\\wsl$\Ubuntu\home\app"),
            Path::new(r"\\wsl.localhost\Debian\home\app")
        ));
        assert!(same_path(Path::new(r"C:\Dev\x\"), Path::new("c:/dev/x")));
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name(Path::new(r"C:\dev\graft")), "graft");
        assert_eq!(
            folder_name(Path::new(r"\\wsl.localhost\Ubuntu\home\app")),
            "app"
        );
    }
}
