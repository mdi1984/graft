//! Telling git's refusals apart by their message, so Graft can say what to
//! do about one. The wording is stable because every command runs with
//! `LC_ALL=C` (see [`crate::Git`]); a git release that rewords one breaks
//! only the matcher here.

/// `branch -d` refused to delete a branch that is not merged.
pub fn not_merged(error: &str) -> bool {
    error.contains("not fully merged")
}

/// The folder is not inside a git repository.
pub(crate) fn not_a_repository(error: &str) -> bool {
    error.contains("not a git repository")
}

/// `stash push` found nothing to stash; `output` is its stdout.
pub(crate) fn nothing_to_stash(output: &str) -> bool {
    output.contains("No local changes to save")
}

/// Git refused to switch branches because local changes would be lost.
pub fn blocked_by_changes(error: &str) -> bool {
    error.contains("would be overwritten by checkout")
        || error.contains("Please commit your changes or stash them")
}

/// Among the changes in the way ([`blocked_by_changes`]) are untracked files.
pub fn blocked_by_untracked(error: &str) -> bool {
    error.contains("untracked working tree files")
}

/// The files git lists between "…would be overwritten by checkout:" and
/// "Please …", joined for a sentence.
pub fn files_in_the_way(error: &str) -> String {
    let files: Vec<&str> = error
        .lines()
        .skip_while(|l| !l.contains("would be overwritten"))
        .skip(1)
        .map(str::trim)
        .take_while(|l| !l.starts_with("Please") && !l.starts_with("Aborting"))
        .filter(|l| !l.is_empty())
        .collect();
    match files.len() {
        0 => "some files".to_owned(),
        1..=5 => files.join(", "),
        n => format!("{} and {} more", files[..4].join(", "), n - 4),
    }
}

/// A push or pull failed because the repository has no remote to use.
pub fn no_remote(error: &str) -> bool {
    error.contains("No configured push destination")
        || error.contains("No remote repository specified")
}

/// A commit failed because git does not know the user's name and email.
pub fn no_identity(error: &str) -> bool {
    error.contains("Please tell me who you are") || error.contains("Author identity unknown")
}

/// A push failed because the remote has commits we don't.
pub fn rejected_push(error: &str) -> bool {
    error.contains("[rejected]")
        || error.contains("non-fast-forward")
        || error.contains("fetch first")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_in_the_way_of_a_checkout() {
        let error = "Your local changes to the following files would be overwritten by checkout:\n\
                     \ta.txt\n\tb.txt\nPlease commit your changes or stash them before you switch branches.\nAborting";
        assert!(blocked_by_changes(error));
        assert!(!blocked_by_untracked(error));
        assert_eq!(files_in_the_way(error), "a.txt, b.txt");
        assert_eq!(files_in_the_way("fatal: something else"), "some files");
    }
}
