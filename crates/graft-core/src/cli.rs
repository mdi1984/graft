//! Running `git.exe`. Every invocation is recorded in the [`Console`] so the
//! user can see exactly what Graft did and what git answered.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::console::{Console, Entry};
use crate::jobs::{CancelToken, Cancelled};
use crate::paths::no_window;
use crate::wsl::Wsl;

/// Output of a finished git command.
#[derive(Debug)]
pub struct Output {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

/// Progress of a long command (clone, fetch, push), parsed from git's stderr.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// E.g. "Receiving objects".
    pub phase: String,
    /// 0.0..=1.0 if known.
    pub fraction: Option<f32>,
    /// The raw line, for display.
    pub line: String,
}

impl Progress {
    /// A step of Graft's own (not a git progress line).
    pub fn step(line: String, fraction: f32) -> Self {
        Self {
            phase: line.clone(),
            fraction: Some(fraction),
            line,
        }
    }
}

pub fn parse_progress(line: &str) -> Progress {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':').unwrap_or((line, ""));
    let fraction = rest
        .split_whitespace()
        .find_map(|w| w.strip_suffix('%'))
        .and_then(|p| p.parse::<f32>().ok())
        .map(|p| (p / 100.0).clamp(0.0, 1.0));
    Progress {
        phase: phase.trim().to_owned(),
        fraction,
        line: line.to_owned(),
    }
}

/// A git invocation bound to a working directory.
#[derive(Clone)]
pub struct Git {
    cwd: PathBuf,
    /// The distro git runs in, for a repository inside WSL.
    wsl: Option<Wsl>,
    console: Console,
    /// Use this index file instead of the worktree's (`GIT_INDEX_FILE`).
    index_file: Option<PathBuf>,
    literal_pathspecs: bool,
}

impl Git {
    pub fn new(cwd: impl Into<PathBuf>, console: Console) -> Self {
        let cwd = cwd.into();
        Self {
            wsl: Wsl::of(&cwd),
            cwd,
            console,
            index_file: None,
            literal_pathspecs: true,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn console(&self) -> &Console {
        &self.console
    }

    /// `path` as git sees it: a Linux path for a repository inside WSL.
    /// Fails for a path git there cannot reach (another distro, a network
    /// share): Linux git would take it as a relative file name.
    pub fn path_arg(&self, path: &Path) -> Result<String> {
        match &self.wsl {
            None => Ok(path.to_string_lossy().into_owned()),
            Some(wsl) => wsl.to_linux(path).with_context(|| {
                format!(
                    "{} is not inside {}. Pick a folder there for this repository.",
                    crate::paths::display(path),
                    wsl.distro()
                )
            }),
        }
    }

    /// An absolute path git printed, as a Windows path.
    pub fn native_path(&self, path: &str) -> PathBuf {
        match &self.wsl {
            Some(wsl) => wsl.to_windows(path),
            None => crate::paths::native(path),
        }
    }

    /// Same console, other directory (e.g. another worktree).
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Self {
        Self::new(cwd, self.console.clone())
    }

    /// Same directory, but every command uses `index` as the index, leaving
    /// the real one alone.
    pub fn with_index(&self, index: impl Into<PathBuf>) -> Self {
        Self {
            index_file: Some(index.into()),
            ..self.clone()
        }
    }

    /// Paths may be patterns again. For commands that need it: `stash push
    /// -u` stores untracked files but leaves them in place when pathspecs
    /// are literal (git 2.5x), and `check-ignore` refuses literal pathspecs.
    pub fn without_literal_pathspecs(&self) -> Self {
        Self {
            literal_pathspecs: false,
            ..self.clone()
        }
    }

    /// Run git and return its output whatever the exit code.
    pub fn run_raw<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_with_input(args, None)
    }

    /// Run git, feeding `input` to stdin.
    pub fn run_with_input<I, S>(&self, args: I, input: Option<&[u8]>) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_expecting(args, input, |out| out.success())
    }

    /// Run git; `expected` says which outcomes are normal, so the console
    /// does not list e.g. "stopped with conflicts" as a failure.
    pub fn run_expecting<I, S>(
        &self,
        args: I,
        input: Option<&[u8]>,
        expected: impl Fn(&Output) -> bool,
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = collect(args);
        let started = Instant::now();
        let mut child = self.spawn(&args, input.is_some())?;
        if let Some(input) = input {
            use std::io::Write;
            let mut stdin = child.stdin.take().expect("stdin was piped");
            // Write on a thread so a large patch cannot deadlock against a full stdout pipe.
            let input = input.to_vec();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&input);
            });
        }
        let out = child.wait_with_output()?;
        let output = Output {
            status: out.status.code().unwrap_or(-1),
            stdout: out.stdout,
            stderr: out.stderr,
        };
        let mut entry =
            Entry::finished(&self.cwd, &args, output.status, started.elapsed(), &output);
        entry.expected = expected(&output);
        self.console.push(entry);
        Ok(output)
    }

    /// Run git and fail with git's own message on a non-zero exit code.
    pub fn run<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.run_raw(args)?;
        if !out.success() {
            bail!("{}", failure_message(&out));
        }
        Ok(out)
    }

    /// Run git and return stdout as (lossy) UTF-8.
    pub fn read<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.run(args)?.stdout_str())
    }

    /// Run a long git command that reports progress on stderr (clone, fetch,
    /// push, pull) and fail with git's message on a non-zero exit code.
    /// `cancel` kills git; the error is then [`Cancelled`].
    pub fn run_progress(
        &self,
        args: &[&str],
        cancel: &CancelToken,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<()> {
        let out = self.run_with_progress(args, cancel, |line| on_progress(parse_progress(line)))?;
        if !out.success() {
            bail!(failure_message(&out));
        }
        Ok(())
    }

    fn run_with_progress(
        &self,
        args: &[&str],
        cancel: &CancelToken,
        mut on_progress: impl FnMut(&str),
    ) -> Result<Output> {
        let args = collect(args);
        let started = Instant::now();
        let mut child = self.spawn(&args, false)?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stdout_reader = std::thread::spawn(move || read_all(stdout));
        let (line_tx, line_rx) = crossbeam_channel::unbounded::<String>();
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let stderr_reader = std::thread::spawn(move || {
            // Progress lines end in '\r' (rewritten in place) or '\n'.
            let mut all = Vec::new();
            let mut line = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &b in &buf[..n] {
                    all.push(b);
                    if b == b'\r' || b == b'\n' {
                        if !line.is_empty() {
                            let _ = line_tx.send(String::from_utf8_lossy(&line).into_owned());
                            line.clear();
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
            if !line.is_empty() {
                let _ = line_tx.send(String::from_utf8_lossy(&line).into_owned());
            }
            all
        });

        let status = loop {
            for line in line_rx.try_iter() {
                on_progress(&line);
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let stderr = stderr_reader.join().unwrap_or_default();
        for line in line_rx.try_iter() {
            on_progress(&line);
        }
        let output = Output {
            status: status.code().unwrap_or(-1),
            stdout: stdout_reader.join().unwrap_or_default(),
            stderr: keep_final_progress(&stderr),
        };
        self.console.push(Entry::finished(
            &self.cwd,
            &args,
            output.status,
            started.elapsed(),
            &output,
        ));
        if cancel.is_cancelled() {
            return Err(Cancelled.into());
        }
        Ok(output)
    }

    fn spawn(&self, args: &[OsString], stdin: bool) -> Result<Child> {
        let mut cmd = self.command("git")?;
        cmd.args(args)
            .stdin(if stdin { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.spawn()
            .with_context(|| "could not start git — is Git for Windows installed?")
    }

    /// `program` (git, or a shell for scripts around it) in this directory
    /// with the environment every git call gets, not logged in the console.
    /// In a repository inside WSL it runs in the distro: Windows git over
    /// `\\wsl.localhost` takes seconds per status and misreads the files.
    pub(crate) fn command(&self, program: &str) -> Result<Command> {
        let mut cmd = match &self.wsl {
            Some(wsl) => wsl.command(&self.cwd, program),
            None => {
                let mut cmd = Command::new(program);
                cmd.current_dir(&self.cwd);
                cmd
            }
        };
        // Never block on a terminal prompt; Git Credential Manager still shows its own window.
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            // Stable, parseable English output: Graft reads git's messages
            // (see `refs` for the ones it tells apart).
            .env("LC_ALL", "C")
            .env("LANG", "C")
            // Reads like `status` must not rewrite the index: that would wake the
            // file watcher and refresh forever.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .envs(extra_config(user_config_count()))
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        if self.literal_pathspecs {
            // Paths are file names, never patterns: `x[ab].txt` must not also
            // match `xa.txt` when discarding or staging.
            cmd.env("GIT_LITERAL_PATHSPECS", "1");
        } else {
            cmd.env_remove("GIT_LITERAL_PATHSPECS");
        }
        if let Some(index) = &self.index_file {
            cmd.env("GIT_INDEX_FILE", self.path_arg(index)?);
        }
        share_env(&mut cmd);
        no_window(&mut cmd);
        Ok(cmd)
    }
}

fn collect<I, S>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().map(|a| a.as_ref().to_owned()).collect()
}

/// Hand the variables set on `cmd` to Linux when it runs in WSL: Linux only
/// sees the ones `WSLENV` names, and a named but unset one arrives empty
/// (an empty `GIT_INDEX_FILE` breaks every command). Must come after the
/// last `env`/`env_remove`.
fn share_env(cmd: &mut Command) {
    if cmd.get_program() != "wsl.exe" {
        return;
    }
    let mut names: Vec<String> = std::env::var("WSLENV")
        .ok()
        .filter(|user| !user.is_empty())
        .into_iter()
        .collect();
    // Settings the user passes in the environment: ours are numbered after
    // them, so Linux git needs theirs too.
    names.extend(inherited_config(user_config_count()));
    names.extend(
        cmd.get_envs()
            .filter(|(name, value)| value.is_some() && *name != "WSLENV")
            .map(|(name, _)| name.to_string_lossy().into_owned()),
    );
    cmd.env("WSLENV", names.join(":"));
}

/// How many settings the user passes in the environment (`GIT_CONFIG_COUNT`).
fn user_config_count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    *COUNT.get_or_init(|| {
        std::env::var("GIT_CONFIG_COUNT")
            .ok()
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0)
    })
}

/// Names of the `GIT_CONFIG_KEY_n` / `GIT_CONFIG_VALUE_n` variables the
/// user's own `GIT_CONFIG_COUNT` covers.
fn inherited_config(count: usize) -> Vec<String> {
    (0..count)
        .flat_map(|i| {
            [
                format!("GIT_CONFIG_KEY_{i}"),
                format!("GIT_CONFIG_VALUE_{i}"),
            ]
        })
        .collect()
}

/// Settings every git command Graft runs gets, added after any the user
/// passes in the environment (`GIT_CONFIG_COUNT`), without touching their
/// config files.
const CONFIG: [(&str, &str); 1] = [
    // Paths past Windows' old 260-character limit (deep node_modules or
    // target folders), e.g. when removing a worktree. Off by default in Git
    // for Windows.
    ("core.longpaths", "true"),
];

fn extra_config(start: usize) -> Vec<(String, String)> {
    let mut env = vec![(
        "GIT_CONFIG_COUNT".to_owned(),
        (start + CONFIG.len()).to_string(),
    )];
    for (i, (key, value)) in CONFIG.iter().enumerate() {
        env.push((format!("GIT_CONFIG_KEY_{}", start + i), (*key).to_owned()));
        env.push((
            format!("GIT_CONFIG_VALUE_{}", start + i),
            (*value).to_owned(),
        ));
    }
    env
}

fn read_all(mut r: impl Read) -> Vec<u8> {
    let mut v = Vec::new();
    let _ = r.read_to_end(&mut v);
    v
}

/// Collapse "\r"-rewritten progress lines so the console shows only the final state.
fn keep_final_progress(stderr: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(stderr);
    let mut out = String::new();
    for line in text.split('\n') {
        let last = line
            .rsplit('\r')
            .find(|s| !s.trim().is_empty())
            .unwrap_or("");
        if !last.is_empty() {
            out.push_str(last);
            out.push('\n');
        }
    }
    out.into_bytes()
}

/// The most useful line(s) of a failed git command, for showing to the user.
pub fn failure_message(out: &Output) -> String {
    let stderr = crate::console::redact(&out.stderr_str());
    let text = if stderr.is_empty() {
        // wsl.exe reports its own failures (e.g. a removed distro) in UTF-16.
        let stdout = if out.stdout.contains(&0) {
            crate::wsl::decode(&out.stdout)
        } else {
            out.stdout_str()
        };
        crate::console::redact(stdout.trim())
    } else {
        stderr
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("hint:"))
        .collect();
    if lines.is_empty() {
        format!("git exited with code {}", out.status)
    } else {
        lines
            .iter()
            .map(|l| {
                l.trim_start_matches("fatal: ")
                    .trim_start_matches("error: ")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_the_environment_is_kept() {
        let env = extra_config(0);
        assert_eq!(env[0], ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()));
        assert_eq!(env[1].0, "GIT_CONFIG_KEY_0");
        // The user already passes two settings: ours come after them.
        assert!(inherited_config(0).is_empty());
        assert_eq!(
            inherited_config(1),
            ["GIT_CONFIG_KEY_0", "GIT_CONFIG_VALUE_0"]
        );
        let env = extra_config(2);
        assert_eq!(env[0].1, "3");
        assert_eq!(
            env[1],
            ("GIT_CONFIG_KEY_2".to_owned(), "core.longpaths".to_owned())
        );
        assert_eq!(env[2], ("GIT_CONFIG_VALUE_2".to_owned(), "true".to_owned()));
    }

    #[test]
    fn progress_lines() {
        let p = parse_progress("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.fraction, Some(0.45));
        let p = parse_progress("remote: Counting objects: 100% (12/12), done.");
        assert_eq!(p.phase, "Counting objects");
        assert_eq!(p.fraction, Some(1.0));
        let p = parse_progress("Cloning into 'graft'...");
        assert_eq!(p.fraction, None);
    }
}
