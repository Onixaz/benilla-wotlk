//! The stable-master verbs, reading the list handler's globals (`0x5a11a0`) at the call: index 0 the current pet, `1..=GetNumStablePets()` the stabled ones in wire order; a pet's family comes from its creature record (`0x5a13c1`), and `PickupStablePet` is cursor payload mode 10.

use mlua::{Lua, MultiValue, Value};

use super::live::{read_live, write_live};

/// The stable slots a hunter can buy, `BuyStableSlot`'s cap (`0x5a1bd0`).
pub const NUM_STABLE_SLOTS: usize = 4;

/// One stable record as the list handler stores it (`0x5a11a0`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StableRow {
    /// The pet's id, which the unstable and swap verbs name; 0 in the zeroed current record.
    pub pet_number: u32,
    /// The creature template, the record's key; 0 asks nothing.
    pub entry: u32,
    pub level: u32,
    /// The name the hunter gave the pet, not the creature template's.
    pub name: String,
}

/// The stable globals: the master `[0xc0f6c0]`, the bought slots `[0xc0f6cc]` and the records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StableList {
    /// The stable master's guid, 0 with none held.
    pub master: u64,
    /// Slots bought, 0..=4, not occupied.
    pub num_slots: u32,
    /// The current pet's record `[0xc0f480]`, zeroed by each list.
    pub current: StableRow,
    /// The stabled pets in wire order, Lua indices `1..`.
    pub stabled: Vec<StableRow>,
}

impl StableList {
    /// The record `GetStablePetInfo` and `PickupStablePet` pick (`0x5a1330`): below 1 the current pet, `1..=n` a stabled pet, past that none.
    fn pick(&self, i: i64) -> Option<&StableRow> {
        match i.saturating_sub(1) {
            ..0 => Some(&self.current),
            k => self.stabled(k),
        }
    }

    /// The stabled pet at 0-based index `k` (`0x50f500`).
    fn stabled(&self, k: i64) -> Option<&StableRow> {
        usize::try_from(k).ok().and_then(|k| self.stabled.get(k))
    }
}

/// A creature record's family parts, as the stable verbs read them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StableFamily {
    /// The `CreatureFamily.dbc` icon path.
    pub icon: String,
    /// The localized family word ("Wolf").
    pub name: String,
    /// The first `TalentTab.dbc` page admitting the family's talent type; `None` with no tree.
    pub talent_tree: Option<String>,
    /// The localized `ItemPetFood.dbc` names the family's food mask takes.
    pub diet: Vec<String>,
}

/// The event a creature record's answer fires for a stable verb that missed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StableCallback {
    /// `PET_STABLE_UPDATE` (`0x5a0e80`).
    Update,
    /// `PET_STABLE_UPDATE_PAPERDOLL` (`0x5a0ea0`).
    Paperdoll,
}

impl StableCallback {
    /// The event the answer fires.
    pub fn event(self) -> &'static str {
        match self {
            Self::Update => "PET_STABLE_UPDATE",
            Self::Paperdoll => "PET_STABLE_UPDATE_PAPERDOLL",
        }
    }
}

/// An outbound stable verb, sent to the held master inside the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StableIntent {
    /// `CMSG_STABLE_PET`: stable the current pet; the server picks the first free bought slot.
    Stable,
    /// `CMSG_UNSTABLE_PET`: summon this pet number.
    Unstable(u32),
    /// `CMSG_STABLE_SWAP_PET`: trade the current pet for this pet number in one step.
    Swap(u32),
    /// `CMSG_BUY_STABLE_SLOT`: buy the next slot.
    BuySlot,
}

/// The selection `[0xc0f6d0]`: -1 the current pet, 0 nothing, else a pet number.
#[derive(Debug, Default)]
pub(crate) struct StableModel {
    pub(crate) selected: i32,
}

impl super::UiScript {
    /// The selection reset of the list handler and the world entry (`0x5a11fa`, `0x528244`).
    pub fn reset_stable_selection(&mut self) {
        self.model_mut().stable.selected = 0;
    }

    /// `GetSelectedStablePet()`'s answer: 0 the current pet, `1..=n` a stabled pet, -1 nothing.
    pub fn stable_selection(&mut self) -> i32 {
        let selected = self.model_mut().stable.selected;
        selected_index(selected, &live_stable()) + 1
    }
}

/// The stable through the lent view; with none lent, the world entry's empty one.
fn live_stable() -> StableList {
    read_live(|l| l.stable()).unwrap_or_default()
}

/// `0x5a0e40`: -1 the current pet, the held pet number's 0-based stabled index, or -2 for nothing,
/// a pet no longer listed included.
fn selected_index(selected: i32, list: &StableList) -> i32 {
    match selected {
        -1 => -1,
        0 => -2,
        pet_number => list
            .stabled
            .iter()
            .position(|p| p.pet_number == pet_number as u32)
            .map_or(-2, |i| i as i32),
    }
}

/// `0x5a0e00`, 0-based index `k` held as the selection: -1 the current pet, a listed stabled pet by its number, anything else nothing.
fn select(k: i64, list: &StableList) -> i32 {
    match k {
        -1 => -1,
        k => list.stabled(k).map_or(0, |p| p.pet_number as i32),
    }
}

/// The verb a drop of the held pet (0-based cursor index `held`) onto Lua index `number` sends (`0x5a1ca0`): a stabled pet onto 0 swaps with a pet out, else unstables; the current pet onto a stabled pet swaps, onto a free bought slot stables. Anything else sends nothing.
fn drop_verb(held: i64, number: i64, list: &StableList, pet_out: bool) -> Option<StableIntent> {
    if held != -1 {
        if number != 0 {
            return None;
        }
        let pet = list.stabled(held).filter(|p| p.pet_number != 0)?;
        return Some(if pet_out {
            StableIntent::Swap(pet.pet_number)
        } else {
            StableIntent::Unstable(pet.pet_number)
        });
    }
    let index = number - 1;
    match list.stabled(index) {
        Some(p) if p.pet_number != 0 => Some(StableIntent::Swap(p.pet_number)),
        _ if (0..i64::from(list.num_slots)).contains(&index) => Some(StableIntent::Stable),
        _ => None,
    }
}

/// The family off `row`'s creature record, a miss asking it with `callback`, or asking nothing with none (`0x5a13c1`, `0x67b6d0`).
fn family_of(row: &StableRow, callback: Option<StableCallback>) -> Option<StableFamily> {
    // A zero entry is the cache's null key: no record and no ask.
    if row.entry == 0 {
        return None;
    }
    write_live(|l| l.stable_family(row.entry, callback)).flatten()
}

/// Send `verb` to the held master through the lent view.
fn send(verb: StableIntent) {
    read_live(|l| l.stable_send(verb));
}

fn str_value(lua: &Lua, s: &str) -> mlua::Result<Value> {
    Ok(Value::String(lua.create_string(s)?))
}

/// Register the stable globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // IsAtStableMaster() (`0x5a1090`): a Lua boolean, a stable master is held.
    g.set(
        "IsAtStableMaster",
        lua.create_function(|_, _: MultiValue| Ok(live_stable().master != 0))?,
    )?;

    // GetStablePetInfo(i) (`0x5a1330`) → icon, name, level, family, and the talent tree when the
    // family has one; no pet, or no creature record yet, answers nil, nil, 0.
    g.set(
        "GetStablePetInfo",
        lua.create_function(|lua, i: i64| {
            let list = live_stable();
            let Some((pet, family)) = list
                .pick(i)
                .and_then(|p| Some((p, family_of(p, Some(StableCallback::Update))?)))
            else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Number(0.0),
                ]));
            };
            let mut out = vec![
                str_value(lua, &family.icon)?,
                str_value(lua, &pet.name)?,
                Value::Number(f64::from(pet.level)),
                str_value(lua, &family.name)?,
            ];
            if let Some(tree) = &family.talent_tree {
                out.push(str_value(lua, tree)?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetStablePetFoodTypes(i) (`0x5a16a0`) → one localized diet name per value: 0 the current pet, `1..=n` a stabled pet, anything else none.
    g.set(
        "GetStablePetFoodTypes",
        lua.create_function(|lua, i: i64| {
            let list = live_stable();
            let pet = match i {
                0 => Some(&list.current),
                n => list.stabled(n - 1),
            };
            let diet = pet
                .and_then(|p| family_of(p, Some(StableCallback::Update)))
                .map(|f| f.diet)
                .unwrap_or_default();
            let mut out = Vec::with_capacity(diet.len());
            for d in &diet {
                out.push(str_value(lua, d)?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // GetNumStableSlots() (`0x5a0fa0`) → slots bought, 0..=4.
    g.set(
        "GetNumStableSlots",
        lua.create_function(|_, ()| Ok(i64::from(live_stable().num_slots)))?,
    )?;

    // GetNumStablePets() (`0x5a0f60`) → the stabled pets, the current one not counted.
    g.set(
        "GetNumStablePets",
        lua.create_function(|_, ()| Ok(live_stable().stabled.len() as i64))?,
    )?;

    // GetNextStableSlotCost() (`0x5a14d0`) → `StableSlotPrices.dbc` row `slots + 1`'s price, 0 past the table.
    g.set(
        "GetNextStableSlotCost",
        lua.create_function(|_, ()| {
            let slots = live_stable().num_slots;
            Ok(i64::from(
                read_live(|l| l.stable_slot_price(slots + 1))
                    .flatten()
                    .unwrap_or(0),
            ))
        })?,
    )?;

    // GetSelectedStablePet() (`0x5a1060`) → 0 the current pet, `1..=n` a stabled pet, -1 nothing.
    g.set(
        "GetSelectedStablePet",
        lua.create_function(|lua, ()| {
            let selected = crate::script::model_mut_of(lua).stable.selected;
            Ok(i64::from(selected_index(selected, &live_stable()) + 1))
        })?,
    )?;

    // ClickStablePet(i) (`0x5a1ca0`) → one value, keyed only on whether the cursor held a pet: a
    // plain click selects and answers 1; a drop answers nil, packet or not, and clears the cursor.
    g.set(
        "ClickStablePet",
        lua.create_function(|lua, i: i64| {
            let list = live_stable();
            let mut model = crate::script::model_mut_of(lua);
            let Some(super::cursor::CursorPayload::StablePet(held)) = model.cursor.clone() else {
                model.stable.selected = select(i - 1, &list);
                return Ok(Value::Number(1.0));
            };
            // The grab holds the 0-based index (`0x5a0fe0`).
            let held = i64::from(held.slot) - 1;
            let intent = (held != i - 1)
                .then(|| {
                    // The fork asks the pet list's head (`0x5a1ca0`).
                    let pet_out = read_live(|l| l.pet_out()).unwrap_or(false);
                    drop_verb(held, i, &list, pet_out)
                })
                .flatten();
            // The send goes out ahead of the clear (`0x5a1d88`, `0x5a1d9f`).
            if let Some(intent) = intent {
                send(intent);
            }
            super::cursor::clear_cursor(&mut model);
            Ok(Value::Nil)
        })?,
    )?;

    // PickupStablePet(i) (`0x5a0fe0`): a listed pet with a pet number goes to the cursor grab (`0x520c60`), which clears the cursor, then takes the pet only with its creature record cached, asking nothing on a miss (`0x520cc1`), and a family icon (`0x520cf2`); no return.
    g.set(
        "PickupStablePet",
        lua.create_function(|lua, i: i64| {
            let list = live_stable();
            let Some(pet) = list.pick(i).filter(|p| p.pet_number != 0) else {
                return Ok(());
            };
            let icon = family_of(pet, None)
                .map(|f| f.icon)
                .filter(|icon| !icon.is_empty());
            let mut model = crate::script::model_mut_of(lua);
            super::cursor::clear_cursor(&mut model);
            if let (Some(texture), Ok(slot)) = (icon, u8::try_from(i.max(0))) {
                // The grab writes the pet and signals nothing more (`0x520d08`).
                model.cursor = Some(super::cursor::CursorPayload::StablePet(
                    super::cursor::CursorStablePet { slot, texture },
                ));
            }
            Ok(())
        })?,
    )?;

    // StablePet() (`0x5a19c0`): sent with a master held or under an open-stable aura.
    g.set(
        "StablePet",
        lua.create_function(|_, ()| {
            let open =
                live_stable().master != 0 || read_live(|l| l.open_stable_aura()).unwrap_or(false);
            if open {
                send(StableIntent::Stable);
            }
            Ok(())
        })?,
    )?;

    // UnstablePet(i) (`0x5a1ac0`): silently refused while our charm or summon is set; its unsigned bound takes stabled pets only, master held or not.
    g.set(
        "UnstablePet",
        lua.create_function(|_, i: i64| {
            let barred = read_live(|l| l.charm_or_summon()).unwrap_or(true);
            let list = live_stable();
            if let Some(pet) = list.stabled(i - 1).filter(|_| !barred) {
                send(StableIntent::Unstable(pet.pet_number));
            }
            Ok(())
        })?,
    )?;

    // SetPetStablePaperdoll(model) (`0x5a1530`): the selection's creature record, the current pet's live body first (`0x5a15df`-`0x5a160a`), else the row's record, whose miss asks with `PET_STABLE_UPDATE_PAPERDOLL` (`0x5a1615`, `0x5a1657`). The pane follows the selection app-side.
    g.set(
        "SetPetStablePaperdoll",
        lua.create_function(|lua, model: Value| {
            let Value::Table(model) = model else {
                return Err(mlua::Error::runtime(
                    "Usage: SetPetStablePaperdoll(characterModel)",
                ));
            };
            if super::object::frame_handle_of(lua, &model).is_err() {
                return Err(mlua::Error::runtime(
                    "SetPetStablePaperdoll(): Couldn't find 'this' in frame object",
                ));
            }
            let list = live_stable();
            let selected = crate::script::model_mut_of(lua).stable.selected;
            let row = match selected_index(selected, &list) {
                -1 if read_live(|l| l.stable_live_pet()).unwrap_or(false) => None,
                -1 => Some(&list.current),
                k => list.stabled(i64::from(k)),
            };
            if let Some(row) = row {
                family_of(row, Some(StableCallback::Paperdoll));
            }
            Ok(())
        })?,
    )?;

    // BuyStableSlot() (`0x5a1bd0`): the client's silent gates, in order: a master held, fewer than 4 slots, a price row and affordability.
    g.set(
        "BuyStableSlot",
        lua.create_function(|lua, ()| {
            let list = live_stable();
            if list.master == 0 || list.num_slots as usize == NUM_STABLE_SLOTS {
                return Ok(());
            }
            let price = read_live(|l| l.stable_slot_price(list.num_slots + 1)).flatten();
            let money = crate::script::live::money(&crate::script::model_mut_of(lua));
            if price.is_some_and(|p| u64::from(p) <= money) {
                send(StableIntent::BuySlot);
            }
            Ok(())
        })?,
    )?;

    // ClosePetStables() (`0x5a1963`-`0x5a19aa`): no packet of its own; our open-stable aura is cancelled, a held master cleared and `PET_STABLE_CLOSED` fired twice inside the call.
    g.set(
        "ClosePetStables",
        lua.create_function(|lua, ()| {
            // A held master is the interact target the list set (`0x5a11d9`), so its clear runs the interaction close's stable arm, which fires first (`0x5a198a`, `0x513065`, `0x5a0f4c`), then the verb fires its own (`0x5a19aa`).
            if write_live(|l| l.close_stables()).unwrap_or(false) {
                super::tick::fire_event_into(lua, "PET_STABLE_CLOSED", Vec::new());
                super::tick::fire_event_into(lua, "PET_STABLE_CLOSED", Vec::new());
            }
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::live::TestView;
    use crate::script::UiScript;

    fn row(number: u32, entry: u32, name: &str, level: u32) -> StableRow {
        StableRow {
            pet_number: number,
            entry,
            level,
            name: name.into(),
        }
    }

    fn wolf() -> StableFamily {
        StableFamily {
            icon: "Interface\\Icons\\Ability_Hunter_Pet_Wolf".into(),
            name: "Wolf".into(),
            talent_tree: Some("Ferocity".into()),
            diet: vec!["Meat".into(), "Fish".into()],
        }
    }

    /// A hunter at master 0x42 with Rex out, Bruiser and Ghost stabled, three slots bought; entry 3 (Ghost) has no record yet.
    fn view() -> TestView {
        let v = TestView {
            pet_out: true,
            ..Default::default()
        };
        *v.stable.borrow_mut() = StableList {
            master: 0x42,
            num_slots: 3,
            current: row(7, 1, "Rex", 41),
            stabled: vec![row(8, 1, "Bruiser", 38), row(9, 3, "Ghost", 20)],
        };
        v.stable_families.borrow_mut().insert(1, wolf());
        v.stable_prices.borrow_mut().extend([(4, 50_000)]);
        v
    }

    #[test]
    fn the_read_surface_answers_the_reference_calls() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumStableSlots()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNumStablePets()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetNextStableSlotCost()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetSelectedStablePet()").unwrap(), -1);

        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
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
                s.eval::<String>("local _, n = GetStablePetInfo(-3) return n")
                    .unwrap(),
                "Rex",
                "below 0 picks the current record too"
            );
            assert_eq!(
                s.eval::<(String, String)>("return GetStablePetFoodTypes(1)")
                    .unwrap(),
                ("Meat".into(), "Fish".into())
            );
            for call in ["GetStablePetFoodTypes(3)", "GetStablePetFoodTypes(-1)"] {
                assert_eq!(s.arity(call).unwrap(), 0, "{call}");
            }
        });
    }

    /// A miss answers nil, nil, 0 and asks the record with the update callback, once per call, as the cache's non-unique ask appends one (`0x5a13c1`, `0x5a1738`).
    #[test]
    fn a_missed_record_answers_three_values_and_asks_once_per_call() {
        let s = UiScript::new().unwrap();
        let v = view();
        v.stable_families.borrow_mut().insert(
            1,
            StableFamily {
                talent_tree: None,
                ..wolf()
            },
        );
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(s.arity("GetStablePetInfo(2)").unwrap(), 3);
            assert_eq!(
                s.eval::<i64>("local _, _, l = GetStablePetInfo(2) return l")
                    .unwrap(),
                0,
                "the level is 0, not nil"
            );
            assert_eq!(s.arity("GetStablePetInfo(1)").unwrap(), 4, "no tree");
            assert_eq!(s.arity("GetStablePetInfo(3)").unwrap(), 3, "no pet");
            s.eval::<()>("GetStablePetFoodTypes(2)").unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(
                    *tv.stable_asks.borrow(),
                    vec![
                        (3, StableCallback::Update),
                        (3, StableCallback::Update),
                        (3, StableCallback::Update)
                    ]
                );
            });
        });
    }

    /// The zeroed current record answers as no pet and asks nothing: entry 0 is the null key.
    #[test]
    fn the_zeroed_current_record_asks_nothing() {
        let s = UiScript::new().unwrap();
        let v = view();
        v.stable.borrow_mut().current = StableRow::default();
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            assert_eq!(s.arity("GetStablePetInfo(0)").unwrap(), 3);
            s.eval::<()>("PickupStablePet(0)").unwrap();
            crate::script::live::test_view(|tv| {
                assert!(tv.stable_asks.borrow().is_empty());
            });
        });
    }

    #[test]
    fn every_plain_click_returns_truthy_and_selects() {
        let mut s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
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
            crate::script::live::test_view(|tv| {
                assert!(
                    tv.stable_sent.borrow().is_empty(),
                    "selecting sends nothing"
                );
            });
        });
    }

    /// The list's reset clears the selection, and a pet the list no longer holds reads as none.
    #[test]
    fn a_list_clears_the_selection_and_a_stale_pet_degrades() {
        let mut s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("ClickStablePet(2)").unwrap();
            assert_eq!(s.stable_selection(), 2);
            crate::script::live::test_view(|tv| {
                tv.stable.borrow_mut().stabled.pop();
            });
            assert_eq!(s.stable_selection(), -1, "Ghost left the list");
            s.eval::<()>("ClickStablePet(0)").unwrap();
            s.reset_stable_selection();
            assert_eq!(s.stable_selection(), -1, "a list clears the selection");
        });
    }

    #[test]
    fn every_drop_returns_falsy_even_when_it_sends() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            assert!(s
                .eval::<bool>("PickupStablePet(1) return ClickStablePet(0) == nil")
                .unwrap());
            crate::script::live::test_view(|tv| {
                assert_eq!(tv.take_sent(), vec![StableIntent::Swap(8)]);
            });

            assert!(s
                .eval::<bool>("PickupStablePet(1) return ClickStablePet(1) == nil")
                .unwrap());
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty());
            });
        });
    }

    /// The drop's send goes out before its cursor clear (`0x5a1d88`, `0x5a1d9f`): a send a `CURSOR_UPDATE` listener makes lands after it.
    #[test]
    fn a_drop_sends_before_it_clears_the_cursor() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("PickupStablePet(1)").unwrap();
            s.run(
                "local f = CreateFrame('Frame') f:RegisterEvent('CURSOR_UPDATE')              f:SetScript('OnEvent', function() StablePet() end)",
            )
            .unwrap();
            s.eval::<()>("ClickStablePet(0)").unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(
                    tv.take_sent(),
                    vec![StableIntent::Swap(8), StableIntent::Stable]
                );
            });
        });
    }

    #[test]
    fn the_current_pet_swaps_onto_a_pet_and_stables_into_a_bought_slot() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("PickupStablePet(0) ClickStablePet(2)")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(tv.take_sent(), vec![StableIntent::Swap(9)]);
            });

            // Index 3 is free and bought, index 4 unbought.
            s.eval::<()>("PickupStablePet(0) ClickStablePet(3)")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(tv.take_sent(), vec![StableIntent::Stable]);
            });
            s.eval::<()>("PickupStablePet(0) ClickStablePet(4)")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty(), "slot 4 is not bought");
            });
            assert!(s.eval::<bool>("return ClickStablePet(1) and true").unwrap());
        });
    }

    #[test]
    fn the_fork_follows_the_pet_list_not_the_record() {
        let s = UiScript::new().unwrap();
        let ghost = || {
            let v = view();
            v.stable_families.borrow_mut().insert(3, wolf());
            v
        };
        let mut v = ghost();
        {
            let mut loan_v = ghost();
            crate::script::live::lending(&mut loan_v, || {
                s.eval::<()>("PickupStablePet(2) ClickStablePet(0)")
                    .unwrap();
                crate::script::live::test_view(|tv| {
                    assert_eq!(tv.take_sent(), vec![StableIntent::Swap(9)]);
                });
            });
        }
        v.pet_out = false;
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("PickupStablePet(2) ClickStablePet(0)")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(
                    tv.take_sent(),
                    vec![StableIntent::Unstable(9)],
                    "a record with no pet out unstables"
                );
            });
        });
    }

    /// The grab takes a pet only with its record cached: a miss clears the cursor, asks nothing and grabs nothing (`0x520c6b`, `0x520cc1`, `0x520cc8`).
    #[test]
    fn a_grab_without_the_record_clears_the_cursor_and_asks_nothing() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("PickupStablePet(1)").unwrap();
            assert!(s.model_mut().cursor.is_some(), "Bruiser's record is cached");
            s.eval::<()>("PickupStablePet(2)").unwrap();
            assert!(
                s.model_mut().cursor.is_none(),
                "Ghost's miss leaves it empty"
            );
            crate::script::live::test_view(|tv| {
                assert!(tv.stable_asks.borrow().is_empty());
            });
            assert!(s.eval::<bool>("return ClickStablePet(0) and true").unwrap());
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty());
            });
        });
    }

    #[test]
    fn stable_to_stable_is_a_no_op_and_an_empty_index_does_not_grab() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("PickupStablePet(1) ClickStablePet(2)")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty());
            });

            s.eval::<()>("PickupStablePet(3)").unwrap();
            assert!(s.eval::<bool>("return ClickStablePet(0) and true").unwrap());
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty());
            });
        });
    }

    #[test]
    fn buy_stable_slot_gates_locally_and_silently() {
        let mut s = UiScript::new().unwrap();
        s.set_money(1_000_000);
        let v = view();
        {
            let mut loan_v = view();
            crate::script::live::lending(&mut loan_v, || {
                s.eval::<()>("BuyStableSlot()").unwrap();
                crate::script::live::test_view(|tv| {
                    assert_eq!(tv.take_sent(), vec![StableIntent::BuySlot]);
                });
                s.set_money(10);
                s.eval::<()>("BuyStableSlot()").unwrap();
                crate::script::live::test_view(|tv| {
                    assert!(tv.take_sent().is_empty(), "unaffordable is silent");
                });
                s.set_money(1_000_000);
                crate::script::live::test_view(|tv| {
                    tv.stable.borrow_mut().num_slots = 4;
                });
                s.eval::<()>("BuyStableSlot()").unwrap();
                crate::script::live::test_view(|tv| {
                    assert!(tv.take_sent().is_empty(), "the cap of 4 is silent");
                });
            });
        }
        v.stable.borrow_mut().master = 0;
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("BuyStableSlot()").unwrap();
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty(), "no master is silent");
            });
        });
    }

    /// `UnstablePet` gates on our charm and summon, not the pet list, and needs no master.
    #[test]
    fn the_unstable_binding_gates_on_our_charm_and_summon() {
        let s = UiScript::new().unwrap();
        let v = view();
        v.stable.borrow_mut().master = 0;
        let mut barred = view();
        barred.charm_or_summon = true;
        {
            let mut loan_v = barred;
            crate::script::live::lending(&mut loan_v, || {
                s.eval::<()>("UnstablePet(1)").unwrap();
                crate::script::live::test_view(|tv| {
                    assert!(tv.take_sent().is_empty(), "a pet is summoned");
                });
            });
        }
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("UnstablePet(0)").unwrap();
            crate::script::live::test_view(|tv| {
                assert!(tv.take_sent().is_empty(), "index 0 is rejected");
            });
            s.eval::<()>("UnstablePet(2)").unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(tv.take_sent(), vec![StableIntent::Unstable(9)]);
            });
        });
    }

    /// `StablePet` sends with a master held, or with none under an open-stable aura.
    #[test]
    fn stable_pet_takes_a_master_or_the_open_stable_aura() {
        let s = UiScript::new().unwrap();
        let v = view();
        v.stable.borrow_mut().master = 0;
        {
            let mut loan_v = v;
            crate::script::live::lending(&mut loan_v, || {
                s.eval::<()>("StablePet()").unwrap();
                crate::script::live::test_view(|tv| {
                    assert!(tv.take_sent().is_empty());
                });
            });
        }
        let mut aura = view();
        aura.open_stable_aura = true;
        aura.stable.borrow_mut().master = 0;
        let mut loan_v = aura;
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("StablePet()").unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(tv.take_sent(), vec![StableIntent::Stable]);
            });
        });
    }

    /// The close clears the master and fires `PET_STABLE_CLOSED` inside the call, the interaction close's then its own, so the next read in the same chunk sees it gone; with none held it fires nothing.
    #[test]
    fn the_close_fires_inside_the_call_and_leaves_the_cursor_alone() {
        let s = UiScript::new().unwrap();
        let mut loan_v = view();
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>(
                "seen = {} local f = CreateFrame('Frame') f:RegisterEvent('PET_STABLE_CLOSED')
             f:SetScript('OnEvent', function() seen[#seen + 1] = tostring(IsAtStableMaster()) end)
             PickupStablePet(1)",
            )
            .unwrap();
            assert_eq!(
            s.eval::<String>("ClosePetStables() local at = tostring(IsAtStableMaster()) ClosePetStables() return table.concat(seen, ',') .. '|' .. at")
                .unwrap(),
            "false,false|false"
        );
            assert_eq!(
                s.eval::<i64>("return GetNumStablePets()").unwrap(),
                2,
                "the list outlives the close"
            );
            assert!(s.model_mut().cursor.is_some(), "the held pet stays");
        });
    }

    /// The setter asks the selection's record with the paperdoll callback: the current pet's only without its live body, a stabled pet's always, nothing selected nothing (`0x5a1530`).
    #[test]
    fn the_paperdoll_setter_asks_the_selected_record() {
        let s = UiScript::new().unwrap();
        let err = s.eval::<()>("SetPetStablePaperdoll(nil)").unwrap_err();
        assert!(err.to_string().contains("Usage: SetPetStablePaperdoll"));
        let mut v = view();
        v.stable_live_pet = true;
        v.stable.borrow_mut().current.entry = 3;
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            let run = |pick: &str| {
                s.eval::<()>(&format!(
                    "{pick} SetPetStablePaperdoll(CreateFrame('PlayerModel'))"
                ))
                .unwrap();
                crate::script::live::test_view(|tv| {
                    std::mem::take(&mut *tv.stable_asks.borrow_mut())
                })
            };
            assert!(run("").is_empty(), "nothing selected");
            assert!(run("ClickStablePet(0)").is_empty(), "the live pet");
            assert_eq!(
                run("ClickStablePet(2)"),
                vec![(3, StableCallback::Paperdoll)]
            );
            assert!(run("ClickStablePet(1)").is_empty(), "a cached record");
        });

        let v = view();
        v.stable.borrow_mut().current.entry = 3;
        let mut loan_v = v;
        crate::script::live::lending(&mut loan_v, || {
            s.eval::<()>("ClickStablePet(0) SetPetStablePaperdoll(CreateFrame('PlayerModel'))")
                .unwrap();
            crate::script::live::test_view(|tv| {
                assert_eq!(
                    *tv.stable_asks.borrow(),
                    vec![(3, StableCallback::Paperdoll)],
                    "no live body: the current row's record"
                );
            });
        });
    }
}
