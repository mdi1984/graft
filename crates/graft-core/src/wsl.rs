//! Repositories inside WSL, opened as `\\wsl.localhost\<distro>\home\...`.
//!
//! Windows git reading such a folder is slow (seconds per `git status`), sees
//! Linux executable bits as changes and cannot follow worktrees, whose paths
//! git records as Linux paths. So git runs inside the distro through
//! `wsl.exe`, and paths are translated on the way in and out.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A distro, as the user's path names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wsl {
    /// `\\wsl.localhost\Ubuntu` or `\\wsl$\Ubuntu`, kept as written so paths
    /// coming back from git compare equal to the ones Graft already has.
    prefix: String,
    distro: String,
}

impl Wsl {
    /// The distro holding `path`, if it is a path into WSL.
    pub fn of(path: &Path) -> Option<Self> {
        split(path).map(|(wsl, _)| wsl)
    }

    pub fn distro(&self) -> &str {
        &self.distro
    }

    /// The Linux path for a Windows path: a path into this distro, or a
    /// drive path (`C:\x` is `/mnt/c/x`). `None` for anything else.
    pub fn to_linux(&self, path: &Path) -> Option<String> {
        if let Some((wsl, linux)) = split(path) {
            return wsl
                .distro
                .eq_ignore_ascii_case(&self.distro)
                .then_some(linux);
        }
        let text = path.to_string_lossy().replace('/', "\\");
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        let mut chars = text.chars();
        let (Some(drive), Some(':')) = (chars.next(), chars.next()) else {
            return None;
        };
        if !drive.is_ascii_alphabetic() {
            return None;
        }
        let rest = chars.as_str().trim_start_matches('\\').replace('\\', "/");
        let drive = drive.to_ascii_lowercase();
        Some(if rest.is_empty() {
            format!("/mnt/{drive}")
        } else {
            format!("/mnt/{drive}/{rest}")
        })
    }

    /// The Windows path for a Linux path git printed.
    pub fn to_windows(&self, linux: &str) -> PathBuf {
        // `/mnt/c/x` is the Windows drive, not a folder inside the distro.
        if let Some(rest) = linux.strip_prefix("/mnt/") {
            let mut chars = rest.chars();
            if let Some(drive) = chars.next().filter(char::is_ascii_alphabetic) {
                let after = chars.as_str();
                if after.is_empty() || after.starts_with('/') {
                    let tail = after.trim_start_matches('/').replace('/', "\\");
                    return PathBuf::from(format!("{}:\\{tail}", drive.to_ascii_uppercase()));
                }
            }
        }
        let tail = linux.trim_start_matches('/').replace('/', "\\");
        if tail.is_empty() {
            PathBuf::from(format!("{}\\", self.prefix))
        } else {
            PathBuf::from(format!("{}\\{tail}", self.prefix))
        }
    }

    /// `program` run inside the distro in the Linux folder for `cwd`.
    /// Environment variables reach it only when named in `WSLENV`.
    pub(crate) fn command(&self, cwd: &Path, program: &str) -> Command {
        let mut cmd = Command::new("wsl.exe");
        cmd.args(["-d", &self.distro]);
        if let Some(dir) = self.to_linux(cwd) {
            cmd.args(["--cd", &dir]);
        }
        cmd.args(["--exec", program]);
        cmd
    }
}

/// `\\wsl.localhost\<distro>\rest` (or `\\wsl$\...`, either slash, with or
/// without `\\?\UNC\`) into the distro and the Linux path `/rest`.
fn split(path: &Path) -> Option<(Wsl, String)> {
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
    let linux = format!("/{}", rest.replace('\\', "/"));
    Some((
        Wsl {
            prefix: format!(r"\\{host}\{distro}"),
            distro: distro.to_owned(),
        },
        linux,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wsl_paths() {
        let wsl = Wsl::of(Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\mdi\app")).unwrap();
        assert_eq!(wsl.distro(), "Ubuntu-24.04");
        assert!(Wsl::of(Path::new(r"\\wsl$\Debian\srv")).is_some());
        assert!(Wsl::of(Path::new("//wsl.localhost/Ubuntu/home")).is_some());
        assert!(Wsl::of(Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\home")).is_some());
        assert!(Wsl::of(Path::new(r"C:\dev\graft")).is_none());
        assert!(Wsl::of(Path::new(r"\\server\share\repo")).is_none());
        assert!(Wsl::of(Path::new(r"\\wsl.localhost\")).is_none());
    }

    #[test]
    fn paths_round_trip() {
        let wsl = Wsl::of(Path::new(r"\\wsl.localhost\Ubuntu\home")).unwrap();
        let unc = Path::new(r"\\wsl.localhost\Ubuntu\home\mdi\app");
        assert_eq!(wsl.to_linux(unc).unwrap(), "/home/mdi/app");
        assert_eq!(wsl.to_windows("/home/mdi/app"), unc);
        // Trailing separators and the distro's case do not matter.
        let other = Path::new(r"\\WSL.LOCALHOST\ubuntu\home\mdi\app\");
        assert_eq!(wsl.to_linux(other).unwrap(), "/home/mdi/app");
        assert_eq!(wsl.to_windows("/"), Path::new(r"\\wsl.localhost\Ubuntu\"));
    }

    #[test]
    fn answers_keep_the_prefix_the_user_opened() {
        let wsl = Wsl::of(Path::new(r"\\wsl$\Ubuntu\home")).unwrap();
        assert_eq!(wsl.to_windows("/srv/x"), Path::new(r"\\wsl$\Ubuntu\srv\x"));
    }

    #[test]
    fn windows_drives_are_mounted() {
        let wsl = Wsl::of(Path::new(r"\\wsl.localhost\Ubuntu\home")).unwrap();
        assert_eq!(
            wsl.to_linux(Path::new(r"C:\dev\x")).unwrap(),
            "/mnt/c/dev/x"
        );
        assert_eq!(wsl.to_linux(Path::new("D:\\")).unwrap(), "/mnt/d");
        assert_eq!(wsl.to_windows("/mnt/c/dev/x"), Path::new(r"C:\dev\x"));
        assert_eq!(wsl.to_windows("/mnt/d"), Path::new(r"D:\"));
        // `/mnt/wsl` is a folder inside the distro, not a drive.
        assert_eq!(
            wsl.to_windows("/mnt/wsl/x"),
            Path::new(r"\\wsl.localhost\Ubuntu\mnt\wsl\x")
        );
        // Another distro's files are not this one's.
        assert!(
            wsl.to_linux(Path::new(r"\\wsl.localhost\Debian\home"))
                .is_none()
        );
        assert!(wsl.to_linux(Path::new(r"\\server\share")).is_none());
    }
}
