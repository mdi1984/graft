//! Starting other programs: the IDE, a terminal, Explorer, a file's default
//! program. Paths and names come from repositories Graft does not control, so
//! nothing here goes through `cmd /c` (which would read `&` or `%` in a path as
//! commands), and a file Windows would run (`.exe`, `.bat`, `.lnk`, ...) is
//! never handed to its default program.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Extensions that Windows runs, or follows to something it runs, instead of
/// opening them as a document. `PATHEXT` adds to these.
const RUNNABLE: &[&str] = &[
    "exe",
    "com",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "msi",
    "msp",
    "msix",
    "appx",
    "appinstaller",
    "scr",
    "pif",
    "cpl",
    "lnk",
    "url",
    "reg",
    "jar",
    "application",
    "appref-ms",
    "scf",
    "chm",
    "msc",
    "inf",
    "settingcontent-ms",
    "diagcab",
    "library-ms",
    "search-ms",
];

/// Whether opening `path` with its default program would run something.
pub fn is_runnable(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    RUNNABLE.iter().any(|r| r.eq_ignore_ascii_case(ext))
        || path_exts()
            .iter()
            .any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(ext))
}

fn path_exts() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Open a file with its default program, or a folder in Explorer. A file that
/// would run goes to the IDE instead (or is shown in Explorer).
pub fn open_path(path: &Path, ide_command: &str) -> std::io::Result<()> {
    if path.is_file() && is_runnable(path) {
        open_in_ide(path, ide_command);
        return Ok(());
    }
    open::that_detached(path)
}

/// Open a folder in Explorer; anything that is not a folder is refused.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not a folder",
        ));
    }
    open::that_detached(dir)
}

/// Show `path` selected in an Explorer window.
pub fn reveal(path: &Path) -> std::io::Result<()> {
    let full = graft_core::repo::native(&path.to_string_lossy());
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(&full);
    Command::new("explorer").arg(arg).spawn().map(drop)
}

/// Open a terminal in `dir`: Windows Terminal if installed, else cmd.
pub fn open_terminal(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    // Windows Terminal reads `;` as "new tab"; `\;` is a plain semicolon.
    let escaped = dir.to_string_lossy().replace(';', "\\;");
    if Command::new("wt.exe")
        .arg("-d")
        .arg(escaped)
        .spawn()
        .is_err()
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.current_dir(dir);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
            cmd.creation_flags(CREATE_NEW_CONSOLE);
        }
        let _ = cmd.spawn();
    }
}

/// Open a folder or file with the IDE command from the settings (`code`,
/// `idea64`, a full path, with optional arguments). Without a working IDE a
/// folder opens in Explorer, a document with its default program, and a file
/// that would run is only shown in Explorer.
pub fn open_in_ide(path: &Path, command: &str) {
    let path = path.to_path_buf();
    let words = split_command(command);
    std::thread::spawn(move || {
        let ok = words.split_first().is_some_and(|(program, args)| {
            let Some(program) = find_program(program) else {
                return false;
            };
            let mut cmd = Command::new(program);
            cmd.args(args).arg(&path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            // A .cmd launcher (code.cmd) goes through cmd.exe; the standard
            // library escapes its arguments and refuses ones it cannot.
            cmd.status().is_ok_and(|s| s.success())
        });
        if !ok {
            let _ = if path.is_file() && is_runnable(&path) {
                reveal(&path)
            } else {
                open::that_detached(&path)
            };
        }
    });
}

/// Split a command line into words; double quotes keep spaces in a word.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in command.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// The program file for `name`: a path as given, or the first match on PATH
/// with one of the `PATHEXT` extensions (`code` -> `code.cmd`). The current
/// folder is never searched.
fn find_program(name: &str) -> Option<PathBuf> {
    let exts = path_exts();
    let resolve = |base: &Path| -> Option<PathBuf> {
        if base.extension().is_some() && base.is_file() {
            return Some(base.to_path_buf());
        }
        exts.iter().find_map(|ext| {
            let mut file = base.as_os_str().to_owned();
            file.push(ext);
            let file = PathBuf::from(file);
            file.is_file().then_some(file)
        })
    };
    let given = Path::new(name);
    if given.is_absolute() || given.components().count() > 1 {
        return resolve(given);
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| resolve(&dir.join(given)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_like_a_shell() {
        assert_eq!(split_command("code"), ["code"]);
        assert_eq!(split_command("  code  -n "), ["code", "-n"]);
        assert_eq!(
            split_command(r#""C:\Program Files\JetBrains\rider64.exe" --wait"#),
            [r"C:\Program Files\JetBrains\rider64.exe", "--wait"]
        );
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn runnable_files_are_recognised() {
        for name in ["setup.EXE", "a.bat", "x.lnk", "y.url", "z.hta", "w.ps1"] {
            assert!(is_runnable(Path::new(name)), "{name}");
        }
        for name in ["README.md", "main.rs", "Makefile", "a.txt"] {
            assert!(!is_runnable(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn programs_are_found_on_path_not_in_the_current_folder() {
        let cmd = find_program("cmd").expect("cmd.exe is on PATH");
        assert!(
            cmd.ends_with("cmd.exe") || cmd.ends_with("cmd.EXE"),
            "{cmd:?}"
        );
        assert_eq!(find_program("graft-no-such-program"), None);
    }

    /// A file name with `&` in it must reach the program as one argument, not
    /// as a second command.
    #[cfg(windows)]
    #[test]
    fn ampersands_in_paths_stay_arguments() {
        let dir = std::env::temp_dir().join(format!("graft-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("echo-arg.cmd");
        std::fs::write(&script, "@echo %1>\"%~dp0out.txt\"\r\n").unwrap();
        let marker = dir.join("pwned.txt");
        let evil = dir.join("a&echo x>pwned.txt&b.txt");
        let status = Command::new(&script)
            .current_dir(&dir)
            .arg(&evil)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!marker.exists(), "the & ran as a command");
        let out = std::fs::read_to_string(dir.join("out.txt")).unwrap();
        assert!(out.contains("a&echo x>pwned.txt&b.txt"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
