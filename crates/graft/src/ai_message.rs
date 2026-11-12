//! Commit messages written by an AI command from the settings (by default
//! `claude -p --model haiku`): it gets instructions, the branch's recent
//! commit summaries (so it follows the repository's style, such as
//! Conventional Commits) and the staged diff on stdin, and answers with the
//! message on stdout.
//!
//! It runs programs and waits for them: call it from background jobs only.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

/// The command Settings starts with.
pub const DEFAULT_COMMAND: &str = "claude -p --model haiku";

/// How long the command may take before Graft stops it.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The most of the diff the command gets; a huge diff only costs time.
const MAX_DIFF: usize = 60_000;

/// How many recent commit summaries show the repository's style.
pub const STYLE_EXAMPLES: usize = 10;

const INSTRUCTIONS: &str = "Write a git commit message for the staged changes below.\n\
First line: a summary in the imperative mood, at most 72 characters.\n\
Then, only if it helps, a blank line and a short body that says what changed and why, \
wrapped at 72 characters.\n\
Reply with the commit message only: no quotes, no code fences, no explanations.\n";

/// A commit message, split as the commit box shows it.
#[derive(Debug, PartialEq, Eq)]
pub struct Message {
    pub summary: String,
    pub body: String,
}

/// Ask `command`, run in `dir`, for a message for `diff` (what is staged)
/// in the style of `recent` (the branch's last commit summaries).
pub fn write(command: &str, dir: &Path, diff: &str, recent: &[String]) -> Result<Message> {
    if diff.trim().is_empty() {
        bail!("Nothing is staged");
    }
    let answer = crate::launch::run_with_input(command, dir, prompt(diff, recent), TIMEOUT)?;
    parse(&answer)
}

/// The instructions, the recent summaries to copy the style of, then the
/// diff (cut at [`MAX_DIFF`] bytes).
fn prompt(diff: &str, recent: &[String]) -> String {
    let mut style = String::new();
    if !recent.is_empty() {
        style.push_str(
            "\nFollow the style of this repository's recent commit messages: for example \
             a type prefix such as feat: or fix(scope):, ticket numbers, language and \
             capitalization. The most recent ones, newest first:\n",
        );
        for summary in recent {
            style.push_str(&format!("- {summary}\n"));
        }
    }
    let mut end = diff.len().min(MAX_DIFF);
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    let cut = if end < diff.len() {
        "\n[The diff is longer; the rest is left out.]\n"
    } else {
        ""
    };
    format!(
        "{INSTRUCTIONS}{style}\nThe staged changes:\n\n{}{cut}",
        &diff[..end]
    )
}

/// The answer as a message: code fences and quotes around it go, the first
/// line is the summary, the rest (trimmed) the body.
fn parse(answer: &str) -> Result<Message> {
    let lines: Vec<&str> = answer
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    let text = lines.join("\n");
    let text = text.trim();
    let (summary, body) = text.split_once('\n').unwrap_or((text, ""));
    let summary = summary.trim().trim_matches(['"', '`']).trim();
    if summary.is_empty() {
        bail!("The command answered with no message");
    }
    Ok(Message {
        summary: summary.to_owned(),
        body: body.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_split_into_summary_and_body() {
        let plain = parse("Fix crash on empty repo\n\nRead HEAD only when it exists.\n").unwrap();
        assert_eq!(plain.summary, "Fix crash on empty repo");
        assert_eq!(plain.body, "Read HEAD only when it exists.");
        let fenced = parse("```\n\"Add login form\"\n```\n").unwrap();
        assert_eq!(fenced.summary, "Add login form");
        assert_eq!(fenced.body, "");
        assert!(parse("  \n```\n```").is_err());
    }

    #[test]
    fn a_long_diff_is_cut_on_a_character_boundary() {
        let diff = "ä".repeat(MAX_DIFF);
        let prompt = prompt(&diff, &[]);
        assert!(prompt.starts_with(INSTRUCTIONS));
        assert!(prompt.ends_with("the rest is left out.]\n"));
        assert!(prompt.len() < INSTRUCTIONS.len() + MAX_DIFF + 100);
        assert!(!super::prompt("+x\n", &[]).contains("left out"));
    }

    /// The recent summaries go into the prompt with the request to follow
    /// their style, so a repository using Conventional Commits gets them.
    #[test]
    fn the_prompt_asks_for_the_style_of_the_recent_commits() {
        let recent = ["feat(ui): add filter".to_owned(), "fix: crash".to_owned()];
        let prompt = prompt("+x\n", &recent);
        assert!(prompt.contains("Follow the style of this repository's recent commit"));
        assert!(prompt.contains("- feat(ui): add filter\n- fix: crash\n"));
        assert!(prompt.ends_with("The staged changes:\n\n+x\n"));
        assert!(!super::prompt("+x\n", &[]).contains("Follow the style"));
    }

    /// The command gets the prompt on stdin and its stdout is the message;
    /// one that fails says why. `git hash-object --stdin` reads stdin like
    /// an AI command would and is on PATH wherever the tests run.
    #[test]
    fn the_command_reads_the_prompt_and_its_failure_is_told() {
        let dir = std::env::temp_dir();
        let out =
            crate::launch::run_with_input("git hash-object --stdin", &dir, "x".into(), TIMEOUT)
                .unwrap();
        assert_eq!(out.trim(), "c1b0730e0133447badcfd47fd144e254807b06e1");
        let err = write("git no-such-command", &dir, "+x\n", &[]).unwrap_err();
        assert!(format!("{err:#}").contains("failed"), "{err:#}");
        let err = write("graft-no-such-program", &dir, "+x\n", &[]).unwrap_err();
        assert!(format!("{err:#}").contains("was not found"), "{err:#}");
        assert!(write("git hash-object --stdin", &dir, " \n", &[]).is_err());
    }
}
