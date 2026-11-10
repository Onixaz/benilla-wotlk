//! The client's locale: which `Data\<locale>\` folder mounts (`0x402d50`) and, from the `Wow.ini`
//! that folder's archives carry (`0x4062e9`), which string slot every localized DBC column reads.

use std::sync::OnceLock;

/// The locale folders the client looks for, in its table order (`0xad3028`).
const FOLDERS: [&str; 12] = [
    "deDE", "enGB", "enUS", "esES", "frFR", "koKR", "zhCN", "zhTW", "enCN", "enTW", "esMX", "ruRU",
];

/// [`FOLDERS`]' enUS, what an unknown name resolves to (`0x635d90`) and the first folder a walk tries.
const ENUS_FOLDER: usize = 2;

/// The locales `Wow.ini` is matched against (`0xad2fe0`); an index here is a DBC string slot.
const INI_LOCALES: [&str; 9] = [
    "enUS", "koKR", "frFR", "deDE", "zhCN", "zhTW", "esES", "esMX", "ruRU",
];

/// The `locale` CVar for this process, set once at boot before the first chain opens.
static CVAR: OnceLock<String> = OnceLock::new();

/// Record the `locale` CVar as `Config.wtf` holds it; without one the chain opens `enUS` (`0x4068ce`).
pub fn set_locale_cvar(value: &str) {
    let _ = CVAR.set(value.to_string());
}

/// The running client's locale, recorded at boot ([`boot_locale`]).
static RUNNING: OnceLock<ClientLocale> = OnceLock::new();

/// The boot step: record the `locale` CVar, then open the install's chain once to learn the locale
/// it runs as, for everything that reads it through [`client_locale`] (`0x4068ce`, `0x406953`).
pub fn boot_locale(cvar: Option<&str>) -> &'static ClientLocale {
    if let Some(cvar) = cvar {
        set_locale_cvar(cvar);
    }
    let found = crate::wow_data()
        .and_then(|data| crate::Chain::open(&data).ok())
        .map(|chain| chain.locale().clone());
    RUNNING.get_or_init(|| found.unwrap_or_else(ClientLocale::lone_archive))
}

/// The running locale, enUS before [`boot_locale`] or with no install.
pub fn client_locale() -> &'static ClientLocale {
    static ENUS: OnceLock<ClientLocale> = OnceLock::new();
    RUNNING
        .get()
        .unwrap_or_else(|| ENUS.get_or_init(ClientLocale::lone_archive))
}

/// The `locale` CVar a [`crate::Chain::open`] resolves.
pub(crate) fn wanted() -> &'static str {
    CVAR.get().map_or("enUS", String::as_str)
}

/// Whether `name` is `folder`, its first four letters in any case (`0x635d90`).
fn names_folder(name: &str, folder: &str) -> bool {
    name.as_bytes()
        .get(..4)
        .is_some_and(|head| head.eq_ignore_ascii_case(folder.as_bytes()))
}

/// The folders holding `<loc>/locale-<loc>.MPQ`, which is what makes a locale exist (`0x402d50`).
pub(crate) fn existing(files: &[String]) -> Vec<&'static str> {
    FOLDERS
        .into_iter()
        .filter(|folder| {
            let archive = format!("{folder}/locale-{folder}.MPQ");
            files.iter().any(|f| f.eq_ignore_ascii_case(&archive))
        })
        .collect()
}

/// The folder a `locale` CVar value mounts: itself when it exists, else the next existing one in
/// table order, wrapping (`0x402d50`); `None` when no locale exists.
pub(crate) fn resolve(files: &[String], wanted: &str) -> Option<&'static str> {
    let found = existing(files);
    let start = FOLDERS
        .iter()
        .position(|folder| names_folder(wanted, folder))
        .unwrap_or(ENUS_FOLDER);
    (0..FOLDERS.len())
        .map(|step| FOLDERS[(start + step) % FOLDERS.len()])
        .find(|folder| found.contains(folder))
}

/// The running locale, as the client derives it from the mounted archives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientLocale {
    /// The mounted folder, also the value the `locale` CVar is written back to (`0x406953`).
    pub folder: String,
    /// The DBC string slot, the index the script layer's locale and plural rule read (`0xc5de9c`).
    pub index: usize,
    /// The locale as four letters packed big-endian, what the logon and caches send (`0x406381`).
    pub code: u32,
    /// Every folder that exists, in table order, for `GetExistingLocales` (`0x511c80`).
    pub existing: Vec<&'static str>,
}

impl ClientLocale {
    /// A single archive opened outside a `Data` folder: enUS, with nothing to choose from.
    pub(crate) fn lone_archive() -> Self {
        Self {
            folder: "enUS".to_string(),
            index: 0,
            code: packed("enUS"),
            existing: vec!["enUS"],
        }
    }

    /// The locale `folder` runs as from `Wow.ini`'s text (`0x4062e9`): with no usable file the
    /// slot stays 0 and the code 0, and an unknown name leaves the slot at 0 with its own code.
    pub(crate) fn from_ini(folder: &str, existing: Vec<&'static str>, ini: Option<&[u8]>) -> Self {
        let name = ini.and_then(ini_locale);
        Self {
            folder: folder.to_string(),
            index: name.as_deref().map_or(0, slot_of),
            code: name.as_deref().map_or(0, packed),
            existing,
        }
    }

    /// The script layer's name for this locale, enUS for an enGB client (`0x4dbfd0`).
    pub fn script_name(&self) -> &'static str {
        INI_LOCALES[self.index]
    }
}

/// The first four bytes of `name`, most significant first (`0x406381`).
pub fn packed(name: &str) -> u32 {
    name.bytes()
        .take(4)
        .fold(0u32, |code, b| (code << 8) + u32::from(b))
}

/// The string slot of an ini locale name; enGB shares enUS's (`0x4063a4`), an unknown name is 0.
fn slot_of(name: &str) -> usize {
    if name.eq_ignore_ascii_case("enGB") {
        return 0;
    }
    INI_LOCALES
        .iter()
        .position(|l| l.eq_ignore_ascii_case(name))
        .unwrap_or(0)
}

/// `[WoW Config]`'s `Language` then `Country`, joined and cut to the client's four letters
/// (`0x406341`); `None` when either is absent.
fn ini_locale(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    let (mut section, mut language, mut country) = (false, None, None);
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.eq_ignore_ascii_case("WoW Config");
        } else if let (true, Some((key, value))) = (section, line.split_once('=')) {
            if key.trim().eq_ignore_ascii_case("Language") {
                language = Some(value.trim().to_string());
            } else if key.trim().eq_ignore_ascii_case("Country") {
                country = Some(value.trim().to_string());
            }
        }
    }
    let mut joined = language? + &country?;
    joined.truncate(
        joined
            .char_indices()
            .nth(4)
            .map_or(joined.len(), |(i, _)| i),
    );
    Some(joined)
}

/// Every localized string block's enUS column, per table; a block is sixteen locale slots and a
/// flag column, of which the record builders read `[column + slot]` (`0x8a9c60` and its siblings).
const LOCALIZED: &[(&str, &[usize])] = &[
    ("Achievement", &[4, 21, 43]),
    ("Achievement_Category", &[2]),
    ("Achievement_Criteria", &[9]),
    ("AreaPOI", &[18, 35]),
    ("AreaTable", &[11]),
    ("AuctionHouse", &[4]),
    ("BarberShopStyle", &[2, 19]),
    ("BattlemasterList", &[11]),
    ("Cfg_Categories", &[4]),
    ("CharTitles", &[2, 19]),
    ("ChatChannels", &[3, 20]),
    ("ChrClasses", &[4, 21, 38]),
    ("ChrRaces", &[14, 31, 48]),
    ("CreatureFamily", &[10]),
    ("CreatureType", &[1]),
    ("CurrencyCategory", &[2]),
    ("DanceMoves", &[6]),
    ("DungeonEncounter", &[5]),
    ("EmotesTextData", &[1]),
    ("Exhaustion", &[5]),
    ("Faction", &[23, 40]),
    ("FactionGroup", &[3]),
    ("GMSurveyAnswers", &[3]),
    ("GMSurveyQuestions", &[1]),
    ("GMTicketCategory", &[1]),
    ("GameTips", &[1]),
    ("HolidayDescriptions", &[1]),
    ("HolidayNames", &[1]),
    ("ItemBagFamily", &[1]),
    ("ItemClass", &[3]),
    ("ItemLimitCategory", &[1]),
    ("ItemPetFood", &[1]),
    ("ItemPurchaseGroup", &[9]),
    ("ItemRandomProperties", &[7]),
    ("ItemRandomSuffix", &[1]),
    ("ItemSet", &[1]),
    ("ItemSubClass", &[10, 27]),
    ("ItemSubClassMask", &[2]),
    ("Languages", &[1]),
    ("LfgDungeonGroup", &[1]),
    ("LfgDungeons", &[1, 32]),
    ("LockType", &[1, 18, 35]),
    ("MailTemplate", &[1, 18]),
    ("Map", &[5, 23, 40]),
    ("MapDifficulty", &[3]),
    ("Package", &[3]),
    ("PetPersonality", &[1]),
    ("QuestInfo", &[1]),
    ("QuestSort", &[1]),
    ("Resistances", &[3]),
    ("ServerMessages", &[1]),
    ("SkillLine", &[3, 20, 38]),
    ("SkillLineCategory", &[1]),
    ("Spell", &[136, 153, 170, 187]),
    ("SpellDispelType", &[1]),
    ("SpellFocusObject", &[1]),
    ("SpellItemEnchantment", &[14]),
    ("SpellMechanic", &[1]),
    ("SpellRange", &[6, 23]),
    ("SpellShapeshiftForm", &[2]),
    ("Startup_Strings", &[2]),
    ("TalentTab", &[1]),
    ("TaxiNodes", &[5]),
    ("TotemCategory", &[1]),
    ("WMOAreaTable", &[11]),
    ("WorldSafeLocs", &[5]),
    ("WorldStateUI", &[5, 22, 42]),
];

/// The strings at `columns` of a DBC, by the row's id in column 0; an empty cell is `""`.
pub fn dbc_strings(
    chain: &mut crate::Chain,
    path: &str,
    columns: &[usize],
) -> anyhow::Result<std::collections::HashMap<u32, Vec<String>>> {
    let bytes = chain.read(path)?;
    anyhow::ensure!(
        bytes.len() >= 20 && &bytes[..4] == b"WDBC",
        "{path} is not a DBC"
    );
    let word = |at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
    };
    let (rows, fields, size, block) = (word(4), word(8), word(12), word(16));
    anyhow::ensure!(size == fields * 4, "{path} is not all four-byte columns");
    let strings = 20 + rows * size;
    anyhow::ensure!(bytes.len() >= strings + block, "{path} is cut short");
    let text = |offset: usize| {
        let tail = bytes.get(strings + offset..strings + block).unwrap_or(&[]);
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        String::from_utf8_lossy(&tail[..end]).into_owned()
    };
    Ok((0..rows)
        .map(|row| {
            let at = 20 + row * size;
            let cells = columns.iter().map(|&c| text(word(at + c * 4))).collect();
            (word(at) as u32, cells)
        })
        .collect())
}

/// Copy each localized block's running-locale slot over its enUS slot, so that every DBC reader
/// keeps reading the column it names; a no-op under enUS.
pub(crate) fn localize_dbc(path: &str, bytes: &mut [u8], slot: usize) {
    if slot == 0 {
        return;
    }
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let Some(stem) = name
        .len()
        .checked_sub(4)
        .filter(|&dot| name[dot..].eq_ignore_ascii_case(".dbc"))
        .map(|dot| &name[..dot])
    else {
        return;
    };
    let Some(&(_, columns)) = LOCALIZED
        .iter()
        .find(|(table, _)| table.eq_ignore_ascii_case(stem))
    else {
        return;
    };
    if bytes.len() < 20 || &bytes[..4] != b"WDBC" {
        return;
    }
    let word = |at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
    };
    let (rows, fields, size) = (word(4), word(8), word(12));
    if size != fields * 4 {
        return;
    }
    let end = 20 + rows * size;
    let Some(table) = bytes.get_mut(20..end) else {
        return;
    };
    for row in table.chunks_exact_mut(size) {
        for &column in columns.iter().filter(|&&c| c + slot < fields) {
            row.copy_within((column + slot) * 4..(column + slot) * 4 + 4, column * 4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_locale_exists_when_its_locale_archive_does() {
        let files = owned(&["enUS/locale-enUS.MPQ", "esES/LOCALE-esES.mpq", "esES/x.MPQ"]);
        assert_eq!(existing(&files), ["enUS", "esES"]);
        assert_eq!(existing(&owned(&["enUS/speech-enUS.MPQ"])), [] as [&str; 0]);
    }

    #[test]
    fn a_missing_locale_walks_to_the_next_existing_folder() {
        let files = owned(&["enUS/locale-enUS.MPQ", "esES/locale-esES.MPQ"]);
        assert_eq!(resolve(&files, "esES"), Some("esES"));
        assert_eq!(resolve(&files, "ESES"), Some("esES"));
        // deDE comes before enUS in the table, so it walks forward onto enUS.
        assert_eq!(resolve(&files, "deDE"), Some("enUS"));
        // An unknown name is enUS, and the walk wraps past the end of the table.
        assert_eq!(resolve(&files, "xxXX"), Some("enUS"));
        let only_german = owned(&["deDE/locale-deDE.MPQ"]);
        assert_eq!(resolve(&only_german, "ruRU"), Some("deDE"));
        assert_eq!(resolve(&[], "enUS"), None);
    }

    #[test]
    fn the_ini_picks_the_slot_and_the_code() {
        let es = ClientLocale::from_ini(
            "esES",
            vec!["esES"],
            Some(b"[WoW Config]\r\nVersion=1\r\nCountry=ES\r\nLanguage=es\r\nRegion=3\r\n"),
        );
        assert_eq!((es.index, es.script_name()), (6, "esES"));
        assert_eq!(es.code, 0x6573_4553);

        let gb = ClientLocale::from_ini(
            "enGB",
            vec!["enGB"],
            Some(b"[WoW Config]\nCountry=GB\nLanguage=en\n"),
        );
        assert_eq!((gb.index, gb.script_name()), (0, "enUS"));
        assert_eq!(gb.code, packed("enGB"));

        let none = ClientLocale::from_ini("esES", vec![], None);
        assert_eq!((none.index, none.code), (0, 0));
        let wrong_section =
            ClientLocale::from_ini("esES", vec![], Some(b"[Other]\nCountry=ES\nLanguage=es\n"));
        assert_eq!(wrong_section.code, 0);
    }

    /// Every localized block a table benilla reads holds is listed, each listed one is sixteen
    /// slots, and the Spanish data carries its strings at slot 6.
    #[test]
    fn the_localized_blocks_match_the_shipped_tables() {
        let data = crate::wow_data_or_skip!();
        let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let en = crate::Chain::open_locale(&data, "enUS").unwrap();
        for table in crate::dbc_tables::READ_TABLES {
            let Ok(bytes) = en.read(&format!("DBFilesClient/{table}.dbc")) else {
                continue;
            };
            let (rows, fields, size) = (
                word(&bytes, 4) as usize,
                word(&bytes, 8) as usize,
                word(&bytes, 12) as usize,
            );
            if size != fields * 4 || fields < 17 {
                continue;
            }
            let cell = |r: usize, c: usize| word(&bytes, 20 + r * size + c * 4);
            let listed = LOCALIZED
                .iter()
                .find(|(t, _)| t.eq_ignore_ascii_case(table))
                .map_or(&[][..], |(_, c)| *c);
            let empty_slots = |c: usize| (1..16).all(|k| (0..rows).all(|r| cell(r, c + k) == 0));
            for c in 0..=fields - 17 {
                let block =
                    empty_slots(c) && (0..rows).any(|r| cell(r, c) != 0 && cell(r, c + 16) != 0);
                assert!(
                    !block || listed.contains(&c),
                    "{table} column {c} is an unlisted block"
                );
            }
            for &c in listed {
                assert!(
                    empty_slots(c),
                    "{table} column {c} is not a localized block"
                );
            }
        }
        let existing = crate::Chain::open_locale(&data, "enUS")
            .unwrap()
            .locale()
            .existing
            .clone();
        if existing.contains(&"esES") {
            let es = crate::Chain::open_locale(&data, "esES").unwrap();
            assert_eq!(es.locale().script_name(), "esES");
            let tips = dbc_strings(
                &mut crate::Chain::open_locale(&data, "esES").unwrap(),
                "DBFilesClient\\GameTips.dbc",
                &[1],
            )
            .unwrap();
            assert!(
                tips.values().any(|row| !row[0].is_empty()),
                "the slot reads the Spanish text"
            );
        }
    }

    /// One table with a localized column at 1: two rows of [id, slot 0, .., slot 15, flags].
    fn wdbc(columns: usize, rows: &[Vec<u32>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"WDBC");
        for v in [rows.len(), columns, columns * 4, 1] {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for row in rows {
            for v in row {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out.push(0);
        out
    }

    #[test]
    fn the_running_slot_lands_on_the_enus_column() {
        let mut row = vec![7u32; 18];
        row[1] = 0;
        row[1 + 6] = 42;
        let mut bytes = wdbc(18, &[row.clone()]);
        let untouched = bytes.clone();

        // enUS: nothing moves, and a table with no localized block is left alone.
        localize_dbc("DBFilesClient\\GameTips.dbc", &mut bytes, 0);
        assert_eq!(bytes, untouched);
        localize_dbc("NotLocalized.dbc", &mut bytes, 6);
        assert_eq!(bytes, untouched);
        localize_dbc("DBFilesClient\\GameTips.txt", &mut bytes, 6);
        assert_eq!(bytes, untouched);

        localize_dbc("DBFilesClient\\GameTips.dbc", &mut bytes, 6);
        let column =
            |b: &[u8], i: usize| u32::from_le_bytes(b[20 + i * 4..24 + i * 4].try_into().unwrap());
        assert_eq!(
            column(&bytes, 1),
            42,
            "the esES slot is read as the enUS one"
        );
        assert_eq!(column(&bytes, 7), 42);
        assert_eq!(column(&bytes, 0), 7);
    }
}
