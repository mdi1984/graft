//! The order of the sidebar's list sections (an app setting). WORKSPACE
//! stays above them and STASHES below, whatever the order.

/// A sidebar section the user can move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Worktrees,
    Branches,
    Remotes,
    Tags,
}

impl Section {
    /// Today's order, and the one Reset order goes back to.
    pub const DEFAULT: [Self; 4] = [Self::Worktrees, Self::Branches, Self::Remotes, Self::Tags];

    /// The heading the sidebar shows.
    pub fn title(self) -> &'static str {
        match self {
            Self::Worktrees => "WORKTREES",
            Self::Branches => "BRANCHES",
            Self::Remotes => "REMOTES",
            Self::Tags => "TAGS",
        }
    }

    /// The name in the settings file.
    fn saved_name(self) -> &'static str {
        match self {
            Self::Worktrees => "Worktrees",
            Self::Branches => "Branches",
            Self::Remotes => "Remotes",
            Self::Tags => "Tags",
        }
    }

    fn from_saved_name(name: &str) -> Option<Self> {
        Self::DEFAULT
            .into_iter()
            .find(|s| s.saved_name().eq_ignore_ascii_case(name.trim()))
    }
}

/// Every section exactly once, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarOrder([Section; 4]);

impl Default for SidebarOrder {
    fn default() -> Self {
        Self(Section::DEFAULT)
    }
}

impl SidebarOrder {
    /// The sections, top to bottom.
    pub fn sections(&self) -> [Section; 4] {
        self.0
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The order a settings file names: unknown names and repeats are
    /// dropped, missing sections follow in the default order.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut order: Vec<Section> = Vec::with_capacity(4);
        let named = names.into_iter().filter_map(Section::from_saved_name);
        for section in named.chain(Section::DEFAULT) {
            if !order.contains(&section) {
                order.push(section);
            }
        }
        Self(order.try_into().expect("every section exactly once"))
    }

    fn position(&self, section: Section) -> usize {
        self.0
            .iter()
            .position(|s| *s == section)
            .expect("every section is in the order")
    }

    pub fn can_move_up(&self, section: Section) -> bool {
        self.position(section) > 0
    }

    pub fn can_move_down(&self, section: Section) -> bool {
        self.position(section) + 1 < self.0.len()
    }

    /// The order with `section` one place higher (unchanged at the top).
    pub fn moved_up(&self, section: Section) -> Self {
        let at = self.position(section);
        self.moved_to(section, at.saturating_sub(1))
    }

    /// The order with `section` one place lower (unchanged at the bottom).
    pub fn moved_down(&self, section: Section) -> Self {
        let at = self.position(section);
        self.moved_to(section, at + 2)
    }

    /// The order with `section` dropped into `gap`: 0 is above the first
    /// section, 4 below the last, and gap `i` is just above section `i`.
    pub fn moved_to(&self, section: Section, gap: usize) -> Self {
        let mut order = self.0.to_vec();
        let from = self.position(section);
        order.remove(from);
        let gap = gap.min(self.0.len());
        let to = if gap > from { gap - 1 } else { gap };
        order.insert(to, section);
        Self(order.try_into().expect("every section exactly once"))
    }
}

impl serde::Serialize for SidebarOrder {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let names: Vec<&str> = self.0.iter().map(|s| s.saved_name()).collect();
        names.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for SidebarOrder {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(deserializer)?;
        Ok(Self::from_names(names.iter().map(String::as_str)))
    }
}

#[cfg(test)]
mod tests {
    use super::Section::*;
    use super::*;

    fn order(sections: [Section; 4]) -> SidebarOrder {
        SidebarOrder(sections)
    }

    #[test]
    fn the_default_is_todays_order() {
        let default = SidebarOrder::default();
        assert_eq!(default.sections(), [Worktrees, Branches, Remotes, Tags]);
        assert!(default.is_default());
        assert!(!default.moved_down(Worktrees).is_default());
    }

    #[test]
    fn move_up_and_down_stop_at_the_ends() {
        let default = SidebarOrder::default();
        assert!(!default.can_move_up(Worktrees));
        assert!(default.can_move_down(Worktrees));
        assert!(default.can_move_up(Tags));
        assert!(!default.can_move_down(Tags));
        assert_eq!(default.moved_up(Worktrees), default, "already at the top");
        assert_eq!(default.moved_down(Tags), default, "already at the bottom");
        assert_eq!(
            default.moved_up(Remotes).sections(),
            [Worktrees, Remotes, Branches, Tags]
        );
        assert_eq!(
            default.moved_down(Worktrees).sections(),
            [Branches, Worktrees, Remotes, Tags]
        );
    }

    #[test]
    fn dropping_into_a_gap_puts_the_section_there() {
        let default = SidebarOrder::default();
        assert_eq!(
            default.moved_to(Tags, 0).sections(),
            [Tags, Worktrees, Branches, Remotes]
        );
        assert_eq!(
            default.moved_to(Worktrees, 4).sections(),
            [Branches, Remotes, Tags, Worktrees]
        );
        assert_eq!(
            default.moved_to(Worktrees, 2).sections(),
            [Branches, Worktrees, Remotes, Tags]
        );
        assert_eq!(
            default.moved_to(Tags, 1).sections(),
            [Worktrees, Tags, Branches, Remotes]
        );
        // The gaps right above and below a section leave it where it is.
        assert_eq!(default.moved_to(Branches, 1), default);
        assert_eq!(default.moved_to(Branches, 2), default);
        assert_eq!(default.moved_to(Branches, 99).sections()[3], Branches);
    }

    #[test]
    fn reset_goes_back_to_the_default() {
        let moved = order([Tags, Remotes, Branches, Worktrees]);
        assert!(!moved.is_default());
        assert_eq!(SidebarOrder::default().sections(), Section::DEFAULT);
    }

    #[test]
    fn odd_saved_lists_still_name_every_section_once() {
        let from = |names: &[&str]| SidebarOrder::from_names(names.iter().copied()).sections();
        assert_eq!(from(&[]), Section::DEFAULT);
        assert_eq!(
            from(&["Tags", "Remotes"]),
            [Tags, Remotes, Worktrees, Branches],
            "missing sections follow in the default order"
        );
        assert_eq!(
            from(&["Stashes", "Tags", "Workspace", "branches"]),
            [Tags, Branches, Worktrees, Remotes],
            "unknown names are dropped, case does not matter"
        );
        assert_eq!(
            from(&["Tags", "Tags", "Worktrees", "Tags"]),
            [Tags, Worktrees, Branches, Remotes],
            "a repeat counts once"
        );
    }

    #[test]
    fn the_order_survives_saving_and_loading() {
        let moved = order([Remotes, Tags, Worktrees, Branches]);
        let saved = serde_json::to_string(&moved).unwrap();
        assert_eq!(saved, r#"["Remotes","Tags","Worktrees","Branches"]"#);
        let loaded: SidebarOrder = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded, moved);
        let odd: SidebarOrder = serde_json::from_str(r#"["Nope","Tags","Tags"]"#).unwrap();
        assert_eq!(odd.sections(), [Tags, Worktrees, Branches, Remotes]);
    }
}
