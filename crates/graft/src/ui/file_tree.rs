//! Changed files under their folders: the rows of a file tree. Rows keep the
//! files' own order, so the tree reads top to bottom like the flat list
//! (Review's "next file not viewed yet" goes the same way). A folder that
//! holds nothing but one folder joins it into one row ("src/ui").

use std::ops::Range;

#[derive(Debug, PartialEq, Eq)]
pub enum Row<'a> {
    /// A folder: `path` is its full path, `name` what the row shows,
    /// `files` the indices of the files inside it, at any depth.
    Folder {
        path: &'a str,
        name: &'a str,
        depth: usize,
        files: Range<usize>,
    },
    /// The file at `index` of the paths given.
    File { index: usize, depth: usize },
}

impl Row<'_> {
    pub fn depth(&self) -> usize {
        match self {
            Row::Folder { depth, .. } | Row::File { depth, .. } => *depth,
        }
    }
}

/// The rows for `paths`, which must be sorted (as Review lists them), so a
/// folder's files are next to each other.
pub fn rows<'a>(paths: &[&'a str]) -> Vec<Row<'a>> {
    let mut rows = Vec::new();
    add(paths, 0..paths.len(), 0, 0, &mut rows);
    rows
}

/// Add the rows of `range`, whose paths all start with the same folder
/// path up to byte `skip`, at `depth`.
fn add<'a>(
    paths: &[&'a str],
    range: Range<usize>,
    skip: usize,
    depth: usize,
    rows: &mut Vec<Row<'a>>,
) {
    let mut i = range.start;
    while i < range.end {
        let Some(slash) = paths[i][skip..].find('/') else {
            rows.push(Row::File { index: i, depth });
            i += 1;
            continue;
        };
        // The folder, joined with the folders below it while it holds only
        // that one.
        let mut end = skip + slash;
        let inside = |end: usize| {
            let prefix = &paths[i][..=end];
            i + paths[i..range.end]
                .iter()
                .take_while(|p| p.starts_with(prefix))
                .count()
        };
        let last = inside(end);
        while let Some(next) = paths[i][end + 1..].find('/').map(|n| end + 1 + n)
            && inside(next) == last
        {
            end = next;
        }
        rows.push(Row::Folder {
            path: &paths[i][..end],
            name: &paths[i][skip..end],
            depth,
            files: i..last,
        });
        add(paths, i..last, end + 1, depth + 1, rows);
        i = last;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows as text: folders end with "/", indented two spaces a level.
    fn shown(paths: &[&str]) -> Vec<String> {
        rows(paths)
            .iter()
            .map(|row| match row {
                Row::Folder { name, depth, .. } => format!("{}{name}/", "  ".repeat(*depth)),
                Row::File { index, depth } => {
                    let name = paths[*index].rsplit('/').next().unwrap();
                    format!("{}{name}", "  ".repeat(*depth))
                }
            })
            .collect()
    }

    #[test]
    fn files_sit_under_their_folders_in_list_order() {
        let paths = [
            "Cargo.toml",
            "crates/core/src/lib.rs",
            "crates/core/src/ops.rs",
            "crates/graft/src/app.rs",
            "crates/graft/src/ui/review.rs",
            "readme.md",
        ];
        assert_eq!(
            shown(&paths),
            [
                "Cargo.toml",
                "crates/",
                "  core/src/",
                "    lib.rs",
                "    ops.rs",
                "  graft/src/",
                "    app.rs",
                "    ui/",
                "      review.rs",
                "readme.md",
            ]
        );
        // Every file shows once, in the order given.
        let files: Vec<usize> = rows(&paths)
            .iter()
            .filter_map(|row| match row {
                Row::File { index, .. } => Some(*index),
                Row::Folder { .. } => None,
            })
            .collect();
        assert_eq!(files, (0..paths.len()).collect::<Vec<_>>());
    }

    #[test]
    fn a_folder_knows_its_files_and_full_path() {
        let paths = ["a/b/one.rs", "a/b/two.rs", "a/c.rs"];
        let rows = rows(&paths);
        assert_eq!(
            rows[0],
            Row::Folder {
                path: "a",
                name: "a",
                depth: 0,
                files: 0..3
            }
        );
        assert_eq!(
            rows[1],
            Row::Folder {
                path: "a/b",
                name: "b",
                depth: 1,
                files: 0..2
            }
        );
    }

    /// A folder named like the start of another ("src" and "src-old") is
    /// its own folder.
    #[test]
    fn folders_with_a_common_start_stay_apart() {
        let paths = ["src-old/a.rs", "src/b.rs", "src/c/d.rs"];
        assert_eq!(
            shown(&paths),
            ["src-old/", "  a.rs", "src/", "  b.rs", "  c/", "    d.rs"]
        );
    }
}
