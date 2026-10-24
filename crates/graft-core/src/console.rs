//! The git output console: a bounded log of every git command Graft ran.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::cli::Output;

const MAX_ENTRIES: usize = 500;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Entry {
    pub at: SystemTime,
    pub cwd: String,
    pub command: String,
    pub status: i32,
    /// The exit code was one the caller expected (e.g. 1 for "conflicts").
    pub expected: bool,
    pub duration: Duration,
    pub stdout: String,
    pub stderr: String,
}

impl Entry {
    pub(crate) fn finished(
        cwd: &Path,
        args: &[OsString],
        status: i32,
        duration: Duration,
        out: &Output,
    ) -> Self {
        let command = std::iter::once("git".to_owned())
            .chain(args.iter().map(|a| quote(&a.to_string_lossy())))
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            at: SystemTime::now(),
            cwd: cwd.display().to_string(),
            command,
            status,
            expected: status == 0,
            duration,
            stdout: clip(&out.stdout),
            stderr: clip(&out.stderr),
        }
    }

    pub fn failed(&self) -> bool {
        !self.expected
    }
}

fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_owned()
    }
}

fn clip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_OUTPUT_BYTES)]);
    let mut text = text.into_owned();
    if bytes.len() > MAX_OUTPUT_BYTES {
        text.push_str("\n… (output truncated)");
    }
    text
}

/// Cheap to clone; all clones share the same log.
#[derive(Clone, Default)]
pub struct Console {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    entries: VecDeque<Entry>,
    /// Increments on every push so the UI can tell when to refresh.
    generation: u64,
}

/// `text` with the credentials of any URL in it masked:
/// `https://user:token@host/x` becomes `https://***@host/x`.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let authority = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(tail.len());
        match tail[..authority].rfind('@') {
            Some(user_end) => {
                out.push_str("***");
                rest = &tail[user_end..];
            }
            None => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

impl Console {
    pub fn push(&self, mut entry: Entry) {
        entry.command = redact(&entry.command);
        entry.stdout = redact(&entry.stdout);
        entry.stderr = redact(&entry.stderr);
        let mut inner = self.inner.lock().expect("console lock");
        if inner.entries.len() == MAX_ENTRIES {
            inner.entries.pop_front();
        }
        inner.entries.push_back(entry);
        inner.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.inner.lock().expect("console lock").generation
    }

    /// Newest last.
    pub fn snapshot(&self) -> Vec<Entry> {
        self.inner
            .lock()
            .expect("console lock")
            .entries
            .iter()
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock().expect("console lock");
        inner.entries.clear();
        inner.generation += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn credentials_in_urls_are_masked() {
        assert_eq!(
            redact("git clone -- https://ada:ghp_secret@github.com/a/b.git C:\\x"),
            "git clone -- https://***@github.com/a/b.git C:\\x"
        );
        assert_eq!(
            redact("fatal: 'https://tok@host/r' and https://host/@not-user"),
            "fatal: 'https://***@host/r' and https://host/@not-user"
        );
        assert_eq!(redact("no urls here"), "no urls here");
        assert_eq!(redact("ssh://git@host:22/x"), "ssh://***@host:22/x");
    }
}
