//! Minimap tracking, `CGMinimapFrame`'s two globals: the active tracking spell (`0xbeba68`) and
//! the town type (`0xbeba64`), one of the fifteen rows at `0xa11c50`. The verbs are the ones the
//! stock `MiniMapTracking` button and its menu call (`Minimap.lua:408-468`).

use mlua::{Lua, MultiValue, Value};

use super::calls::{CastAt, ScriptCall};

/// What a town type tracks, `TrackingTypeInfo +0x0` with its `+0x4` operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TownKind {
    /// Type 1: a unit sharing a bit with this `UNIT_NPC_FLAGS` mask (`0x71acf0`).
    Npc(u32),
    /// Type 2: a GameObject of this `GAMEOBJECT_TYPE` (`0x70f850`).
    GameObject(u32),
    /// Type 3: low-level quest givers draw as available ones (`0x57bf30`).
    TrivialQuests,
}

/// One `0xa11c50` row: what it tracks, its `GlobalStrings` token, its file under
/// `Interface\Minimap\Tracking` and the `1 << class` mask that limits it (0: every class).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TownType {
    pub kind: TownKind,
    pub token: &'static str,
    pub file: &'static str,
    pub class_mask: u32,
}

const fn npc(flags: u32, token: &'static str, file: &'static str, class_mask: u32) -> TownType {
    TownType {
        kind: TownKind::Npc(flags),
        token,
        file,
        class_mask,
    }
}

/// The town types in menu order (`0xa11c50`, stride `0x14`).
pub const TOWN_TYPES: [TownType; 15] = [
    npc(0x1000, "MINIMAP_TRACKING_REPAIR", "Repair", 0),
    npc(0x200, "MINIMAP_TRACKING_VENDOR_FOOD", "Food", 0),
    npc(0x400, "MINIMAP_TRACKING_VENDOR_POISON", "Poisons", 0x10),
    npc(0x100, "MINIMAP_TRACKING_VENDOR_AMMO", "Ammunition", 0x1a),
    npc(0x800, "MINIMAP_TRACKING_VENDOR_REAGENT", "Reagents", 0),
    npc(0x10000, "MINIMAP_TRACKING_INNKEEPER", "Innkeeper", 0),
    npc(0x2000, "MINIMAP_TRACKING_FLIGHTMASTER", "FlightMaster", 0),
    npc(
        0x400000,
        "MINIMAP_TRACKING_STABLEMASTER",
        "StableMaster",
        0x8,
    ),
    npc(0x100000, "MINIMAP_TRACKING_BATTLEMASTER", "BattleMaster", 0),
    npc(0x20, "MINIMAP_TRACKING_TRAINER_CLASS", "Class", 0),
    npc(0x40, "MINIMAP_TRACKING_TRAINER_PROFESSION", "Profession", 0),
    npc(0x200000, "MINIMAP_TRACKING_AUCTIONEER", "Auctioneer", 0),
    npc(0x20000, "MINIMAP_TRACKING_BANKER", "Banker", 0),
    TownType {
        kind: TownKind::GameObject(19),
        token: "MINIMAP_TRACKING_MAILBOX",
        file: "Mailbox",
        class_mask: 0,
    },
    TownType {
        kind: TownKind::TrivialQuests,
        token: "MINIMAP_TRACKING_TRIVIAL_QUESTS",
        file: "TrivialQuests",
        class_mask: 0,
    },
];

/// The folder every town texture and the `None` fallback live in (`0xa1206c`).
const TRACKING_DIR: &str = "Interface\\Minimap\\Tracking";

/// A known tracking spell, a spell with an aura of type 44, 45 or 151 (`0x7fdf60`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackingSpell {
    pub spell_id: u32,
    pub name: String,
    /// `SpellIconID`'s texture.
    pub icon: Option<String>,
    /// `ActiveIconID`'s texture, shown while the spell is the active one.
    pub active_icon: Option<String>,
}

impl TrackingSpell {
    /// The active icon when there is one, else the spell's own (`0x57f4f0`).
    fn shown_icon(&self, active: bool) -> Option<&String> {
        self.active_icon
            .as_ref()
            .filter(|_| active)
            .or(self.icon.as_ref())
    }
}

/// The VM's tracking state.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrackingModel {
    /// The known tracking spells in book display order (`s_trackingSpells`, sorted by `0x53ca70`).
    pub(crate) spells: Vec<TrackingSpell>,
    /// The active tracking spell, 0 for none.
    pub(crate) spell: u32,
    /// The active town type, an index into [`TOWN_TYPES`].
    pub(crate) town: Option<usize>,
    /// The player's class, for the class masks.
    pub(crate) class_id: u8,
}

impl TrackingModel {
    /// Whether the row is offered to this class (`0x57e980`).
    fn offered(&self, row: &TownType) -> bool {
        row.class_mask == 0 || row.class_mask & (1u32 << self.class_id.min(31)) != 0
    }

    /// The offered town types, menu order (`0x57eb00`).
    fn towns(&self) -> impl Iterator<Item = usize> + '_ {
        (0..TOWN_TYPES.len()).filter(|&i| self.offered(&TOWN_TYPES[i]))
    }

    /// `GetTrackingTexture` (`0x57f4f0`): the town file, else the active spell's icon, else
    /// `Tracking\None`.
    fn texture(&self) -> String {
        if let Some(t) = self.town {
            return format!("{TRACKING_DIR}\\{}", TOWN_TYPES[t].file);
        }
        self.spells
            .iter()
            .find(|s| s.spell_id == self.spell && self.spell != 0)
            .and_then(|s| s.shown_icon(true).cloned())
            .unwrap_or_else(|| format!("{TRACKING_DIR}\\None"))
    }
}

/// A menu row, as the 1-based `GetTrackingInfo` index resolves: the spells, then the town types.
enum Row<'a> {
    Spell(&'a TrackingSpell),
    Town(usize),
}

fn row(model: &TrackingModel, index: i64) -> Option<Row<'_>> {
    let i = usize::try_from(index.checked_sub(1)?).ok()?;
    match model.spells.get(i) {
        Some(s) => Some(Row::Spell(s)),
        None => model.towns().nth(i - model.spells.len()).map(Row::Town),
    }
}

/// `LoadTrackingInfo` (`0x57e070`): the town type into the global and `minimapTrackedInfo`, then
/// `MINIMAP_UPDATE_TRACKING`.
fn load_town(lua: &Lua, town: Option<usize>) {
    {
        let mut model = super::model_mut_of(lua);
        model.minimap_tracking.town = town;
        let token = town.map_or("", |t| TOWN_TYPES[t].token);
        super::cvars::set_from_engine(&mut model, "minimapTrackedInfo", token.to_string());
    }
    super::tick::fire_event_into(lua, "MINIMAP_UPDATE_TRACKING", Vec::new());
}

fn model_of(lua: &Lua) -> mlua::AppDataRef<'_, super::ModelCell> {
    lua.app_data_ref::<super::ModelCell>()
        .expect("model app_data")
}

/// A number argument truncated toward zero, as `lua_tonumber` feeds the `int64` cast.
fn index_arg(lua: &Lua, v: &Value) -> mlua::Result<Option<i64>> {
    Ok(lua.coerce_number(v.clone())?.map(|n| n as i64))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumTrackingTypes() (`0x57f170`): the known tracking spells plus the offered town types.
    g.set(
        "GetNumTrackingTypes",
        lua.create_function(|lua, ()| {
            let model = model_of(lua);
            let t = &model.minimap_tracking;
            Ok(t.spells.len() + t.towns().count())
        })?,
    )?;

    // GetTrackingInfo(index) (`0x57f1b0`): `name, texture, active, category`, a spell row as
    // `"spell"` and a town row as `"other"`; past the list, nothing.
    g.set(
        "GetTrackingInfo",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, &index)?.unwrap_or(0);
            let (name, texture, active, category) = {
                let model = model_of(lua);
                let t = &model.minimap_tracking;
                match row(t, index) {
                    None => return Ok(MultiValue::new()),
                    Some(Row::Spell(s)) => {
                        let active = t.spell == s.spell_id;
                        (
                            s.name.clone(),
                            s.shown_icon(active).cloned(),
                            active,
                            "spell",
                        )
                    }
                    Some(Row::Town(i)) => (
                        TOWN_TYPES[i].token.to_string(),
                        Some(format!("{TRACKING_DIR}\\{}", TOWN_TYPES[i].file)),
                        t.town == Some(i),
                        "other",
                    ),
                }
            };
            // A town row names itself through its global string.
            let name = if category == "other" {
                super::get_text::global_text(lua, &name)
            } else {
                name
            };
            let str_or_nil = |s: Option<String>| -> mlua::Result<Value> {
                s.map_or(Ok(Value::Nil), |s| lua.create_string(&s).map(Value::String))
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                str_or_nil(texture)?,
                if active {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                },
                Value::String(lua.create_string(category)?),
            ]))
        })?,
    )?;

    // SetTracking([index]) (`0x57f380`): a spell row casts the spell; a town row cancels the
    // tracking aura and loads the type; no number cancels the aura and clears the type.
    g.set(
        "SetTracking",
        lua.create_function(|lua, index: Value| {
            let index = index_arg(lua, &index)?;
            let Some(index) = index else {
                let fired = {
                    let mut model = super::model_mut_of(lua);
                    let spell = std::mem::take(&mut model.minimap_tracking.spell);
                    if spell != 0 {
                        model.cancel_aura_requests.push(spell);
                    }
                    spell != 0
                };
                if fired {
                    super::tick::fire_event_into(lua, "MINIMAP_UPDATE_TRACKING", Vec::new());
                }
                load_town(lua, None);
                return Ok(());
            };
            let town = {
                let mut model = super::model_mut_of(lua);
                let picked = match row(&model.minimap_tracking, index) {
                    None => return Ok(()),
                    Some(Row::Spell(s)) => Err(s.spell_id),
                    Some(Row::Town(i)) => Ok(i),
                };
                match picked {
                    Err(spell_id) => {
                        // `Spell_C::CastSpell` at the null guid.
                        model.script_calls.push(ScriptCall::CastSpell {
                            spell_id,
                            at: CastAt::Selection,
                            via_book: false,
                        });
                        return Ok(());
                    }
                    Ok(i) => {
                        let spell = model.minimap_tracking.spell;
                        if spell != 0 {
                            model.cancel_aura_requests.push(spell);
                        }
                        i
                    }
                }
            };
            load_town(lua, Some(town));
            Ok(())
        })?,
    )?;

    // GetTrackingTexture() (`0x57f4f0`), the stock button's icon.
    g.set(
        "GetTrackingTexture",
        lua.create_function(|lua, ()| Ok(model_of(lua).minimap_tracking.texture()))?,
    )?;

    Ok(())
}

impl super::UiScript {
    /// Replace the known tracking spells and the player's class; the reference rebuilds the list
    /// with the book (`0x542030`). Read first: the feed calls this on every book pass.
    pub fn set_tracking_spells(&mut self, spells: Vec<TrackingSpell>, class_id: u8) {
        let same = {
            let t = &self.model_ref().minimap_tracking;
            t.spells == spells && (class_id == 0 || t.class_id == class_id)
        };
        if same {
            return;
        }
        let mut model = self.model_mut();
        let t = &mut model.minimap_tracking;
        t.spells = spells;
        if class_id != 0 {
            t.class_id = class_id;
        }
    }

    /// Whether `spell_id` is a known tracking spell (`IsTrackingSpell` `0x53bd10`).
    pub fn is_tracking_spell(&self, spell_id: u32) -> bool {
        self.model_ref()
            .minimap_tracking
            .spells
            .iter()
            .any(|s| s.spell_id == spell_id)
    }

    /// `SetTrackingSpell` (`0x57ea30`), from a tracking aura's add or removal on the player: a new
    /// spell clears the town type, and either way `MINIMAP_UPDATE_TRACKING` fires.
    pub fn set_tracking_spell(&mut self, spell_id: u32) {
        // Only a known tracking spell moves it (`0x727760` add, `0x722090` removal).
        if spell_id != 0 && !self.is_tracking_spell(spell_id) {
            return;
        }
        self.model_mut().minimap_tracking.spell = spell_id;
        if spell_id != 0 {
            load_town(&self.lua, None);
        } else {
            self.fire_event("MINIMAP_UPDATE_TRACKING", vec![]);
        }
    }

    /// `LoadTrackedState` (`0x5128d0` → `0x57ea60`), before `VARIABLES_LOADED`: the town type
    /// `minimapTrackedInfo` names, if this class is offered it.
    pub fn load_tracked_state(&mut self, class_id: u8) {
        let town = {
            let mut model = self.model_mut();
            if class_id != 0 {
                model.minimap_tracking.class_id = class_id;
            }
            let saved = model
                .cvars
                .get("minimaptrackedinfo")
                .map(|s| s.value.clone())
                .unwrap_or_default();
            let town = model
                .minimap_tracking
                .towns()
                .find(|&i| TOWN_TYPES[i].token == saved);
            town
        };
        if let Some(town) = town {
            load_town(&self.lua, Some(town));
        }
    }

    /// The active town type, which filters the minimap's blips.
    pub fn minimap_town_tracking(&self) -> Option<&'static TownType> {
        self.model_ref()
            .minimap_tracking
            .town
            .map(|i| &TOWN_TYPES[i])
    }

    /// `OnUpdateShapeshiftForm` (`0x6da130`): a form entered with nothing tracked casts the first
    /// known tracking spell `allowed` admits in it.
    pub fn track_in_form(&mut self, allowed: impl Fn(u32) -> bool) {
        let pick = {
            let t = &self.model_ref().minimap_tracking;
            if t.spell != 0 || t.town.is_some() {
                return;
            }
            t.spells.iter().map(|s| s.spell_id).find(|&id| allowed(id))
        };
        if let Some(spell_id) = pick {
            self.model_mut().script_calls.push(ScriptCall::CastSpell {
                spell_id,
                at: CastAt::Selection,
                via_book: false,
            });
        }
    }

    /// `IsTrackingTrivialQuests` (`0x57bf30`): the low-level quest type is the town type.
    pub fn tracking_trivial_quests(&self) -> bool {
        self.minimap_town_tracking()
            .is_some_and(|t| t.kind == TownKind::TrivialQuests)
    }

    /// The active tracking spell, 0 for none.
    pub fn tracking_spell(&self) -> u32 {
        self.model_ref().minimap_tracking.spell
    }

    /// Drop the session's tracking spell and spells; the town type lives on in its CVar.
    pub fn clear_tracking(&mut self) {
        let mut model = self.model_mut();
        model.minimap_tracking.spells.clear();
        model.minimap_tracking.spell = 0;
    }
}

#[cfg(test)]
mod tests;
