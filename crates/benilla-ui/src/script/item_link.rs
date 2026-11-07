//! The item string: `item:<id>:<enchant>:<gem1>:<gem2>:<gem3>:<gem4>:<property>:<seed>:<level>`.
//! Every link the client shows comes out of one builder (`0x61e290`) and every reader goes through
//! one parser (`0x50f630`), so a shape can only change here.

/// The item-quality palette (`0xad2d84`, eight entries), and the escape strings (`0xad2a84`).
/// `GetItemQualityColor` answers each channel times 1/255; a quality past the table reads Common.
pub(super) const QUALITY_COLORS: [(u8, u8, u8, &str); 8] = [
    (0x9d, 0x9d, 0x9d, "|cff9d9d9d"), // 0 Poor
    (0xff, 0xff, 0xff, "|cffffffff"), // 1 Common
    (0x1e, 0xff, 0x00, "|cff1eff00"), // 2 Uncommon
    (0x00, 0x70, 0xdd, "|cff0070dd"), // 3 Rare
    (0xa3, 0x35, 0xee, "|cffa335ee"), // 4 Epic
    (0xff, 0x80, 0x00, "|cffff8000"), // 5 Legendary
    (0xe6, 0xcc, 0x80, "|cffe6cc80"), // 6 Artifact
    (0xe6, 0xcc, 0x80, "|cffe6cc80"), // 7 Heirloom
];

/// What an item string carries past its id. The builder writes the gem past the third as 0, and
/// the level is the link's own, never written back: a build stamps the active player's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemLinkFields {
    pub enchant: i32,
    pub gems: [i32; 3],
    /// Positive names `ItemRandomProperties.dbc`, negative `ItemRandomSuffix.dbc`.
    pub random_property: i32,
    pub seed: i32,
    pub level: i32,
}

impl ItemLinkFields {
    /// A roll with nothing enchanted: what loot, roll and merchant rows carry.
    pub fn roll(random_property: i32, seed: i32) -> Self {
        Self {
            random_property,
            seed,
            ..Self::default()
        }
    }

    /// The id and fields of the first `item:` in `text` (`0x50f630`): a case-sensitive substring
    /// search, a missing field 0, and an id at or below 0 is no item.
    pub fn parse(text: &str) -> Option<(u32, Self)> {
        let rest = &text[text.find("item:")? + 5..];
        let mut fields = rest.split(':').map(reference_atoi);
        let id = u32::try_from(fields.next()?).ok().filter(|&id| id != 0)?;
        let mut next = || fields.next().unwrap_or(0) as i32;
        let enchant = next();
        let gems = [next(), next(), next()];
        let _fourth = next();
        Some((
            id,
            Self {
                enchant,
                gems,
                random_property: next(),
                seed: next(),
                level: next(),
            },
        ))
    }

    /// The enchant slots the string names, as `(slot, id)`: permanent in 0, gems in 2..=4.
    pub fn enchant_slots(&self) -> [(u8, i32); 4] {
        [
            (0, self.enchant),
            (2, self.gems[0]),
            (3, self.gems[1]),
            (4, self.gems[2]),
        ]
    }
}

/// The reference's integer parse over an `item:` field: an optional `-`, then digits up to the
/// first non-digit, with no whitespace skip and no `+`. Deviation: an overflow saturates at
/// `u32::MAX` where the reference wraps, because only a script can overflow it.
pub(super) fn reference_atoi(s: &str) -> i64 {
    let b = s.as_bytes();
    let (neg, mut i) = match b.first() {
        Some(b'-') => (true, 1),
        _ => (false, 0),
    };
    let mut n: i64 = 0;
    while let Some(&c) = b.get(i) {
        if !c.is_ascii_digit() {
            break;
        }
        n = n
            .saturating_mul(10)
            .saturating_add(i64::from(c - b'0'))
            .min(i64::from(u32::MAX));
        i += 1;
    }
    if neg {
        -n
    } else {
        n
    }
}

/// The item-link builder (`0x61e290`, format `0xa24f04`): the quality's escape, a past-the-table
/// quality reading Common, the nine fields with the fourth gem 0, the name and a reset. `level` is
/// the active player's, 0 before one exists.
pub fn item_hyperlink(
    item_id: u32,
    quality: u32,
    fields: &ItemLinkFields,
    level: u32,
    name: &str,
) -> String {
    let hex = QUALITY_COLORS
        .get(quality as usize)
        .unwrap_or(&QUALITY_COLORS[1])
        .3;
    let [g1, g2, g3] = fields.gems;
    format!(
        "{hex}|Hitem:{item_id}:{}:{g1}:{g2}:{g3}:0:{}:{}:{level}|h[{name}]|h|r",
        fields.enchant, fields.random_property, fields.seed
    )
}

#[cfg(test)]
mod tests {
    use super::{item_hyperlink, ItemLinkFields};

    #[test]
    fn a_link_round_trips_nine_fields_and_a_negative_suffix() {
        let f = ItemLinkFields {
            enchant: 3370,
            gems: [3520, 0, 3531],
            random_property: -39,
            seed: 28,
            level: 0,
        };
        let link = item_hyperlink(40400, 4, &f, 80, "Wall of Terror");
        assert_eq!(
            link,
            "|cffa335ee|Hitem:40400:3370:3520:0:3531:0:-39:28:80|h[Wall of Terror]|h|r"
        );
        let (id, back) = ItemLinkFields::parse(&link).unwrap();
        assert_eq!((id, back), (40400, ItemLinkFields { level: 80, ..f }));
    }

    #[test]
    fn a_short_string_reads_zeros_and_a_non_positive_id_is_no_item() {
        assert_eq!(
            ItemLinkFields::parse("item:117"),
            Some((117, ItemLinkFields::default()))
        );
        assert_eq!(ItemLinkFields::parse("item:0:1"), None);
        assert_eq!(ItemLinkFields::parse("item:-4"), None);
        assert_eq!(ItemLinkFields::parse("Tough Jerky"), None);
    }

    #[test]
    fn a_quality_past_the_table_reads_common() {
        assert!(item_hyperlink(1, 9, &ItemLinkFields::default(), 0, "x").starts_with("|cffffffff"));
    }
}
