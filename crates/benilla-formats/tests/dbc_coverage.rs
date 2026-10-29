//! benilla's DBC tables against 12340's (`reference/3.3.5a-dbc.tsv`, generated from the client's
//! table loaders): each table benilla reads is one 12340 loads, held to the width 12340's loader
//! checks; each table 12340 reads is read here or listed in [`NOT_READ`]; and each table has one
//! schema in this crate, as a second copy is how a reader goes on parsing a layout the table no
//! longer has.
//!
//! A table comes in with the feature that reads it, never on its own: a schema nothing reads is
//! checked for its width alone. Its TSV row names 12340's readers (`reader_vas`, with the Lua
//! bindings and opcode handlers among them), and those functions are the spec for both the
//! schema's fields and the feature. A table read only through helpers is followed up to the
//! binding or handler that calls them, and the readers are a floor: a read through a computed
//! pointer is not listed. A table several features read lands with the first of them. Where
//! benilla serves a table's rows from a constant, reading the table replaces the constant.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use benilla_formats::{dbc_width, READ_TABLES};

/// Tables 12340 reads that benilla does not, each with what it backs in the reference and the
/// open issue that builds it, where one does.
const NOT_READ: &[(&str, &str)] = &[
    ("AreaGroup", "#240 GetTradeSkillTools' area requirement"),
    ("BannedAddOns", "#245 the addon ban list"),
    (
        "Cfg_Configs",
        "#245 realm type rules: GetRealmInfo; realm_select hardcodes its rows",
    ),
    ("CreatureMovementInfo", "#243 creature movement"),
    ("DanceMoves", "#243 dance studio emotes"),
    ("DeclinedWord", "#242 Russian name declension"),
    ("DeclinedWordCases", "#242 Russian name declension"),
    ("DestructibleModelData", "#243 destructible buildings"),
    ("DungeonMapChunk", "#238 dungeon map levels"),
    ("FileData", "#245 the movie file table"),
    ("GameObjectArtKit", "#243 game object art kits"),
    ("GemProperties", "#202 gems and sockets"),
    ("GMSurveyAnswers", "#245 GM surveys"),
    ("GMSurveyCurrentSurvey", "#245 GM surveys"),
    ("GMSurveyQuestions", "#245 GM surveys"),
    ("GMSurveySurveys", "#245 GM surveys"),
    ("gtNPCManaCostScaler", "#233 NPC spell costs"),
    (
        "Item",
        "#239 item class, display and sheath by id: GetItemIcon, PickupItem",
    ),
    (
        "ItemLimitCategory",
        "#239 unique-equipped limits: GetItemUniqueness",
    ),
    ("ItemPurchaseGroup", "#239 merchant purchase groups"),
    ("ItemRandomSuffix", "#239 random suffix item names"),
    ("LoadingScreenTaxiSplines", "#243 the taxi loading screen"),
    ("MailTemplate", "#245 templated mail text: GetInboxText"),
    ("Movie", "#130 movies"),
    ("MovieFileData", "#130 movies"),
    ("MovieVariation", "#130 movies"),
    ("NamesProfanity", "#242 character name checks"),
    ("NamesReserved", "#242 character name checks"),
    ("ObjectEffect", "#243 object effects"),
    ("ObjectEffectGroup", "#243 object effects"),
    ("ObjectEffectModifier", "#243 object effects"),
    ("ObjectEffectPackage", "#243 object effects"),
    ("ObjectEffectPackageElem", "#243 object effects"),
    (
        "OverrideSpellData",
        "#235 the vehicle and override action bar",
    ),
    (
        "PaperDollItemFrame",
        "#239 equipment slot names: GetInventorySlotInfo",
    ),
    (
        "PowerDisplay",
        "#240 alternate power display: UnitPowerType",
    ),
    ("RandPropPoints", "#239 random property and suffix stats"),
    ("Resistances", "#233 resistance names"),
    ("ScalingStatDistribution", "#239 heirloom stat scaling"),
    ("ScalingStatValues", "#239 heirloom stat scaling"),
    ("ScreenEffect", "#243 screen effects"),
    ("SkillCostsData", "#240 skill training costs"),
    ("SkillTiers", "#240 skill tiers"),
    ("SoundEntriesAdvanced", "#244 sound entry extensions"),
    ("SpellDifficulty", "#240 per-difficulty spell variants"),
    ("SpellItemEnchantmentCondition", "#202 meta gem conditions"),
    ("SpellMissile", "#110 spell missiles"),
    ("SpellMissileMotion", "#110 spell missile motion"),
    ("SpellVisualKitAreaModel", "#110 area spell visual kits"),
    (
        "SpellVisualKitModelAttach",
        "#110 spell visual kit attachments",
    ),
    ("Startup_Strings", "#245 the client's startup error text"),
    ("StringLookups", "#245 string lookups"),
    ("TransportPhysics", "#243 transport rocking"),
    ("TransportRotation", "#243 transport rotation"),
    ("VideoHardware", "#245 graphics defaults by video card"),
    ("WorldChunkSounds", "#244 per-chunk zone sounds"),
    (
        "WorldSafeLocs",
        "#238 graveyards on the map: GetMapLandmarkInfo",
    ),
    ("WorldStateZoneSounds", "#244 world-state zone sounds"),
];

/// Tables benilla reads that 12340 loads and never reads: each is a reader the reference does
/// not have, so each names what reads it here.
const READ_PAST_12340: &[(&str, &str)] = &[];

struct Table {
    columns: usize,
    row_size: usize,
    readers: usize,
}

fn reference() -> BTreeMap<String, Table> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/3.3.5a-dbc.tsv");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines = text.lines().filter(|l| !l.starts_with('#'));
    let header: Vec<&str> = lines.next().unwrap().split('\t').collect();
    let col = |name: &str| header.iter().position(|h| *h == name).unwrap();
    let (table, columns, row_size, readers) = (
        col("table"),
        col("columns"),
        col("row_size"),
        col("readers"),
    );
    lines
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            let n = |i: usize| f[i].parse::<usize>().unwrap();
            let row = Table {
                columns: n(columns),
                row_size: n(row_size),
                readers: n(readers),
            };
            (f[table].to_owned(), row)
        })
        .collect()
}

#[test]
fn the_reference_is_every_12340_loader() {
    let reference = reference();
    assert_eq!(reference.len(), 241, "12340 has 241 table loaders");
}

#[test]
fn the_install_files_have_the_widths_12340_checks() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).unwrap();
    let mut wrong = Vec::new();
    for (table, r) in reference() {
        let bytes = chain
            .read_file(&format!("DBFilesClient\\{table}.dbc"))
            .unwrap();
        let header = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let file = (header(8), header(12));
        if &bytes[..4] != b"WDBC" || file != (r.columns, r.row_size) {
            wrong.push(format!(
                "{table}: file {file:?}, 12340 checks {:?}",
                (r.columns, r.row_size)
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn every_table_read_is_a_12340_table_at_its_width() {
    let reference = reference();
    let mut wrong = Vec::new();
    for table in READ_TABLES {
        let ours = dbc_width(table).unwrap();
        match reference.get(*table) {
            None => wrong.push(format!("{table}: 12340 has no such table")),
            Some(r) if (r.columns, r.row_size) != ours => wrong.push(format!(
                "{table}: ours {ours:?}, 12340 checks {:?}",
                (r.columns, r.row_size)
            )),
            Some(_) => {}
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn every_table_12340_reads_is_read_or_listed() {
    let reference = reference();
    let read: BTreeSet<&str> = READ_TABLES.iter().copied().collect();
    let listed: BTreeMap<&str, &str> = NOT_READ.iter().copied().collect();
    assert_eq!(listed.len(), NOT_READ.len(), "a table listed twice");
    let unlisted: Vec<_> = reference
        .iter()
        .filter(|(t, r)| {
            r.readers > 0 && !read.contains(t.as_str()) && !listed.contains_key(t.as_str())
        })
        .map(|(t, _)| t)
        .collect();
    assert!(
        unlisted.is_empty(),
        "12340 reads these, benilla does not, and NOT_READ does not say what they back: {unlisted:?}"
    );
    let stale: Vec<_> = listed
        .keys()
        .filter(|t| read.contains(*t) || reference.get(**t).is_none_or(|r| r.readers == 0))
        .collect();
    assert!(
        stale.is_empty(),
        "NOT_READ lists tables now read, never read by 12340, or unknown: {stale:?}"
    );
}

#[test]
fn a_table_12340_never_reads_is_read_only_where_listed() {
    let reference = reference();
    let listed: BTreeMap<&str, &str> = READ_PAST_12340.iter().copied().collect();
    let past: BTreeSet<&str> = READ_TABLES
        .iter()
        .copied()
        .filter(|t| reference.get(*t).is_some_and(|r| r.readers == 0))
        .collect();
    assert_eq!(
        past,
        listed.keys().copied().collect(),
        "the tables benilla reads that 12340 loads and never reads"
    );
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The crate's loader sources, by path under `src/`: all but the `bin/` tools and the `tests.rs`
/// fixture modules, whose synthetic tables build the loader's own constants.
fn loader_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files
        .into_iter()
        .filter(|f| f.file_name().is_some_and(|n| n != "tests.rs"))
        .filter(|f| !f.strip_prefix(&src).unwrap().starts_with("bin"))
        .map(|f| {
            let at = f.strip_prefix(&src).unwrap().display().to_string();
            (at, std::fs::read_to_string(&f).unwrap())
        })
        .collect()
}

/// The string literal opening `rest`, past leading whitespace.
fn leading_literal(rest: &str) -> Option<&str> {
    let rest = rest.trim_start().strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

/// The tables a source file builds a schema for: the name a `Schema::new` or `…schema(` call
/// opens with.
fn schema_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (i, m) in text.match_indices("chema(") {
        if let Some(name) = leading_literal(&text[i + m.len()..]) {
            names.push(name.to_owned());
        }
    }
    for (i, m) in text.match_indices("Schema::new(") {
        if let Some(name) = leading_literal(&text[i + m.len()..]) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The tables a source file opens by path: each literal `DBFilesClient\\<table>.dbc` in its code,
/// past its comments and a formatted name.
fn dbc_paths(text: &str) -> Vec<String> {
    const PREFIX: &str = "DBFilesClient\\\\";
    let mut names = Vec::new();
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or("");
        for (i, _) in code.match_indices(PREFIX) {
            let rest = &code[i + PREFIX.len()..];
            let Some(name) = rest.split(".dbc").next() else {
                continue;
            };
            if rest.contains(".dbc")
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                names.push(name.to_owned());
            }
        }
    }
    names
}

#[test]
fn the_scan_sees_each_way_a_table_is_named() {
    let src = "Schema::new(\"A\"); n_u32_schema(\n    \"B\", 3); id_name_schema(\"C\", 1, 18)";
    assert_eq!(schema_names(src), ["B", "C", "A"]);
    let src = "read(\"DBFilesClient\\\\D.dbc\"); format!(\"DBFilesClient\\\\{t}.dbc\") // \
               DBFilesClient\\\\E.dbc";
    assert_eq!(dbc_paths(src), ["D"]);
}

#[test]
fn every_table_is_declared_once() {
    let mut sites: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (at, text) in loader_sources() {
        for name in schema_names(&text) {
            sites.entry(name).or_default().push(at.clone());
        }
    }
    assert!(sites.len() > 50, "the scan found the schemas: {sites:?}");
    let doubled: Vec<_> = sites.iter().filter(|(_, at)| at.len() > 1).collect();
    assert!(
        doubled.is_empty(),
        "tables with more than one schema: {doubled:?}"
    );
}

#[test]
fn the_registry_names_every_table_a_loader_reads() {
    let read: BTreeSet<&str> = READ_TABLES.iter().copied().collect();
    assert_eq!(read.len(), READ_TABLES.len(), "a table registered twice");
    let mut missing = BTreeMap::new();
    for (at, text) in loader_sources() {
        for name in schema_names(&text).into_iter().chain(dbc_paths(&text)) {
            if !read.contains(name.as_str()) {
                missing.insert(name, at.clone());
            }
        }
    }
    assert!(
        missing.is_empty(),
        "loaders read tables `dbc_tables.rs` does not register: {missing:?}"
    );
}
