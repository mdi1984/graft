//! Full details of one commit: message, people, and the files it changed.

use anyhow::{Result, bail};

use crate::changes::FileChange;
use crate::changes::{EMPTY_TREE, between};
use crate::cli::Git;
use crate::model::Oid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub message: String,
    /// Changes against the first parent (or the empty tree for a root commit).
    pub files: Vec<FileChange>,
}

impl CommitDetails {
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }

    /// The message without its first line and the blank line after it.
    pub fn body(&self) -> &str {
        match self.message.split_once('\n') {
            Some((_, rest)) => rest.trim_start_matches(['\n', '\r']).trim_end(),
            None => "",
        }
    }
}

pub fn load(git: &Git, id: Oid) -> Result<CommitDetails> {
    let hex = id.to_string();
    let out = git.run([
        "-c",
        "log.showSignature=false",
        "show",
        "-s",
        "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B",
        &hex,
    ])?;
    let text = out.stdout_str();
    let f: Vec<&str> = text.splitn(9, '\0').collect();
    let [_, parents, an, ae, at, cn, ce, ct, message] = f[..] else {
        bail!("unexpected output from git show");
    };
    let parents: Vec<Oid> = parents
        .split_whitespace()
        .filter_map(|p| Oid::from_hex(p.as_bytes()).ok())
        .collect();
    let base = parents
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| EMPTY_TREE.to_owned());
    let files = between(git, &base, Some(&hex))?;
    Ok(CommitDetails {
        id,
        parents,
        author: an.to_owned(),
        author_email: ae.to_owned(),
        author_time: at.trim().parse().unwrap_or(0),
        committer: cn.to_owned(),
        committer_email: ce.to_owned(),
        commit_time: ct.trim().parse().unwrap_or(0),
        message: message.trim_end().to_owned(),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_and_body() {
        let d = CommitDetails {
            id: Oid::null(gix::hash::Kind::Sha1),
            parents: vec![],
            author: String::new(),
            author_email: String::new(),
            author_time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            message: "Title\n\nBody line\nmore".into(),
            files: vec![],
        };
        assert_eq!(d.summary(), "Title");
        assert_eq!(d.body(), "Body line\nmore");
    }
}
