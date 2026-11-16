//! The `/who` filter parser, the reference's `SendWho` (`0x6b5630`): the typed string into
//! `CMSG_WHO`'s fields.
//!
//! The filter is cut to 127 bytes and split on whitespace outside double quotes; a word still
//! inside a quote at the end is dropped. A word that starts with a tag (the `WHO_TAG_*` strings,
//! without case) sets its field: `n-` the name and `g-` the guild, each cut to 47 bytes; `z-` every
//! top-level `AreaTable` zone whose name contains the rest, at most ten, or zone 0 when none does;
//! `r-` and `c-` every `ChrRaces`/`ChrClasses` row whose name contains it, a miss leaving the mask
//! at 0. A word that starts with a digit or `-` is a level range, sent as typed: `N` is N..N, `N-`
//! N..100, `-M` keeps the low bound and sets M, `N-M` N..M. Anything else is a search word, and the
//! parse stops at the fourth.

use benilla_formats::{AreaTableCatalog, ChrClassNames, CreateRaceRow};
use benilla_protocol::messages::WhoRequest;
use benilla_ui::script::{contains_utf8i, starts_with_utf8i};

/// `SStrCopy`'s cap on the filter copy (`0x6b564e`), less its terminator.
const FILTER_MAX_BYTES: usize = 127;
/// `SStrCopy`'s cap on the name and guild values (`0x6b578c`, `0x6b57cc`), less the terminator.
const VALUE_MAX_BYTES: usize = 47;
/// The search words the parse keeps before it stops (`0x6b5a52`).
const MAX_WORDS: usize = 4;
/// The zones a `z-` word adds (`0x6b5811`).
const MAX_ZONES: usize = 10;
/// The high bound an open range `N-` sends (`0x6b59f8`).
const OPEN_LEVEL_MAX: u32 = 100;

/// The five tags as the player's `GlobalStrings.lua` spells them (`WHO_TAG_NAME` ... `_CLASS`),
/// tried in the reference's order: name, guild, zone, race, class.
#[derive(Clone, Debug)]
pub(crate) struct WhoTags {
    pub(crate) name: String,
    pub(crate) guild: String,
    pub(crate) zone: String,
    pub(crate) race: String,
    pub(crate) class: String,
}

impl WhoTags {
    /// The tags through a `GetText` lookup.
    pub(crate) fn from_text(text: impl Fn(&str) -> String) -> Self {
        Self {
            name: text("WHO_TAG_NAME"),
            guild: text("WHO_TAG_GUILD"),
            zone: text("WHO_TAG_ZONE"),
            race: text("WHO_TAG_RACE"),
            class: text("WHO_TAG_CLASS"),
        }
    }
}

/// The tables a `z-`, `r-` or `c-` word searches, in record order.
#[derive(Clone, Copy, Default)]
pub(crate) struct WhoTables<'a> {
    pub(crate) areas: Option<&'a AreaTableCatalog>,
    pub(crate) races: &'a [CreateRaceRow],
    pub(crate) classes: &'a [ChrClassNames],
}

/// Parse a `/who` filter into the wire request, as `SendWho` builds it.
pub(crate) fn parse(filter: &str, tags: &WhoTags, tables: WhoTables) -> WhoRequest {
    let mut request = WhoRequest::default();
    let text = clip(filter, FILTER_MAX_BYTES);
    // The copy gets a trailing space, so the last word ends like any other.
    let mut chars = text.chars().chain(std::iter::once(' ')).peekable();
    while chars.next_if(|c| c.is_whitespace()).is_some() {}
    let mut word = String::new();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && c.is_whitespace() {
            take_word(&std::mem::take(&mut word), tags, tables, &mut request);
            if request.search_terms.len() >= MAX_WORDS {
                break;
            }
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
        } else {
            word.push(c);
        }
    }
    request
}

/// One finished word into its field.
fn take_word(word: &str, tags: &WhoTags, tables: WhoTables, request: &mut WhoRequest) {
    if let Some(value) = strip_tag(word, &tags.name) {
        request.player_name = clip(value, VALUE_MAX_BYTES).to_string();
    } else if let Some(value) = strip_tag(word, &tags.guild) {
        request.guild_name = clip(value, VALUE_MAX_BYTES).to_string();
    } else if let Some(value) = strip_tag(word, &tags.zone) {
        if request.zones.len() < MAX_ZONES {
            let zones = tables.areas.into_iter().flat_map(|a| a.rows_in_order());
            for (id, row) in zones {
                if row.zone_id == 0
                    && request.zones.len() < MAX_ZONES
                    && contains_utf8i(&row.name, value)
                {
                    request.zones.push(id);
                }
            }
        }
        if request.zones.is_empty() {
            request.zones.push(0);
        }
    } else if let Some(value) = strip_tag(word, &tags.race) {
        let rows = tables
            .races
            .iter()
            .map(|r| (u32::from(r.id), r.name.as_str()));
        request.race_mask = add_matches(request.race_mask, rows, value);
    } else if let Some(value) = strip_tag(word, &tags.class) {
        let rows = tables.classes.iter().map(|c| (c.id, c.name.as_str()));
        request.class_mask = add_matches(request.class_mask, rows, value);
    } else if !level_word(word, request) {
        request.search_terms.push(word.to_string());
    }
}

/// The value after a tag the word starts with, without case (`0x6b577d`). An empty tag, a token
/// the string table lacks, tags nothing.
fn strip_tag<'w>(word: &'w str, tag: &str) -> Option<&'w str> {
    if tag.is_empty() || !starts_with_utf8i(word, tag) {
        return None;
    }
    // The reference skips the tag's byte length; folded letters share theirs.
    Some(word.get(tag.len()..).unwrap_or(""))
}

/// A race or class word: the all-ones default drops to 0 first, then every row whose name holds
/// the word ORs in `1 << id` (`0x6b58c3`-`0x6b590b`, `0x6b5952`-`0x6b599a`).
fn add_matches<'a>(mask: u32, rows: impl Iterator<Item = (u32, &'a str)>, value: &str) -> u32 {
    let mut mask = if mask == u32::MAX { 0 } else { mask };
    for (id, name) in rows {
        if contains_utf8i(name, value) {
            // `shl edx,cl` takes the count mod 32.
            mask |= 1u32.wrapping_shl(id);
        }
    }
    mask
}

/// A level word (`0x6b59ad`-`0x6b5a2c`), written into the request as typed; false for a word that
/// is no range, which becomes a search word.
fn level_word(word: &str, request: &mut WhoRequest) -> bool {
    let bytes = word.as_bytes();
    let (next, is_level) = match bytes.first() {
        Some(b) if b.is_ascii_digit() => {
            let low = str_to_int(word);
            request.level_min = low;
            request.level_max = low;
            let mut i = 1 + bytes[1..].iter().take_while(|b| b.is_ascii_digit()).count();
            if bytes.get(i) == Some(&b'-') {
                i += 1;
                request.level_max = OPEN_LEVEL_MAX;
            }
            (i, true)
        }
        Some(b'-') => (1, false),
        _ => return false,
    };
    if bytes.get(next).is_some_and(u8::is_ascii_digit) {
        request.level_max = str_to_int(&word[next..]);
        true
    } else {
        is_level
    }
}

/// `SStrToInt` (`0x76f0d0`) on a run of digits: wrapping, as the reference's unsigned accumulator.
fn str_to_int(digits: &str) -> u32 {
    digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0u32, |n, d| {
            n.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
        })
}

/// `s` cut to at most `max` bytes, at a letter boundary.
fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> WhoTags {
        WhoTags {
            name: "n-".into(),
            guild: "g-".into(),
            zone: "z-".into(),
            race: "r-".into(),
            class: "c-".into(),
        }
    }

    fn classes() -> Vec<ChrClassNames> {
        [
            (1, "Warrior"),
            (2, "Paladin"),
            (6, "Death Knight"),
            (9, "Warlock"),
        ]
        .into_iter()
        .map(|(id, name)| ChrClassNames {
            id,
            name: name.into(),
            ..Default::default()
        })
        .collect()
    }

    fn races() -> Vec<CreateRaceRow> {
        [
            (4, "Night Elf"),
            (8, "Troll"),
            (10, "Blood Elf"),
            (21, "Ice Troll"),
        ]
        .into_iter()
        .map(|(id, name)| CreateRaceRow {
            id,
            name: name.into(),
            ..Default::default()
        })
        .collect()
    }

    fn area(name: &str, parent: u32) -> benilla_formats::AreaTableRow {
        benilla_formats::AreaTableRow {
            map_id: 0,
            zone_id: parent,
            explore_flag: 0,
            flags: 0,
            faction_group_mask: 0,
            exploration_level: -1,
            name: name.into(),
        }
    }

    fn run(filter: &str) -> WhoRequest {
        let (races, classes) = (races(), classes());
        let areas = AreaTableCatalog::from_rows(vec![
            (12, area("Elwynn Forest", 0)),
            (87, area("Goldshire", 12)),
            (1519, area("Stormwind City", 0)),
            (40, area("Westfall", 0)),
        ]);
        parse(
            filter,
            &tags(),
            WhoTables {
                areas: Some(&areas),
                races: &races,
                classes: &classes,
            },
        )
    }

    #[test]
    fn an_empty_filter_is_the_default_query() {
        assert_eq!(run(""), WhoRequest::default());
        assert_eq!(run("   "), WhoRequest::default());
    }

    #[test]
    fn tags_route_to_their_fields() {
        let q = run("n-bob g-\"Legacy of Steel\" 10-20");
        assert_eq!(q.player_name, "bob");
        assert_eq!(q.guild_name, "Legacy of Steel");
        assert_eq!((q.level_min, q.level_max), (10, 20));
        assert!(q.search_terms.is_empty(), "tagged terms are not also terms");
        assert_eq!(run("N-Bob").player_name, "Bob", "the tag without case");
    }

    /// Every row whose name holds the word: `c-war` is Warrior and Warlock, `r-troll` both trolls.
    #[test]
    fn class_and_race_words_take_every_row_containing_them() {
        let q = run("c-war r-TROLL");
        assert_eq!(q.class_mask, (1 << 1) | (1 << 9));
        assert_eq!(q.race_mask, (1 << 8) | (1 << 21));
        assert_eq!(
            run("r-elf c-\"death knight\"").race_mask,
            (1 << 4) | (1 << 10)
        );
        assert_eq!(run("c-mage c-pal").class_mask, 1 << 2, "a miss then a hit");
        assert!(run("c-war").search_terms.is_empty());
    }

    /// A word that matches nothing leaves the mask at 0, and is never a search word.
    #[test]
    fn a_class_or_race_miss_sends_an_empty_mask() {
        let q = run("c-necromancer r-nightelf");
        assert_eq!((q.class_mask, q.race_mask), (0, 0));
        assert!(q.search_terms.is_empty());
    }

    /// Top-level rows only, by substring; a miss sends zone 0.
    #[test]
    fn zone_words_take_top_level_zones_containing_them() {
        assert_eq!(run("z-elwynn").zones, vec![12]);
        assert_eq!(run("z-gold").zones, vec![0], "a subzone is never sent");
        assert_eq!(run("z-st").zones, vec![12, 1519, 40], "record order");
        let miss = run("z-nowhere");
        assert_eq!(miss.zones, vec![0]);
        assert!(miss.search_terms.is_empty());
        assert_eq!(
            run("z-nowhere z-westfall").zones,
            vec![0, 40],
            "the miss's zone 0 stays"
        );
    }

    /// No area table finds nothing, so the zone goes out as 0.
    #[test]
    fn a_zone_word_without_the_table_sends_zone_0() {
        let q = parse("z-elwynn", &tags(), WhoTables::default());
        assert_eq!(q.zones, vec![0]);
    }

    #[test]
    fn levels_go_out_as_typed() {
        let lv = |f: &str| {
            let q = run(f);
            (q.level_min, q.level_max)
        };
        assert_eq!(lv("60"), (60, 60));
        assert_eq!(lv("70-"), (70, 100));
        assert_eq!(lv("-30"), (0, 30));
        assert_eq!(lv("10-20"), (10, 20));
        assert_eq!(lv("20-10"), (20, 10), "no reorder");
        assert_eq!(lv("0-9999"), (0, 9999), "no clamp");
        assert_eq!(lv("10 -5"), (10, 5), "`-M` keeps the low bound");
        assert_eq!(lv("15abc"), (15, 15), "trailing letters ignored");
        assert!(run("60").search_terms.is_empty());
    }

    /// `-`, `-x` and a hyphenated word are search words; the level stays default.
    #[test]
    fn hyphens_that_are_not_ranges() {
        assert_eq!(run("well-met").search_terms, vec!["well-met".to_string()]);
        let q = run("- -x");
        assert_eq!(q.search_terms, vec!["-".to_string(), "-x".to_string()]);
        assert_eq!((q.level_min, q.level_max), (0, 100));
    }

    /// The fourth search word ends the parse: later words, tags included, are never read.
    #[test]
    fn the_parse_stops_at_four_words() {
        let q = run("a b c d e n-bob");
        assert_eq!(q.search_terms, ["a", "b", "c", "d"]);
        assert!(q.player_name.is_empty());
    }

    #[test]
    fn quotes_hold_a_value_together() {
        assert_eq!(run("z-\"Elwynn Forest\" 1-10").zones, vec![12]);
        assert_eq!(run("\"two words\"").search_terms, ["two words"]);
        // The quote never closes, so the last word never ends and is dropped.
        assert!(run("n-\"bo").player_name.is_empty());
        // An empty quoted word is a search word.
        assert_eq!(run("a \"\" b").search_terms, ["a", "", "b"]);
    }

    /// The filter is cut to 127 bytes, and a name or guild value to 47.
    #[test]
    fn the_copies_are_capped() {
        let long = "x".repeat(60);
        assert_eq!(run(&format!("n-{long}")).player_name.len(), 47);
        assert_eq!(run(&format!("g-{long}")).guild_name.len(), 47);
        let filter = format!("{} 10", "y".repeat(126));
        assert_eq!(run(&filter).level_max, 100, "the level fell past the cut");
    }

    #[test]
    fn real_tables_resolve_partial_words() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable.dbc");
        let classes = benilla_formats::load_chr_classes(&mut chain).expect("ChrClasses.dbc");
        let create = benilla_formats::CharCreateCatalog::load(&mut chain).expect("ChrRaces.dbc");
        let tables = WhoTables {
            areas: Some(&areas),
            races: create.race_rows(),
            classes: classes.names(),
        };
        let q = parse("z-elwynn c-war 70-", &tags(), tables);
        assert_eq!(q.zones, vec![12], "Elwynn Forest");
        assert_eq!(q.class_mask, (1 << 1) | (1 << 9), "Warrior and Warlock");
        assert_eq!((q.level_min, q.level_max), (70, 100));
        let trolls = parse("r-troll", &tags(), tables).race_mask;
        assert_ne!(trolls & (1 << 8), 0, "Troll");
        assert!(trolls.count_ones() > 1, "and the other troll rows");
    }
}
