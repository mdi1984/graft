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

/// Whether files deleted under `path` can go to the Recycle Bin. Windows
/// has none inside WSL or on a network share: deleting there is for good.
pub fn has_recycle_bin(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    let unc = match path.components().next() {
        Some(Component::Prefix(prefix)) => {
            matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
        }
        _ => false,
    };
    !unc && wsl_parts(path).is_none()
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

/// `*` matches any run of characters, `?` one character; case-insensitive
/// like the Windows file system.
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The patterns in a list setting the user typed: separated by commas,
/// semicolons or line breaks.
pub(crate) fn pattern_list(text: &str) -> impl Iterator<Item = &str> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
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
    fn no_recycle_bin_in_wsl_or_on_shares() {
        assert!(has_recycle_bin(Path::new(r"C:\dev\app")));
        assert!(has_recycle_bin(Path::new(r"\\?\C:\dev\app")));
        assert!(!has_recycle_bin(Path::new(
            r"\\wsl.localhost\Ubuntu\home\app"
        )));
        assert!(!has_recycle_bin(Path::new("//wsl$/Ubuntu/home/app")));
        assert!(!has_recycle_bin(Path::new(r"\\server\share\app")));
        assert!(!has_recycle_bin(Path::new(r"\\?\UNC\server\share\app")));
    }

    #[test]
    fn globs() {
        assert!(glob_match(".env", ".env"));
        assert!(glob_match(".env.*", ".env.local"));
        assert!(!glob_match(".env.*", ".env"));
        assert!(glob_match("*.local.json", "appsettings.Local.json"));
        assert!(!glob_match("*.local", "notes.md"));
        assert!(glob_match("a?c*", "abcdef"));
    }

    #[test]
    fn pattern_lists() {
        let list: Vec<&str> = pattern_list("main, release/*;hotfix/*\n\n  dev ;").collect();
        assert_eq!(list, ["main", "release/*", "hotfix/*", "dev"]);
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
