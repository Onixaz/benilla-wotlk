//! The shipped character-creation rows, which [`super::customization`]'s load-time guards compare
//! against to catch a misparse.

/// The shipped rows, the load-time guards against a misparse.
pub(super) struct Shipped {
    /// The ChrRaces fileStrings, guarding `load_races`' string columns.
    pub(super) files: &'static [(u8, &'static str)],
    /// The `CharBaseInfo` rows by race, guarding the 2-byte parse: each must be present, and a
    /// patch may add more, as the class-list builder `0x4706b0` lists every row for the race.
    pub(super) combos: &'static [(u8, &'static [u8])],
    /// The Human Warrior male's start outfit, (display, InventoryType), guarding the packed
    /// CharStartOutfit layout.
    pub(super) human_warrior: &'static [(u32, u8)],
}

pub(super) const SHIPPED: Shipped = Shipped {
    files: &[
        (1, "Human"),
        (2, "Orc"),
        (3, "Dwarf"),
        (4, "NightElf"),
        (5, "Scourge"),
        (6, "Tauren"),
        (7, "Gnome"),
        (8, "Troll"),
        (10, "BloodElf"),
        (11, "Draenei"),
    ],
    // Every race offers the Death Knight (6); no dead Dwarf-Mage row.
    combos: &[
        (1, &[1, 2, 4, 5, 6, 8, 9]),
        (2, &[1, 3, 4, 6, 7, 9]),
        (3, &[1, 2, 3, 4, 5, 6]),
        (4, &[1, 3, 4, 5, 6, 11]),
        (5, &[1, 4, 5, 6, 8, 9]),
        (6, &[1, 3, 6, 7, 11]),
        (7, &[1, 4, 6, 8, 9]),
        (8, &[1, 3, 4, 5, 6, 7, 8]),
        (10, &[2, 3, 4, 5, 6, 8, 9]),
        (11, &[1, 2, 3, 5, 6, 7, 8]),
    ],
    // The shirt, pants, boots and a two-handed sword.
    human_warrior: &[(9891, 4), (9892, 7), (10141, 8), (2380, 17)],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_chr_races_files_are_exactly_the_playable_races() {
        let ids: Vec<u8> = SHIPPED.files.iter().map(|&(id, _)| id).collect();
        assert_eq!(ids, super::super::customization::PLAYABLE_RACES);
    }
}
