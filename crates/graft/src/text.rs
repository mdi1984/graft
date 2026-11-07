//! Words for messages: counts with the right plural.

/// "1 file", "3 files": `n` and `word`, with an "s" unless `n` is 1.
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn plural_adds_an_s_unless_one() {
        assert_eq!(super::plural(0, "file"), "0 files");
        assert_eq!(super::plural(1, "file"), "1 file");
        assert_eq!(super::plural(2, "line"), "2 lines");
    }
}
