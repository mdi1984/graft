//! Words for messages: counts with the right plural.

/// "1 file", "3 files": `n` and `one`, or `many` unless `n` is 1.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    #[test]
    fn plural_is_one_only_for_one() {
        assert_eq!(super::plural(0, "file", "files"), "0 files");
        assert_eq!(super::plural(1, "file", "files"), "1 file");
        assert_eq!(super::plural(2, "branch", "branches"), "2 branches");
    }
}
