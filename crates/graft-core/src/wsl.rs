//! Repositories inside WSL, opened as `\\wsl.localhost\<distro>\home\...`.
//!
//! Windows git reading such a folder is slow (seconds per `git status`), sees
//! Linux executable bits as changes and cannot follow worktrees, whose paths
//! git records as Linux paths. So git runs inside the distro through
//! `wsl.exe`, and paths are translated on the way in and out.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::paths::{native, no_window, wsl_parts};

/// A distro, as the user's path names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wsl {
    /// `\\wsl.localhost\Ubuntu` or `\\wsl$\Ubuntu`, kept as written so paths
    /// coming back from git compare equal to the ones Graft already has.
    prefix: String,
    distro: String,
}

impl Wsl {
    /// The distro `name`, reached through `\\wsl.localhost`.
    pub fn new(name: &str) -> Self {
        Self {
            prefix: format!(r"\\wsl.localhost\{name}"),
            distro: name.to_owned(),
        }
    }

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

    /// A folder the user typed for this distro: `~/x` (needs `home`), `/x`,
    /// or a Windows path, which is taken as is.
    pub fn resolve(&self, text: &str, home: Option<&str>) -> Option<PathBuf> {
        let text = text.trim();
        if text.contains('\\') || text.chars().nth(1) == Some(':') {
            return Some(native(text));
        }
        let linux = if text == "~" {
            home?.to_owned()
        } else if let Some(rest) = text.strip_prefix("~/") {
            format!("{}/{rest}", home?.trim_end_matches('/'))
        } else if text.starts_with('/') {
            text.trim_end_matches('/').to_owned()
        } else {
            return None;
        };
        Some(self.to_windows(&linux))
    }

    /// `path` the way a Linux user writes it: `~/x` under `home`, else `/x`.
    pub fn show(&self, path: &Path, home: Option<&str>) -> Option<String> {
        let linux = self.to_linux(path)?;
        if let Some(home) = home
            .map(|h| h.trim_end_matches('/'))
            .filter(|h| !h.is_empty())
        {
            if linux == home {
                return Some("~".to_owned());
            }
            if let Some(rest) = linux.strip_prefix(home).and_then(|r| r.strip_prefix('/')) {
                return Some(format!("~/{rest}"));
            }
        }
        Some(linux)
    }

    /// The home folder of the distro's default user (starts the distro).
    pub fn home(&self) -> Option<String> {
        let out = no_window(
            self.command(Path::new("/"), "sh")
                .args(["-c", "echo $HOME"]),
        )
        .output()
        .ok()?;
        let home = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && home.starts_with('/')).then_some(home)
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

/// Installed distros, the default one first, without Docker's and Rancher
/// Desktop's internal ones. Starts none of them. Empty without WSL.
pub fn distros() -> Vec<String> {
    let Ok(out) = no_window(Command::new("wsl.exe").args(["-l", "-v"])).output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse_list(&decode(&out.stdout))
}

/// wsl.exe answers in UTF-16 unless `WSL_UTF8` is set.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let zeros = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    if bytes.len() >= 2 && zeros * 4 >= bytes.len() {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `wsl -l -v`: a (translated) header, then `* name state version` lines.
fn parse_list(text: &str) -> Vec<String> {
    let mut list = Vec::new();
    for line in text.lines().skip(1) {
        let line = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        let (default, line) = match line.strip_prefix('*') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, line),
        };
        // The state is translated and can be several words ("Wird
        // ausgeführt"): the name comes first, the version last.
        let parts: Vec<&str> = line.split_whitespace().collect();
        let (3.., Some(name), Some(version)) = (parts.len(), parts.first(), parts.last()) else {
            continue;
        };
        if !version.chars().all(|c| c.is_ascii_digit())
            || name.starts_with("docker-desktop")
            || name.starts_with("rancher-desktop")
        {
            continue;
        }
        if default {
            list.insert(0, name.to_string());
        } else {
            list.push(name.to_string());
        }
    }
    list
}

/// A path into WSL as its distro and the Linux path `/rest`.
fn split(path: &Path) -> Option<(Wsl, String)> {
    let parts = wsl_parts(path)?;
    Some((
        Wsl {
            prefix: format!(r"\\{}\{}", parts.host, parts.distro),
            distro: parts.distro,
        },
        parts.linux,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_paths() {
        let wsl = Wsl::new("Ubuntu");
        let home = Some("/home/mdi");
        let unc = |s: &str| Some(PathBuf::from(s));
        assert_eq!(
            wsl.resolve("~/projects", home),
            unc(r"\\wsl.localhost\Ubuntu\home\mdi\projects")
        );
        assert_eq!(
            wsl.resolve(" ~ ", home),
            unc(r"\\wsl.localhost\Ubuntu\home\mdi")
        );
        assert_eq!(
            wsl.resolve("/srv/x/", home),
            unc(r"\\wsl.localhost\Ubuntu\srv\x")
        );
        assert_eq!(wsl.resolve("~/x", None), None, "home not known yet");
        assert_eq!(wsl.resolve("projects", home), None, "relative");
        assert_eq!(wsl.resolve(r"C:\dev", home), unc(r"C:\dev"));

        let p = Path::new(r"\\wsl.localhost\Ubuntu\home\mdi\projects\x");
        assert_eq!(wsl.show(p, home).unwrap(), "~/projects/x");
        let home_dir = Path::new(r"\\wsl.localhost\Ubuntu\home\mdi");
        assert_eq!(wsl.show(home_dir, home).unwrap(), "~");
        let other = Path::new(r"\\wsl.localhost\Ubuntu\home\mdix");
        assert_eq!(wsl.show(other, home).unwrap(), "/home/mdix");
        assert_eq!(crate::paths::display(p), "/home/mdi/projects/x (Ubuntu)");
        assert_eq!(crate::paths::display(Path::new(r"C:\dev")), r"C:\dev");
    }

    #[test]
    fn distro_list() {
        let text = "  NAME                   STATE           VERSION\r\n\
                    * Ubuntu-24.04           Running         2\r\n  \
                    rancher-desktop-data   Stopped         2\r\n  \
                    Debian                 Stopped         2\r\n";
        assert_eq!(parse_list(text), ["Ubuntu-24.04", "Debian"]);
        let german = "  NAME            STATUS            VERSION\r\n\
                      * Ubuntu          Wird ausgeführt   2\r\n  \
                      Debian          Beendet           2\r\n";
        assert_eq!(parse_list(german), ["Ubuntu", "Debian"]);
        let utf16: Vec<u8> = "NAME\r\n  Alpine Stopped 2\r\n* Arch Running 2\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(parse_list(&decode(&utf16)), ["Arch", "Alpine"]);
        let none = "Windows Subsystem for Linux has no installed distributions.\r\n\
                    Use 'wsl.exe --list --online' to list available distributions.";
        assert!(parse_list(none).is_empty());
    }

    #[test]
    fn real_distros() {
        // Whatever is installed here: never Docker's or Rancher's own.
        let list = distros();
        assert!(!list.iter().any(|d| d.starts_with("rancher-desktop")));
    }

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
