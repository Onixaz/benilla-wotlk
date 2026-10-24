//! The character create verbs (table `0xac4228`, registered by `0x4dfd00`) and the state they keep,
//! `CCharacterCreation`'s: the creatable races (`0x4e1c20`), the selected race index, class and
//! sex, the five appearance dials, the dials each (race, sex) last held, the facing, and the name
//! generator's dictionary.
//!
//! The host seeds the tables ([`GlueCreateCatalog`]) from the DBCs; the character itself is the
//! host's scene drive, which reads [`super::UiScript::glue_create_look`]. Indices are 1-based in Lua
//! and 0-based inside. The dials are `[skin, face, hairStyle, hairColor, facialHair]`, the order
//! `CycleCharCustomization`'s index names them in, each a `CharSections` index
//! ([`super::glue_customize`]).

use std::collections::HashMap;

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{number_arg, string_arg};
use super::glue_customize::{Customizer, GlueCustomizeGrid};
use super::ScriptValue;

/// `g_scriptSexIndex`: Lua's sex numbers for male, female and none, as `GetCharacterInfo` answers.
const SCRIPT_SEX: [u8; 3] = [2, 3, 1];

/// `s_classRoles` (`0xa394b0`, read by `0x552940` at class id − 1): bit `0x2` tank, `0x4` healer,
/// `0x8` damage.
const CLASS_ROLES: [u8; 11] = [
    0x0B, 0x0F, 0x09, 0x09, 0x0D, 0x0B, 0x0D, 0x09, 0x09, 0x00, 0x0F,
];

/// `deg = rad · 57.29578` and back, the floats `0x4e0be0` and `0x4e0c10` multiply by.
const RAD_TO_DEG: f32 = 57.295_78;
const DEG_TO_RAD: f32 = 0.017_453_292;

/// One `ChrRaces.dbc` row as the create screen reads it.
#[derive(Clone, Debug, Default)]
pub struct GlueCreateRace {
    pub id: u8,
    /// Field 1: bit `0x1` keeps the race off the screen.
    pub flags: u32,
    pub required_expansion: u8,
    /// `Name_Lang`, `Name_female_lang`, `Name_male_lang`.
    pub name: String,
    pub name_female: String,
    pub name_male: String,
    /// `clientFileString`.
    pub file: String,
    /// The race's faction group, `FactionGroup.dbc`'s localized name and `InternalName`.
    pub faction_name: String,
    pub faction_internal: String,
}

/// One `ChrClasses.dbc` row as the create screen reads it.
#[derive(Clone, Debug, Default)]
pub struct GlueCreateClass {
    pub id: u8,
    pub required_expansion: u8,
    pub name: String,
    pub name_female: String,
    pub name_male: String,
    /// `Filename`, the class token.
    pub file: String,
}

/// The tables the create verbs read, seeded by the host.
#[derive(Clone, Debug, Default)]
pub struct GlueCreateCatalog {
    /// Every `ChrRaces.dbc` row, table order.
    pub races: Vec<GlueCreateRace>,
    /// Every `ChrClasses.dbc` row, table order: the 1-based class index Lua passes is a row here.
    pub classes: Vec<GlueCreateClass>,
    /// `CharBaseInfo.dbc`'s (race, class) pairs, row order.
    pub base_info: Vec<(u8, u8)>,
    /// The `CharSections` grid per (race, sex); a (race, sex) without one has no body to build.
    pub customize: HashMap<(u8, u8), GlueCustomizeGrid>,
    /// A race's hair token and its [male, female] facial-hair tokens.
    pub hair: HashMap<u8, (String, [String; 2])>,
    /// `NameGen.dbc`'s (name, race, sex), table order.
    pub names: Vec<(String, u8, u8)>,
}

impl GlueCreateCatalog {
    fn race(&self, id: u8) -> Option<&GlueCreateRace> {
        self.races.iter().find(|r| r.id == id)
    }

    fn class(&self, id: u8) -> Option<&GlueCreateClass> {
        self.classes.iter().find(|c| c.id == id)
    }

    fn is_race_class_valid(&self, race: u8, class: u8) -> bool {
        self.base_info.contains(&(race, class))
    }
}

/// `GetDisplayRaceNameFromRecord`/`GetDisplayClassNameFromRecord`: the gendered name, the neutral
/// one where it is empty.
fn display_name<'a>(name: &'a str, female: &'a str, male: &'a str, sex: u8) -> &'a str {
    let gendered = match sex {
        0 => male,
        1 => female,
        _ => "",
    };
    if gendered.is_empty() {
        name
    } else {
        gendered
    }
}

/// The character the screen builds: `CCharacterCreation::m_character`'s preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlueCreateCharacter {
    pub race: u8,
    pub sex: u8,
    pub dials: [u8; 5],
}

/// What the scene drive stands on the create screen: the character, the selected class, whose
/// starting outfit dresses it, and the facing in radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlueCreateLook {
    pub character: GlueCreateCharacter,
    pub class: u8,
    pub facing: f32,
}

/// A create request: `CHARACTER_CREATE_INFO`, handed to the host's `CGlueMgr::CreateCharacter`
/// (`0x4d8c60`), or, for an existing character's paid change, with that character's guid and
/// paid-service flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlueCharCreate {
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    /// The existing character's guid and paid-service flags, for a paid change.
    pub existing: Option<(u64, u32)>,
}

/// The create verbs' slots in the glue model.
pub(crate) struct GlueCreate {
    /// `m_charCustomizeFrame`: the model frame `SetCharCustomizeFrame` found.
    pub(super) customize_frame: Option<String>,
    pub(super) catalog: GlueCreateCatalog,
    /// `m_races`: the creatable race ids, Alliance then Horde, each in table order.
    races: Vec<u8>,
    /// `m_raceIndex` into `races`.
    race_index: Option<usize>,
    /// `m_classes`: the class ids `CharBaseInfo` lists for the race, row order.
    classes: Vec<u8>,
    /// `m_selectedClassID`, 1 at load.
    class_id: u8,
    /// `m_existingCharacterIndex`, a roster index for a paid change.
    existing: Option<usize>,
    character: Option<GlueCreateCharacter>,
    /// `m_charPreferences`: the dials each (race, sex) held when the screen left it.
    preferences: HashMap<(u8, u8), [u8; 5]>,
    /// `m_charFacing`, radians.
    facing: f32,
    /// `m_prevSkinIndex`: the skin last drawn, built with or stepped to.
    prev_skin: u8,
    /// The name generator's trigram dictionary for the race and sex (`0x4e6560`), sorted.
    dictionary: Vec<(String, u32)>,
    rng: u64,
}

impl Default for GlueCreate {
    fn default() -> Self {
        GlueCreate {
            customize_frame: None,
            catalog: GlueCreateCatalog::default(),
            races: Vec::new(),
            race_index: None,
            classes: Vec::new(),
            class_id: 1,
            existing: None,
            character: None,
            preferences: HashMap::new(),
            facing: 0.0,
            prev_skin: 0,
            dictionary: Vec::new(),
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

/// `CRandom::dice_`'s role: a uniform draw in `0..n` (`n · r >> 32`).
fn roll(rng: &mut u64, n: usize) -> usize {
    // xorshift64*
    let mut x = *rng;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *rng = x;
    let r = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32;
    ((n as u64 * u64::from(r)) >> 32) as usize
}

impl GlueCreate {
    fn roll(&mut self, n: usize) -> usize {
        roll(&mut self.rng, n)
    }

    /// `CCharacterCreation::Initialize` (`0x4e1c20`): the races not flagged `0x1`, Alliance's
    /// first and then Horde's, each in table order.
    fn initialize(&mut self) {
        self.facing = 0.0;
        self.existing = None;
        self.preferences.clear();
        self.races.clear();
        for faction in ["Alliance", "Horde"] {
            for r in &self.catalog.races {
                if r.flags & 0x1 == 0 && r.faction_internal.eq_ignore_ascii_case(faction) {
                    self.races.push(r.id);
                }
            }
        }
    }

    fn expansion_ok(required: u8, expansion: u8) -> bool {
        expansion >= required
    }

    /// `0x4e1ed0`: the classes `CharBaseInfo` lists for the race, row order.
    fn calc_classes(&mut self, race: u8) {
        self.classes = self
            .catalog
            .base_info
            .iter()
            .filter(|(r, _)| *r == race)
            .map(|&(_, c)| c)
            .collect();
    }

    /// `0x4e0f50`: a random class of the race's the account may play.
    fn random_class(&mut self, expansion: u8) -> u8 {
        let open: Vec<u8> = self
            .classes
            .iter()
            .copied()
            .filter(|&c| {
                self.catalog
                    .class(c)
                    .is_some_and(|row| Self::expansion_ok(row.required_expansion, expansion))
            })
            .collect();
        match open.len() {
            0 => self.classes.first().copied().unwrap_or(1),
            n => {
                let i = self.roll(n);
                open[i]
            }
        }
    }

    /// `ValidateComponentData` (`0x4e9d50`) for the class.
    fn validate(&self, race: u8, sex: u8, class: u8, dials: [u8; 5]) -> [u8; 5] {
        let Some(grid) = self.catalog.customize.get(&(race, sex)) else {
            return dials;
        };
        let mut c = Customizer::new(grid, class, dials);
        c.validate();
        c.dials()
    }

    /// `0x4e13a0`: a new character of the race, sex and class, its dials as given or drawn skin,
    /// hair colour, hair style, face, facial hair; a race and sex with no model leave the old one.
    fn init_character(&mut self, race: u8, sex: u8, class: u8, dials: [u8; 5], randomize: bool) {
        let Some(grid) = self.catalog.customize.get(&(race, sex)) else {
            return;
        };
        let mut c = Customizer::new(grid, class, dials);
        if randomize {
            let rng = &mut self.rng;
            let mut roll = |n: i32| roll(rng, n as usize) as i32;
            c.random_skin(&mut roll);
            c.random_hair_color(&mut roll);
            c.random_hair_style(&mut roll);
            c.random_face(&mut roll);
            c.random_facial(&mut roll);
        }
        let dials = c.dials();
        self.prev_skin = dials[0];
        self.character = Some(GlueCreateCharacter { race, sex, dials });
    }

    fn selected_race(&self) -> Option<u8> {
        self.races.get(self.race_index?).copied()
    }

    fn save_preferences(&mut self) {
        if let (Some(race), Some(c)) = (self.selected_race(), self.character) {
            self.preferences.insert((race, c.sex), c.dials);
        }
    }

    /// `0x4e1740`.
    fn set_class(&mut self, class: u8) {
        if !self.classes.contains(&class) {
            return;
        }
        self.class_id = class;
        if let Some(c) = self.character {
            let dials = self.validate(c.race, c.sex, class, c.dials);
            self.init_character(c.race, c.sex, class, dials, false);
        }
    }

    /// `0x4e1540`: the dials the race and sex last held, or random ones; an existing character's
    /// own where it is of that sex and may be customized.
    fn set_sex(&mut self, sex: u8, roster: &[super::GlueCharacter]) {
        let Some(c) = self.character else { return };
        if sex >= 3 || sex == c.sex {
            return;
        }
        self.save_preferences();
        if let Some(e) = self.existing.and_then(|i| roster.get(i)) {
            if e.gender == sex && e.customize_flags & 0x1 != 0 {
                self.init_character(e.race_id, e.gender, e.class_id, e.dials, false);
                self.load_names();
                return;
            }
        }
        let race = self.selected_race().unwrap_or(c.race);
        let class = self.class_id;
        match self.preferences.get(&(race, sex)).copied() {
            Some(saved) => {
                let dials = self.validate(c.race, sex, class, saved);
                self.init_character(c.race, sex, class, dials, false);
            }
            None => self.init_character(c.race, sex, class, [0; 5], true),
        }
        self.load_names();
    }

    /// `0x4e20b0`.
    fn set_race(&mut self, index: usize, expansion: u8, roster: &[super::GlueCharacter]) {
        if index >= self.races.len() || Some(index) == self.race_index {
            return;
        }
        let Some(c) = self.character else { return };
        self.save_preferences();
        self.race_index = Some(index);
        let race = self.races[index];
        let existing = self.existing.and_then(|i| roster.get(i)).cloned();
        if let Some(e) = existing.filter(|e| e.race_id == race && e.customize_flags & 0x110000 != 0)
        {
            self.init_character(e.race_id, e.gender, e.class_id, e.dials, false);
            self.set_sex(e.gender, roster);
        } else if let Some(saved) = self.preferences.get(&(race, c.sex)).copied() {
            self.calc_classes(race);
            if !self.catalog.is_race_class_valid(race, self.class_id) {
                self.class_id = self.random_class(expansion);
            }
            let dials = self.validate(race, c.sex, self.class_id, saved);
            self.init_character(race, c.sex, self.class_id, dials, false);
        } else {
            self.calc_classes(race);
            if !self.catalog.is_race_class_valid(race, self.class_id) {
                let class = self.random_class(expansion);
                self.set_class(class);
            }
            self.init_character(race, c.sex, self.class_id, [0; 5], true);
        }
        self.load_names();
    }

    /// `0x4e1fd0`: a random race the account may play and a random sex, its classes, a random
    /// character drawn for no class, and a random class, which validates it.
    fn reset(&mut self, expansion: u8) {
        self.existing = None;
        if self.races.is_empty() {
            return;
        }
        let sex = self.roll(2) as u8;
        let mut race = self.races[0];
        for _ in 0..64 {
            let i = self.roll(self.races.len());
            race = self.races[i];
            if self
                .catalog
                .race(race)
                .is_some_and(|r| Self::expansion_ok(r.required_expansion, expansion))
            {
                break;
            }
        }
        self.calc_classes(race);
        self.init_character(race, sex, 0, [0; 5], true);
        let class = self.random_class(expansion);
        self.set_class(class);
        self.race_index = self.races.iter().position(|&r| r == race);
        self.load_names();
    }

    /// `0x4e2330`: the screen takes an existing character, for a paid change.
    fn set_existing(&mut self, index: usize, roster: &[super::GlueCharacter]) {
        let Some(e) = roster.get(index).cloned() else {
            return;
        };
        self.existing = Some(index);
        self.calc_classes(e.race_id);
        self.init_character(e.race_id, e.gender, e.class_id, e.dials, false);
        self.class_id = e.class_id;
        if let Some(i) = self.races.iter().position(|&r| r == e.race_id) {
            self.race_index = Some(i);
        }
        self.load_names();
    }

    /// `0x4e01f0`: one dial to the next or previous slot the class admits; a face steps from the
    /// skin last picked (`m_prevSkinIndex`), which only a skin step moves.
    fn cycle(&mut self, dial: usize, delta: i32) {
        let Some(ch) = self.character else { return };
        if delta == 0 || dial >= 5 {
            return;
        }
        let Some(grid) = self.catalog.customize.get(&(ch.race, ch.sex)) else {
            return;
        };
        let step = delta.signum();
        let mut c = Customizer::new(grid, self.class_id, ch.dials);
        match dial {
            0 => {
                c.step_skin(step);
                self.prev_skin = c.dials()[0];
            }
            1 => {
                c.step_face(step, self.prev_skin.into());
            }
            2 => {
                c.step_hair_style(step);
            }
            3 => {
                c.step_hair_color(step);
            }
            _ => {
                c.step_facial(step);
            }
        }
        self.character = Some(GlueCreateCharacter {
            dials: c.dials(),
            ..ch
        });
    }

    /// `0x4e17f0`: skin, face, hair colour, hair style, facial hair drawn in turn.
    fn randomize(&mut self) {
        let Some(ch) = self.character else { return };
        let Some(grid) = self.catalog.customize.get(&(ch.race, ch.sex)) else {
            return;
        };
        let mut c = Customizer::new(grid, self.class_id, ch.dials);
        let rng = &mut self.rng;
        let mut roll = |n: i32| roll(rng, n as usize) as i32;
        c.random_skin(&mut roll);
        c.random_face(&mut roll);
        c.random_hair_color(&mut roll);
        c.random_hair_style(&mut roll);
        c.random_facial(&mut roll);
        let dials = c.dials();
        self.prev_skin = dials[0];
        self.character = Some(GlueCreateCharacter { dials, ..ch });
    }

    /// `CNameGen::LoadNames` (`0x4e6790`) and `CreateDictionary` (`0x4e6560`): every three-letter
    /// run of the race and sex's names, each led by `_`, lower-cased and counted, sorted.
    fn load_names(&mut self) {
        let Some(c) = self.character else { return };
        let mut counts: Vec<(String, u32)> = Vec::new();
        for (name, race, sex) in &self.catalog.names {
            if *race != c.race || *sex != c.sex {
                continue;
            }
            let word: Vec<char> = format!("_{name}").to_lowercase().chars().collect();
            for j in 0..word.len().saturating_sub(1) {
                let key: String = word[j..(j + 3).min(word.len())].iter().collect();
                match counts.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((key, 1)),
                }
            }
        }
        counts.sort_by(|a, b| a.0.cmp(&b.0));
        self.dictionary = counts;
    }

    /// `CNameGen::GenerateName` (`0x4e63a0`): a `_`-led trigram to start, then each next trigram
    /// chosen among those whose first two letters end the name so far, weighted by count, until a
    /// trigram of two letters ends it or the name fills `size − 1`.
    fn generate_name(&mut self, size: usize) -> String {
        let mut buf = String::new();
        if self.dictionary.is_empty() {
            return buf;
        }
        loop {
            let chars: Vec<char> = buf.chars().collect();
            let tail: String = chars[chars.len().saturating_sub(2)..].iter().collect();
            let matching: Vec<usize> = (0..self.dictionary.len())
                .filter(|&i| {
                    let k = &self.dictionary[i].0;
                    if buf.is_empty() {
                        k.starts_with('_')
                    } else {
                        k.starts_with(&tail)
                    }
                })
                .collect();
            let total: u32 = matching.iter().map(|&i| self.dictionary[i].1).sum();
            if total == 0 {
                break;
            }
            let mut roll = self.roll(total as usize) as u32;
            let mut choice = matching[0];
            for &i in &matching {
                if roll < self.dictionary[i].1 {
                    choice = i;
                    break;
                }
                roll -= self.dictionary[i].1;
            }
            let key = self.dictionary[choice].0.clone();
            if buf.is_empty() {
                buf = key;
                continue;
            }
            let Some(next) = key.chars().nth(2) else {
                break;
            };
            buf.push(next);
            if buf.chars().count() >= size - 1 {
                break;
            }
        }
        let mut chars = buf.chars().skip(1);
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }
}

/// The client's name check before a create (`ClientServices::CharacterValidateName`,
/// `0x6b0f90`), as the token its failure shows: none for a name that passes.
///
/// Deviation: the profanity and reserved-name lists are left to the server, which refuses those
/// names with its own result code.
pub fn validate_character_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("CHAR_NAME_NO_NAME");
    }
    if !name.chars().all(|c| c.is_ascii_alphabetic()) {
        return Some("CHAR_NAME_INVALID_CHARACTER");
    }
    let len = name.chars().count();
    if len < 2 {
        return Some("CHAR_NAME_TOO_SHORT");
    }
    if len > 12 {
        return Some("CHAR_NAME_TOO_LONG");
    }
    let lower: Vec<char> = name.to_ascii_lowercase().chars().collect();
    if lower.windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) {
        return Some("CHAR_NAME_THREE_CONSECUTIVE");
    }
    None
}

fn model(lua: &Lua) -> crate::script::ModelGuard<'_> {
    crate::script::model_mut_of(lua)
}

fn arg(args: &MultiValue, i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Nil)
}

fn push_name_file_available(
    out: &mut Vec<Value>,
    lua: &Lua,
    row: Option<(&str, &str, bool)>,
) -> mlua::Result<()> {
    match row {
        Some((name, file, available)) => {
            out.push(Value::String(lua.create_string(name)?));
            out.push(Value::String(lua.create_string(file)?));
            out.push(Value::Number(if available { 1.0 } else { 0.0 }));
        }
        None => out.extend([Value::Nil, Value::Nil, Value::Nil]),
    }
    Ok(())
}

impl super::UiScript {
    /// Seed the create screen's tables and rebuild its race list (`0x4e1c20`).
    pub fn set_glue_create_catalog(&mut self, catalog: GlueCreateCatalog) {
        let mut m = self.model_mut();
        m.glue.create.catalog = catalog;
        m.glue.create.initialize();
    }

    /// What the create screen's scene stands, while a character is built.
    pub fn glue_create_look(&self) -> Option<GlueCreateLook> {
        let m = self.model_ref();
        let c = &m.glue.create;
        Some(GlueCreateLook {
            character: c.character?,
            class: c.class_id,
            facing: c.facing,
        })
    }
}

/// The create screen's model frame, while the name `SetCharCustomizeFrame` took still names a
/// model pane.
pub(super) fn customize_frame_handle(
    m: &crate::script::Model,
) -> Option<crate::widget::FrameHandle> {
    let h = m.arena.lookup(m.glue.create.customize_frame.as_deref()?)?;
    m.arena
        .frame(h)
        .is_some_and(|f| f.kind.is_model())
        .then_some(h)
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `SetCharCustomizeFrame(name)` (`0x4e05b0`): a string or the usage error; the global of that
    // name castable to the model type becomes the frame, else the call changes nothing.
    g.set(
        "SetCharCustomizeFrame",
        lua.create_function(|lua, args: MultiValue| {
            let name = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharCustomizeFrame(\"frameName\")",
            )?;
            let mut m = model(lua);
            let is_model = m
                .arena
                .lookup(&name)
                .and_then(|h| m.arena.frame(h))
                .is_some_and(|f| f.kind.is_model());
            if is_model {
                m.glue.create.customize_frame = Some(name);
            }
            Ok(())
        })?,
    )?;

    // `SetCharCustomizeBackground(file)` (`0x4e0620` → `0x4e0040`): needs the frame and a
    // non-empty name, skips the file the scene already holds, else the frame's own `SetModel`.
    g.set(
        "SetCharCustomizeBackground",
        lua.create_function(|lua, args: MultiValue| {
            let file = string_arg(
                lua,
                arg(&args, 0),
                "Usage: SetCharCustomizeBackground(\"filename\")",
            )?;
            let mut m = model(lua);
            let Some(h) = customize_frame_handle(&m).filter(|_| !file.is_empty()) else {
                return Ok(());
            };
            let held = match m.arena.frame(h).map(|f| &f.kind_state) {
                Some(crate::widget::KindState::Model(pane)) => pane
                    .path
                    .as_deref()
                    .is_some_and(|p| p.eq_ignore_ascii_case(&file)),
                _ => false,
            };
            if !held {
                m.set_pane_model(h, file);
            }
            Ok(())
        })?,
    )?;

    // `ResetCharCustomize()` (`0x4e2460` → `0x4e1fd0`): nothing without the frame.
    g.set(
        "ResetCharCustomize",
        lua.create_function(|lua, _: MultiValue| {
            let mut m = model(lua);
            if customize_frame_handle(&m).is_none() {
                return Ok(());
            }
            let expansion = m.account_expansion;
            m.glue.create.reset(expansion);
            Ok(())
        })?,
    )?;

    // `GetNameForRace()` (`0x4e0670`): the character's race name for its sex and file string, or
    // two nils.
    g.set(
        "GetNameForRace",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let row = c
                .character
                .and_then(|ch| c.catalog.race(ch.race).map(|r| (r, ch.sex)));
            Ok(match row {
                Some((r, sex)) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(display_name(
                        &r.name,
                        &r.name_female,
                        &r.name_male,
                        sex,
                    ))?),
                    Value::String(lua.create_string(&r.file)?),
                ]),
                None => MultiValue::from_vec(vec![Value::Nil, Value::Nil]),
            })
        })?,
    )?;

    // `GetFactionForRace(index)` (`0x4e06f0`): the race's faction group name and internal name,
    // or two nils.
    g.set(
        "GetFactionForRace",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: GetFactionForRace(index)")?;
            let m = model(lua);
            let c = &m.glue.create;
            let race = usize::try_from(index - 1)
                .ok()
                .and_then(|i| c.races.get(i))
                .and_then(|&id| c.catalog.race(id))
                .filter(|r| !r.faction_internal.is_empty());
            Ok(match race {
                Some(r) => MultiValue::from_vec(vec![
                    Value::String(lua.create_string(&r.faction_name)?),
                    Value::String(lua.create_string(&r.faction_internal)?),
                ]),
                None => MultiValue::from_vec(vec![Value::Nil, Value::Nil]),
            })
        })?,
    )?;

    // `GetAvailableRaces()` (`0x4e1880`): per creatable race its name for the character's sex,
    // its file string and 1 when the account's expansion reaches it, else 0.
    g.set(
        "GetAvailableRaces",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let sex = c.character.map_or(0, |ch| ch.sex);
            let mut out = Vec::new();
            for &id in &c.races {
                let row = c.catalog.race(id).map(|r| {
                    (
                        display_name(&r.name, &r.name_female, &r.name_male, sex),
                        r.file.as_str(),
                        GlueCreate::expansion_ok(r.required_expansion, m.account_expansion),
                    )
                });
                push_name_file_available(&mut out, lua, row)?;
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `GetAvailableClasses()` (`0x4e0830`): the same three per `ChrClasses` row, table order.
    g.set(
        "GetAvailableClasses",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let sex = c.character.map_or(0, |ch| ch.sex);
            let mut out = Vec::new();
            for r in &c.catalog.classes {
                push_name_file_available(
                    &mut out,
                    lua,
                    Some((
                        display_name(&r.name, &r.name_female, &r.name_male, sex),
                        &r.file,
                        GlueCreate::expansion_ok(r.required_expansion, m.account_expansion),
                    )),
                )?;
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `GetClassesForRace()` (`0x4e19a0`): the same three per class the race may be.
    g.set(
        "GetClassesForRace",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let sex = c.character.map_or(0, |ch| ch.sex);
            let mut out = Vec::new();
            for &id in &c.classes {
                let row = c.catalog.class(id).map(|r| {
                    (
                        display_name(&r.name, &r.name_female, &r.name_male, sex),
                        r.file.as_str(),
                        GlueCreate::expansion_ok(r.required_expansion, m.account_expansion),
                    )
                });
                push_name_file_available(&mut out, lua, row)?;
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // `GetHairCustomization()` (`0x4e0900`) and `GetFacialHairCustomization()` (`0x4e0970`): the
    // selected race's tokens, `"NORMAL"` without a race.
    g.set(
        "GetHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let token = c
                .selected_race()
                .and_then(|r| c.catalog.hair.get(&r))
                .map_or("NORMAL", |(hair, _)| hair.as_str());
            lua.create_string(token)
        })?,
    )?;
    g.set(
        "GetFacialHairCustomization",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let sex = usize::from(c.character.map_or(0, |ch| ch.sex).min(1));
            let token = c
                .selected_race()
                .and_then(|r| c.catalog.hair.get(&r))
                .map_or("NORMAL", |(_, facial)| facial[sex].as_str());
            lua.create_string(token)
        })?,
    )?;

    // `GetSelectedRace()` (`0x4e09f0`) and `GetSelectedSex()` (`0x4e0a30`).
    g.set(
        "GetSelectedRace",
        lua.create_function(|lua, _: MultiValue| {
            Ok(model(lua)
                .glue
                .create
                .race_index
                .map_or(0.0, |i| (i + 1) as f64))
        })?,
    )?;
    g.set(
        "GetSelectedSex",
        lua.create_function(|lua, _: MultiValue| {
            let sex = model(lua).glue.create.character.map_or(0, |c| c.sex);
            Ok(f64::from(SCRIPT_SEX[usize::from(sex.min(2))]))
        })?,
    )?;

    // `GetSelectedClass()` (`0x4e0a60`): name, file, the row's 1-based index, and the class's tank,
    // healer and damage roles; nothing for a class with no row.
    g.set(
        "GetSelectedClass",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let Some((index, r)) = c
                .catalog
                .classes
                .iter()
                .enumerate()
                .find(|(_, r)| r.id == c.class_id)
            else {
                return Ok(MultiValue::new());
            };
            let sex = c.character.map_or(0, |ch| ch.sex);
            let roles = CLASS_ROLES
                .get(usize::from(r.id).wrapping_sub(1))
                .copied()
                .unwrap_or(0);
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(display_name(
                    &r.name,
                    &r.name_female,
                    &r.name_male,
                    sex,
                ))?),
                Value::String(lua.create_string(&r.file)?),
                Value::Number((index + 1) as f64),
                Value::Boolean(roles & 0x2 != 0),
                Value::Boolean(roles & 0x4 != 0),
                Value::Boolean(roles & 0x8 != 0),
            ]))
        })?,
    )?;

    // `SetSelectedRace(index)` (`0x4e2470` → `0x4e20b0`).
    g.set(
        "SetSelectedRace",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: SetSelectedRace(index)")?;
            let mut m = model(lua);
            let expansion = m.account_expansion;
            let roster = m.glue.roster.characters();
            if let Ok(i) = usize::try_from(index - 1) {
                m.glue.create.set_race(i, expansion, &roster);
            }
            Ok(())
        })?,
    )?;

    // `SetSelectedSex(n)` (`0x4e1a60`): the sex whose script number is `n`.
    g.set(
        "SetSelectedSex",
        lua.create_function(|lua, args: MultiValue| {
            let n = number_arg(lua, arg(&args, 0), "Usage: SetSelectedSex(index)")?;
            let mut m = model(lua);
            let roster = m.glue.roster.characters();
            if let Some(sex) = SCRIPT_SEX.iter().position(|&s| i32::from(s) == n) {
                m.glue.create.set_sex(sex as u8, &roster);
            }
            Ok(())
        })?,
    )?;

    // `SetSelectedClass(index)` (`0x4e1ac0` → `0x4e1740`): the `ChrClasses` row at the index.
    g.set(
        "SetSelectedClass",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(lua, arg(&args, 0), "Usage: SetSelectedClass(index)")?;
            let mut m = model(lua);
            let c = &mut m.glue.create;
            if let Some(id) = usize::try_from(index - 1)
                .ok()
                .and_then(|i| c.catalog.classes.get(i))
                .map(|r| r.id)
            {
                c.set_class(id);
            }
            Ok(())
        })?,
    )?;

    // `UpdateCustomizationBackground()` (`0x4e24e0`): `SetSelectedRace` of the selected index,
    // which returns at once.
    g.set(
        "UpdateCustomizationBackground",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;
    // `UpdateCustomizationScene()` (`0x4dfc90`): the character's render prep, the scene drive's.
    g.set(
        "UpdateCustomizationScene",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;

    // `CycleCharCustomization(index, delta)` (`0x4e0b50` → `0x4e01f0`).
    g.set(
        "CycleCharCustomization",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: CycleCharCustomization(index, delta)";
            let index = number_arg(lua, arg(&args, 0), USAGE)?;
            let delta = number_arg(lua, arg(&args, 1), USAGE)?;
            if let Ok(dial) = usize::try_from(index - 1) {
                model(lua).glue.create.cycle(dial, delta);
            }
            Ok(())
        })?,
    )?;

    // `RandomizeCharCustomization()` (`0x4e1b60` → `0x4e17f0`).
    g.set(
        "RandomizeCharCustomization",
        lua.create_function(|lua, _: MultiValue| {
            model(lua).glue.create.randomize();
            Ok(())
        })?,
    )?;

    // `GetCharacterCreateFacing()` (`0x4e0be0`) and `SetCharacterCreateFacing(degrees)`
    // (`0x4e0c10`), which returns its argument.
    g.set(
        "GetCharacterCreateFacing",
        lua.create_function(|lua, _: MultiValue| {
            Ok(f64::from(model(lua).glue.create.facing * RAD_TO_DEG))
        })?,
    )?;
    g.set(
        "SetCharacterCreateFacing",
        lua.create_function(|lua, args: MultiValue| {
            let v = arg(&args, 0);
            let Some(degrees) = lua.coerce_number(v.clone())? else {
                return Err(mlua::Error::runtime(
                    "Usage: SetCharacterCreateFacing(degrees)",
                ));
            };
            model(lua).glue.create.facing = degrees as f32 * DEG_TO_RAD;
            Ok(v)
        })?,
    )?;

    // `GetRandomName()` (`0x4dfca0`): generated names until one of 4 to 10 letters passes the
    // client's name check, or an empty one.
    g.set(
        "GetRandomName",
        lua.create_function(|lua, _: MultiValue| {
            let mut m = model(lua);
            let mut name = String::new();
            for _ in 0..256 {
                name = m.glue.create.generate_name(14);
                let len = name.chars().count();
                if len == 0 || (validate_character_name(&name).is_none() && (4..=10).contains(&len))
                {
                    break;
                }
            }
            lua.create_string(&name)
        })?,
    )?;

    // `CreateCharacter(name)` (`0x4e0c60` → `0x4e0380`): the name checked (not for a paid change
    // that keeps the character's own), a failure shown in the status dialog, else the request.
    g.set(
        "CreateCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let name = match arg(&args, 0) {
                v @ (Value::String(_) | Value::Integer(_) | Value::Number(_)) => {
                    lua.coerce_string(v)?.map(|s| s.to_string_lossy())
                }
                _ => None,
            };
            let (request, failure) = {
                let m = model(lua);
                let c = &m.glue.create;
                let roster = m.glue.roster.characters();
                let existing = match c.existing {
                    Some(i) => match roster.get(i) {
                        Some(e) => Some(e.clone()),
                        None => return Ok(()),
                    },
                    None => None,
                };
                let mut validate = true;
                let name = match (&existing, name) {
                    (Some(e), Some(n)) => {
                        validate = !e.name.eq_ignore_ascii_case(&n);
                        Some(n)
                    }
                    (Some(e), None) => Some(e.name.clone()),
                    (None, n) => n,
                };
                let failure = if validate {
                    validate_character_name(name.as_deref().unwrap_or(""))
                } else {
                    None
                };
                let request = c.character.map(|ch| GlueCharCreate {
                    name: name.unwrap_or_default(),
                    race: ch.race,
                    class: c.class_id,
                    sex: ch.sex,
                    skin: ch.dials[0],
                    face: ch.dials[1],
                    hair_style: ch.dials[2],
                    hair_color: ch.dials[3],
                    facial_hair: ch.dials[4],
                    existing: existing.map(|e| (e.guid, e.customize_flags)),
                });
                (request, failure)
            };
            if let Some(token) = failure {
                let text = super::glue_core::global_text_of(lua, token);
                super::tick::fire_event_into(
                    lua,
                    "OPEN_STATUS_DIALOG",
                    vec![ScriptValue::Str("OKAY".into()), ScriptValue::Str(text)],
                );
                return Ok(());
            }
            if let Some(request) = request {
                super::glue_core::queue(
                    lua,
                    super::glue_core::GlueRequest::CreateCharacter(request),
                );
            }
            Ok(())
        })?,
    )?;

    // `CustomizeExistingCharacter(index)` (`0x4e2500` → `0x4e2330`).
    g.set(
        "CustomizeExistingCharacter",
        lua.create_function(|lua, args: MultiValue| {
            let index = number_arg(
                lua,
                arg(&args, 0),
                "Usage: CustomizeExistingCharacter(index)",
            )?;
            let mut m = model(lua);
            let roster = m.glue.roster.characters();
            if let Ok(i) = usize::try_from(index - 1) {
                m.glue.create.set_existing(i, &roster);
            }
            Ok(())
        })?,
    )?;

    // The paid-change readers: `PaidChange_GetPreviousRaceIndex` (`0x608560`) is 0; the current
    // race (`0x4e0ca0`) and class (`0x4e0cd0`) indices are the existing character's, plus one (0
    // without one); `PaidChange_GetName` (`0x4e1b70`) is its name or nil.
    g.set(
        "PaidChange_GetPreviousRaceIndex",
        lua.create_function(|_, _: MultiValue| Ok(0.0))?,
    )?;
    g.set(
        "PaidChange_GetCurrentRaceIndex",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let roster = m.glue.roster.characters();
            let i = c
                .existing
                .and_then(|i| roster.get(i))
                .and_then(|e| c.races.iter().position(|&r| r == e.race_id));
            Ok(i.map_or(0.0, |i| (i + 1) as f64))
        })?,
    )?;
    g.set(
        "PaidChange_GetCurrentClassIndex",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let roster = m.glue.roster.characters();
            let i = c
                .existing
                .and_then(|i| roster.get(i))
                .and_then(|e| c.catalog.classes.iter().position(|r| r.id == e.class_id));
            Ok(i.map_or(0.0, |i| (i + 1) as f64))
        })?,
    )?;
    g.set(
        "PaidChange_GetName",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let roster = m.glue.roster.characters();
            Ok(match m.glue.create.existing.and_then(|i| roster.get(i)) {
                Some(e) => Value::String(lua.create_string(&e.name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // `IsRaceClassValid(raceIndex, classIndex)` (`0x4e0d00`): 1 when `CharBaseInfo` pairs the
    // race at the index with the `ChrClasses` row at the index, else nil; nothing for an index off
    // the class table.
    g.set(
        "IsRaceClassValid",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: IsRaceClassValid(raceIndex, classIndex)";
            let race_index = number_arg(lua, arg(&args, 0), USAGE)?;
            let class_index = number_arg(lua, arg(&args, 1), USAGE)?;
            let m = model(lua);
            let c = &m.glue.create;
            let Some(class) = usize::try_from(class_index - 1)
                .ok()
                .and_then(|i| c.catalog.classes.get(i))
                .map(|r| r.id)
            else {
                return Ok(MultiValue::new());
            };
            let race = usize::try_from(race_index - 1)
                .ok()
                .and_then(|i| c.races.get(i))
                .copied()
                .unwrap_or(0);
            Ok(MultiValue::from_vec(vec![
                if c.catalog.is_race_class_valid(race, class) {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                },
            ]))
        })?,
    )?;

    // `IsRaceClassRestricted(raceID, classID)` (`0x4dfa70`): the server-sent restriction mask's
    // bit for the class, 0 here, where no restriction list arrives; an unknown race raises.
    g.set(
        "IsRaceClassRestricted",
        lua.create_function(|lua, args: MultiValue| {
            const USAGE: &str = "Usage: Script_IsRaceClassRestricted(raceID, classID)";
            let race = number_arg(lua, arg(&args, 0), USAGE)?;
            number_arg(lua, arg(&args, 1), USAGE)?;
            if !matches!(race, 1..=8 | 10 | 11) {
                return Err(mlua::Error::runtime(format!(
                    "Script_IsRaceClassRestricted: unsupported race ID({race})"
                )));
            }
            Ok(0.0)
        })?,
    )?;

    // `GetCreateBackgroundModel()` (`0x4e0dd0`): the Death Knight's class file, else the race's
    // file string with Gnome read as Dwarf and Troll as Orc, else empty.
    g.set(
        "GetCreateBackgroundModel",
        lua.create_function(|lua, _: MultiValue| {
            let m = model(lua);
            let c = &m.glue.create;
            let name = if c.class_id == 6 {
                c.catalog.class(6).map(|r| r.file.clone())
            } else {
                c.character.and_then(|ch| {
                    let race = match ch.race {
                        7 => 3,
                        8 => 2,
                        r => r,
                    };
                    c.catalog.race(race).map(|r| r.file.clone())
                })
            };
            lua.create_string(name.unwrap_or_default())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "glue_create_tests.rs"]
mod tests;
