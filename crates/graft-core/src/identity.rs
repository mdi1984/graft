//! The name and email git records on commits (`user.name`, `user.email`),
//! for all repositories or just one.

use anyhow::Result;

use crate::cli::Git;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub email: String,
    /// Set for this repository only (overrides the global values).
    pub local: bool,
}

/// A config value; empty when not set.
fn get(git: &Git, args: &[&str]) -> Result<String> {
    Ok(git.config(args)?.unwrap_or_default())
}

/// `git config --unset` exits with 5 when the key is not set.
fn unset(git: &Git, scope: &str, key: &str) -> Result<()> {
    git.run_accepting(["config", scope, "--unset", key], None, |o| {
        o.status == 0 || o.status == 5
    })?;
    Ok(())
}

/// The identity commits in this repository get.
pub fn load(git: &Git) -> Result<Identity> {
    Ok(Identity {
        name: get(git, &["user.name"])?,
        email: get(git, &["user.email"])?,
        local: !get(git, &["--local", "user.name"])?.is_empty()
            || !get(git, &["--local", "user.email"])?.is_empty(),
    })
}

/// Save for this repository (`local`) or for all of them. Saving globally
/// removes this repository's own values, so the global ones apply.
pub fn save(git: &Git, identity: &Identity) -> Result<()> {
    let scope = if identity.local {
        "--local"
    } else {
        "--global"
    };
    for (key, value) in [
        ("user.name", identity.name.trim()),
        ("user.email", identity.email.trim()),
    ] {
        if value.is_empty() {
            unset(git, scope, key)?;
        } else {
            git.run(["config", scope, key, value])?;
        }
        if !identity.local {
            unset(git, "--local", key)?;
        }
    }
    Ok(())
}
