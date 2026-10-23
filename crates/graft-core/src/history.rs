//! Commit history: walking with gix, ordering like `git log --date-order`, and
//! laying out the commit graph into colored lanes.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::Path;

use anyhow::Result;
use smallvec::SmallVec;

use crate::cli::CancelToken;
use crate::model::{Oid, null_oid};

#[derive(Clone, Debug)]
pub struct Commit {
    pub id: Oid,
    pub parents: SmallVec<[Oid; 2]>,
    pub author: String,
    pub email: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    /// Committer time; used for ordering like git does.
    pub commit_time: i64,
    pub summary: String,
}

impl Commit {
    /// The pseudo commit standing for uncommitted changes.
    pub fn is_worktree(&self) -> bool {
        self.id.is_null()
    }
}

/// Walked commits, newest first, before ordering and layout.
#[derive(Clone, Debug, Default)]
pub struct Commits {
    pub list: Vec<Commit>,
    /// True if the walk stopped at the limit.
    pub truncated: bool,
}

/// Walk all commits reachable from `tips`, newest first, up to `limit`.
pub fn walk(root: &Path, tips: &[Oid], limit: usize, cancel: &CancelToken) -> Result<Commits> {
    let mut repo = gix::open(root)?;
    repo.object_cache_size_if_unset(64 * 1024 * 1024);
    let mut commits = Commits::default();
    if tips.is_empty() {
        return Ok(commits);
    }
    let walk = repo
        .rev_walk(tips.iter().copied())
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()?;
    for info in walk {
        if cancel.is_cancelled() {
            anyhow::bail!("Cancelled");
        }
        if commits.list.len() >= limit {
            commits.truncated = true;
            break;
        }
        let info = info?;
        let commit = info.object()?;
        let author = commit.author()?;
        let time = author.time().map(|t| t.seconds).unwrap_or(0);
        let summary = summary_line(commit.message_raw_sloppy());
        commits.list.push(Commit {
            id: info.id,
            parents: info.parent_ids.iter().copied().collect(),
            author: author.name.to_string(),
            email: author.email.to_string(),
            time,
            commit_time: info.commit_time.unwrap_or(time),
            summary,
        });
    }
    Ok(commits)
}

fn summary_line(message: &[u8]) -> String {
    let text = String::from_utf8_lossy(message);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

/// Commits in display order with their graph rows.
#[derive(Clone, Debug, Default)]
pub struct History {
    pub commits: Vec<Commit>,
    pub rows: Vec<GraphRow>,
    pub lanes: usize,
    pub index: HashMap<Oid, usize>,
    pub truncated: bool,
}

impl History {
    /// Order and lay out `commits`. With `worktree_parent`, a pseudo commit for
    /// uncommitted changes is shown on top of that commit.
    pub fn build(commits: &Commits, worktree_parent: Option<Oid>) -> Self {
        let mut list = commits.list.clone();
        if let Some(parent) = worktree_parent {
            list.push(Commit {
                id: null_oid(),
                parents: SmallVec::from_slice(&[parent]),
                author: String::new(),
                email: String::new(),
                time: i64::MAX,
                commit_time: i64::MAX,
                summary: String::new(),
            });
        }
        let ordered = date_order(list);
        let (rows, lanes) = layout(&ordered);
        let index = ordered.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        Self {
            commits: ordered,
            rows,
            lanes,
            index,
            truncated: commits.truncated,
        }
    }
}

/// Children before parents; among ready commits the newest first
/// (`git log --date-order`).
fn date_order(list: Vec<Commit>) -> Vec<Commit> {
    let index: HashMap<Oid, usize> = list.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut children = vec![0usize; list.len()];
    for c in &list {
        for p in unique(&c.parents) {
            if let Some(&pi) = index.get(&p) {
                children[pi] += 1;
            }
        }
    }
    let mut ready: BinaryHeap<(i64, Reverse<usize>)> = children
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == 0)
        .map(|(i, _)| (list[i].commit_time, Reverse(i)))
        .collect();
    let mut order = Vec::with_capacity(list.len());
    while let Some((_, Reverse(i))) = ready.pop() {
        order.push(i);
        for p in unique(&list[i].parents) {
            if let Some(&pi) = index.get(&p) {
                children[pi] -= 1;
                if children[pi] == 0 {
                    ready.push((list[pi].commit_time, Reverse(pi)));
                }
            }
        }
    }
    let mut slots: Vec<Option<Commit>> = list.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

fn unique(parents: &[Oid]) -> SmallVec<[Oid; 2]> {
    let mut out: SmallVec<[Oid; 2]> = SmallVec::new();
    for p in parents {
        if !out.contains(p) {
            out.push(*p);
        }
    }
    out
}

/// One row of the commit graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub lane: u16,
    pub color: u8,
    pub lines: SmallVec<[Line; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// Lane at the top of the row (`Pass`, `In`) or at the bottom (`Out`).
    pub lane: u16,
    /// For `Pass`: lane at the bottom (equal to `lane`: lanes never shift).
    pub to: u16,
    pub color: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// Top edge to bottom edge, not touching this row's commit.
    Pass,
    /// Top edge into this row's commit (a child above).
    In,
    /// From this row's commit to the bottom edge (towards a parent).
    Out,
}

/// Number of distinct lane colors the UI provides.
pub const LANE_COLORS: u8 = 8;

/// Assign every commit a lane. Lanes keep their column for their whole life,
/// freed columns are reused, and each new lane gets the next color.
pub fn layout(commits: &[Commit]) -> (Vec<GraphRow>, usize) {
    // Each lane: the commit it is waiting for and its color.
    let mut lanes: Vec<Option<(Oid, u8)>> = Vec::new();
    let mut next_color = 0u8;
    let mut new_color = || {
        let c = next_color;
        next_color = (next_color + 1) % LANE_COLORS;
        c
    };
    let mut rows = Vec::with_capacity(commits.len());
    let mut max_lanes = 0;

    for commit in commits {
        let mut row = GraphRow::default();
        let expecting: SmallVec<[usize; 4]> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Some((id, _)) if *id == commit.id))
            .map(|(i, _)| i)
            .collect();

        let (node_lane, node_color) = match expecting.first() {
            Some(&i) => (i, lanes[i].expect("expecting lane").1),
            None => (free_slot(&mut lanes), new_color()),
        };
        row.lane = node_lane as u16;
        row.color = node_color;

        for &i in &expecting {
            let color = lanes[i].expect("expecting lane").1;
            row.lines.push(Line {
                kind: LineKind::In,
                lane: i as u16,
                to: node_lane as u16,
                color,
            });
            lanes[i] = None;
        }
        for (i, lane) in lanes.iter().enumerate() {
            if let Some((_, color)) = lane {
                row.lines.push(Line {
                    kind: LineKind::Pass,
                    lane: i as u16,
                    to: i as u16,
                    color: *color,
                });
            }
        }

        for (k, parent) in unique(&commit.parents).into_iter().enumerate() {
            let existing = lanes
                .iter()
                .position(|l| matches!(l, Some((id, _)) if *id == parent));
            let (target, color) = match existing {
                Some(j) => (j, lanes[j].expect("existing lane").1),
                None if k == 0 && lanes[node_lane].is_none() => {
                    lanes[node_lane] = Some((parent, node_color));
                    (node_lane, node_color)
                }
                None => {
                    let j = free_slot(&mut lanes);
                    let color = new_color();
                    lanes[j] = Some((parent, color));
                    (j, color)
                }
            };
            row.lines.push(Line {
                kind: LineKind::Out,
                lane: target as u16,
                to: target as u16,
                color,
            });
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        max_lanes = max_lanes.max(lanes.len()).max(node_lane + 1);
        rows.push(row);
    }
    (rows, max_lanes)
}

fn free_slot(lanes: &mut Vec<Option<(Oid, u8)>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(n: u8) -> Oid {
        Oid::from_bytes_or_panic(&[n; 20])
    }

    fn commit(n: u8, parents: &[u8], time: i64) -> Commit {
        Commit {
            id: oid(n),
            parents: parents.iter().map(|&p| oid(p)).collect(),
            author: String::new(),
            email: String::new(),
            time,
            commit_time: time,
            summary: format!("c{n}"),
        }
    }

    fn ids(h: &History) -> Vec<String> {
        h.commits.iter().map(|c| c.summary.clone()).collect()
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let commits = Commits {
            list: vec![commit(3, &[2], 30), commit(2, &[1], 20), commit(1, &[], 10)],
            truncated: false,
        };
        let h = History::build(&commits, None);
        assert_eq!(ids(&h), ["c3", "c2", "c1"]);
        assert_eq!(h.lanes, 1);
        assert!(h.rows.iter().all(|r| r.lane == 0));
    }

    #[test]
    fn branch_and_merge_get_two_lanes() {
        // 1 <- 2 <- 4 (merge of 2 and 3); 1 <- 3
        let commits = Commits {
            list: vec![
                commit(4, &[2, 3], 40),
                commit(3, &[1], 30),
                commit(2, &[1], 20),
                commit(1, &[], 10),
            ],
            truncated: false,
        };
        let h = History::build(&commits, None);
        assert_eq!(ids(&h), ["c4", "c3", "c2", "c1"]);
        assert_eq!(h.lanes, 2);
        // The merge has two outgoing lines.
        let outs = h.rows[0]
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Out)
            .count();
        assert_eq!(outs, 2);
        // c3 sits in the second lane, c2 back in the first.
        assert_eq!(h.rows[1].lane, 1);
        assert_eq!(h.rows[2].lane, 0);
        // c2 joins the lane already waiting for c1 instead of opening its own.
        let out = h.rows[2]
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Out)
            .unwrap();
        assert_eq!(out.lane, 1);
        assert_eq!(h.rows[3].lane, 1);
    }

    #[test]
    fn children_come_before_parents_despite_clock_skew() {
        // Child 2 has an older timestamp than its parent 1.
        let commits = Commits {
            list: vec![commit(1, &[], 50), commit(2, &[1], 10)],
            truncated: false,
        };
        let h = History::build(&commits, None);
        assert_eq!(ids(&h), ["c2", "c1"]);
    }

    #[test]
    fn worktree_row_sits_on_top_of_head() {
        let commits = Commits {
            list: vec![commit(2, &[1], 20), commit(1, &[], 10)],
            truncated: false,
        };
        let h = History::build(&commits, Some(oid(2)));
        assert!(h.commits[0].is_worktree());
        assert_eq!(h.commits[1].id, oid(2));
        assert_eq!(h.rows[0].lane, h.rows[1].lane);
    }
}
