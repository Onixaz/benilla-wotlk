//! The player's item search (`0x7546f0`) and the verbs that name an item: by id, by name or by link,
//! walking the backpack, the bags (each bag before its contents), the keyring, the bank and the
//! currency slots, then the equipment.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, ftol, number_arg};
use super::cursor::{item_link_name, EQUIPMENT_BAG};
use super::model::Model;
use super::ScriptCall;

/// `BAG_SEARCH` flags: where a search looks.
pub(super) const IN_EQUIPMENT: u32 = 1;
const IN_BAGS: u32 = 2;
const IN_BACKPACK: u32 = 4;
const IN_BANK: u32 = 8;
const IN_KEYRING: u32 = 0x40;
const IGNORE_STATE: u32 = 0x80;
const IN_CURRENCY: u32 = 0x200;
/// The bags and backpack alone, the equip verbs' first pass.
pub(super) const CARRIED: u32 = IN_BAGS | IN_BACKPACK;
/// What a search without a location reads as (`0x7546f0`).
const DEFAULT_PLACES: u32 = IN_EQUIPMENT | CARRIED | IN_KEYRING | IN_CURRENCY;

/// The Lua ids of the walk's containers.
const BACKPACK: i64 = 0;
const KEYRING: i64 = -2;
const BANK: i64 = -1;
const CURRENCY: i64 = -4;
/// The doll ids of the equipped bags.
const FIRST_BAG_SLOT: usize = 20;

/// One held item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Held {
    /// A Lua container, or [`EQUIPMENT_BAG`] with `slot` the doll id.
    pub(super) bag: i64,
    pub(super) slot: u32,
    pub(super) item_id: u32,
    name: String,
    link: Option<String>,
    count: u32,
    pub(super) locked: bool,
    /// A zero-durability item, or one flagged broken, the state filter drops (`0x7546f0`).
    broken: bool,
}

/// What a verb names an item by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ItemQuery {
    Id(u32),
    /// A prefix of the full name, case-folded (`0x753b20`).
    Prefix(String),
    /// The full name, case-folded (`0x753b70`).
    Name(String),
    /// A link's `(id, enchant, gems, random property, seed)`, its zero fields unread (`0x7545d0`).
    Link([i64; 7]),
}

/// A link's fields, `item:` onward.
fn link_fields(link: &str) -> Option<[i64; 7]> {
    let rest = &link[link.find("item:")? + 5..];
    let mut fields = rest.split(':').map(super::item_link::reference_atoi);
    let mut next = || fields.next().unwrap_or(0);
    let (id, enchant, gems) = (next(), next(), [next(), next(), next()]);
    next();
    Some([id, enchant, gems[0], gems[1], gems[2], next(), next()])
}

impl Held {
    /// The fields of the item's own link.
    pub(super) fn link_fields(&self) -> Option<super::item_link::ItemLinkFields> {
        let link = self.link.as_deref()?;
        Some(super::item_link::ItemLinkFields::parse(link)?.1)
    }
}

/// The stacks of `item_id` a search reads (`0x7546f0`): the bank with `bank`, and a broken item
/// too with `every`.
pub(super) fn held_count(model: &Model, item_id: u32, bank: bool, every: bool) -> u32 {
    let flags = if bank { IN_BANK } else { 0 } | if every { IGNORE_STATE } else { 0 };
    walk(model, flags)
        .iter()
        .filter(|h| h.item_id == item_id)
        .map(|h| h.count)
        .sum()
}

impl ItemQuery {
    /// Lua's item argument: a number is an id, a string with `item:` a link, else `by_name`.
    pub(super) fn of(
        lua: &Lua,
        v: &Value,
        by_name: fn(String) -> Self,
    ) -> mlua::Result<Option<Self>> {
        if let Some(n) = lua.coerce_number(v.clone())? {
            return Ok(Some(Self::Id(ftol(n) as u32)));
        }
        let Some(text) = super::binding_abi::optional_string(lua, v).filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        Ok(Some(match link_fields(&text) {
            Some(fields) => Self::Link(fields),
            None => by_name(text),
        }))
    }

    fn matches(&self, held: &Held) -> bool {
        match self {
            Self::Id(id) => held.item_id == *id,
            Self::Prefix(name) => held.name.to_lowercase().starts_with(&name.to_lowercase()),
            Self::Name(name) => held.name.to_lowercase() == name.to_lowercase(),
            Self::Link(want) => {
                let have = held.link.as_deref().and_then(link_fields);
                want[0] == i64::from(held.item_id)
                    && have.is_some_and(|have| {
                        want.iter()
                            .zip(have)
                            .skip(1)
                            .all(|(&w, h)| w == 0 || w == h)
                    })
            }
        }
    }
}

/// The items in Lua container `bag`, in slot order.
fn contents(model: &Model, bag: i64, out: &mut Vec<Held>) {
    let Some(c) = model.containers.get(&bag) else {
        return;
    };
    for slot in 1..=c.num_slots {
        let Some(s) = c.slots.get(&slot).filter(|s| s.item_id != 0) else {
            continue;
        };
        out.push(Held {
            bag,
            slot,
            item_id: s.item_id,
            name: item_link_name(s.link.as_deref()),
            link: s.link.clone(),
            count: s.count.max(1),
            locked: s.locked,
            broken: s.durability.is_some_and(|(now, _)| now == 0),
        });
    }
}

/// The item in doll id `id`.
fn worn(model: &Model, id: usize, out: &mut Vec<Held>) {
    let Some(Some(s)) = model.inventory_slots.get(id) else {
        return;
    };
    if s.item_id == 0 {
        return;
    }
    let wrapped = s.flags & 8 != 0;
    out.push(Held {
        bag: EQUIPMENT_BAG,
        slot: id as u32,
        item_id: s.item_id,
        name: match s.link.as_deref() {
            Some(link) => item_link_name(Some(link)),
            None => s.name.clone().unwrap_or_default(),
        },
        link: s.link.clone(),
        count: s.count.max(1),
        locked: s.locked,
        broken: s.flags & 0x10 != 0 || (!wrapped && s.durability.is_some_and(|(now, _)| now == 0)),
    });
}

/// Every item a search with `flags` reads, in the walk's order.
fn walk(model: &Model, flags: u32) -> Vec<Held> {
    let flags = if flags & DEFAULT_PLACES == 0 {
        flags | DEFAULT_PLACES
    } else {
        flags
    };
    let mut out = Vec::new();
    if flags & IN_BACKPACK != 0 {
        contents(model, BACKPACK, &mut out);
    }
    if flags & IN_BAGS != 0 {
        for i in 0..4 {
            worn(model, FIRST_BAG_SLOT + i, &mut out);
            contents(model, i as i64 + 1, &mut out);
        }
    }
    if flags & IN_KEYRING != 0 {
        contents(model, KEYRING, &mut out);
    }
    if flags & IN_BANK != 0 {
        contents(model, BANK, &mut out);
        for bag in 5..=11 {
            contents(model, bag, &mut out);
        }
    }
    if flags & IN_CURRENCY != 0 {
        contents(model, CURRENCY, &mut out);
    }
    if flags & IN_EQUIPMENT != 0 {
        for id in 1..=19 {
            worn(model, id, &mut out);
        }
    }
    if flags & IGNORE_STATE == 0 {
        out.retain(|h| !h.broken);
    }
    out
}

/// The first item a search with `flags` finds.
pub(super) fn find_item(model: &Model, query: &ItemQuery, flags: u32) -> Option<Held> {
    walk(model, flags).into_iter().find(|h| query.matches(h))
}

/// An item id by a Lua number, link or name (`0x709de0`): a name a held item or a cached template
/// carries.
fn item_id_of(lua: &Lua, model: &Model, v: &Value) -> mlua::Result<u32> {
    Ok(match ItemQuery::of(lua, v, ItemQuery::Name)? {
        Some(ItemQuery::Id(id)) => id,
        Some(ItemQuery::Link(fields)) => fields[0] as u32,
        Some(ItemQuery::Name(name)) => super::socket::item_id_by_name(model, &name)
            .or_else(|| {
                model
                    .item_templates
                    .iter()
                    .find(|(_, t)| t.name.eq_ignore_ascii_case(&name))
                    .map(|(&id, _)| id)
            })
            .unwrap_or(0),
        _ => 0,
    })
}

/// `PickupItem(item)` (`0x522fb0`): the cursor clears, then holds the id as an action-bar item, no
/// bag slot behind it.
fn pickup_item(model: &mut Model, id: u32) {
    let texture = model.item_templates.get(&id).and_then(|t| t.icon.clone());
    super::cursor::clear_cursor(model);
    if id == 0 {
        return;
    }
    model.cursor = Some(super::cursor::CursorPayload::Action(
        super::cursor::CursorAction {
            src_slot: 0,
            kind: super::action::ACTION_KIND_ITEM,
            action: id,
            texture,
        },
    ));
    super::cursor::queue_cursor_update(model);
}

/// The first on-use spell a template names (`0x706b90`).
fn use_spell(model: &Model, item_id: u32) -> Option<u32> {
    let view = model.item_templates.get(&item_id)?;
    let spells = view.spell_triggers.iter();
    spells
        .filter(|(t, _, _)| *t == 0)
        .map(|(_, s, _)| *s)
        .next()
}

/// The item's use spell, as `GetItemSpell` picks it, and the VM's spell source.
#[allow(clippy::type_complexity)]
fn item_range_spell(
    lua: &Lua,
    item: &Value,
) -> mlua::Result<(
    Option<u32>,
    Option<std::rc::Rc<dyn super::spell_info::SpellInfoSource>>,
)> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let id = item_id_of(lua, &model, item)?;
    let spell = use_spell(&model, id).or_else(|| enchant_use_spell(&model, id));
    Ok((spell, model.spell_info.clone()))
}

/// With no on-use spell, the use spell of an enchant on the worn item (`0x5a86b0`, `0x707c60`).
fn enchant_use_spell(model: &Model, item_id: u32) -> Option<u32> {
    let query = ItemQuery::Id(item_id);
    let item = find_item(model, &query, IN_EQUIPMENT | IGNORE_STATE)?;
    let slot = model.inventory_slots.get(item.slot as usize)?.as_ref()?;
    slot.enchants
        .iter()
        .map(|e| e.use_spell)
        .find(|&spell| spell != 0)
}

/// `EquipItemByName`'s target, 0-based: a number less one, or a slot name's doll id with nothing
/// taken off, one slot higher (`0x51cfb4` against `0x51cf23`), as 12340 runs it.
fn equip_slot(lua: &Lua, model: &Model, slot: &Value) -> mlua::Result<u32> {
    const INVALID: &str = "EquipItemByName(): Invalid inventory dstSlot";
    let dst = match lua.coerce_number(slot.clone())? {
        Some(n) => ftol(n).wrapping_sub(1) as u32,
        None => {
            let name = super::binding_abi::optional_string(lua, slot).unwrap_or_default();
            match super::char_stats::slot_id_by_name(model, &name) {
                Some(id) => id as u32,
                None => return Err(mlua::Error::runtime(INVALID)),
            }
        }
    };
    if dst > 18 {
        return Err(mlua::Error::runtime(INVALID));
    }
    Ok(dst)
}

impl super::UiScript {
    /// Drain the inventory slots `CancelItemTempEnchantment` named.
    pub fn take_temp_enchant_cancels(&mut self) -> Vec<u32> {
        drain_field!(self, temp_enchant_cancels)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // UseItemByName(item [, unit]) (`0x51cd00`): the first match anywhere, at the unit.
    g.set(
        "UseItemByName",
        lua.create_function(|lua, (item, unit): (Value, Value)| {
            let Some(query) = ItemQuery::of(lua, &item, ItemQuery::Name)? else {
                return Ok(());
            };
            let Some(at) = super::calls::cast_at_arg(lua, Some(&unit)) else {
                return Ok(());
            };
            let mut model = crate::script::model_mut_of(lua);
            if model.unit_guids.player == 0 {
                return Ok(());
            }
            if let Some(held) = find_item(&model, &query, 0) {
                model.script_calls.push(ScriptCall::UseItemAt {
                    bag: held.bag,
                    slot: held.slot,
                    at,
                });
            }
            Ok(())
        })?,
    )?;

    // EquipItemByName(item [, slot]) (`0x51cdb0`): a prefix name or link, carried first; a locked
    // item, or one already there, stays.
    g.set(
        "EquipItemByName",
        lua.create_function(|lua, (item, slot): (Value, Value)| {
            let mut model = crate::script::model_mut_of(lua);
            if model.unit_guids.player == 0 {
                return Ok(());
            }
            let dst = match slot {
                Value::Nil | Value::Boolean(_) | Value::Table(_) => None,
                ref slot => Some(equip_slot(lua, &model, slot)?),
            };
            let Some(query) = ItemQuery::of(lua, &item, ItemQuery::Prefix)? else {
                return Ok(());
            };
            let found = find_item(&model, &query, CARRIED).or_else(|| find_item(&model, &query, 0));
            let Some(held) = found.filter(|h| !h.locked) else {
                return Ok(());
            };
            let worn = held.bag == EQUIPMENT_BAG && (1..=19).contains(&held.slot);
            let stays = match dst {
                Some(dst) => held.bag == EQUIPMENT_BAG && held.slot == dst + 1,
                None => worn,
            };
            if !stays {
                model.script_calls.push(ScriptCall::EquipItem {
                    bag: held.bag,
                    slot: held.slot,
                    dst: dst.map(|d| d as u8),
                });
            }
            Ok(())
        })?,
    )?;

    // PickupItem(item) (`0x522fb0`).
    g.set(
        "PickupItem",
        lua.create_function(|lua, item: Value| {
            let id = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                item_id_of(lua, &model, &item)?
            };
            pickup_item(&mut crate::script::model_mut_of(lua), id);
            Ok(())
        })?,
    )?;

    // GetItemSpell(item) (`0x517100`): the on-use spell's name and rank, read off the caches.
    g.set(
        "GetItemSpell",
        lua.create_function(|lua, item: Value| {
            let (spell, source) = {
                let model = lua
                    .app_data_ref::<crate::script::ModelCell>()
                    .expect("model app_data");
                let id = item_id_of(lua, &model, &item)?;
                let spell = use_spell(&model, id).or_else(|| enchant_use_spell(&model, id));
                (spell, model.spell_info.clone())
            };
            let info = spell
                .zip(source)
                .and_then(|(id, s)| s.spell_info(id, false));
            let mut out = vec![Value::Nil, Value::Nil];
            if let Some(info) = info {
                out[0] = Value::String(lua.create_string(&info.name)?);
                out[1] = Value::String(lua.create_string(&info.rank)?);
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;

    // ItemHasRange(item) (`0x51c870`): 1 when the item's use spell has a range, else nil.
    g.set(
        "ItemHasRange",
        lua.create_function(|lua, item: Value| {
            let (spell, source) = item_range_spell(lua, &item)?;
            let has = spell
                .zip(source)
                .is_some_and(|(id, s)| s.spell_has_range(id, false));
            Ok(if has { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    // IsItemInRange(item, unit) (`0x51c9c0`): the use spell's range check: 1, 0 or nil.
    g.set(
        "IsItemInRange",
        lua.create_function(|lua, (item, unit): (Value, Value)| {
            let (spell, source) = item_range_spell(lua, &item)?;
            let target = super::spell_info::range_unit(lua, &unit);
            let near = spell
                .zip(source)
                .zip(target)
                .and_then(|((id, s), guid)| s.spell_in_range(id, false, guid));
            Ok(super::spell_info::range_answer(near))
        })?,
    )?;

    // IsEquippableItem(item) (`0x511150`): a cached template with an inventory type, 1 or nil.
    g.set(
        "IsEquippableItem",
        lua.create_function(|lua, item: Value| {
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            let id = item_id_of(lua, &model, &item)?;
            Ok(flag(
                model
                    .item_templates
                    .get(&id)
                    .is_some_and(|t| t.inventory_type != 0),
            ))
        })?,
    )?;

    // IsEquippedItem(item) (`0x51c690`): 1 or nil, a broken item reading nil.
    g.set(
        "IsEquippedItem",
        lua.create_function(|lua, item: Value| {
            let query = ItemQuery::of(lua, &item, ItemQuery::Name)?;
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(flag(query.is_some_and(|q| {
                find_item(&model, &q, IN_EQUIPMENT).is_some()
            })))
        })?,
    )?;

    // CancelItemTempEnchantment(slot) (`0x8021d0`): 1 the main hand, 2 the off hand, each with a
    // temporary enchant.
    g.set(
        "CancelItemTempEnchantment",
        lua.create_function(|lua, slot: Value| {
            const USAGE: &str = "Usage: CancelItemTempEnchantment(slot)";
            let n = number_arg(lua, slot, USAGE)?;
            if !(1..=2).contains(&n) {
                return Err(mlua::Error::runtime(USAGE));
            }
            let inv_slot = n as u32 - 1 + 15;
            let mut model = crate::script::model_mut_of(lua);
            let enchanted = model
                .inventory_slots
                .get(inv_slot as usize + 1)
                .and_then(Option::as_ref)
                .is_some_and(|s| s.enchants.iter().any(|e| e.slot == 1 && e.id != 0));
            if enchanted {
                model.temp_enchant_cancels.push(inv_slot);
            }
            Ok(())
        })?,
    )?;

    // GetContainerFreeSlots(bag [, table]) (`0x5d7820`): the free slots in order, appended over a
    // given table's front; nothing for the bank with no banker, or a bag not held.
    g.set(
        "GetContainerFreeSlots",
        lua.create_function(|lua, (bag, table): (Value, Value)| {
            const USAGE: &str = "Usage: GetContainerFreeSlots(index [, returnTable])";
            let Some(bag) = lua.coerce_number(bag)? else {
                return Err(mlua::Error::runtime(USAGE));
            };
            let bag = ftol(bag);
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            if model.unit_guids.player == 0
                || !(-1..=11).contains(&bag)
                || (bag == BANK as i32 && model.bank.is_none())
            {
                return Ok(Value::Nil);
            }
            let held = model.containers.get(&i64::from(bag));
            let size = match bag {
                0 => 16,
                -1 => 28,
                _ => match held.map(|c| c.num_slots) {
                    Some(n) if n > 0 => n,
                    _ => return Ok(Value::Nil),
                },
            };
            let out = match table {
                Value::Table(t) => t,
                _ => lua.create_table()?,
            };
            let mut count = 1;
            for slot in 1..=size {
                if !held.is_some_and(|c| c.slots.get(&slot).is_some_and(|s| s.item_id != 0)) {
                    out.raw_set(count, slot)?;
                    count += 1;
                }
            }
            Ok(Value::Table(out))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{ContainerSlot, ContainerState, UiScript, UnitGuids};

    /// A player with `id` in backpack slot 3.
    fn carrying(id: u32) -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_paperdoll_item_frame(crate::script::PaperDollSlotInfo::test_rows());
        s.set_unit_guids(&UnitGuids {
            player: 1,
            ..Default::default()
        });
        let mut model = s.model_mut();
        let mut bag = ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        bag.slots.insert(
            3,
            ContainerSlot {
                item_id: id,
                link: Some("|Hitem:25|h[Worn Sword]|h".into()),
                ..Default::default()
            },
        );
        model.containers.insert(0, bag);
        drop(model);
        s
    }

    fn equips(s: &mut UiScript) -> Vec<ScriptCall> {
        s.take_script_calls()
            .into_iter()
            .filter(|c| matches!(c, ScriptCall::EquipItem { .. }))
            .collect()
    }

    /// `0x51cf23` against `0x51cfb4`: a number takes one off; a name lands a slot higher.
    #[test]
    fn a_slot_name_lands_one_slot_past_its_number() {
        let mut s = carrying(25);
        s.run("EquipItemByName(25, 13)").unwrap();
        let by_number = ScriptCall::EquipItem {
            bag: 0,
            slot: 3,
            dst: Some(12),
        };
        assert_eq!(equips(&mut s), vec![by_number]);
        s.run("EquipItemByName('Worn', 'Trinket0Slot')").unwrap();
        let by_name = ScriptCall::EquipItem {
            bag: 0,
            slot: 3,
            dst: Some(13),
        };
        assert_eq!(equips(&mut s), vec![by_name]);
        assert!(s.run("EquipItemByName(25, 'TabardSlot')").is_err());
        assert!(s.run("EquipItemByName(25, 'NoSuchSlot')").is_err());
    }

    /// `0x5d7820`: the free slots in order, into a given table.
    #[test]
    fn free_slots_skip_what_is_held() {
        let s = carrying(25);
        let (first, third, count): (i32, i32, i32) = s
            .eval("local t = GetContainerFreeSlots(0) return t[1], t[3], #t")
            .unwrap();
        assert_eq!((first, third, count), (1, 4, 15));
        let kept: i32 = s
            .eval("local t = {} GetContainerFreeSlots(0, t) return t[2]")
            .unwrap();
        assert_eq!(kept, 2);
    }

    /// `0x7545d0`: a zero link field is unread; the set ones must match.
    #[test]
    fn a_link_matches_on_its_set_fields() {
        let held = Held {
            bag: 0,
            slot: 1,
            item_id: 25,
            count: 1,
            name: "Worn Sword".into(),
            link: Some("|Hitem:25:7:0:0:0:0:3:9:80|h[Worn Sword]|h".into()),
            locked: false,
            broken: false,
        };
        let q = |link: &str| ItemQuery::Link(link_fields(link).unwrap());
        assert!(q("item:25").matches(&held));
        assert!(q("item:25:7:0:0:0:0:3").matches(&held));
        assert!(!q("item:25:8").matches(&held), "another enchant");
        assert!(!q("item:26").matches(&held), "another item");
        assert!(ItemQuery::Prefix("worn".into()).matches(&held));
        assert!(!ItemQuery::Name("worn".into()).matches(&held));
    }
}
