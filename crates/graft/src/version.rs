//! Graft's version numbers, `major.minor.patch`, compared by number.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u64, u64, u64);

impl Version {
    /// `major.minor.patch`; anything else (pre-releases included) is not a
    /// version.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('.').map(|p| p.parse::<u64>().ok());
        let v = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(v)
    }
}

/// Is `candidate` a version above `current`? Not if either is no version.
pub fn newer(candidate: &str, current: &str) -> bool {
    match (Version::parse(candidate), Version::parse(current)) {
        (Some(c), Some(cur)) => c > cur,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("1.0.1", "1.0.0"));
        assert!(newer("1.0.10", "1.0.9"));
        assert!(newer("1.10.0", "1.9.3"));
        assert!(!newer("1.0.0", "1.0.0"));
        assert!(!newer("0.9.9", "1.0.0"));
        assert!(!newer("2.0.0-beta", "1.0.0"));
        assert!(!newer("2.0", "1.0.0"));
        assert!(!newer("2.0.0", "garbage"));
    }
}
