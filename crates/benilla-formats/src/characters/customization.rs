//! Character-creation source data from ChrRaces, CharBaseInfo and CharStartOutfit, and per (race,
//! sex) the CharSections grid the appearance dials walk (`0x4f3dd0`) with its count of
//! CharacterFacialHairStyles rows (`0x4f41b0`).

use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

use super::shipped::{Shipped, SHIPPED};

const CHR_RACES: &str = "DBFilesClient\\ChrRaces.dbc";
const CHAR_BASE_INFO: &str = "DBFilesClient\\CharBaseInfo.dbc";
const CHAR_START_OUTFIT: &str = "DBFilesClient\\CharStartOutfit.dbc";
/// `CharBaseInfo.dbc`'s columns and row bytes: race and class, one byte each.
pub(crate) const CHAR_BASE_INFO_WIDTH: (usize, usize) = (2, 2);
const CHAR_FACIAL_HAIR_STYLES: &str = "DBFilesClient\\CharacterFacialHairStyles.dbc";
const CHAR_SECTIONS: &str = "DBFilesClient\\CharSections.dbc";
const NAME_GEN: &str = "DBFilesClient\\NameGen.dbc";

/// CharSections `BaseSection` values the grid keeps, skin through underwear.
const SECTION_GRID_TYPES: usize = 5;

/// The `ChrRaces` ids a player can create, ascending: the eight original races, Blood Elf and
/// Draenei; the shipped `CharBaseInfo` lists exactly these.
pub(super) const PLAYABLE_RACES: [u8; 10] = [1, 2, 3, 4, 5, 6, 7, 8, 10, 11];

/// The `CharBaseInfo` misparse guard: every shipped pair is present; a patch's extra pairs pass.
fn shipped_combos_present(shipped: &Shipped, combos: &HashSet<(u8, u8)>) -> Result<()> {
    for &(race, classes) in shipped.combos {
        let missing: Vec<u8> = classes
            .iter()
            .copied()
            .filter(|&c| !combos.contains(&(race, c)))
            .collect();
        if !missing.is_empty() {
            bail!(
                "CharBaseInfo misparse: race {race} lacks shipped classes {missing:?} \
                 (check the 2-byte race/class layout)"
            );
        }
    }
    Ok(())
}

/// One worn item of a CharStartOutfit row: its ItemDisplayInfo id and its InventoryType.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartOutfitItem {
    pub display_id: u32,
    pub inv_type: u8,
}

/// One (race, sex)'s CharSections grid as `BuildComponentArray` (`0x4f3dd0`) lays it out: per base
/// section, skin, face, facial hair, hair, underwear, `[variation][color]`, each slot the `Flags` of
/// the last row at it or `None`; a variation's width is its widest colour plus one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharSectionGrid {
    pub sections: [Vec<Vec<Option<u32>>>; SECTION_GRID_TYPES],
    /// `CharacterFacialHairStyles.dbc`'s rows for the race and sex (`CountFacialFeatures`).
    pub facial_hair_styles: u32,
}

/// One `ChrRaces.dbc` row as the create screen reads it, every row in table order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateRaceRow {
    pub id: u8,
    /// Field 1: bit `0x1` keeps the race off the create screen (`0x4e1c20`).
    pub flags: u32,
    /// Field 2, the race's `FactionTemplate` id.
    pub faction_template: u32,
    /// The expansion level the race needs.
    pub required_expansion: u32,
    /// `Name_Lang`, `Name_female_lang` and `Name_male_lang` at the enUS slot.
    pub name: String,
    pub name_female: String,
    pub name_male: String,
    /// `clientFileString`.
    pub file: String,
    /// `ClientPrefix`, which names a helm's per-race model (`0x4e7800`).
    pub client_prefix: String,
}

/// One `NameGen.dbc` row: a name the random-name generator learns for a race and sex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameGenRow {
    pub name: String,
    pub race: u32,
    pub sex: u32,
}

/// Character-creation source data, loaded once from the DBCs ([`Self::load`]).
pub struct CharCreateCatalog {
    /// race → (male displayId, female displayId), from ChrRaces cols 4/5.
    displays: HashMap<u8, (u32, u32)>,
    /// race → ChrRaces fileString, the GlueStrings key stem (`RACE_INFO_<FILE>`).
    files: HashMap<u8, String>,
    /// race → ChrRaces `Name_Lang` at the enUS slot, the race name `GetCharacterInfo` returns.
    names: HashMap<u8, String>,
    /// race → ([male, female] facial-hair token, hair token).
    custom_tokens: HashMap<u8, ([String; 2], String)>,
    combos: HashSet<(u8, u8)>,
    grids: HashMap<(u8, u8), CharSectionGrid>,
    /// (race, class, sex) → the worn starting-outfit items.
    start_outfits: HashMap<(u8, u8, u8), Vec<StartOutfitItem>>,
    /// Every `ChrRaces.dbc` row, table order.
    race_rows: Vec<CreateRaceRow>,
    /// `CharBaseInfo.dbc`'s (race, class) pairs in row order, the order `0x4e1ed0` lists a race's
    /// classes in.
    base_info: Vec<(u8, u8)>,
    /// `NameGen.dbc`, table order; empty when the table does not load.
    name_gen: Vec<NameGenRow>,
}

impl CharCreateCatalog {
    /// The body displayId for a (race, sex), resolved through CreatureDisplayInfo like any unit.
    /// The reference reads the race row's male id for gender 0 and the female for 1 and nothing
    /// for any other gender byte (`0x5ec4c0`), and a zero id finds no model (`0x5ec485`).
    pub fn body_display(&self, race: u8, sex: u8) -> Option<u32> {
        let &(male, female) = self.displays.get(&race)?;
        let id = match sex {
            0 => male,
            1 => female,
            _ => return None,
        };
        (id != 0).then_some(id)
    }

    /// Whether a race may be created as a class (a `CharBaseInfo` row).
    pub fn allows(&self, race: u8, class: u8) -> bool {
        self.combos.contains(&(race, class))
    }

    /// The ChrRaces fileString for a race (`"Human"`, `"Scourge"`), the GlueStrings key stem.
    pub fn race_file(&self, race: u8) -> Option<&str> {
        self.files.get(&race).map(String::as_str)
    }

    /// The ChrRaces `Name_Lang` name for a race (`"Night Elf"`, `"Undead"`) at the enUS slot:
    /// `GetCharacterInfo`'s second return.
    pub fn race_name(&self, race: u8) -> Option<&str> {
        self.names.get(&race).map(String::as_str)
    }

    /// A race's hair glue token (`"NORMAL"`, `"HORNS"`), keying `HAIR_<tok>_STYLE`/`_COLOR`.
    pub fn hair_customization(&self, race: u8) -> Option<&str> {
        self.custom_tokens.get(&race).map(|(_, h)| h.as_str())
    }

    /// A (race, sex)'s facial-hair glue token, keying `FACIAL_HAIR_<tok>`; the glue hides the dial
    /// when `GetFacialHairCustomization()` is `"NONE"`.
    pub fn facial_hair_customization(&self, race: u8, sex: u8) -> Option<&str> {
        self.custom_tokens
            .get(&race)
            .map(|(f, _)| f[(sex as usize).min(1)].as_str())
    }

    /// The classes a race may be created as, ascending by id: the client lists them in CharBaseInfo
    /// row order (`0x4706b0`), which is ascending class id in the shipped file.
    pub fn classes_for_race(&self, race: u8) -> Vec<u8> {
        let mut cs: Vec<u8> = self
            .combos
            .iter()
            .filter(|&&(r, _)| r == race)
            .map(|&(_, c)| c)
            .collect();
        cs.sort_unstable();
        cs
    }

    /// Every `ChrRaces.dbc` row in table order.
    pub fn race_rows(&self) -> &[CreateRaceRow] {
        &self.race_rows
    }

    /// `CharBaseInfo.dbc`'s (race, class) pairs in row order.
    pub fn base_info(&self) -> &[(u8, u8)] {
        &self.base_info
    }

    /// `NameGen.dbc`'s rows in table order.
    pub fn name_gen(&self) -> &[NameGenRow] {
        &self.name_gen
    }

    /// The CharSections grid the appearance dials of a (race, sex) walk.
    pub fn section_grid(&self, race: u8, sex: u8) -> Option<&CharSectionGrid> {
        self.grids.get(&(race, sex))
    }

    /// The worn level-1 gear the create preview dresses a (race, class, sex) in.
    pub fn start_outfit(&self, race: u8, class: u8, sex: u8) -> &[StartOutfitItem] {
        self.start_outfits
            .get(&(race, class, sex))
            .map_or(&[], Vec::as_slice)
    }

    /// Load the catalog from the patch chain, checked against the shipped rows.
    pub fn load(chain: &mut Chain) -> Result<Self> {
        let (displays, files, names, custom_tokens, race_rows) = load_races(chain)?;
        let base_info = load_base_info(chain)?;
        let combos: HashSet<(u8, u8)> = base_info.iter().copied().collect();
        let name_gen = load_name_gen(chain).unwrap_or_default();
        let start_outfits = load_start_outfits(chain)?;
        let mut grids = load_section_grids(chain)?;
        for (key, count) in load_facial_hair_counts(chain)? {
            grids.entry(key).or_default().facial_hair_styles = count;
        }

        let catalog = Self {
            displays,
            files,
            names,
            custom_tokens,
            combos,
            grids,
            start_outfits,
            race_rows,
            base_info,
            name_gen,
        };
        catalog.self_check()?;
        Ok(catalog)
    }

    fn self_check(&self) -> Result<()> {
        let shipped = &SHIPPED;
        shipped_combos_present(shipped, &self.combos)?;
        for &(race, file) in shipped.files {
            if self.race_file(race) != Some(file) {
                bail!(
                    "ChrRaces misparse: race {race} fileString {:?} != known {file:?} \
                     (check the string-column positions in load_races)",
                    self.race_file(race)
                );
            }
        }
        let hw = self.start_outfit(1, 1, 0);
        for &(disp, inv) in shipped.human_warrior {
            if !hw
                .iter()
                .any(|it| it.display_id == disp && it.inv_type == inv)
            {
                bail!(
                    "CharStartOutfit misparse: Human Warrior male missing display {disp} inv {inv} \
                     (got {hw:?}) — check the packed layout"
                );
            }
        }
        for &race in &PLAYABLE_RACES {
            for sex in [0u8, 1] {
                if self.body_display(race, sex).unwrap_or(0) == 0 {
                    bail!("ChrRaces: race {race} sex {sex} has no body displayId");
                }
                if self.facial_hair_customization(race, sex).is_none()
                    || self.hair_customization(race).is_none()
                {
                    bail!("ChrRaces: race {race} has no customization tokens");
                }
                let g = self
                    .section_grid(race, sex)
                    .with_context(|| format!("no CharSections grid for race {race} sex {sex}"))?;
                if g.sections[0].is_empty()
                    || g.sections[1].is_empty()
                    || g.sections[3].is_empty()
                    || g.facial_hair_styles == 0
                {
                    bail!("CharSections: race {race} sex {sex} lacks skins, faces, hair or beards");
                }
            }
        }
        Ok(())
    }
}

/// `ChrRaces.dbc`, the one schema every reader of the table shares: the string columns benilla
/// reads typed as strings, every other column a `u32`.
pub(crate) fn chr_races_schema(l: &crate::dbc_layout::ChrRaces) -> Schema {
    let facial = l.facial_hair_customization;
    let strings = [
        l.client_prefix,
        l.client_file,
        l.name,
        l.name_female,
        l.name_male,
        facial,
        facial + 1,
        l.hair_customization,
    ];
    let mut schema = Schema::new("ChrRaces");
    for i in 0..l.fields {
        let ty = if strings.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new(format!("f{i}"), ty));
    }
    schema
}

/// ChrRaces → per playable race: displayIds (cols 4/5), fileString, facial-hair and hair tokens;
/// other string columns are read as ignored `u32`s.
#[allow(clippy::type_complexity)] // one pass over one DBC → the catalog's race-keyed maps and rows
fn load_races(
    chain: &mut Chain,
) -> Result<(
    HashMap<u8, (u32, u32)>,
    HashMap<u8, String>,
    HashMap<u8, String>,
    HashMap<u8, ([String; 2], String)>,
    Vec<CreateRaceRow>,
)> {
    let bytes = chain
        .read_file(CHR_RACES)
        .with_context(|| format!("reading {CHR_RACES}"))?;
    let l = &crate::dbc_layout::LAYOUTS.chr_races;
    let facial = l.facial_hair_customization;
    let rs = parse(&bytes, chr_races_schema(l), "ChrRaces")?;
    let mut displays = HashMap::new();
    let mut files = HashMap::new();
    let mut names = HashMap::new();
    let mut tokens = HashMap::new();
    let mut rows = Vec::new();
    for r in rs.records() {
        let Some(race) = u32_at(r, 0) else { continue };
        let race = race as u8;
        let text = |field| str_at(&rs, r, field).unwrap_or_default();
        rows.push(CreateRaceRow {
            id: race,
            flags: u32_at(r, 1).unwrap_or(0),
            faction_template: u32_at(r, 2).unwrap_or(0),
            required_expansion: u32_at(r, l.required_expansion).unwrap_or(0),
            name: text(l.name),
            name_female: text(l.name_female),
            name_male: text(l.name_male),
            file: text(l.client_file),
            client_prefix: text(l.client_prefix),
        });
        if !PLAYABLE_RACES.contains(&race) {
            continue;
        }
        if let (Some(male), Some(female)) = (u32_at(r, 4), u32_at(r, 5)) {
            displays.insert(race, (male, female));
        }
        if let Some(file) = str_at(&rs, r, l.client_file) {
            files.insert(race, file);
        }
        if let Some(name) = str_at(&rs, r, l.name) {
            names.insert(race, name);
        }
        if let (Some(fm), Some(ff), Some(hair)) = (
            str_at(&rs, r, facial),
            str_at(&rs, r, facial + 1),
            str_at(&rs, r, l.hair_customization),
        ) {
            tokens.insert(race, ([fm, ff], hair));
        }
    }
    Ok((displays, files, names, tokens, rows))
}

/// `NameGen.dbc`'s four columns: `ID`, `Name`, `RaceID`, `Sex`.
pub(crate) fn name_gen_schema() -> Schema {
    let mut schema = Schema::new("NameGen");
    schema.add_field(SchemaField::new("id", FieldType::UInt32));
    schema.add_field(SchemaField::new("name", FieldType::String));
    schema.add_field(SchemaField::new("race", FieldType::UInt32));
    schema.add_field(SchemaField::new("sex", FieldType::UInt32));
    schema
}

/// `NameGen.dbc`: `ID`, `Name`, `RaceID`, `Sex`, in table order (`0x4e6790` takes one race and
/// sex's run of rows).
fn load_name_gen(chain: &mut Chain) -> Result<Vec<NameGenRow>> {
    let bytes = chain
        .read_file(NAME_GEN)
        .with_context(|| format!("reading {NAME_GEN}"))?;
    let rs = parse(&bytes, name_gen_schema(), "NameGen")?;
    Ok(rs
        .records()
        .iter()
        .filter_map(|r| {
            Some(NameGenRow {
                name: str_at(&rs, r, 1)?,
                race: u32_at(r, 2)?,
                sex: u32_at(r, 3)?,
            })
        })
        .collect())
}

/// CharBaseInfo → (race, class) pairs in row order: two one-byte fields, parsed by hand off the
/// record block.
fn load_base_info(chain: &mut Chain) -> Result<Vec<(u8, u8)>> {
    let bytes = chain
        .read_file(CHAR_BASE_INFO)
        .with_context(|| format!("reading {CHAR_BASE_INFO}"))?;
    if bytes.len() < 20 || &bytes[0..4] != b"WDBC" {
        bail!("CharBaseInfo: not a WDBC file");
    }
    let record_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let field_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let record_size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if (field_count, record_size) != CHAR_BASE_INFO_WIDTH {
        bail!(
            "CharBaseInfo: unexpected layout (fields {field_count}, record size {record_size}; \
             expected {CHAR_BASE_INFO_WIDTH:?})"
        );
    }
    let data = &bytes[20..];
    let mut pairs = Vec::with_capacity(record_count);
    for i in 0..record_count {
        let off = i * record_size;
        if off + 2 > data.len() {
            bail!("CharBaseInfo: record {i} runs past the file");
        }
        pairs.push((data[off], data[off + 1]));
    }
    Ok(pairs)
}

/// CharStartOutfit → (race, class, sex) → worn items, parsed by hand: `ID` u32 @0, race/class/gender
/// u8 @4/5/6, then the item, display and InventoryType arrays at the layout's offsets (24 slots,
/// displays @104, types @200 in 296 bytes). A slot is kept when both are ≥ 1, dropping the
/// `-1` tail and the InventoryType-0 bags.
fn load_start_outfits(chain: &mut Chain) -> Result<HashMap<(u8, u8, u8), Vec<StartOutfitItem>>> {
    let bytes = chain
        .read_file(CHAR_START_OUTFIT)
        .with_context(|| format!("reading {CHAR_START_OUTFIT}"))?;
    if bytes.len() < 20 || &bytes[0..4] != b"WDBC" {
        bail!("CharStartOutfit: not a WDBC file");
    }
    let l = &crate::dbc_layout::LAYOUTS.char_start_outfit;
    let record_count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let field_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let record_size = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if field_count != l.fields || record_size != l.record_size {
        bail!(
            "CharStartOutfit: unexpected layout (fields {field_count}, record size {record_size}; \
             expected {}/{})",
            l.fields,
            l.record_size
        );
    }
    let data = &bytes[20..];
    let i32_at = |rec: &[u8], off: usize| i32::from_le_bytes(rec[off..off + 4].try_into().unwrap());
    let mut map: HashMap<(u8, u8, u8), Vec<StartOutfitItem>> = HashMap::new();
    for i in 0..record_count {
        let off = i * record_size;
        if off + record_size > data.len() {
            bail!("CharStartOutfit: record {i} runs past the file");
        }
        let rec = &data[off..off + record_size];
        let (race, class, sex) = (rec[4], rec[5], rec[6]);
        let mut items = Vec::new();
        for slot in 0..l.slots {
            let display = i32_at(rec, l.display_ids + slot * 4);
            let inv = i32_at(rec, l.inventory_types + slot * 4);
            if display >= 1 && inv >= 1 {
                items.push(StartOutfitItem {
                    display_id: display as u32,
                    inv_type: inv as u8,
                });
            }
        }
        map.insert((race, class, sex), items);
    }
    Ok(map)
}

/// CharSections → per (race, sex) the grid `BuildComponentArray` (`0x4f3dd0`) builds: rows of
/// base section 5 and up are skipped, and a later row at a slot replaces an earlier one.
fn load_section_grids(chain: &mut Chain) -> Result<HashMap<(u8, u8), CharSectionGrid>> {
    let bytes = chain
        .read_file(CHAR_SECTIONS)
        .with_context(|| format!("reading {CHAR_SECTIONS}"))?;
    // ID, Race, Sex, SectionType, then the variation, color, textures and flags where the layout
    // puts them; we read the ints.
    let l = &crate::dbc_layout::LAYOUTS.char_sections;
    let rs = parse(
        &bytes,
        super::sections::char_sections_schema(l),
        "CharSections",
    )?;
    let mut grids: HashMap<(u8, u8), CharSectionGrid> = HashMap::new();
    for r in rs.records() {
        let (Some(race), Some(sex), Some(ty), Some(var), Some(color), Some(flags)) = (
            u32_at(r, 1),
            u32_at(r, 2),
            u32_at(r, 3),
            u32_at(r, l.variation),
            u32_at(r, l.color),
            u32_at(r, l.flags),
        ) else {
            continue;
        };
        if ty as usize >= SECTION_GRID_TYPES {
            continue;
        }
        let grid = &mut grids.entry((race as u8, sex as u8)).or_default().sections[ty as usize];
        let (var, color) = (var as usize, color as usize);
        if grid.len() <= var {
            grid.resize(var + 1, Vec::new());
        }
        let colors = &mut grid[var];
        if colors.len() <= color {
            colors.resize(color + 1, None);
        }
        colors[color] = Some(flags);
    }
    Ok(grids)
}

/// CharacterFacialHairStyles → (race, sex) → its row count, keyed on cols 0/1 (`0x4f41b0`).
fn load_facial_hair_counts(chain: &mut Chain) -> Result<HashMap<(u8, u8), u32>> {
    let bytes = chain
        .read_file(CHAR_FACIAL_HAIR_STYLES)
        .with_context(|| format!("reading {CHAR_FACIAL_HAIR_STYLES}"))?;
    let rs = parse(
        &bytes,
        super::geosets::char_facial_hair_schema(
            &crate::dbc_layout::LAYOUTS.character_facial_hair_styles,
        ),
        "CharacterFacialHairStyles",
    )?;
    let mut map: HashMap<(u8, u8), u32> = HashMap::new();
    for r in rs.records() {
        if let (Some(race), Some(sex)) = (u32_at(r, 0), u32_at(r, 1)) {
            *map.entry((race as u8, sex as u8)).or_default() += 1;
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every playable race and sex has a file for the Death Knight hood under its own
    /// `ClientPrefix` (`0x4e7800`), the expansion races included.
    #[test]
    fn every_playable_race_has_its_helm_file_by_client_prefix() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let c = CharCreateCatalog::load(&mut chain).expect("catalog");
        let displays = crate::load_item_display_catalog(&mut chain).expect("item displays");
        let hood = c
            .start_outfit(1, 6, 0)
            .iter()
            .find(|i| i.inv_type == 1)
            .and_then(|i| displays.get(i.display_id))
            .and_then(|d| d.model[0].clone())
            .expect("the Death Knight outfit's helm model");
        let stem = hood.rsplit_once('.').map_or(hood.as_str(), |(s, _)| s);
        for row in c
            .race_rows()
            .iter()
            .filter(|r| PLAYABLE_RACES.contains(&r.id))
        {
            assert!(
                !row.client_prefix.is_empty(),
                "race {} has a prefix",
                row.id
            );
            for letter in ['M', 'F'] {
                let path = format!(
                    "Item\\ObjectComponents\\Head\\{stem}_{}{letter}.m2",
                    row.client_prefix
                );
                assert!(chain.read_file(&path).is_ok(), "{path} is in the archive");
            }
        }
    }

    /// The create screen's columns off the install: the expansion races and the class gate read
    /// at their own fields, and `NameGen.dbc` loaded.
    #[test]
    fn the_create_rows_read_their_own_columns() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let c = CharCreateCatalog::load(&mut chain).expect("catalog");
        let row = |id| c.race_rows().iter().find(|r| r.id == id).expect("race row");
        assert_eq!(row(1).required_expansion, 0);
        assert_eq!(row(10).required_expansion, 1);
        assert_eq!(row(11).required_expansion, 1);
        assert_eq!(row(1).file, "Human");
        // enUS fills the neutral name alone; the gendered columns are empty there.
        assert_eq!(row(4).name, "Night Elf");
        assert_ne!(row(1).faction_template, row(2).faction_template);
        assert_eq!(
            c.base_info().len(),
            SHIPPED.combos.iter().map(|(_, cs)| cs.len()).sum::<usize>()
        );
        assert!(c.name_gen().iter().any(|n| n.race == 1 && n.sex == 0));
        let classes = crate::load_chr_classes(&mut chain).expect("ChrClasses");
        let dk = classes
            .names()
            .iter()
            .find(|n| n.id == 6)
            .expect("Death Knight");
        assert_eq!(
            (dk.file.as_str(), dk.required_expansion),
            ("DEATHKNIGHT", 2)
        );
        let warrior = classes.names().iter().find(|n| n.id == 1).expect("Warrior");
        assert_eq!(warrior.required_expansion, 0);
    }

    #[test]
    fn a_patched_combo_passes_the_guard_and_a_lost_one_does_not() {
        let mut combos: HashSet<(u8, u8)> = SHIPPED
            .combos
            .iter()
            .flat_map(|&(r, cs)| cs.iter().map(move |&c| (r, c)))
            .collect();
        assert!(shipped_combos_present(&SHIPPED, &combos).is_ok());
        // A patch that adds Human Hunter: the builder lists it, so the load must go on.
        combos.insert((1, 3));
        assert!(shipped_combos_present(&SHIPPED, &combos).is_ok());
        // A misparse that drops a shipped pair still refuses.
        combos.remove(&(2, 7));
        assert!(shipped_combos_present(&SHIPPED, &combos).is_err());
    }

    /// `GetCharacterInfo`'s race is ChrRaces `Name_Lang`, not the file string
    /// it: Night Elf and Undead differ from "NightElf" and "Scourge" in the one column each, so a
    /// slip of a column reads the wrong one.
    #[test]
    fn race_names_are_the_name_lang_column() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = CharCreateCatalog::load(&mut chain).expect("load char-create catalog");
        for (race, name, file) in [
            (1, "Human", "Human"),
            (2, "Orc", "Orc"),
            (3, "Dwarf", "Dwarf"),
            (4, "Night Elf", "NightElf"),
            (5, "Undead", "Scourge"),
            (6, "Tauren", "Tauren"),
            (7, "Gnome", "Gnome"),
            (8, "Troll", "Troll"),
        ] {
            assert_eq!(cat.race_name(race), Some(name), "race {race}");
            assert_eq!(cat.race_file(race), Some(file), "race {race}");
        }
        assert_eq!(cat.race_name(0), None);
        // The gender byte picks the race row's male (0) or female (1) display and nothing else.
        for race in PLAYABLE_RACES {
            assert!(cat.body_display(race, 0).is_some() && cat.body_display(race, 1).is_some());
            assert_eq!(cat.body_display(race, 2), None, "race {race}");
        }
    }

    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn char_create_catalog_matches_the_12340_dbcs() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = CharCreateCatalog::load(&mut chain).expect("load char-create catalog");
        let creatures = crate::load_creature_catalog(&mut chain).expect("load creatures");

        for &race in &PLAYABLE_RACES {
            for sex in [0u8, 1] {
                let display = cat.body_display(race, sex).expect("has display");
                let model = creatures.model(display).unwrap_or_else(|| {
                    panic!("display {display} (race {race} sex {sex}) resolves")
                });
                assert!(
                    model
                        .model_path
                        .to_ascii_lowercase()
                        .starts_with("character\\"),
                    "race {race} sex {sex} display {display} → {:?} (expected a Character body)",
                    model.model_path
                );
            }
        }

        // The playable sets are the raw table.
        for &(race, classes) in SHIPPED.combos {
            assert_eq!(
                cat.classes_for_race(race),
                classes,
                "combos for race {race}"
            );
        }

        // The glue tokens against the shipped rows, pinning cols 26/27's male/female order; no
        // shipped row is "NONE".
        assert_eq!(cat.hair_customization(6), Some("HORNS"), "Tauren hair tok");
        assert_eq!(cat.hair_customization(1), Some("NORMAL"), "Human hair tok");
        assert_eq!(
            cat.facial_hair_customization(1, 0),
            Some("NORMAL"),
            "Human male facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(1, 1),
            Some("PIERCINGS"),
            "Human female facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(4, 1),
            Some("MARKINGS"),
            "Night Elf female facial tok"
        );
        assert_eq!(
            cat.facial_hair_customization(8, 0),
            Some("TUSKS"),
            "Troll male facial tok"
        );

        // Every creatable combo wears at least one item; the Human Mage wears a robe (inv 20).
        for &(race, classes) in SHIPPED.combos {
            for &class in classes {
                for sex in [0u8, 1] {
                    let out = cat.start_outfit(race, class, sex);
                    assert!(
                        !out.is_empty(),
                        "race {race} class {class} sex {sex} has no start outfit"
                    );
                    assert!(
                        out.iter().all(|it| it.display_id >= 1 && it.inv_type >= 1),
                        "race {race} class {class} sex {sex} outfit has a 0/empty entry: {out:?}"
                    );
                }
            }
        }
        let mage = cat.start_outfit(1, 8, 0);
        assert!(
            mage.iter()
                .any(|it| it.inv_type == 20 && it.display_id == 12647),
            "Human Mage male should wear a robe (inv 20, disp 12647): {mage:?}"
        );
    }

    /// Each CharSections row of base section 0..4 sits at its (race, sex, section, variation,
    /// color) slot with the last such row's flags, every filled slot is a row's, and the shipped
    /// grids carry the rows the class selection tells apart: Death Knight skins (`0x4`) and hair
    /// the create screen does not offer (no `0x1`).
    #[test]
    fn the_section_grid_is_the_char_sections_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = CharCreateCatalog::load(&mut chain).expect("load char-create catalog");
        let bytes = chain.read_file(CHAR_SECTIONS).expect("CharSections");
        let l = &crate::dbc_layout::LAYOUTS.char_sections;
        let rs = parse(
            &bytes,
            super::super::sections::char_sections_schema(l),
            "CS",
        )
        .unwrap();
        let mut last = HashMap::new();
        for r in rs.records() {
            let col = |i| u32_at(r, i).unwrap() as usize;
            if col(3) < SECTION_GRID_TYPES {
                let key = (col(1), col(2), col(3), col(l.variation), col(l.color));
                last.insert(key, col(l.flags) as u32);
            }
        }
        for (&(race, sex, ty, var, color), &flags) in &last {
            let g = cat.section_grid(race as u8, sex as u8).expect("grid");
            assert_eq!(
                g.sections[ty][var][color],
                Some(flags),
                "{race} {sex} {ty} {var} {color}"
            );
        }
        let mut filled = 0;
        for race in 0..=u8::MAX {
            for sex in [0u8, 1] {
                if let Some(g) = cat.section_grid(race, sex) {
                    filled += g.sections.iter().flatten().flatten().flatten().count();
                }
            }
        }
        assert_eq!(filled, last.len(), "no slot without a row");
        let flags_of = |race, sex, ty: usize| {
            let g = cat.section_grid(race, sex).expect("grid");
            g.sections[ty]
                .iter()
                .flatten()
                .flatten()
                .copied()
                .collect::<Vec<u32>>()
        };
        assert!(
            flags_of(1, 0, 0).iter().any(|f| f & 0x4 != 0),
            "a Death Knight skin"
        );
        assert!(
            flags_of(1, 0, 3).iter().any(|f| f & 0x1 == 0),
            "hair off the create screen"
        );
    }
}
