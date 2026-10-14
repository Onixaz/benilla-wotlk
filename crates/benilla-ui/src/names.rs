//! The client's name checker `0x7e18c0`, behind a guild charter's name (`ValidateGuildName
//! 0x7e1f60`) and a pet's rename (`0x7e1f00`), and the message its code shows
//! (`CGGameUI::DisplayNameError 0x5218c0`). The server re-checks every name and answers a refusal
//! with the same codes (`SMSG_GUILD_COMMAND_RESULT` 6, `SMSG_PET_NAME_INVALID`).
//!
//! Not built: the reserved and profane pattern lists (code 8, `NamesReserved.dbc` and
//! `NamesProfanity.dbc` compiled at `ValidateNameInitialize 0x7e2250`), and the realm category's
//! charset mask from `Cfg_Categories.dbc` (`0x4db582`). Every charset is allowed, the reference's
//! own fallback when the category has no row (`0x4db5a7` passes mask 0).

/// A checker or server code: the `u32` `DisplayNameError` takes, and the `reason` of
/// `SMSG_PET_NAME_INVALID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameError(pub u32);

impl NameError {
    pub const INVALID: Self = Self(1);
    pub const NO_NAME: Self = Self(2);
    pub const TOO_SHORT: Self = Self(3);
    pub const TOO_LONG: Self = Self(4);
    /// A character outside the name's charset and its extras; `DisplayNameError`'s default arm.
    pub const INVALID_CHARACTER: Self = Self(5);
    pub const RESERVED: Self = Self(8);
    /// An apostrophe first or last.
    pub const INVALID_APOSTROPHE: Self = Self(9);
    pub const MULTIPLE_APOSTROPHES: Self = Self(10);
    pub const THREE_CONSECUTIVE: Self = Self(11);
    /// A space first or last.
    pub const INVALID_SPACE: Self = Self(12);
    pub const CONSECUTIVE_SPACES: Self = Self(13);
    pub const RUSSIAN_CONSECUTIVE_SILENT_CHARACTERS: Self = Self(14);
    pub const RUSSIAN_SILENT_CHARACTER_AT_BEGINNING_OR_END: Self = Self(15);

    /// The message `DisplayNameError` shows for this code: its jump table at `0x52199c` covers
    /// 2..=16, and 5, 9, 10 and every code outside it take `ERR_NAME_INVALID` (`0x52198d`).
    pub fn key(self) -> &'static str {
        match self.0 {
            2 => "ERR_NAME_NO_NAME",
            3 => "ERR_NAME_TOO_SHORT",
            4 => "ERR_NAME_TOO_LONG",
            6 => "ERR_NAME_MIXED_LANGUAGES",
            7 => "ERR_NAME_PROFANE",
            8 => "ERR_NAME_RESERVED",
            11 => "ERR_NAME_THREE_CONSECUTIVE",
            12 => "ERR_NAME_INVALID_SPACE",
            13 => "ERR_NAME_CONSECUTIVE_SPACES",
            14 => "ERR_NAME_RUSSIAN_CONSECUTIVE_SILENT_CHARACTERS",
            15 => "ERR_NAME_RUSSIAN_SILENT_CHARACTER_AT_BEGINNING_OR_END",
            16 => "ERR_NAME_DECLENSION_DOESNT_MATCH_BASE_NAME",
            _ => "ERR_NAME_INVALID",
        }
    }
}

/// `ValidateGuildName 0x7e1f60`: the space is the one extra, and more than 24 UTF-16 units is
/// too long.
pub fn check_guild_name(name: &str) -> Result<(), NameError> {
    let checked = check(name, &[u16::from(b' ')])?;
    if checked.len > 24 {
        return Err(NameError::TOO_LONG);
    }
    Ok(())
}

/// `0x7e1f00` as `PetRename 0x5d5670` calls it: no extras, and more than 12 UTF-16 units is too
/// long, 8 for a Hangul or Han name.
pub fn check_pet_name(name: &str) -> Result<(), NameError> {
    let checked = check(name, &[])?;
    let max = if matches!(checked.charset, Some(HANGUL | HAN)) {
        8
    } else {
        12
    };
    if checked.len > max {
        return Err(NameError::TOO_LONG);
    }
    Ok(())
}

const LATIN1: usize = 0;
const ASCII: usize = 1;
const CYRILLIC: usize = 2;
const HANGUL: usize = 3;
const HAN: usize = 4;

/// What a passing name measured.
struct Checked {
    /// UTF-16 units.
    len: usize,
    /// The charset its first letter picked.
    charset: Option<usize>,
}

/// Whether `c` is a letter of `charset`, the five tests `0x7e1080` dispatches to.
fn is_letter(c: u16, charset: usize) -> bool {
    match charset {
        // `0x7e0f30`: Latin-1 letters, without `×`, `÷`, `Þ` and `þ`.
        LATIN1 => {
            ascii_letter(c)
                || ((0xc0..=0xdd).contains(&c) && c != 0xd7)
                || c == 0xdf
                || ((0xe0..=0xff).contains(&c) && c != 0xf7 && c != 0xfe)
        }
        // `0x7e0f90`.
        ASCII => ascii_letter(c),
        // `0x7e1050`.
        CYRILLIC => (0x410..=0x44f).contains(&c) || c == 0x401 || c == 0x451,
        // `0x7e0fc0` admits only the syllables of its table at `0xaf2b98`, all inside this block;
        // the block stands in for the table, which is the client's data.
        HANGUL => (0xac00..=0xd7a3).contains(&c),
        // `0x7e0ff0`.
        HAN => {
            (0x4e00..=0x9fff).contains(&c)
                || (0x3400..=0x4dff).contains(&c)
                || (0xf900..=0xfaff).contains(&c)
        }
        _ => false,
    }
}

fn ascii_letter(c: u16) -> bool {
    u8::try_from(c).is_ok_and(|b| b.is_ascii_alphabetic())
}

/// The checker's case fold for its repeat and Russian tests: Latin, Latin-1, `Œ` and Cyrillic
/// capitals to their small letters.
fn fold(c: u16) -> u16 {
    match c {
        0x41..=0x5a | 0xc0..=0xde | 0x410..=0x42f => c + 0x20,
        0x152 => 0x153,
        0x401 => 0x451,
        _ => c,
    }
}

/// `0x7e18c0` with its prefix rules on (`param_9` 0): one pass over the UTF-16 units. The first
/// letter picks the charset the rest must share; any other unit is refused unless it is one of
/// `extras`, and an apostrophe or space among them has its own placement rules.
fn check(name: &str, extras: &[u16]) -> Result<Checked, NameError> {
    const SPACE: u16 = b' ' as u16;
    const APOSTROPHE: u16 = b'\'' as u16;
    const HARD_SIGN: u16 = 0x44a;
    const SOFT_SIGN: u16 = 0x44c;

    let units: Vec<u16> = name.encode_utf16().collect();
    let mut charset = None;
    let mut apostrophe_at: Option<usize> = None;
    let mut space_at: Option<usize> = None;
    for (i, &c) in units.iter().enumerate() {
        let n = i + 1;
        let letter = match charset {
            Some(set) => is_letter(c, set),
            None => {
                charset = (0..5).find(|&set| is_letter(c, set));
                charset.is_some()
            }
        };
        if !letter {
            if !extras.contains(&c) {
                return Err(NameError::INVALID_CHARACTER);
            }
            if c == APOSTROPHE {
                if n == 1 {
                    return Err(NameError::INVALID_APOSTROPHE);
                }
                if apostrophe_at.is_some() {
                    return Err(NameError::MULTIPLE_APOSTROPHES);
                }
                apostrophe_at = Some(i);
            } else if c == SPACE {
                if n == 1 {
                    return Err(NameError::INVALID_SPACE);
                }
                if space_at == Some(i - 1) {
                    return Err(NameError::CONSECUTIVE_SPACES);
                }
                space_at = Some(i);
            }
            continue;
        }
        if n > 2 {
            let (a, b, c) = (fold(units[i - 2]), fold(units[i - 1]), fold(c));
            if a == b && b == c {
                return Err(NameError::THREE_CONSECUTIVE);
            }
        }
        if charset == Some(CYRILLIC) {
            let c = fold(c);
            let next = units.get(n).copied().unwrap_or(0);
            let silent = |c| c == HARD_SIGN || c == SOFT_SIGN;
            if (n == 1 && silent(c)) || ((next == 0 || next == SPACE) && c == HARD_SIGN) {
                return Err(NameError::RUSSIAN_SILENT_CHARACTER_AT_BEGINNING_OR_END);
            }
            if n > 1 && silent(c) && silent(fold(units[i - 1])) {
                return Err(NameError::RUSSIAN_CONSECUTIVE_SILENT_CHARACTERS);
            }
        }
    }
    let len = units.len();
    match len {
        0 => return Err(NameError::NO_NAME),
        1 => return Err(NameError::TOO_SHORT),
        _ => {}
    }
    if apostrophe_at == Some(len - 1) {
        return Err(NameError::INVALID_APOSTROPHE);
    }
    if space_at == Some(len - 1) {
        return Err(NameError::INVALID_SPACE);
    }
    Ok(Checked { len, charset })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guild_name_takes_letters_and_single_inner_spaces() {
        assert_eq!(check_guild_name("Legacy of Steel"), Ok(()));
        assert_eq!(check_guild_name("Ünter Ölbaum"), Ok(()));
        assert_eq!(check_guild_name(""), Err(NameError::NO_NAME));
        assert_eq!(check_guild_name("A"), Err(NameError::TOO_SHORT));
        assert_eq!(check_guild_name(" Steel"), Err(NameError::INVALID_SPACE));
        assert_eq!(check_guild_name("Steel "), Err(NameError::INVALID_SPACE));
        assert_eq!(
            check_guild_name("Legacy  Steel"),
            Err(NameError::CONSECUTIVE_SPACES)
        );
        assert_eq!(
            check_guild_name("Legacy's"),
            Err(NameError::INVALID_CHARACTER)
        );
        assert_eq!(
            check_guild_name("Steel4ever"),
            Err(NameError::INVALID_CHARACTER)
        );
        assert_eq!(
            check_guild_name("Brrr Club"),
            Err(NameError::THREE_CONSECUTIVE)
        );
        assert_eq!(check_guild_name(&"Ab ".repeat(8)[..23]), Ok(()));
        assert_eq!(
            check_guild_name("Abcdefghij Klmnopqrst Uvwxy"),
            Err(NameError::TOO_LONG)
        );
    }

    #[test]
    fn the_first_letter_fixes_the_charset() {
        // A Latin name cannot turn Cyrillic: the second letter is not of the first's charset.
        assert_eq!(check_guild_name("Aб"), Err(NameError::INVALID_CHARACTER));
        assert_eq!(check_guild_name("Бобр"), Ok(()));
        assert_eq!(
            check_guild_name("Ббб"),
            Err(NameError::THREE_CONSECUTIVE),
            "the fold joins the cases"
        );
    }

    #[test]
    fn russian_silent_letters_have_placement_rules() {
        assert_eq!(
            check_guild_name("ьяна"),
            Err(NameError::RUSSIAN_SILENT_CHARACTER_AT_BEGINNING_OR_END)
        );
        assert_eq!(
            check_guild_name("Объ"),
            Err(NameError::RUSSIAN_SILENT_CHARACTER_AT_BEGINNING_OR_END)
        );
        assert_eq!(
            check_guild_name("Подъь"),
            Err(NameError::RUSSIAN_CONSECUTIVE_SILENT_CHARACTERS)
        );
    }

    #[test]
    fn a_pet_name_takes_no_space_and_twelve_units() {
        assert_eq!(check_pet_name("Fluffy"), Ok(()));
        assert_eq!(check_pet_name(""), Err(NameError::NO_NAME));
        assert_eq!(
            check_pet_name("Mr Fluffy"),
            Err(NameError::INVALID_CHARACTER)
        );
        assert_eq!(check_pet_name("Abcdefghijkl"), Ok(()));
        assert_eq!(check_pet_name("Abcdefghijklm"), Err(NameError::TOO_LONG));
        assert_eq!(check_pet_name("가나다라마바사아"), Ok(()));
        assert_eq!(
            check_pet_name("가나다라마바사아자"),
            Err(NameError::TOO_LONG)
        );
    }

    #[test]
    fn every_code_shows_a_catalog_row() {
        for code in 0..=20 {
            let key = NameError(code).key();
            assert!(
                crate::messages::by_key(key).is_some(),
                "{code} shows {key}, not a catalog row"
            );
        }
        assert_eq!(NameError::INVALID_CHARACTER.key(), "ERR_NAME_INVALID");
        assert_eq!(NameError::MULTIPLE_APOSTROPHES.key(), "ERR_NAME_INVALID");
    }
}
