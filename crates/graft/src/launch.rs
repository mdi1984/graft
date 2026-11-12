//! Starting other programs: the IDE, a terminal, Explorer, a file's default
//! program. Paths and names come from repositories Graft does not control, so
//! nothing here goes through `cmd /c` (which would read `&` or `%` in a path as
//! commands), and a file Windows would run (`.exe`, `.bat`, `.lnk`, ...) is
//! never handed to its default program.
//!
//! Launching looks at the disk and starts processes: it runs in background
//! jobs (see `project::launch`), never on the UI thread.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ask for a folder in Windows' folder picker, opened at `start` if that
/// is one. Waits for the user: run it on a thread of its own.
pub fn pick_folder(title: &str, start: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// Ask for a font file (.ttf, .otf, .ttc). Waits for the user: run it on a
/// thread of its own.
pub fn pick_font_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Add a font file")
        .add_filter("Fonts", &["ttf", "otf", "ttc", "otc"])
        .pick_file()
}

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
pub fn open_path(path: &Path, ide_command: &str) -> io::Result<()> {
    if path.is_file() && is_runnable(path) {
        return open_in_ide(path, ide_command);
    }
    open::that_detached(path)
}

/// Open a folder in Explorer; anything that is not a folder is refused.
pub fn open_folder(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Err(not_a_folder());
    }
    open::that_detached(dir)
}

/// Show `path` selected in an Explorer window.
pub fn reveal(path: &Path) -> io::Result<()> {
    let full = graft_core::paths::native(&path.to_string_lossy());
    let mut arg = std::ffi::OsString::from("/select,");
    arg.push(&full);
    Command::new("explorer").arg(arg).spawn().map(drop)
}

/// Open a terminal in `dir`: Windows Terminal if installed, else cmd.
pub fn open_terminal(dir: &Path) -> io::Result<()> {
    if !dir.is_dir() {
        return Err(not_a_folder());
    }
    // Windows Terminal reads `;` as "new tab"; `\;` is a plain semicolon.
    let escaped = dir.to_string_lossy().replace(';', "\\;");
    if Command::new("wt.exe")
        .arg("-d")
        .arg(escaped)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    let mut cmd = Command::new("cmd.exe");
    cmd.current_dir(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
    }
    cmd.spawn().map(drop)
}

/// Open a folder or file with the IDE command from the settings (`code`,
/// `idea64`, a full path, with optional arguments). Without a working IDE a
/// folder opens in Explorer, a document with its default program, and a file
/// that would run is only shown in Explorer. Waits for the IDE command.
pub fn open_in_ide(path: &Path, command: &str) -> io::Result<()> {
    if run_ide(path, command) {
        return Ok(());
    }
    if path.is_file() && is_runnable(path) {
        reveal(path)
    } else {
        open::that_detached(path)
    }
}

/// Run the IDE command on `path`; whether it ran and succeeded.
fn run_ide(path: &Path, command: &str) -> bool {
    let words = split_command(command);
    let Some((program, args)) = words.split_first() else {
        return false;
    };
    let Some(program) = find_program(program) else {
        return false;
    };
    let mut cmd = Command::new(program);
    cmd.args(args).arg(path);
    graft_core::paths::no_window(&mut cmd);
    // A .cmd launcher (code.cmd) goes through cmd.exe; the standard library
    // escapes its arguments and refuses ones it cannot.
    cmd.status().is_ok_and(|s| s.success())
}

/// Run a command from the settings (a program with optional arguments, as
/// for the IDE) in `dir`, with `input` on its stdin; its stdout. Fails when
/// it cannot start, exits with an error (saying what it printed), or takes
/// longer than `timeout` (it is stopped then).
pub fn run_with_input(
    command: &str,
    dir: &Path,
    input: String,
    timeout: std::time::Duration,
) -> anyhow::Result<String> {
    use std::io::{Read, Write};
    use std::process::Stdio;

    use anyhow::{Context, bail};

    let words = split_command(command);
    let Some((name, args)) = words.split_first() else {
        bail!("No command is set");
    };
    let program = find_program(name).with_context(|| format!("{name} was not found"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if dir.is_dir() {
        cmd.current_dir(dir);
    }
    graft_core::paths::no_window(&mut cmd);
    // It starts paused and runs only once it is in the job, so nothing it
    // starts can get out of the job.
    Job::start_paused(&mut cmd);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("Could not start {name}"))?;
    // A .cmd launcher's program runs under cmd.exe: the job holds both.
    let job = match Job::holding(&child) {
        Ok(job) => job,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Could not start {name}: {e}");
        }
    };
    // Each pipe on its own thread, so a full one can't block the others.
    let mut stdin = child.stdin.take().expect("piped");
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(Box::new(child.stdout.take().expect("piped")));
    let stderr = read(Box::new(child.stderr.take().expect("piped")));
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            // Closing the job stops everything the command started too.
            drop(job);
            let _ = child.kill();
            let _ = child.wait();
            bail!("{name} took longer than {} seconds", timeout.as_secs());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    // Whatever it left running goes too, so it can't hold the pipes open.
    drop(job);
    // A command that exits without reading all of stdin is fine.
    let _ = writer.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if !status.success() {
        let said = [stderr.trim(), stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("no output");
        bail!("{name} failed ({status}): {said}");
    }
    Ok(stdout)
}

/// A Windows job object holding a process and every process it starts;
/// they all stop when it is dropped. Nothing outside Windows.
struct Job(#[cfg(windows)] windows_sys::Win32::Foundation::HANDLE);

impl Job {
    /// Make `cmd` start paused; `holding` lets it run.
    fn start_paused(cmd: &mut Command) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
            cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        }
        #[cfg(not(windows))]
        let _ = cmd;
    }

    /// Put the paused `child` in a new job, then let it run.
    #[cfg(windows)]
    fn holding(child: &std::process::Child) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;

        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: plain Win32 calls on a handle this function owns (closed
        // by `Drop`) and on the child's handle, which `child` keeps open.
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let job = Job(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of_val(&info) as u32,
            );
            if set == 0 || AssignProcessToJobObject(job.0, child.as_raw_handle()) == 0 {
                return Err(io::Error::last_os_error());
            }
            resume(child.id())?;
            Ok(job)
        }
    }

    #[cfg(not(windows))]
    fn holding(_: &std::process::Child) -> io::Result<Self> {
        Ok(Job())
    }
}

/// Let the threads of the paused process `pid` run (it has one).
#[cfg(windows)]
fn resume(pid: u32) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    // SAFETY: plain Win32 calls; each handle opened here is closed here.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = false;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    resumed |= ResumeThread(thread) != u32::MAX;
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        if resumed {
            Ok(())
        } else {
            Err(io::Error::other("its thread could not be started"))
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateJobObjectW and is closed once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn not_a_folder() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "not a folder")
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
        // The AI command's tool list stays one argument.
        assert_eq!(
            split_command(crate::ai_message::DEFAULT_COMMAND),
            [
                "claude",
                "-p",
                "--model",
                "haiku",
                "--allowedTools",
                "Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*)"
            ]
        );
    }

    /// A command that takes too long is stopped with everything it started:
    /// a .cmd launcher's nested process would otherwise write the marker.
    #[cfg(windows)]
    #[test]
    fn a_slow_command_is_stopped_with_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let script = dir.path().join("slow.cmd");
        std::fs::write(
            &script,
            "@cmd /c \"ping -n 4 127.0.0.1 >nul & echo done> \"%~dp0marker\"\"\r\n",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let command = format!("\"{}\"", script.display());
        let timeout = std::time::Duration::from_secs(1);
        let err = run_with_input(&command, dir.path(), String::new(), timeout).unwrap_err();
        assert!(err.to_string().contains("took longer"), "{err:#}");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        std::thread::sleep(std::time::Duration::from_secs(5));
        assert!(!marker.exists(), "a child kept running");
    }

    /// A child started at once by a command that then exits stops with it,
    /// so it can't hold the output open or keep running.
    #[cfg(windows)]
    #[test]
    fn a_child_left_behind_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let script = dir.path().join("quick.cmd");
        std::fs::write(
            &script,
            "@start /b cmd /c \"ping -n 3 127.0.0.1 >nul & echo done> \"%~dp0marker\"\"
@echo hi
",
        )
        .unwrap();
        let start = std::time::Instant::now();
        let command = format!("\"{}\"", script.display());
        let timeout = std::time::Duration::from_secs(20);
        let out = run_with_input(&command, dir.path(), String::new(), timeout).unwrap();
        assert_eq!(out.trim(), "hi");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(std::time::Duration::from_secs(4));
        assert!(!marker.exists(), "a child kept running");
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

    /// A bare name is looked up on PATH with the PATHEXT extensions; a path
    /// is taken as given, also without its extension.
    #[cfg(windows)]
    #[test]
    fn programs_are_found_on_path_or_as_given() {
        let cmd = find_program("cmd").expect("cmd.exe is on PATH");
        assert!(
            cmd.ends_with("cmd.exe") || cmd.ends_with("cmd.EXE"),
            "{cmd:?}"
        );
        assert_eq!(find_program("graft-no-such-program"), None);

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("my-ide.cmd");
        std::fs::write(&script, "@exit 0\r\n").unwrap();
        let without_ext = dir.path().join("my-ide");
        let lower = |p: &Path| p.to_string_lossy().to_lowercase();
        let found = find_program(without_ext.to_str().unwrap()).expect("found with PATHEXT");
        assert_eq!(lower(&found), lower(&script));
        assert_eq!(find_program(script.to_str().unwrap()), Some(script));
    }

    /// A file name with `&` in it must reach a `.cmd` IDE launcher as one
    /// argument, not as a second command.
    #[cfg(windows)]
    #[test]
    fn ampersands_in_paths_stay_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("echo-arg.cmd");
        std::fs::write(&script, "@echo %1>\"%~dp0out.txt\"\r\n").unwrap();
        let evil = dir.path().join("a&echo x>pwned.txt&b.txt");
        let command = format!("\"{}\"", script.display());
        assert!(run_ide(&evil, &command), "the IDE command failed");
        let cwd = std::env::current_dir().unwrap();
        for folder in [dir.path(), cwd.as_path()] {
            assert!(!folder.join("pwned.txt").exists(), "the & ran as a command");
        }
        let out = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert!(out.contains("a&echo x>pwned.txt&b.txt"), "{out}");
    }
}
