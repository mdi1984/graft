//! Running `git.exe`. Every invocation is recorded in the [`Console`] so the
//! user can see exactly what Graft did and what git answered.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::console::{Console, Entry};

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

/// A git invocation bound to a working directory.
#[derive(Clone)]
pub struct Git {
    cwd: PathBuf,
    console: Console,
}

impl Git {
    pub fn new(cwd: impl Into<PathBuf>, console: Console) -> Self {
        Self {
            cwd: cwd.into(),
            console,
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn console(&self) -> &Console {
        &self.console
    }

    /// Same console, other directory (e.g. another worktree).
    pub fn at(&self, cwd: impl Into<PathBuf>) -> Self {
        Self::new(cwd, self.console.clone())
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
        let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let started = Instant::now();
        let mut cmd = command(&self.cwd);
        cmd.args(&args);
        cmd.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| "could not start git — is Git for Windows installed?")?;
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
        self.console.push(Entry::finished(
            &self.cwd,
            &args,
            output.status,
            started.elapsed(),
            &output,
        ));
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
    /// push, pull). `on_progress` gets each progress line; `cancel` kills git.
    pub fn run_with_progress<I, S>(
        &self,
        args: I,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(&str),
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
        let started = Instant::now();
        let mut cmd = command(&self.cwd);
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| "could not start git — is Git for Windows installed?")?;

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
            if cancel.load(Ordering::Relaxed) {
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
        if cancel.load(Ordering::Relaxed) {
            bail!("Cancelled");
        }
        Ok(output)
    }
}

/// Spawn a child process in the background without flashing a console window.
pub(crate) fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd)
        // Never block on a terminal prompt; Git Credential Manager still shows its own window.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Stable, parseable English output.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        // Reads like `status` must not rewrite the index: that would wake the
        // file watcher and refresh forever.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
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
    let stderr = out.stderr_str();
    let text = if stderr.is_empty() {
        out.stdout_str().trim().to_owned()
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

/// Shared flag for cancelling a running job.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.0
    }
}
