//! The `/who` sort (`SortWho` `0x6b4eb0`, comparator `0x6b4c80`): a chain of seven `{key, dir}`
//! slots (`0xc79f9c`/`0xc79fa0`), seeded zone, level, class, group, name, race, guild, all
//! ascending (`0x6b8a64`-`0x6b8a8e`). A click promotes its key to the front and the comparator
//! walks the chain as tie-breakers, so earlier clicks still order ties. The direction flips only
//! when the key was already in front (`0x6b4fb3`-`0x6b4fc5`); promoted from behind, it keeps its
//! old direction. Sorting is local, never a server request, and the friend list's setup reseeds the
//! chain at every world entry (`0x405653`).

use std::cmp::Ordering;
use std::iter::Peekable;
use std::str::Chars;

use super::social::WhoInfo;

/// The seven keys `SortWho` maps its argument to, declared in the reference's numbering
/// (`0x6b4edf`-`0x6b4f96`), which is also the chain's seeded order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhoSortKey {
    /// `0`, `"zone"`, the frame's resting column.
    Zone,
    /// `1`, `"level"`, the one numeric arm: ascending puts the lower level first.
    Level,
    /// `2`, `"class"`.
    Class,
    /// `3`, `"group"`: its jump-table slot is the loop's continue (`0x6b4e57`), but promoting it
    /// still displaces the front key. The stock Who frame has no header for it.
    Group,
    /// `4`, `"name"`, and the answer for any other string (preloaded at `0x6b4edf`).
    #[default]
    Name,
    /// `5`, `"race"`.
    Race,
    /// `6`, `"guild"`.
    Guild,
}

impl WhoSortKey {
    /// Every key, in the chain's seeded order.
    const ALL: [Self; 7] = [
        Self::Zone,
        Self::Level,
        Self::Class,
        Self::Group,
        Self::Name,
        Self::Race,
        Self::Guild,
    ];

    /// `SortWho`'s argument to its key: seven ASCII case-insensitive compares (`0x76e780`),
    /// [`Self::Name`] when none match.
    pub fn from_sort_type(sort_type: &str) -> Self {
        match () {
            _ if sort_type.eq_ignore_ascii_case("name") => Self::Name,
            _ if sort_type.eq_ignore_ascii_case("level") => Self::Level,
            _ if sort_type.eq_ignore_ascii_case("class") => Self::Class,
            _ if sort_type.eq_ignore_ascii_case("group") => Self::Group,
            _ if sort_type.eq_ignore_ascii_case("race") => Self::Race,
            _ if sort_type.eq_ignore_ascii_case("zone") => Self::Zone,
            _ if sort_type.eq_ignore_ascii_case("guild") => Self::Guild,
            _ => Self::Name,
        }
    }

    /// This key's ascending order of two rows, the jump-table arms at `0x6b4e8c`.
    fn order(self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        let records = |x: &Option<String>, y: &Option<String>| match (x, y) {
            // A missing record on either side passes to the next key (`0x6b4d5e`).
            (Some(x), Some(y)) => sort_utf8i(x, y),
            _ => Ordering::Equal,
        };
        match self {
            Self::Name => cmp_utf8i(&a.name, &b.name),
            Self::Guild => sort_utf8i(&a.guild, &b.guild),
            Self::Level => a.level.cmp(&b.level),
            Self::Class => records(&a.records.class, &b.records.class),
            Self::Race => records(&a.records.race, &b.records.race),
            Self::Zone => records(&a.records.zone, &b.records.zone),
            Self::Group => Ordering::Equal,
        }
    }
}

/// The seven-slot chain: keys most recently used first, each with the direction it was left in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoSortChain {
    /// `(key, reversed)`, slot 0 first; always a permutation of all seven keys.
    slots: [(WhoSortKey, bool); 7],
}

impl Default for WhoSortChain {
    /// The seed `0x6b8a64`-`0x6b8a8e`: `key[i] = i`, `dir[i] = 0`.
    fn default() -> Self {
        Self {
            slots: WhoSortKey::ALL.map(|key| (key, false)),
        }
    }
}

impl WhoSortChain {
    /// `SortWho`'s promote (`0x6b4f9b`-`0x6b4fef`): move the key to the front, flipping its
    /// direction only if it was already there; the keys it passes shift down one slot.
    pub fn promote(&mut self, sort_type: &str) {
        let key = WhoSortKey::from_sort_type(sort_type);
        let Some(at) = self.slots.iter().position(|(k, _)| *k == key) else {
            // Unreachable; the reference's not-found edge skips the promote and sorts (`0x6b4fb1`).
            return;
        };
        let dir = if at == 0 {
            !self.slots[0].1
        } else {
            self.slots[at].1
        };
        self.slots[..=at].rotate_right(1);
        self.slots[0] = (key, dir);
    }

    /// The comparator (`0x6b4c80`): the first key that does not tie, negated when its slot is
    /// reversed (`0x6b4e6a`), else `Equal`.
    pub fn compare(&self, a: &WhoInfo, b: &WhoInfo) -> Ordering {
        for (key, reversed) in self.slots {
            let order = key.order(a, b);
            if order != Ordering::Equal {
                return if reversed { order.reverse() } else { order };
            }
        }
        Ordering::Equal
    }

    /// Order `rows` by the chain, as the reference's `qsort` does on a click (`0x6b500b`) and on
    /// each `SMSG_WHO` answer (`0x6b88fe`). An insertion sort over the 50 rows at most: like
    /// `qsort` it cannot fail on the comparator's ties between a missing record and a present one,
    /// which are not a total order.
    pub fn sort(&self, rows: &mut [WhoInfo]) {
        for i in 1..rows.len() {
            let mut j = i;
            while j > 0 && self.compare(&rows[j - 1], &rows[j]) == Ordering::Greater {
                rows.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    #[cfg(test)]
    fn keys(&self) -> [(WhoSortKey, bool); 7] {
        self.slots
    }
}

/// A code point as Storm reads it: past the Basic Multilingual Plane, U+FFFD.
fn code(c: char) -> u32 {
    match u32::from(c) {
        c @ 0..=0xFFFF => c,
        _ => 0xFFFD,
    }
}

/// The UTF-8 case-insensitive compare's fold (`0x76e8d0`): ASCII and Latin-1 lower case up, `œ` to
/// `Œ`, and the Cyrillic letters folded so `Ё` and `ё` compare as `Е`.
fn cmp_upper(c: u32) -> u32 {
    match c {
        0x61..=0x7A | 0xE0..=0xFE => c - 32,
        0x153 => 0x152,
        0x401 | 0x451 => 0x415,
        0x410..=0x415 => c - 1,
        0x430..=0x44F => c - u32::from(c <= 0x435) - 32,
        _ => c,
    }
}

/// The UTF-8 case-insensitive compare, the name arm: folded code points in order, a shorter string
/// first.
pub(crate) fn cmp_utf8i(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.chars().map(|c| cmp_upper(code(c))).collect::<Vec<_>>();
    fold(a).cmp(&fold(b))
}

/// The UTF-8 sort compare's fold (`0x76e7a0`): ASCII, Latin-1 and Cyrillic lower case up, `œ` to
/// `Œ`.
fn sort_upper(c: u32) -> u32 {
    match c {
        0x61..=0x7A | 0xE0..=0xFE | 0x430..=0x44F => c - 32,
        0x153 => 0x152,
        0x451 => 0x401,
        _ => c,
    }
}

/// The sort letter (`0x76eab0`): accented Latin capitals to their base letter, `Ё` to `Е`.
fn sort_char(upper: u32) -> u32 {
    match upper {
        0xC0..=0xC6 => 'A' as u32,
        0xC7 => 'C' as u32,
        0xC8..=0xCB => 'E' as u32,
        0xCC..=0xCF => 'I' as u32,
        0xD1 => 'N' as u32,
        0xD2..=0xD6 | 0x152 => 'O' as u32,
        0xD9..=0xDC => 'U' as u32,
        0xDF => 'S' as u32,
        0x401 => 0x415,
        _ => upper,
    }
}

/// The sort-letter compare (`0x76eb80`): two letters match on their base letter, and a ligature
/// (`Æ`, `ß`, `Œ`) also takes the other string's next `E`, `S` or `E`.
fn sort_chars_match(
    upper1: u32,
    upper2: u32,
    s1: &mut Peekable<Chars<'_>>,
    s2: &mut Peekable<Chars<'_>>,
) -> bool {
    if sort_char(upper1) != sort_char(upper2) {
        return false;
    }
    if upper1 == upper2 {
        return true;
    }
    let takes = |other: &mut Peekable<Chars<'_>>, second: char| {
        let next = other.peek().map_or(0, |&c| sort_char(sort_upper(code(c))));
        next == second as u32 && other.next().is_some()
    };
    match (upper1, upper2) {
        (0xC6, _) => takes(s2, 'E'),
        (_, 0xC6) => takes(s1, 'E'),
        (0xDF, _) => takes(s2, 'S'),
        (_, 0xDF) => takes(s1, 'S'),
        (0x152, _) => takes(s2, 'E'),
        (_, 0x152) => takes(s1, 'E'),
        _ => true,
    }
}

/// The UTF-8 sort compare (`0x76ec80`), the guild, class, race and zone arms: by base letter, and
/// two strings equal so far by the first differing raw code point.
pub(crate) fn sort_utf8i(a: &str, b: &str) -> Ordering {
    let (mut s1, mut s2) = (a.chars().peekable(), b.chars().peekable());
    let mut tie = Ordering::Equal;
    loop {
        let (c1, c2) = match (s1.next(), s2.next()) {
            (None, None) => return tie,
            (x, y) => (x.map_or(0, code), y.map_or(0, code)),
        };
        let (upper1, upper2) = (sort_upper(c1), sort_upper(c2));
        if !sort_chars_match(upper1, upper2, &mut s1, &mut s2) {
            return sort_char(upper1).cmp(&sort_char(upper2));
        }
        if tie == Ordering::Equal {
            tie = c1.cmp(&c2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::social::WhoRecords;
    use super::*;

    fn row(name: &str, level: u32, class: &str, zone: &str) -> WhoInfo {
        let record = |s: &str| (!s.is_empty()).then(|| s.to_string());
        WhoInfo {
            name: name.to_string(),
            level,
            race: "Human".to_string(),
            class: class.to_string(),
            zone: zone.to_string(),
            records: WhoRecords {
                race: Some("Human".to_string()),
                class: record(class),
                zone: record(zone),
            },
            ..Default::default()
        }
    }

    fn names(rows: &[WhoInfo]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn the_chain_is_seeded_in_key_order() {
        assert_eq!(
            WhoSortChain::default().keys(),
            [
                (WhoSortKey::Zone, false),
                (WhoSortKey::Level, false),
                (WhoSortKey::Class, false),
                (WhoSortKey::Group, false),
                (WhoSortKey::Name, false),
                (WhoSortKey::Race, false),
                (WhoSortKey::Guild, false),
            ]
        );
    }

    #[test]
    fn the_sort_type_maps_case_insensitively_and_falls_back_to_name() {
        for (arg, key) in [
            ("zone", WhoSortKey::Zone),
            ("level", WhoSortKey::Level),
            ("class", WhoSortKey::Class),
            ("group", WhoSortKey::Group),
            ("name", WhoSortKey::Name),
            ("race", WhoSortKey::Race),
            ("guild", WhoSortKey::Guild),
            ("LEVEL", WhoSortKey::Level),
            ("Guild", WhoSortKey::Guild),
        ] {
            assert_eq!(WhoSortKey::from_sort_type(arg), key, "{arg}");
        }
        for arg in ["", "sortme", "5", "levels"] {
            assert_eq!(
                WhoSortKey::from_sort_type(arg),
                WhoSortKey::Name,
                "{arg} must fall back to name"
            );
        }
    }

    #[test]
    fn a_repeated_click_reverses() {
        let mut rows = vec![
            row("Galas", 60, "Warrior", "Elwynn Forest"),
            row("Erdrin", 60, "Warrior", "Elwynn Forest"),
        ];
        let mut chain = WhoSortChain::default();

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Erdrin", "Galas"], "first click ascends");

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Galas", "Erdrin"], "the repeat reverses");

        chain.promote("name");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Erdrin", "Galas"], "and back again");
    }

    #[test]
    fn promoting_from_behind_carries_the_direction_it_left() {
        let mut chain = WhoSortChain::default();
        chain.promote("name"); // ascending
        chain.promote("name"); // descending
        chain.promote("level"); // name drops to slot 1, still descending
        assert_eq!(chain.keys()[1], (WhoSortKey::Name, true));

        chain.promote("name"); // back to the front, not a flip
        assert_eq!(chain.keys()[0], (WhoSortKey::Name, true));

        let mut rows = vec![
            row("Aaa", 1, "Mage", "Durotar"),
            row("Bbb", 1, "Mage", "Durotar"),
        ];
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Bbb", "Aaa"], "still descending");
    }

    #[test]
    fn earlier_clicks_survive_as_tie_breakers() {
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.promote("class");
        assert_eq!(
            chain.keys()[..2],
            [(WhoSortKey::Class, false), (WhoSortKey::Level, false)]
        );

        let mut rows = vec![
            row("Ca", 40, "Rogue", "Westfall"),
            row("Aa", 60, "Mage", "Westfall"),
            row("Ba", 20, "Rogue", "Westfall"),
        ];
        chain.sort(&mut rows);
        assert_eq!(
            names(&rows),
            ["Aa", "Ba", "Ca"],
            "Mage before Rogue; the two Rogues in the level order the earlier click gave them"
        );
    }

    #[test]
    fn level_ascends_before_it_reverses() {
        let mut rows = vec![row("High", 60, "Mage", "Z"), row("Low", 12, "Mage", "Z")];
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Low", "High"]);
        chain.promote("level");
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["High", "Low"]);
    }

    /// The class arm reads the record's `Name_lang`, so a row sorts on the same word whatever
    /// gendered name it shows.
    #[test]
    fn the_record_arms_order_by_the_record_name_not_the_shown_one() {
        let mut chain = WhoSortChain::default();
        chain.promote("class");
        let mut a = row("Aaa", 1, "Warrior", "Z");
        a.class = "Zzz".into();
        let b = row("Bbb", 1, "Mage", "Z");
        let mut rows = vec![a, b];
        chain.sort(&mut rows);
        assert_eq!(names(&rows), ["Bbb", "Aaa"], "Mage before Warrior");
    }

    /// The name arm is the UTF-8 case-insensitive compare: case folded over ASCII and Latin-1, then
    /// by code point.
    #[test]
    fn the_name_arm_folds_latin_1_case() {
        assert_eq!(cmp_utf8i("Apple", "apple"), Ordering::Equal);
        assert_eq!(cmp_utf8i("Zoë", "ZOË"), Ordering::Equal);
        assert_eq!(cmp_utf8i("apple", "Zed"), Ordering::Less);
        assert_eq!(cmp_utf8i("Ann", "Anne"), Ordering::Less, "prefix first");
        // No base-letter fold here: É (0xC9) is past every ASCII letter.
        assert_eq!(cmp_utf8i("Éa", "Zb"), Ordering::Greater);
    }

    /// The record and guild arms are the UTF-8 sort compare: by base letter, ligatures spelled out,
    /// the raw code point only breaking a tie.
    #[test]
    fn the_sort_arm_orders_accents_by_their_base_letter() {
        assert_eq!(sort_utf8i("Éa", "Zb"), Ordering::Less, "É sorts as E");
        assert_eq!(
            sort_utf8i("Éa", "Eb"),
            Ordering::Less,
            "the base letters decide first"
        );
        assert_eq!(
            sort_utf8i("e", "é"),
            Ordering::Less,
            "a tie goes to the raw code point"
        );
        assert_eq!(sort_utf8i("Æther", "Aether"), Ordering::Greater);
        assert_eq!(sort_utf8i("Æthar", "Aether"), Ordering::Less, "Æ spells AE");
        assert_eq!(sort_utf8i("Straße", "Strasse"), Ordering::Greater);
        assert_eq!(
            sort_utf8i("Legacy", "legacy"),
            Ordering::Less,
            "L before l at a tie"
        );
        assert_eq!(sort_utf8i("", "Legacy"), Ordering::Less, "no guild first");
    }

    /// A row whose zone has no record ties with any row on zone, so the next key decides.
    #[test]
    fn a_missing_record_passes_to_the_next_key() {
        let chain = WhoSortChain::default();
        let known = row("Bbb", 10, "Mage", "Westfall");
        let unknown = row("Aaa", 10, "Mage", "");
        assert_eq!(chain.compare(&unknown, &known), Ordering::Less, "by name");
        assert_eq!(chain.compare(&known, &unknown), Ordering::Greater);
    }

    #[test]
    fn sorting_a_list_with_missing_records_keeps_every_row() {
        let zones = ["", "Westfall", "Durotar", "", "Elwynn Forest", "Ashenvale"];
        let mut rows: Vec<WhoInfo> = (0..50)
            .map(|i| {
                row(
                    &format!("N{i:02}"),
                    1 + (i % 7) as u32,
                    "Mage",
                    zones[i % 6],
                )
            })
            .collect();
        WhoSortChain::default().sort(&mut rows);
        let mut seen = names(&rows);
        seen.sort_unstable();
        assert_eq!(seen.len(), 50);
        seen.dedup();
        assert_eq!(seen.len(), 50, "a permutation of the input");
    }

    #[test]
    fn the_group_key_displaces_but_orders_nothing() {
        let mut chain = WhoSortChain::default();
        chain.promote("level");
        chain.promote("group");
        assert_eq!(
            chain.keys()[..2],
            [(WhoSortKey::Group, false), (WhoSortKey::Level, false)]
        );
        let mut rows = vec![row("A", 60, "Mage", "Z"), row("B", 12, "Mage", "Z")];
        chain.sort(&mut rows);
        assert_eq!(
            names(&rows),
            ["B", "A"],
            "ordered by level, group having tied"
        );
    }

    #[test]
    fn identical_rows_tie_on_every_key() {
        let chain = WhoSortChain::default();
        let a = row("Same", 30, "Priest", "Ironforge");
        assert_eq!(chain.compare(&a, &a.clone()), Ordering::Equal);
    }
}
