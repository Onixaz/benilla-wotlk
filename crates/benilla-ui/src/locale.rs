//! The running client's locale as the script layer reads it, set once at boot (`0xc5de9c`).

use std::sync::OnceLock;

/// The locales a string slot indexes (`0xad2fe0`).
const NAMES: [&str; 9] = [
    "enUS", "koKR", "frFR", "deDE", "zhCN", "zhTW", "esES", "esMX", "ruRU",
];

struct Running {
    index: usize,
    existing: Vec<&'static str>,
}

static RUNNING: OnceLock<Running> = OnceLock::new();

/// Record the running locale's string slot and the installed locales; a later call changes nothing.
pub fn set(index: usize, existing: &[&'static str]) {
    let _ = RUNNING.set(Running {
        index: index.min(NAMES.len() - 1),
        existing: existing.to_vec(),
    });
}

/// The running locale's string slot, 0 (enUS) until [`set`].
pub fn index() -> usize {
    RUNNING.get().map_or(0, |r| r.index)
}

/// The classes' and races' names under a locale other than enUS.
type IdNames = Vec<(u32, String)>;

static UNIT_NAMES: OnceLock<(IdNames, IdNames)> = OnceLock::new();

/// Record the classes' and races' names by id; a later call changes nothing.
pub fn set_names(classes: &[(u32, String)], races: &[(u32, String)]) {
    let _ = UNIT_NAMES.set((classes.to_vec(), races.to_vec()));
}

/// The running locale's name of a class, `None` under enUS.
pub(crate) fn class_name(id: u32) -> Option<&'static str> {
    let (classes, _) = UNIT_NAMES.get()?;
    classes
        .iter()
        .find(|(c, _)| *c == id)
        .map(|(_, n)| n.as_str())
}

/// The running locale's name of a race, `None` under enUS.
pub(crate) fn race_name(id: u32) -> Option<&'static str> {
    let (_, races) = UNIT_NAMES.get()?;
    races
        .iter()
        .find(|(c, _)| *c == id)
        .map(|(_, n)| n.as_str())
}

/// `GetLocale()`'s answer (`0x4dbfd0`), enUS for an enGB client.
pub fn script_name() -> &'static str {
    NAMES[index()]
}

/// The installed locales, as `GetExistingLocales()` returns them (`0x511c80`).
pub fn existing() -> &'static [&'static str] {
    RUNNING.get().map_or(&["enUS"], |r| r.existing.as_slice())
}

/// A `.toc` directive's key under the running locale: `Title-esES` is `Title` on an esES client and
/// nothing on another, and a suffix that names no locale stays part of the key (`0x5f85d0`).
pub(crate) fn toc_key(key: &str) -> Option<&str> {
    toc_key_for(key, index())
}

fn toc_key_for(key: &str, running: usize) -> Option<&str> {
    let Some((name, suffix)) = key.rsplit_once('-') else {
        return Some(key);
    };
    match NAMES
        .iter()
        .position(|l| suffix.len() == 4 && l.eq_ignore_ascii_case(suffix))
    {
        Some(i) if i == running => Some(name.trim_end()),
        Some(_) => None,
        None => Some(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locale_suffixed_toc_key_is_kept_only_under_that_locale() {
        assert_eq!(toc_key_for("Title", 6), Some("Title"));
        assert_eq!(toc_key_for("Title-esES", 6), Some("Title"));
        assert_eq!(toc_key_for("Notes-ESES", 6), Some("Notes"));
        assert_eq!(toc_key_for("Title-deDE", 6), None);
        assert_eq!(toc_key_for("Title-esES", 0), None);
        assert_eq!(toc_key_for("X-Website", 6), Some("X-Website"));
        assert_eq!(toc_key_for("Title-frFR", 2), Some("Title"));
    }
}
