//! Time syntax highlighting of a file shown as all-new lines:
//! `cargo run --release -p graft-core --example syntax_timing -- <file>`.
use std::time::Instant;

use graft_core::diff::{FileDiff, Hunk, Kind, Line};

fn main() {
    let path = std::env::args().nth(1).expect("file path");
    let text = std::fs::read(&path).expect("readable file");
    let lines: Vec<Line> = text
        .split(|&b| b == b'\n')
        .enumerate()
        .map(|(i, content)| Line {
            kind: Kind::Added,
            old_no: None,
            new_no: Some(i as u32 + 1),
            content: content.to_vec(),
            no_newline: false,
        })
        .collect();
    let count = lines.len();
    let diff = FileDiff {
        path: path.clone(),
        hunks: vec![Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: count as u32,
            section: String::new(),
            lines,
        }],
        ..FileDiff::default()
    };
    let t = Instant::now();
    graft_core::syntax::highlight(&diff);
    println!(
        "first (loads syntaxes): {:?} for {count} lines",
        t.elapsed()
    );
    let t = Instant::now();
    graft_core::syntax::highlight(&diff);
    println!("again: {:?}", t.elapsed());
}
