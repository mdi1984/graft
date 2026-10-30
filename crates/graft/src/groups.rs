//! Tab groups: named, colored sets of repositories, e.g. one per customer.
//! A group outlives its tabs: closing them keeps the group, and the start
//! page opens all of its repositories again at once.
//!
//! The tabs of a group always sit next to each other, after the group's
//! label. A collapsed group shows only its label (and the active tab, if
//! that is one of its own).

use std::path::{Path, PathBuf};

use graft_core::same_path;
use serde::{Deserialize, Serialize};

pub type GroupId = u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: GroupId,
    pub name: String,
    /// Index into the group colors (`theme::color::lanes`).
    pub color: usize,
    pub collapsed: bool,
    /// Repository roots, open or not.
    pub repos: Vec<PathBuf>,
}

/// The group `root` belongs to.
pub fn group_of(groups: &[TabGroup], root: &Path) -> Option<GroupId> {
    groups
        .iter()
        .find(|g| g.repos.iter().any(|r| same_path(r, root)))
        .map(|g| g.id)
}

/// Put `root` into `group` (or into none), out of any other. Groups left
/// without repositories are dropped.
pub fn set_group(groups: &mut Vec<TabGroup>, root: &Path, group: Option<GroupId>) {
    if group_of(groups, root) == group {
        return;
    }
    for g in groups.iter_mut() {
        g.repos.retain(|r| !same_path(r, root));
    }
    if let Some(g) = groups.iter_mut().find(|g| Some(g.id) == group) {
        g.repos.push(root.to_path_buf());
    }
    groups.retain(|g| !g.repos.is_empty());
}

/// A fresh id and the first color no group uses yet.
pub fn next_id_and_color(groups: &[TabGroup], colors: usize) -> (GroupId, usize) {
    let id = groups.iter().map(|g| g.id).max().map_or(1, |m| m + 1);
    let color = (0..colors)
        .find(|c| groups.iter().all(|g| g.color != *c))
        .unwrap_or(id as usize % colors.max(1));
    (id, color)
}

/// The tab order with each group's tabs together, where its first tab is,
/// in the group's own order (`rank`, e.g. the place in [`TabGroup::repos`]).
/// `tabs` holds the group of every tab; the result lists old indexes.
pub fn contiguous_order(tabs: &[Option<GroupId>], rank: impl Fn(usize) -> usize) -> Vec<usize> {
    let mut order = Vec::with_capacity(tabs.len());
    let mut placed = vec![false; tabs.len()];
    for i in 0..tabs.len() {
        if placed[i] {
            continue;
        }
        match tabs[i] {
            None => {
                placed[i] = true;
                order.push(i);
            }
            Some(g) => {
                let mut members: Vec<usize> =
                    (i..tabs.len()).filter(|&j| tabs[j] == Some(g)).collect();
                members.sort_by_key(|&j| rank(j));
                for &j in &members {
                    placed[j] = true;
                }
                order.extend(members);
            }
        }
    }
    order
}

/// Make each group's order follow its open tabs (`roots`, in tab order);
/// repositories that aren't open keep their place after them.
pub fn follow_tabs(groups: &mut [TabGroup], roots: &[PathBuf]) {
    for group in groups {
        let rank = |r: &PathBuf| {
            roots
                .iter()
                .position(|o| same_path(o, r))
                .unwrap_or(usize::MAX)
        };
        group.repos.sort_by_key(rank);
    }
}

/// What the tab strip shows, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Label(GroupId),
    Tab(usize),
}

/// The strip's items for tabs in contiguous order (see [`contiguous_order`]).
pub fn visible(
    tabs: &[Option<GroupId>],
    collapsed: impl Fn(GroupId) -> bool,
    active: Option<usize>,
) -> Vec<Item> {
    let mut items = Vec::with_capacity(tabs.len());
    for (i, group) in tabs.iter().enumerate() {
        match *group {
            None => items.push(Item::Tab(i)),
            Some(g) => {
                if i == 0 || tabs[i - 1] != Some(g) {
                    items.push(Item::Label(g));
                }
                if !collapsed(g) || active == Some(i) {
                    items.push(Item::Tab(i));
                }
            }
        }
    }
    items
}

/// A dragged tab's next place: move it to `to` (as `Vec::remove` then
/// `insert`) and give it `group`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub to: usize,
    pub group: Option<GroupId>,
}

/// Where tab `from` goes when dragged one place left or right. Past the end
/// of its group it leaves the group; onto an open group it joins it; past a
/// collapsed group it jumps over the whole group.
pub fn drag_step(
    tabs: &[Option<GroupId>],
    items: &[Item],
    collapsed: impl Fn(GroupId) -> bool,
    from: usize,
    right: bool,
) -> Option<Step> {
    let pos = items.iter().position(|it| *it == Item::Tab(from))?;
    let first = |g| tabs.iter().position(|t| *t == Some(g));
    let last = |g| tabs.iter().rposition(|t| *t == Some(g));
    let neighbour = if right {
        items.get(pos + 1).copied()
    } else {
        pos.checked_sub(1).map(|p| items[p])
    };
    let own = tabs[from];
    if let Some(g) = own {
        return match neighbour {
            Some(Item::Tab(n)) if tabs[n] == Some(g) => Some(Step {
                to: n,
                group: Some(g),
            }),
            // At the end of its group: step out of it, past any of its
            // tabs that a collapsed group hides.
            _ => Some(Step {
                to: if right { last(g)? } else { first(g)? },
                group: None,
            }),
        };
    }
    match neighbour? {
        Item::Tab(n) => match tabs[n] {
            None => Some(Step { to: n, group: None }),
            // The last tab of an open group, on the left: join at its end.
            Some(h) => Some(Step {
                to: from,
                group: Some(h),
            }),
        },
        Item::Label(h) if collapsed(h) => Some(Step {
            to: if right { last(h)? } else { first(h)? },
            group: None,
        }),
        // An open group's label, on the right: join at its start.
        Item::Label(h) => Some(Step {
            to: from,
            group: Some(h),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Option<GroupId> = Some(1);
    const B: Option<GroupId> = Some(2);

    fn group(id: GroupId, repos: &[&str]) -> TabGroup {
        TabGroup {
            id,
            name: format!("g{id}"),
            color: 0,
            collapsed: false,
            repos: repos.iter().map(PathBuf::from).collect(),
        }
    }

    fn moved(tabs: &[Option<GroupId>], from: usize, step: Step) -> Vec<Option<GroupId>> {
        let mut tabs = tabs.to_vec();
        tabs.remove(from);
        tabs.insert(step.to, step.group);
        tabs
    }

    #[test]
    fn a_group_gathers_its_tabs_where_its_first_one_is() {
        let as_listed = |i| i;
        assert_eq!(
            contiguous_order(&[A, None, B, A, None, B], as_listed),
            [0, 3, 1, 2, 5, 4]
        );
        assert_eq!(contiguous_order(&[None, None], as_listed), [0, 1]);
        // Tabs that opened in any order line up in the group's order.
        let rank = |i| [2, 0, 1][i];
        assert_eq!(contiguous_order(&[A, A, A], rank), [1, 2, 0]);
    }

    #[test]
    fn a_group_remembers_the_order_of_its_tabs() {
        let mut groups = vec![group(1, &["a", "b", "closed", "c"])];
        let roots: Vec<PathBuf> = ["x", "c", "a", "b"].iter().map(PathBuf::from).collect();
        follow_tabs(&mut groups, &roots);
        let names: Vec<_> = groups[0]
            .repos
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect();
        assert_eq!(names, ["c", "a", "b", "closed"]);
    }

    #[test]
    fn collapsed_groups_show_only_their_label_and_the_active_tab() {
        let tabs = [None, A, A, B, B];
        let open = visible(&tabs, |_| false, None);
        use Item::*;
        assert_eq!(
            open,
            [Tab(0), Label(1), Tab(1), Tab(2), Label(2), Tab(3), Tab(4)]
        );
        let shut = visible(&tabs, |g| g == 1, Some(2));
        assert_eq!(shut, [Tab(0), Label(1), Tab(2), Label(2), Tab(3), Tab(4)]);
    }

    #[test]
    fn dragging_moves_within_a_group_and_steps_out_at_its_ends() {
        let tabs = [None, A, A, None];
        let items = visible(&tabs, |_| false, Some(1));
        // Right within the group: swap.
        let step = drag_step(&tabs, &items, |_| false, 1, true).unwrap();
        assert_eq!(step, Step { to: 2, group: A });
        // Left at its first tab: leave the group, still before the others.
        let step = drag_step(&tabs, &items, |_| false, 1, false).unwrap();
        assert_eq!(step, Step { to: 1, group: None });
        assert_eq!(moved(&tabs, 1, step), [None, None, A, None]);
        // Right at its last tab: leave, staying where it is.
        let step = drag_step(&tabs, &items, |_| false, 2, true).unwrap();
        assert_eq!(moved(&tabs, 2, step), [None, A, None, None]);
    }

    #[test]
    fn dragging_onto_an_open_group_joins_it() {
        let tabs = [None, A, A, None];
        let items = visible(&tabs, |_| false, Some(0));
        let step = drag_step(&tabs, &items, |_| false, 0, true).unwrap();
        assert_eq!(moved(&tabs, 0, step), [A, A, A, None]);
        let step = drag_step(&tabs, &items, |_| false, 3, false).unwrap();
        assert_eq!(moved(&tabs, 3, step), [None, A, A, A]);
    }

    #[test]
    fn dragging_past_a_collapsed_group_jumps_over_it() {
        let tabs = [None, A, A, None];
        let shut = |g| g == 1;
        let items = visible(&tabs, shut, Some(0));
        let step = drag_step(&tabs, &items, shut, 0, true).unwrap();
        assert_eq!(step, Step { to: 2, group: None });
        assert_eq!(moved(&tabs, 0, step), [A, A, None, None]);
        let items = visible(&tabs, shut, Some(3));
        let step = drag_step(&tabs, &items, shut, 3, false).unwrap();
        assert_eq!(moved(&tabs, 3, step), [None, None, A, A]);
    }

    #[test]
    fn the_visible_tab_of_a_collapsed_group_leaves_past_its_hidden_ones() {
        let tabs = [A, A, A, None];
        let shut = |g| g == 1;
        let items = visible(&tabs, shut, Some(0));
        let step = drag_step(&tabs, &items, shut, 0, true).unwrap();
        assert_eq!(moved(&tabs, 0, step), [A, A, None, None]);
    }

    #[test]
    fn nothing_past_the_ends() {
        let tabs = [None, None];
        let items = visible(&tabs, |_| false, Some(0));
        assert_eq!(drag_step(&tabs, &items, |_| false, 0, false), None);
        assert_eq!(drag_step(&tabs, &items, |_| false, 1, true), None);
    }

    #[test]
    fn membership_moves_between_groups_and_empty_groups_go() {
        let mut groups = vec![group(1, &[r"C:\a", r"C:\b"]), group(2, &[r"C:\c"])];
        assert_eq!(group_of(&groups, Path::new(r"c:\A\")), Some(1));
        set_group(&mut groups, Path::new(r"C:\c"), Some(1));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].repos.len(), 3);
        set_group(&mut groups, Path::new(r"C:\a"), None);
        assert_eq!(group_of(&groups, Path::new(r"C:\a")), None);
    }

    #[test]
    fn new_groups_take_an_unused_color() {
        let groups = vec![group(4, &["a"]), {
            let mut g = group(7, &["b"]);
            g.color = 1;
            g
        }];
        assert_eq!(next_id_and_color(&groups, 8), (8, 2));
        assert_eq!(next_id_and_color(&[], 8), (1, 0));
    }
}
