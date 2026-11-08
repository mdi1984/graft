//! The git output console: a bounded log of every git command Graft ran.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const MAX_ENTRIES: usize = 500;
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Entry {
    /// Position in the log since Graft started (0, 1, 2, …); stays the
    /// same when older entries drop out.
    pub seq: u64,
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
        stdout: &[u8],
        stderr: &[u8],
    ) -> Self {
        let command = std::iter::once("git".to_owned())
            .chain(args.iter().map(|a| quote(&a.to_string_lossy())))
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            seq: 0,
            at: SystemTime::now(),
            cwd: cwd.display().to_string(),
            command,
            status,
            expected: status == 0,
            duration,
            stdout: clip(stdout),
            stderr: clip(stderr),
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
    /// The `seq` of the next entry.
    next: u64,
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
        entry.seq = inner.next;
        inner.next += 1;
        inner.entries.push_back(entry);
    }

    /// The `seq` the next entry gets: changes with every new entry, so the
    /// UI can tell when to read the log again.
    pub fn next_seq(&self) -> u64 {
        self.inner.lock().expect("console lock").next
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_keep_their_number_when_old_ones_drop_out() {
        let console = Console::default();
        let push = |status: i32, stderr: &[u8]| {
            console.push(Entry::finished(
                Path::new("."),
                &[],
                status,
                Duration::ZERO,
                &[],
                stderr,
            ));
        };
        for _ in 0..MAX_ENTRIES + 3 {
            push(0, b"");
        }
        let entries = console.snapshot();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].seq, 3);
        assert_eq!(entries.last().unwrap().seq, MAX_ENTRIES as u64 + 2);
        assert_eq!(console.next_seq(), MAX_ENTRIES as u64 + 3);

        // A failure after the log is full is still found by the UI's
        // "failed since I last looked" check.
        let seen = console.next_seq();
        push(128, b"fatal: boom");
        let new_failures: Vec<Entry> = console
            .snapshot()
            .into_iter()
            .filter(|e| e.seq >= seen && e.failed())
            .collect();
        assert_eq!(new_failures.len(), 1);
        assert_eq!(new_failures[0].stderr, "fatal: boom");
        assert_eq!(console.snapshot().len(), MAX_ENTRIES);
    }

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
