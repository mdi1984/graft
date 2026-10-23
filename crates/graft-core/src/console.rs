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
            duration,
            stdout: clip(&out.stdout),
            stderr: clip(&out.stderr),
        }
    }

    pub fn failed(&self) -> bool {
        self.status != 0
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

impl Console {
    pub fn push(&self, entry: Entry) {
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
