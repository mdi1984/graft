//! Syntax colors for diff lines. syntect parses each hunk once per side (the
//! old version: context and removed lines; the new one: context and added
//! lines), and every span is sorted into a few token kinds that the UI colors
//! per theme.

use std::ops::Range;
use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::diff::{FileDiff, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Keyword,
    String,
    Comment,
    Number,
    Constant,
    Function,
    Type,
}

/// Colored byte ranges of one line's display text ([`crate::diff::Line::text`]).
pub type Spans = Vec<(Range<usize>, Token)>;
/// Spans per hunk and line; empty when the language is unknown or the diff
/// is too big to be worth it.
pub type Syntax = Vec<Vec<Spans>>;

/// Diffs with more lines are shown without colors.
const MAX_LINES: usize = 20_000;
/// Longer lines (minified code) are left plain; they are slow to parse.
const MAX_LINE_LEN: usize = 1_000;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Scope prefixes and their token, most specific first.
fn classes() -> &'static [(Scope, Token)] {
    static CLASSES: OnceLock<Vec<(Scope, Token)>> = OnceLock::new();
    CLASSES.get_or_init(|| {
        [
            ("comment", Token::Comment),
            ("string", Token::String),
            ("constant.numeric", Token::Number),
            ("constant.character", Token::String),
            ("constant", Token::Constant),
            ("variable.language", Token::Keyword),
            ("variable.function", Token::Function),
            ("support.function", Token::Function),
            ("entity.name.function", Token::Function),
            ("entity.name.tag", Token::Keyword),
            ("entity.other.attribute-name", Token::Type),
            ("entity.name", Token::Type),
            ("entity.other.inherited-class", Token::Type),
            ("support.type", Token::Type),
            ("support.class", Token::Type),
            ("storage.type", Token::Keyword),
            ("storage", Token::Keyword),
            ("keyword.operator", Token::Keyword),
            ("keyword", Token::Keyword),
        ]
        .into_iter()
        .filter_map(|(s, t)| Some((Scope::new(s).ok()?, t)))
        .collect()
    })
}

/// The token of the innermost scope that has one.
fn classify(stack: &ScopeStack) -> Option<Token> {
    stack.as_slice().iter().rev().find_map(|scope| {
        classes()
            .iter()
            .find(|(prefix, _)| prefix.is_prefix_of(*scope))
            .map(|&(_, token)| token)
    })
}

/// The language of `path`, by extension or file name.
fn language(path: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name.rsplit_once('.').map_or(name, |(_, ext)| ext);
    // Close relatives the default set does not have.
    let ext = match ext.to_ascii_lowercase().as_str() {
        "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
        "cshtml" | "razor" | "vue" | "svelte" => "html",
        "csproj" | "props" | "targets" | "xaml" | "svg" | "resx" => "xml",
        "kts" => "kt",
        _ => ext,
    };
    set.find_syntax_by_extension(ext)
        .or_else(|| set.find_syntax_by_extension(name))
        .filter(|s| s.name != "Plain Text")
}

pub fn highlight(diff: &FileDiff) -> Syntax {
    let total: usize = diff.hunks.iter().map(|h| h.lines.len()).sum();
    if total > MAX_LINES || diff.binary {
        return Vec::new();
    }
    let Some(syntax) = language(&diff.path) else {
        return Vec::new();
    };
    let mut out: Syntax = diff
        .hunks
        .iter()
        .map(|h| vec![Vec::new(); h.lines.len()])
        .collect();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        // Context lines take their colors from the new side.
        for (skip, keep) in [(Kind::Removed, Kind::Added), (Kind::Added, Kind::Removed)] {
            let mut state = ParseState::new(syntax);
            let mut stack = ScopeStack::new();
            for (l, line) in hunk.lines.iter().enumerate() {
                if line.kind == skip {
                    continue;
                }
                let text = line.text();
                if text.len() > MAX_LINE_LEN {
                    continue;
                }
                let Ok(ops) = state.parse_line(&format!("{text}\n"), syntaxes()) else {
                    break;
                };
                let mut spans = Spans::new();
                let mut push = |range: Range<usize>, token: Option<Token>| {
                    let Some(token) = token else { return };
                    if range.is_empty() {
                        return;
                    }
                    match spans.last_mut() {
                        Some((last, t)) if *t == token && last.end == range.start => {
                            last.end = range.end;
                        }
                        _ => spans.push((range, token)),
                    }
                };
                let mut pos = 0;
                for (at, op) in ops {
                    let at = at.min(text.len());
                    if at > pos {
                        push(pos..at, classify(&stack));
                        pos = at;
                    }
                    if stack.apply(&op).is_err() {
                        break;
                    }
                }
                if pos < text.len() {
                    push(pos..text.len(), classify(&stack));
                }
                if line.kind == keep || (line.kind == Kind::Context && keep == Kind::Added) {
                    out[h][l] = spans;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{Hunk, Line};

    fn line(kind: Kind, text: &str) -> Line {
        Line {
            kind,
            old_no: None,
            new_no: None,
            content: text.as_bytes().to_vec(),
            no_newline: false,
        }
    }

    fn token_at(spans: &Spans, text: &str, word: &str) -> Option<Token> {
        let at = text.find(word)?;
        spans.iter().find(|(r, _)| r.contains(&at)).map(|&(_, t)| t)
    }

    #[test]
    fn rust_tokens_on_both_sides() {
        let lines = [
            (Kind::Context, "/* a comment"),
            (Kind::Context, "   still the comment */"),
            (Kind::Removed, "fn old() -> u32 { 42 }"),
            (Kind::Added, "fn new() -> &'static str { \"hi\" }"),
        ];
        let diff = FileDiff {
            path: "src/lib.rs".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 3,
                new_start: 1,
                new_count: 3,
                section: String::new(),
                lines: lines.iter().map(|(k, t)| line(*k, t)).collect(),
            }],
            ..FileDiff::default()
        };
        let s = highlight(&diff);
        // State carries across lines: the comment continues.
        assert_eq!(
            token_at(&s[0][1], lines[1].1, "still"),
            Some(Token::Comment)
        );
        assert_eq!(token_at(&s[0][2], lines[2].1, "fn"), Some(Token::Keyword));
        assert_eq!(token_at(&s[0][2], lines[2].1, "42"), Some(Token::Number));
        assert_eq!(token_at(&s[0][2], lines[2].1, "old"), Some(Token::Function));
        assert_eq!(token_at(&s[0][3], lines[3].1, "hi"), Some(Token::String));
    }

    #[test]
    fn unknown_languages_stay_plain() {
        let diff = FileDiff {
            path: "notes.unknownext".into(),
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                section: String::new(),
                lines: vec![line(Kind::Context, "fn x() {}")],
            }],
            ..FileDiff::default()
        };
        assert!(highlight(&diff).is_empty());
        assert!(language("App.tsx").is_some());
        assert!(language("Program.cs").is_some());
    }
}
