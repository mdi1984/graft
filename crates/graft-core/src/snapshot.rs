//! Building a [`RepoSnapshot`]: refs, worktrees with their status, remotes, stashes.

use std::path::Path;

use anyhow::Result;

use crate::cli::Git;
use crate::model::{
    Branch, Oid, Remote, RemoteBranch, RepoSnapshot, Tag, Upstream, Worktree, same_path,
};
use crate::repo::native;
use crate::status;

/// Read the whole snapshot. `git` must point at the main worktree.
pub fn load(git: &Git) -> Result<RepoSnapshot> {
    let root = git.cwd().to_path_buf();
    let refs = load_refs(git)?;
    let mut worktrees = load_worktrees(git, &root)?;
    for wt in &mut worktrees {
        if !wt.prunable {
            wt.status = status::load(&git.at(&wt.path)).ok();
        }
    }
    Ok(RepoSnapshot {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string()),
        root,
        worktrees,
        branches: refs.branches,
        remote_branches: refs.remote_branches,
        tags: refs.tags,
        remotes: load_remotes(git)?,
        stash_count: load_stash_count(git)?,
    })
}

#[derive(Default)]
struct Refs {
    branches: Vec<Branch>,
    remote_branches: Vec<RemoteBranch>,
    tags: Vec<Tag>,
}

fn load_refs(git: &Git) -> Result<Refs> {
    let out = git.read([
        "for-each-ref",
        "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(symref)",
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ])?;
    Ok(parse_refs(&out))
}

fn parse_refs(text: &str) -> Refs {
    let mut refs = Refs::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        let [name, oid, peeled, upstream, track, symref] = f[..] else {
            continue;
        };
        if !symref.is_empty() {
            continue; // refs/remotes/origin/HEAD
        }
        let Ok(oid) = Oid::from_hex(oid.as_bytes()) else {
            continue;
        };
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            refs.branches.push(Branch {
                name: branch.to_owned(),
                target: oid,
                upstream: parse_upstream(upstream, track),
            });
        } else if let Some(short) = name.strip_prefix("refs/remotes/") {
            let remote = short.split('/').next().unwrap_or_default().to_owned();
            refs.remote_branches.push(RemoteBranch {
                name: short.to_owned(),
                remote,
                target: oid,
            });
        } else if let Some(tag) = name.strip_prefix("refs/tags/") {
            let peeled_oid = Oid::from_hex(peeled.as_bytes()).ok();
            refs.tags.push(Tag {
                name: tag.to_owned(),
                target: peeled_oid.unwrap_or(oid),
                annotated: peeled_oid.is_some(),
            });
        }
    }
    refs
}

fn parse_upstream(upstream: &str, track: &str) -> Upstream {
    if upstream.is_empty() {
        return Upstream::None;
    }
    if track == "gone" {
        return Upstream::Gone {
            name: upstream.to_owned(),
        };
    }
    let (mut ahead, mut behind) = (0, 0);
    for part in track.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    Upstream::Tracking {
        name: upstream.to_owned(),
        ahead,
        behind,
    }
}

fn load_worktrees(git: &Git, root: &Path) -> Result<Vec<Worktree>> {
    let out = git.run(["worktree", "list", "--porcelain", "-z"])?;
    Ok(parse_worktrees(&out.stdout, root))
}

fn parse_worktrees(bytes: &[u8], root: &Path) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in bytes.split(|&b| b == 0) {
        let field = String::from_utf8_lossy(field);
        if field.is_empty() {
            // Blank field ends a record.
            list.extend(current.take());
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((&field, ""));
        match key {
            "worktree" => {
                list.extend(current.take());
                let path = native(value);
                current = Some(Worktree {
                    is_main: same_path(&path, root),
                    path,
                    branch: None,
                    head: None,
                    locked: false,
                    prunable: false,
                    status: None,
                });
            }
            "HEAD" => {
                if let Some(wt) = &mut current {
                    wt.head = Oid::from_hex(value.as_bytes())
                        .ok()
                        .filter(|o| !o.is_null());
                }
            }
            "branch" => {
                if let Some(wt) = &mut current {
                    wt.branch = Some(value.trim_start_matches("refs/heads/").to_owned());
                }
            }
            "locked" => {
                if let Some(wt) = &mut current {
                    wt.locked = true;
                }
            }
            "prunable" => {
                if let Some(wt) = &mut current {
                    wt.prunable = true;
                }
            }
            _ => {}
        }
    }
    list.extend(current);
    // `git worktree list` puts the main worktree first; keep it that way even if
    // path comparison failed.
    if !list.iter().any(|wt| wt.is_main)
        && let Some(first) = list.first_mut()
    {
        first.is_main = true;
    }
    list
}

fn load_remotes(git: &Git) -> Result<Vec<Remote>> {
    let out = git.read(["remote", "-v"])?;
    let mut remotes: Vec<Remote> = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let remote = match remotes.iter_mut().find(|r| r.name == name) {
            Some(r) => r,
            None => {
                remotes.push(Remote {
                    name: name.to_owned(),
                    fetch_url: String::new(),
                    push_url: String::new(),
                });
                remotes.last_mut().expect("just pushed")
            }
        };
        if kind == "(push)" {
            remote.push_url = url.to_owned();
        } else {
            remote.fetch_url = url.to_owned();
        }
    }
    Ok(remotes)
}

fn load_stash_count(git: &Git) -> Result<usize> {
    let out = git.read(["stash", "list", "--format=%H"])?;
    Ok(out.lines().filter(|l| !l.is_empty()).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_and_tracking() {
        let oid = "1111111111111111111111111111111111111111";
        let peeled = "2222222222222222222222222222222222222222";
        let text = format!(
            "refs/heads/main\0{oid}\0\0origin/main\0ahead 2, behind 1\0\n\
             refs/heads/old\0{oid}\0\0origin/old\0gone\0\n\
             refs/heads/local\0{oid}\0\0\0\0\n\
             refs/remotes/origin/HEAD\0{oid}\0\0\0\0refs/remotes/origin/main\n\
             refs/remotes/origin/feature/x\0{oid}\0\0\0\0\n\
             refs/tags/v1\0{oid}\0{peeled}\0\0\0\n"
        );
        let refs = parse_refs(&text);
        assert_eq!(refs.branches.len(), 3);
        assert_eq!(
            refs.branches[0].upstream,
            Upstream::Tracking {
                name: "origin/main".into(),
                ahead: 2,
                behind: 1
            }
        );
        assert!(matches!(refs.branches[1].upstream, Upstream::Gone { .. }));
        assert_eq!(refs.branches[2].upstream, Upstream::None);
        assert_eq!(refs.remote_branches.len(), 1);
        assert_eq!(refs.remote_branches[0].branch_name(), "feature/x");
        assert!(refs.tags[0].annotated);
        assert_eq!(refs.tags[0].target.to_string(), peeled);
    }

    #[test]
    fn worktree_records() {
        let raw = b"worktree C:/dev/graft\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0\
worktree C:/dev/graft.wt/x\0HEAD 2222222222222222222222222222222222222222\0detached\0locked\0\0\
worktree C:/gone\0HEAD 3333333333333333333333333333333333333333\0branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0";
        let wts = parse_worktrees(raw, Path::new(r"C:\dev\graft"));
        assert_eq!(wts.len(), 3);
        assert!(wts[0].is_main);
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert!(wts[1].branch.is_none() && wts[1].locked);
        assert!(wts[2].prunable);
    }
}
