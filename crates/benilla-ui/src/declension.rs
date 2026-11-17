//! The case lookup the `|3` grammar rule replaces its word with (`0x7e0a50`): the name caches'
//! declined forms, then `DeclinedWord`/`DeclinedWordCases`, then the name caches again on the
//! part before a `-`. It declines only in string slot 8, ruRU (`0x76dd20`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock, RwLock};

/// The ruRU string slot.
const RU_RU: usize = 8;

/// The word index 12340 builds over the two tables (`0x7e08e0`).
#[derive(Debug, Default)]
pub struct DeclinedWords {
    /// A word to its id and its first case row, for words that have one.
    index: HashMap<String, (u32, usize)>,
    /// `(word id, case, form)` in file order.
    cases: Vec<(u32, u32, String)>,
}

impl DeclinedWords {
    /// Index `words` against `cases`, both in file order: a merge walk on the word id, so a word
    /// whose cases come before the walk's position is skipped, and the first of two equal words
    /// is the one found (`0x7e08e0`).
    pub fn new(words: Vec<(u32, String)>, cases: Vec<(u32, u32, String)>) -> Self {
        let mut index = HashMap::new();
        let mut at = 0;
        for (id, word) in words {
            while at < cases.len() && cases[at].0 < id {
                at += 1;
            }
            if cases.get(at).is_some_and(|c| c.0 == id) {
                index.entry(word).or_insert((id, at));
            }
        }
        Self { index, cases }
    }

    /// `Some` for an indexed word: its form for `case`, or the word itself when it has none
    /// (`0x7e0cd6`).
    pub fn form<'a>(&'a self, word: &'a str, case: i32) -> Option<&'a str> {
        let &(id, first) = self.index.get(word)?;
        let found = self.cases[first..]
            .iter()
            .take_while(|c| c.0 == id && c.1 as i32 <= case)
            .find(|c| c.1 as i32 == case);
        Some(found.map_or(word, |c| c.2.as_str()))
    }
}

/// Which name cache a declined set belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    Player,
    Pet,
}

/// The five declined forms the name caches hold by name, matched without case.
#[derive(Debug, Default)]
pub struct NameForms {
    players: HashMap<String, [String; 5]>,
    pets: HashMap<String, [String; 5]>,
}

impl NameForms {
    /// Record or clear a name's forms, as a name answer with or without its declension block does.
    pub fn set(&mut self, kind: NameKind, name: &str, forms: Option<[String; 5]>) {
        let map = match kind {
            NameKind::Player => &mut self.players,
            NameKind::Pet => &mut self.pets,
        };
        let key = name.to_lowercase();
        match forms {
            Some(forms) => {
                map.insert(key, forms);
            }
            None => {
                map.remove(&key);
            }
        }
    }

    /// The player cache's form, then the pet cache's (`0x7e0c73`, `0x7e0ca3`).
    fn form(&self, name: &str, index: usize) -> Option<&str> {
        let key = name.to_lowercase();
        self.players
            .get(&key)
            .or_else(|| self.pets.get(&key))
            .map(|forms| forms[index].as_str())
    }
}

type Loader = Box<dyn Fn() -> Option<DeclinedWords> + Send + Sync>;

static LOADER: OnceLock<Loader> = OnceLock::new();
static WORDS: OnceLock<Option<DeclinedWords>> = OnceLock::new();
static NAMES: LazyLock<RwLock<NameForms>> = LazyLock::new(Default::default);

/// Give the lookup its table loader, run on the first word that needs it; a later call changes
/// nothing.
pub fn set_loader(load: impl Fn() -> Option<DeclinedWords> + Send + Sync + 'static) {
    let _ = LOADER.set(Box::new(load));
}

/// Record a name answer's declined forms, or clear them when it carried none.
pub fn set_name_forms(kind: NameKind, name: &str, forms: Option<[String; 5]>) {
    if let Ok(mut names) = NAMES.write() {
        names.set(kind, name, forms);
    }
}

fn words() -> Option<&'static DeclinedWords> {
    WORDS
        .get_or_init(|| LOADER.get().and_then(|load| load()))
        .as_ref()
}

/// `word` in grammatical case `case` under the running locale.
pub(crate) fn decline(word: &str, case: i32) -> Cow<'_, str> {
    let locale = crate::locale::index();
    if locale != RU_RU {
        return Cow::Borrowed(word);
    }
    let names = NAMES.read().unwrap_or_else(|e| e.into_inner());
    decline_with(word, case, locale, words(), &names)
}

/// [`decline`] over explicit tables. A word opening with `|` keeps its `|c`, `|H…|h` and `|T…|t`
/// wrappers and an optional `[`, declines the text up to `]` or `|`, and keeps the rest.
fn decline_with<'a>(
    word: &'a str,
    case: i32,
    locale: usize,
    words: Option<&DeclinedWords>,
    names: &NameForms,
) -> Cow<'a, str> {
    if !word.starts_with('|') {
        return decline_bare(word, case, locale, words, names);
    }
    let b = word.as_bytes();
    let mut at = 0;
    // The wrapper walk (`0x7e0a76`).
    while b.get(at) == Some(&b'|') {
        at += 1;
        match b.get(at) {
            Some(b'c' | b'C') => {
                let colour = (b.len() - at - 1).min(8);
                at += 1 + colour;
                if colour < 8 {
                    break;
                }
            }
            Some(&kind @ (b'H' | b'T')) => {
                let close = if kind == b'H' { b'h' } else { b't' };
                match (at..b.len()).find(|&i| b[i] == b'|' && b.get(i + 1) == Some(&close)) {
                    Some(i) => at = i + 2,
                    None => {
                        at = b.len();
                        break;
                    }
                }
            }
            Some(_) => at += 1,
            None => break,
        }
    }
    if b.get(at) == Some(&b'[') {
        at += 1;
    }
    let mut end = at;
    while end < b.len() && b[end] != b']' && b[end] != b'|' && end - at < 1023 {
        end += 1;
    }
    while !word.is_char_boundary(end) {
        end -= 1;
    }
    let inner = decline_bare(&word[at..end], case, locale, words, names);
    if inner == word[at..end] {
        return Cow::Borrowed(word);
    }
    Cow::Owned(format!("{}{inner}{}", &word[..at], &word[end..]))
}

/// The lookup for a word with no wrapper.
fn decline_bare<'a>(
    word: &'a str,
    case: i32,
    locale: usize,
    words: Option<&DeclinedWords>,
    names: &NameForms,
) -> Cow<'a, str> {
    // Only a ruRU client whose word opens with a Cyrillic letter declines (`0x76dd20`, `0x76e270`).
    let cyrillic = word
        .chars()
        .next()
        .is_some_and(|c| (0x400..=0x4ff).contains(&u32::from(c)));
    if locale != RU_RU || !cyrillic {
        return Cow::Borrowed(word);
    }
    let index = case.wrapping_sub(1);
    if (0..5).contains(&index) {
        if let Some(form) = names.form(word, index as usize) {
            return Cow::Owned(form.to_owned());
        }
    }
    if let Some(form) = words.and_then(|w| w.form(word, case)) {
        return Cow::Owned(form.to_owned());
    }
    // A hyphenated name declines its first part and keeps the rest (`0x7e0d4a`).
    if let Some(dash) = word.find('-') {
        if index < 0 {
            return Cow::Borrowed(word);
        }
        let mut cut = dash.min(1023);
        while !word.is_char_boundary(cut) {
            cut -= 1;
        }
        if index < 5 {
            if let Some(form) = names.form(&word[..cut], index as usize) {
                return Cow::Owned(format!("{form}{}", &word[dash..]));
            }
        }
    }
    // The generated declension of a name (`0x76dd60`) is #242.
    Cow::Borrowed(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(stem: &str) -> [String; 5] {
        std::array::from_fn(|i| format!("{stem}{}", i + 1))
    }

    fn table() -> DeclinedWords {
        DeclinedWords::new(
            vec![
                (1, "волк".into()),
                (2, "заяц".into()),
                (3, "лиса".into()),
                (4, "волк".into()),
            ],
            vec![
                (1, 1, "волка".into()),
                (1, 2, "волку".into()),
                (1, 4, "волком".into()),
                (3, 1, "лисы".into()),
                (4, 1, "второй".into()),
            ],
        )
    }

    #[test]
    fn only_a_ru_ru_client_declines_and_only_a_cyrillic_word() {
        let words = table();
        let names = NameForms::default();
        assert_eq!(decline_with("волк", 1, 0, Some(&words), &names), "волк");
        assert_eq!(decline_with("волк", 1, 8, Some(&words), &names), "волка");
        assert_eq!(decline_with("wolf", 1, 8, Some(&words), &names), "wolf");
        assert_eq!(decline_with("", 1, 8, Some(&words), &names), "");
    }

    #[test]
    fn the_table_walks_a_words_cases_up_to_the_asked_one() {
        let words = table();
        let names = NameForms::default();
        let at = |w, c| decline_with(w, c, 8, Some(&words), &names).into_owned();
        assert_eq!(at("волк", 2), "волку");
        assert_eq!(at("волк", 4), "волком");
        // No row for the case: the word itself, never the next lookup.
        assert_eq!(at("волк", 3), "волк");
        assert_eq!(at("волк", 9), "волк");
        // A word with no case rows is not indexed; the first of two equal words wins.
        assert_eq!(at("заяц", 1), "заяц");
        assert_eq!(at("лиса", 1), "лисы");
    }

    #[test]
    fn the_name_caches_come_first_for_cases_one_to_five() {
        let words = table();
        let mut names = NameForms::default();
        names.set(NameKind::Pet, "Волк", Some(forms("пёс")));
        let at = |w, c, n: &NameForms| decline_with(w, c, 8, Some(&words), n).into_owned();
        assert_eq!(at("волк", 2, &names), "пёс2");
        names.set(NameKind::Player, "волк", Some(forms("игрок")));
        assert_eq!(at("ВОЛК", 5, &names), "игрок5");
        // Past case five the caches are not asked.
        assert_eq!(at("волк", 6, &names), "волк");
        // An answer without forms clears them.
        names.set(NameKind::Player, "волк", None);
        assert_eq!(at("волк", 1, &names), "пёс1");
    }

    #[test]
    fn a_hyphenated_name_declines_its_first_part() {
        let mut names = NameForms::default();
        names.set(NameKind::Player, "Борис", Some(forms("Борис")));
        let at = |w, c| decline_with(w, c, 8, None, &names).into_owned();
        assert_eq!(at("Борис-Гордунни", 3), "Борис3-Гордунни");
        assert_eq!(at("Борис-Гордунни", 0), "Борис-Гордунни");
        assert_eq!(at("Борис-Гордунни", 6), "Борис-Гордунни");
        assert_eq!(at("Глеб-Гордунни", 1), "Глеб-Гордунни");
    }

    #[test]
    fn wrappers_stay_around_the_declined_text() {
        let words = table();
        let names = NameForms::default();
        let at = |w| decline_with(w, 1, 8, Some(&words), &names).into_owned();
        assert_eq!(at("|cffff0000волк|r"), "|cffff0000волка|r");
        assert_eq!(
            at("|cffff0000|Hunit:1|h[волк]|h|r"),
            "|cffff0000|Hunit:1|h[волка]|h|r"
        );
        assert_eq!(
            at("|TInterface\\Icon:0|tволк"),
            "|TInterface\\Icon:0|tволка"
        );
        // The text runs to the `]`, and what follows is kept as it is.
        assert_eq!(
            at("|cffffffff[волк] и лиса|r"),
            "|cffffffff[волка] и лиса|r"
        );
        // The whole text is the word: no row for it, nothing changes.
        assert_eq!(at("|cffffffffволк и лиса"), "|cffffffffволк и лиса");
        // A short colour or an open link has no text left to decline.
        assert_eq!(at("|cff00"), "|cff00");
        assert_eq!(at("|Hunit:1 волк"), "|Hunit:1 волк");
        // Under another locale every wrapper and word comes back as it was.
        assert_eq!(
            decline_with("|cffff0000волк|r", 1, 0, Some(&words), &names),
            "|cffff0000волк|r"
        );
    }
}
