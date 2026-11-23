//! The group loot roll bindings `GroupLootFrame` calls: each reads the client's roll list at the
//! call through the live view ([`super::LiveState::loot_roll`]); a vote, 0 Pass, 1 Need, 2 Greed,
//! 3 Disenchant, asks first or goes out inside its call, the host sending it and closing the frame
//! ([`super::LootVerb::Roll`]). `START_LOOT_ROLL` carries `(rollID, rollTime)` and
//! `CANCEL_LOOT_ROLL` the `rollID`. The `rollID` is client-internal, as `CMSG_LOOT_ROLL` addresses
//! `(lootedTarget, itemSlot)`, so the app allocates its own.

use mlua::{Lua, Value};

use super::Model;

/// `rollType` 3, whose confirm is `CONFIRM_DISENCHANT_ROLL` on a rare or better item (`0x6fbce2`).
const DISENCHANT: i32 = 3;

/// The quality from which a Disenchant vote asks first (`0x6fbc70`: `quality >= 3`).
const DISENCHANT_CONFIRM_QUALITY: u32 = 3;

/// One group loot roll on the client's list, resolved at the call.
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
    /// Our vote went out (`+0x42`): the roll stays listed until it resolves, but takes no vote.
    pub finished: bool,
}

/// A fixture's open rolls, in the order they opened, read where no view is lent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LootRollsState {
    pub rolls: Vec<LootRollEntry>,
}

impl super::UiScript {
    /// A test's or a capture fixture's open rolls, read where no view is lent.
    #[cfg(any(test, feature = "fixtures"))]
    pub fn set_loot_rolls(&mut self, state: LootRollsState) {
        set_field!(self, loot_rolls, state);
    }
}

/// The roll `roll_id` on the list at the call (`0x6fb150`); with no view lent, a fixture's.
pub(crate) fn find(model: &Model, roll_id: i64) -> Option<LootRollEntry> {
    if let Some(entry) = super::read_live(|l| l.loot_roll(roll_id)).flatten() {
        return entry;
    }
    #[cfg(any(test, feature = "fixtures"))]
    {
        model
            .loot_rolls
            .rolls
            .iter()
            .find(|r| i64::from(r.roll_id) == roll_id)
            .cloned()
    }
    #[cfg(not(any(test, feature = "fixtures")))]
    {
        let _ = model;
        None
    }
}

/// `0x6fbc70`, inside its call: a roll our vote finished takes none (`0x6fbcab`); a vote other
/// than Pass, unconfirmed, on a cached record asks first, Disenchant on rare or better with
/// `CONFIRM_DISENCHANT_ROLL (id, 3)` (`0x6fbcfb`), any other on bind on pickup with
/// `CONFIRM_LOOT_ROLL (id, rollType)` (`0x6fbd22`); else a vote its mask's byte allows
/// (`0x6fbd38`) goes out through the host.
fn roll(lua: &Lua, roll_id: i64, roll_type: i64, confirmed: bool) {
    let entry = {
        let model = crate::script::model_mut_of(lua);
        find(&model, roll_id)
    };
    let Some(entry) = entry.filter(|e| !e.finished) else {
        return;
    };
    // The binding's integer conversion (`0x588d9d`).
    let roll_type = roll_type as i32;
    let id = i64::from(entry.roll_id);
    if roll_type != 0 && !confirmed {
        if let Some(quality) = entry.quality {
            let ask = if roll_type == DISENCHANT {
                (quality >= DISENCHANT_CONFIRM_QUALITY).then_some("CONFIRM_DISENCHANT_ROLL")
            } else {
                entry.bind_on_pickup.then_some("CONFIRM_LOOT_ROLL")
            };
            if let Some(event) = ask {
                let args = vec![
                    super::ScriptValue::Int(id),
                    super::ScriptValue::Int(i64::from(roll_type)),
                ];
                super::tick::fire_event_into(lua, event, args);
                return;
            }
        }
    }
    // `shl` takes the count's low five bits; the test reads the mask's byte.
    let bit = 1u32.wrapping_shl(roll_type as u32) as u8;
    if bit & entry.vote_mask != 0 {
        let verb = super::LootVerb::Roll {
            roll_id: entry.roll_id,
            roll_type: roll_type as u8,
        };
        super::calls::make(lua, super::ScriptCall::Loot(verb));
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
            let model = crate::script::model_mut_of(lua);
            let Some(r) = find(&model, roll_id as i64) else {
                return Ok(mlua::MultiValue::new());
            };
            drop(model);
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
                let model = crate::script::model_mut_of(lua);
                find(&model, roll_id).and_then(|r| r.link)
            };
            match link {
                Some(link) => Ok(Value::String(lua.create_string(&link)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // `0x588cc0`: the roll's end less the clock at the call, unsigned, so an expired roll reads
    // near 2^32 (`0x588d33`); 0 for an unknown id.
    g.set(
        "GetLootRollTimeLeft",
        lua.create_function(|lua, roll_id: i64| {
            let model = crate::script::model_mut_of(lua);
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
                    finished: false,
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
                    finished: false,
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

    /// The votes the calls left waiting for a host, as `(rollID, rollType)`.
    fn votes(s: &mut UiScript) -> Vec<(u32, u8)> {
        s.take_loot_verbs()
            .into_iter()
            .filter_map(|v| match v {
                crate::script::LootVerb::Roll { roll_id, roll_type } => Some((roll_id, roll_type)),
                _ => None,
            })
            .collect()
    }

    /// The confirms raised inside the calls since the last read, as `event:rollID:rollType`.
    fn asks(s: &UiScript) -> Vec<String> {
        s.eval("local t = ASKS ASKS = {} return t").unwrap()
    }

    /// `0x6fbc70`: Pass never asks; Need or Greed on bind-on-pickup and Disenchant on rare or better
    /// raise their confirm inside the call; the vote mask's byte decides what goes out.
    #[test]
    fn roll_on_loot_asks_then_gates_on_the_vote_mask() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "ASKS = {} local f = CreateFrame('Frame') f:RegisterEvent('CONFIRM_LOOT_ROLL')              f:RegisterEvent('CONFIRM_DISENCHANT_ROLL') f:SetScript('OnEvent',              function(_, e, id, t) ASKS[#ASKS + 1] = e .. ':' .. id .. ':' .. t end)",
        )
        .unwrap();
        s.set_loot_rolls(rolls());
        for v in [
            "RollOnLoot(7, 0)",
            "RollOnLoot(7, 1)",
            "RollOnLoot(7, 2)",
            "RollOnLoot(7, 3)",
        ] {
            s.run(v).unwrap();
        }
        assert_eq!(votes(&mut s), vec![(7, 0)]);
        assert_eq!(
            asks(&s),
            [
                "CONFIRM_LOOT_ROLL:7:1",
                "CONFIRM_LOOT_ROLL:7:2",
                "CONFIRM_DISENCHANT_ROLL:7:3"
            ]
        );
        s.run("ConfirmLootRoll(7, 3)").unwrap();
        assert_eq!(votes(&mut s), vec![(7, 3)], "the popup's accept");

        // Roll 8 (mask 0x05): Need and Disenchant are not allowed, rare Disenchant still asks.
        for v in ["RollOnLoot(8, 1)", "RollOnLoot(8, 2)", "RollOnLoot(8, 3)"] {
            s.run(v).unwrap();
        }
        assert_eq!(votes(&mut s), vec![(8, 2)]);
        assert_eq!(asks(&s), ["CONFIRM_DISENCHANT_ROLL:8:3"]);
        s.run("ConfirmLootRoll(8, 3)").unwrap();
        assert!(votes(&mut s).is_empty(), "the mask refuses it");

        // Uncached: no confirm, straight to the mask.
        s.run("RollOnLoot(9, 1)").unwrap();
        assert_eq!(votes(&mut s), vec![(9, 1)]);
        s.run("RollOnLoot(99, 1)").unwrap();
        assert!(
            votes(&mut s).is_empty() && asks(&s).is_empty(),
            "no such roll"
        );
        // Any other type on bind on pickup asks with it (`0x6fbd0a`); unconfirmed it never sends.
        s.run("RollOnLoot(7, 9)").unwrap();
        assert_eq!(asks(&s), ["CONFIRM_LOOT_ROLL:7:9"]);
        // Confirmed, the shift takes the count's low five bits: 33 tests bit 1 (`0x6fbd38`).
        s.run("ConfirmLootRoll(7, 33)").unwrap();
        assert_eq!(votes(&mut s), vec![(7, 33)]);
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
