//! Running `git.exe`. Every invocation is recorded in the [`Console`] so the
//! user can see exactly what Graft did and what git answered.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossbeam_channel::RecvTimeoutError;

use crate::console::{Console, Entry};
use crate::jobs::{CancelToken, Cancelled};
use crate::model::Oid;
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

fn parse_progress(line: &str) -> Progress {
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

    /// `path` as git sees it: a Linux path for a repository inside WSL.
    /// Fails for a path git there cannot reach (another distro, a network
    /// share): Linux git would take it as a relative file name.
    pub(crate) fn path_arg(&self, path: &Path) -> Result<String> {
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
    pub(crate) fn native_path(&self, path: &str) -> PathBuf {
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
    pub(crate) fn with_index(&self, index: impl Into<PathBuf>) -> Self {
        Self {
            index_file: Some(index.into()),
            ..self.clone()
        }
    }

    /// Paths may be patterns again. For commands that need it: `stash push
    /// -u` stores untracked files but leaves them in place when pathspecs
    /// are literal (git 2.5x), and `check-ignore` refuses literal pathspecs.
    pub(crate) fn without_literal_pathspecs(&self) -> Self {
        Self {
            literal_pathspecs: false,
            ..self.clone()
        }
    }

    /// Run git and return its output whatever the exit code. The console
    /// lists a non-zero exit code as a failure.
    pub(crate) fn run_raw<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_expecting(args, None, Output::success)
    }

    /// Run git, feeding `input` (if any) to stdin, and return its output
    /// whatever the exit code; `expected` says which outcomes are normal, so
    /// the console does not list e.g. "stopped with conflicts" as a failure.
    pub(crate) fn run_expecting<I, S>(
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
        let mut entry = Entry::finished(
            &self.cwd,
            &args,
            output.status,
            started.elapsed(),
            &output.stdout,
            &output.stderr,
        );
        entry.expected = expected(&output);
        self.console.push(entry);
        Ok(output)
    }

    /// Run git, feeding `input` (if any) to stdin, and fail with git's own
    /// message unless `accept` takes the outcome (e.g. exit code 1 for
    /// "not set").
    pub(crate) fn run_accepting<I, S>(
        &self,
        args: I,
        input: Option<&[u8]>,
        accept: impl Fn(&Output) -> bool,
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.run_expecting(args, input, &accept)?;
        if !accept(&out) {
            bail!("{}", failure_message(&out));
        }
        Ok(out)
    }

    /// A config value: `git config <args>`, e.g. `["--local", "user.name"]`.
    /// `None` when it is not set (git exits with 1).
    pub(crate) fn config(&self, args: &[&str]) -> Result<Option<String>> {
        let all = std::iter::once("config").chain(args.iter().copied());
        let out = self.run_accepting(all, None, |o| o.status <= 1)?;
        Ok((out.status == 0).then(|| out.stdout_str().trim().to_owned()))
    }

    /// Run git and fail with git's own message on a non-zero exit code.
    pub fn run<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_accepting(args, None, Output::success)
    }

    /// [`Git::run`], feeding `input` to stdin.
    pub(crate) fn run_input<I, S>(&self, args: I, input: &[u8]) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_accepting(args, Some(input), Output::success)
    }

    /// Run git on `paths`, fed as pathspecs on stdin: a long list does not
    /// fit on the command line (32,767 characters on Windows and for
    /// wsl.exe). Does nothing without paths, which would mean every file.
    pub(crate) fn run_paths<'a>(
        &self,
        args: &[&str],
        paths: impl IntoIterator<Item = &'a String>,
    ) -> Result<()> {
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        if input.is_empty() {
            return Ok(());
        }
        let from_stdin = ["--pathspec-from-file=-", "--pathspec-file-nul"];
        self.run_input(args.iter().chain(&from_stdin), &input)?;
        Ok(())
    }

    /// Run git and return stdout as (lossy) UTF-8.
    pub fn read<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.run(args)?.stdout_str())
    }

    /// The commit `rev` names, or `None` if it names none (e.g. `HEAD`
    /// before the first commit, `x^1` of a root commit).
    pub(crate) fn rev(&self, rev: &str) -> Result<Option<Oid>> {
        let out = self.run_accepting(["rev-parse", "-q", "--verify", rev], None, |o| {
            o.status <= 1
        })?;
        if !out.success() {
            return Ok(None);
        }
        let text = out.stdout_str();
        let id = Oid::from_hex(text.trim().as_bytes())
            .with_context(|| format!("unexpected answer from git rev-parse: {text}"))?;
        Ok(Some(id))
    }

    /// The branch checked out here, or `None` for a detached HEAD.
    pub(crate) fn current_branch(&self) -> Result<Option<String>> {
        let out = self.run_accepting(["symbolic-ref", "-q", "--short", "HEAD"], None, |o| {
            o.status <= 1
        })?;
        let branch = out.stdout_str().trim().to_owned();
        Ok((out.success() && !branch.is_empty()).then_some(branch))
    }

    /// Run a long git command that reports progress on stderr (clone, fetch,
    /// push, pull) and fail with git's message on a non-zero exit code.
    /// `cancel` kills git; the error is then [`Cancelled`].
    pub(crate) fn run_progress(
        &self,
        args: &[&str],
        cancel: &CancelToken,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<()> {
        let out = self.run_with_progress(args, cancel, |line| on_progress(parse_progress(line)))?;
        if !out.success() {
            bail!("{}", failure_message(&out));
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
            match line_rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => on_progress(&line),
                Err(RecvTimeoutError::Timeout) => {}
                // stderr closed: git is done or about to be.
                Err(RecvTimeoutError::Disconnected) => break child.wait()?,
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
        };
        let stderr = stderr_reader.join().expect("stderr reader");
        for line in line_rx.try_iter() {
            on_progress(&line);
        }
        let output = Output {
            status: status.code().unwrap_or(-1),
            stdout: stdout_reader.join().expect("stdout reader"),
            stderr: keep_final_progress(&stderr),
        };
        self.console.push(Entry::finished(
            &self.cwd,
            &args,
            output.status,
            started.elapsed(),
            &output.stdout,
            &output.stderr,
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
                no_window(&mut cmd);
                cmd
            }
        };
        // Never block on a terminal prompt; Git Credential Manager still shows its own window.
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            // Never open an editor (e.g. `merge --continue` for the message):
            // nobody would see it, and git would wait for it forever. The
            // variable, not `core.editor`, because a user's `GIT_EDITOR` wins
            // over the setting.
            .env("GIT_EDITOR", "true")
            // Stable, parseable English output: Graft reads git's messages
            // (see `refusal` for the ones it tells apart).
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
///
/// Most of them undo user settings that change the output Graft reads, so every
/// command (and every caller) gets git's plain machine-readable form.
const CONFIG: [(&str, &str); 8] = [
    // Paths past Windows' old 260-character limit (deep node_modules or
    // target folders), e.g. when removing a worktree. Off by default in Git
    // for Windows.
    ("core.longpaths", "true"),
    // Paths in git's output as they are (UTF-8), not with non-ASCII bytes
    // escaped as octal: Graft shows and passes them back as they come.
    ("core.quotepath", "false"),
    // No "gpg: Signature made ..." lines in `show`/`log` output (commit
    // messages, details, file history).
    ("log.showSignature", "false"),
    // Patches keep their `a/` and `b/` prefixes: Graft parses the headers
    // and `git apply` (staging lines, Preview in main) expects them.
    ("diff.noprefix", "false"),
    ("diff.mnemonicPrefix", "false"),
    ("diff.srcPrefix", "a/"),
    ("diff.dstPrefix", "b/"),
    // Empty context lines keep their leading space, as `git apply` and
    // Graft's diff parser read them.
    ("diff.suppressBlankEmpty", "false"),
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
pub(crate) fn failure_message(out: &Output) -> String {
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

    /// What `git config --get key` answers for `cmd`.
    fn config_value(mut cmd: Command, key: &str) -> String {
        let out = cmd.args(["config", "--get", key]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn every_command_gets_our_settings() {
        let git = Git::new(std::env::temp_dir(), Console::default());
        let cmd = git.command("git").unwrap();
        assert_eq!(config_value(cmd, "core.longpaths"), "true");
        let cmd = git.command("git").unwrap();
        assert_eq!(config_value(cmd, "core.quotepath"), "false");
        let cmd = git.command("git").unwrap();
        assert_eq!(config_value(cmd, "log.showsignature"), "false");
        let cmd = git.command("git").unwrap();
        assert_eq!(config_value(cmd, "diff.noprefix"), "false");
    }

    /// Settings the user passes in the environment (`GIT_CONFIG_COUNT`)
    /// still reach git next to ours, which are numbered after them.
    #[test]
    fn settings_from_the_environment_are_kept() {
        let with_user_setting = || {
            let mut cmd = Command::new("git");
            cmd.env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "graft.test")
                .env("GIT_CONFIG_VALUE_0", "mine")
                .envs(extra_config(1));
            cmd
        };
        assert_eq!(config_value(with_user_setting(), "graft.test"), "mine");
        assert_eq!(config_value(with_user_setting(), "core.longpaths"), "true");
    }

    /// Linux only sees the variables `WSLENV` names, and a named but unset
    /// one arrives empty (an empty `GIT_INDEX_FILE` breaks every command).
    #[test]
    fn wsl_gets_the_variables_that_are_set_and_only_those() {
        let mut cmd = Command::new("wsl.exe");
        cmd.env("GIT_INDEX_FILE", "/tmp/x.index")
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR");
        share_env(&mut cmd);
        let wslenv = cmd
            .get_envs()
            .find(|(name, _)| *name == "WSLENV")
            .and_then(|(_, value)| value)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let names: Vec<&str> = wslenv.split(':').collect();
        assert!(names.contains(&"GIT_INDEX_FILE"), "{wslenv}");
        assert!(names.contains(&"LC_ALL"), "{wslenv}");
        assert!(!names.contains(&"GIT_DIR"), "{wslenv}");
        assert!(!names.contains(&"WSLENV"), "{wslenv}");
    }

    /// The editor git would open, e.g. for `merge --continue`, is one that
    /// returns at once, whatever the user set (here and in WSL).
    #[test]
    fn no_command_opens_an_editor() {
        let git = Git::new(std::env::temp_dir(), Console::default());
        let mut cmd = git.command("git").unwrap();
        let args = ["-c", "core.editor=false", "var", "GIT_EDITOR"];
        let out = cmd.args(args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "true");

        let mut cmd = Command::new("wsl.exe");
        cmd.env("GIT_EDITOR", "true");
        share_env(&mut cmd);
        let wslenv = cmd.get_envs().find(|(name, _)| *name == "WSLENV");
        let wslenv = wslenv.and_then(|(_, value)| value).unwrap();
        assert!(
            wslenv
                .to_string_lossy()
                .split(':')
                .any(|n| n == "GIT_EDITOR")
        );
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
