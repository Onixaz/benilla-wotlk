//! The display names benilla answers from enUS literals, read from the DBCs under any other locale
//! (`ChrRaces`, `ChrClasses`, `CreatureType`). Nothing loads under enUS.

use std::collections::HashMap;
use std::sync::OnceLock;

use benilla_formats::{dbc_strings, Chain, ClientLocale};

/// One table's rows by id, each its name columns.
type Rows = HashMap<u32, Vec<String>>;

struct Names {
    races: Rows,
    classes: Rows,
    creature_types: Rows,
}

static NAMES: OnceLock<Names> = OnceLock::new();

/// Load the names once for a locale that is not enUS, and hand the script layer its copy.
pub(crate) fn boot(locale: &ClientLocale) {
    // The `|3` rule's tables load on the first word that needs them (`0x7e0a6a`).
    benilla_ui::declension::set_loader(|| {
        let mut chain = Chain::open(&benilla_formats::wow_data()?).ok()?;
        declined_words(&mut chain)
    });
    if locale.index == 0 {
        return;
    }
    let Some(names) = benilla_formats::wow_data()
        .and_then(|data| Chain::open(&data).ok())
        .and_then(|mut chain| load(&mut chain))
    else {
        return;
    };
    let plain = |rows: &Rows| {
        rows.iter()
            .filter_map(|(id, row)| Some((*id, row.first()?.clone())))
            .collect::<Vec<_>>()
    };
    benilla_ui::locale::set_names(&plain(&names.classes), &plain(&names.races));
    let _ = NAMES.set(names);
}

/// `DeclinedWord` and `DeclinedWordCases`, indexed as the `|3` rule reads them.
fn declined_words(chain: &mut Chain) -> Option<benilla_ui::declension::DeclinedWords> {
    match benilla_formats::load_declined_words(chain) {
        Ok(rows) => Some(benilla_ui::declension::DeclinedWords::new(
            rows.words, rows.cases,
        )),
        Err(e) => {
            bevy::log::warn!("locale: no declined words — {e:#}");
            None
        }
    }
}

fn load(chain: &mut Chain) -> Option<Names> {
    Some(Names {
        races: dbc_strings(chain, "DBFilesClient\\ChrRaces.dbc", &[14, 31, 48]).ok()?,
        classes: dbc_strings(chain, "DBFilesClient\\ChrClasses.dbc", &[4, 21, 38]).ok()?,
        creature_types: dbc_strings(chain, "DBFilesClient\\CreatureType.dbc", &[1]).ok()?,
    })
}

/// A row's display name for `sex` (0 male, 1 female): that sex's column, else the other's, else the
/// plain name (`0x715970`, `0x7159e0`). `row` is plain, female, male.
fn gendered(row: &[String], sex: Option<u8>) -> Option<&str> {
    let (plain, female, male) = (row.first()?, row.get(1)?, row.get(2)?);
    let picked = match sex {
        Some(0) if !male.is_empty() => male,
        Some(0) if !female.is_empty() => female,
        Some(1) if !female.is_empty() => female,
        Some(1) if !male.is_empty() => male,
        _ => plain,
    };
    (!picked.is_empty()).then_some(picked.as_str())
}

/// The running locale's name of a race, `None` under enUS.
pub(crate) fn race(id: u8, sex: Option<u8>) -> Option<&'static str> {
    gendered(NAMES.get()?.races.get(&u32::from(id))?, sex)
}

/// The running locale's name of a class, `None` under enUS.
pub(crate) fn class(id: u8, sex: Option<u8>) -> Option<&'static str> {
    gendered(NAMES.get()?.classes.get(&u32::from(id))?, sex)
}

fn single(rows: &'static Rows, id: u32) -> Option<&'static str> {
    rows.get(&id)?
        .first()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
}

/// The running locale's creature type word, `None` under enUS.
pub(crate) fn creature_type(id: u32) -> Option<&'static str> {
    single(&NAMES.get()?.creature_types, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_declined_words_give_back_each_case_row() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = Chain::open(&data).expect("open patch chain");
        let rows = benilla_formats::load_declined_words(&mut chain).expect("load");
        let words = declined_words(&mut chain).expect("index");
        // The first word with case rows, read back case by case through the index.
        let (id, word) = rows
            .words
            .iter()
            .find(|(id, _)| rows.cases.iter().any(|c| c.0 == *id))
            .expect("a word with cases");
        let cases: Vec<_> = rows.cases.iter().filter(|c| c.0 == *id).collect();
        assert!(!cases.is_empty());
        for (_, case, form) in cases {
            assert_eq!(words.form(word, *case as i32), Some(form.as_str()));
        }
        // A case with no row gives the word back; a word not in the table is not indexed.
        assert_eq!(words.form(word, 0), Some(word.as_str()));
        assert_eq!(words.form("benilla", 1), None);
    }

    #[test]
    fn a_sex_prefers_its_own_column_then_the_other_then_the_plain_name() {
        let both = [
            "Cazador".to_string(),
            "Cazadora".to_string(),
            "Cazador".to_string(),
        ];
        assert_eq!(gendered(&both, Some(1)), Some("Cazadora"));
        assert_eq!(gendered(&both, Some(0)), Some("Cazador"));
        let female_only = ["P".to_string(), "F".to_string(), String::new()];
        assert_eq!(gendered(&female_only, Some(0)), Some("F"));
        let plain_only = ["P".to_string(), String::new(), String::new()];
        assert_eq!(gendered(&plain_only, Some(1)), Some("P"));
        assert_eq!(gendered(&plain_only, None), Some("P"));
    }
}
