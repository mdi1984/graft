//! A string literal with a run of spaces in the middle of a line is almost
//! always a sentence whose `\` line continuation got lost, which then shows
//! as a gap in the UI. Scans the sources of every crate in `crates/`.

use std::path::{Path, PathBuf};

/// Literals that have such gaps on purpose: (file, start of the literal).
const ALLOWED: &[(&str, &str)] = &[
    // `wsl --list --verbose` tables, in a test.
    ("graft-core/src/wsl.rs", "NAME  "),
];

#[test]
fn no_string_literal_has_a_gap_mid_line() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(crates).unwrap() {
        rust_files(&entry.unwrap().path().join("src"), &mut files);
    }
    for krate in ["graft", "graft-core"] {
        let src = crates.join(krate).join("src");
        assert!(files.iter().any(|f| f.starts_with(&src)), "{krate}");
    }
    let mut found = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        let name = file.strip_prefix(crates).unwrap().to_string_lossy();
        let name = name.replace('\\', "/");
        for (line, gap) in gaps(&source) {
            if !ALLOWED
                .iter()
                .any(|(f, start)| *f == name && gap.starts_with(start))
            {
                found.push(format!("{name}:{line}: {gap}"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "gaps in string literals:\n{}",
        found.join("\n")
    );
}

/// The literal from 31c93df's fix, as it was before.
#[test]
fn a_lost_line_continuation_is_found() {
    let source = r#"
        bail!(
            "Nothing was merged: a {} is in progress in {place}. Finish or abort it, then              merge again.",
            op.kind.noun().to_lowercase()
        );
        let ok = "a sentence \
                  continued";
        let fixture = "line one
    indented line two";
        // A comment with a "quoted    gap" is not a literal.
        let c = '"'; let s = "fine";
    "#;
    let found: Vec<_> = gaps(source).into_iter().map(|(line, _)| line).collect();
    assert_eq!(found, [3]);
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The string literals in `source` (trimmed, with the line of their first
/// gap) that have a non-space, four or more spaces and a non-space on one
/// line.
fn gaps(source: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut line = 1;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\n' => line += 1,
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                let end = find(&chars, i + 2, &['*', '/']).unwrap_or(chars.len());
                line += chars[i..end].iter().filter(|c| **c == '\n').count();
                i = end + 2;
                continue;
            }
            // A char literal ('"', '\'', 'x'); otherwise a lifetime.
            '\'' if chars.get(i + 1) == Some(&'\\') => {
                i = find(&chars, i + 3, &['\'']).unwrap_or(chars.len()) + 1;
                continue;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') => {
                i += 3;
                continue;
            }
            'r' if raw_start(&chars, i).is_some() => {
                let (hashes, start) = raw_start(&chars, i).unwrap();
                let mut close = vec!['"'];
                close.extend(std::iter::repeat_n('#', hashes));
                let end = find(&chars, start, &close).unwrap_or(chars.len());
                check(&chars[start..end], line, &mut out);
                line += chars[i..end].iter().filter(|c| **c == '\n').count();
                i = end + close.len();
                continue;
            }
            '"' => {
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && chars[end] != '"' {
                    end += if chars[end] == '\\' { 2 } else { 1 };
                }
                let end = end.min(chars.len());
                check(&chars[start..end], line, &mut out);
                line += chars[i..end].iter().filter(|c| **c == '\n').count();
                i = end + 1;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// After `r` at `i`: a raw string's (`r"`, `r#"`, `br"`…) number of `#`
/// and where its text starts.
fn raw_start(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let ident = |at: Option<usize>| {
        at.and_then(|at| chars.get(at))
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
    };
    let prefixed = i > 0 && matches!(chars[i - 1], 'b' | 'c');
    if ident(i.checked_sub(1)) && !(prefixed && !ident(i.checked_sub(2))) {
        return None;
    }
    let hashes = chars[i + 1..].iter().take_while(|c| **c == '#').count();
    (chars.get(i + 1 + hashes) == Some(&'"')).then_some((hashes, i + 2 + hashes))
}

fn find(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    (from..chars.len()).find(|&at| chars[at..].starts_with(needle))
}

/// Record the literal `text` (starting on `line`) if it has a gap.
fn check(text: &[char], line: usize, out: &mut Vec<(usize, String)>) {
    let text: String = text.iter().collect();
    let gap = text
        .split('\n')
        .position(|part| part.trim().contains("    "));
    if let Some(n) = gap {
        out.push((line + n, text.trim().to_owned()));
    }
}
