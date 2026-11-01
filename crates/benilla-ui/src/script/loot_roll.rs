//! The group loot roll bindings `GroupLootFrame` calls: the app pushes the open rolls and drains
//! the votes `RollOnLoot` queues, 0 Pass, 1 Need, 2 Greed, 3 Disenchant. `START_LOOT_ROLL` carries
//! `(rollID, rollTime)` and `CANCEL_LOOT_ROLL` the `rollID`. The `rollID` is client-internal, as
//! `CMSG_LOOT_ROLL` addresses `(lootedTarget, itemSlot)`, so the app allocates its own.

use mlua::{Lua, Value};

use super::Model;

/// `rollType` 3, whose confirm is `CONFIRM_DISENCHANT_ROLL` on a rare or better item (`0x6fbc70`).
const DISENCHANT: i64 = 3;

/// The quality from which a Disenchant vote asks first (`0x6fbc70`: `quality >= 3`).
const DISENCHANT_CONFIRM_QUALITY: u32 = 3;

/// One open group loot roll, resolved by the app and pushed whole each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollEntry {
    /// The client-internal `rollID`, allocated by the app and stable for the roll's life.
    pub roll_id: u32,
    /// `None` while the item template is in flight, when `GetLootRollItemInfo` answers nothing.
    pub name: Option<String>,
    /// Icon texture path (`Interface\Icons\…`).
    pub texture: Option<String>,
    /// The stack size, `SMSG_LOOT_START_ROLL`'s count.
    pub quantity: u32,
    /// Item quality 0..7; `None` while the template is in flight.
    pub quality: Option<u32>,
    /// Bind on pickup: the gold backdrop and the `CONFIRM_LOOT_ROLL` gate.
    pub bind_on_pickup: bool,
    /// The start's vote mask, bit `1 << rollType`; `RollOnLoot` sends only a vote it allows.
    pub vote_mask: u8,
    /// Why Need, Greed and Disenchant are refused, 0 when allowed (`0x6fa7a0`), the suffix of
    /// `LOOT_ROLL_INELIGIBLE_REASON<n>`.
    pub reasons: [u8; 3],
    /// `RequiredDisenchantSkill`, -1 when the item cannot be disenchanted.
    pub disenchant_skill: i32,
    /// Milliseconds left, from `SMSG_LOOT_START_ROLL`'s countdown; saturates at 0.
    pub time_left_ms: u32,
    /// The item id, the key of the hover's item lookup.
    pub item_id: u32,
    /// The link `GetLootRollItemLink` answers; `None` until the item template lands.
    pub link: Option<String>,
    /// The drop's random property, the hover's enchant source.
    pub random_property_id: u32,
}

/// Every open group loot roll, in the order the app opened them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollsState {
    pub rolls: Vec<LootRollEntry>,
}

impl super::UiScript {
    /// Push the open-rolls snapshot (an empty list means no roll is open).
    pub fn set_loot_rolls(&mut self, state: LootRollsState) {
        set_field!(self, loot_rolls, state);
    }

    /// Drain the `(roll_id, roll_type)` votes queued by `RollOnLoot` since the last call. The app
    /// maps each roll id back to its `(lootedTarget, itemSlot)` for `CMSG_LOOT_ROLL`.
    pub fn take_loot_roll_votes(&mut self) -> Vec<(u32, u8)> {
        drain_field!(self, loot_roll_votes)
    }

    /// Drain the `(roll_id, roll_type)` calls that ask first: Need or Greed on a bind-on-pickup
    /// roll (`CONFIRM_LOOT_ROLL`) and Disenchant on a rare or better item
    /// (`CONFIRM_DISENCHANT_ROLL`); the popup's `ConfirmLootRoll` queues the real vote.
    pub fn take_loot_roll_confirms(&mut self) -> Vec<(u32, u8)> {
        drain_field!(self, loot_roll_confirms)
    }
}

fn find(model: &Model, roll_id: i64) -> Option<&LootRollEntry> {
    model
        .loot_rolls
        .rolls
        .iter()
        .find(|r| i64::from(r.roll_id) == roll_id)
}

/// `0x6fbc70`: a vote other than Pass asks first unless confirmed, by the cached template, then
/// goes out only when the vote mask allows it.
fn roll(lua: &Lua, roll_id: i64, roll_type: i64, confirmed: bool) {
    let mut model = crate::script::model_mut_of(lua);
    let Some(entry) = find(&model, roll_id) else {
        return;
    };
    let (Ok(id), Ok(kind)) = (u32::try_from(roll_id), u8::try_from(roll_type)) else {
        return;
    };
    if roll_type != 0 && !confirmed {
        if let Some(quality) = entry.quality {
            let asks = if roll_type == DISENCHANT {
                quality >= DISENCHANT_CONFIRM_QUALITY
            } else {
                entry.bind_on_pickup
            };
            if asks {
                model.loot_roll_confirms.push((id, kind));
                return;
            }
        }
    }
    if 1u32
        .checked_shl(u32::from(kind))
        .is_some_and(|bit| bit & u32::from(entry.vote_mask) != 0)
    {
        model.loot_roll_votes.push((id, kind));
    }
}

/// The two numeric arguments of `RollOnLoot` and `ConfirmLootRoll`, or the usage error.
fn two_numbers(lua: &Lua, a: Value, b: Value, usage: &str) -> mlua::Result<(i64, i64)> {
    match (lua.coerce_number(a)?, lua.coerce_number(b)?) {
        (Some(a), Some(b)) => Ok((a as i64, b as i64)),
        _ => Err(mlua::Error::RuntimeError(usage.into())),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `0x5889d0`: nothing for an unknown roll or an uncached item, else the twelve values.
    g.set(
        "GetLootRollItemInfo",
        lua.create_function(|lua, roll_id: Value| {
            let Some(roll_id) = lua.coerce_number(roll_id)? else {
                return Err(mlua::Error::RuntimeError(
                    "Usage: GetLootRollItemInfo(id)".into(),
                ));
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let Some(r) = find(&model, roll_id as i64) else {
                return Ok(mlua::MultiValue::new());
            };
            let (Some(name), Some(quality)) = (r.name.clone(), r.quality) else {
                return Ok(mlua::MultiValue::new());
            };
            let flag = |on: bool| if on { Value::Integer(1) } else { Value::Nil };
            let mask = r.vote_mask;
            lua.pack_multi((
                r.texture.clone(),
                name,
                r.quantity,
                quality,
                flag(r.bind_on_pickup),
                flag(mask & 0x2 != 0),
                flag(mask & 0x4 != 0),
                flag(mask & 0x8 != 0),
                r.reasons[0],
                r.reasons[1],
                r.reasons[2],
                r.disenchant_skill,
            ))
        })?,
    )?;

    // `nil` for an unknown id or while the item template is in flight.
    g.set(
        "GetLootRollItemLink",
        lua.create_function(|lua, roll_id: i64| {
            let link = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                find(&model, roll_id).and_then(|r| r.link.clone())
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // 0 for an unknown id; the stock `OnUpdate` clamps it to the bar's minimum anyway.
    g.set(
        "GetLootRollTimeLeft",
        lua.create_function(|lua, roll_id: i64| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(find(&model, roll_id).map_or(0, |r| r.time_left_ms))
        })?,
    )?;

    // `0x588d60`: the vote, gated as the module's `roll` describes.
    g.set(
        "RollOnLoot",
        lua.create_function(|lua, (id, kind): (Value, Value)| {
            let (id, kind) = two_numbers(lua, id, kind, "Usage: RollOnLoot(id, rollType)")?;
            roll(lua, id, kind, false);
            Ok(())
        })?,
    )?;

    // `0x588dd0`: the same vote, past the confirm.
    g.set(
        "ConfirmLootRoll",
        lua.create_function(|lua, (id, kind): (Value, Value)| {
            let (id, kind) = two_numbers(lua, id, kind, "Usage: ConfirmLootRoll(id, rollType)")?;
            roll(lua, id, kind, true);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LootRollEntry, LootRollsState};
    use crate::script::UiScript;

    /// Roll 7: a resolved Epic bind-on-pickup with every vote allowed. Roll 8: a resolved Rare
    /// that may not be needed or disenchanted. Roll 9: no template yet.
    fn rolls() -> LootRollsState {
        LootRollsState {
            rolls: vec![
                LootRollEntry {
                    roll_id: 7,
                    name: Some("Staff of Jordan".into()),
                    texture: Some("Interface\\Icons\\INV_Staff_12".into()),
                    quantity: 1,
                    quality: Some(4),
                    bind_on_pickup: true,
                    vote_mask: 0x0F,
                    reasons: [0, 0, 0],
                    disenchant_skill: 225,
                    time_left_ms: 42_000,
                    item_id: 17182,
                    link: Some("|cffa335ee|Hitem:17182:0:0:0|h[Staff of Jordan]|h|r".into()),
                    random_property_id: 0,
                },
                LootRollEntry {
                    roll_id: 8,
                    name: Some("Orcish Cleaver".into()),
                    texture: Some("Interface\\Icons\\INV_Axe_04".into()),
                    quantity: 2,
                    quality: Some(3),
                    bind_on_pickup: false,
                    vote_mask: 0x05,
                    reasons: [1, 0, 3],
                    disenchant_skill: -1,
                    time_left_ms: 60_000,
                    item_id: 4949,
                    link: None,
                    random_property_id: 0,
                },
                LootRollEntry {
                    roll_id: 9,
                    quantity: 1,
                    vote_mask: 0x0F,
                    time_left_ms: 55_000,
                    item_id: 4306,
                    ..Default::default()
                },
            ],
        }
    }

    /// `0x5889d0`: twelve values for a resolved roll, nothing for an unknown or uncached one.
    #[test]
    fn roll_info_answers_twelve_values_or_nothing() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetLootRollItemInfo(7)").unwrap(), 0);
        s.set_loot_rolls(rolls());
        assert_eq!(s.arity("GetLootRollItemInfo(7)").unwrap(), 12);
        assert!(s
            .eval::<bool>(
                "local t, n, c, q, bop, need, greed, de, rn, rg, rd, skill = GetLootRollItemInfo(7)\n\
                 return t == 'Interface\\\\Icons\\\\INV_Staff_12' and n == 'Staff of Jordan' and c == 1\n\
                 and q == 4 and bop == 1 and need == 1 and greed == 1 and de == 1\n\
                 and rn == 0 and rg == 0 and rd == 0 and skill == 225",
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, n, c, q, bop, need, greed, de, rn, rg, rd, skill = GetLootRollItemInfo(8)\n\
                 return c == 2 and bop == nil and need == nil and greed == 1 and de == nil\n\
                 and rn == 1 and rg == 0 and rd == 3 and skill == -1",
            )
            .unwrap());
        assert_eq!(s.arity("GetLootRollItemInfo(9)").unwrap(), 0, "uncached");
        assert_eq!(s.arity("GetLootRollItemInfo(99)").unwrap(), 0, "unknown");
        assert_eq!(
            s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(),
            42_000
        );
        assert!(s
            .eval::<bool>("return GetLootRollItemLink(8) == nil")
            .unwrap());
    }

    /// `0x6fbc70`: Pass never asks; Need or Greed on bind-on-pickup and Disenchant on rare or better
    /// ask; the vote mask decides what goes out.
    #[test]
    fn roll_on_loot_asks_then_gates_on_the_vote_mask() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        for v in [
            "RollOnLoot(7, 0)",
            "RollOnLoot(7, 1)",
            "RollOnLoot(7, 2)",
            "RollOnLoot(7, 3)",
        ] {
            s.run(v).unwrap();
        }
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 0)]);
        assert_eq!(s.take_loot_roll_confirms(), vec![(7, 1), (7, 2), (7, 3)]);
        s.run("ConfirmLootRoll(7, 3)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(7, 3)], "the popup's accept");

        // Roll 8 (mask 0x05): Need and Disenchant are not allowed, rare Disenchant still asks.
        for v in ["RollOnLoot(8, 1)", "RollOnLoot(8, 2)", "RollOnLoot(8, 3)"] {
            s.run(v).unwrap();
        }
        assert_eq!(s.take_loot_roll_votes(), vec![(8, 2)]);
        assert_eq!(s.take_loot_roll_confirms(), vec![(8, 3)]);
        s.run("ConfirmLootRoll(8, 3)").unwrap();
        assert!(s.take_loot_roll_votes().is_empty(), "the mask refuses it");

        // Uncached: no confirm, straight to the mask.
        s.run("RollOnLoot(9, 1)").unwrap();
        assert_eq!(s.take_loot_roll_votes(), vec![(9, 1)]);
        s.run("RollOnLoot(99, 1)").unwrap();
        s.run("RollOnLoot(7, 9)").unwrap();
        assert!(s.take_loot_roll_votes().is_empty());
        assert!(s.run("RollOnLoot('x')").is_err(), "usage error");
    }

    #[test]
    fn clearing_the_rolls_empties_them() {
        let mut s = UiScript::new().unwrap();
        s.set_loot_rolls(rolls());
        s.set_loot_rolls(LootRollsState::default());
        assert_eq!(s.arity("GetLootRollItemInfo(7)").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetLootRollTimeLeft(7)").unwrap(), 0);
    }
}
