//! The pet bindings: the action bar `PetActionBarFrame.lua` drives, the stat block the pet frames
//! read, and the pet menu's predicates and verbs. Every slot reads the live view at the call; the
//! plain orders act inside it, and the spell and Attack presses queue for the app to send. A token
//! slot's name and texture are the names of globals, which the stock Lua resolves
//! (`PetActionBarFrame.lua:98-104`). Booleans are 1/nil and cooldowns go cold once elapsed, as in
//! [`super::action`].

use mlua::{Lua, MultiValue, Value};

use super::ScriptValue;

// The slot-word values the one-shot orders synthesize, restated from
// `benilla_protocol::messages::pet` because this crate does not depend on the protocol.
const PET_ACT_COMMAND: u8 = 0x07;
const PET_ACT_REACTION: u8 = 0x06;
const PET_COMMAND_STAY: u32 = 0;
const PET_COMMAND_FOLLOW: u32 = 1;
const PET_COMMAND_ATTACK: u32 = 2;
const PET_REACT_PASSIVE: u32 = 0;
const PET_REACT_DEFENSIVE: u32 = 1;
const PET_REACT_AGGRESSIVE: u32 = 2;
/// The pet's bit in the temporary bar's owners (`0x5d4ad0`).
const TEMP_BAR_PET: u32 = 0x1;

/// A slot word as the server packs it: the type in the top byte, the action in the low bits.
const fn order(kind: u8, action: u32) -> u32 {
    (kind as u32) << 24 | action
}

/// `PET_DISMISS_START`'s argument, the dismiss cast in ms (`0x5d48c3`).
const PET_DISMISS_MS: i64 = 10_000;

/// A pet menu verb the app runs inside its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetMenuVerb<'a> {
    Abandon,
    Dismiss,
    Rename(&'a str),
}

/// What a pet menu verb did inside its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetMenuOutcome {
    Done,
    /// The dismiss order went out: `PET_DISMISS_START` fires (`0x5d48c3`).
    DismissStart,
    /// The pet is the controlled vehicle our seat may dismiss: its dismiss (`0x5d47fe`).
    VehicleExit,
    /// A refusal's message key, shown as the message display shows it (`0x5d56c8`, `0x5d575f`).
    Refused(&'static str),
}

/// A menu verb run inside the call when no earlier script call waits, as 12340's are done when they return; false leaves it to the queue behind that call.
fn menu_in_call(lua: &Lua, verb: PetMenuVerb<'_>) -> bool {
    if !crate::script::model_mut_of(lua).script_calls.is_empty() {
        return false;
    }
    let Some(outcome) = super::live::write_live(|l| l.pet_menu_verb(verb)).flatten() else {
        return false;
    };
    match outcome {
        PetMenuOutcome::Done => {}
        PetMenuOutcome::DismissStart => crate::script::tick::fire_event_into(
            lua,
            "PET_DISMISS_START",
            vec![ScriptValue::Int(PET_DISMISS_MS)],
        ),
        PetMenuOutcome::VehicleExit => crate::script::model_mut_of(lua)
            .vehicle_requests
            .push(super::VehicleRequest::Exit),
        PetMenuOutcome::Refused(key) => crate::script::model_mut_of(lua).ui_errors.push(key),
    }
    true
}

/// One pet bar slot, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetActionView {
    /// `GetPetActionInfo`'s first return and the occupancy test: `None` hides the button
    /// (`PetActionBarFrame.lua:122-128`). The spell's name, or for a token the name of a global.
    pub name: Option<String>,
    /// The second return: the spell's rank line, `None` for a token.
    pub subtext: Option<String>,
    /// The third return: an icon path, or for a token the name of a global.
    pub texture: Option<String>,
    /// A command or reaction token, whose `name` and `texture` are global names.
    pub is_token: bool,
    /// The slot's spell, for `GameTooltip:SetPetAction`; not a `GetPetActionInfo` return.
    pub spell_id: Option<u32>,
    /// The checked ring.
    pub active: bool,
    /// The slot can autocast: the static `UI-AutoCastableOverlay` ring.
    pub autocast_allowed: bool,
    /// Autocast is on: the sparkle trail.
    pub autocast_enabled: bool,
    /// `IsPetAttackActive`: a left click calls the pet off instead of running the slot.
    pub attack_active: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, as [`super::action::ActionState`].
    pub cooldown: Option<(i64, u32, bool)>,
    /// The slot's packed word, verbatim, for the drag ([`super::cursor::pet`]): the reference
    /// compares occupants under `& 0x3FFFFFFF` and writes the source word through unchanged. `0` is
    /// the empty slot.
    pub packed: u32,
    /// `SPELL_ATTR_PASSIVE` (`Attributes & 0x40`) on a spell slot: the drop silently refuses a
    /// passive source.
    pub passive: bool,
    /// `GetPetActionSlotUsable` (`0x5d3fd0`): the slot is usable now, which undims its icon.
    pub slot_usable: bool,
}

/// The pet stat block: the four hunter stat bindings and `HasPetUI`'s second return, plus the
/// family word, icon and diet. The stats share one hunter-pet gate and fail their own ways: loyalty
/// to nil, the pairs to `(0, 0)`, happiness to `(nil, 100.0)`. The family word has no gate; the
/// diet shares the stats'.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetStats {
    /// `HasPetUI`'s second return; false makes every gated binding answer its failure form.
    pub hunter_pet: bool,
    /// `GetPetHappiness`'s first return, bucketed `0..=3` by the client; `Some(0)` is not nil.
    pub happiness: Option<u32>,
    /// Return 2: the damage percentage, already scaled by 100.
    pub damage_percentage: f32,
    /// `GetPetExperience`: `(currXP, nextXP)`.
    pub experience: (u32, u32),
    /// `UnitCreatureFamily("pet")`: the localized `CreatureFamily.dbc` word off the cached creature
    /// record. `None` is nil: no record yet, id 0, or an id with no row (10, 13, 14, 18 and 22 have
    /// none). `PetPaperDollFrame.lua:68-70` guards its level line on it.
    pub family: Option<String>,
    /// `GetPetIcon()`: the family row's `CreatureFamily.dbc` icon, gated on a hunter pet (`0x5d3d1c`).
    pub icon: Option<String>,
    /// `GetPetFoodTypes()`: the diet names the family's food mask selects, in record order (bit
    /// `1 << (id - 1)` of `CreatureFamily` column 7, names from `ItemPetFood`). Empty for a zero
    /// mask (every warlock minion) or a failed hunter gate, which the app applies.
    pub food_types: Vec<String>,
    /// `GetPetTalentTree()` (`0x5d3d80`): the name of the first `TalentTab.dbc` row the family's
    /// talent type admits, for a pet that can gain experience (`0x71b630`); `None` is nil.
    pub talent_tree: Option<String>,
}

/// The pet bar's bar-wide state as its verbs read it at the call: the pet setter's guid, words and globals (`0x5d6900`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetBarLive {
    /// `PetHasActionBar()`: the cached pet guid is nonzero, even for ten empty slots.
    pub has_bar: bool,
    /// `GetPetActionsUsable()` (`0x5d3e80`): false desaturates the whole bar, and refuses the drag's bar writes with it.
    pub actions_usable: bool,
    /// `PickupPetAction`'s gate: the pet's object resolves and `UNIT_FLAG_POSSESSED` is clear, which blocks the drop as well as the pick-up.
    pub pickup_allowed: bool,
    /// `IsPetAttackActive()`, the pet-attacking global (`[0xc234dc]`, `0x5d3163`).
    pub attacking: bool,
    /// `GetPetTimeRemaining()`: the charm expiry less the tick count as an unsigned difference (`[0xc234cc]`), `None` while no expiry is held.
    pub time_remaining_ms: Option<u32>,
}

/// The possess spell `[0xc234e0]`'s record, read through the live view when called.
pub(crate) fn held_possess() -> Option<PossessView> {
    super::live::read_live(|l| l.possess()).flatten()
}

/// The held possess spell as `IsPossessBarVisible` and `GetPossessInfo` read its record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PossessView {
    /// The spell's id, for `GameTooltip:SetPossession(1)`.
    pub spell_id: u32,
    /// `IsPossessBarVisible()`: the spell carries no aura 236, `CONTROL_VEHICLE` (`0x5a8872`).
    pub bar_visible: bool,
    /// The spell's `Spell.dbc` name; `None` when the id has no record, which empties slot 1
    /// (`0x5d58dc`) and leaves slot 2 nameless.
    pub name: Option<String>,
    /// Slot 1's texture: the `ActiveIconID` row when that id is nonzero, else `SpellIconID`'s
    /// (`0x5d58e6`); `None` for a row `SpellIcon.dbc` lacks.
    pub icon: Option<String>,
    /// Slot 2's texture, `SpellIcon.dbc` row 693 (`0x5d5973`).
    pub cancel_icon: Option<String>,
    /// Slot 2's third return: false when the spell's `Attributes` sign bit is set (`0x5d59c7`).
    pub cancel_enabled: bool,
}

impl super::UiScript {
    /// Take the `CastPetAction` calls out of the call stream, as 1-based slots; the app decides
    /// what each one sends.
    pub fn take_pet_actions(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::PetAction(slot) => Some(*slot),
            _ => None,
        })
    }

    /// Take the one-shot orders (`PetAttack` and the rest) out of the call stream, each a packed
    /// slot word.
    pub fn take_pet_orders(&mut self) -> Vec<u32> {
        self.take_calls_where(|c| match c {
            super::ScriptCall::PetOrder(packed) => Some(*packed),
            _ => None,
        })
    }

    /// Drain the `PetAbandon()` and `PetDismiss()` call counts, as `(abandons, dismisses)`.
    pub fn take_pet_gives_up(&mut self) -> (u32, u32) {
        {
            let m = self.model_ref();
            if m.pet_abandons == 0 && m.pet_dismisses == 0 {
                return (0, 0);
            }
        }
        let m = &mut *self.model_mut();
        (
            std::mem::replace(&mut m.pet_abandons, 0),
            std::mem::replace(&mut m.pet_dismisses, 0),
        )
    }

    /// Drain the names `PetRename(name)` queued, in order.
    pub fn take_pet_renames(&mut self) -> Vec<String> {
        drain_field!(self, pet_renames)
    }
}

/// The pet stat block read live; with no view lent, no pet.
fn live_stats() -> (bool, PetStats) {
    super::read_live(|l| l.pet_stats()).unwrap_or_default()
}

/// The pet menu's predicates read live; with no view lent, the no-pet answers.
fn live_menu() -> (bool, bool, bool) {
    super::read_live(|l| l.pet_menu()).unwrap_or((false, false, true))
}

/// The pet bar's bar-wide state read live; with no view lent, no bar.
pub(crate) fn live_bar() -> PetBarLive {
    super::read_live(|l| l.pet_bar())
        .flatten()
        .unwrap_or_default()
}

/// The slot at a 1-based button index read live, `None` past the ten words or with no bar.
pub(crate) fn live_slot(lua: &Lua, i: u32) -> Option<PetActionView> {
    let index = usize::try_from(i.checked_sub(1)?).ok()?;
    let (anchor, ui_now) = {
        let model = lua
            .app_data_ref::<crate::script::ModelCell>()
            .expect("model app_data");
        (model.clock_wall.get(), model.now)
    };
    super::read_live(|l| l.pet_action(index, anchor, ui_now)).flatten()
}

/// The ten slots read live, without their cooldowns, for the readers that want the words alone.
pub(crate) fn live_slots() -> Vec<PetActionView> {
    let now = std::time::Instant::now();
    (0..10)
        .map_while(|i| super::read_live(|l| l.pet_action(i, now, 0.0)).flatten())
        .collect()
}

/// A press run inside the verb (`0x5d4210`) when no earlier script call waits, so it reads the selection those calls would move; its `PET_BAR_UPDATE` fires before the verb returns (`0x5d4530`). False leaves it to the script call: a spell or Attack, or a press behind a queued call.
fn press_in_call(lua: &Lua, slot: u32, order: Option<u32>) -> bool {
    if !crate::script::model_mut_of(lua).script_calls.is_empty() {
        return false;
    }
    match super::live::write_live(|l| l.pet_press(slot, order)).flatten() {
        Some(signalled) => {
            if signalled {
                crate::script::tick::fire_event_into(lua, "PET_BAR_UPDATE", Vec::new());
            }
            true
        }
        None => false,
    }
}

/// Register the pet-bar globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };

    // GetPetTimeRemaining() (`0x5d3180`): nil with no expiry, else the milliseconds to it as an unsigned 32-bit tick difference, so one past its expiry wraps to about 4.29e9 until the next packet. The stock caller is commented out (`PetFrame.lua:91-97`).
    g.set(
        "GetPetTimeRemaining",
        lua.create_function(|_, ()| {
            Ok(match live_bar().time_remaining_ms {
                Some(ms) => Value::Number(f64::from(ms)),
                None => Value::Nil,
            })
        })?,
    )?;

    // IsPossessBarVisible() (`0x5a8820`): false while no possess spell `[0xc234e0]` is held.
    g.set(
        "IsPossessBarVisible",
        lua.create_function(|_, _: mlua::MultiValue| {
            Ok(held_possess().is_some_and(|p| p.bar_visible))
        })?,
    )?;
    // GetPossessInfo(index) (`0x5d5820`): a zero index raises; the rounded index picks the spell
    // (1) or its cancel button (2), anything else or no spell three nils.
    g.set(
        "GetPossessInfo",
        lua.create_function(|lua, index: Value| {
            let index = lua.coerce_number(index)?.unwrap_or(0.0);
            if index == 0.0 {
                return Err(mlua::Error::runtime("Usage: GetPossessInfo(index)"));
            }
            let str_or_nil = |s: &Option<String>| -> mlua::Result<Value> {
                s.as_deref()
                    .map_or(Ok(Value::Nil), |s| lua.create_string(s).map(Value::String))
            };
            let nils = (Value::Nil, Value::Nil, Value::Nil);
            let Some(p) = held_possess() else {
                return Ok(nils);
            };
            #[allow(clippy::cast_possible_truncation)] // `0x5d58a7` rounds to an int
            Ok(match index.round() as i64 {
                1 if p.name.is_some() => (
                    str_or_nil(&p.icon)?,
                    str_or_nil(&p.name)?,
                    Value::Boolean(true),
                ),
                2 => (
                    str_or_nil(&p.cancel_icon)?,
                    str_or_nil(&p.name)?,
                    Value::Boolean(p.cancel_enabled),
                ),
                _ => nils,
            })
        })?,
    )?;

    // PetHasActionBar() (`0x5d3720`): nil while the pet's actions own the main bar's temporary
    // page (`m_tempPageActiveFlags & 1`), so the stock pet bar hides behind the bonus bar.
    g.set(
        "PetHasActionBar",
        lua.create_function(move |lua, ()| {
            let temp = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data")
                .temp_action_bar;
            Ok(flag(live_bar().has_bar && temp & TEMP_BAR_PET == 0))
        })?,
    )?;

    // GetPetActionsUsable() (`0x5d53c0`): the usability predicate at the call.
    g.set(
        "GetPetActionsUsable",
        lua.create_function(move |_, ()| Ok(flag(live_bar().actions_usable)))?,
    )?;

    // GetPetActionInfo(index) (`0x5d4ec0`): the slot word read at the call; an out-of-range index answers a single nil, which the Lua reads as an empty slot.
    g.set(
        "GetPetActionInfo",
        lua.create_function(move |lua, i: u32| {
            let Some(v) = live_slot(lua, i) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let text = |s: &Option<String>| match s {
                Some(s) => Ok(Value::String(lua.create_string(s)?)),
                None => Ok::<_, mlua::Error>(Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                text(&v.name)?,
                text(&v.subtext)?,
                text(&v.texture)?,
                flag(v.is_token),
                flag(v.active),
                flag(v.autocast_allowed),
                flag(v.autocast_enabled),
            ]))
        })?,
    )?;

    // GetPetActionCooldown(index) (`0x5d5280`): the pet's cooldown history asked at the call (`0x5d5345` into `0x809000`); an elapsed or absent cooldown answers (0, 0, 1).
    g.set(
        "GetPetActionCooldown",
        lua.create_function(|lua, i: u32| {
            let now = crate::script::clock::now(lua);
            let cooldown = live_slot(lua, i).and_then(|s| s.cooldown).map(
                |(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                },
            );
            Ok(match cooldown {
                Some((start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    // GetPetActionSlotUsable(index) (`0x5d5400`): 1 or nil; with no pet or past slot 10 there is
    // no slot word, which reads usable (`0x5d3fd0`).
    g.set(
        "GetPetActionSlotUsable",
        lua.create_function(move |lua, i: Value| {
            let i =
                super::binding_abi::number_arg(lua, i, "Usage: GetPetActionSlotsUsable(index)")?;
            // The slot word and the usability walk, both at the call.
            let usable = u32::try_from(i)
                .ok()
                .and_then(|i| live_slot(lua, i))
                .is_none_or(|s| s.slot_usable);
            Ok(flag(usable))
        })?,
    )?;

    // IsPetAttackAction(index) (`0x5d5a10`): a real boolean, true for the Attack order's slot while
    // there is a pet (the pet info's attack-action test, `0x5d41b0`).
    g.set(
        "IsPetAttackAction",
        lua.create_function(|lua, i: Value| {
            let i = super::binding_abi::number_arg(lua, i, "Usage: IsPetAttackAction(index)")?;
            Ok(u32::try_from(i)
                .ok()
                .and_then(|i| live_slot(lua, i))
                .is_some_and(|s| {
                    s.packed & 0x3f00_0000 == 0x0700_0000 && s.packed & 0xff_ffff == 2
                }))
        })?,
    )?;

    // IsPetAttackActive() (`0x5d3160`): the pet-attacking global as a real boolean; any argument is ignored.
    g.set(
        "IsPetAttackActive",
        lua.create_function(move |_, _: MultiValue| Ok(live_bar().attacking))?,
    )?;

    // An empty slot, whose button shows only under show-grid, queues nothing; the reference sends
    // its zero word, which vmangos ignores (`PetHandler.cpp:178`).
    g.set(
        "CastPetAction",
        lua.create_function(|lua, i: u32| {
            if live_slot(lua, i).is_some_and(|s| s.name.is_some()) && !press_in_call(lua, i, None) {
                crate::script::model_mut_of(lua)
                    .script_calls
                    .push(super::ScriptCall::PetAction(i));
            }
            Ok(())
        })?,
    )?;

    // TogglePetAutocast(index) (`0x5d66f0`): `index - 1` in 0..=10 (`0x5d6745`), the cursor cleared or its pet action dropped (`0x5d676d`), else the toggle runs inside the call and signals the bar after its send (`0x5d6794`, `0x5d5e01`).
    g.set(
        "TogglePetAutocast",
        lua.create_function(|lua, i: Value| {
            let i = super::binding_abi::number_arg(lua, i, "Usage: TogglePetAutocast(index)")?;
            let index = (i as u32).wrapping_sub(1);
            if index > 10 {
                return Err(mlua::Error::runtime("Invalid slot in TogglePetAutocast"));
            }
            if crate::script::cursor::toggle_cursor_fork(lua, i as u32) {
                return Ok(());
            }
            if crate::script::live::write_live(|l| l.pet_toggle_autocast(index)) == Some(true) {
                crate::script::tick::fire_event_into(lua, "PET_BAR_UPDATE", Vec::new());
            }
            Ok(())
        })?,
    )?;

    // HasPetUI() (`0x5d3960`): two returns on every path, `(nil, nil)` with no pet UI.
    g.set(
        "HasPetUI",
        lua.create_function(move |_, ()| {
            let (has_ui, stats) = live_stats();
            Ok((flag(has_ui), flag(has_ui && stats.hunter_pet)))
        })?,
    )?;

    // GetPetHappiness() (`0x5d3b00`): two returns, the bucket off `PetPersonality.dbc` plus one
    // and its damage percentage; `nil, 100` without a hunter's pet.
    g.set(
        "GetPetHappiness",
        lua.create_function(move |_, ()| {
            let (_, s) = live_stats();
            Ok(match s.happiness.filter(|_| s.hunter_pet) {
                Some(b) => (Value::Integer(i64::from(b)), f64::from(s.damage_percentage)),
                None => (Value::Nil, 100.0),
            })
        })?,
    )?;

    // GetPetExperience() (`0x5d3a20`): numbers on every path, `(0, 0)` unless the pet can gain experience (`0x5d3a5d`).
    g.set(
        "GetPetExperience",
        lua.create_function(move |_, ()| {
            let (_, s) = live_stats();
            let (cur, next) = if s.hunter_pet { s.experience } else { (0, 0) };
            Ok((f64::from(cur), f64::from(next)))
        })?,
    )?;

    // UnitCreatureFamily(unit) (`0x611820`): any unit's family off its cached creature record, nil without a record or a row; a non-string raises the usage.
    g.set(
        "UnitCreatureFamily",
        lua.create_function(move |lua, unit: Value| {
            let unit =
                super::binding_abi::string_arg(lua, unit, r#"Usage: UnitCreatureFamily("unit")"#)?;
            // The token resolves off the pushed unit guids until the unit tokens read live (#1289).
            let guid = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .token_guid(&unit)
                    .or_else(|| model.unit(&unit).filter(|u| u.exists).map(|u| u.guid))
            };
            let family = guid.and_then(|g| super::read_live(|l| l.creature_family(g)).flatten());
            Ok(match family {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetIcon() (`0x5d3ce0`): the family's icon for a pet that can gain experience; a real nil, never "", since the stable reads it as a predicate and as a texture (`PetStable.lua:51`).
    g.set(
        "GetPetIcon",
        lua.create_function(move |lua, ()| {
            let (_, s) = live_stats();
            Ok(match s.icon.as_deref().filter(|_| s.hunter_pet) {
                Some(path) => Value::String(lua.create_string(path)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetTalentTree() (`0x5d3d80`).
    g.set(
        "GetPetTalentTree",
        lua.create_function(move |lua, ()| {
            let (_, s) = live_stats();
            Ok(match s.talent_tree.as_deref() {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetFoodTypes() (`0x5d3bd0`): one return per diet and never a nil: an empty diet is zero values, for which `BuildListString` answers nil (`PetPaperDollFrame.xml:269`).
    g.set(
        "GetPetFoodTypes",
        lua.create_function(move |lua, ()| {
            let (_, s) = live_stats();
            let values: Vec<Value> = s
                .food_types
                .iter()
                .map(|f| lua.create_string(f).map(Value::String))
                .collect::<mlua::Result<_>>()?;
            Ok(MultiValue::from_vec(values))
        })?,
    )?;

    // The six one-shot orders each write the slot word the bar's own reaction or command slot
    // carries and re-enter, as a `CastPetAction` press does; PetAttack takes the current selection.
    // With no pet bar the order is silently dropped.
    for (name, packed) in [
        ("PetPassiveMode", order(PET_ACT_REACTION, PET_REACT_PASSIVE)),
        (
            "PetDefensiveMode",
            order(PET_ACT_REACTION, PET_REACT_DEFENSIVE),
        ),
        (
            "PetAggressiveMode",
            order(PET_ACT_REACTION, PET_REACT_AGGRESSIVE),
        ),
        ("PetWait", order(PET_ACT_COMMAND, PET_COMMAND_STAY)),
        ("PetFollow", order(PET_ACT_COMMAND, PET_COMMAND_FOLLOW)),
        ("PetAttack", order(PET_ACT_COMMAND, PET_COMMAND_ATTACK)),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                if live_bar().has_bar && !press_in_call(lua, 0, Some(packed)) {
                    crate::script::model_mut_of(lua)
                        .script_calls
                        .push(super::ScriptCall::PetOrder(packed));
                }
                Ok(())
            })?,
        )?;
    }

    // PetStopAttack() (`0x5d5640`): the clear runs inside the call (`0x5d4650`).
    g.set(
        "PetStopAttack",
        lua.create_function(|_, ()| {
            crate::script::live::write_live(|l| l.pet_stop_attack());
            Ok(())
        })?,
    )?;

    // ── The pet menu ─────────────────────────────────────────────────────────

    // The three predicates read the pet list's head when called (`0x5d3780`, `0x5d3820`, `0x5d38c0`).
    g.set(
        "PetCanBeAbandoned",
        lua.create_function(move |_, ()| Ok(flag(live_menu().0)))?,
    )?;

    g.set(
        "PetCanBeDismissed",
        lua.create_function(move |_, ()| Ok(flag(live_menu().2)))?,
    )?;

    g.set(
        "PetCanBeRenamed",
        lua.create_function(move |_, ()| Ok(flag(live_menu().1)))?,
    )?;

    // The `ABANDON_PET` popup's OnAccept, after its confirm.
    g.set(
        "PetAbandon",
        lua.create_function(|lua, ()| {
            if !menu_in_call(lua, PetMenuVerb::Abandon) {
                crate::script::model_mut_of(lua).pet_abandons += 1;
            }
            Ok(())
        })?,
    )?;

    // The menu row itself, with no confirm (`UnitPopup.lua:591`).
    g.set(
        "PetDismiss",
        lua.create_function(|lua, ()| {
            if !menu_in_call(lua, PetMenuVerb::Dismiss) {
                crate::script::model_mut_of(lua).pet_dismisses += 1;
            }
            Ok(())
        })?,
    )?;

    // A number coerces, as `lua_tostring` does. Every call is queued, a missing name as an empty
    // one: `PetRename 0x5d5670` checks the pet before the name, and shows `ERR_NAME_NO_NAME` for
    // either, so the app runs both checks.
    g.set(
        "PetRename",
        lua.create_function(|lua, name: Option<mlua::String>| {
            let name = match name {
                Some(name) => name.to_str()?.to_string(),
                None => String::new(),
            };
            if !menu_in_call(lua, PetMenuVerb::Rename(&name)) {
                crate::script::model_mut_of(lua).pet_renames.push(name);
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PetActionView, PetStats};
    use crate::script::UiScript;

    /// A charmed or possessed unit's time left in ms as the view reads it at the call, nil for none, and a wrapped difference passed through as a number.
    #[test]
    fn pet_time_remaining_reads_the_expiry_at_the_call() {
        let s = UiScript::new().unwrap();
        assert!(s
            .eval::<Option<f64>>("return GetPetTimeRemaining()")
            .unwrap()
            .is_none());
        let lend = |ms: Option<u32>| {
            crate::script::live::lend_test(crate::script::live::TestView {
                pet_bar: Some(super::PetBarLive {
                    has_bar: true,
                    time_remaining_ms: ms,
                    ..Default::default()
                }),
                ..Default::default()
            })
        };
        let loan = lend(Some(2500));
        assert_eq!(
            s.eval::<f64>("return GetPetTimeRemaining()").unwrap(),
            2500.0
        );
        drop(loan);
        let loan = lend(Some(u32::MAX - 499));
        assert_eq!(
            s.eval::<f64>("return GetPetTimeRemaining()").unwrap(),
            f64::from(u32::MAX - 499),
            "0.5 s past it"
        );
        drop(loan);
        let _none = lend(None);
        assert!(s
            .eval::<Option<f64>>("return GetPetTimeRemaining()")
            .unwrap()
            .is_none());
    }

    /// With nothing possessed the bar is hidden by a real `false`, and each slot reads three nils.
    #[test]
    fn with_nothing_possessed_the_bar_hides_and_its_slots_are_empty() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<mlua::Value>("return IsPossessBarVisible()")
                .unwrap(),
            mlua::Value::Boolean(false)
        );
        assert_eq!(s.arity("GetPossessInfo(1)").unwrap(), 3);
        assert!(s
            .eval::<Option<String>>("return (GetPossessInfo(2))")
            .unwrap()
            .is_none());
        for bad in [
            "GetPossessInfo(0)",
            "GetPossessInfo()",
            "GetPossessInfo('x')",
        ] {
            assert!(s.run(bad).is_err(), "{bad}");
        }
    }

    /// A held spell shows the bar; slot 1 is the spell, slot 2 the cancel button under the spell's
    /// name, the rounded index picks between them, and a third slot is empty.
    #[test]
    fn a_held_possess_spell_fills_the_spell_and_cancel_slots() {
        let s = UiScript::new().unwrap();
        let held = super::PossessView {
            spell_id: 605,
            bar_visible: true,
            name: Some("possess-spell".into()),
            icon: Some("spell-icon".into()),
            cancel_icon: Some("cancel-icon".into()),
            cancel_enabled: true,
        };
        let lend = |p: Option<super::PossessView>| {
            crate::script::live::lend_test(crate::script::live::TestView {
                possess: p,
                ..Default::default()
            })
        };
        let loan = lend(Some(held.clone()));
        assert_eq!(
            s.eval::<mlua::Value>("return IsPossessBarVisible()")
                .unwrap(),
            mlua::Value::Boolean(true)
        );
        let slot = |s: &UiScript, i: &str| {
            s.eval::<(Option<String>, Option<String>, Option<bool>)>(&format!(
                "return GetPossessInfo({i})"
            ))
            .unwrap()
        };
        let spell = (held.icon.clone(), held.name.clone(), Some(true));
        let cancel = (held.cancel_icon.clone(), held.name.clone(), Some(true));
        assert_eq!(slot(&s, "1"), spell);
        assert_eq!(slot(&s, "1.4"), spell, "rounded");
        assert_eq!(slot(&s, "'2'"), cancel, "a numeric string is a number");
        assert_eq!(slot(&s, "1.6"), cancel, "rounded up");
        assert_eq!(slot(&s, "3"), (None, None, None));

        // A vehicle's control aura hides the bar; a sign-bit spell disables the cancel button; a
        // spell with no record empties slot 1 and names nothing on slot 2.
        drop(loan);
        let loan = lend(Some(super::PossessView {
            bar_visible: false,
            cancel_enabled: false,
            name: None,
            ..held
        }));
        assert_eq!(
            s.eval::<mlua::Value>("return IsPossessBarVisible()")
                .unwrap(),
            mlua::Value::Boolean(false)
        );
        assert_eq!(slot(&s, "1"), (None, None, None));
        assert_eq!(
            slot(&s, "2"),
            (Some("cancel-icon".into()), None, Some(false))
        );
        drop(loan);
        let _none = lend(None);
        assert_eq!(slot(&s, "2"), (None, None, None));
    }

    /// Attack (a token, attacking), Claw (a spell, autocasting, cooling down), an empty slot.
    fn slots() -> Vec<PetActionView> {
        vec![
            PetActionView {
                name: Some("PET_ACTION_ATTACK".into()),
                texture: Some("PET_ATTACK_TEXTURE".into()),
                is_token: true,
                active: true,
                attack_active: true,
                ..Default::default()
            },
            PetActionView {
                name: Some("Claw".into()),
                subtext: Some("Rank 3".into()),
                texture: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
                autocast_allowed: true,
                autocast_enabled: true,
                cooldown: Some((9400, 1500, true)),
                ..Default::default()
            },
            PetActionView::default(),
        ]
    }

    #[test]
    fn slot_info_reads_and_out_of_range_is_one_nil() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return PetHasActionBar() == nil").unwrap());
        assert!(s.eval::<bool>("return GetPetActionInfo(1) == nil").unwrap());

        let _pets = crate::script::live::lend_pets(true, true, true, slots());
        assert_eq!(s.eval::<i64>("return PetHasActionBar()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPetActionsUsable()").unwrap(), 1);

        let (name, subtext, texture, is_token, active, allowed, enabled) = s
            .eval::<(
                String,
                Option<String>,
                String,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
            )>("return GetPetActionInfo(1)")
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                subtext,
                texture.as_str(),
                is_token,
                active,
                allowed,
                enabled
            ),
            (
                "PET_ACTION_ATTACK",
                None,
                "PET_ATTACK_TEXTURE",
                Some(1),
                Some(1),
                None,
                None
            )
        );

        assert!(s
            .eval::<bool>(
                "local n, sub, tex, tok, act, allow, on = GetPetActionInfo(2) \
                 return n == 'Claw' and sub == 'Rank 3' and tok == nil and act == nil \
                 and allow == 1 and on == 1 and string.find(tex, 'Icons') ~= nil"
            )
            .unwrap());

        // The empty slot has returns, unlike the out-of-range single nil, but no name.
        assert!(s
            .eval::<bool>("local n, _, tex = GetPetActionInfo(3) return n == nil and tex == nil")
            .unwrap());
        assert!(s.eval::<bool>("return GetPetActionInfo(4) == nil").unwrap());
    }

    #[test]
    fn cooldown_triple_stamps_to_the_vm_clock_and_goes_cold() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0); // GetTime == 10
        let _pets = crate::script::live::lend_pets(true, true, true, slots());

        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetPetActionCooldown(1)")
                .unwrap(),
            (0.0, 0.0, 1),
            "no cooldown reads cold"
        );
        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>("return GetPetActionCooldown(2)")
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 1.5).abs() < 1e-9);
        assert_eq!(enable, 1);

        s.tick(2.0); // now == 12 > 9.4 + 1.5
        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetPetActionCooldown(2)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    #[test]
    fn intents_queue_and_the_meaningless_ones_are_dropped() {
        let mut s = UiScript::new().unwrap();
        let pets = crate::script::live::lend_pets(true, true, true, slots());

        s.run("CastPetAction(1) CastPetAction(3) CastPetAction(9)")
            .unwrap();
        assert_eq!(
            s.take_pet_actions(),
            vec![1],
            "empty + out-of-range dropped"
        );
        assert!(s.take_pet_actions().is_empty(), "drain empties");

        s.run("PetStopAttack() PetStopAttack()").unwrap();
        assert_eq!(
            pets.view().pet_stops.get(),
            2,
            "each reaches the clear inside its call"
        );
    }

    /// `TogglePetAutocast` toggles inside the call (`0x5d6794`): the bar's handler and next verb read the flipped slot, a token signals unflipped, and slot 11 passes the gate (`0x5d6745`).
    #[test]
    fn the_autocast_toggle_flips_and_signals_inside_the_call() {
        let s = UiScript::new().unwrap();
        let pets = crate::script::live::lend_pets(true, true, true, slots());
        s.run(
            "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('PET_BAR_UPDATE')              f:SetScript('OnEvent', function() SEEN[#SEEN + 1] = select(7, GetPetActionInfo(2)) or 0 end)",
        )
        .unwrap();
        let after = s
            .eval::<(i64, Option<i64>)>(
                "TogglePetAutocast(2) return #SEEN, select(7, GetPetActionInfo(2))",
            )
            .unwrap();
        assert_eq!(
            after,
            (1, None),
            "one signal inside the call, the next verb reads it off"
        );
        assert_eq!(
            s.eval::<i64>("return SEEN[1]").unwrap(),
            0,
            "the handler read it off"
        );
        s.run("TogglePetAutocast(1) TogglePetAutocast(11)").unwrap();
        assert_eq!(
            s.eval::<i64>("return #SEEN").unwrap(),
            3,
            "a token and slot 11 signal too"
        );
        assert_eq!(*pets.view().pet_toggles.borrow(), vec![1, 0, 10]);
        for (call, err) in [
            ("TogglePetAutocast(0)", "Invalid slot in TogglePetAutocast"),
            ("TogglePetAutocast(12)", "Invalid slot in TogglePetAutocast"),
            ("TogglePetAutocast('x')", "Usage: TogglePetAutocast(index)"),
        ] {
            let e = s.run(call).unwrap_err().to_string();
            assert!(e.contains(err), "{call}: {e}");
        }
        drop(pets);
        let _unusable = crate::script::live::lend_pets(true, false, true, slots());
        s.run("TogglePetAutocast(2)").unwrap();
        assert_eq!(
            s.eval::<i64>("return #SEEN").unwrap(),
            3,
            "an unusable pet signals nothing"
        );
    }

    /// `IsPetAttackActive` answers the pet-attacking global whatever its argument (`0x5d3160`).
    #[test]
    fn attack_active_is_the_global_whatever_the_slot() {
        let s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return IsPetAttackActive() == false")
            .unwrap());
        let _pets = crate::script::live::lend_pets(true, true, true, slots());
        for call in [
            "IsPetAttackActive()",
            "IsPetAttackActive(1)",
            "IsPetAttackActive(2)",
            "IsPetAttackActive(9)",
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} == true")).unwrap(),
                "{call}"
            );
        }
    }

    /// `IsPetAttackAction` reads the slot word, a real boolean (`0x5d5a10`); the stock bar's
    /// `OnLoad` calls it for every slot.
    #[test]
    fn is_pet_attack_action_reads_the_attack_order_word() {
        let s = UiScript::new().unwrap();
        let mut bar = slots();
        bar[0].packed = 0x0700_0002;
        bar[1].packed = 0xC100_0BC2;
        let _pets = crate::script::live::lend_pets(true, true, true, bar);
        assert!(s
            .eval::<bool>("return IsPetAttackAction(1) == true")
            .unwrap());
        assert!(s
            .eval::<bool>("return IsPetAttackAction(2) == false")
            .unwrap());
        assert!(s
            .eval::<bool>("return IsPetAttackAction(11) == false")
            .unwrap());
        let err = s.run("IsPetAttackAction()").unwrap_err().to_string();
        assert!(err.contains("Usage: IsPetAttackAction(index)"), "{err}");
    }

    /// `GetPetActionSlotUsable` answers the lent slot's walk at the call, and 1 with no slot (`0x5d5400`).
    #[test]
    fn slot_usable_reads_the_lent_walk_and_no_slot_is_usable() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetPetActionSlotUsable(1)").unwrap(),
            1,
            "no pet, no slot word"
        );
        let mut bar = slots();
        bar[0].slot_usable = true;
        let _pets = crate::script::live::lend_pets(true, true, true, bar);
        assert_eq!(
            s.eval::<i64>("return GetPetActionSlotUsable(1)").unwrap(),
            1
        );
        assert!(s
            .eval::<bool>("return GetPetActionSlotUsable(2) == nil")
            .unwrap());
        let err = s.run("GetPetActionSlotUsable()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: GetPetActionSlotsUsable(index)"),
            "{err}"
        );
    }

    /// The predicates are checked the way `UnitPopup.lua`'s row conditions read them.
    #[test]
    fn the_menu_predicates_fork_the_rows_and_the_verbs_queue() {
        let mut s = UiScript::new().unwrap();

        // No pet pushed: every row is off, but Dismiss answers 1 (`0x5d3893`).
        assert!(s
            .eval::<bool>("return PetCanBeAbandoned() == nil and PetCanBeRenamed() == nil")
            .unwrap());
        assert_eq!(s.eval::<i64>("return PetCanBeDismissed()").unwrap(), 1);

        // A hunter's freshly tamed pet.
        let _live = menu_loan(true, true, true);
        assert_eq!(s.eval::<i64>("return PetCanBeAbandoned()").unwrap(), 1);
        assert!(
            s.eval::<bool>("return PetCanBeAbandoned() and PetCanBeRenamed()")
                .unwrap(),
            "the rename row wants BOTH"
        );
        assert!(!s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());

        // The same pet after one rename.
        let _live = menu_loan(true, false, true);
        assert!(s.eval::<bool>("return PetCanBeAbandoned() ~= nil").unwrap());
        assert!(s.eval::<bool>("return PetCanBeRenamed() == nil").unwrap());

        // A warlock's demon: only Dismiss shows.
        let _live = menu_loan(false, false, true);
        assert!(s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());
        // A summon whose `SummonProperties` row bars it: no Dismiss either.
        let _live = menu_loan(false, false, false);
        assert!(s.eval::<bool>("return PetCanBeDismissed() == nil").unwrap());

        assert_eq!(s.take_pet_gives_up(), (0, 0));
        s.run("PetAbandon() PetDismiss() PetDismiss()").unwrap();
        assert_eq!(s.take_pet_gives_up(), (1, 2));
        assert_eq!(s.take_pet_gives_up(), (0, 0), "drain empties");

        // An empty name is queued for the app's checks, a number coerces, no argument is empty.
        s.run("PetRename(\"Bruce\") PetRename(\"\") PetRename(7) PetRename()")
            .unwrap();
        assert_eq!(
            s.take_pet_renames(),
            vec![
                "Bruce".to_string(),
                String::new(),
                "7".to_string(),
                String::new()
            ]
        );
        assert!(s.take_pet_renames().is_empty(), "drain empties");
    }

    #[test]
    fn a_disabled_bar_is_still_a_bar() {
        let s = UiScript::new().unwrap();
        let _pets = crate::script::live::lend_pets(true, false, true, slots());
        assert_eq!(s.eval::<i64>("return PetHasActionBar()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return GetPetActionsUsable() == nil")
            .unwrap());
    }

    /// Reactions are type 6 and commands type 7, each with its state in the low byte.
    #[test]
    fn the_pet_one_shots_synthesize_the_bars_slot_words() {
        let mut s = UiScript::new().unwrap();
        s.run("PetAttack(); PetFollow(); PetWait(); PetPassiveMode()")
            .unwrap();
        assert!(s.take_pet_orders().is_empty(), "no bar, no order");
        let _pets = crate::script::live::lend_pets(true, true, true, slots());
        s.run(
            "PetPassiveMode(); PetDefensiveMode(); PetAggressiveMode(); PetWait(); PetFollow(); \
             PetAttack()",
        )
        .unwrap();
        assert_eq!(
            s.take_pet_orders(),
            vec![
                0x0600_0000,
                0x0600_0001,
                0x0600_0002,
                0x0700_0000,
                0x0700_0001,
                0x0700_0002
            ]
        );
        assert!(s.take_pet_orders().is_empty(), "drained");
    }

    #[test]
    fn the_pet_icon_answers_a_path_or_a_real_nil() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetPetIcon() == nil").unwrap());

        let _live = stats_loan(true, hunter_stats());
        assert_eq!(
            s.eval::<String>("return GetPetIcon()").unwrap(),
            "Interface\\Icons\\Ability_Hunter_Pet_Boar"
        );

        assert_eq!(s.arity("GetPetIcon()").unwrap(), 1);
    }

    /// `GetPetTalentTree` is the live tree name, nil without one (`0x5d3d80`).
    #[test]
    fn the_talent_tree_is_the_pushed_name_or_nil() {
        let s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetPetTalentTree() == nil").unwrap());
        let _live = stats_loan(true, hunter_stats());
        assert_eq!(
            s.eval::<String>("return GetPetTalentTree()").unwrap(),
            "Tenacity"
        );
    }

    /// `stats` lent as the pet list's head (guid 2, ours 1), for the test's length.
    fn stats_loan(
        has_ui: bool,
        stats: PetStats,
    ) -> crate::script::LiveLoan<crate::script::live::TestView> {
        crate::script::live::lend_test(crate::script::live::TestView {
            player: Some(1),
            units: [(1, (0, 1)), (2, (0, 1))].into(),
            pet_stats: (has_ui, stats),
            ..Default::default()
        })
    }

    /// The three menu predicates lent, for the test's length.
    fn menu_loan(
        abandon: bool,
        rename: bool,
        dismiss: bool,
    ) -> crate::script::LiveLoan<crate::script::live::TestView> {
        crate::script::live::lend_test(crate::script::live::TestView {
            pet_menu: Some((abandon, rename, dismiss)),
            ..Default::default()
        })
    }

    /// Our guid 1 and the pet's 2 as the unit tokens name them.
    fn pet_tokens(s: &mut UiScript) {
        s.set_unit_guids(&crate::script::UnitGuids {
            player: 1,
            pet: 2,
            held: [(1, 0), (2, 0)].into(),
            ..Default::default()
        });
    }

    fn hunter_stats() -> PetStats {
        PetStats {
            hunter_pet: true,
            icon: Some("Interface\\Icons\\Ability_Hunter_Pet_Boar".into()),
            happiness: Some(3),
            damage_percentage: 125.0,
            experience: (4200, 8000),
            family: Some("Boar".into()),
            food_types: vec![
                "Meat".into(),
                "Fish".into(),
                "Cheese".into(),
                "Bread".into(),
                "Fungus".into(),
                "Fruit".into(),
            ],
            talent_tree: Some("Tenacity".into()),
        }
    }

    #[test]
    fn a_hunters_pet_answers_every_stat_binding() {
        let s = UiScript::new().unwrap();
        let _live = stats_loan(true, hunter_stats());

        assert!(s
            .eval::<bool>(
                "local h, dmg = GetPetHappiness() \
                 return h == 3 and dmg == 125 and select('#', GetPetHappiness()) == 2"
            )
            .unwrap());
        assert!(s
            .eval::<bool>("local c, n = GetPetExperience() return c == 4200 and n == 8000")
            .unwrap());
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == 1 and hunter == 1")
            .unwrap());
    }

    #[test]
    fn a_non_hunter_pet_fails_three_different_ways() {
        let s = UiScript::new().unwrap();
        let _live = stats_loan(
            true,
            PetStats {
                hunter_pet: false,
                ..hunter_stats()
            },
        );

        assert!(
            s.eval::<bool>(
                "local h, dmg = GetPetHappiness() \
                 return h == nil and dmg == 100 and select('#', GetPetHappiness()) == 2"
            )
            .unwrap(),
            "happiness is nil but its two numbers are still numbers"
        );
        assert!(s
            .eval::<bool>("local c, n = GetPetExperience() return c == 0 and n == 0")
            .unwrap());
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == 1 and hunter == nil")
            .unwrap());
    }

    #[test]
    fn happiness_bucket_zero_is_not_the_failure_case() {
        let s = UiScript::new().unwrap();
        let _live = stats_loan(
            true,
            PetStats {
                happiness: Some(0),
                damage_percentage: 100.0,
                ..hunter_stats()
            },
        );
        assert!(s
            .eval::<bool>("local h = GetPetHappiness() return h == 0 and h ~= nil")
            .unwrap());
        assert!(
            s.eval::<bool>("return not GetPetHappiness() == false")
                .unwrap(),
            "0 is truthy in Lua, so the reference's `not happiness` hide-test does NOT fire"
        );
    }

    #[test]
    fn no_pet_still_answers_two_values_from_has_pet_ui() {
        let s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == nil and hunter == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("local h, dmg = GetPetHappiness() return h == nil and dmg == 100")
            .unwrap());
    }

    #[test]
    fn unit_creature_family_is_nil_on_every_absent_path() {
        let mut s = UiScript::new().unwrap();
        pet_tokens(&mut s);
        // No pet pushed.
        assert!(s
            .eval::<bool>(r#"return UnitCreatureFamily("pet") == nil"#)
            .unwrap());

        // Family 0 and an unanswered creature query both arrive as `None`.
        let _live = stats_loan(
            true,
            PetStats {
                family: None,
                ..hunter_stats()
            },
        );
        assert!(s
            .eval::<bool>(r#"return UnitCreatureFamily("pet") == nil"#)
            .unwrap());
        assert!(
            s.eval::<bool>(r#"return UnitCreatureFamily("pet") ~= ''"#)
                .unwrap(),
            "nil, never an empty string — '' is TRUTHY in Lua, so it would pass the ref's guard \
             and print a bare 'Level 58 ' with a trailing space"
        );

        // With a family: `"pet"` answers it; a token naming nobody, or a player, nil.
        let _live = stats_loan(true, hunter_stats());
        assert_eq!(
            s.eval::<String>(r#"return UnitCreatureFamily("pet")"#)
                .unwrap(),
            "Boar"
        );
        for token in [r#""target""#, r#""player""#] {
            assert!(
                s.eval::<bool>(&format!("return UnitCreatureFamily({token}) == nil"))
                    .unwrap(),
                "{token} must answer nil"
            );
        }
        let err = s.run("UnitCreatureFamily(nil)").unwrap_err().to_string();
        assert!(
            err.contains(r#"Usage: UnitCreatureFamily("unit")"#),
            "{err}"
        );
    }

    #[test]
    fn get_pet_food_types_returns_one_value_per_diet() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);

        let _live = stats_loan(true, hunter_stats());
        assert_eq!(
            s.arity("GetPetFoodTypes()").unwrap(),
            6,
            "a boar's six diets are six returns, not one string"
        );
        assert!(s
            .eval::<bool>(
                "local a, b, c = GetPetFoodTypes() \
                 return a == 'Meat' and b == 'Fish' and c == 'Cheese'"
            )
            .unwrap());

        let _live = stats_loan(
            true,
            PetStats {
                food_types: vec![],
                ..hunter_stats()
            },
        );
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }

    /// The diet's hunter gate is the app's to apply, so here a non-hunter's diet is just empty.
    #[test]
    fn the_family_word_answers_for_a_non_hunter_pet() {
        let mut s = UiScript::new().unwrap();
        pet_tokens(&mut s);
        let _live = stats_loan(
            true,
            PetStats {
                hunter_pet: false,
                family: Some("Imp".into()),
                food_types: vec![],
                ..PetStats::default()
            },
        );
        assert_eq!(
            s.eval::<String>(r#"return UnitCreatureFamily("pet")"#)
                .unwrap(),
            "Imp"
        );
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }
}
