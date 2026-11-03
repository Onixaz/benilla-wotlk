//! The stable-master bindings: the app pushes a snapshot of the hunter's stable
//! ([`UiScript::set_stable`], every wire row resolved to strings) and the Lua's clicks queue verbs
//! the app drains.
//!
//! The list holds two things, as 12340's `0x5a11a0` stores them: the current pet's record, index
//! 0, and the stabled pets packed in wire order, indices `1..=GetNumStablePets()`. Up to
//! `NUM_PET_STABLE_SLOTS` (4) stable slots are bought (`0x5a1bd0` stops at 4). The current record
//! can come from a dismissed or distant pet, and the stock window falls back to
//! `GetStablePetInfo(0)` when `UnitExists("pet")` is false.
//!
//! `PickupStablePet` puts the pet on the global cursor as payload mode 10
//! ([`super::cursor::CursorPayload::StablePet`]).

use mlua::{Lua, MultiValue, Value};

/// The stable slots a hunter can buy, `BuyStableSlot`'s cap (`0x5a1bd0`).
pub const NUM_STABLE_SLOTS: usize = 4;

/// One stable row, every wire field resolved to what the Lua renders.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StablePetSlot {
    /// The pet's id, which the unstable and swap verbs name.
    pub pet_number: u32,
    /// The family icon path; with [`Self::family`], `None` while the creature query is in flight.
    pub icon: Option<String>,
    /// The name the hunter gave the pet, not the creature template's.
    pub name: String,
    pub level: u32,
    /// The localized `CreatureFamily.dbc` word ("Wolf"), `None` until the creature query lands;
    /// without it `GetStablePetInfo` answers as for no pet (`0x5a14a2`).
    pub family: Option<String>,
    /// The first `TalentTab.dbc` page admitting the family's talent type ("Ferocity"), the fifth
    /// `GetStablePetInfo` value; `None` for a family with no tree.
    pub talent_tree: Option<String>,
    /// The localized food names the pet's family eats: `GetStablePetFoodTypes`'s returns.
    pub diet: Vec<String>,
}

/// The open stable's snapshot, pushed whole; `None` when no stable is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StableState {
    /// Stable slots bought, 0..=4 (the wire's `numStableSlots`), not occupied: it enables buttons
    /// `1..=n` and prices the next slot.
    pub num_stable_slots: u32,
    /// The next slot's price in copper (`StableSlotPrices.dbc` row `num_stable_slots + 1`), 0 past
    /// the table, where the stock window has hidden the purchase row.
    pub next_slot_cost: u32,
    /// The current pet's record, index 0: the list row whose flags lack bit `0x2`.
    pub current: Option<StablePetSlot>,
    /// The stabled pets in wire order, indices `1..`.
    pub stabled: Vec<StablePetSlot>,
    /// A live pet is out: the client's pet-guid test, which forks a stabled pet's drop between swap
    /// and unstable. Not `current.is_some()`: a dismissed or distant pet keeps its record with no
    /// live guid.
    pub has_live_pet: bool,
}

impl StableState {
    /// The record at a Lua index, as `0x5a1330` picks it: 0 and below the current pet, `1..=n` a
    /// stabled pet, past that nothing.
    fn pet(&self, i: i64) -> Option<&StablePetSlot> {
        match i.saturating_sub(1) {
            ..0 => self.current.as_ref(),
            k => usize::try_from(k).ok().and_then(|k| self.stabled.get(k)),
        }
    }
}

/// An outbound stable verb, drained by the app, which addresses it to the open stable master.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StableIntent {
    /// `CMSG_STABLE_PET`: stable the current pet; the server picks the first free bought slot.
    Stable,
    /// `CMSG_UNSTABLE_PET`: summon this pet number, valid only with no current pet.
    Unstable(u32),
    /// `CMSG_STABLE_SWAP_PET`: trade the current pet for this pet number in one step.
    Swap(u32),
    /// `CMSG_BUY_STABLE_SLOT`: buy the next slot.
    BuySlot,
}

/// The window's own state beside the snapshot: the selection, the queued verbs and the close flag.
#[derive(Debug)]
pub(crate) struct StableModel {
    pub(crate) state: Option<StableState>,
    /// The selection as the client holds it (`[0xc0f6d0]`): -1 the current pet, 0 nothing, else a
    /// pet number, so a pet no longer listed reads back as nothing.
    pub(crate) selected: i32,
    pub(crate) intents: Vec<StableIntent>,
    pub(crate) close: bool,
}

/// Nothing selected, the client's reset; Lua reads it as -1 through
/// [`super::UiScript::stable_selection`].
impl Default for StableModel {
    fn default() -> Self {
        Self {
            state: None,
            selected: 0,
            intents: Vec::new(),
            close: false,
        }
    }
}

impl super::UiScript {
    /// Push the open stable's snapshot, or clear it with `None`. Every push clears the selection,
    /// as each list does in the client (`0x5a11fa`); the stock window then re-picks one.
    pub fn set_stable(&mut self, state: Option<StableState>) {
        let mut model = self.model_mut();
        model.stable.selected = 0;
        // A held pet stays on the cursor: the reference's close clears only the stable-master guid.
        model.stable.state = state;
    }

    /// Drain the queued stable verbs.
    pub fn take_stable_intents(&mut self) -> Vec<StableIntent> {
        drain_field!(self, stable.intents)
    }

    /// Whether `ClosePetStables()` was called since the last drain; no packet exists for it.
    pub fn take_stable_close(&mut self) -> bool {
        drain_field!(self, stable.close)
    }

    /// `GetSelectedStablePet()`'s answer: 0 the current pet, `1..=n` a stabled pet, -1 nothing.
    pub fn stable_selection(&mut self) -> i32 {
        let model = self.model_mut();
        selected_index(&model.stable) + 1
    }
}

/// `0x5a0e40`: -1 the current pet, the held pet number's 0-based stabled index, or -2 for nothing,
/// a pet no longer listed included.
fn selected_index(m: &StableModel) -> i32 {
    match m.selected {
        -1 => -1,
        0 => -2,
        pet_number => m
            .state
            .as_ref()
            .and_then(|s| {
                s.stabled
                    .iter()
                    .position(|p| i64::from(p.pet_number) == i64::from(pet_number))
            })
            .map_or(-2, |i| i as i32),
    }
}

/// `0x5a0e00`, Lua index `i` held as the selection: 0 the current pet, a listed stabled pet by its
/// number, anything else nothing.
fn select(m: &mut StableModel, i: i64) {
    m.selected = match i.saturating_sub(1) {
        -1 => -1,
        k => usize::try_from(k)
            .ok()
            .and_then(|k| m.state.as_ref()?.stabled.get(k))
            .map_or(0, |p| p.pet_number as i32),
    };
}

/// The verb a drop of the held pet (Lua index `held`) onto index `to` sends (`0x5a1ca0`): a
/// stabled pet onto 0 swaps with a live pet out, else unstables; the current pet onto a stabled
/// pet swaps, onto a free bought slot stables. Anything else sends nothing.
fn drop_verb(held: i64, to: i64, state: &StableState) -> Option<StableIntent> {
    if held >= 1 {
        if to != 0 {
            return None;
        }
        let pet = state.pet(held).filter(|p| p.pet_number != 0)?;
        return Some(if state.has_live_pet {
            StableIntent::Swap(pet.pet_number)
        } else {
            StableIntent::Unstable(pet.pet_number)
        });
    }
    match state.pet(to).filter(|_| to >= 1) {
        Some(p) if p.pet_number != 0 => Some(StableIntent::Swap(p.pet_number)),
        _ if (1..=i64::from(state.num_stable_slots)).contains(&to) => Some(StableIntent::Stable),
        _ => None,
    }
}

/// Register the stable globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // IsAtStableMaster() (`0x5a1090`): a Lua boolean, the stable master's guid `[0xc0f6c0]` is
    // set; the app pushes a snapshot exactly while one is.
    g.set(
        "IsAtStableMaster",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.stable.state.is_some())
        })?,
    )?;

    // GetStablePetInfo(i) (`0x5a1330`) → icon, name, level, family, and the talent tree when the
    // family has one; no pet, or no creature record yet, answers nil, nil, 0.
    g.set(
        "GetStablePetInfo",
        lua.create_function(|lua, i: i64| {
            let pet = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model.stable.state.as_ref().and_then(|s| s.pet(i).cloned())
            };
            let Some((pet, family)) = pet.and_then(|p| {
                let family = p.family.clone()?;
                Some((p, family))
            }) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                ]));
            };
            let mut out = vec![
                Value::String(lua.create_string(pet.icon.as_deref().unwrap_or_default())?),
                Value::String(lua.create_string(&pet.name)?),
                Value::Number(f64::from(pet.level)),
                Value::String(lua.create_string(&family)?),
            ];
            if let Some(tree) = &pet.talent_tree {
                out.push(Value::String(lua.create_string(tree)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetStablePetFoodTypes(i) (`0x5a16a0`) → one localized diet name per value; none past the list
    // or for a family with no diet.
    g.set(
        "GetStablePetFoodTypes",
        lua.create_function(|lua, i: i64| {
            let diet = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                model
                    .stable
                    .state
                    .as_ref()
                    .and_then(|s| s.pet(i))
                    .map(|p| p.diet.clone())
                    .unwrap_or_default()
            };
            let mut out = Vec::with_capacity(diet.len());
            for d in &diet {
                out.push(Value::String(lua.create_string(d)?));
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetNumStableSlots() → slots bought, 0..=4.
    g.set(
        "GetNumStableSlots",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(
                model
                    .stable
                    .state
                    .as_ref()
                    .map_or(0, |s| s.num_stable_slots),
            ))
        })?,
    )?;

    // GetNumStablePets() (`0x5a0f60`) → the stabled pets, the current one not counted.
    g.set(
        "GetNumStablePets",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(model.stable.state.as_ref().map_or(0, |s| s.stabled.len()) as i64)
        })?,
    )?;

    // GetNextStableSlotCost() → the next slot's price in copper, 0 with no stable open or past the
    // table.
    g.set(
        "GetNextStableSlotCost",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(
                model.stable.state.as_ref().map_or(0, |s| s.next_slot_cost),
            ))
        })?,
    )?;

    // GetSelectedStablePet() (`0x5a1060`) → 0 the current pet, `1..=n` a stabled pet, -1 nothing.
    g.set(
        "GetSelectedStablePet",
        lua.create_function(|lua, ()| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(i64::from(selected_index(&model.stable) + 1))
        })?,
    )?;

    // ClickStablePet(i) (`0x5a1ca0`) → one value, keyed only on whether the cursor held a pet: a
    // plain click selects and answers 1; a drop answers nil, packet or not, and clears the cursor.
    g.set(
        "ClickStablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let Some(super::cursor::CursorPayload::StablePet(held)) = model.cursor.clone() else {
                select(&mut model.stable, i);
                return Ok(Value::Number(1.0));
            };
            let held = i64::from(held.slot);
            let intent = (held != i)
                .then(|| {
                    model
                        .stable
                        .state
                        .as_ref()
                        .and_then(|state| drop_verb(held, i, state))
                })
                .flatten();
            super::cursor::clear_cursor(&mut model);
            if let Some(intent) = intent {
                model.stable.intents.push(intent);
            }
            Ok(Value::Nil)
        })?,
    )?;

    // PickupStablePet(i) (`0x5a0fe0`): the mode-10 cursor grab of any listed pet with a pet number,
    // returning nothing.
    g.set(
        "PickupStablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let texture = model
                .stable
                .state
                .as_ref()
                .and_then(|s| s.pet(i))
                .filter(|p| p.pet_number != 0)
                .map(|p| p.icon.clone().unwrap_or_default());
            if let (Some(texture), Ok(slot)) = (texture, u8::try_from(i.max(0))) {
                model.cursor = Some(super::cursor::CursorPayload::StablePet(
                    super::cursor::CursorStablePet { slot, texture },
                ));
                super::cursor::queue_cursor_update(&mut model);
            }
            Ok(())
        })?,
    )?;

    // StablePet() (`0x5a19c0`): no arguments, gated only on an open stable; stock FrameXML stables
    // by drag.
    g.set(
        "StablePet",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            if model.stable.state.is_some() {
                model.stable.intents.push(StableIntent::Stable);
            }
            Ok(())
        })?,
    )?;

    // UnstablePet(i) (`0x5a1ac0`): silently refused with a pet out, a gate the drag path lacks; its
    // unsigned bound takes stabled pets only. The reference's charm gate is not built.
    g.set(
        "UnstablePet",
        lua.create_function(|lua, i: i64| {
            let mut model = crate::script::model_mut_of(lua);
            let pet_number = model
                .stable
                .state
                .as_ref()
                .filter(|s| !s.has_live_pet && i >= 1)
                .and_then(|s| s.pet(i))
                .map(|p| p.pet_number);
            if let Some(pet_number) = pet_number {
                model
                    .stable
                    .intents
                    .push(StableIntent::Unstable(pet_number));
            }
            Ok(())
        })?,
    )?;

    // Deviation: SetPetStablePaperdoll(model) is inert, because the model pane is an app-side
    // booth that follows the selection every frame, with no VM-side unit to point.
    g.set(
        "SetPetStablePaperdoll",
        lua.create_function(|_, _model: Value| Ok(()))?,
    )?;

    // BuyStableSlot() (`0x5a1bd0`): the client's silent gates, in order: an open stable, fewer than
    // 4 slots, a price row and affordability.
    g.set(
        "BuyStableSlot",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            // Affordability reads `model.money`, the purse `GetMoney()` answers.
            let money = model.money;
            let allowed = model.stable.state.as_ref().is_some_and(|s| {
                s.num_stable_slots as usize != NUM_STABLE_SLOTS
                    && s.next_slot_cost != 0
                    && u64::from(s.next_slot_cost) <= money
            });
            if allowed {
                model.stable.intents.push(StableIntent::BuySlot);
            }
            Ok(())
        })?,
    )?;

    // ClosePetStables(): no packet exists; the flag tells the app to clear its session.
    g.set(
        "ClosePetStables",
        lua.create_function(|lua, ()| {
            let mut model = crate::script::model_mut_of(lua);
            model.stable.close = true;
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn pet(number: u32, name: &str, level: u32) -> StablePetSlot {
        StablePetSlot {
            pet_number: number,
            icon: Some("Interface\\Icons\\Ability_Hunter_Pet_Wolf".into()),
            name: name.into(),
            level,
            family: Some("Wolf".into()),
            talent_tree: Some("Ferocity".into()),
            diet: vec!["Meat".into(), "Fish".into()],
        }
    }

    /// A hunter with Rex out, Bruiser and Ghost stabled, three slots bought.
    fn state() -> StableState {
        StableState {
            num_stable_slots: 3,
            next_slot_cost: 50_000,
            current: Some(pet(7, "Rex", 41)),
            stabled: vec![pet(8, "Bruiser", 38), pet(9, "Ghost", 20)],
            has_live_pet: true,
        }
    }

    fn open(s: &mut UiScript) {
        s.set_stable(Some(state()));
    }

    #[test]
    fn the_read_surface_answers_the_reference_calls() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumStableSlots()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNumStablePets()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNextStableSlotCost()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetSelectedStablePet()").unwrap(), -1);

        open(&mut s);
        assert_eq!(s.eval::<i64>("return GetNumStableSlots()").unwrap(), 3);
        assert_eq!(
            s.eval::<i64>("return GetNumStablePets()").unwrap(),
            2,
            "the current pet is not counted"
        );
        assert_eq!(
            s.eval::<i64>("return GetNextStableSlotCost()").unwrap(),
            50_000
        );
        assert_eq!(
            s.eval::<(String, String, i64, String, String)>("return GetStablePetInfo(1)")
                .unwrap(),
            (
                "Interface\\Icons\\Ability_Hunter_Pet_Wolf".into(),
                "Bruiser".into(),
                38,
                "Wolf".into(),
                "Ferocity".into()
            )
        );
        assert_eq!(
            s.eval::<String>("local _, n = GetStablePetInfo(0) return n")
                .unwrap(),
            "Rex",
            "index 0 is the current pet's record"
        );
        assert_eq!(
            s.eval::<String>("local _, n = GetStablePetInfo(2) return n")
                .unwrap(),
            "Ghost",
            "stabled pets are packed in wire order"
        );
        assert_eq!(
            s.eval::<(String, String)>("return GetStablePetFoodTypes(1)")
                .unwrap(),
            ("Meat".into(), "Fish".into())
        );
        assert!(s
            .eval::<bool>("return GetStablePetFoodTypes(3) == nil")
            .unwrap());
    }

    /// No pet, or no creature record yet, answers three values; a family with no tree four.
    #[test]
    fn the_answer_arity_follows_the_record() {
        let mut s = UiScript::new().unwrap();
        let mut st = state();
        st.stabled[1].family = None;
        st.stabled[0].talent_tree = None;
        s.set_stable(Some(st));
        for call in ["GetStablePetInfo(3)", "GetStablePetInfo(2)"] {
            assert_eq!(s.arity(call).unwrap(), 3, "{call} arity");
            assert!(s.eval::<bool>(&format!("return not {call}")).unwrap());
            assert_eq!(
                s.eval::<i64>(&format!("local _, _, l = {call} return l"))
                    .unwrap(),
                0,
                "{call} level is 0, not nil"
            );
        }
        assert_eq!(s.arity("GetStablePetInfo(1)").unwrap(), 4);
        assert_eq!(s.arity("GetStablePetInfo(0)").unwrap(), 5);

        s.set_stable(Some(StableState {
            current: None,
            ..state()
        }));
        assert_eq!(
            s.arity("GetStablePetInfo(0)").unwrap(),
            3,
            "no current row: the empty record"
        );
    }

    #[test]
    fn every_plain_click_returns_truthy_and_selects() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        for (call, selected) in [
            ("ClickStablePet(1)", 1),
            ("ClickStablePet(0)", 0),
            ("ClickStablePet(2)", 2),
            ("ClickStablePet(3)", -1),
            ("ClickStablePet(-4)", -1),
        ] {
            assert!(
                s.eval::<bool>(&format!("return {call} and true or false"))
                    .unwrap(),
                "{call}"
            );
            assert_eq!(s.stable_selection(), selected, "{call} selects");
        }
        assert!(
            s.take_stable_intents().is_empty(),
            "selecting sends nothing"
        );
    }

    #[test]
    fn a_list_clears_the_selection_and_a_stale_pet_degrades() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("ClickStablePet(2)").unwrap();
        assert_eq!(s.stable_selection(), 2);
        open(&mut s);
        assert_eq!(s.stable_selection(), -1, "a list clears the selection");
    }

    #[test]
    fn every_drop_returns_falsy_even_when_it_sends() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        assert!(s
            .eval::<bool>("PickupStablePet(1) return ClickStablePet(0) == nil")
            .unwrap());
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(8)]);

        assert!(s
            .eval::<bool>("PickupStablePet(1) return ClickStablePet(1) == nil")
            .unwrap());
        assert!(s.take_stable_intents().is_empty());
    }

    #[test]
    fn the_current_pet_swaps_onto_a_pet_and_stables_into_a_bought_slot() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("PickupStablePet(0) ClickStablePet(2)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(9)]);

        // Index 3 is free and bought, index 4 unbought.
        s.eval::<()>("PickupStablePet(0) ClickStablePet(3)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Stable]);
        s.eval::<()>("PickupStablePet(0) ClickStablePet(4)")
            .unwrap();
        assert!(s.take_stable_intents().is_empty(), "slot 4 is not bought");
        assert!(s.eval::<bool>("return ClickStablePet(1) and true").unwrap());
    }

    #[test]
    fn the_fork_follows_the_live_pet_not_the_record() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("PickupStablePet(2) ClickStablePet(0)")
            .unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Swap(9)]);

        s.set_stable(Some(StableState {
            has_live_pet: false,
            ..state()
        }));
        s.eval::<()>("PickupStablePet(2) ClickStablePet(0)")
            .unwrap();
        assert_eq!(
            s.take_stable_intents(),
            vec![StableIntent::Unstable(9)],
            "a record without a live guid unstables"
        );
    }

    #[test]
    fn stable_to_stable_is_a_no_op_and_an_empty_index_does_not_grab() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("PickupStablePet(1) ClickStablePet(2)")
            .unwrap();
        assert!(s.take_stable_intents().is_empty());

        s.eval::<()>("PickupStablePet(3)").unwrap();
        assert!(s.eval::<bool>("return ClickStablePet(0) and true").unwrap());
        assert!(s.take_stable_intents().is_empty());
    }

    #[test]
    fn buy_stable_slot_gates_locally_and_silently() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000_000);
        open(&mut s);
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::BuySlot]);

        s.set_money(10);
        open(&mut s);
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert!(s.take_stable_intents().is_empty(), "unaffordable is silent");

        s.set_money(1_000_000);
        s.set_stable(Some(StableState {
            num_stable_slots: 4,
            ..state()
        }));
        s.eval::<()>("BuyStableSlot()").unwrap();
        assert!(s.take_stable_intents().is_empty(), "the cap of 4 is silent");
    }

    #[test]
    fn the_unstable_binding_gates_where_the_drag_path_does_not() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        s.eval::<()>("UnstablePet(1)").unwrap();
        assert!(s.take_stable_intents().is_empty(), "a pet is out — silent");

        s.set_stable(Some(StableState {
            has_live_pet: false,
            ..state()
        }));
        s.eval::<()>("UnstablePet(0)").unwrap();
        assert!(s.take_stable_intents().is_empty(), "index 0 is rejected");
        s.eval::<()>("UnstablePet(2)").unwrap();
        assert_eq!(s.take_stable_intents(), vec![StableIntent::Unstable(9)]);
    }

    #[test]
    fn close_drains_and_leaves_the_cursor_alone() {
        let mut s = UiScript::new().unwrap();
        open(&mut s);
        assert!(!s.take_stable_close());
        s.eval::<()>("ClosePetStables()").unwrap();
        assert!(s.take_stable_close());
        assert!(!s.take_stable_close(), "drain clears");
    }

    #[test]
    fn the_paperdoll_setter_is_callable() {
        let s = UiScript::new().unwrap();
        s.eval::<()>("SetPetStablePaperdoll(nil)").unwrap();
    }
}
